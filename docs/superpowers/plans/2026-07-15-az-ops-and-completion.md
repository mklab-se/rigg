# rigg az Operations + Dynamic Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rigg az <noun> <verb>` runtime operations (indexer run/reset/status, index query/stats, kb ask, agent ask), matching MCP tools, and dynamic tab completion from local workspace files; released as v1.6.0.

**Architecture:** Thin ops layer: new data-plane methods in rigg-client (search + foundry), thin `Remote` wrappers, `commands/az/` per-noun modules, `completion_dynamic.rs` candidate functions hooked into main.rs via clap_complete's `CompleteEnv`. Registry untouched.

**Tech Stack:** clap 4 / clap_complete 4.5 (`unstable-dynamic`), wiremock, inquire.

**Spec:** `docs/superpowers/specs/2026-07-15-az-ops-and-completion-design.md` (API contracts pinned in §2).

## Global Constraints

- Pre-push verification: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Ops address resources by physical name; no project ownership required; env via `-e`/`RIGG_ENV`/default; print the remote target banner.
- reset / run --reset: default-No confirm naming reprocessing cost; `--yes` OK; protected env gate via confirm_protected_env. Read ops ungated.
- Completion candidates from LOCAL files only; outside a workspace, complete nothing, silently.
- `--watch` poll interval 5s, env-tunable `RIGG_WATCH_INTERVAL_SECS` for tests; failed run → exit 1 with errors rendered.

---

### Task 1: rigg-client search ops (run/reset/status/stats/search/retrieve)

**Files:** Modify `crates/rigg-client/src/client.rs`.
**Produces:** on `AzureSearchClient`:
- `pub async fn indexer_run(&self, name: &str) -> Result<(), ClientError>` — POST `{base}/indexers/{name}/run?api-version=` (expect 2xx; 202 typical)
- `pub async fn indexer_reset(&self, name: &str) -> Result<(), ClientError>` — POST `/indexers/{name}/reset`
- `pub async fn indexer_status(&self, name: &str) -> Result<Value, ClientError>` — GET `/indexers/{name}/status`
- `pub async fn index_stats(&self, name: &str) -> Result<Value, ClientError>` — GET `/indexes/{name}/stats`
- `pub async fn search_docs(&self, index: &str, body: &Value) -> Result<Value, ClientError>` — POST `/indexes/{index}/docs/search`
- `pub async fn kb_retrieve(&self, kb: &str, body: &Value) -> Result<Value, ClientError>` — POST `/knowledgebases('{kb}')/retrieve` (also accept 206 as success)

Steps: implement using the existing `request` helper/URL patterns (mirror `resource_url`; the retrieve path uses the parenthesized OData form — build it directly with `urlencoding::encode(kb)`); `cargo build -p rigg-client`; commit `feat(client): search data-plane operations`.
(Wiremock coverage arrives with the CLI tests in Tasks 3–5.)

### Task 2: rigg-client foundry agent invocation

**Files:** Modify `crates/rigg-client/src/foundry.rs`.
**Produces:** `pub async fn agent_respond(&self, agent: &str, input: &str) -> Result<Value, ClientError>` — POST `{base_url}/api/projects/{project}/openai/v1/responses` (match the existing URL-building helper style) with body:

```json
{"agent_reference": {"name": "<agent>", "type": "agent_reference"}, "input": "<input>", "stream": false}
```

Steps: implement; build; commit `feat(client): foundry agent responses invocation`.

### Task 3: `rigg az indexer` (run/reset/status, --watch)

**Files:**
- Create `crates/rigg/src/commands/az/mod.rs`, `crates/rigg/src/commands/az/indexer.rs`
- Modify `crates/rigg/src/cli.rs` (Commands::Az + arg structs + dispatch), `crates/rigg/src/commands/mod.rs`, `crates/rigg/src/commands/remote.rs` (thin wrappers `indexer_run/reset/status`)
- Test `crates/rigg/tests/sync.rs`

**CLI shape (cli.rs):**

```rust
/// Operate the live Azure resources (run indexers, query indexes, prompt
/// knowledge bases and agents)
Az {
    #[command(subcommand)]
    command: AzCommands,
},

#[derive(Subcommand)]
pub enum AzCommands {
    /// Indexer operations
    Indexer { #[command(subcommand)] command: AzIndexerCommands },
    /// Index operations
    Index { #[command(subcommand)] command: AzIndexCommands },
    /// Knowledge-base operations
    #[command(name = "knowledge-base", alias = "kb")]
    KnowledgeBase { #[command(subcommand)] command: AzKbCommands },
    /// Agent operations
    Agent { #[command(subcommand)] command: AzAgentCommands },
}

#[derive(Subcommand)]
pub enum AzIndexerCommands {
    /// Trigger a run now
    Run(AzIndexerRunArgs),
    /// Clear change-tracking state (next run reprocesses EVERYTHING)
    Reset(AzIndexerResetArgs),
    /// Execution state, last run, per-document errors
    Status { name: String },
}
#[derive(Args)] pub struct AzIndexerRunArgs {
    pub name: String,
    /// Poll until the run completes; exit non-zero on failure
    #[arg(long)] pub watch: bool,
    /// Reset change tracking first (confirm-gated: full reprocess)
    #[arg(long)] pub reset: bool,
    #[arg(long, value_name = "ENV")] pub confirm_env: Option<String>,
}
#[derive(Args)] pub struct AzIndexerResetArgs {
    pub name: String,
    #[arg(long, value_name = "ENV")] pub confirm_env: Option<String>,
}
```

