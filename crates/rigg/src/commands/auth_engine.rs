//! The auth verification engine (spec §3.3, §4.1): turn an environment's
//! identity graph into a verified [`Report`], apply the fixes it found, and
//! render it as text or JSON.
//!
//! `rigg auth doctor` and `rigg push`'s auth preflight are both thin over
//! [`verify`] — the difference is only which documents go in
//! ([`VerifyScope`]) and what the caller does with the result. `rigg status
//! --auth` uses the same call and prints nothing but the summary.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result};
use colored::Colorize;
use serde_json::{Value, json};

use rigg_client::arm::{ArmClient, ResourceIdentity};
use rigg_client::arm_reads::StorageAccountInfo;
use rigg_client::arm_resources::resolve_account_scope;
use rigg_core::binding::{BindingCache, BindingType, EnvBindings, TargetKind};
use rigg_core::identity::{
    Check, CheckKind, Constraint, Edge, EdgeKind, Graph, Principal, Scope, Source, graph_for_docs,
    operator_edges, roles,
};
use rigg_core::registry::Provider;
use rigg_core::resources::ResourceKind;
use rigg_core::store::{ProjectState, Store, SyncClass};
use rigg_core::workspace::{ResolvedEnv, Workspace};

use super::GlobalContext;
use super::remote::Remote;

/// Which documents the graph is built from.
pub enum VerifyScope {
    /// Every file in the environment's tree (`rigg auth doctor`).
    EnvTree,
    /// Only the documents a push would create or update (`--plan`, and the
    /// push preflight, which hands its own plan straight in).
    Plan(Vec<(ResourceKind, String, Value)>),
}

/// Knobs [`verify`] takes.
#[derive(Debug, Clone, Default)]
pub struct VerifyOpts {
    /// Check operator rights for this object id instead of the caller's
    /// (`--principal`, e.g. the CI identity).
    pub principal: Option<String>,
    /// Also require the data-plane read roles `rigg query` / `ask` /
    /// `push --verify` need.
    pub verify_roles: bool,
    /// Read each indexer's last execution result and attribute auth-shaped
    /// failures to the edge that would explain them.
    pub live: bool,
}

/// The verdict on one edge or check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// The role/setting is not in place (`✗` for an edge, `!` for a check).
    Missing,
    /// rigg could not determine the answer — an unbound scope, an identity
    /// that does not exist yet, an ARM read that was denied.
    Unresolved,
    /// Deliberately not checked here, with the reason.
    Skipped(String),
    /// Reported for context; never a failure.
    Informational,
}

impl Status {
    fn symbol(&self, is_check: bool) -> colored::ColoredString {
        match self {
            Status::Ok => "✓".green().bold(),
            Status::Missing if is_check => "!".yellow().bold(),
            Status::Missing => "✗".red().bold(),
            Status::Unresolved => "?".yellow().bold(),
            Status::Skipped(_) => "-".dimmed(),
            Status::Informational => "ⓘ".blue(),
        }
    }

    fn as_str(&self) -> &str {
        match self {
            Status::Ok => "ok",
            Status::Missing => "missing",
            Status::Unresolved => "unresolved",
            Status::Skipped(_) => "skipped",
            Status::Informational => "informational",
        }
    }
}

/// What a [`ReportItem`] is about.
#[derive(Debug, Clone)]
pub enum What {
    Edge(Box<Edge>),
    Check(Box<Check>),
}

impl What {
    fn is_check(&self) -> bool {
        matches!(self, What::Check(_))
    }

    fn sources(&self) -> &[Source] {
        match self {
            What::Edge(e) => &e.sources,
            What::Check(c) => &c.sources,
        }
    }

    fn reason(&self) -> &str {
        match self {
            What::Edge(e) => &e.reason,
            What::Check(c) => &c.reason,
        }
    }

    /// The ARM id this item is about, when it has one — what `--live`
    /// attribution and `auth roles` match on.
    fn scope_id(&self) -> Option<&str> {
        match self {
            What::Edge(e) => e.scope.arm_id(),
            What::Check(c) => check_scope(c).and_then(Scope::arm_id),
        }
    }
}

/// One verified requirement.
#[derive(Debug, Clone)]
pub struct ReportItem {
    pub id: String,
    pub what: What,
    pub status: Status,
    /// The specifics: what was found, and what it means.
    pub detail: String,
    pub fix: Option<Fix>,
}

impl ReportItem {
    fn new(what: What, status: Status, detail: impl Into<String>) -> ReportItem {
        let id = match &what {
            What::Edge(e) => e.id.clone(),
            What::Check(c) => c.id.clone(),
        };
        ReportItem {
            id,
            what,
            status,
            detail: detail.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: Fix) -> ReportItem {
        self.fix = Some(fix);
        self
    }

    /// The one-line headline: principal → role @ scope, or the check's name.
    pub fn headline(&self) -> String {
        match &self.what {
            What::Edge(e) => format!("{} → {} @ {}", e.principal, e.role.name, e.scope.describe()),
            What::Check(c) => check_label(c),
        }
    }
}

/// How many of each verdict — what the exit code and `status --auth` read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub ok: usize,
    pub missing: usize,
    pub unresolved: usize,
}

impl Summary {
    /// Is anything wrong? (`missing` or `unresolved` — either means rigg
    /// cannot promise a push will work.)
    pub fn clean(&self) -> bool {
        self.missing == 0 && self.unresolved == 0
    }
}

/// A verified identity graph for one environment.
pub struct Report {
    pub env: String,
    /// Where the environment points, for the report header.
    pub targets: Vec<String>,
    /// Service-identity edges and setting/network checks.
    pub items: Vec<ReportItem>,
    /// The operator's own rights, and whether they can grant what is missing.
    pub operator: Vec<ReportItem>,
    /// Who the operator edges were checked for.
    pub operator_label: String,
    /// `--live` findings that no scope in the graph explains.
    pub live: Vec<String>,
    pub summary: Summary,
}

impl Report {
    fn all(&self) -> impl Iterator<Item = &ReportItem> {
        self.items.iter().chain(self.operator.iter())
    }

    /// Every distinct fix the report found, in report order.
    pub fn fixes(&self) -> Vec<Fix> {
        let mut seen = BTreeSet::new();
        self.all()
            .filter(|i| i.status == Status::Missing)
            .filter_map(|i| i.fix.clone())
            .filter(|f| seen.insert(f.key()))
            .collect()
    }

    /// Missing items rigg has no fix for — what keeps the exit code at 4
    /// even after `--fix`.
    pub fn unfixable(&self) -> Vec<&ReportItem> {
        self.all()
            .filter(|i| i.status == Status::Missing && i.fix.is_none())
            .collect()
    }

