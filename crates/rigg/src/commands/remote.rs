//! Shared remote access for sync commands: one façade over the three clients
//! (Search data plane, Foundry v1 data plane, ARM control plane), created
//! lazily per project + environment.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use tokio::sync::OnceCell;

use rigg_client::arm_resources::ArmResourceClient;
use rigg_client::client::AzureSearchClient;
use rigg_client::error::ClientError;
use rigg_client::foundry::FoundryClient;
use rigg_core::registry::{self, Domain};
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::workspace::{FoundryConnection, Project, ResolvedEnv, SearchConnection};

pub struct Remote {
    search_conn: Option<SearchConnection>,
    foundry_conn: Option<FoundryConnection>,
    search: OnceCell<AzureSearchClient>,
    foundry: OnceCell<FoundryClient>,
    arm: OnceCell<ArmResourceClient>,
}

impl Remote {
    /// Build for a project in an environment. Connections are optional; using
    /// a kind whose connection is missing yields a clear error.
    pub fn for_project(env: &ResolvedEnv, project: &Project) -> Remote {
        Remote {
            search_conn: env.search_for(project).ok().cloned(),
            foundry_conn: env.foundry_for(project).ok().cloned(),
            search: OnceCell::new(),
            foundry: OnceCell::new(),
            arm: OnceCell::new(),
        }
    }

    pub fn has_search(&self) -> bool {
        self.search_conn.is_some()
    }

    pub fn has_foundry(&self) -> bool {
        self.foundry_conn.is_some()
    }

    /// Which kinds are reachable with the configured connections?
    pub fn supported_kinds(&self) -> Vec<ResourceKind> {
        ResourceKind::all()
            .iter()
            .copied()
            .filter(|k| match registry::meta(*k).domain {
                Domain::Search => self.has_search(),
                Domain::FoundryData | Domain::FoundryArm => self.has_foundry(),
            })
            .collect()
    }

    async fn search(&self) -> Result<&AzureSearchClient> {
        let conn = self
            .search_conn
            .as_ref()
            .context("no search connection configured for this project/environment")?;
        self.search
            .get_or_try_init(|| async { Ok(AzureSearchClient::from_connection(conn)?) })
            .await
    }

    async fn foundry(&self) -> Result<&FoundryClient> {
        let conn = self
            .foundry_conn
            .as_ref()
            .context("no foundry connection configured for this project/environment")?;
        self.foundry
            .get_or_try_init(|| async { Ok(FoundryClient::from_connection(conn)?) })
            .await
    }

    async fn arm(&self) -> Result<&ArmResourceClient> {
        let conn = self
            .foundry_conn
            .as_ref()
            .context("no foundry connection configured for this project/environment")?;
        self.arm
            .get_or_try_init(|| async {
                ArmResourceClient::for_account(&conn.account, &conn.project)
                    .await
                    .map_err(anyhow::Error::from)
            })
            .await
    }

    /// GET one resource; Ok(None) when it does not exist remotely.
    pub async fn get(&self, r: &ResourceRef) -> Result<Option<Value>> {
        match registry::meta(r.kind).domain {
            Domain::Search => match self.search().await?.get(r.kind, &r.name).await {
                Ok(v) => Ok(Some(v)),
                Err(ClientError::NotFound { .. }) => Ok(None),
                Err(e) => Err(e.into()),
            },
            Domain::FoundryData => match self.foundry().await?.get_agent(&r.name).await {
                Ok(v) => Ok(Some(v)),
                Err(ClientError::NotFound { .. }) => Ok(None),
                Err(e) => Err(e.into()),
            },
            Domain::FoundryArm => Ok(self.arm().await?.get(r.kind, &r.name).await?),
        }
    }

    /// List all resources of a kind. Every returned item carries a "name".
    pub async fn list(&self, kind: ResourceKind) -> Result<Vec<Value>> {
        let items = match registry::meta(kind).domain {
            Domain::Search => self.search().await?.list(kind).await?,
            Domain::FoundryData => self.foundry().await?.list_agents().await?,
            Domain::FoundryArm => self.arm().await?.list(kind).await?,
        };
        Ok(items)
    }

