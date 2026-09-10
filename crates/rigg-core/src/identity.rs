//! The identity graph (spec §3): which service-identity roles does this
//! configuration require, on which scopes, and what settings must hold?
//!
//! Edges are derived from the documents themselves — every infrastructure
//! reference the registry knows about ([`crate::infra::extract`]) — and
//! scoped through the environment's bindings ([`EnvBindings`]), so a scope is
//! an ARM id whenever the binding resolves and an explicit "unbound" marker
//! when it does not. `rigg auth doctor` verifies and repairs them; `rigg push`
//! runs the same graph as a preflight over its plan.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use crate::binding::{BindingEntry, BindingType, EnvBindings, Wanted};
use crate::infra::{self, FoundRef, Target};
use crate::registry::InfraForm;
use crate::resources::ResourceKind;

/// Built-in Azure role definitions, by GUID (spec §3.2).
pub mod roles {
    /// A role definition: its built-in GUID and its display name.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
    pub struct Role {
        pub id: &'static str,
        pub name: &'static str,
    }

    const fn role(id: &'static str, name: &'static str) -> Role {
        Role { id, name }
    }

    pub const STORAGE_BLOB_DATA_READER: Role = role(
        "2a2b9908-6ea1-4ae2-8e65-a410df84e7d1",
        "Storage Blob Data Reader",
    );
    pub const STORAGE_BLOB_DATA_CONTRIBUTOR: Role = role(
        "ba92f5b4-2d11-453d-a403-e96b0029c9fe",
        "Storage Blob Data Contributor",
    );
    pub const STORAGE_TABLE_DATA_CONTRIBUTOR: Role = role(
        "0a9a7e1f-b9d0-4cc4-a60d-0319b160aaa3",
        "Storage Table Data Contributor",
    );
    pub const READER_AND_DATA_ACCESS: Role = role(
        "c12c1c16-33a1-487b-954d-41c89c60f349",
        "Reader and Data Access",
    );
    pub const COGNITIVE_SERVICES_OPENAI_USER: Role = role(
        "5e0bd9bd-7b93-4f28-af87-19fc36ad61bd",
        "Cognitive Services OpenAI User",
    );
    pub const COGNITIVE_SERVICES_USER: Role = role(
        "a97b65f3-24c7-4388-baec-2e87135dc908",
        "Cognitive Services User",
    );
    pub const COGNITIVE_SERVICES_CONTRIBUTOR: Role = role(
        "25fbc0a9-bd7c-42a3-aa1a-3b75d497ee68",
        "Cognitive Services Contributor",
    );
    pub const SEARCH_INDEX_DATA_READER: Role = role(
        "1407120a-92aa-4202-b7e9-c0e197c71c8f",
        "Search Index Data Reader",
    );
    pub const SEARCH_INDEX_DATA_CONTRIBUTOR: Role = role(
        "8ebe5a00-799e-43f5-93ac-243d3dce84a7",
        "Search Index Data Contributor",
    );
    pub const SEARCH_SERVICE_CONTRIBUTOR: Role = role(
        "7ca78c08-252a-4471-8644-bb5ff32d4ba0",
        "Search Service Contributor",
    );
    pub const KEY_VAULT_CRYPTO_SERVICE_ENCRYPTION_USER: Role = role(
        "e147488a-f6f5-4113-8e2d-b22465e65bf6",
        "Key Vault Crypto Service Encryption User",
    );
    pub const KEY_VAULT_CRYPTO_USER: Role = role(
        "12338af0-0e69-4776-bea7-57ae8d297424",
        "Key Vault Crypto User",
    );
    pub const KEY_VAULT_SECRETS_USER: Role = role(
        "4633458b-17de-408a-b874-0445c86b69e6",
        "Key Vault Secrets User",
    );
    pub const FOUNDRY_USER: Role = role("53ca6127-db72-4b80-b1b0-d745d6d5456d", "Azure AI User");
    pub const FOUNDRY_PROJECT_MANAGER: Role = role(
        "eadc314b-1a2d-4efa-be10-5d325db5065e",
        "Azure AI Project Manager",
    );
    pub const FOUNDRY_ACCOUNT_OWNER: Role = role(
        "e47c6f54-e4a2-4754-9501-8e0985b135e1",
        "Azure AI Account Owner",
    );

    /// Not an ARM role: the marker carried by [`super::EdgeKind::AppAuthorization`]
    /// edges, where authorization lives in an Entra app registration (Easy
    /// Auth) rather than in a role assignment.
    pub const APP_AUTHORIZATION: Role = role("app-authorization", "app authorization (Entra)");
}

/// Whose identity must hold the role (spec §3.1).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Principal {
    /// The search service's system-assigned managed identity.
    SearchSystem,
    /// A user-assigned managed identity named by an `identity`/`authIdentity`
    /// field; `binding` is the environment binding that owns it (or the bare
    /// physical name when nothing binds it).
    SearchUser { binding: String },
    /// The Foundry project's system-assigned managed identity.
    FoundryProject,
    /// The caller (az login user, or the service principal in CI).
    Operator,
    /// Any named principal (`--principal`), e.g. a CI identity.
    Named { object_id: String },
}

impl std::fmt::Display for Principal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Principal::SearchSystem => write!(f, "search-system"),
            Principal::SearchUser { binding } => write!(f, "search-user:{binding}"),
            Principal::FoundryProject => write!(f, "foundry-project"),
            Principal::Operator => write!(f, "operator"),
            Principal::Named { object_id } => write!(f, "principal:{object_id}"),
        }
    }
}

impl Serialize for Principal {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// Where a role must be assigned: an ARM id when the binding resolves, or an
/// explicit "not bound / not resolved yet" marker so callers can say exactly
/// what is missing instead of dropping the requirement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Resolved(String),
    #[serde(rename_all = "camelCase")]
    Unresolved {
        /// The binding that names this resource — empty when nothing in the
        /// environment binds it at all.
        binding: String,
        kind: Option<BindingType>,
        physical: String,
    },
}

impl Scope {
    /// The ARM id, when this scope resolved to one.
    pub fn arm_id(&self) -> Option<&str> {
        match self {
            Scope::Resolved(id) => Some(id.as_str()),
            Scope::Unresolved { .. } => None,
        }
    }

    /// Stable key for edge/check identity and de-duplication.
    pub fn key(&self) -> String {
        match self {
            Scope::Resolved(id) => id.clone(),
            Scope::Unresolved {
                binding, physical, ..
            } => format!("unresolved:{binding}:{physical}"),
        }
    }

    /// Human phrasing for reports.
    pub fn describe(&self) -> String {
        match self {
            Scope::Resolved(id) => id.clone(),
            Scope::Unresolved {
                binding,
                kind,
                physical,
            } if binding.is_empty() => match kind {
                Some(k) => format!("'{physical}' (no {k} binding in this environment)"),
                None => format!("'{physical}' (not bound in this environment)"),
            },
            Scope::Unresolved {
                binding, physical, ..
            } => format!("binding '{binding}' ({physical}, not resolved yet)"),
        }
    }
}

/// A requirement that is not a role assignment but still gates the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Constraint {
    /// The referenced Cognitive Services account must be of kind `AIServices`.
    AiServicesKindRequired,
    /// Storage's trusted-services exception works only with the search
    /// service's system-assigned identity — a UAMI cannot use it.
    TrustedServiceNeedsSystemIdentity,
    /// The feature this edge comes from exists on the preview channel only.
    PreviewOnly(&'static str),
}

/// The file evidence for an edge or check: which document, and where in it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Source {
    pub kind: ResourceKind,
    pub name: String,
    /// Concrete (indexed) document path, e.g. `skills[2].uri`.
    pub path: String,
}

/// How an edge is verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// An ARM role assignment — verifiable and fixable.
    Rbac,
    /// Entra app authorization (Easy Auth audience) rather than ARM RBAC.
    AppAuthorization,
    /// Reported with guidance only.
    Informational,
}

