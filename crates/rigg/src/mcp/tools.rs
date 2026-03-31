//! MCP tool implementations for rigg

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

use rigg_core::resources::ResourceKind;
use rigg_core::service::ServiceDomain;

use crate::commands;

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct EnvParam {
    /// Environment name (uses default if omitted)
    #[schemars(default)]
    pub env: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ValidateParams {
    /// Environment name (uses default if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Enable strict validation (warn on unknown fields)
    #[schemars(default)]
    pub strict: Option<bool>,
    /// Verify that cross-resource references are valid
    #[schemars(default)]
    pub check_references: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResourceParams {
    /// Environment name (uses default if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Resource type filter: "indexes", "indexers", "datasources", "skillsets",
    /// "synonymmaps", "aliases", "knowledgebases", "knowledgesources", "agents", "all"
    #[schemars(default)]
    pub resource_type: Option<String>,
    /// Specific resource name to operate on
    #[schemars(default)]
    pub name: Option<String>,
    /// Generate AI-enhanced explanations of changes (requires ai: config in rigg.yaml)
    #[schemars(default)]
    pub explain: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct MutatingParams {
    /// Environment name (uses default if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Resource type filter: "indexes", "indexers", "datasources", "skillsets",
    /// "synonymmaps", "aliases", "knowledgebases", "knowledgesources", "agents", "all"
    #[schemars(default)]
    pub resource_type: Option<String>,
    /// Specific resource name to operate on
    #[schemars(default)]
    pub name: Option<String>,
    /// When true, execute immediately. When false (default), return a preview of what would happen.
    #[schemars(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeleteParams {
    /// Environment name (uses default if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Resource type: "indexes", "indexers", "datasources", "skillsets",
    /// "synonymmaps", "aliases", "knowledgebases", "knowledgesources", "agents"
    pub resource_type: String,
    /// Name of the resource to delete
    pub name: String,
    /// Where to delete: "remote" (from Azure) or "local" (remove local files)
    pub target: String,
    /// When true, execute the deletion. When false (default), return a preview of what would happen.
    #[schemars(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ListParams {
    /// Environment name (uses default if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Resource type filter: "indexes", "indexers", "datasources", "skillsets",
    /// "synonymmaps", "aliases", "knowledgebases", "knowledgesources", "agents"
    #[schemars(default)]
    pub resource_type: Option<String>,
    /// Where to list from: "local" (disk, default), "remote" (Azure), "both"
    #[schemars(default)]
    pub source: Option<String>,
}

// ---------------------------------------------------------------------------
// Server implementation
// ---------------------------------------------------------------------------

/// The rigg MCP server
#[derive(Clone)]
pub struct RiggMcpServer {
    tool_router: ToolRouter<Self>,
}

impl RiggMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl RiggMcpServer {
    /// Show rigg project status including environment, auth, and resource counts.
    #[tool(
        description = "Show rigg project status: environment info, auth state, resource counts, and last sync time"
    )]
    async fn rigg_status(&self, Parameters(params): Parameters<EnvParam>) -> String {
        match build_status(params.env.as_deref()) {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Get a complete project description with all resources, dependencies, and agent configurations.
    #[tool(
        description = "Full project description: all resources, dependencies, agent configs, knowledge base flows. This is the fastest way to understand the entire project."
    )]
    async fn rigg_describe(&self, Parameters(params): Parameters<EnvParam>) -> String {
        match build_describe(params.env.as_deref()) {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// List all configured environments.
    #[tool(description = "List all configured deployment environments from rigg.yaml")]
    async fn rigg_env_list(&self) -> String {
        match build_env_list() {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Validate local resource files for structural and referential integrity.
    #[tool(
        description = "Validate local resource files: check JSON/YAML syntax, name consistency, and cross-resource references"
    )]
    async fn rigg_validate(&self, Parameters(params): Parameters<ValidateParams>) -> String {
        match build_validate(
            params.env.as_deref(),
            params.strict.unwrap_or(false),
            params.check_references.unwrap_or(false),
        ) {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// List resource names by type. Fast enumeration without full details.
    #[tool(
        description = "List resource names by type. Use source='local' for disk scan (fast, no network), 'remote' for Azure, or 'both' to find drift."
    )]
    async fn rigg_list(&self, Parameters(params): Parameters<ListParams>) -> String {
        match build_list(
            params.env.as_deref(),
            params.resource_type.as_deref(),
            params.source.as_deref().unwrap_or("local"),
        )
        .await
        {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Compare local resource files against live Azure services (diff).
    #[tool(
        description = "Compare local resource files against live Azure. Returns enhanced JSON with resource_type, status, summary, and per-change descriptions. Use resource_type and name to narrow scope. NOTE: Knowledge source changes may require delete-and-recreate due to a known Azure limitation."
    )]
    async fn rigg_diff(&self, Parameters(params): Parameters<ResourceParams>) -> String {
        match run_diff(
            params.env.as_deref(),
            params.resource_type.as_deref(),
            params.name.as_deref(),
            params.explain.unwrap_or(false),
        )
        .await
        {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Pull resources from Azure to local files.
    #[tool(
        description = "Pull resource definitions from Azure to local files. Without force, returns a preview. With force=true, executes the pull. Knowledge sources and their managed sub-resources (index, indexer, data source, skillset) are pulled together automatically."
    )]
    async fn rigg_pull(&self, Parameters(params): Parameters<MutatingParams>) -> String {
        match run_pull(
            params.env.as_deref(),
            params.resource_type.as_deref(),
            params.name.as_deref(),
            params.force.unwrap_or(false),
        )
        .await
        {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Push local resource files to Azure.
    #[tool(
        description = "Push local resource changes to Azure. Without force, returns a preview of what would change. With force=true, executes the push. Always validate and diff first. IMPORTANT: When pushing knowledge sources (--knowledgesources), rigg automatically handles all managed sub-resources (index, indexer, data source, skillset) — do NOT push these sub-resources separately. Knowledge source updates may fail due to a known Azure limitation where Azure tries to recreate managed sub-resources. If this happens, use rigg_delete to delete the knowledge source from Azure first, then push again."
    )]
    async fn rigg_push(&self, Parameters(params): Parameters<MutatingParams>) -> String {
        match run_push(
            params.env.as_deref(),
            params.resource_type.as_deref(),
            params.name.as_deref(),
            params.force.unwrap_or(false),
        )
        .await
        {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }

    /// Delete a resource from Azure or remove local files.
    #[tool(
        description = "Delete a resource. target='remote' deletes from Azure only (local files kept). target='local' removes local files only (Azure untouched). Without force, returns a preview. With force=true, executes. IMPORTANT: Local files are shared across all environments — removing them locally affects all environments. After deleting, use rigg_push or rigg_pull to sync. For knowledge sources, deleting from Azure also removes managed sub-resources (index, indexer, data source, skillset) and all indexed data."
    )]
    async fn rigg_delete(&self, Parameters(params): Parameters<DeleteParams>) -> String {
        match run_delete(
            params.env.as_deref(),
            &params.resource_type,
            &params.name,
            &params.target,
            params.force.unwrap_or(false),
        )
        .await
        {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap(),
            Err(e) => format!("Error: {}", e),
        }
    }
}

#[tool_handler]
impl ServerHandler for RiggMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "rigg manages Azure AI Search and Microsoft Foundry configuration as code. \
                 Use rigg_describe for a complete project overview, rigg_status for environment info, \
                 and rigg_diff/rigg_pull/rigg_push for syncing with Azure. \
                 \
                 ENVIRONMENTS: All tools accept an optional 'env' parameter. If omitted, the default \
                 environment is used. Use rigg_env_list to see all configured environments. Always \
                 pass 'env' explicitly when operating on non-default environments. \
                 \
                 IMPORTANT — Knowledge Source managed sub-resources: \
                 When you push knowledge sources (resource_type='knowledgesources'), rigg automatically \
                 handles all managed sub-resources (index, indexer, data source, skillset) in the correct \
                 order. Do NOT create or push these sub-resources separately — they are managed by Azure \
                 as part of the knowledge source lifecycle. If you need to modify a managed index schema \
                 or skillset, edit the corresponding file in the knowledge source directory and push with \
                 resource_type='knowledgesources'. \
                 \
                 Known Azure limitation: Updating an existing knowledge source may fail because Azure \
                 tries to recreate managed sub-resources that already exist. Workaround: use rigg_delete \
                 with target='remote' to delete the knowledge source from Azure, then use rigg_push to \
                 recreate it. \
                 \
                 DELETING RESOURCES — use rigg_delete: \
                 target='remote': deletes from the Azure service only (local files are NOT affected). \
                 target='local': removes local files only (Azure resources are NOT affected). \
                 Local files are shared across all environments — removing them locally affects all envs. \
                 After deleting, use rigg_push or rigg_pull to sync."
                    .into(),
            ),
            server_info: rmcp::model::Implementation {
                name: "rigg".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            },
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Helper functions (return serde_json::Value for MCP tools)
// ---------------------------------------------------------------------------

fn build_status(env_override: Option<&str>) -> anyhow::Result<serde_json::Value> {
    let (project_root, config, env) = commands::load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);
    let state = rigg_core::state::LocalState::load_env(&project_root, &env.name)?;

    let mut resource_counts = serde_json::Map::new();
    let mut total = 0;

    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_root, search_svc);
        for kind in ResourceKind::stable() {
            let dir = search_base.join(kind.directory_name());
            let count = count_json_files(&dir);
            let entry = resource_counts
                .entry(kind.display_name().to_string())
                .or_insert(serde_json::json!(0));
            *entry = serde_json::json!(entry.as_u64().unwrap_or(0) + count as u64);
            total += count;
        }
        if env.sync.include_preview {
            let kb_dir = search_base.join(ResourceKind::KnowledgeBase.directory_name());
            let kb_count = count_json_files(&kb_dir);
            let kb_entry = resource_counts
                .entry(ResourceKind::KnowledgeBase.display_name().to_string())
                .or_insert(serde_json::json!(0));
            *kb_entry = serde_json::json!(kb_entry.as_u64().unwrap_or(0) + kb_count as u64);
            total += kb_count;

            let ks_dir = search_base.join(ResourceKind::KnowledgeSource.directory_name());
            let ks_count = count_subdirs(&ks_dir);
            let ks_entry = resource_counts
                .entry(ResourceKind::KnowledgeSource.display_name().to_string())
                .or_insert(serde_json::json!(0));
            *ks_entry = serde_json::json!(ks_entry.as_u64().unwrap_or(0) + ks_count as u64);
            total += ks_count;
        }
    }

    if env.has_foundry() {
        let mut agent_total = 0;
        for foundry_config in &env.foundry {
            let agents_dir = env
                .foundry_service_dir(&files_root, foundry_config)
                .join("agents");
            agent_total += count_yaml_files(&agents_dir);
        }
        resource_counts.insert("Agent".to_string(), serde_json::json!(agent_total));
        total += agent_total;
    }

    let auth_status = match rigg_client::auth::get_auth_provider() {
        Ok(provider) => match provider.get_token() {
            Ok(_) => format!("OK ({})", provider.method_name()),
            Err(e) => format!("Failed - {}", e),
        },
        Err(e) => format!("Not configured - {}", e),
    };

    let last_sync = state
        .last_sync
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string());

    let primary = env.primary_search_service();
    let service_name = primary.map(|s| s.name.as_str()).unwrap_or("(none)");

    let mut status = serde_json::json!({
        "project_root": project_root.display().to_string(),
        "environment": env.name,
        "service_name": service_name,
        "include_preview": env.sync.include_preview,
        "last_sync": last_sync,
        "resources": resource_counts,
        "total_resources": total,
        "authentication": auth_status,
    });
    if let Some(svc) = primary {
        status["service_url"] = serde_json::json!(svc.service_url());
        status["api_version"] = serde_json::json!(&svc.api_version);
        status["preview_api_version"] = serde_json::json!(&svc.preview_api_version);
    }

    Ok(status)
}

