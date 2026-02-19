# AI Agent Integration

hoist includes a built-in [MCP](https://modelcontextprotocol.io/) (Model Context Protocol) server that lets AI coding tools interact with your Azure AI Search and Microsoft Foundry configuration directly — no shell commands needed. The AI can pull, push, diff, validate, and explore your resources through structured tool calls.

## Supported Tools

Any MCP-compatible AI tool works with hoist, including:

- [Claude Code](https://claude.ai/code)
- [GitHub Copilot](https://github.com/features/copilot) (VS Code)
- [Cursor](https://cursor.com/)
- [Codex CLI](https://github.com/openai/codex)
- [Gemini CLI](https://github.com/google-gemini/gemini-cli)
- [Claude Desktop](https://claude.ai/download)

## Setup

### Automatic (project-level)

If you cloned a hoist project that includes `.mcp.json` in the repo root, the MCP server is auto-discovered by Claude Code and VS Code when you open the project. No setup needed.

To add auto-discovery to your own project, create `.mcp.json` in the repo root:

```json
{
  "mcpServers": {
    "hoist": {
      "command": "hoist",
      "args": ["mcp", "serve"]
    }
  }
}
```

### Manual (user-level)

To register hoist as an MCP server across all your projects (not just ones with `.mcp.json`):

```bash
# Claude Code
hoist mcp install claude-code

# VS Code (GitHub Copilot)
hoist mcp install vs-code
```

For other MCP clients, configure them to run `hoist mcp serve` as a stdio server. The server communicates via JSON-RPC over stdin/stdout.

### Verify it's working

In Claude Code, type `/hoist-status` — the AI will call the MCP tools and report your project's environment, auth state, and resource inventory. If you see environment and resource details, MCP is working.

In VS Code with Copilot, open the MCP panel and check that "hoist" appears as a connected server with its tools listed.

## Available Tools

The MCP server exposes 8 tools. All tools accept an optional `env` parameter to target a specific environment (uses default if omitted).

### Read-only tools

| Tool | What it does |
|------|--------------|
| `hoist_status` | Project status: environment info, auth state, resource counts, last sync time |
| `hoist_describe` | Full project description with all resources, dependencies, agent configurations, and knowledge base flows. Includes file paths for every resource so the AI can read full definitions |
| `hoist_env_list` | List all configured environments with their services |
| `hoist_validate` | Validate local resource files for syntax errors and broken cross-references |
| `hoist_list` | List resource names by type. Source can be `local` (fast disk scan), `remote` (Azure API), or `both` (find drift) |
| `hoist_diff` | Compare local files against live Azure services. Shows field-level changes |

### Mutating tools

| Tool | What it does |
|------|--------------|
| `hoist_pull` | Pull resource definitions from Azure to local files |
| `hoist_push` | Push local changes to Azure |

Mutating tools use a **safe-by-default** pattern:
- **Without `force`**: returns a preview of what would change, but doesn't execute
- **With `force: true`**: executes the operation

This means the AI always shows you what will happen before making changes.

### Tool parameters

**Common parameters** (all tools):

| Parameter | Type | Description |
|-----------|------|-------------|
| `env` | string | Target environment name. Uses default if omitted |

**Resource filtering** (diff, pull, push):

| Parameter | Type | Description |
|-----------|------|-------------|
| `resource_type` | string | Filter by type: `indexes`, `agents`, `datasources`, `skillsets`, `indexers`, `synonymmaps`, `aliases`, `knowledgebases`, `knowledgesources` |
| `name` | string | Filter to a single resource by name (requires `resource_type`) |

**Validation options** (validate):

| Parameter | Type | Description |
|-----------|------|-------------|
| `strict` | bool | Treat warnings as errors |
| `check_references` | bool | Validate cross-resource references (e.g., indexer references valid index) |

**List options** (list):

| Parameter | Type | Description |
|-----------|------|-------------|
| `resource_type` | string | Filter by type (same values as above) |
| `source` | string | Where to list from: `local` (disk), `remote` (Azure), or `both` |

## Agent Skills

hoist ships with [agent skills](https://agentskills.io/) — cross-platform workflow instructions that work with Claude Code, GitHub Copilot, Codex, Cursor, and Gemini CLI. Skills teach the AI *when* and *how* to use hoist tools.

### Slash commands

| Command | What it does |
|---------|--------------|
| `/hoist-status` | Show environment info, auth state, and full resource inventory |
| `/hoist-pull` | Pull resources from Azure with preview and confirmation |
| `/hoist-push` | Safe push: validate, diff, show preview, confirm before pushing |

Skills accept an optional environment name as an argument:

```
/hoist-pull test
/hoist-push prod
```

### Auto-loaded guide

The `hoist-guide` skill loads automatically when the AI detects hoist context (e.g., working with `hoist.yaml`, search indexes, or Foundry agents). It provides the AI with background knowledge about hoist's file structure, workflows, and safety rules without any user action.

## Example Workflows

### "What does my project look like?"

Ask the AI to describe your project. It will call `hoist_describe` and give you a structured overview of all resources, how they connect, and where the files are:

```
> Describe my hoist project

Your project "My RAG System" has:
- 1 Foundry agent (research-assistant, gpt-4o) connected to regulatory-kb
- 1 knowledge base (regulatory-kb) with extractive retrieval
- 1 knowledge source (regulatory) indexing Azure Blob Storage
- 1 index (regulatory-index, 13 fields with vector search and semantic config)
...
```

### "Pull the latest from Azure"

```
> /hoist-pull

Previewing pull from prod...
- regulatory-index.json: 2 fields changed
- research-assistant.yaml: instructions updated

Proceed? [confirm]

Pulled 2 resources from prod.
```

### "Help me optimize my agent"

The AI can read your agent's full instructions, tools, model, and connected knowledge sources via `hoist_describe`, then suggest changes:

```
> How can I improve my research-assistant agent?

Looking at your agent configuration...
[reads full instructions, tools, knowledge base config, index schema]

Suggestions:
1. Your retrieval instructions could be more specific about...
2. Consider adding a file_search tool for...
3. The index has a semantic config but the agent isn't using...
```

### "Deploy to a new environment"

```
> /hoist-push test

Validating local files... 0 errors, 0 warnings
Diffing against test environment...
+ Index 'regulatory-index' (new)
+ Agent 'research-assistant' (new)

Push 2 resources to test? [confirm]
```

## How It Works

The MCP server runs as a subcommand of the hoist CLI itself (`hoist mcp serve`). It's not a separate binary — if you have hoist installed, you have the MCP server. It communicates over stdio using the MCP JSON-RPC protocol.

When an AI tool calls an MCP tool, hoist:
1. Loads your `hoist.yaml` configuration
2. Resolves the target environment
3. Performs the requested operation (using the same code as the CLI)
4. Returns structured JSON that the AI can reason about

The `hoist_describe` tool is particularly important for AI agents — it returns the complete project graph in a single call, including file paths for every resource. This lets the AI understand your entire Agentic RAG stack without reading individual files.