/// One identity requirement: principal, role, scope, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Edge {
    /// Stable id: `"<principal>|<role id>|<scope key>"`.
    pub id: String,
    pub principal: Principal,
    pub role: roles::Role,
    /// Roles that satisfy this edge instead of `role` (operator edges only).
    pub alternatives: Vec<roles::Role>,
    pub scope: Scope,
    pub reason: String,
    pub sources: Vec<Source>,
    pub constraints: Vec<Constraint>,
    pub kind: EdgeKind,
}

impl Edge {
    fn new(principal: Principal, role: roles::Role, scope: Scope, kind: EdgeKind) -> Edge {
        let id = format!("{principal}|{}|{}", role.id, scope.key());
        Edge {
            id,
            principal,
            role,
            alternatives: Vec::new(),
            scope,
            reason: String::new(),
            sources: Vec::new(),
            constraints: Vec::new(),
            kind,
        }
    }

    fn rbac(principal: Principal, role: roles::Role, scope: Scope) -> Edge {
        Edge::new(principal, role, scope, EdgeKind::Rbac)
    }

    fn because(mut self, reason: impl Into<String>) -> Edge {
        self.reason = reason.into();
        self
    }

    fn evidence(mut self, source: Source) -> Edge {
        self.sources.push(source);
        self
    }

    fn constrained_by(mut self, constraint: Constraint) -> Edge {
        if !self.constraints.contains(&constraint) {
            self.constraints.push(constraint);
        }
        self
    }

    fn or_role(mut self, role: roles::Role) -> Edge {
        self.alternatives.push(role);
        self
    }
}

/// A setting or network condition that must hold (spec §3.3), plus the
/// operator's own ability to grant a role (spec §3.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "check")]
pub enum CheckKind {
    /// Free SKU has no managed identity; knowledge bases need Basic+.
    SearchSku,
    /// A system-assigned identity (or the referenced UAMI) must exist.
    SearchIdentity,
    /// The service must accept bearer tokens (`authOptions.aadOrApiKey` or
    /// `disableLocalAuth`).
    SearchRbacEnabled,
    StorageNetwork {
        account: Scope,
    },
    StorageSoftDelete {
        account: Scope,
    },
    StorageSharedKey {
        account: Scope,
    },
    AiServicesKind {
        account: Scope,
    },
    FunctionAppNetwork {
        site: Scope,
    },
    EasyAuth {
        site: Scope,
        audience: String,
    },
    DeploymentAvailability {
        stem: String,
    },
    /// The operator can create a role assignment at this scope
    /// (`Microsoft.Authorization/roleAssignments/write`).
    CanGrant {
        scope: Scope,
    },
}

impl CheckKind {
    fn id(&self) -> String {
        match self {
            CheckKind::SearchSku => "search-sku".into(),
            CheckKind::SearchIdentity => "search-identity".into(),
            CheckKind::SearchRbacEnabled => "search-rbac-enabled".into(),
            CheckKind::StorageNetwork { account } => format!("storage-network:{}", account.key()),
            CheckKind::StorageSoftDelete { account } => {
                format!("storage-soft-delete:{}", account.key())
            }
            CheckKind::StorageSharedKey { account } => {
                format!("storage-shared-key:{}", account.key())
            }
            CheckKind::AiServicesKind { account } => format!("ai-services-kind:{}", account.key()),
            CheckKind::FunctionAppNetwork { site } => {
                format!("function-app-network:{}", site.key())
            }
            CheckKind::EasyAuth { site, audience } => {
                format!("easy-auth:{}:{audience}", site.key())
            }
            CheckKind::DeploymentAvailability { stem } => format!("deployment-availability:{stem}"),
            CheckKind::CanGrant { scope } => format!("can-grant:{}", scope.key()),
        }
    }
}

/// One setting/network/permission requirement, with its file evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: String,
    pub kind: CheckKind,
    pub reason: String,
    pub sources: Vec<Source>,
}

impl Check {
    fn new(kind: CheckKind, reason: impl Into<String>) -> Check {
        Check {
            id: kind.id(),
            kind,
            reason: reason.into(),
            sources: Vec::new(),
        }
    }

    fn evidence(mut self, source: Source) -> Check {
        self.sources.push(source);
        self
    }
}

/// The identity graph for a set of documents: service-identity edges, the
/// settings they depend on, and (when computed) the operator's own edges.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Graph {
    pub edges: Vec<Edge>,
    pub checks: Vec<Check>,
    pub operator: Vec<Edge>,
}

// ---------------------------------------------------------------------
// graph construction
// ---------------------------------------------------------------------

/// Accumulates edges and checks, de-duplicating by id and merging evidence.
struct Builder<'a> {
    env: &'a EnvBindings,
    edges: Vec<Edge>,
    edge_ix: BTreeMap<String, usize>,
    checks: Vec<Check>,
    check_ix: BTreeMap<String, usize>,
}

impl<'a> Builder<'a> {
    fn new(env: &'a EnvBindings) -> Builder<'a> {
        Builder {
            env,
            edges: Vec::new(),
            edge_ix: BTreeMap::new(),
            checks: Vec::new(),
            check_ix: BTreeMap::new(),
        }
    }

    fn edge(&mut self, edge: Edge) {
        match self.edge_ix.get(&edge.id) {
            Some(&i) => {
                let existing = &mut self.edges[i];
                for s in edge.sources {
                    if !existing.sources.contains(&s) {
                        existing.sources.push(s);
                    }
                }
                for c in edge.constraints {
                    if !existing.constraints.contains(&c) {
                        existing.constraints.push(c);
                    }
                }
            }
            None => {
                self.edge_ix.insert(edge.id.clone(), self.edges.len());
                self.edges.push(edge);
            }
        }
    }

    fn check(&mut self, check: Check) {
        match self.check_ix.get(&check.id) {
            Some(&i) => {
                let existing = &mut self.checks[i];
                for s in check.sources {
                    if !existing.sources.contains(&s) {
                        existing.sources.push(s);
                    }
                }
            }
            None => {
                self.check_ix.insert(check.id.clone(), self.checks.len());
                self.checks.push(check);
            }
        }
    }
}

/// One document under inspection, with its infrastructure references
/// already extracted at concrete paths.
struct Doc<'a> {
    kind: ResourceKind,
    name: &'a str,
    value: &'a Value,
    refs: Vec<FoundRef>,
}

impl Doc<'_> {
    fn source(&self, path: &str) -> Source {
        Source {
            kind: self.kind,
            name: self.name.to_string(),
            path: path.to_string(),
        }
    }

    fn label(&self) -> String {
        format!("{} '{}'", self.kind.display_name(), self.name)
    }

    /// The reference at exactly `path`, if the document has one there.
    fn at(&self, path: &str) -> Option<&FoundRef> {
        self.refs.iter().find(|r| r.path == path)
    }

    fn of_form(&self, form: InfraForm) -> impl Iterator<Item = &FoundRef> {
        self.refs.iter().filter(move |r| r.form == form)
    }

    /// The value at a concrete (indexed) path, e.g. `skills[2].authResourceId`.
    /// Segments are split on `.`, so keys that contain one (`@odata.type`)
    /// must be read from the value this returns rather than addressed here.
    fn value_at(&self, path: &str) -> Option<&Value> {
        let mut cur = self.value;
        for raw in path.split('.') {
            let (key, index) = match raw.split_once('[') {
                Some((key, rest)) => (key, rest.trim_end_matches(']').parse::<usize>().ok()),
                None => (raw, None),
            };
            cur = cur.get(key)?;
            if let Some(i) = index {
                cur = cur.get(i)?;
            }
        }
        Some(cur)
    }
}