    /// Items rigg could not judge. Applying a fix does not resolve them —
    /// an edge that could not be checked because the identity did not exist
    /// still has not been checked once the identity is created — so they
    /// keep the exit code at 4 and earn a "re-run" line.
    pub fn unresolved(&self) -> Vec<&ReportItem> {
        self.all()
            .filter(|i| i.status == Status::Unresolved)
            .collect()
    }
}

/// A repair rigg can perform itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    RoleAssignment {
        scope: String,
        principal_id: String,
        principal_type: String,
        role: roles::Role,
        description: String,
    },
    EnableSystemIdentity {
        resource_id: String,
        provider: Provider,
    },
    EnableRbac {
        search_id: String,
    },
    BlobSoftDelete {
        account_id: String,
        days: u32,
    },
    StorageBypassAzureServices {
        account_id: String,
    },
    StorageResourceRule {
        account_id: String,
        tenant: String,
        search_id: String,
    },
}

impl Fix {
    /// De-duplication key: two report items may want the very same repair.
    fn key(&self) -> String {
        format!("{self:?}")
    }

    fn kind(&self) -> &'static str {
        match self {
            Fix::RoleAssignment { .. } => "role-assignment",
            Fix::EnableSystemIdentity { .. } => "enable-system-identity",
            Fix::EnableRbac { .. } => "enable-rbac",
            Fix::BlobSoftDelete { .. } => "blob-soft-delete",
            Fix::StorageBypassAzureServices { .. } => "storage-bypass-azure-services",
            Fix::StorageResourceRule { .. } => "storage-resource-instance-rule",
        }
    }

    /// What applying this does, in one line.
    pub fn describe(&self) -> String {
        match self {
            Fix::RoleAssignment {
                scope,
                principal_id,
                role,
                ..
            } => format!("assign '{}' to {principal_id} at {scope}", role.name),
            Fix::EnableSystemIdentity { resource_id, .. } => {
                format!("enable the system-assigned identity on {resource_id}")
            }
            Fix::EnableRbac { search_id } => {
                format!("accept Entra tokens (authOptions.aadOrApiKey) on {search_id}")
            }
            Fix::BlobSoftDelete { account_id, days } => {
                format!("enable blob soft delete ({days} days) on {account_id}")
            }
            Fix::StorageBypassAzureServices { account_id } => {
                format!("add the AzureServices firewall bypass on {account_id}")
            }
            Fix::StorageResourceRule {
                account_id,
                search_id,
                ..
            } => format!(
                "admit {search_id} through {account_id}'s firewall (resource-instance rule)"
            ),
        }
    }

    /// The equivalent `az` command, for a user who would rather run it.
    pub fn command(&self) -> String {
        match self {
            Fix::RoleAssignment {
                scope,
                principal_id,
                role,
                ..
            } => format!(
                "az role assignment create --assignee {principal_id} --role \"{}\" --scope \"{scope}\"",
                role.name
            ),
            Fix::EnableSystemIdentity { resource_id, .. } => format!(
                "az resource update --ids \"{resource_id}\" --set identity.type=SystemAssigned"
            ),
            Fix::EnableRbac { search_id } => format!(
                "az search service update --ids \"{search_id}\" --aad-auth-failure-mode http401WithBearerChallenge --auth-options aadOrApiKey"
            ),
            Fix::BlobSoftDelete { account_id, days } => format!(
                "az storage account blob-service-properties update --ids \"{account_id}\" --enable-delete-retention true --delete-retention-days {days}"
            ),
            Fix::StorageBypassAzureServices { account_id } => format!(
                "az storage account update --ids \"{account_id}\" --bypass AzureServices Logging Metrics"
            ),
            Fix::StorageResourceRule {
                account_id,
                tenant,
                search_id,
            } => format!(
                "az storage account network-rule add --account-name {} --resource-id \"{search_id}\" --tenant-id {tenant}  # account: {account_id}",
                account_id.rsplit('/').next().unwrap_or(account_id)
            ),
        }
    }

    fn to_json(&self) -> Value {
        let mut obj = json!({"kind": self.kind(), "command": self.command()});
        match self {
            Fix::RoleAssignment {
                scope,
                principal_id,
                principal_type,
                role,
                description,
            } => {
                obj["scope"] = json!(scope);
                obj["principal_id"] = json!(principal_id);
                obj["principal_type"] = json!(principal_type);
                obj["role"] = json!({"id": role.id, "name": role.name});
                obj["description"] = json!(description);
            }
            Fix::EnableSystemIdentity { resource_id, .. } => obj["resource"] = json!(resource_id),
            Fix::EnableRbac { search_id } => obj["resource"] = json!(search_id),
            Fix::BlobSoftDelete { account_id, days } => {
                obj["resource"] = json!(account_id);
                obj["days"] = json!(days);
            }
            Fix::StorageBypassAzureServices { account_id } => obj["resource"] = json!(account_id),
            Fix::StorageResourceRule {
                account_id,
                search_id,
                ..
            } => {
                obj["resource"] = json!(account_id);
                obj["admits"] = json!(search_id);
            }
        }
        obj
    }
}

/// Apply `fixes` in order, one result per fix. A failure never aborts the
/// rest — a caller wants to know everything that could and could not be
/// repaired in one pass.
pub async fn apply(
    ctx: &GlobalContext,
    arm: &ArmClient,
    fixes: &[Fix],
) -> Result<Vec<(Fix, Result<(), String>)>> {
    let mut out = Vec::new();
    for fix in fixes {
        crate::say!(ctx, "  {} {}", "fix".cyan().bold(), fix.describe());
        let outcome = match fix {
            Fix::RoleAssignment {
                scope,
                principal_id,
                principal_type,
                role,
                description,
            } => arm
                .create_role_assignment_described(
                    scope,
                    principal_id,
                    role.id,
                    principal_type,
                    description,
                )
                .await
                .map_err(|e| format!("{e}")),
            Fix::EnableSystemIdentity {
                resource_id,
                provider,
            } => arm
                .enable_system_identity(resource_id, *provider)
                .await
                .map_err(|e| format!("{e}")),
            Fix::EnableRbac { search_id } => arm
                .set_search_auth_options(search_id)
                .await
                .map_err(|e| format!("{e}")),
            Fix::BlobSoftDelete { account_id, days } => arm
                .set_blob_soft_delete(account_id, *days)
                .await
                .map_err(|e| format!("{e}")),
            Fix::StorageBypassAzureServices { account_id } => arm
                .add_storage_bypass_azure_services(account_id)
                .await
                .map_err(|e| format!("{e}")),
            Fix::StorageResourceRule {
                account_id,
                tenant,
                search_id,
            } => arm
                .add_storage_resource_instance_rule(account_id, tenant, search_id)
                .await
                .map_err(|e| format!("{e}")),
        };
        match &outcome {
            Ok(()) => crate::say!(ctx, "      {} applied", "✓".green()),
            Err(e) => crate::say!(ctx, "      {} {e}", "✗".red()),
        }
        out.push((fix.clone(), outcome));
    }
    Ok(out)
}

