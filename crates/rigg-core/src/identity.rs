//! Identity-graph extraction: which service-to-service RBAC edges does this
//! workspace's configuration require? (spec §8.2)
//!
//! Edges are derived from the actual resource files — data source connection
//! strings, knowledge-base model wiring, agent→knowledge-base grounding, and
//! encryption key references. `rigg auth doctor` verifies and repairs them.

use serde_json::Value;

use crate::registry;
use crate::resources::traits::ResourceKind;
use crate::store::Store;
use crate::workspace::Workspace;

/// Built-in Azure role definition IDs.
pub mod roles {
    pub const STORAGE_BLOB_DATA_READER: (&str, &str) = (
        "ba92f5b4-2d11-453d-a403-e96b0029c9fe",
        "Storage Blob Data Reader",
    );
    pub const COGNITIVE_SERVICES_OPENAI_USER: (&str, &str) = (
        "5e0bd9bd-7b93-4f28-af87-19fc36ad61bd",
        "Cognitive Services OpenAI User",
    );
    pub const COGNITIVE_SERVICES_USER: (&str, &str) = (
        "a97b65f3-24c7-4388-baec-2e87135dc908",
        "Cognitive Services User",
    );
    pub const SEARCH_INDEX_DATA_READER: (&str, &str) = (
        "1407120a-92aa-4202-b7e9-c0e197c71c8f",
        "Search Index Data Reader",
    );
    pub const KEY_VAULT_SECRETS_USER: (&str, &str) = (
        "4633458b-17de-408a-b874-0445c86b69e6",
        "Key Vault Secrets User",
    );
}

/// Whose identity must hold the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Principal {
    SearchService,
    FoundryProject,
}

/// How the edge can be verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum EdgeKind {
    /// ARM RBAC role assignment — verifiable and fixable.
    Rbac,
    /// Data-plane permission model outside ARM RBAC (Cosmos SQL roles,
    /// Azure SQL contained users) — reported with guidance only.
    Informational,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct IdentityEdge {
    pub principal: Principal,
    /// ARM resource id of the target scope, when derivable from config.
    pub scope: Option<String>,
    pub target: String,
    pub role_id: String,
    pub role_name: String,
    pub kind: EdgeKind,
    pub reason: String,
}

impl IdentityEdge {
    fn rbac(
        principal: Principal,
        scope: Option<String>,
        target: impl Into<String>,
        role: (&str, &str),
        reason: impl Into<String>,
    ) -> Self {
        IdentityEdge {
            principal,
            scope,
            target: target.into(),
            role_id: role.0.to_string(),
            role_name: role.1.to_string(),
            kind: EdgeKind::Rbac,
            reason: reason.into(),
        }
    }
}

/// Extract every identity edge the workspace's resources require, for one
/// environment's tree.
pub fn identity_edges(ws: &Workspace, env: &str) -> Vec<IdentityEdge> {
    let mut edges: Vec<IdentityEdge> = Vec::new();
    for project in &ws.projects {
        let store = Store::new(project, env);
        let Ok(files) = store.list() else { continue };
        for (r, _) in files {
            let Ok(value) = store.read(&r) else { continue };
            match r.kind {
                ResourceKind::DataSource => datasource_edges(&r.name, &value, &mut edges),
                ResourceKind::KnowledgeBase
                    if value
                        .get("models")
                        .is_some_and(|m| !m.as_array().is_none_or(|a| a.is_empty()))
                        || value.get("answerInstructions").is_some() =>
                {
                    edges.push(IdentityEdge::rbac(
                        Principal::SearchService,
                        None,
                        "Foundry account (model access)",
                        roles::COGNITIVE_SERVICES_USER,
                        format!(
                            "knowledge base '{}' uses a model for retrieval/synthesis",
                            r.name
                        ),
                    ));
                }
                ResourceKind::Skillset if value.to_string().contains("AzureOpenAI") => {
                    edges.push(IdentityEdge::rbac(
                        Principal::SearchService,
                        None,
                        "Foundry account (model access)",
                        roles::COGNITIVE_SERVICES_OPENAI_USER,
                        format!("skillset '{}' calls Azure OpenAI", r.name),
                    ));
                }
                ResourceKind::Index => {
                    let has_vectorizer = value
                        .get("vectorSearch")
                        .and_then(|v| v.get("vectorizers"))
                        .and_then(Value::as_array)
                        .is_some_and(|a| !a.is_empty());
                    if has_vectorizer {
                        edges.push(IdentityEdge::rbac(
                            Principal::SearchService,
                            None,
                            "Foundry account (model access)",
                            roles::COGNITIVE_SERVICES_OPENAI_USER,
                            format!("index '{}' has vectorizers", r.name),
                        ));
                    }
                }
                ResourceKind::Agent => {
                    for (kind, name) in registry::extract_references(r.kind, &value) {
                        if kind == ResourceKind::KnowledgeBase {
                            edges.push(IdentityEdge::rbac(
                                Principal::FoundryProject,
                                None,
                                "Search service (knowledge base retrieval)",
                                roles::SEARCH_INDEX_DATA_READER,
                                format!("agent '{}' grounds on knowledge base '{name}'", r.name),
                            ));
                        }
                    }
                }
                _ => {}
            }
            // encryption keys → Key Vault
            if let Some(uri) = value
                .get("encryptionKey")
                .and_then(|k| k.get("keyVaultUri"))
                .and_then(Value::as_str)
            {
                edges.push(IdentityEdge::rbac(
                    Principal::SearchService,
                    None,
                    format!("Key Vault {uri}"),
                    roles::KEY_VAULT_SECRETS_USER,
                    format!(
                        "{} '{}' uses customer-managed encryption",
                        r.kind.display_name(),
                        r.name
                    ),
                ));
            }
        }
    }
    dedup(edges)
}

