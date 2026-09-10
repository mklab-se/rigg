//! MCP tool implementations for rigg (project-scoped, 0.18+).
//!
//! Every tool shells out to the rigg CLI (`rigg ... --output json`) so stdout
//! of this process stays clean for JSON-RPC, and tool behavior is exactly the
//! CLI behavior. Mutating tools follow the preview/execute pattern: without
//! `force` they return a preview, with `force: true` they execute.
//!
//! The needs-input loop: a guided flow (e.g. the protected-environment gate)
//! that needs an answer it cannot prompt for makes the underlying `rigg`
//! subprocess exit 6 with a `needs-input` JSON document on stdout; `rigg_cli`
//! passes that document through unchanged as the tool result (not an error —
//! see its exit-6 arm). A caller answers by re-calling the same tool with
//! `answers` (question id → value) filled in, which is threaded through to
//! `--answer <id>=<value>` on the CLI invocation via `with_common_answers`.

use std::collections::BTreeMap;

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
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
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
    /// Required consent for protected environments: must equal the environment's name.
    /// Ignored unless force=true.
    #[schemars(default)]
    pub confirm_env: Option<String>,
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
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
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
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
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
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
    /// After the push, prove the stack works: run every indexer to
    /// completion, retrieve from every knowledge base, ask every agent.
    /// Takes as long as ingestion takes. Ignored unless force=true.
    #[schemars(default)]
    pub verify: Option<bool>,
    /// Skip the identity/RBAC preflight that runs before anything is
    /// written. Only for a caller that cannot read Azure Resource Manager.
    #[schemars(default)]
    pub skip_auth_preflight: Option<bool>,
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct VerifyParams {
    /// Project name (omit when the workspace has exactly one project)
    #[schemars(default)]
    pub project: Option<String>,
    /// Verify every project in the workspace
    #[schemars(default)]
    pub all: Option<bool>,
    /// Environment name (uses the default environment if omitted)
    #[schemars(default)]
    pub env: Option<String>,
    /// Required consent for protected environments: must equal the environment's name.
    #[schemars(default)]
    pub confirm_env: Option<String>,
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct PromoteParams {
    /// Project name (omit when the workspace has exactly one project)
    #[schemars(default)]
    pub project: Option<String>,
    /// Source environment
    pub from: String,
    /// Target environment
    pub to: String,
    /// Without force (default): returns the rewiring/resources/checks preview
    /// (--dry-run), writing nothing. With force=true: writes the translated
    /// files (--yes).
    #[schemars(default)]
    pub force: Option<bool>,
    /// Skip every Azure lookup (candidate lists, Web API auth re-derivation,
    /// deployment availability); unresolved items are reported instead.
    #[schemars(default)]
    pub offline: Option<bool>,
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
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
    /// Answers to questions a previous call returned as `needs-input` (id → value)
    #[schemars(default)]
    pub answers: Option<BTreeMap<String, String>>,
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

    /// Every tool this server exposes, name-sorted — exactly what a client
    /// gets from `tools/list`. Used by `rigg mcp tools` to generate the
    /// `MCP.md` table so the docs cannot describe a tool that is not here.
    pub fn tool_list(&self) -> Vec<rmcp::model::Tool> {
        self.tool_router.list_all()
    }
}

impl Default for RiggMcpServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Run the rigg CLI as a subprocess and return its stdout (plus a note on
/// non-zero exits, mapped to rigg's documented exit codes).
fn rigg_cli<S: AsRef<str>>(args: &[S]) -> String {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(e) => return format!("Error: cannot locate rigg executable: {e}"),
    };
    let output = std::process::Command::new(exe)
        .args(args.iter().map(S::as_ref))
        .env("RIGG_NO_UPDATE_CHECK", "1")
        .output();
    match output {
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            let code = out.status.code().unwrap_or(-1);
            match code {
                0 => stdout,
                // needs-input: the JSON document on stdout IS the tool
                // result (not an error) — a caller answers the listed
                // questions (`answers`, id → value) and calls again. The
                // mutating tools run the CLI in text mode (push prints no
                // JSON on success), so the plan narration precedes the
                // document on stdout: isolate it.
                6 => isolate_needs_input(&stdout),
                3 => format!("VALIDATION FAILED (exit 3)\n{stdout}\n{stderr}"),
                4 => format!("AUTH/PERMISSION DENIED (exit 4)\n{stdout}\n{stderr}"),
                5 => format!("DRIFT/CONFLICT DETECTED (exit 5)\n{stdout}\n{stderr}"),
                _ => format!("ERROR (exit {code})\n{stdout}\n{stderr}"),
            }
        }
        Err(e) => format!("Error: failed to run rigg: {e}"),
    }
}