// ---------------------------------------------------------------------
// verification
// ---------------------------------------------------------------------

/// A principal that resolved to a directory object id.
#[derive(Debug, Clone)]
struct Resolved {
    id: String,
    /// `ServicePrincipal` or `User` — what a role assignment must carry.
    principal_type: String,
    label: String,
}

/// Holds the ARM client and every read the verification does, so a scope
/// touched by several edges is fetched once.
pub struct Verifier {
    arm: Option<ArmClient>,
    env_name: String,
    tenant: Option<String>,
    subscription: Option<String>,
    bindings: EnvBindings,
    /// `rigg:<workspace>:<env>` — the description prefix rigg stamps on the
    /// role assignments it creates, and matches on when removing them.
    description_prefix: String,
    search_id: Option<String>,
    search: Option<rigg_client::arm_reads::SearchServiceInfo>,
    foundry_project_id: Option<String>,
    principals: BTreeMap<String, std::result::Result<Resolved, String>>,
    storage: BTreeMap<String, std::result::Result<StorageAccountInfo, String>>,
}

impl Verifier {
    fn arm(&self) -> Result<&ArmClient> {
        self.arm
            .as_ref()
            .context("this check needs Azure Resource Manager access (az login)")
    }

    async fn storage_account(
        &mut self,
        id: &str,
    ) -> std::result::Result<StorageAccountInfo, String> {
        if let Some(hit) = self.storage.get(id) {
            return hit.clone();
        }
        let got = match self.arm() {
            Ok(arm) => arm
                .get_storage_account(id)
                .await
                .map_err(|e| format!("{e}")),
            Err(e) => Err(format!("{e}")),
        };
        self.storage.insert(id.to_string(), got.clone());
        got
    }
}

/// The description rigg stamps on a role assignment it creates:
/// `rigg:<workspace>:<env>:<reason>`.
pub fn role_description(prefix: &str, reason: &str) -> String {
    format!("{prefix}:{reason}")
}

/// `rigg:<workspace>:<env>` — the prefix `rigg auth roles` filters on.
pub fn description_prefix(ws: &Workspace, env: &str) -> String {
    let name = ws
        .config
        .name
        .clone()
        .or_else(|| ws.root.file_name().map(|f| f.to_string_lossy().to_string()))
        .unwrap_or_else(|| "workspace".to_string());
    format!("rigg:{name}:{env}")
}

/// Build the environment's binding table, resolving anything the cache does
/// not already know through ARM and saving what it learns (spec §4.1 step 1).
pub async fn bindings_for(
    ws: &Workspace,
    env: &ResolvedEnv,
    arm: Option<&ArmClient>,
) -> EnvBindings {
    let mut cache = BindingCache::load(ws, &env.name);
    let Some(arm) = arm else {
        return EnvBindings::of_env(&env.name, &env.env, Some(&cache));
    };

    let mut wanted: Vec<(String, TargetKind, String)> = Vec::new();
    if let Some(search) = &env.env.search {
        wanted.push((
            "search".to_string(),
            TargetKind::Search,
            search.service.clone(),
        ));
    }
    if let Some(foundry) = &env.env.foundry {
        wanted.push((
            "foundry".to_string(),
            TargetKind::Foundry,
            foundry.account.clone(),
        ));
    }
    for (name, binding) in &env.env.dependencies {
        wanted.push((
            name.clone(),
            TargetKind::Binding(binding.kind),
            binding.value.clone(),
        ));
    }

    let mut learned = false;
    for (name, kind, value) in wanted {
        // Already cached (and still pointing at the same physical resource)?
        // Leave it alone: `rigg env show --refresh` is the explicit refresh.
        if cache
            .get(&name)
            .is_some_and(|r| r.arm_id.is_some() || r.endpoint.is_some())
        {
            continue;
        }
        if let Ok(mut resolved) = arm
            .resolve_target(kind, &value, env.env.subscription.as_deref())
            .await
        {
            resolved.name = name.clone();
            cache.bindings.insert(name, resolved);
            learned = true;
        }
    }
    if learned {
        let _ = cache.save(ws, &env.name);
    }
    EnvBindings::of_env(&env.name, &env.env, Some(&cache))
}

/// Every document in the environment's tree.
pub fn env_documents(ws: &Workspace, env: &str) -> Vec<(ResourceKind, String, Value)> {
    let mut docs = Vec::new();
    for project in &ws.projects {
        let store = Store::new(project, env);
        let Ok(files) = store.list() else { continue };
        for (r, _) in files {
            if let Ok(value) = store.read(&r) {
                docs.push((r.kind, r.name.clone(), value));
            }
        }
    }
    docs
}

/// The documents a push would create or update — the same classification
/// push itself uses, so `--plan` and the push preflight see one plan.
pub async fn plan_documents(
    ws: &Workspace,
    env: &ResolvedEnv,
) -> Result<Vec<(ResourceKind, String, Value)>> {
    let mut docs = Vec::new();
    for project in &ws.projects {
        let store = Store::new(project, &env.name);
        let remote = Remote::for_project(env, project);
        let state = ProjectState::load(ws, &env.name, &project.name);
        let reachable = remote.supported_kinds();
        for (r, _) in store.list()? {
            let body = store.read(&r)?;
            let planned = if reachable.contains(&r.kind) {
                let remote_doc = remote.get(&r).await?;
                matches!(
                    state.classify(&r, Some(&body), remote_doc.as_ref()),
                    SyncClass::LocalAhead | SyncClass::LocalOnly | SyncClass::Untracked
                )
            } else {
                // Nothing to compare against — assume a push would send it.
                true
            };
            if planned {
                docs.push((r.kind, r.name.clone(), body));
            }
        }
    }
    Ok(docs)
}

