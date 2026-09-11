# MCP Server

rigg includes a built-in [MCP](https://modelcontextprotocol.io/) (Model
Context Protocol) server that gives AI coding tools structured access to your
Azure AI Search and Microsoft Foundry configuration — pull, push, diff,
validate and explore through tool calls instead of shell commands.

## Why Connect Your AI Tool to Rigg?

**Your Agentic RAG stack is a graph.** Agents connect to knowledge bases,
which route to knowledge sources, which search indexes fed by indexers,
skillsets and data sources. Understanding one piece in isolation isn't enough
to make meaningful improvements — and that's all your AI coding tool can do
when configuration lives behind Azure portals and REST APIs.

rigg changes this in two ways:

1. **Every resource as a local file.** Your entire configuration is on disk —
   agent definitions, index schemas, skillset pipelines, knowledge base rules
   — as JSON files under `projects/`. Your AI tool can already read these
   directly. But files alone don't capture how everything connects.

2. **A structured API for the complete picture.** The `rigg_describe` tool
   returns the full workspace graph in a single call: every project, every
   resource with its definition and file path, the dependency graph, and the
   custom Web APIs your skillsets expect you to implement. The AI doesn't
   need to discover your project structure by reading files one at a time; it
   gets the complete system map instantly.

With this context, your AI tool can help you optimize agent instructions,
debug retrieval quality end to end, plan schema changes knowing what depends
on what, deploy across environments, and detect drift.

## Compatible Tools

Any MCP-compatible AI tool works with rigg, including:

- [Claude Code](https://claude.ai/code)
- [GitHub Copilot](https://github.com/features/copilot) (VS Code)
- [Cursor](https://cursor.com/)
- [Codex CLI](https://github.com/openai/codex)
- [Gemini CLI](https://github.com/google-gemini/gemini-cli)
- [Claude Desktop](https://claude.ai/download)

## Setup

```bash
# Claude Code (delegates to `claude mcp add`)
rigg mcp install claude-code

# VS Code (GitHub Copilot) — writes .vscode/mcp.json
rigg mcp install vs-code

# Register user-wide instead of per-workspace
rigg mcp install claude-code --scope global
```

**With `--scope workspace`** (the default), the configuration lives in the
repo: `.mcp.json` for Claude Code, `.vscode/mcp.json` for VS Code. Commit it,
and anyone who clones the workspace gets the MCP server auto-discovered when
they open it.

**For other MCP clients,** configure them to run `rigg mcp serve` as a stdio
server — it speaks MCP JSON-RPC over stdin/stdout. It's not a separate
binary: if you have rigg installed, you have the MCP server.

### Verify it's working

In Claude Code, type `/rigg-status` — the AI will call the MCP tools and
report sync state per project. In VS Code with Copilot, open the MCP panel
and check that "rigg" appears as a connected server with 14 tools.

## Available Tools

**The server exposes 14 tools:** 9 project-scoped configuration tools
(including `rigg_promote`) and 5 runtime-operation tools (`rigg_indexer_*`,
`rigg_query`, `rigg_ask`, `rigg_verify`).

Every tool that talks to Azure accepts an optional `env` (environment name;
the default environment is used if omitted), and tools that operate on a
project accept an optional `project` (may be omitted when the workspace has
exactly one project). `rigg_promote` is the exception — it names two
environments (`from`/`to`) instead of one, so it has no `env` parameter.

**Mutating tools** (`rigg_pull`, `rigg_push`, `rigg_promote`, `rigg_delete`)
follow a **preview/force** pattern: without `force` they return a preview of
what would change and change nothing; with `force: true` they execute. The AI
always shows you what will happen before doing it.

**The protected-environment gate.** `rigg_push`, `rigg_delete`,
`rigg_indexer_run` and `rigg_verify` additionally accept `confirm_env`. If
the target environment has `policy: { protected: true }` in `rigg.yaml` (see
[CONCEPTS.md](CONCEPTS.md#environments)), the mutation does not go through
unannounced:
with no confirmation the tool returns a `needs-input` document asking
`confirm.protected.<env>` (see below), and the answer must equal the
environment's name exactly.

An AI agent can't push, delete, re-index or verify a protected environment
(e.g. prod) on its own initiative; the caller has to name it explicitly.
`confirm_env` remains the shorthand: passing it answers that question up
front.

### The needs-input loop

Guided flows ask questions. When a tool call needs an answer the caller
hasn't given, the underlying CLI exits 6 and the tool returns the
`needs-input` JSON document instead of its usual result — **that document is
the result, not an error**:

```json
{
  "status": "needs-input",
  "command": "push",
  "context": { "project": "docs-rag", "env": "prod" },
  "questions": [
    {
      "id": "confirm.protected.prod",
      "kind": "confirm-env",
      "prompt": "Environment 'prod' is protected. Type its name to confirm push:"
    }
  ]
}
```

Answer by calling the **same tool again** with `answers` filled in — a map of
question id → value, here `{"confirm.protected.prod": "prod"}`. Answered
questions are never asked again. Every tool that can reach a question accepts
`answers` (see the tool table below).

### Every tool at a glance

Names, purpose and full parameter list, generated from the server's own
tool registry — run `rigg mcp tools --markdown` and replace the text
between the markers to refresh it. A `*` marks a required parameter;
everything else is optional. The sections after the table explain what
each tool does.

<!-- generated:mcp-tools:start -->
| Tool | Purpose | Parameters |
|---|---|---|
| `rigg_ask` | Prompt a live knowledge base (agentic retrieval: grounding content + references) or a Foundry agent (single-shot reply). | `prompt`: string*, `agent`: string, `env`: string, `knowledge_base`: string |
| `rigg_delete` | Delete ALL of a project's resources from Azure (local files are kept — pushing re-creates everything). | `project`: string*, `answers`: object, `confirm_env`: string, `env`: string, `force`: boolean |
| `rigg_describe` | Full workspace description: projects, all resources with definitions and file paths, the dependency graph, and 'APIs to implement' (OpenAPI specs in apis/ that skillsets reference). | `answers`: object, `env`: string, `project`: string |
| `rigg_diff` | Semantic diff of local project files vs live Azure (or one env vs another with compare_env). | `answers`: object, `compare_env`: string, `env`: string, `only`: string, `project`: string |
| `rigg_env_list` | List all configured deployment environments from rigg.yaml | none |
| `rigg_indexer_run` | Trigger a live indexer run. | `indexer`: string*, `answers`: object, `confirm_env`: string, `env`: string, `force`: boolean |
| `rigg_indexer_status` | Execution status of a live indexer: state, last run result, per-document errors and warnings. | `indexer`: string*, `env`: string |
| `rigg_promote` | Translate a project's resources from one environment to another (e.g. dev → staging): every infrastructure reference is re-pointed at the target's binding of the same name (shared bindings are reported unchanged), sibling references follow renamed physical names, and the target's own name/x-rigg-pin/Web-API auth carrier are kept. | `from`: string*, `to`: string*, `answers`: object, `force`: boolean, `offline`: boolean, `project`: string |
| `rigg_pull` | Pull remote resource definitions into the project's files. | `adopt`: boolean, `answers`: object, `env`: string, `force`: boolean, `project`: string |
| `rigg_push` | Push local project files to Azure in dependency order. | `allow_replace`: boolean, `answers`: object, `confirm_env`: string, `env`: string, `force`: boolean, `project`: string, `prune`: boolean, `skip_auth_preflight`: boolean, `verify`: boolean |
| `rigg_query` | Run a search query against a live index (smoke-test retrieval without the portal). | `index`: string*, `search`: string*, `env`: string, `filter`: string, `select`: string, `top`: integer |
| `rigg_status` | Show sync status per project: which resources are in sync, local-ahead, remote-ahead, conflicted, plus unmanaged remote resources. | `answers`: object, `env`: string, `project`: string |
| `rigg_validate` | Validate local files: JSON structure, name/filename consistency, exclusive ownership across projects, reference resolution, no-secrets enforcement, data source types. | `project`: string, `strict`: boolean |
| `rigg_verify` | Prove a pushed project actually works against the live services: every indexer is RUN and watched to completion, every knowledge base gets a retrieve, every agent a one-turn question. | `all`: boolean, `answers`: object, `confirm_env`: string, `env`: string, `project`: string |
<!-- generated:mcp-tools:end -->

### rigg_status

Sync status per project: which resources are in sync, local-ahead,
remote-ahead, or conflicted, plus unmanaged remote resources. Covers all
environments unless `env` narrows to one; an unreachable environment is
reported per env without hiding the others.

### rigg_describe

Full workspace description: projects, all resources with definitions and file
paths, the dependency graph, and "APIs to implement" (OpenAPI specs in
`apis/` that skillsets reference). The fastest way to understand the
workspace.

### rigg_env_list

List all configured deployment environments from `rigg.yaml`. No parameters.

### rigg_validate

Validate local files: JSON structure, name/filename consistency, exclusive
ownership across projects, reference resolution, no-secrets enforcement, data
source types, and OpenAPI contracts for linked WebApiSkills.

### rigg_diff

Semantic diff of local project files vs live Azure (or one environment vs
another). Volatile server fields are ignored; array order doesn't matter.

### rigg_pull

Pull remote resource definitions into the project's files.

### rigg_push

Push local project files to Azure in dependency order. Only
semantically-changed resources are touched.

**An identity/RBAC preflight runs before the first write** — the same graph
`rigg auth doctor` verifies, scoped to exactly the documents this push would
send. Requirements rigg may grant itself are listed and, once every
confirmation has been given, applied and then *waited out* (rigg polls until
Azure reports the assignment) before the first PUT.

Anything only a human may grant fails with exit 4 and the exact
`az role assignment create` line, because rigg never grants the caller their
own rights. A refusal happens before the protected-environment gate, the
grants after it, so a push that is never confirmed changes nothing at all.
Without `force`, the preview reports the whole remediation and refuses
nothing.

### rigg_promote

Translate a project's resources from one environment to another (e.g. `dev` →
`staging`): every infrastructure reference is re-pointed at the target's own
binding of the same name (a binding shared between the two environments is
reported unchanged, not skipped), sibling references follow renamed physical
names, and the target's own `name`, `x-rigg-pin` paths, and Web API auth
carrier are kept. Resources that only exist in the target are never touched,
and nothing is deleted. See
[CONCEPTS.md](CONCEPTS.md#promoting-between-environments) for the full
translation model.

**Anything the translation cannot decide comes back as a `needs-input`
question instead of a guess** — an unbound infrastructure reference, a
binding the target environment lacks, an external API URL with no binding in
either environment, or a deployment that is unavailable or short on quota in
the target region. Answer it the same way as any other tool (`answers`, id →
value).

**A missing target environment is not a question.** The tool call fails as a
usage error (exit 2) whose message gives the exact
`rigg env add <to> --like <from>` command to run first — create the
environment, then call `rigg_promote` again. (Running `rigg promote`
interactively offers to create it inline instead of failing.)

After a successful promote, run `rigg_validate`, then `rigg_push` (preview
first) against the target environment.

### rigg_indexer_status

Execution status of a live indexer (read-only): state, last run result,
per-document errors/warnings. Use after `rigg_push` or `rigg_indexer_run` to
verify ingestion.

### rigg_indexer_run

Trigger a live indexer run. Without `force`: returns the current status
(preview). With `force: true`: triggers the run (fire-and-forget) — poll
`rigg_indexer_status` until it completes.

### rigg_query

Search a live index (read-only) — the smoke test for retrieval.

### rigg_ask

Prompt a live knowledge base (agentic retrieval: grounding + references) or
Foundry agent (single-shot reply). Read-only. Pass exactly one of
`knowledge_base`/`agent`.

Together these close the loop for AI agents: `rigg_push` →
`rigg_indexer_run` → `rigg_indexer_status` → `rigg_query` → `rigg_ask` — a
self-verified deployment with no human in the portal.

Run `rigg_validate` first — the tool description tells the AI to, and
well-behaved agents will.

### rigg_verify

Prove a pushed project actually works against the live services: every
indexer is run and watched to completion, every knowledge base gets a
retrieve, every agent a one-turn question.

**Not read-only** — it triggers real indexer runs (ingestion, skill and
embedding costs) and takes as long as ingestion takes; it changes no
configuration. A failure whose message looks like an authorization problem is
attributed to the identity edge that would explain it. Fails (exit 1) when
any check fails. The same checks as `rigg push --verify`, for a stack that is
already pushed.

### rigg_delete

Delete ALL of a project's resources from Azure. Local files are kept, so
pushing re-creates everything. To delete a single resource instead: delete
its local file, then `rigg_push` with `prune: true`.

## Example Workflows

### "What does my workspace look like?"

Ask the AI to describe your workspace. It calls `rigg_describe` and reasons
over the graph:

```
> Describe my rigg workspace

Your project "docs-rag" has a complete pipeline:
- docs-ds (blob) → docs-index ← docs-indexer (+ docs-skills)
- docs-ks exposes docs-index; docs-kb routes retrieval to it
- docs-agent (docs-model deployment) grounds on docs-kb via MCP
- One API to implement: doc-enrichment (referenced by docs-skills)
```

### "Push my changes"

```
> /rigg-push

Validating... OK
Push plan for docs-rag (dry run):
  ~ indexes/docs-index    2 fields added
  ~ agents/docs-agent     instructions updated

Push 2 resources to dev? [confirm]
```

The AI calls `rigg_validate`, then `rigg_push` without `force` to show the
plan, asks you, and only then calls `rigg_push` with `force: true`.

### "Has anything drifted?"

```
> Did anyone change our search config in the portal?

[rigg_status → conflict on indexes/docs-index]
[rigg_diff only: "indexes/docs-index"]

Someone added a field 'reviewedBy' directly in Azure. Options: pull it
into the file, or push to overwrite it.
```

## How It Works

**Every MCP tool shells out to the rigg CLI itself** (`rigg … --output
json`), so tool behavior is *exactly* CLI behavior — same validation, same
normalization, same exit codes. Non-zero exit codes are surfaced to the AI
with their meaning (exit 2 = usage error, 3 = validation failed, 4 = auth
denied, 5 = drift/conflict, 6 = needs input), so it can react appropriately.

**Exit 6 is the one that isn't a failure:** the CLI printed a `needs-input`
document, and the tool returns that document on its own (any prose the
command printed first is stripped). Call the tool again with `answers` to
continue — see [the needs-input loop](#the-needs-input-loop).

## See Also

- [SKILLS.md](SKILLS.md) — Agent skills and slash commands (work
  independently of MCP)
- [INSTALL.md](INSTALL.md) — Installation and setup