    /// Create or update; returns the server's post-write document
    /// (GETs it back when the API returns 204/no body) for canonicalization.
    pub async fn put(&self, r: &ResourceRef, body: &Value) -> Result<Value> {
        match registry::meta(r.kind).domain {
            Domain::Search => {
                let client = self.search().await?;
                match client.create_or_update(r.kind, &r.name, body).await? {
                    Some(v) => Ok(v),
                    None => client.get(r.kind, &r.name).await.map_err(Into::into),
                }
            }
            Domain::FoundryData => {
                let client = self.foundry().await?;
                let exists = client.get_agent(&r.name).await.is_ok();
                let result = if exists {
                    client.update_agent(&r.name, body).await?
                } else {
                    client.create_agent(body).await?
                };
                Ok(result)
            }
            Domain::FoundryArm => Ok(self.arm().await?.put(r.kind, &r.name, body).await?),
        }
    }

    /// Delete a remote resource (idempotent: missing is not an error).
    pub async fn delete(&self, r: &ResourceRef) -> Result<()> {
        let result = match registry::meta(r.kind).domain {
            Domain::Search => self.search().await?.delete(r.kind, &r.name).await,
            Domain::FoundryData => self.foundry().await?.delete_agent(&r.name).await,
            Domain::FoundryArm => return Ok(self.arm().await?.delete(r.kind, &r.name).await?),
        };
        match result {
            Ok(()) => Ok(()),
            Err(ClientError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Fetch every remote resource of the supported kinds as (ref, doc).
    pub async fn snapshot(&self) -> Result<Vec<(ResourceRef, Value)>> {
        let mut out = Vec::new();
        for kind in self.supported_kinds() {
            let items = self
                .list(kind)
                .await
                .with_context(|| format!("failed to list remote {}", kind.directory_name()))?;
            for item in items {
                let Some(name) = item.get("name").and_then(Value::as_str) else {
                    continue;
                };
                if rigg_core::resources::validate_resource_name(name).is_err() {
                    continue;
                }
                out.push((ResourceRef::new(kind, name.to_string()), item));
            }
        }
        Ok(out)
    }
}

/// Resolve environment-specific values into a push body: `x-rigg-ref`
/// annotations that point at knowledge bases inject the KB's MCP endpoint
/// into a sibling `server_url`/`url` field when empty.
pub fn resolve_cross_service_refs(
    env_search: Option<&SearchConnection>,
    body: &mut Value,
) -> Result<()> {
    let Some(search) = env_search else {
        return Ok(());
    };
    resolve_walk(&search.service, body)
}

fn resolve_walk(search_service: &str, value: &mut Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            let kb_ref = map
                .get(registry::X_RIGG_REF)
                .and_then(Value::as_str)
                .and_then(|s| s.split_once('/'))
                .filter(|(dir, _)| *dir == "knowledge-bases")
                .map(|(_, name)| name.to_string());
            if let Some(kb) = kb_ref {
                // The knowledge base exposes an MCP endpoint for agent grounding.
                let mcp_url = format!(
                    "https://{search_service}.search.windows.net/knowledgeBases('{kb}')/mcp?api-version={}",
                    registry::SEARCH_STABLE_API_VERSION
                );
                for field in ["server_url", "url", "endpoint"] {
                    let empty = map
                        .get(field)
                        .map(|v| v.as_str().is_none_or(str::is_empty))
                        .unwrap_or(field == "server_url");
                    if empty {
                        map.insert(field.to_string(), Value::String(mcp_url.clone()));
                        break;
                    }
                }
            }
            for (_, v) in map.iter_mut() {
                resolve_walk(search_service, v)?;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                resolve_walk(search_service, item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Bail with a helpful error when a project has no usable connections.
pub fn ensure_any_connection(remote: &Remote, project: &Project) -> Result<()> {
    if !remote.has_search() && !remote.has_foundry() {
        bail!(
            "project '{}' has no reachable services in this environment (configure `search:` or `foundry:` in rigg.yaml)",
            project.name
        );
    }
    Ok(())
}