/// Verify the identity graph `scope` implies for `env` (spec §4.1 steps 2–6).
pub async fn verify(
    _ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    scope: VerifyScope,
    opts: VerifyOpts,
) -> Result<Report> {
    let arm = ArmClient::for_tenant(env.env.tenant.as_deref()).ok();
    let bindings = bindings_for(ws, env, arm.as_ref()).await;

    let docs = match scope {
        VerifyScope::EnvTree => env_documents(ws, &env.name),
        VerifyScope::Plan(docs) => docs,
    };
    let graph = graph_for_docs(&bindings, &docs);

    let mut v = Verifier {
        arm,
        env_name: env.name.clone(),
        tenant: env.env.tenant.clone(),
        subscription: env.env.subscription.clone(),
        bindings: bindings.clone(),
        description_prefix: description_prefix(ws, &env.name),
        search_id: None,
        search: None,
        foundry_project_id: None,
        principals: BTreeMap::new(),
        storage: BTreeMap::new(),
    };
    v.resolve_targets(env, &bindings).await;

    // 1. Service-identity edges.
    let mut items: Vec<ReportItem> = Vec::new();
    for edge in &graph.edges {
        items.push(v.verify_edge(edge).await);
    }

    // 2. Settings, network, and the rest of §3.3.
    for check in &graph.checks {
        items.push(v.verify_check(check, &graph).await);
    }

    // 3. The operator's own rights, plus a grant check per scope that is
    //    missing something (there is no point offering `--fix` for a scope
    //    the caller cannot write role assignments at).
    let missing_edges: Vec<&Edge> = graph
        .edges
        .iter()
        .filter(|e| {
            items
                .iter()
                .any(|i| i.id == e.id && i.status == Status::Missing)
        })
        .collect();
    let kinds: Vec<ResourceKind> = docs.iter().map(|(k, ..)| *k).collect();
    let (op_edges, op_checks) = operator_edges(
        &bindings,
        &kinds,
        opts.verify_roles,
        &missing_edges,
        v.foundry_project_id.as_deref(),
    );
    let operator_principal = match &opts.principal {
        Some(id) => Principal::Named {
            object_id: id.clone(),
        },
        None => Principal::Operator,
    };
    let mut operator: Vec<ReportItem> = Vec::new();
    for edge in &op_edges {
        let mut edge = edge.clone();
        edge.principal = operator_principal.clone();
        operator.push(v.verify_edge(&edge).await);
    }
    for check in &op_checks {
        operator.push(v.verify_check(check, &graph).await);
    }

    // 4. `--live`: proof beats a green report.
    let mut live = Vec::new();
    if opts.live {
        live = live_findings(ws, env, &docs, &mut items).await;
    }

    let operator_label = match v.principal_label(&operator_principal).await {
        Ok(label) => label,
        Err(e) => format!("unknown ({e})"),
    };

    let summary = summarize(items.iter().chain(operator.iter()));
    Ok(Report {
        env: env.name.clone(),
        targets: Remote::for_env(env).target_lines(),
        items,
        operator,
        operator_label,
        live,
        summary,
    })
}

fn summarize<'a>(items: impl Iterator<Item = &'a ReportItem>) -> Summary {
    let mut s = Summary::default();
    for item in items {
        match item.status {
            Status::Ok => s.ok += 1,
            Status::Missing => s.missing += 1,
            Status::Unresolved => s.unresolved += 1,
            _ => {}
        }
    }
    s
}

impl Verifier {
    /// Resolve the two ARM ids every check leans on: the search service and
    /// the Foundry project (`<account id>/projects/<project>`, which the
    /// binding table does not carry).
    async fn resolve_targets(&mut self, env: &ResolvedEnv, bindings: &EnvBindings) {
        if let Some(conn) = env.search() {
            self.search_id = match bindings
                .get("search")
                .and_then(|e| e.resolved.as_ref().and_then(|r| r.arm_id.clone()))
            {
                Some(id) => Some(id),
                None => match self.arm.as_ref() {
                    Some(arm) => arm.find_search_service_id(&conn.service).await.ok(),
                    None => None,
                },
            };
            if let (Some(arm), Some(id)) = (self.arm.as_ref(), self.search_id.as_deref()) {
                self.search = arm.get_search_service(id).await.ok();
            }
        }
        if let Some(conn) = env.foundry() {
            let account = match bindings.get("foundry").and_then(|e| {
                e.resolved
                    .as_ref()
                    .and_then(|r| r.arm_id.clone())
            }) {
                Some(id) => Some(id),
                None => match self.arm.as_ref() {
                    Some(arm) => resolve_account_scope(arm, &conn.account)
                        .await
                        .ok()
                        .map(|s| {
                            format!(
                                "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}",
                                s.subscription_id, s.resource_group, s.account
                            )
                        }),
                    None => None,
                },
            };
            self.foundry_project_id = account.map(|a| format!("{a}/projects/{}", conn.project));
        }
    }

    /// Resolve `principal` to a directory object id, caching the answer (and
    /// the failure — a search service with no identity must not be looked up
    /// once per edge).
    async fn principal(&mut self, principal: &Principal) -> std::result::Result<Resolved, String> {
        let key = principal.to_string();
        if let Some(hit) = self.principals.get(&key) {
            return hit.clone();
        }
        let resolved = self.resolve_principal(principal).await;
        self.principals.insert(key, resolved.clone());
        resolved
    }

    async fn resolve_principal(
        &mut self,
        principal: &Principal,
    ) -> std::result::Result<Resolved, String> {
        match principal {
            Principal::SearchSystem => {
                let info = self.search.as_ref().ok_or_else(|| {
                    format!(
                        "environment '{}' has no reachable search service",
                        self.env_name
                    )
                })?;
                match &info.identity.principal_id {
                    Some(id) => Ok(Resolved {
                        id: id.clone(),
                        principal_type: "ServicePrincipal".into(),
                        label: format!("search service '{}' (system-assigned)", info.name),
                    }),
                    None => Err(format!(
                        "search service '{}' has no system-assigned managed identity",
                        info.name
                    )),
                }
            }
            Principal::SearchUser { binding } => {
                let entry = self.uami_principal(binding).await?;
                Ok(Resolved {
                    id: entry,
                    principal_type: "ServicePrincipal".into(),
                    label: format!("user-assigned identity '{binding}'"),
                })
            }
            Principal::FoundryProject => {
                let id = self.foundry_project_id.clone().ok_or_else(|| {
                    format!(
                        "environment '{}' has no reachable Foundry project",
                        self.env_name
                    )
                })?;
                let arm = self.arm().map_err(|e| format!("{e}"))?;
                let identity: Option<ResourceIdentity> = arm
                    .get_resource_identity(&id, Provider::CognitiveServicesArm)
                    .await
                    .map_err(|e| format!("{e}"))?;
                identity
                    .and_then(|i| i.principal_id)
                    .map(|pid| Resolved {
                        id: pid,
                        principal_type: "ServicePrincipal".into(),
                        label: format!("Foundry project '{id}'"),
                    })
                    .ok_or_else(|| {
                        "the Foundry project has no system-assigned managed identity".to_string()
                    })
            }
            Principal::Named { object_id } => Ok(Resolved {
                id: object_id.clone(),
                // A named principal is normally a CI service principal; a
                // wrong `principalType` only affects fixes, which name it.
                principal_type: "ServicePrincipal".into(),
                label: format!("principal {object_id}"),
            }),
            Principal::Operator => {
                let arm = self.arm().map_err(|e| format!("{e}"))?;
                let caller = arm
                    .caller_object_id()
                    .await
                    .map_err(|e| format!("could not determine who you are from the token: {e}"))?;
                Ok(Resolved {
                    id: caller.object_id,
                    principal_type: caller.principal_type,
                    label: caller.display,
                })
            }
        }
    }