/// `a.b.resourceUri` → `a.b.<leaf>` — the sibling field of an infra
/// reference (its `authIdentity`/`identity` companion).
fn sibling(path: &str, leaf: &str) -> String {
    match path.rfind('.') {
        Some(i) => format!("{}.{leaf}", &path[..i]),
        None => leaf.to_string(),
    }
}

/// Which principal an `identity`/`authIdentity` reference names: the binding
/// that owns the user-assigned identity, or the search service's
/// system-assigned identity when there is no such reference.
fn principal_for(env: &EnvBindings, identity: Option<&FoundRef>) -> Principal {
    match identity {
        Some(found) => {
            let physical = &found.physical.physical;
            let binding = env
                .find_physical(Wanted::Type(BindingType::Identity), physical)
                .map(|e| e.name.clone())
                .unwrap_or_else(|| physical.clone());
            Principal::SearchUser { binding }
        }
        None => Principal::SearchSystem,
    }
}

/// The scope a reference to `physical` resolves to, through the
/// environment's bindings.
fn scope_for(env: &EnvBindings, target: Target, physical: &str) -> Scope {
    let entry = env.find_physical(infra::wanted_for(target), physical);
    scope_of(entry, infra::binding_type_for(target), physical)
}

fn scope_of(entry: Option<&BindingEntry>, kind: Option<BindingType>, physical: &str) -> Scope {
    let arm_id = entry.and_then(|e| {
        e.resolved
            .as_ref()
            .and_then(|r| r.arm_id.clone())
            .or_else(|| {
                e.declared
                    .as_ref()
                    .and_then(|b| b.arm_id().map(String::from))
            })
    });
    match arm_id {
        Some(id) => Scope::Resolved(id),
        None => Scope::Unresolved {
            binding: entry.map(|e| e.name.clone()).unwrap_or_default(),
            kind,
            physical: physical.to_string(),
        },
    }
}

/// The environment's implicit `search` target as a scope.
fn search_scope(env: &EnvBindings) -> Scope {
    let entry = env.get("search");
    let physical = entry.map(|e| e.physical_name.clone()).unwrap_or_default();
    scope_of(entry, None, &physical)
}

/// The environment's implicit `foundry` account as a scope.
fn foundry_scope(env: &EnvBindings) -> Scope {
    let entry = env.get("foundry");
    let physical = entry.map(|e| e.physical_name.clone()).unwrap_or_default();
    scope_of(entry, None, &physical)
}

/// Build the identity graph for `docs` (spec §3.2 and §3.3). `operator` is
/// left empty — operator edges depend on the plan, see [`operator_edges`].
pub fn graph_for_docs(env: &EnvBindings, docs: &[(ResourceKind, String, Value)]) -> Graph {
    let mut b = Builder::new(env);
    for (kind, name, value) in docs {
        let doc = Doc {
            kind: *kind,
            name,
            value,
            refs: infra::extract(*kind, value),
        };
        data_source_storage(&mut b, &doc);
        knowledge_source_storage(&mut b, &doc);
        knowledge_store_storage(&mut b, &doc);
        model_host(&mut b, &doc);
        ai_services(&mut b, &doc);
        web_api_skills(&mut b, &doc);
        agent_connections(&mut b, &doc, docs);
        encryption_keys(&mut b, &doc);
        deployments(&mut b, &doc);
    }
    search_checks(&mut b);
    Graph {
        edges: b.edges,
        checks: b.checks,
        operator: Vec::new(),
    }
}

/// Row 1 — a blob data source reads from its storage account.
fn data_source_storage(b: &mut Builder<'_>, doc: &Doc<'_>) {
    if doc.kind != ResourceKind::DataSource {
        return;
    }
    let Some(found) = doc.at("credentials.connectionString") else {
        return;
    };
    let physical = found.physical.physical.clone();
    let scope = scope_for(b.env, Target::Storage, &physical);
    let principal = principal_for(b.env, doc.at("identity"));
    b.edge(storage_edge(
        principal,
        roles::STORAGE_BLOB_DATA_READER,
        scope.clone(),
        format!(
            "{} reads blobs from storage account '{physical}'",
            doc.label()
        ),
        doc.source(&found.path),
    ));
    storage_checks(b, doc, &scope, &found.path);
    let soft_delete = doc
        .value_at("dataDeletionDetectionPolicy")
        .and_then(|p| p.get("@odata.type"))
        .and_then(Value::as_str)
        .is_some_and(|t| t.ends_with("NativeBlobSoftDeleteDeletionDetectionPolicy"));
    if soft_delete {
        b.check(
            Check::new(
                CheckKind::StorageSoftDelete {
                    account: scope.clone(),
                },
                format!(
                    "{} uses NativeBlobSoftDeleteDeletionDetectionPolicy — blob soft delete must \
                     be enabled on '{physical}' (and blob versioning off)",
                    doc.label()
                ),
            )
            .evidence(doc.source("dataDeletionDetectionPolicy")),
        );
    }
}

/// Row 2 — a blob knowledge source reads its container, and writes its
/// enrichment asset store when it declares one.
fn knowledge_source_storage(b: &mut Builder<'_>, doc: &Doc<'_>) {
    if doc.kind != ResourceKind::KnowledgeSource {
        return;
    }
    let identity = doc.at("azureBlobParameters.ingestionParameters.identity");
    let principal = principal_for(b.env, identity);
    if let Some(found) = doc.at("azureBlobParameters.connectionString") {
        let physical = found.physical.physical.clone();
        let scope = scope_for(b.env, Target::Storage, &physical);
        b.edge(storage_edge(
            principal.clone(),
            roles::STORAGE_BLOB_DATA_READER,
            scope.clone(),
            format!(
                "{} ingests blobs from storage account '{physical}'",
                doc.label()
            ),
            doc.source(&found.path),
        ));
        storage_checks(b, doc, &scope, &found.path);
    }
    if let Some(found) =
        doc.at("azureBlobParameters.ingestionParameters.assetStore.connectionString")
    {
        let physical = found.physical.physical.clone();
        let scope = scope_for(b.env, Target::Storage, &physical);
        b.edge(storage_edge(
            principal,
            roles::STORAGE_BLOB_DATA_CONTRIBUTOR,
            scope.clone(),
            format!(
                "{} writes extracted assets to storage account '{physical}'",
                doc.label()
            ),
            doc.source(&found.path),
        ));
        storage_checks(b, doc, &scope, &found.path);
    }
}

/// Row 3 — a skillset's knowledge store writes projections.
fn knowledge_store_storage(b: &mut Builder<'_>, doc: &Doc<'_>) {
    if doc.kind != ResourceKind::Skillset {
        return;
    }
    let Some(found) = doc.at("knowledgeStore.storageConnectionString") else {
        return;
    };
    let physical = found.physical.physical.clone();
    let scope = scope_for(b.env, Target::Storage, &physical);
    let principal = principal_for(b.env, doc.at("knowledgeStore.identity"));
    b.edge(storage_edge(
        principal.clone(),
        roles::STORAGE_BLOB_DATA_CONTRIBUTOR,
        scope.clone(),
        format!(
            "{}'s knowledge store projects into storage account '{physical}'",
            doc.label()
        ),
        doc.source(&found.path),
    ));
    let has_tables = doc
        .value_at("knowledgeStore.projections")
        .and_then(Value::as_array)
        .is_some_and(|ps| {
            ps.iter().any(|p| {
                p.get("tables")
                    .and_then(Value::as_array)
                    .is_some_and(|t| !t.is_empty())
            })
        });
    if has_tables {
        for role in [
            roles::STORAGE_TABLE_DATA_CONTRIBUTOR,
            roles::READER_AND_DATA_ACCESS,
        ] {
            b.edge(storage_edge(
                principal.clone(),
                role,
                scope.clone(),
                format!(
                    "{}'s knowledge store has table projections on '{physical}'",
                    doc.label()
                ),
                doc.source("knowledgeStore.projections"),
            ));
        }
    }
    storage_checks(b, doc, &scope, &found.path);
}