/// Isolate the `needs-input` protocol document from an exit-6 run's stdout.
///
/// In text mode the document is preceded by the command's own prose (the
/// push plan, the delete list). The document is always the trailing
/// pretty-printed JSON object, so scan back to the last line that is
/// exactly `{` and try to parse from there; return that document alone when
/// it really is one, and the untouched stdout when it is not (so nothing is
/// ever silently dropped).
fn isolate_needs_input(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().collect();
    let Some(start) = lines.iter().rposition(|l| l.trim_end() == "{") else {
        return stdout.to_string();
    };
    let tail = lines[start..].join("\n");
    match serde_json::from_str::<serde_json::Value>(&tail) {
        Ok(doc) if doc.get("status").and_then(|s| s.as_str()) == Some("needs-input") => {
            serde_json::to_string_pretty(&doc).unwrap_or(tail)
        }
        _ => stdout.to_string(),
    }
}

/// `--answer <id>=<value>` flags for every pre-supplied answer, sorted by id
/// (`BTreeMap` iteration order) for deterministic output.
fn answer_flags(answers: Option<&BTreeMap<String, String>>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(answers) = answers {
        for (id, value) in answers {
            out.push("--answer".to_string());
            out.push(format!("{id}={value}"));
        }
    }
    out
}

fn with_common<'a>(args: Vec<&'a str>, env: &'a Option<String>, json: bool) -> Vec<String> {
    with_common_answers(args, env, json, None)
}

/// `rigg promote` argument building, factored out so it is unit-testable
/// without going through the async tool call: `--from`/`--to` are always
/// present, `force` selects `--dry-run` vs `--yes`, `offline` adds
/// `--offline`, and `--output json --quiet` plus any `--answer` flags are
/// appended via [`with_common_answers`] (promote has no `env` parameter of
/// its own — `--from`/`--to` name the environments).
fn promote_args(params: &PromoteParams) -> Vec<String> {
    let mut args = vec!["promote"];
    if let Some(p) = &params.project {
        args.push(p);
    }
    args.extend(["--from", params.from.as_str(), "--to", params.to.as_str()]);
    if params.force.unwrap_or(false) {
        args.push("--yes");
    } else {
        args.push("--dry-run");
    }
    if params.offline.unwrap_or(false) {
        args.push("--offline");
    }
    with_common_answers(args, &None, true, params.answers.as_ref())
}