    /// The principal id of a user-assigned identity, resolved on demand.
    ///
    /// `binding` is the environment binding that owns the identity — or,
    /// when nothing binds it, the bare name the file's ARM id ends in
    /// (see `identity::principal_for`). A cached resolution already carries
    /// the principal id; otherwise the binding's declared value (or that
    /// bare name) is looked up through ARM now, because bindings are only
    /// refreshed explicitly and doctor must not report "unknown" for an
    /// identity Azure can name.
    async fn uami_principal(&mut self, binding: &str) -> std::result::Result<String, String> {
        let entry = self.bindings.get(binding);
        if let Some(pid) = entry
            .and_then(|e| e.resolved.as_ref())
            .and_then(|r| r.principal_id.clone())
        {
            return Ok(pid);
        }
        let value = match entry {
            Some(e) => e
                .declared
                .as_ref()
                .map(|b| b.value.clone())
                .unwrap_or_else(|| e.physical_name.clone()),
            None => binding.to_string(),
        };
        let subscription = self.subscription.clone();
        let env_name = self.env_name.clone();
        let arm = self.arm().map_err(|e| format!("{e}"))?;
        match arm
            .resolve_binding(BindingType::Identity, &value, subscription.as_deref())
            .await
        {
            Ok(r) => r
                .principal_id
                .ok_or_else(|| format!("user-assigned identity '{binding}' has no principal id")),
            Err(e) => Err(format!(
                "identity '{binding}' is not bound in '{env_name}' ({e})"
            )),
        }
    }

    async fn principal_label(
        &mut self,
        principal: &Principal,
    ) -> std::result::Result<String, String> {
        self.principal(principal).await.map(|r| r.label)
    }