**az/mod.rs:** dispatch + `pub(crate) async fn connect(ctx) -> Result<(Workspace, ResolvedEnv, Remote)>` — load workspace, resolve env, pick the FIRST project for connections (ops are service-level; connections are env-level — use `select_projects(&ws, None, false)` when single project else any project whose env has connections: implement as: iterate ws.projects, first with a usable Remote; error if none) and print the target banner.

**indexer.rs behavior:**
- run: protected gate → if `--reset`: confirm (`interactive::confirm_default_no` or `--yes`; non-interactive without --yes → Usage error) → reset → run → if `--watch`: poll `indexer_status` every `RIGG_WATCH_INTERVAL_SECS` (default 5): print transition lines; terminal when lastResult.status ∈ {success, transientFailure→keep? no: success|error|reset} — treat `success` → exit Ok; `error` → render errors, `Err(CommandError::Validation)`? use plain anyhow → exit 1; keep polling while overall/lastResult is `inProgress` or run not yet visible. Cap at 720 polls (1h) then error.
- status: fetch + render (status, lastResult status/start/end/items, errors/warnings ≤20 each with counts). `--output json` prints raw.

**Wiremock tests (sync.rs):** `az_indexer_run_watch_reports_failure_with_errors` — POST run 202; status returns inProgress once then error with 2 doc errors (stateful wiremock: use `Mock::up_to_n_times(1)` mounted FIRST for the inProgress response, then a catch-all error-status mock; set RIGG_WATCH_INTERVAL_SECS=0). Assert exit failure + stderr/stdout contains the error message. `az_indexer_reset_requires_yes_non_interactively` — exit 2, no POSTs. `az_indexer_status_renders` — GET status → stdout contains items processed.

Steps: failing tests → implement → pass → fmt/clippy → commit `feat: rigg az indexer run/reset/status with --watch`.

### Task 4: `rigg az index` (query/stats)

**Files:** Create `crates/rigg/src/commands/az/index.rs`; extend cli.rs enums; remote.rs wrappers; tests in sync.rs.

```rust
#[derive(Subcommand)]
pub enum AzIndexCommands {
    /// Run a search query against the live index
    Query(AzIndexQueryArgs),
    /// Document count and storage size
    Stats { name: String },
}
#[derive(Args)] pub struct AzIndexQueryArgs {
    pub name: String,
    /// Search text (* for all)
    pub search: String,
    #[arg(long, default_value_t = 5)] pub top: u32,
    #[arg(long)] pub filter: Option<String>,
    /// Comma-separated fields to return
    #[arg(long)] pub select: Option<String>,
}
```

Query body: `{"search": ..., "top": ..., "count": true}` + optional `filter`/`select`. Render: `N total` then per hit: score + fields (strings truncated at 200 chars). Stats render: documentCount + storageSize humanized.

**Tests:** `az_index_query_renders_hits` (POST docs/search mock with 2 hits incl. @search.score → stdout shows count and a field value; assert request body carried top/filter/select), `az_index_stats_renders` (GET stats → shows documentCount).

Commit `feat: rigg az index query/stats`.

### Task 5: `rigg az kb ask` + `rigg az agent ask`

**Files:** Create `crates/rigg/src/commands/az/kb.rs`, `crates/rigg/src/commands/az/agent.rs`; cli.rs; remote.rs wrappers (`kb_retrieve`, `agent_ask` — agent path needs the foundry client); tests in sync.rs.

```rust
#[derive(Subcommand)] pub enum AzKbCommands {
    /// Retrieve grounding content for a prompt (agentic retrieval)
    Ask { name: String, prompt: String },
}
#[derive(Subcommand)] pub enum AzAgentCommands {
    /// Send a single prompt and print the reply
    Ask { name: String, prompt: String },
}
```

- kb ask request: `{"intents": [{"type": "semantic", "search": prompt}], "includeActivity": true}`. Render: each `response[].content[]` where type==text → print text; then `references[]` numbered: `[{i}] {sourceData.title || docKey || url} (score {rerankerScore})`. Empty references → note. 206 → prefix warning "partial result (a knowledge source reported an error)".
- agent ask render: walk `output[]`, items with `content[]`, entries with `text` (type `output_text`) → print; fallback `output_text` top-level string if present; else pretty-print JSON with a note.