/// Like [`with_common`], additionally appending `--answer <id>=<value>` for
/// every pre-supplied answer — the MCP counterpart of the CLI's `--answer`
/// flag, letting a caller resolve a prior `needs-input` result by re-calling
/// the same tool with `answers` filled in.
fn with_common_answers<'a>(
    mut args: Vec<&'a str>,
    env: &'a Option<String>,
    json: bool,
    answers: Option<&BTreeMap<String, String>>,
) -> Vec<String> {
    if let Some(env) = env {
        args.push("--env");
        args.push(env);
    }
    if json {
        args.push("--output");
        args.push("json");
    }
    args.push("--quiet");
    let mut out: Vec<String> = args.into_iter().map(String::from).collect();
    out.extend(answer_flags(answers));
    out
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
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            true,
            params.answers.as_ref(),
        ))
    }

    #[tool(
        description = "Full workspace description: projects, all resources with definitions and file paths, the dependency graph, and 'APIs to implement' (OpenAPI specs in apis/ that skillsets reference). Scoped to ONE environment (the default unless `env` is set). The fastest way to understand the workspace."
    )]
    async fn rigg_describe(&self, Parameters(params): Parameters<ProjectParams>) -> String {
        let mut args = vec!["describe"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            true,
            params.answers.as_ref(),
        ))
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
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            false,
            params.answers.as_ref(),
        ))
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
            let preview = rigg_cli(&with_common_answers(
                args,
                &params.env,
                false,
                params.answers.as_ref(),
            ));
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
            return rigg_cli(&with_common_answers(
                args,
                &params.env,
                false,
                params.answers.as_ref(),
            ));
        }
        let mut args = vec!["pull"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        args.push("--yes");
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            false,
            params.answers.as_ref(),
        ))
    }

    #[tool(
        description = "Push local project files to Azure in dependency order. Without force: returns the push plan (dry run). With force=true: executes (--yes). prune=true also deletes remote resources whose local files were removed. Protected environments additionally require confirm_env to match the environment name (matches `rigg push --confirm-env`). Plans containing a replace (e.g. a knowledge-source kind change after `rigg migrate`) additionally require allow_replace=true — the replaced index is rebuilt from source data. An identity/RBAC preflight runs before anything is written: missing role assignments rigg may grant are applied (with force=true) and waited out, anything only a human may grant fails with exit 4 and the exact az command — skip_auth_preflight=true bypasses it. verify=true additionally proves the pushed stack works (see rigg_verify). Always rigg_validate first."
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
            if params.verify.unwrap_or(false) {
                args.push("--verify");
            }
        } else {
            args.push("--dry-run");
        }
        if params.skip_auth_preflight.unwrap_or(false) {
            args.push("--skip-auth-preflight");
        }
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            false,
            params.answers.as_ref(),
        ))
    }

    #[tool(
        description = "Prove a pushed project actually works against the live services: every indexer is RUN and watched to completion, every knowledge base gets a retrieve, every agent a one-turn question. NOT read-only — it triggers real indexer runs (ingestion, skill and embedding costs) and takes as long as ingestion takes; it changes no configuration. Protected environments additionally require confirm_env to match the environment name (or the equivalent `answers` entry) — the runs cost money. all=true verifies every project. Failures that look like an authorization problem are attributed to the identity edge that would explain them. Fails (exit 1) when any check fails. The same checks as `rigg push --verify`, for a stack that is already pushed."
    )]
    async fn rigg_verify(&self, Parameters(params): Parameters<VerifyParams>) -> String {
        let mut args = vec!["verify"];
        if let Some(p) = &params.project {
            args.push(p);
        }
        if params.all.unwrap_or(false) {
            args.push("--all");
        }
        if let Some(confirm_env) = &params.confirm_env {
            args.extend(["--confirm-env", confirm_env]);
        }
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            false,
            params.answers.as_ref(),
        ))
    }

    #[tool(
        description = "Translate a project's resources from one environment to another (e.g. dev → staging): every infrastructure reference is re-pointed at the target's binding of the same name (shared bindings are reported unchanged), sibling references follow renamed physical names, and the target's own name/x-rigg-pin/Web-API auth carrier are kept. Without force: preview only (--dry-run), writing nothing — shows rewiring, renamed siblings, changed/new/unchanged/kept-only-in-target resources, and checks. With force=true: writes the translated files (--yes). offline=true skips Azure lookups (auth re-derivation, deployment availability/quota) and reports those items as unresolved instead. May return needs-input for an unbound infrastructure reference, a binding missing in the target environment, an external API URL with no binding in either environment, or a deployment availability/quota problem in the target region — answer by re-calling with `answers` filled in. A missing target environment is NOT a needs-input question: the call fails immediately as a usage error (exit 2) whose message gives the exact `rigg env add <to> --like <from>` command to run first; create the environment, then call rigg_promote again (interactively, `rigg promote` offers to create it inline instead of failing). Follow a successful promote with rigg_validate, then rigg_push (preview first) on the target environment."
    )]
    async fn rigg_promote(&self, Parameters(params): Parameters<PromoteParams>) -> String {
        rigg_cli(&promote_args(&params))
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
        description = "Trigger a live indexer run. Without force: returns the current status as a preview. With force=true: triggers the run (fire-and-forget) — poll rigg_indexer_status until the run completes. Protected environments additionally require confirm_env to match the environment name (or the equivalent `answers` entry). Part of the post-push verification flow: rigg_push → rigg_indexer_run → rigg_indexer_status → rigg_query → rigg_ask."
    )]
    async fn rigg_indexer_run(&self, Parameters(params): Parameters<IndexerRunParams>) -> String {
        if !params.force.unwrap_or(false) {
            let args = vec!["az", "indexer", "status", &params.indexer];
            let preview = rigg_cli(&with_common(args, &params.env, true));
            return format!(
                "PREVIEW (no run triggered) — current status:\n{preview}\nRun again with force=true to trigger a run."
            );
        }
        let mut args = vec!["az", "indexer", "run", &params.indexer, "--yes"];
        if let Some(confirm_env) = &params.confirm_env {
            args.extend(["--confirm-env", confirm_env]);
        }
        rigg_cli(&with_common_answers(
            args,
            &params.env,
            false,
            params.answers.as_ref(),
        ))
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
                    answers: params.answers.clone(),
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
        let mut args: Vec<String> = args.into_iter().map(String::from).collect();
        args.extend(answer_flags(params.answers.as_ref()));
        rigg_cli(&args)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for RiggMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "rigg manages Azure AI Search and Microsoft Foundry configuration as code. \
             A workspace contains projects; each project owns its resources exclusively, \
             and pull/push/diff operate on whole projects. Typical flow: rigg_describe to \
             understand the workspace, rigg_validate before changes, rigg_diff to inspect \
             drift, rigg_push (preview first, then force=true). rigg_promote translates a \
             project's resources between environments (e.g. dev → staging), re-pointing \
             infrastructure references at the target's own bindings rather than copying \
             source values. Resource definitions are \
             JSON files under projects/<name>/envs/<env>/{search,foundry}/<kind>/; secrets are never \
             stored in files — identity-based access only. Guided flows can ask questions: a \
             mutating tool call may come back as a `needs-input` JSON document (the questions, \
             with ids/prompts/candidates) instead of its usual result — that document IS the \
             tool result, not an error. Answer by re-calling the same tool with `answers` \
             (question id → value) filled in; answered questions are never asked again.",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_common_appends_answer_flags_in_sorted_order() {
        let answers: BTreeMap<String, String> = [
            ("b".to_string(), "2".to_string()),
            ("a".to_string(), "1".to_string()),
        ]
        .into_iter()
        .collect();
        let args = with_common_answers(vec!["push"], &None, true, Some(&answers));
        assert!(args.windows(2).any(|w| w == ["--answer", "a=1"]));
        assert!(args.windows(2).any(|w| w == ["--answer", "b=2"]));
    }

    #[test]
    fn isolate_needs_input_returns_the_document_alone() {
        let stdout = "Push project 'demo' (env: prod, protected)\n  \
                      search: mock (https://example.invalid)\n  \
                      + indexes/idx\n{\n  \"status\": \"needs-input\",\n  \
                      \"command\": \"push\",\n  \"context\": {\n    \"env\": \"prod\"\n  },\n  \
                      \"questions\": []\n}\n";
        let isolated = isolate_needs_input(stdout);
        assert!(
            !isolated.contains("Push project"),
            "prose must be stripped: {isolated}"
        );
        let doc: serde_json::Value = serde_json::from_str(&isolated).expect("pure JSON");
        assert_eq!(doc["status"], "needs-input");
        assert_eq!(doc["command"], "push");
    }

    #[test]
    fn isolate_needs_input_passes_through_anything_else() {
        let plain = "Push project 'demo'\n  + indexes/idx\nAborted.\n";
        assert_eq!(isolate_needs_input(plain), plain);
        // A trailing JSON object that is NOT a needs-input document is left
        // where it is, prose and all.
        let other = "note\n{\n  \"status\": \"ok\"\n}\n";
        assert_eq!(isolate_needs_input(other), other);
    }

    #[test]
    fn with_common_answers_none_matches_with_common() {
        let a = with_common(vec!["status"], &None, true);
        let b = with_common_answers(vec!["status"], &None, true, None);
        assert_eq!(a, b);
    }

    #[test]
    fn promote_args_without_force_previews_with_dry_run() {
        let params = PromoteParams {
            project: Some("regulus".to_string()),
            from: "dev".to_string(),
            to: "staging".to_string(),
            force: None,
            offline: None,
            answers: None,
        };
        let args = promote_args(&params);
        assert_eq!(
            args,
            vec![
                "promote",
                "regulus",
                "--from",
                "dev",
                "--to",
                "staging",
                "--dry-run",
                "--output",
                "json",
                "--quiet",
            ]
        );
    }

    #[test]
    fn promote_args_with_force_writes_with_yes() {
        let params = PromoteParams {
            project: None,
            from: "dev".to_string(),
            to: "staging".to_string(),
            force: Some(true),
            offline: Some(true),
            answers: None,
        };
        let args = promote_args(&params);
        assert_eq!(
            args,
            vec![
                "promote",
                "--from",
                "dev",
                "--to",
                "staging",
                "--yes",
                "--offline",
                "--output",
                "json",
                "--quiet",
            ]
        );
    }

    #[test]
    fn promote_args_includes_answer_flags() {
        let mut answers = BTreeMap::new();
        answers.insert(
            "promote.external.example.invalid".to_string(),
            "keep".to_string(),
        );
        let params = PromoteParams {
            project: None,
            from: "dev".to_string(),
            to: "staging".to_string(),
            force: None,
            offline: None,
            answers: Some(answers),
        };
        let args = promote_args(&params);
        assert!(
            args.windows(2)
                .any(|w| w == ["--answer", "promote.external.example.invalid=keep"])
        );
    }
}