    /// Verify one edge: principal exists, scope resolved, role assigned.
    async fn verify_edge(&mut self, edge: &Edge) -> ReportItem {
        let what = What::Edge(Box::new(edge.clone()));
        if edge.kind != EdgeKind::Rbac {
            let detail = match edge.kind {
                EdgeKind::AppAuthorization => {
                    "authorization lives in the function app's Entra registration — see the \
                     easy-auth check"
                        .to_string()
                }
                _ => String::new(),
            };
            return ReportItem::new(what, Status::Informational, detail);
        }

        let Some(scope) = edge.scope.arm_id().map(str::to_string) else {
            return ReportItem::new(
                what,
                Status::Unresolved,
                format!(
                    "{} — bind it in rigg.yaml, then `rigg env show {} --refresh`",
                    edge.scope.describe(),
                    self.env_name
                ),
            );
        };

        let principal = match self.principal(&edge.principal).await {
            Ok(p) => p,
            Err(e) => return ReportItem::new(what, Status::Unresolved, e),
        };

        let arm = match self.arm() {
            Ok(arm) => arm,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        let assignments = match arm.role_assignments_for(&scope, &principal.id).await {
            Ok(a) => a,
            Err(e) => {
                return ReportItem::new(
                    what,
                    Status::Unresolved,
                    format!("could not read role assignments at {scope}: {e}"),
                );
            }
        };
        let satisfied = assignments.iter().any(|a| {
            let guid = a.role_guid();
            guid.eq_ignore_ascii_case(edge.role.id)
                || edge
                    .alternatives
                    .iter()
                    .any(|alt| guid.eq_ignore_ascii_case(alt.id))
        });
        if satisfied {
            ReportItem::new(what, Status::Ok, format!("{} holds it", principal.label))
        } else {
            let alternatives = if edge.alternatives.is_empty() {
                String::new()
            } else {
                format!(
                    " (or {})",
                    edge.alternatives
                        .iter()
                        .map(|r| r.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            ReportItem::new(
                what,
                Status::Missing,
                format!(
                    "{} lacks '{}'{alternatives} here",
                    principal.label, edge.role.name
                ),
            )
            .with_fix(Fix::RoleAssignment {
                scope,
                principal_id: principal.id,
                principal_type: principal.principal_type,
                role: edge.role,
                description: role_description(&self.description_prefix, &edge.reason),
            })
        }
    }

    /// Verify one check (spec §3.3).
    async fn verify_check(&mut self, check: &Check, graph: &Graph) -> ReportItem {
        let what = What::Check(Box::new(check.clone()));
        match &check.kind {
            CheckKind::SearchSku => match &self.search {
                Some(info) if info.sku.eq_ignore_ascii_case("free") => ReportItem::new(
                    what,
                    Status::Missing,
                    "the Free SKU has no managed identity and cannot host knowledge bases — \
                     recreate the service at Basic or higher",
                ),
                Some(info) => ReportItem::new(what, Status::Ok, format!("SKU '{}'", info.sku)),
                None => ReportItem::new(what, Status::Unresolved, self.no_search()),
            },
            CheckKind::SearchIdentity => match (&self.search, self.search_id.clone()) {
                (Some(info), Some(id)) => {
                    if info.identity.principal_id.is_some() {
                        ReportItem::new(what, Status::Ok, "system-assigned identity enabled")
                    } else if !info.identity.user_assigned.is_empty() {
                        ReportItem::new(
                            what,
                            Status::Ok,
                            "user-assigned identities attached (no system-assigned identity — \
                             the storage trusted-services exception needs one)",
                        )
                    } else {
                        ReportItem::new(
                            what,
                            Status::Missing,
                            "the search service has no managed identity",
                        )
                        .with_fix(Fix::EnableSystemIdentity {
                            resource_id: id,
                            provider: Provider::SearchArm,
                        })
                    }
                }
                _ => ReportItem::new(what, Status::Unresolved, self.no_search()),
            },
            CheckKind::SearchRbacEnabled => match (&self.search, self.search_id.clone()) {
                (Some(info), Some(id)) => {
                    if info.rbac_enabled || info.disable_local_auth {
                        ReportItem::new(what, Status::Ok, "the service accepts Entra tokens")
                    } else {
                        ReportItem::new(
                            what,
                            Status::Missing,
                            "the service accepts API keys only — rigg authenticates with bearer \
                             tokens",
                        )
                        .with_fix(Fix::EnableRbac { search_id: id })
                    }
                }
                _ => ReportItem::new(what, Status::Unresolved, self.no_search()),
            },
            CheckKind::StorageNetwork { account } => {
                self.storage_network(what, account, graph).await
            }
            CheckKind::StorageSoftDelete { account } => {
                self.storage_soft_delete(what, account).await
            }
            CheckKind::StorageSharedKey { account } => {
                let Some(id) = account.arm_id() else {
                    return ReportItem::new(what, Status::Unresolved, account.describe());
                };
                match self.storage_account(id).await {
                    Ok(info) => ReportItem::new(
                        what,
                        Status::Ok,
                        match info.allow_shared_key_access {
                            Some(false) => "shared-key access is disabled — identity-based access \
                                            is unaffected"
                                .to_string(),
                            _ => "shared-key access is allowed; rigg uses identity-based access \
                                  regardless"
                                .to_string(),
                        },
                    ),
                    Err(e) => ReportItem::new(what, Status::Unresolved, e),
                }
            }
            CheckKind::AiServicesKind { account } => {
                let Some(id) = account.arm_id() else {
                    return ReportItem::new(what, Status::Unresolved, account.describe());
                };
                let arm = match self.arm() {
                    Ok(arm) => arm,
                    Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
                };
                match arm.get_cognitive_account_by_id(id).await {
                    Ok(acct) => {
                        let kind = acct.kind.clone();
                        if kind.eq_ignore_ascii_case("AIServices") {
                            ReportItem::new(what, Status::Ok, "account kind is AIServices")
                        } else {
                            ReportItem::new(
                                what,
                                Status::Missing,
                                format!(
                                    "account kind is '{kind}' — AIServicesByIdentity requires an \
                                     AIServices account; point the skillset at one"
                                ),
                            )
                        }
                    }
                    Err(e) => ReportItem::new(what, Status::Unresolved, format!("{e}")),
                }
            }
            CheckKind::FunctionAppNetwork { site } => self.function_app_network(what, site).await,
            CheckKind::EasyAuth { site, audience } => self.easy_auth(what, site, audience).await,
            CheckKind::DeploymentAvailability { .. } => ReportItem::new(
                what,
                Status::Skipped("checked by promote/push".to_string()),
                "model, version, region and quota are checked when the deployment is pushed",
            ),
            CheckKind::CanGrant { scope } => {
                let Some(id) = scope.arm_id() else {
                    return ReportItem::new(what, Status::Unresolved, scope.describe());
                };
                let arm = match self.arm() {
                    Ok(arm) => arm,
                    Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
                };
                match arm.can_write_role_assignments(id).await {
                    Ok(true) => {
                        ReportItem::new(what, Status::Ok, "you can create role assignments here")
                    }
                    Ok(false) => ReportItem::new(
                        what,
                        Status::Missing,
                        "you cannot create role assignments here — ask for Owner or User Access \
                         Administrator at this scope, or have someone run the az command above",
                    ),
                    Err(e) => ReportItem::new(what, Status::Unresolved, format!("{e}")),
                }
            }
        }
    }

    fn no_search(&self) -> String {
        format!(
            "environment '{}' has no reachable search service",
            self.env_name
        )
    }

    async fn storage_network(&mut self, what: What, account: &Scope, graph: &Graph) -> ReportItem {
        let Some(id) = account.arm_id().map(str::to_string) else {
            return ReportItem::new(what, Status::Unresolved, account.describe());
        };
        let info = match self.storage_account(&id).await {
            Ok(info) => info,
            Err(e) => return ReportItem::new(what, Status::Unresolved, e),
        };
        if info.public_network_access.eq_ignore_ascii_case("Disabled") {
            return ReportItem::new(
                what,
                Status::Missing,
                "public network access is disabled — only a shared private link from the search \
                 service reaches this account (Basic+ SKU, indexer executionEnvironment: private)",
            );
        }
        if !info.is_firewalled() {
            return ReportItem::new(
                what,
                Status::Ok,
                "the account's firewall admits all networks",
            );
        }
        // Firewalled. The trusted-services exception works only with the
        // search service's *system* identity (spec §3.3, §7) — a UAMI needs
        // a resource-instance rule instead.
        let uami = graph.edges.iter().any(|e| {
            e.scope.arm_id() == Some(id.as_str())
                && e.constraints
                    .contains(&Constraint::TrustedServiceNeedsSystemIdentity)
        });
        let admitted = self
            .search_id
            .as_deref()
            .is_some_and(|s| info.admits_resource(s));
        if uami {
            if admitted {
                return ReportItem::new(
                    what,
                    Status::Ok,
                    "a resource-instance rule admits the search service (the only firewall \
                     exception a user-assigned identity can use)",
                );
            }
            let item = ReportItem::new(
                what,
                Status::Missing,
                "trusted-services exception needs the system-assigned identity (or a \
                 resource-instance rule)",
            );
            return match (self.search_id.clone(), self.tenant.clone()) {
                (Some(search_id), Some(tenant)) => item.with_fix(Fix::StorageResourceRule {
                    account_id: id,
                    tenant,
                    search_id,
                }),
                // Without a tenant on the environment rigg cannot write the
                // rule; the az command in the detail still says how.
                _ => item,
            };
        }
        if info.bypasses_azure_services() || admitted {
            ReportItem::new(
                what,
                Status::Ok,
                "the firewall admits the search service (trusted services / resource-instance \
                 rule)",
            )
        } else {
            ReportItem::new(
                what,
                Status::Missing,
                format!(
                    "the firewall denies by default and its bypass is '{}' — the search service \
                     cannot reach the account",
                    info.bypass
                ),
            )
            .with_fix(Fix::StorageBypassAzureServices { account_id: id })
        }
    }

    async fn storage_soft_delete(&mut self, what: What, account: &Scope) -> ReportItem {
        let Some(id) = account.arm_id().map(str::to_string) else {
            return ReportItem::new(what, Status::Unresolved, account.describe());
        };
        let arm = match self.arm() {
            Ok(arm) => arm,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        let blob = match arm.get_blob_service_properties(&id).await {
            Ok(b) => b,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        if blob.versioning_enabled {
            return ReportItem::new(
                what,
                Status::Missing,
                "blob versioning is on — native soft-delete deletion detection requires it off",
            );
        }
        if blob.soft_delete_enabled {
            ReportItem::new(
                what,
                Status::Ok,
                format!(
                    "blob soft delete is on ({} day(s))",
                    blob.soft_delete_days.unwrap_or(0)
                ),
            )
        } else {
            ReportItem::new(
                what,
                Status::Missing,
                "blob soft delete is off — the data source's deletion detection policy needs it",
            )
            .with_fix(Fix::BlobSoftDelete {
                account_id: id,
                days: 7,
            })
        }
    }

    async fn function_app_network(&mut self, what: What, site: &Scope) -> ReportItem {
        let Some(id) = site.arm_id() else {
            return ReportItem::new(what, Status::Unresolved, site.describe());
        };
        let arm = match self.arm() {
            Ok(arm) => arm,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        let config = match arm.site_config(id).await {
            Ok(c) => c,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        let rules = config
            .pointer("/properties/ipSecurityRestrictions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let allows_all = rules.is_empty()
            || rules.iter().any(|r| {
                r.get("action")
                    .and_then(Value::as_str)
                    .is_some_and(|a| a.eq_ignore_ascii_case("Allow"))
                    && r.get("ipAddress")
                        .and_then(Value::as_str)
                        .is_some_and(|ip| ip == "Any" || ip == "0.0.0.0/0")
            });
        let admits_search = rules.iter().any(|r| {
            r.get("tag")
                .and_then(Value::as_str)
                .is_some_and(|t| t.eq_ignore_ascii_case("ServiceTag"))
                && r.get("ipAddress")
                    .and_then(Value::as_str)
                    .is_some_and(|ip| ip.eq_ignore_ascii_case("AzureCognitiveSearch"))
        });
        if allows_all || admits_search {
            ReportItem::new(
                what,
                Status::Ok,
                "the app's access restrictions admit search",
            )
        } else {
            ReportItem::new(
                what,
                Status::Missing,
                format!(
                    "the app's access restrictions do not admit the search service — add the \
                     AzureCognitiveSearch service tag:\n      az webapp config access-restriction \
                     add --ids \"{id}\" --rule-name rigg-search --action Allow --priority 100 \
                     --service-tag AzureCognitiveSearch"
                ),
            )
        }
    }

    async fn easy_auth(&mut self, what: What, site: &Scope, audience: &str) -> ReportItem {
        let Some(id) = site.arm_id() else {
            return ReportItem::new(what, Status::Unresolved, site.describe());
        };
        let arm = match self.arm() {
            Ok(arm) => arm,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        let settings = match arm.site_auth_settings(id).await {
            Ok(s) => s,
            Err(e) => return ReportItem::new(what, Status::Unresolved, format!("{e}")),
        };
        let enabled = settings
            .pointer("/properties/platform/enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && settings
                .pointer("/properties/identityProviders/azureActiveDirectory/enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let audiences: Vec<&str> = settings
            .pointer(
                "/properties/identityProviders/azureActiveDirectory/validation/allowedAudiences",
            )
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        if !enabled {
            ReportItem::new(
                what,
                Status::Missing,
                "Microsoft Entra authentication is not enabled on the function app — run `rigg \
                 auth easy-auth <binding>`",
            )
        } else if audiences.contains(&audience) {
            ReportItem::new(
                what,
                Status::Ok,
                format!("the app accepts the audience '{audience}'"),
            )
        } else {
            ReportItem::new(
                what,
                Status::Missing,
                format!(
                    "the app's allowed audiences ({}) do not include '{audience}' — run `rigg \
                     auth easy-auth <binding>`",
                    if audiences.is_empty() {
                        "none".to_string()
                    } else {
                        audiences.join(", ")
                    }
                ),
            )
        }
    }
}

// ---------------------------------------------------------------------
// --live
// ---------------------------------------------------------------------

/// Auth-shaped markers in an indexer's last execution error (spec §4.1.6).
const AUTH_MARKERS: &[&str] = &[
    "403",
    "401",
    "Unauthorized",
    "Forbidden",
    "AuthorizationPermissionMismatch",
    "AuthorizationFailure",
    "AuthorizationFailed",
    // Storage's own 403 wording, which does not contain any of the above.
    "not authorized",
    "AADSTS",
];

fn looks_like_auth(message: &str) -> bool {
    AUTH_MARKERS
        .iter()
        .any(|m| message.to_lowercase().contains(&m.to_lowercase()))
}

/// Read each indexer's last run and attribute auth-shaped failures to the
/// item whose scope's resource name the message names.
async fn live_findings(
    ws: &Workspace,
    env: &ResolvedEnv,
    docs: &[(ResourceKind, String, Value)],
    items: &mut [ReportItem],
) -> Vec<String> {
    let mut unattributed = Vec::new();
    let _ = ws;
    let remote = Remote::for_env(env);
    if !remote.has_search() {
        return unattributed;
    }
    for (kind, name, _) in docs {
        if *kind != ResourceKind::Indexer {
            continue;
        }
        let status = match remote.indexer_status(name).await {
            Ok(v) => v,
            Err(e) => {
                unattributed.push(format!("indexer '{name}': status unavailable ({e})"));
                continue;
            }
        };
        let message = last_result_error(&status);
        let Some(message) = message else { continue };
        if !looks_like_auth(&message) {
            continue;
        }
        let finding = format!("indexer '{name}' last run failed: {message}");
        match items.iter_mut().find(|i| {
            i.what
                .scope_id()
                .and_then(|id| id.rsplit('/').next())
                .is_some_and(|resource| {
                    !resource.is_empty()
                        && message.to_lowercase().contains(&resource.to_lowercase())
                })
        }) {
            Some(item) => {
                if item.status == Status::Ok {
                    item.status = Status::Missing;
                }
                item.detail = format!("{}\n      live: {finding}", item.detail);
            }
            None => unattributed.push(finding),
        }
    }
    unattributed
}

fn last_result_error(status: &Value) -> Option<String> {
    let last = status.get("lastResult")?;
    if last
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|s| s.eq_ignore_ascii_case("success"))
    {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(m) = last.get("errorMessage").and_then(Value::as_str) {
        parts.push(m.to_string());
    }
    for err in last
        .get("errors")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(m) = err.get("errorMessage").and_then(Value::as_str) {
            parts.push(m.to_string());
        }
    }
    (!parts.is_empty()).then(|| parts.join("; "))
}

// ---------------------------------------------------------------------
// rendering
// ---------------------------------------------------------------------

/// The `Scope` a check is about, when it has one.
pub fn check_scope(check: &Check) -> Option<&Scope> {
    match &check.kind {
        CheckKind::StorageNetwork { account }
        | CheckKind::StorageSoftDelete { account }
        | CheckKind::StorageSharedKey { account }
        | CheckKind::AiServicesKind { account } => Some(account),
        CheckKind::FunctionAppNetwork { site } | CheckKind::EasyAuth { site, .. } => Some(site),
        CheckKind::CanGrant { scope } => Some(scope),
        _ => None,
    }
}

/// Human name for a check, with the resource it is about.
fn check_label(check: &Check) -> String {
    let at = check_scope(check)
        .map(|s| format!(" @ {}", short_scope(s)))
        .unwrap_or_default();
    let name = match &check.kind {
        CheckKind::SearchSku => "search SKU",
        CheckKind::SearchIdentity => "search identity",
        CheckKind::SearchRbacEnabled => "search accepts Entra tokens",
        CheckKind::StorageNetwork { .. } => "storage firewall",
        CheckKind::StorageSoftDelete { .. } => "blob soft delete",
        CheckKind::StorageSharedKey { .. } => "storage shared-key access",
        CheckKind::AiServicesKind { .. } => "AI services account kind",
        CheckKind::FunctionAppNetwork { .. } => "function app access restrictions",
        CheckKind::EasyAuth { .. } => "function app Entra authentication",
        CheckKind::DeploymentAvailability { .. } => "model deployment availability",
        CheckKind::CanGrant { .. } => "you can grant roles",
    };
    format!("{name}{at}")
}

/// A scope shortened to its resource name for headlines.
fn short_scope(scope: &Scope) -> String {
    match scope.arm_id() {
        Some(id) => id.rsplit('/').next().unwrap_or(id).to_string(),
        None => scope.describe(),
    }
}

fn source_line(sources: &[Source]) -> Option<String> {
    if sources.is_empty() {
        return None;
    }
    Some(
        sources
            .iter()
            .map(|s| format!("{}/{}.json:{}", s.kind.directory_name(), s.name, s.path))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn render_item(item: &ReportItem, fix_mode: bool) {
    println!(
        "  {} {}",
        item.status.symbol(item.what.is_check()),
        item.headline()
    );
    if let Status::Skipped(why) = &item.status {
        println!("      skipped: {why}");
    }
    for line in item.detail.lines().filter(|l| !l.trim().is_empty()) {
        println!("      {}", line.trim_start());
    }
    if item.status != Status::Ok
        && !item.what.reason().is_empty()
        && item.what.reason() != item.detail
    {
        println!("      reason: {}", item.what.reason());
    }
    if let Some(files) = source_line(item.what.sources()) {
        println!("      files:  {files}");
    }
    if let Some(fix) = &item.fix
        && item.status == Status::Missing
        && !fix_mode
    {
        println!("      fix:    {}", fix.command());
    }
}

/// The text report (spec §4.1 step 4). `fix_mode` suppresses the `fix:`
/// lines — `--fix` is about to apply them.
pub fn render_text(report: &Report, fix_mode: bool) {
    println!(
        "{} {}",
        "auth doctor".bold(),
        format!("env: {}", report.env).bold()
    );
    for line in &report.targets {
        println!("{line}");
    }
    if report.items.is_empty() && report.operator.is_empty() {
        println!("  (no identity requirements found in this environment)");
        return;
    }
    for item in &report.items {
        render_item(item, fix_mode);
    }
    if !report.operator.is_empty() {
        println!(
            "  {}",
            format!("operator: {}", report.operator_label).bold()
        );
        for item in &report.operator {
            render_item(item, fix_mode);
        }
    }
    for line in &report.live {
        println!("  {} {line}", "!".yellow().bold());
    }
    println!();
    println!(
        "summary: {} ok, {} missing, {} unresolved",
        report.summary.ok, report.summary.missing, report.summary.unresolved
    );
}

fn item_json(item: &ReportItem) -> Value {
    let mut obj = json!({
        "id": item.id,
        "status": item.status.as_str(),
        "reason": item.what.reason(),
        "detail": item.detail,
        "sources": item.what.sources().iter().map(|s| json!({
            "kind": s.kind.directory_name(),
            "name": s.name,
            "path": s.path,
        })).collect::<Vec<_>>(),
    });
    match &item.what {
        What::Edge(e) => {
            obj["principal"] = json!(e.principal.to_string());
            obj["role"] = json!({"id": e.role.id, "name": e.role.name});
            obj["scope"] = json!(e.scope);
        }
        What::Check(c) => {
            obj["check"] = json!(c.kind);
            if let Some(scope) = check_scope(c) {
                obj["scope"] = json!(scope);
            }
        }
    }
    if let Some(fix) = &item.fix {
        obj["fix"] = fix.to_json();
    }
    obj
}

/// The JSON report (brief item 6).
pub fn to_json(report: &Report) -> Value {
    let mut value = json!({
        "env": report.env,
        "edges": report.items.iter()
            .filter(|i| !i.what.is_check())
            .map(item_json)
            .collect::<Vec<_>>(),
        "checks": report.items.iter()
            .filter(|i| i.what.is_check())
            .map(item_json)
            .collect::<Vec<_>>(),
        "operator": report.operator.iter().map(item_json).collect::<Vec<_>>(),
        "summary": {
            "ok": report.summary.ok,
            "missing": report.summary.missing,
            "unresolved": report.summary.unresolved,
        },
    });
    if !report.live.is_empty() {
        value["live"] = json!(report.live);
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_shaped_errors_are_recognised_case_insensitively() {
        assert!(looks_like_auth("Credential provided is not valid (403)"));
        assert!(looks_like_auth("AuthorizationPermissionMismatch"));
        assert!(looks_like_auth("AADSTS700016: application not found"));
        assert!(looks_like_auth("this request is not authorized"));
        assert!(!looks_like_auth("could not parse document at line 4"));
    }

    #[test]
    fn last_result_error_ignores_successful_runs_and_joins_item_errors() {
        let ok = json!({"lastResult": {"status": "success", "errorMessage": null}});
        assert_eq!(last_result_error(&ok), None);
        let failed = json!({"lastResult": {
            "status": "transientFailure",
            "errorMessage": "top",
            "errors": [{"errorMessage": "item"}]
        }});
        assert_eq!(last_result_error(&failed).as_deref(), Some("top; item"));
    }

    #[test]
    fn az_commands_name_the_principal_role_and_scope() {
        let fix = Fix::RoleAssignment {
            scope: "/subscriptions/s/rg/acct".into(),
            principal_id: "pid".into(),
            principal_type: "ServicePrincipal".into(),
            role: roles::STORAGE_BLOB_DATA_READER,
            description: "rigg:ws:dev:because".into(),
        };
        let cmd = fix.command();
        assert!(cmd.contains("--assignee pid"));
        assert!(cmd.contains("--role \"Storage Blob Data Reader\""));
        assert!(cmd.contains("--scope \"/subscriptions/s/rg/acct\""));
    }

    #[test]
    fn a_report_deduplicates_identical_fixes() {
        let fix = Fix::EnableRbac {
            search_id: "/s".into(),
        };
        let item = |id: &str| ReportItem {
            id: id.to_string(),
            what: What::Check(Box::new(Check {
                id: id.to_string(),
                kind: CheckKind::SearchRbacEnabled,
                reason: String::new(),
                sources: Vec::new(),
            })),
            status: Status::Missing,
            detail: String::new(),
            fix: Some(fix.clone()),
        };
        let report = Report {
            env: "dev".into(),
            targets: Vec::new(),
            items: vec![item("a"), item("b")],
            operator: Vec::new(),
            operator_label: String::new(),
            live: Vec::new(),
            summary: Summary::default(),
        };
        assert_eq!(report.fixes().len(), 1);
    }
}