/// A storage edge, with the trusted-services constraint attached whenever a
/// user-assigned identity is used (the exception works only with the search
/// service's system-assigned identity — spec §3.3).
fn storage_edge(
    principal: Principal,
    role: roles::Role,
    scope: Scope,
    reason: String,
    source: Source,
) -> Edge {
    let user_assigned = matches!(principal, Principal::SearchUser { .. });
    let edge = Edge::rbac(principal, role, scope)
        .because(reason)
        .evidence(source);
    if user_assigned {
        edge.constrained_by(Constraint::TrustedServiceNeedsSystemIdentity)
    } else {
        edge
    }
}

fn storage_checks(b: &mut Builder<'_>, doc: &Doc<'_>, scope: &Scope, path: &str) {
    b.check(
        Check::new(
            CheckKind::StorageNetwork {
                account: scope.clone(),
            },
            format!(
                "{} reaches this storage account — its firewall must admit the search service",
                doc.label()
            ),
        )
        .evidence(doc.source(path)),
    );
    b.check(
        Check::new(
            CheckKind::StorageSharedKey {
                account: scope.clone(),
            },
            format!(
                "{} uses identity-based access — shared-key access may be disabled",
                doc.label()
            ),
        )
        .evidence(doc.source(path)),
    );
}

/// Rows 4 and 5 — model access on the host named by a `resourceUri`.
fn model_host(b: &mut Builder<'_>, doc: &Doc<'_>) {
    let refs: Vec<(String, String)> = doc
        .of_form(InfraForm::OpenAiEndpoint)
        .map(|r| (r.path.clone(), r.physical.physical.clone()))
        .collect();
    for (path, physical) in refs {
        let scope = scope_for(b.env, Target::ModelHost, &physical);
        let principal = principal_for(b.env, doc.at(&sibling(&path, "authIdentity")));
        // Chat completion (knowledge base answer synthesis, knowledge-source
        // verbalization) uses the Cognitive Services User role; embeddings
        // use the narrower Azure OpenAI User role.
        let chat = doc.kind == ResourceKind::KnowledgeBase || path.contains("chatCompletionModel");
        let role = if chat {
            roles::COGNITIVE_SERVICES_USER
        } else {
            roles::COGNITIVE_SERVICES_OPENAI_USER
        };
        let what = if chat {
            "calls a chat-completion model on"
        } else {
            "calls an embedding model on"
        };
        b.edge(
            Edge::rbac(principal, role, scope)
                .because(format!("{} {what} '{physical}'", doc.label()))
                .evidence(doc.source(&path)),
        );
    }
}

/// Row 6 — identity-based AI services enrichment.
fn ai_services(b: &mut Builder<'_>, doc: &Doc<'_>) {
    let refs: Vec<(String, String)> = doc
        .of_form(InfraForm::AiServicesSubdomain)
        .map(|r| (r.path.clone(), r.physical.physical.clone()))
        .collect();
    for (path, physical) in refs {
        let scope = scope_for(b.env, Target::AiServices, &physical);
        let identity = doc
            .at(&sibling(&path, "identity"))
            .or_else(|| doc.at("azureBlobParameters.ingestionParameters.identity"));
        let principal = principal_for(b.env, identity);
        b.edge(
            Edge::rbac(principal, roles::COGNITIVE_SERVICES_USER, scope.clone())
                .because(format!(
                    "{} uses identity-based AI services enrichment on '{physical}'",
                    doc.label()
                ))
                .evidence(doc.source(&path))
                .constrained_by(Constraint::AiServicesKindRequired),
        );
        b.check(
            Check::new(
                CheckKind::AiServicesKind {
                    account: scope.clone(),
                },
                format!("'{physical}' must be a Cognitive Services account of kind 'AIServices'"),
            )
            .evidence(doc.source(&path)),
        );
    }
}