fn build_describe(env_override: Option<&str>) -> anyhow::Result<serde_json::Value> {
    let (project_root, config, env) = commands::load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    let output = std::process::Command::new(std::env::current_exe()?)
        .args(["describe", "--output", "json"])
        .env("RIGG_ENV", env_override.unwrap_or(&env.name))
        .current_dir(&project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()?;

    if output.status.success() {
        let json_str = String::from_utf8_lossy(&output.stdout);
        let value: serde_json::Value = serde_json::from_str(&json_str)
            .unwrap_or_else(|_| serde_json::json!({"error": "Failed to parse describe output"}));

        let mut result = value;
        result["files_root"] = serde_json::json!(files_root.display().to_string());
        result["project_root"] = serde_json::json!(project_root.display().to_string());
        Ok(result)
    } else {
        anyhow::bail!(
            "rigg describe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    }
}

fn build_env_list() -> anyhow::Result<serde_json::Value> {
    let (_project_root, config) = commands::load_config()?;

    let mut envs = Vec::new();
    for (name, env_config) in &config.environments {
        let mut entry = serde_json::json!({
            "name": name,
            "default": env_config.default,
        });

        let search_services: Vec<serde_json::Value> = env_config
            .search
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "label": s.label,
                })
            })
            .collect();

        let foundry_services: Vec<serde_json::Value> = env_config
            .foundry
            .iter()
            .map(|f| {
                serde_json::json!({
                    "name": f.name,
                    "project": f.project,
                    "label": f.label,
                })
            })
            .collect();

        entry["search"] = serde_json::json!(search_services);
        entry["foundry"] = serde_json::json!(foundry_services);
        envs.push(entry);
    }

    Ok(serde_json::json!({ "environments": envs }))
}