fn datasource_edges(name: &str, value: &Value, edges: &mut Vec<IdentityEdge>) {
    let ds_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let conn = value
        .get("credentials")
        .and_then(|c| c.get("connectionString"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let scope = parse_resource_id(conn);
    match ds_type {
        "azureblob" | "adlsgen2" | "azurefile" | "azurefiles" | "azuretable" => {
            edges.push(IdentityEdge::rbac(
                Principal::SearchService,
                scope.clone(),
                scope
                    .clone()
                    .unwrap_or_else(|| "storage account (set ResourceId= in the connection string)".into()),
                roles::STORAGE_BLOB_DATA_READER,
                format!("data source '{name}' ({ds_type}) reads from storage"),
            ));
        }
        "cosmosdb" => edges.push(IdentityEdge {
            principal: Principal::SearchService,
            scope: scope.clone(),
            target: scope.unwrap_or_else(|| "Cosmos DB account".into()),
            role_id: String::new(),
            role_name: "Cosmos DB Built-in Data Reader (SQL role)".into(),
            kind: EdgeKind::Informational,
            reason: format!(
                "data source '{name}' reads Cosmos DB — grant via `az cosmosdb sql role assignment create` \
                 (Cosmos data-plane roles are not ARM RBAC)"
            ),
        }),
        "azuresql" => edges.push(IdentityEdge {
            principal: Principal::SearchService,
            scope: scope.clone(),
            target: scope.unwrap_or_else(|| "Azure SQL database".into()),
            role_id: String::new(),
            role_name: "db_datareader (contained AAD user)".into(),
            kind: EdgeKind::Informational,
            reason: format!(
                "data source '{name}' reads Azure SQL — CREATE USER [search-service-name] FROM EXTERNAL PROVIDER; \
                 ALTER ROLE db_datareader ADD MEMBER [...] (SQL AAD users are not ARM RBAC)"
            ),
        }),
        _ => {}
    }
}

/// Parse `ResourceId=/subscriptions/...;<rest>` connection strings.
pub fn parse_resource_id(conn: &str) -> Option<String> {
    let start = conn.find("ResourceId=")? + "ResourceId=".len();
    let rest = &conn[start..];
    let end = rest.find(';').unwrap_or(rest.len());
    let id = rest[..end].trim();
    (id.starts_with("/subscriptions/") && !id.contains('<')).then(|| id.to_string())
}

fn dedup(edges: Vec<IdentityEdge>) -> Vec<IdentityEdge> {
    let mut seen = std::collections::BTreeSet::new();
    edges
        .into_iter()
        .filter(|e| {
            seen.insert((
                format!("{:?}", e.principal),
                e.scope.clone().unwrap_or_else(|| e.target.clone()),
                e.role_id.clone(),
                e.role_name.clone(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::traits::ResourceRef;
    use crate::workspace::{PROJECT_FILE, PROJECTS_DIR, WORKSPACE_FILE};
    use serde_json::json;

    fn ws_with(resources: &[(ResourceKind, &str, Value)]) -> (tempfile::TempDir, Workspace) {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(WORKSPACE_FILE),
            "environments:\n  dev:\n    default: true\n    search: { service: s }\n    foundry: { account: f, project: p }\n",
        )
        .unwrap();
        let pdir = tmp.path().join(PROJECTS_DIR).join("demo");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join(PROJECT_FILE), "{}\n").unwrap();
        let ws = Workspace::load(tmp.path()).unwrap();
        {
            let store = Store::new(ws.project("demo").unwrap(), "dev");
            for (kind, name, value) in resources {
                store.write(&ResourceRef::new(*kind, *name), value).unwrap();
            }
        }
        let ws = Workspace::load(tmp.path()).unwrap();
        (tmp, ws)
    }

    #[test]
    fn blob_datasource_yields_storage_edge_with_scope() {
        let (_tmp, ws) = ws_with(&[(
            ResourceKind::DataSource,
            "ds",
            json!({
                "name": "ds",
                "type": "azureblob",
                "credentials": {"connectionString": "ResourceId=/subscriptions/s1/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct;"},
                "container": {"name": "c"}
            }),
        )]);
        let edges = identity_edges(&ws, "dev");
        assert_eq!(edges.len(), 1);
        let e = &edges[0];
        assert_eq!(e.principal, Principal::SearchService);
        assert_eq!(e.kind, EdgeKind::Rbac);
        assert_eq!(e.role_name, "Storage Blob Data Reader");
        assert_eq!(
            e.scope.as_deref(),
            Some(
                "/subscriptions/s1/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct"
            )
        );
    }

    #[test]
    fn cosmos_and_sql_are_informational() {
        let (_tmp, ws) = ws_with(&[
            (
                ResourceKind::DataSource,
                "cds",
                json!({"name": "cds", "type": "cosmosdb", "credentials": {"connectionString": "ResourceId=/subscriptions/s/resourceGroups/r/providers/Microsoft.DocumentDB/databaseAccounts/c;Database=d"}, "container": {"name": "x"}}),
            ),
            (
                ResourceKind::DataSource,
                "sds",
                json!({"name": "sds", "type": "azuresql", "credentials": {"connectionString": "ResourceId=/subscriptions/s/resourceGroups/r/providers/Microsoft.Sql/servers/sv;Database=d"}, "container": {"name": "t"}}),
            ),
        ]);
        let edges = identity_edges(&ws, "dev");
        assert_eq!(edges.len(), 2);
        assert!(edges.iter().all(|e| e.kind == EdgeKind::Informational));
    }

    #[test]
    fn kb_models_agent_grounding_and_vectorizers() {
        let (_tmp, ws) = ws_with(&[
            (
                ResourceKind::KnowledgeBase,
                "kb",
                json!({"name": "kb", "models": [{"kind": "azureOpenAI"}], "knowledgeSources": [{"name": "ks"}]}),
            ),
            (
                ResourceKind::Agent,
                "agent",
                json!({"name": "agent", "model": "m", "tools": [{"type": "mcp", "x-rigg-ref": "knowledge-bases/kb"}]}),
            ),
            (
                ResourceKind::Index,
                "idx",
                json!({"name": "idx", "fields": [], "vectorSearch": {"vectorizers": [{"name": "v"}]}}),
            ),
        ]);
        let edges = identity_edges(&ws, "dev");
        let roles: Vec<&str> = edges.iter().map(|e| e.role_name.as_str()).collect();
        assert!(roles.contains(&"Cognitive Services User"), "{roles:?}");
        assert!(roles.contains(&"Search Index Data Reader"), "{roles:?}");
        assert!(
            roles.contains(&"Cognitive Services OpenAI User"),
            "{roles:?}"
        );
        let grounding = edges
            .iter()
            .find(|e| e.role_name == "Search Index Data Reader")
            .unwrap();
        assert_eq!(grounding.principal, Principal::FoundryProject);
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

    #[test]
    fn duplicate_edges_dedup() {
        let ds = json!({"name": "ds", "type": "azureblob", "credentials": {"connectionString": "ResourceId=/subscriptions/s/resourceGroups/r/providers/Microsoft.Storage/storageAccounts/a;"}, "container": {"name": "c"}});
        let mut ds2 = ds.clone();
        ds2["name"] = json!("ds2");
        let (_tmp, ws) = ws_with(&[
            (ResourceKind::DataSource, "ds", ds),
            (ResourceKind::DataSource, "ds2", ds2),
        ]);
        assert_eq!(
            identity_edges(&ws, "dev").len(),
            1,
            "same scope+role dedups"
        );
    }
}