/// Row 7 — a Web API skill authenticating with a managed identity needs the
/// target app to accept its token (Easy Auth), not an ARM role.
fn web_api_skills(b: &mut Builder<'_>, doc: &Doc<'_>) {
    if doc.kind != ResourceKind::Skillset {
        return;
    }
    let refs: Vec<(String, String, Target)> = doc
        .of_form(InfraForm::ApiUri)
        .map(|r| {
            (
                r.path.clone(),
                r.physical.physical.clone(),
                r.physical.target,
            )
        })
        .collect();
    for (path, physical, target) in refs {
        let Some(audience) = doc
            .value_at(&sibling(&path, "authResourceId"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        let scope = scope_for(b.env, target, &physical);
        let principal = principal_for(b.env, doc.at(&sibling(&path, "authIdentity")));
        b.edge(
            Edge::new(
                principal,
                roles::APP_AUTHORIZATION,
                scope.clone(),
                EdgeKind::AppAuthorization,
            )
            .because(format!(
                "{} calls '{physical}' with a managed identity — the app must accept the audience \
                 '{audience}'",
                doc.label()
            ))
            .evidence(doc.source(&path)),
        );
        if target != Target::FunctionApp {
            continue;
        }
        b.check(
            Check::new(
                CheckKind::FunctionAppNetwork {
                    site: scope.clone(),
                },
                format!(
                    "'{physical}' must admit the search service (AzureCognitiveSearch service tag \
                     or the service's IP)"
                ),
            )
            .evidence(doc.source(&path)),
        );
        b.check(
            Check::new(
                CheckKind::EasyAuth {
                    site: scope.clone(),
                    audience: audience.clone(),
                },
                format!(
                    "'{physical}' must have Microsoft Entra authentication enabled with '{audience}' \
                     among its allowed audiences"
                ),
            )
            .evidence(doc.source(&sibling(&path, "authResourceId"))),
        );
    }
}

/// Row 8 — an agent reaching a knowledge base through an MCP connection that
/// authenticates with the Foundry project's own identity.
fn agent_connections(b: &mut Builder<'_>, doc: &Doc<'_>, docs: &[(ResourceKind, String, Value)]) {
    if doc.kind != ResourceKind::Agent {
        return;
    }
    let Some(tools) = doc.value_at("tools").and_then(Value::as_array) else {
        return;
    };
    for (i, tool) in tools.iter().enumerate() {
        let is_mcp = tool
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| t.eq_ignore_ascii_case("mcp"));
        let Some(conn) = tool.get("project_connection_id").and_then(Value::as_str) else {
            continue;
        };
        if !is_mcp || conn.is_empty() {
            continue;
        }
        // The connection must authenticate with the project's identity. When
        // the connection is not among `docs` (a plan-scoped graph), the edge
        // is kept — an unverifiable requirement beats a silently dropped one.
        let declared = docs
            .iter()
            .find(|(k, n, _)| *k == ResourceKind::Connection && n == conn);
        if let Some((_, _, c)) = declared {
            let auth = c
                .get("properties")
                .and_then(|p| p.get("authType"))
                .or_else(|| c.get("authType"))
                .and_then(Value::as_str)
                .unwrap_or("ProjectManagedIdentity");
            if !auth.eq_ignore_ascii_case("ProjectManagedIdentity") {
                continue;
            }
        }
        let scope = search_scope(b.env);
        b.edge(
            Edge::rbac(
                Principal::FoundryProject,
                roles::SEARCH_INDEX_DATA_READER,
                scope,
            )
            .because(format!(
                "{} calls the search service through connection '{conn}' with the project's \
                 managed identity",
                doc.label()
            ))
            .evidence(doc.source(&format!("tools[{i}].project_connection_id"))),
        );
    }
}

/// Row 9 — customer-managed encryption keys without an explicit credential.
fn encryption_keys(b: &mut Builder<'_>, doc: &Doc<'_>) {
    let Some(found) = doc.at("encryptionKey.keyVaultUri") else {
        return;
    };
    let has_credentials = doc
        .value_at("encryptionKey.accessCredentials")
        .is_some_and(|v| !v.is_null());
    if has_credentials {
        return;
    }
    let physical = found.physical.physical.clone();
    let scope = scope_for(b.env, Target::KeyVault, &physical);
    b.edge(
        Edge::rbac(
            Principal::SearchSystem,
            roles::KEY_VAULT_CRYPTO_SERVICE_ENCRYPTION_USER,
            scope,
        )
        .because(format!(
            "{} is encrypted with a customer-managed key in key vault '{physical}'",
            doc.label()
        ))
        .evidence(doc.source(&found.path)),
    );
}

/// Every model deployment must be available (model, version, region, quota).
fn deployments(b: &mut Builder<'_>, doc: &Doc<'_>) {
    if doc.kind != ResourceKind::Deployment {
        return;
    }
    b.check(
        Check::new(
            CheckKind::DeploymentAvailability {
                stem: doc.name.to_string(),
            },
            format!(
                "{} must name a model and version available in the account's region, with quota \
                 headroom for its capacity",
                doc.label()
            ),
        )
        .evidence(doc.source("properties.model")),
    );
}

/// The three checks every environment with a search target carries.
fn search_checks(b: &mut Builder<'_>) {
    if b.env.get("search").is_none() {
        return;
    }
    for (kind, reason) in [
        (
            CheckKind::SearchSku,
            "the Free SKU has no managed identity, and knowledge bases need Basic or higher",
        ),
        (
            CheckKind::SearchIdentity,
            "the search service needs a managed identity to reach anything keylessly",
        ),
        (
            CheckKind::SearchRbacEnabled,
            "the search service must accept bearer tokens (authOptions.aadOrApiKey, or \
             disableLocalAuth)",
        ),
    ] {
        b.check(Check::new(kind, reason));
    }
}

// ---------------------------------------------------------------------
// operator edges
// ---------------------------------------------------------------------

/// The operator's own rights for a plan (spec §3.2): what the caller must
/// hold to apply `kinds_in_plan`, plus — for every edge in `needs_grants` —
/// the ability to create a role assignment at that edge's scope.
///
/// `verify` covers the data-plane reads `rigg push --verify`, `rigg query`
/// and `rigg ask` perform.
pub fn operator_edges(
    env: &EnvBindings,
    kinds_in_plan: &[ResourceKind],
    verify: bool,
    needs_grants: &[&Edge],
) -> (Vec<Edge>, Vec<Check>) {
    let mut edges = Vec::new();
    let has = |k: ResourceKind| kinds_in_plan.contains(&k);
    let any_search = kinds_in_plan
        .iter()
        .any(|k| k.domain() == crate::service::ServiceDomain::Search);

    if any_search {
        edges.push(
            Edge::rbac(
                Principal::Operator,
                roles::SEARCH_SERVICE_CONTRIBUTOR,
                search_scope(env),
            )
            .because("create and update Azure AI Search resources (and run/reset indexers)"),
        );
    }
    if verify {
        edges.push(
            Edge::rbac(
                Principal::Operator,
                roles::SEARCH_INDEX_DATA_READER,
                search_scope(env),
            )
            .or_role(roles::SEARCH_INDEX_DATA_CONTRIBUTOR)
            .because("query indexes and knowledge bases (rigg query / ask / push --verify)"),
        );
    }
    if has(ResourceKind::Agent) {
        edges.push(
            Edge::rbac(Principal::Operator, roles::FOUNDRY_USER, foundry_scope(env)).because(
                "create and update Foundry agents on the project (Owner/Contributor do not \
                 suffice)",
            ),
        );
    }
    if has(ResourceKind::Connection) {
        edges.push(
            Edge::rbac(
                Principal::Operator,
                roles::FOUNDRY_PROJECT_MANAGER,
                foundry_scope(env),
            )
            .because("create project connections"),
        );
    }
    if has(ResourceKind::Deployment) || has(ResourceKind::Guardrail) {
        edges.push(
            Edge::rbac(
                Principal::Operator,
                roles::FOUNDRY_ACCOUNT_OWNER,
                foundry_scope(env),
            )
            .or_role(roles::COGNITIVE_SERVICES_CONTRIBUTOR)
            .because("create model deployments and content-filter policies on the account"),
        );
    }

    let mut checks: Vec<Check> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for edge in needs_grants {
        if !seen.insert(edge.scope.key()) {
            continue;
        }
        let mut check = Check::new(
            CheckKind::CanGrant {
                scope: edge.scope.clone(),
            },
            format!(
                "granting '{}' here needs Microsoft.Authorization/roleAssignments/write at {}",
                edge.role.name,
                edge.scope.describe()
            ),
        );
        check.sources = edge.sources.clone();
        checks.push(check);
    }
    (edges, checks)
}

// ---------------------------------------------------------------------
// compatibility
// ---------------------------------------------------------------------

/// The identity edges ONE document requires, with no environment bindings —
/// scopes resolve only where the document itself is unambiguous. Used by
/// push to diagnose an RBAC-shaped rejection of that document.
pub fn edges_for(kind: ResourceKind, name: &str, value: &Value) -> Vec<Edge> {
    let env = EnvBindings::of_env("", &crate::workspace::Environment::default(), None);
    graph_for_docs(&env, &[(kind, name.to_string(), value.clone())]).edges
}

/// Parse `ResourceId=/subscriptions/...;<rest>` connection strings.
pub fn parse_resource_id(conn: &str) -> Option<String> {
    let start = conn.find("ResourceId=")? + "ResourceId=".len();
    let rest = &conn[start..];
    let end = rest.find(';').unwrap_or(rest.len());
    let id = rest[..end].trim();
    (id.starts_with("/subscriptions/") && !id.contains('<')).then(|| id.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, BindingCache, ResolvedBinding, TargetKind};
    use crate::workspace::{Environment, FoundryConnection, SearchConnection};
    use serde_json::json;

    const STORAGE_ID: &str =
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct";
    const ASSETS_ID: &str =
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/assets";
    const FOUNDRY_ID: &str =
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.CognitiveServices/accounts/fndr";
    const AISVC_ID: &str =
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.CognitiveServices/accounts/aisvc";
    const SEARCH_ID: &str =
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Search/searchServices/srch";
    const SITE_ID: &str = "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Web/sites/fn";
    const VAULT_ID: &str =
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.KeyVault/vaults/kv";
    const UAMI_ID: &str = "/subscriptions/s/resourceGroups/rg/providers/Microsoft.ManagedIdentity/\
                           userAssignedIdentities/rigg-mi";

    /// An environment whose dependency bindings are declared as full ARM ids
    /// and whose implicit `search`/`foundry` targets come from a resolution
    /// cache — so every scope in these tests resolves.
    fn env() -> EnvBindings {
        let deps = [
            ("docs", BindingType::Storage, STORAGE_ID),
            ("assets", BindingType::Storage, ASSETS_ID),
            ("enrichment", BindingType::AiServices, AISVC_ID),
            ("fn", BindingType::FunctionApp, SITE_ID),
            ("vault", BindingType::KeyVault, VAULT_ID),
            ("mi", BindingType::Identity, UAMI_ID),
        ];
        let environment = Environment {
            search: Some(SearchConnection {
                service: "srch".into(),
                ..Default::default()
            }),
            foundry: Some(FoundryConnection {
                account: "fndr".into(),
                project: "p".into(),
                ..Default::default()
            }),
            dependencies: deps
                .into_iter()
                .map(|(n, kind, value)| {
                    (
                        n.to_string(),
                        Binding {
                            kind,
                            value: value.to_string(),
                        },
                    )
                })
                .collect(),
            ..Default::default()
        };
        let mut cache = BindingCache::default();
        for (name, kind, physical, arm_id) in [
            ("search", TargetKind::Search, "srch", SEARCH_ID),
            ("foundry", TargetKind::Foundry, "fndr", FOUNDRY_ID),
        ] {
            cache.bindings.insert(
                name.to_string(),
                ResolvedBinding {
                    name: name.into(),
                    kind,
                    physical_name: physical.into(),
                    arm_id: Some(arm_id.into()),
                    subscription: Some("s".into()),
                    resource_group: Some("rg".into()),
                    location: None,
                    endpoint: None,
                    principal_id: None,
                    resolved_at: "2026-09-10T00:00:00Z".into(),
                },
            );
        }
        EnvBindings::of_env("dev", &environment, Some(&cache))
    }

    fn graph(docs: &[(ResourceKind, &str, Value)]) -> Graph {
        let owned: Vec<_> = docs
            .iter()
            .map(|(k, n, v)| (*k, n.to_string(), v.clone()))
            .collect();
        graph_for_docs(&env(), &owned)
    }

    fn rbac_edges(g: &Graph) -> Vec<&Edge> {
        g.edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Rbac)
            .collect()
    }

    fn find(g: &Graph, role: roles::Role) -> Vec<&Edge> {
        g.edges.iter().filter(|e| e.role == role).collect()
    }

    fn conn(id: &str) -> String {
        format!("ResourceId={id};")
    }

    fn has_check(g: &Graph, id: &str) -> bool {
        g.checks.iter().any(|c| c.id == id)
    }

    // ---- row 1: data source ----------------------------------------

    #[test]
    fn data_source_storage_edge_uses_the_system_identity_and_the_bound_scope() {
        let g = graph(&[(
            ResourceKind::DataSource,
            "ds",
            json!({
                "name": "ds", "type": "azureblob",
                "credentials": {"connectionString": conn(STORAGE_ID)},
                "container": {"name": "c"}
            }),
        )]);
        let edges = rbac_edges(&g);
        assert_eq!(edges.len(), 1, "{edges:?}");
        let e = edges[0];
        assert_eq!(e.principal, Principal::SearchSystem);
        assert_eq!(e.role, roles::STORAGE_BLOB_DATA_READER);
        assert_eq!(e.scope, Scope::Resolved(STORAGE_ID.into()));
        assert_eq!(
            e.sources,
            vec![Source {
                kind: ResourceKind::DataSource,
                name: "ds".into(),
                path: "credentials.connectionString".into()
            }]
        );
        assert!(e.constraints.is_empty());
        assert_eq!(e.id, format!("search-system|{}|{STORAGE_ID}", e.role.id));
        assert!(has_check(&g, &format!("storage-network:{STORAGE_ID}")));
        assert!(has_check(&g, &format!("storage-shared-key:{STORAGE_ID}")));
        assert!(
            !has_check(&g, &format!("storage-soft-delete:{STORAGE_ID}")),
            "soft delete is only checked when the policy asks for it"
        );
    }

    #[test]
    fn data_source_with_a_user_assigned_identity_names_the_binding_and_the_constraint() {
        let g = graph(&[(
            ResourceKind::DataSource,
            "ds",
            json!({
                "name": "ds", "type": "azureblob",
                "credentials": {"connectionString": conn(STORAGE_ID)},
                "identity": {
                    "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
                    "userAssignedIdentity": UAMI_ID
                },
                "dataDeletionDetectionPolicy": {
                    "@odata.type": "#Microsoft.Azure.Search.NativeBlobSoftDeleteDeletionDetectionPolicy"
                },
                "container": {"name": "c"}
            }),
        )]);
        let e = rbac_edges(&g)[0];
        assert_eq!(
            e.principal,
            Principal::SearchUser {
                binding: "mi".into()
            }
        );
        assert_eq!(
            e.constraints,
            vec![Constraint::TrustedServiceNeedsSystemIdentity]
        );
        assert!(has_check(&g, &format!("storage-soft-delete:{STORAGE_ID}")));
    }

    #[test]
    fn an_unbound_user_assigned_identity_falls_back_to_its_physical_name() {
        let g = graph(&[(
            ResourceKind::DataSource,
            "ds",
            json!({
                "name": "ds", "type": "azureblob",
                "credentials": {"connectionString": conn(STORAGE_ID)},
                "identity": {
                    "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
                    "userAssignedIdentity": "/subscriptions/s/resourceGroups/rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/stray"
                },
                "container": {"name": "c"}
            }),
        )]);
        assert_eq!(
            rbac_edges(&g)[0].principal,
            Principal::SearchUser {
                binding: "stray".into()
            }
        );
    }

    // ---- row 2: knowledge source -----------------------------------

    #[test]
    fn knowledge_source_reads_blobs_and_writes_its_asset_store() {
        let g = graph(&[(
            ResourceKind::KnowledgeSource,
            "ks",
            json!({
                "name": "ks", "kind": "azureBlob",
                "azureBlobParameters": {
                    "connectionString": conn(STORAGE_ID),
                    "containerName": "c",
                    "ingestionParameters": {
                        "assetStore": {"connectionString": conn(ASSETS_ID)}
                    }
                }
            }),
        )]);
        let reader = find(&g, roles::STORAGE_BLOB_DATA_READER);
        assert_eq!(reader.len(), 1);
        assert_eq!(reader[0].scope, Scope::Resolved(STORAGE_ID.into()));
        assert_eq!(reader[0].principal, Principal::SearchSystem);
        let writer = find(&g, roles::STORAGE_BLOB_DATA_CONTRIBUTOR);
        assert_eq!(writer.len(), 1);
        assert_eq!(writer[0].scope, Scope::Resolved(ASSETS_ID.into()));
    }

    // ---- row 3: knowledge store ------------------------------------

    #[test]
    fn knowledge_store_projections_add_table_roles_only_when_tables_exist() {
        let without = graph(&[(
            ResourceKind::Skillset,
            "ss",
            json!({
                "name": "ss", "skills": [],
                "knowledgeStore": {
                    "storageConnectionString": conn(STORAGE_ID),
                    "projections": [{"objects": [{"storageContainer": "o"}]}]
                }
            }),
        )]);
        assert_eq!(rbac_edges(&without).len(), 1);
        assert_eq!(
            rbac_edges(&without)[0].role,
            roles::STORAGE_BLOB_DATA_CONTRIBUTOR
        );

        let with = graph(&[(
            ResourceKind::Skillset,
            "ss",
            json!({
                "name": "ss", "skills": [],
                "knowledgeStore": {
                    "storageConnectionString": conn(STORAGE_ID),
                    "identity": {
                        "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
                        "userAssignedIdentity": UAMI_ID
                    },
                    "projections": [{"tables": [{"tableName": "t"}]}]
                }
            }),
        )]);
        let roles_used: Vec<&str> = rbac_edges(&with).iter().map(|e| e.role.name).collect();
        assert!(
            roles_used.contains(&"Storage Blob Data Contributor"),
            "{roles_used:?}"
        );
        assert!(
            roles_used.contains(&"Storage Table Data Contributor"),
            "{roles_used:?}"
        );
        assert!(
            roles_used.contains(&"Reader and Data Access"),
            "{roles_used:?}"
        );
        assert!(rbac_edges(&with).iter().all(|e| e.principal
            == Principal::SearchUser {
                binding: "mi".into()
            }));
    }

    // ---- row 4: embeddings -----------------------------------------

    #[test]
    fn vectorizers_embedding_skills_and_knowledge_source_embeddings_need_openai_user() {
        let g = graph(&[
            (
                ResourceKind::Index,
                "idx",
                json!({
                    "name": "idx", "fields": [],
                    "vectorSearch": {"vectorizers": [{
                        "name": "v", "kind": "azureOpenAI",
                        "azureOpenAIParameters": {
                            "resourceUri": "https://fndr.openai.azure.com",
                            "deploymentId": "embed"
                        }
                    }]}
                }),
            ),
            (
                ResourceKind::Skillset,
                "ss",
                json!({
                    "name": "ss",
                    "skills": [{
                        "@odata.type": "#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill",
                        "resourceUri": "https://fndr.openai.azure.com",
                        "authIdentity": {
                            "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
                            "userAssignedIdentity": UAMI_ID
                        },
                        "inputs": [], "outputs": []
                    }]
                }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks",
                json!({
                    "name": "ks", "kind": "azureBlob",
                    "azureBlobParameters": {"ingestionParameters": {"embeddingModel": {
                        "azureOpenAIParameters": {"resourceUri": "https://fndr.openai.azure.com"}
                    }}}
                }),
            ),
        ]);
        let openai = find(&g, roles::COGNITIVE_SERVICES_OPENAI_USER);
        assert_eq!(openai.len(), 2, "system + UAMI principals, one edge each");
        assert!(
            openai
                .iter()
                .all(|e| e.scope == Scope::Resolved(FOUNDRY_ID.into()))
        );
        let system = openai
            .iter()
            .find(|e| e.principal == Principal::SearchSystem)
            .expect("system-identity edge");
        assert_eq!(system.sources.len(), 2, "index + knowledge source merged");
        assert!(openai.iter().any(|e| e.principal
            == Principal::SearchUser {
                binding: "mi".into()
            }));
    }

    // ---- row 5: chat completion ------------------------------------

    #[test]
    fn knowledge_base_models_and_chat_completion_need_cognitive_services_user() {
        let g = graph(&[
            (
                ResourceKind::KnowledgeBase,
                "kb",
                json!({
                    "name": "kb", "knowledgeSources": [{"name": "ks"}],
                    "models": [{"kind": "azureOpenAI", "azureOpenAIParameters": {
                        "resourceUri": "https://fndr.openai.azure.com", "deploymentId": "chat"
                    }}]
                }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks",
                json!({
                    "name": "ks", "kind": "azureBlob",
                    "azureBlobParameters": {"ingestionParameters": {"chatCompletionModel": {
                        "azureOpenAIParameters": {"resourceUri": "https://fndr.openai.azure.com"}
                    }}}
                }),
            ),
        ]);
        let cs = find(&g, roles::COGNITIVE_SERVICES_USER);
        assert_eq!(cs.len(), 1, "one principal, one scope, two sources");
        assert_eq!(cs[0].principal, Principal::SearchSystem);
        assert_eq!(cs[0].scope, Scope::Resolved(FOUNDRY_ID.into()));
        assert_eq!(cs[0].sources.len(), 2);
        assert!(find(&g, roles::COGNITIVE_SERVICES_OPENAI_USER).is_empty());
    }

    // ---- row 6: AI services ----------------------------------------

    #[test]
    fn ai_services_by_identity_carries_the_kind_constraint_and_a_check() {
        let g = graph(&[(
            ResourceKind::Skillset,
            "ss",
            json!({
                "name": "ss", "skills": [],
                "cognitiveServices": {
                    "@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity",
                    "subdomainUrl": "https://aisvc.cognitiveservices.azure.com/"
                }
            }),
        )]);
        let e = rbac_edges(&g)[0];
        assert_eq!(e.role, roles::COGNITIVE_SERVICES_USER);
        assert_eq!(e.principal, Principal::SearchSystem);
        assert_eq!(e.scope, Scope::Resolved(AISVC_ID.into()));
        assert_eq!(e.constraints, vec![Constraint::AiServicesKindRequired]);
        assert!(has_check(&g, &format!("ai-services-kind:{AISVC_ID}")));
    }

    // ---- row 7: Web API skills -------------------------------------

    #[test]
    fn web_api_skill_with_auth_resource_id_is_app_authorization_plus_site_checks() {
        let g = graph(&[(
            ResourceKind::Skillset,
            "ss",
            json!({
                "name": "ss",
                "skills": [
                    {"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                     "uri": "https://fn.azurewebsites.net/api/enrich",
                     "authResourceId": "api://abc",
                     "inputs": [], "outputs": []},
                    {"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                     "uri": "https://fn.azurewebsites.net/api/other",
                     "inputs": [], "outputs": []}
                ]
            }),
        )]);
        let app: Vec<&Edge> = g
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::AppAuthorization)
            .collect();
        assert_eq!(app.len(), 1, "only the authResourceId skill yields an edge");
        assert_eq!(app[0].principal, Principal::SearchSystem);
        assert_eq!(app[0].scope, Scope::Resolved(SITE_ID.into()));
        assert!(app[0].reason.contains("api://abc"));
        assert!(has_check(&g, &format!("function-app-network:{SITE_ID}")));
        assert!(has_check(&g, &format!("easy-auth:{SITE_ID}:api://abc")));
    }

    // ---- row 8: agent → connection → search ------------------------

    #[test]
    fn agent_mcp_tool_over_a_project_managed_identity_connection_needs_index_data_reader() {
        let agent = json!({
            "name": "a", "model": "m",
            "tools": [{"type": "mcp", "project_connection_id": "kb-conn"}]
        });
        let g = graph(&[
            (ResourceKind::Agent, "a", agent.clone()),
            (
                ResourceKind::Connection,
                "kb-conn",
                json!({"name": "kb-conn", "properties": {
                    "category": "RemoteTool", "authType": "ProjectManagedIdentity",
                    "target": "https://srch.search.windows.net/knowledgebases/kb/mcp"
                }}),
            ),
        ]);
        let e = find(&g, roles::SEARCH_INDEX_DATA_READER);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].principal, Principal::FoundryProject);
        assert_eq!(e[0].scope, Scope::Resolved(SEARCH_ID.into()));

        let keyed = graph(&[
            (ResourceKind::Agent, "a", agent),
            (
                ResourceKind::Connection,
                "kb-conn",
                json!({"name": "kb-conn", "properties": {
                    "category": "RemoteTool", "authType": "ApiKey",
                    "target": "https://srch.search.windows.net/knowledgebases/kb/mcp"
                }}),
            ),
        ]);
        assert!(
            find(&keyed, roles::SEARCH_INDEX_DATA_READER).is_empty(),
            "a key-based connection needs no role"
        );
    }

    // ---- row 9: customer-managed keys ------------------------------

    #[test]
    fn encryption_key_without_access_credentials_needs_crypto_service_encryption_user() {
        let g = graph(&[(
            ResourceKind::Index,
            "idx",
            json!({
                "name": "idx", "fields": [],
                "encryptionKey": {
                    "keyVaultUri": "https://kv.vault.azure.net",
                    "keyVaultKeyName": "k", "keyVaultKeyVersion": "1"
                }
            }),
        )]);
        let e = rbac_edges(&g)[0];
        assert_eq!(e.role, roles::KEY_VAULT_CRYPTO_SERVICE_ENCRYPTION_USER);
        assert_eq!(e.principal, Principal::SearchSystem);
        assert_eq!(e.scope, Scope::Resolved(VAULT_ID.into()));

        let with_creds = graph(&[(
            ResourceKind::Index,
            "idx",
            json!({
                "name": "idx", "fields": [],
                "encryptionKey": {
                    "keyVaultUri": "https://kv.vault.azure.net",
                    "keyVaultKeyName": "k", "keyVaultKeyVersion": "1",
                    "accessCredentials": {"applicationId": "app"}
                }
            }),
        )]);
        assert!(
            rbac_edges(&with_creds).is_empty(),
            "explicit credentials, no identity edge"
        );
    }

    // ---- scopes ----------------------------------------------------

    #[test]
    fn a_binding_without_an_arm_id_yields_an_unresolved_scope_naming_it() {
        let environment = Environment {
            search: Some(SearchConnection {
                service: "srch".into(),
                ..Default::default()
            }),
            dependencies: [(
                "docs".to_string(),
                Binding {
                    kind: BindingType::Storage,
                    value: "acct".into(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let bindings = EnvBindings::of_env("dev", &environment, None);
        let g = graph_for_docs(
            &bindings,
            &[(
                ResourceKind::DataSource,
                "ds".to_string(),
                json!({
                    "name": "ds", "type": "azureblob",
                    "credentials": {"connectionString": conn(STORAGE_ID)},
                    "container": {"name": "c"}
                }),
            )],
        );
        assert_eq!(
            g.edges[0].scope,
            Scope::Unresolved {
                binding: "docs".into(),
                kind: Some(BindingType::Storage),
                physical: "acct".into()
            }
        );
        assert_eq!(
            g.edges[0].id,
            "search-system|2a2b9908-6ea1-4ae2-8e65-a410df84e7d1|unresolved:docs:acct"
        );
    }

    #[test]
    fn an_unbound_reference_still_yields_an_edge_with_an_empty_binding() {
        let bindings = EnvBindings::of_env("dev", &Environment::default(), None);
        let g = graph_for_docs(
            &bindings,
            &[(
                ResourceKind::DataSource,
                "ds".to_string(),
                json!({
                    "name": "ds", "type": "azureblob",
                    "credentials": {"connectionString": conn(STORAGE_ID)},
                    "container": {"name": "c"}
                }),
            )],
        );
        assert_eq!(
            g.edges[0].scope,
            Scope::Unresolved {
                binding: String::new(),
                kind: Some(BindingType::Storage),
                physical: "acct".into()
            }
        );
        assert!(
            g.checks.iter().all(|c| c.kind != CheckKind::SearchSku),
            "no search target, no search checks"
        );
    }

    #[test]
    fn edges_dedup_by_principal_role_and_scope_merging_their_sources() {
        let ds = |name: &str| {
            json!({
                "name": name, "type": "azureblob",
                "credentials": {"connectionString": conn(STORAGE_ID)},
                "container": {"name": "c"}
            })
        };
        let g = graph(&[
            (ResourceKind::DataSource, "one", ds("one")),
            (ResourceKind::DataSource, "two", ds("two")),
        ]);
        assert_eq!(rbac_edges(&g).len(), 1);
        assert_eq!(rbac_edges(&g)[0].sources.len(), 2);
        let network: Vec<&Check> = g
            .checks
            .iter()
            .filter(|c| c.id.starts_with("storage-network:"))
            .collect();
        assert_eq!(network.len(), 1, "checks dedup too");
        assert_eq!(network[0].sources.len(), 2);
    }

    // ---- checks ----------------------------------------------------

    #[test]
    fn every_environment_with_a_search_target_carries_the_three_service_checks() {
        let g = graph(&[]);
        assert!(has_check(&g, "search-sku"));
        assert!(has_check(&g, "search-identity"));
        assert!(has_check(&g, "search-rbac-enabled"));
    }

    #[test]
    fn a_deployment_yields_an_availability_check_named_by_its_stem() {
        let g = graph(&[(
            ResourceKind::Deployment,
            "gpt-4o",
            json!({"name": "gpt-4o", "sku": {"name": "GlobalStandard", "capacity": 1},
                   "properties": {"model": {"name": "gpt-4o", "version": "2024-11-20"}}}),
        )]);
        assert!(has_check(&g, "deployment-availability:gpt-4o"));
    }

    // ---- operator edges --------------------------------------------

    #[test]
    fn operator_edges_follow_the_plan_contents() {
        let env = env();
        let (edges, checks) = operator_edges(&env, &[ResourceKind::Index], false, &[]);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].role, roles::SEARCH_SERVICE_CONTRIBUTOR);
        assert_eq!(edges[0].principal, Principal::Operator);
        assert_eq!(edges[0].scope, Scope::Resolved(SEARCH_ID.into()));
        assert!(checks.is_empty());

        let (verify, _) = operator_edges(&env, &[ResourceKind::Index], true, &[]);
        let reader = verify
            .iter()
            .find(|e| e.role == roles::SEARCH_INDEX_DATA_READER)
            .expect("--verify adds the data-plane read");
        assert_eq!(
            reader.alternatives,
            vec![roles::SEARCH_INDEX_DATA_CONTRIBUTOR]
        );

        let (foundry, _) = operator_edges(
            &env,
            &[
                ResourceKind::Agent,
                ResourceKind::Connection,
                ResourceKind::Deployment,
            ],
            false,
            &[],
        );
        let used: Vec<&str> = foundry.iter().map(|e| e.role.name).collect();
        assert_eq!(
            used,
            vec![
                "Azure AI User",
                "Azure AI Project Manager",
                "Azure AI Account Owner"
            ]
        );
        assert!(
            foundry
                .iter()
                .all(|e| e.scope == Scope::Resolved(FOUNDRY_ID.into()))
        );
        let owner = foundry.last().unwrap();
        assert_eq!(
            owner.alternatives,
            vec![roles::COGNITIVE_SERVICES_CONTRIBUTOR]
        );

        let (guardrail, _) = operator_edges(&env, &[ResourceKind::Guardrail], false, &[]);
        assert_eq!(guardrail.len(), 1);
        assert_eq!(guardrail[0].role, roles::FOUNDRY_ACCOUNT_OWNER);
    }

    #[test]
    fn operator_checks_are_one_can_grant_per_distinct_scope() {
        let env = env();
        let g = graph(&[
            (
                ResourceKind::DataSource,
                "ds",
                json!({
                    "name": "ds", "type": "azureblob",
                    "credentials": {"connectionString": conn(STORAGE_ID)},
                    "container": {"name": "c"}
                }),
            ),
            (
                ResourceKind::Index,
                "idx",
                json!({
                    "name": "idx", "fields": [],
                    "vectorSearch": {"vectorizers": [{"name": "v", "kind": "azureOpenAI",
                        "azureOpenAIParameters": {"resourceUri": "https://fndr.openai.azure.com"}}]}
                }),
            ),
        ]);
        let missing: Vec<&Edge> = g.edges.iter().collect();
        assert_eq!(missing.len(), 2);
        let (_, checks) = operator_edges(&env, &[ResourceKind::Index], false, &missing);
        assert_eq!(checks.len(), 2);
        assert!(
            checks
                .iter()
                .any(|c| c.id == format!("can-grant:{STORAGE_ID}"))
        );
        assert!(
            checks
                .iter()
                .any(|c| c.id == format!("can-grant:{FOUNDRY_ID}"))
        );
        assert!(checks.iter().all(|c| !c.sources.is_empty()));

        let same: Vec<&Edge> = vec![missing[0], missing[0]];
        let (_, deduped) = operator_edges(&env, &[], false, &same);
        assert_eq!(deduped.len(), 1);
    }

    // ---- compatibility ---------------------------------------------

    #[test]
    fn edges_for_one_document_works_without_any_bindings() {
        let edges = edges_for(
            ResourceKind::Skillset,
            "ss",
            &json!({
                "name": "ss", "skills": [],
                "cognitiveServices": {
                    "@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity",
                    "subdomainUrl": "https://aisvc.cognitiveservices.azure.com/"
                }
            }),
        );
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].role, roles::COGNITIVE_SERVICES_USER);
        assert!(matches!(edges[0].scope, Scope::Unresolved { .. }));
        assert_eq!(edges[0].scope.arm_id(), None);
    }

    #[test]
    fn placeholder_resource_ids_are_not_scopes() {
        assert_eq!(
            parse_resource_id("ResourceId=/subscriptions/<subscription-id>/...;"),
            None
        );
        assert_eq!(parse_resource_id("AccountKey=zzz"), None);
        assert_eq!(
            parse_resource_id("ResourceId=/subscriptions/a/resourceGroups/b;Database=d").as_deref(),
            Some("/subscriptions/a/resourceGroups/b")
        );
    }
}