fn build_validate(
    env_override: Option<&str>,
    strict: bool,
    check_references: bool,
) -> anyhow::Result<serde_json::Value> {
    let (project_root, _config, env) = commands::load_config_and_env(env_override)?;

    let mut cmd = std::process::Command::new(std::env::current_exe()?);
    cmd.args(["validate", "--output", "json"])
        .env("RIGG_ENV", env_override.unwrap_or(&env.name))
        .current_dir(&project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());

    if strict {
        cmd.arg("--strict");
    }
    if check_references {
        cmd.arg("--check-references");
    }

    let output = cmd.output()?;
    let json_str = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&json_str)
        .unwrap_or_else(|_| serde_json::json!({"error": "Failed to parse validate output", "raw": json_str.to_string()}));

    Ok(value)
}

async fn build_list(
    env_override: Option<&str>,
    resource_type: Option<&str>,
    source: &str,
) -> anyhow::Result<serde_json::Value> {
    let (project_root, config, env) = commands::load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    let kinds = resolve_resource_types(resource_type, env.sync.include_preview);
    let mut results = Vec::new();

    if source == "local" || source == "both" {
        for kind in &kinds {
            if kind.domain() == ServiceDomain::Search {
                for search_svc in &env.search {
                    let search_base = env.search_service_dir(&files_root, search_svc);
                    let names = list_local_resources(&search_base, *kind);
                    for name in names {
                        results.push(serde_json::json!({
                            "name": name,
                            "kind": kind.display_name(),
                            "source": "local",
                        }));
                    }
                }
            } else if kind.domain() == ServiceDomain::Foundry {
                for foundry_config in &env.foundry {
                    let agents_dir = env
                        .foundry_service_dir(&files_root, foundry_config)
                        .join("agents");
                    if agents_dir.exists() {
                        if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                            for entry in entries.flatten() {
                                let path = entry.path();
                                if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                                    if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                                        results.push(serde_json::json!({
                                            "name": name,
                                            "kind": "Agent",
                                            "source": "local",
                                        }));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if source == "remote" || source == "both" {
        let search_kinds: Vec<ResourceKind> = kinds
            .iter()
            .filter(|k| k.domain() == ServiceDomain::Search)
            .copied()
            .collect();
        let foundry_kinds: Vec<ResourceKind> = kinds
            .iter()
            .filter(|k| k.domain() == ServiceDomain::Foundry)
            .copied()
            .collect();

        if !search_kinds.is_empty() {
            if let Some(search_svc) = env.primary_search_service() {
                let client = rigg_client::AzureSearchClient::from_service_config(search_svc)?;
                for kind in &search_kinds {
                    match client.list(*kind).await {
                        Ok(resources) => {
                            for r in &resources {
                                if let Some(name) = r.get("name").and_then(|n| n.as_str()) {
                                    results.push(serde_json::json!({
                                        "name": name,
                                        "kind": kind.display_name(),
                                        "source": "remote",
                                    }));
                                }
                            }
                        }
                        Err(e) => {
                            results.push(serde_json::json!({
                                "error": format!("Failed to list {}: {}", kind.display_name(), e),
                                "kind": kind.display_name(),
                                "source": "remote",
                            }));
                        }
                    }
                }
            }
        }

        if !foundry_kinds.is_empty() && env.has_foundry() {
            for foundry_config in &env.foundry {
                let client = rigg_client::FoundryClient::new(foundry_config)?;
                match client.list_agents().await {
                    Ok(agents) => {
                        for a in &agents {
                            if let Some(name) = a.get("name").and_then(|n| n.as_str()) {
                                results.push(serde_json::json!({
                                    "name": name,
                                    "kind": "Agent",
                                    "source": "remote",
                                }));
                            }
                        }
                    }
                    Err(e) => {
                        results.push(serde_json::json!({
                            "error": format!("Failed to list agents: {}", e),
                            "kind": "Agent",
                            "source": "remote",
                        }));
                    }
                }
            }
        }
    }

    Ok(serde_json::json!({ "resources": results }))
}

async fn run_diff(
    env_override: Option<&str>,
    resource_type: Option<&str>,
    name: Option<&str>,
    explain: bool,
) -> anyhow::Result<serde_json::Value> {
    let (project_root, _config, env) = commands::load_config_and_env(env_override)?;

    let mut args = vec!["diff".into(), "--format".into(), "json".into()];
    if explain {
        args.push("--explain".into());
    } else {
        args.push("--no-explain".into());
    }
    add_resource_type_args(resource_type, name, &mut args);

    let output = std::process::Command::new(std::env::current_exe()?)
        .args(&args)
        .env("RIGG_ENV", env_override.unwrap_or(&env.name))
        .current_dir(&project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()?;

    let json_str = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&json_str)
        .unwrap_or_else(|_| serde_json::json!({"output": json_str.to_string()}));

    Ok(value)
}

async fn run_pull(
    env_override: Option<&str>,
    resource_type: Option<&str>,
    name: Option<&str>,
    force: bool,
) -> anyhow::Result<serde_json::Value> {
    if !force {
        let diff_result = run_diff(env_override, resource_type, name, false).await?;
        return Ok(serde_json::json!({
            "preview": true,
            "message": "Preview of what would be pulled. Re-run with force=true to execute.",
            "diff": diff_result,
        }));
    }

    let (project_root, _config, env) = commands::load_config_and_env(env_override)?;

    let mut args = vec!["pull".into(), "--force".into()];
    add_resource_type_args(resource_type, name, &mut args);

    let output = std::process::Command::new(std::env::current_exe()?)
        .args(&args)
        .env("RIGG_ENV", env_override.unwrap_or(&env.name))
        .current_dir(&project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(serde_json::json!({
        "success": output.status.success(),
        "output": stdout.to_string(),
        "errors": if stderr.is_empty() { None } else { Some(stderr.to_string()) },
    }))
}

async fn run_push(
    env_override: Option<&str>,
    resource_type: Option<&str>,
    name: Option<&str>,
    force: bool,
) -> anyhow::Result<serde_json::Value> {
    if !force {
        let diff_result = run_diff(env_override, resource_type, name, false).await?;
        return Ok(serde_json::json!({
            "preview": true,
            "message": "Preview of what would be pushed. Re-run with force=true to execute.",
            "diff": diff_result,
        }));
    }

    let (project_root, _config, env) = commands::load_config_and_env(env_override)?;

    let mut args = vec!["push".into(), "--force".into()];
    add_resource_type_args(resource_type, name, &mut args);

    let output = std::process::Command::new(std::env::current_exe()?)
        .args(&args)
        .env("RIGG_ENV", env_override.unwrap_or(&env.name))
        .current_dir(&project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(serde_json::json!({
        "success": output.status.success(),
        "output": stdout.to_string(),
        "errors": if stderr.is_empty() { None } else { Some(stderr.to_string()) },
    }))
}

async fn run_delete(
    env_override: Option<&str>,
    resource_type: &str,
    name: &str,
    target: &str,
    force: bool,
) -> anyhow::Result<serde_json::Value> {
    // Validate target
    if target != "remote" && target != "local" {
        return Ok(serde_json::json!({
            "error": "Invalid target. Must be 'remote' (delete from Azure) or 'local' (remove local files)."
        }));
    }

    // Resolve the singular CLI flag for the resource type
    let flag = match resource_type {
        "indexes" => "--index",
        "indexers" => "--indexer",
        "datasources" => "--datasource",
        "skillsets" => "--skillset",
        "synonymmaps" => "--synonymmap",
        "aliases" => "--alias",
        "knowledgebases" => "--knowledgebase",
        "knowledgesources" => "--knowledgesource",
        "agents" => "--agent",
        _ => {
            return Ok(serde_json::json!({
                "error": format!("Unknown resource type '{}'. Valid types: indexes, indexers, datasources, skillsets, synonymmaps, aliases, knowledgebases, knowledgesources, agents", resource_type)
            }));
        }
    };

    let (project_root, _config, env) = commands::load_config_and_env(env_override)?;

    if !force {
        // Preview mode: describe what would happen
        let mut preview = serde_json::json!({
            "preview": true,
            "environment": env.name,
            "resource_type": resource_type,
            "name": name,
            "target": target,
        });

        if target == "remote" {
            let service_name = if resource_type == "agents" {
                env.foundry
                    .first()
                    .map(|f| f.name.as_str())
                    .unwrap_or("(unknown)")
            } else {
                env.primary_search_service()
                    .map(|s| s.name.as_str())
                    .unwrap_or("(unknown)")
            };
            preview["message"] = serde_json::json!(format!(
                "Would delete {} '{}' from {} (environment '{}').",
                resource_type.trim_end_matches('s'),
                name,
                service_name,
                env.name
            ));
            preview["service"] = serde_json::json!(service_name);

            if resource_type == "knowledgesources" {
                preview["warning"] = serde_json::json!(
                    "Deleting a knowledge source from Azure also deletes its managed sub-resources (index, indexer, data source, skillset) and all indexed data."
                );
            }
        } else {
            preview["message"] = serde_json::json!(format!(
                "Would remove local files for {} '{}'. Local files are shared across all environments.",
                resource_type.trim_end_matches('s'),
                name,
            ));
        }

        preview["instruction"] =
            serde_json::json!("Re-run with force=true to execute the deletion.");
        return Ok(preview);
    }

    // Execute mode
    let mut args: Vec<String> = vec![
        "delete".into(),
        flag.into(),
        name.into(),
        "--target".into(),
        target.into(),
        "--force".into(),
    ];

    if let Some(env_name) = env_override {
        args.push("--env".into());
        args.push(env_name.into());
    }

    let output = std::process::Command::new(std::env::current_exe()?)
        .args(&args)
        .env("RIGG_ENV", env_override.unwrap_or(&env.name))
        .current_dir(&project_root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(serde_json::json!({
        "success": output.status.success(),
        "environment": env.name,
        "target": target,
        "output": stdout.to_string(),
        "errors": if stderr.is_empty() { None } else { Some(stderr.to_string()) },
    }))
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

fn count_json_files(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
                .count()
        })
        .unwrap_or(0)
}

fn count_yaml_files(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("yaml"))
                .count()
        })
        .unwrap_or(0)
}

fn count_subdirs(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

fn list_local_resources(search_base: &std::path::Path, kind: ResourceKind) -> Vec<String> {
    let resource_dir = search_base.join(kind.directory_name());
    if !resource_dir.exists() {
        return Vec::new();
    }

    if kind == ResourceKind::KnowledgeSource {
        std::fs::read_dir(&resource_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().is_dir())
                    .filter_map(|e| e.file_name().to_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::fs::read_dir(&resource_dir)
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
                    .filter_map(|e| {
                        e.path()
                            .file_stem()
                            .and_then(|n| n.to_str())
                            .map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn resolve_resource_types(resource_type: Option<&str>, include_preview: bool) -> Vec<ResourceKind> {
    match resource_type {
        None | Some("all") => {
            if include_preview {
                ResourceKind::all().to_vec()
            } else {
                ResourceKind::stable().to_vec()
            }
        }
        Some(rt) => match rt {
            "indexes" => vec![ResourceKind::Index],
            "indexers" => vec![ResourceKind::Indexer],
            "datasources" => vec![ResourceKind::DataSource],
            "skillsets" => vec![ResourceKind::Skillset],
            "synonymmaps" => vec![ResourceKind::SynonymMap],
            "aliases" => vec![ResourceKind::Alias],
            "knowledgebases" => vec![ResourceKind::KnowledgeBase],
            "knowledgesources" => vec![ResourceKind::KnowledgeSource],
            "agents" => vec![ResourceKind::Agent],
            _ => Vec::new(),
        },
    }
}

fn add_resource_type_args(resource_type: Option<&str>, name: Option<&str>, args: &mut Vec<String>) {
    match (resource_type, name) {
        // When both type and name are given, use singular flag: --index <name>
        (Some(rt), Some(n)) => {
            let flag = match rt {
                "indexes" => "--index",
                "indexers" => "--indexer",
                "datasources" => "--datasource",
                "skillsets" => "--skillset",
                "synonymmaps" => "--synonymmap",
                "aliases" => "--alias",
                "knowledgebases" => "--knowledgebase",
                "knowledgesources" => "--knowledgesource",
                "agents" => "--agent",
                _ => {
                    args.push("--all".into());
                    return;
                }
            };
            args.push(flag.into());
            args.push(n.into());
        }
        // Type only: use plural flag
        (Some(rt), None) => {
            let flag = match rt {
                "indexes" => "--indexes",
                "indexers" => "--indexers",
                "datasources" => "--datasources",
                "skillsets" => "--skillsets",
                "synonymmaps" => "--synonymmaps",
                "aliases" => "--aliases",
                "knowledgebases" => "--knowledgebases",
                "knowledgesources" => "--knowledgesources",
                "agents" => "--agents",
                _ => "--all",
            };
            args.push(flag.into());
        }
        // No type: default to all
        (None, _) => args.push("--all".into()),
    }
}