**Tests:** `az_kb_ask_renders_references` (wiremock POST retrieve on `/knowledgebases('test-kb')/retrieve` — note wiremock path matching with parens: use `path("/knowledgebases('test-kb')/retrieve")` — returns contract sample → stdout contains grounding text + reference title). `az_agent_ask_renders_reply` — workspace fixture needs `foundry: {account: mock, project: proj, endpoint: server.uri()}`; mock POST `/api/projects/proj/openai/v1/responses` → output_text reply; assert stdout.

Commit `feat: rigg az kb ask / agent ask`.

### Task 6: MCP tools

**Files:** Modify `crates/rigg/src/mcp/tools.rs` (+ skill.rs reference doc if it lists tools).

- `rigg_indexer_status {indexer, env?}` → `rigg az indexer status <n> --output json`
- `rigg_query {index, search, top?, filter?, select?, env?}` → `rigg az index query ... --output json`
- `rigg_ask {knowledge_base?, agent?, prompt, env?}` → exactly one of kb/agent required (error string otherwise) → `rigg az knowledge-base ask ...` / `rigg az agent ask ...` with `--output json`
- `rigg_indexer_run {indexer, env?, force?}` → without force: status preview + "run again with force=true"; with force: `rigg az indexer run <n> --yes` (no --watch; hint at rigg_indexer_status)
- Update server instructions string + MCP.md table.

Commit `feat(mcp): runtime operation tools`.

### Task 7: dynamic completion

**Files:**
- Create `crates/rigg/src/completion_dynamic.rs`
- Modify `crates/rigg/Cargo.toml` (`clap_complete = { version = "4.5", features = ["unstable-dynamic"] }` via workspace dep table if needed), `crates/rigg/src/main.rs` (CompleteEnv hook FIRST, before Cli::parse), `crates/rigg/src/cli.rs` (attach `add: ArgValueCompleter`), `crates/rigg/src/commands/completion.rs` + `init.rs` (teach registration one-liner).
- Test: unit tests in completion_dynamic.rs + cli_surface smoke.

**main.rs hook:**
```rust
clap_complete::CompleteEnv::with_factory(Cli::command).complete();
```
(early in main, before parse; it exits the process when COMPLETE is set).

**completion_dynamic.rs:** candidate functions, all `fn(cur: &OsStr) -> Vec<CompletionCandidate>` built from pure helpers (unit-tested):
- `fn workspace() -> Option<Workspace>` (discover from cwd, silent None)
- `fn env_name() -> Option<String>` (RIGG_ENV or default env)
- `pub fn projects(...)`, `pub fn envs(...)`, `pub fn kinds(...)`
- `pub fn resource_names(kind) -> ...` (list every project's Store for the env; physical names)
- `pub fn selectors(...)` → `<kind-dir>/<name>` for all kinds present
Attach via `#[arg(add = ArgValueCompleter::new(...))]` on: az noun name args (indexers/indexes/knowledge-bases/agents), project positionals (push/pull/diff/status/validate/delete/promote/adopt/describe), global `--env`, adopt selectors, `diff --only`, `new` kind, `migrate knowledge-source` name.

**completion.rs help:** extend long_about: static script + dynamic one-liners per shell (`source <(COMPLETE=zsh rigg)`, bash same, fish `COMPLETE=fish rigg | source`).

**Tests:** unit — temp workspace with two projects/resources: projects() lists both; resource_names(Indexer) lists indexer stems; selectors contain "indexes/docs"; outside workspace → empty. cli_surface — `COMPLETE=zsh rigg` (env var, no args) exits 0 and emits non-empty script containing "_rigg" or "COMPLETE".

Commit `feat: dynamic tab completion from workspace files`.

### Task 8: docs + full verification

**Files:** CHANGELOG.md (1.6.0), README.md (az section + completion), MCP.md (new tools), `.claude/skills/rigg-guide/SKILL.md` (az commands + completion note).

Steps: write docs; `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`; commit `docs: rigg az + completion`.

### Task 9: live smoke (mklabsrch / mklabaifndr)

From `e2e-test/`: `rigg az indexer status test-ks-indexer`; `rigg az index stats test-ks-index`; `rigg az index query test-ks-index "regulatory" --top 2`; `rigg az kb ask` against a temp KB over test-ks (create/push/ask/delete/prune) or skip KB live if regulatory-kb suffices via... use `regulatory-kb` READ-ONLY (ask is read-only — safe); `rigg az agent ask Regulus "Say hello"`. Fix findings; commit.

### Task 10: release v1.6.0

Bump workspace Cargo.toml (incl. internal deps) 1.5.0 → 1.6.0; CHANGELOG already dated; commit `Release v1.6.0`; push; tag `v1.6.0`; push tag; watch workflow to completion (`gh run watch`); confirm RELEASE_OK.

## Self-review

- Spec coverage: CLI (§1: T3–T5), client contracts (§2: T1–T2), MCP (§3: T6), completion (§4: T7), architecture (§5: file layout as specified), testing (§6: per-task + T9). Release per goal (T10). ✓
- No placeholders; names consistent (`indexer_run`, `kb_retrieve`, `agent_respond`, `AzCommands`, `completion_dynamic`). ✓
- Watch terminal-state rule and 206 handling specified. ✓
