# MCP Server

rigg includes a built-in [MCP](https://modelcontextprotocol.io/) (Model Context Protocol) server that gives AI coding tools structured access to your Azure AI Search and Microsoft Foundry configuration — pull, push, diff, validate, and explore through tool calls instead of shell commands.

## Why Connect Your AI Tool to Rigg?

Your Agentic RAG stack is a graph: agents connect to knowledge bases, which route to knowledge sources, which search indexes fed by indexers, skillsets, and data sources. Understanding one piece in isolation isn't enough to make meaningful improvements — but that's all your AI coding tool can do when configuration lives behind Azure portals and REST APIs.

rigg changes this in two ways:

1. **Every resource as a local file.** Your entire configuration is on disk — agent definitions, index schemas, skillset pipelines, knowledge base rules — as JSON files under `projects/`. Your AI tool can already read these directly. But files alone don't capture how everything connects.

2. **A structured API for the complete picture.** The `rigg_describe` tool returns the full workspace graph in a single call — every project, every resource with its definition and file path, the dependency graph, and the custom Web APIs your skillsets expect you to implement. The AI doesn't need to discover your project structure by reading files one at a time; it gets the complete system map instantly.

With this context, your AI tool can help you optimize agent instructions, debug retrieval quality end to end, plan schema changes knowing what depends on what, deploy across environments, and detect drift.

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

With `--scope workspace` (the default), the configuration lives in the repo (`.mcp.json` for Claude Code, `.vscode/mcp.json` for VS Code) — commit it, and anyone who clones the workspace gets the MCP server auto-discovered when they open it.

For other MCP clients, configure them to run `rigg mcp serve` as a stdio server — it speaks MCP JSON-RPC over stdin/stdout. It's not a separate binary: if you have rigg installed, you have the MCP server.

### Verify it's working

In Claude Code, type `/rigg-status` — the AI will call the MCP tools and report sync state per project. In VS Code with Copilot, open the MCP panel and check that "rigg" appears as a connected server with 8 tools.

## Available Tools

The server exposes 8 project-scoped tools. Every tool that talks to Azure accepts an optional `env` (environment name; the default environment is used if omitted), and tools that operate on a project accept an optional `project` (may be omitted when the workspace has exactly one project).

Mutating tools (`rigg_pull`, `rigg_push`, `rigg_delete`) follow a **preview/force** pattern: without `force` they return a preview of what would change and change nothing; with `force: true` they execute. The AI always shows you what will happen before doing it.

`rigg_push` and `rigg_delete` additionally accept `confirm_env`: if the target environment has `policy: { protected: true }` in `rigg.yaml` (see [CONCEPTS.md](CONCEPTS.md#environments)), the mutation is refused unless `confirm_env` matches the environment's name exactly — an AI agent can't push or delete against a protected environment (e.g. prod) just because it decided to; the caller has to name it explicitly.

### rigg_status

Sync status per project: which resources are in sync, local-ahead, remote-ahead, or conflicted, plus unmanaged remote resources.

| Parameter | Type | Description |
|---|---|---|
| `project` | string? | Project name (omit when the workspace has exactly one project) |
| `env` | string? | Environment name |

### rigg_describe

Full workspace description: projects, all resources with definitions and file paths, the dependency graph, and "APIs to implement" (OpenAPI specs in `apis/` that skillsets reference). The fastest way to understand the workspace.

| Parameter | Type | Description |
|---|---|---|
| `project` | string? | Project name |
| `env` | string? | Environment name |

### rigg_env_list

List all configured deployment environments from `rigg.yaml`. No parameters.

### rigg_validate

Validate local files: JSON structure, name/filename consistency, exclusive ownership across projects, reference resolution, no-secrets enforcement, data source types, and OpenAPI contracts for linked WebApiSkills.

| Parameter | Type | Description |
|---|---|---|
| `project` | string? | Project name (omit to validate all projects) |
| `strict` | bool? | Enable stricter checks (cross-service reference resolution) |

### rigg_diff

Semantic diff of local project files vs live Azure (or one environment vs another). Volatile server fields are ignored; array order doesn't matter.

| Parameter | Type | Description |
|---|---|---|
| `project` | string? | Project name |
| `env` | string? | Environment name |
| `only` | string? | Restrict to one resource: `<kind-dir>/<name>` (e.g. `indexes/my-index`) |
| `compare_env` | string? | Compare `env` against this environment instead of local files |

### rigg_pull

Pull remote resource definitions into the project's files.

| Parameter | Type | Description |
|---|---|---|
| `project` | string? | Project name |
| `env` | string? | Environment name |
| `adopt` | bool? | Adopt unmanaged remote resources into the project (requires an explicit `project`) |
| `force` | bool? | Without force: returns the local-vs-remote diff as a preview. With `force: true`: executes the pull |

### rigg_push

Push local project files to Azure in dependency order. Only semantically-changed resources are touched.

| Parameter | Type | Description |
|---|---|---|
| `project` | string? | Project name |
| `env` | string? | Environment name |
| `prune` | bool? | Also delete remote resources whose local files were removed |
| `force` | bool? | Without force: returns the push plan (dry run). With `force: true`: executes |
| `confirm_env` | string? | Required when `env` is a protected environment: must equal its name. Ignored unless `force: true` |
| `allow_replace` | bool? | Required when the plan contains a replace (delete + recreate, e.g. a knowledge-source kind change after `rigg migrate`): the replaced index is rebuilt from source data. Ignored unless `force: true` |

Run `rigg_validate` first — the tool description tells the AI to, and well-behaved agents will.

### rigg_delete

Delete ALL of a project's resources from Azure. Local files are kept, so pushing re-creates everything. To delete a single resource instead: delete its local file, then `rigg_push` with `prune: true`.

| Parameter | Type | Description |
|---|---|---|
| `project` | string | Project whose remote resources should be deleted (required) |
| `env` | string? | Environment name |
| `force` | bool? | Without force: returns a preview of what would be removed. With `force: true`: executes |
| `confirm_env` | string? | Required when `env` is a protected environment: must equal its name. Ignored unless `force: true` |

## Example Workflows

### "What does my workspace look like?"

Ask the AI to describe your workspace. It calls `rigg_describe` and reasons over the graph:

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

The AI calls `rigg_validate`, then `rigg_push` without `force` to show the plan, asks you, and only then calls `rigg_push` with `force: true`.

### "Has anything drifted?"

```
> Did anyone change our search config in the portal?

[rigg_status → conflict on indexes/docs-index]
[rigg_diff only: "indexes/docs-index"]

Someone added a field 'reviewedBy' directly in Azure. Options: pull it
into the file, or push to overwrite it.
```

## How It Works

Every MCP tool shells out to the rigg CLI itself (`rigg … --output json`), so tool behavior is *exactly* CLI behavior — same validation, same normalization, same exit codes. Non-zero exit codes are surfaced to the AI with their meaning (exit 3 = validation failed, 4 = auth denied, 5 = drift/conflict), so it can react appropriately.

## See Also

- [SKILLS.md](SKILLS.md) — Agent skills and slash commands (work independently of MCP)
- [INSTALL.md](INSTALL.md) — Installation and setup
