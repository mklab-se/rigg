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
pub struct IndexerStatusParams {
    /// Indexer name
    pub indexer: String,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct IndexerRunParams {
    /// Indexer name
    pub indexer: String,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Without force (default): returns the indexer's current status as a
    /// preview. With force=true: triggers a run (fire-and-forget; poll with
    /// rigg_indexer_status).
    #[schemars(default)]
    pub force: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct QueryParams {
    /// Index name
    pub index: String,
    /// Search text (* matches all documents)
    pub search: String,
    /// Number of results (default 5)
    #[schemars(default)]
    pub top: Option<u32>,
    /// OData filter expression
    #[schemars(default)]
    pub filter: Option<String>,
    /// Comma-separated fields to return
    #[schemars(default)]
    pub select: Option<String>,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct AskParams {
    /// Knowledge base name (exactly one of knowledge_base/agent)
    #[schemars(default)]
    pub knowledge_base: Option<String>,
    /// Agent name (exactly one of knowledge_base/agent)
    #[schemars(default)]
    pub agent: Option<String>,
    /// The prompt / question
    pub prompt: String,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
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
    /// Required consent for protected environments: must equal the environment's name.
    /// Ignored unless force=true.
    #[schemars(default)]
    pub confirm_env: Option<String>,
    /// Required when the plan contains a replace (delete + recreate, e.g. a
    /// knowledge-source kind change): the index is REBUILT from source data
    /// (time, ingestion cost, downtime). Ignored unless force=true.
    #[schemars(default)]
    pub allow_replace: Option<bool>,
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
    /// Required consent for protected environments: must equal the environment's name.
    /// Ignored unless force=true.
    #[schemars(default)]
    pub confirm_env: Option<String>,
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
        description = "Show sync status per project: which resources are in sync, local-ahead, remote-ahead, conflicted, plus unmanaged remote resources. Covers ALL environments unless `env` is set (then just that one). Unreachable environments are reported per env without failing the others."
    )]
    async fn rigg_status(&self, Parameters(params): Parameters<ProjectParams>) -> String {
        let mut args = vec!["status"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        rigg_cli(&with_common(args, &params.env, true))
    }

    #[tool(
        description = "Full workspace description: projects, all resources with definitions and file paths, the dependency graph, and 'APIs to implement' (OpenAPI specs in apis/ that skillsets reference). Scoped to ONE environment (the default unless `env` is set). The fastest way to understand the workspace."
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
        description = "Push local project files to Azure in dependency order. Without force: returns the push plan (dry run). With force=true: executes (--yes). prune=true also deletes remote resources whose local files were removed. Protected environments additionally require confirm_env to match the environment name (matches `rigg push --confirm-env`). Plans containing a replace (e.g. a knowledge-source kind change after `rigg migrate`) additionally require allow_replace=true — the replaced index is rebuilt from source data. Always rigg_validate first."
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
            if let Some(confirm_env) = &params.confirm_env {
                args.extend(["--confirm-env", confirm_env]);
            }
            if params.allow_replace.unwrap_or(false) {
                args.push("--allow-replace");
            }
        } else {
            args.push("--dry-run");
        }
        rigg_cli(&with_common(args, &params.env, false))
    }

    #[tool(
        description = "Execution status of a live indexer: state, last run result, per-document errors and warnings. Use after rigg_push or rigg_indexer_run to verify ingestion. Read-only."
    )]
    async fn rigg_indexer_status(
        &self,
        Parameters(params): Parameters<IndexerStatusParams>,
    ) -> String {
        let args = vec!["az", "indexer", "status", &params.indexer];
        rigg_cli(&with_common(args, &params.env, true))
    }

    #[tool(
        description = "Trigger a live indexer run. Without force: returns the current status as a preview. With force=true: triggers the run (fire-and-forget) — poll rigg_indexer_status until the run completes. Part of the post-push verification flow: rigg_push → rigg_indexer_run → rigg_indexer_status → rigg_query → rigg_ask."
    )]
    async fn rigg_indexer_run(&self, Parameters(params): Parameters<IndexerRunParams>) -> String {
        if !params.force.unwrap_or(false) {
            let args = vec!["az", "indexer", "status", &params.indexer];
            let preview = rigg_cli(&with_common(args, &params.env, true));
            return format!(
                "PREVIEW (no run triggered) — current status:\n{preview}\nRun again with force=true to trigger a run."
            );
        }
        let args = vec!["az", "indexer", "run", &params.indexer, "--yes"];
        rigg_cli(&with_common(args, &params.env, false))
    }

    #[tool(
        description = "Run a search query against a live index (smoke-test retrieval without the portal). Read-only."
    )]
    async fn rigg_query(&self, Parameters(params): Parameters<QueryParams>) -> String {
        let top = params.top.map(|t| t.to_string());
        let mut args = vec!["az", "index", "query", &params.index, &params.search];
        if let Some(top) = &top {
            args.extend(["--top", top]);
        }
        if let Some(filter) = &params.filter {
            args.extend(["--filter", filter]);
        }
        if let Some(select) = &params.select {
            args.extend(["--select", select]);
        }
        rigg_cli(&with_common(args, &params.env, true))
    }

    #[tool(
        description = "Prompt a live knowledge base (agentic retrieval: grounding content + references) or a Foundry agent (single-shot reply). Pass EXACTLY ONE of knowledge_base or agent. Read-only — the end-to-end 'does my RAG stack work' probe."
    )]
    async fn rigg_ask(&self, Parameters(params): Parameters<AskParams>) -> String {
        let args = match (&params.knowledge_base, &params.agent) {
            (Some(kb), None) => vec!["az", "knowledge-base", "ask", kb, &params.prompt],
            (None, Some(agent)) => vec!["az", "agent", "ask", agent, &params.prompt],
            _ => return "Error: pass exactly one of knowledge_base or agent".to_string(),
        };
        rigg_cli(&with_common(args, &params.env, true))
    }

    #[tool(
        description = "Delete ALL of a project's resources from Azure (local files are kept — pushing re-creates everything). Without force: preview. With force=true: executes. Protected environments additionally require confirm_env to match the environment name (matches `rigg delete --confirm-env`). For deleting a single resource: delete its local file, then rigg_push with prune=true."
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
        if let Some(confirm_env) = &params.confirm_env {
            args.extend(["--confirm-env", confirm_env]);
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
