//! MCP tool implementations for rigg (project-scoped, 0.18+).
//!
//! Every tool shells out to the rigg CLI (`rigg ... --output json`) so stdout
//! of this process stays clean for JSON-RPC, and tool behavior is exactly the
//! CLI behavior. Mutating tools follow the preview/execute pattern: without
//! `force` they return a preview, with `force: true` they execute.

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Parameter structs
// ---------------------------------------------------------------------------

#[derive(Deserialize, JsonSchema)]
pub struct ProjectParams {
    /// Project name (omit when the workspace has exactly one project)
    #[schemars(default)]
    pub project: Option<String>,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ValidateParams {
    /// Project name (omit to validate all projects)
    #[schemars(default)]
    pub project: Option<String>,
    /// Enable stricter checks (cross-service reference resolution)
    #[schemars(default)]
    pub strict: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DiffParams {
    /// Project name (omit when the workspace has exactly one project)
    #[schemars(default)]
    pub project: Option<String>,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Restrict to one resource: "<kind-dir>/<name>" (e.g. "indexes/my-index")
    #[schemars(default)]
    pub only: Option<String>,
    /// Compare this environment against another one instead of local files
    #[schemars(default)]
    pub compare_env: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PullParams {
    /// Project name (omit when the workspace has exactly one project)
    #[schemars(default)]
    pub project: Option<String>,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Adopt unmanaged remote resources into this project
    #[schemars(default)]
    pub adopt: Option<bool>,
    /// Without force (default) returns a preview (diff). With force=true, executes the pull.
    #[schemars(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PushParams {
    /// Project name (omit when the workspace has exactly one project)
    #[schemars(default)]
    pub project: Option<String>,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Also delete remote resources whose local files were removed
    #[schemars(default)]
    pub prune: Option<bool>,
    /// Without force (default) returns the push plan (dry run). With force=true, executes.
    #[schemars(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct DeleteParams {
    /// Project whose REMOTE resources should be deleted (local files are kept)
    pub project: String,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Without force (default) returns a preview. With force=true, executes the deletion.
    #[schemars(default)]
    pub force: Option<bool>,
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

impl Default for RiggMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the rigg CLI as a subprocess and return its stdout (plus a note on
/// non-zero exits, mapped to rigg's documented exit codes).
fn rigg_cli(args: &[&str]) -> String {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return format!("Error: cannot locate rigg executable: {e}"),
    };
    let output = std::process::Command::new(exe)
        .args(args)
        .env("RIGG_NO_UPDATE_CHECK", "1")
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let code = out.status.code().unwrap_or(-1);
            match code {
                0 => stdout,
                3 => format!("VALIDATION FAILED (exit 3)\n{stdout}\n{stderr}"),
                4 => format!("AUTH/PERMISSION DENIED (exit 4)\n{stdout}\n{stderr}"),
                5 => format!("DRIFT/CONFLICT DETECTED (exit 5)\n{stdout}\n{stderr}"),
                _ => format!("ERROR (exit {code})\n{stdout}\n{stderr}"),
            }
        }
        Err(e) => format!("Error: failed to run rigg: {e}"),
    }
}

fn with_common<'a>(mut args: Vec<&'a str>, env: &'a Option<String>, json: bool) -> Vec<&'a str> {
    if let Some(env) = env {
        args.push("--env");
        args.push(env);
    }
    if json {
        args.push("--output");
        args.push("json");
    }
    args.push("--quiet");
    args
}

#[tool_router]
impl RiggMcpServer {
    #[tool(
        description = "Show sync status per project: which resources are in sync, local-ahead, remote-ahead, conflicted, plus unmanaged remote resources."
    )]
    async fn rigg_status(&self, Parameters(params): Parameters<ProjectParams>) -> String {
        let mut args = vec!["status"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        rigg_cli(&with_common(args, &params.env, true))
    }

    #[tool(
        description = "Full workspace description: projects, all resources with definitions and file paths, the dependency graph, and 'APIs to implement' (OpenAPI specs in apis/ that skillsets reference). The fastest way to understand the workspace."
    )]
    async fn rigg_describe(&self, Parameters(params): Parameters<ProjectParams>) -> String {
        let mut args = vec!["describe"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        rigg_cli(&with_common(args, &params.env, true))
    }

    #[tool(description = "List all configured deployment environments from rigg.yaml")]
    async fn rigg_env_list(&self) -> String {
        rigg_cli(&["env", "list", "--output", "json", "--quiet"])
    }

    #[tool(
        description = "Validate local files: JSON structure, name/filename consistency, exclusive ownership across projects, reference resolution, no-secrets enforcement, data source types. Exit 3 = problems found."
    )]
    async fn rigg_validate(&self, Parameters(params): Parameters<ValidateParams>) -> String {
        let mut args = vec!["validate"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        if params.strict.unwrap_or(false) {
            args.push("--strict");
        }
        args.extend(["--output", "json", "--quiet"]);
        rigg_cli(&args)
    }

    #[tool(
        description = "Semantic diff of local project files vs live Azure (or one env vs another with compare_env). Volatile server fields are ignored; array order does not matter."
    )]
    async fn rigg_diff(&self, Parameters(params): Parameters<DiffParams>) -> String {
        let mut args = vec!["diff"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        args.extend(["--format", "json"]);
        if let Some(only) = &params.only {
            args.extend(["--only", only]);
        }
        if let Some(ce) = &params.compare_env {
            args.extend(["--compare-env", ce]);
        }
        rigg_cli(&with_common(args, &params.env, false))
    }

    #[tool(
        description = "Pull remote resource definitions into the project's files. Without force: returns the diff (preview). With force=true: executes the pull (--yes). adopt=true instead adopts ALL unmanaged remote resources into the project (equivalent to `rigg adopt <project> all --yes`); for finer-grained adoption (a single kind or resource), use the `rigg adopt <project> <selector>` CLI directly."
    )]
    async fn rigg_pull(&self, Parameters(params): Parameters<PullParams>) -> String {
        if !params.force.unwrap_or(false) {
            let mut args = vec!["diff"];
            if let Some(p) = &params.project {
                args.push(p);
            }
            args.extend(["--format", "json"]);
            let preview = rigg_cli(&with_common(args, &params.env, false));
            return format!(
                "PREVIEW (no changes made) — differences between local and remote:\n{preview}\nRun again with force=true to pull."
            );
        }
        if params.adopt.unwrap_or(false) {
            let project = params.project.as_deref().unwrap_or_default();
            if project.is_empty() {
                return "Error: adopt=true requires an explicit project".to_string();
            }
            let args = vec!["adopt", project, "all", "--yes"];
            return rigg_cli(&with_common(args, &params.env, false));
        }
        let mut args = vec!["pull"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        args.push("--yes");
        rigg_cli(&with_common(args, &params.env, false))
    }

    #[tool(
        description = "Push local project files to Azure in dependency order. Without force: returns the push plan (dry run). With force=true: executes (--yes). prune=true also deletes remote resources whose local files were removed. Always rigg_validate first."
    )]
    async fn rigg_push(&self, Parameters(params): Parameters<PushParams>) -> String {
        let mut args = vec!["push"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        if params.prune.unwrap_or(false) {
            args.push("--prune");
        }
        if params.force.unwrap_or(false) {
            args.push("--yes");
        } else {
            args.push("--dry-run");
        }
        rigg_cli(&with_common(args, &params.env, false))
    }

    #[tool(
        description = "Delete ALL of a project's resources from Azure (local files are kept — pushing re-creates everything). Without force: preview. With force=true: executes. For deleting a single resource: delete its local file, then rigg_push with prune=true."
    )]
    async fn rigg_delete(&self, Parameters(params): Parameters<DeleteParams>) -> String {
        if !params.force.unwrap_or(false) {
            let status = self
                .rigg_status(Parameters(ProjectParams {
                    project: Some(params.project.clone()),
                    env: params.env.clone(),
                }))
                .await;
            return format!(
                "PREVIEW (no changes made) — deleting project '{}' would remove its remote resources. Current state:\n{status}\nRun again with force=true to delete.",
                params.project
            );
        }
        let mut args = vec!["delete", params.project.as_str(), "--remote", "--yes"];
        if let Some(env) = &params.env {
            args.extend(["--env", env]);
        }
        args.push("--quiet");
        rigg_cli(&args)
    }
}

#[tool_handler]
impl ServerHandler for RiggMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            instructions: Some(
                "rigg manages Azure AI Search and Microsoft Foundry configuration as code. \
                 A workspace contains projects; each project owns its resources exclusively, \
                 and pull/push/diff operate on whole projects. Typical flow: rigg_describe to \
                 understand the workspace, rigg_validate before changes, rigg_diff to inspect \
                 drift, rigg_push (preview first, then force=true). Resource definitions are \
                 JSON files under projects/<name>/envs/<env>/{search,foundry}/<kind>/; secrets are never \
                 stored in files — identity-based access only."
                    .to_string(),
            ),
            ..Default::default()
        }
    }
}
