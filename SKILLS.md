# Agent Skills

rigg ships with [agent skills](https://agentskills.io/) — cross-platform workflow instructions that teach AI coding tools *when* and *how* to work with rigg. Skills work with Claude Code, GitHub Copilot, Codex, Cursor, and Gemini CLI.

Skills are independent of the [MCP server](MCP.md). The MCP server gives AI tools *access* to rigg operations through structured tool calls. Skills give AI tools *knowledge* about rigg — file conventions, safety rules, and multi-step workflows. You can use either or both:

| Setup | What you get |
|-------|--------------|
| MCP only | AI can call rigg tools (pull, push, diff, etc.) but doesn't know rigg conventions |
| Skills only | AI understands rigg workflows and can read/edit resource files, but runs CLI commands via shell |
| MCP + Skills | Full integration — AI knows the workflows *and* has structured tool access |

## Available Skills

| Skill | Trigger | What it does |
|-------|---------|--------------|
| `rigg-guide` | Auto-loaded | Background knowledge: workspace/project model, file layout, `x-rigg-*` annotations, `$file` sidecars, no-secrets rules, key workflows, exit codes |
| `/rigg-status` | User-invoked | Inspect sync state — projects, environments, drift, unmanaged remote resources |
| `/rigg-pull` | User-invoked | Pull configuration from Azure into project files, previewing changes first |
| `/rigg-push` | User-invoked | Safe push: validate, review the dependency-ordered plan, confirm, then apply |

The `rigg-guide` skill activates automatically when the AI detects rigg context — working with `rigg.yaml`, project resource files, search indexes, or Foundry agents. No user action needed. The slash commands orchestrate multi-step workflows: `/rigg-push`, for example, validates local files, shows the push plan, asks for confirmation, and only then pushes — whether it uses MCP tools or shell commands under the hood.

Two additional skills live in the rigg repository for **rigg's own development** (they are not part of a user workspace): `api-watchdog` checks whether rigg's pinned Azure API versions are still current at the start of a coding session, and `test-complete-enduser-experience` runs an end-to-end test of the CLI against live Azure services before releases.

## How Skills Work

Skills are markdown instruction files in `.claude/skills/` (and equivalent paths for other AI tools). They're not code — they're structured prompts that tell the AI how to accomplish specific tasks. This makes them portable across AI tools that support the [agent skills](https://agentskills.io/) convention.

For tools that don't discover `.claude/skills/` directories, rigg can emit its skill as a single markdown file you can place wherever your tool reads instructions:

```bash
rigg ai skill --emit > ~/.claude/skills/rigg.md   # or your tool's equivalent path
rigg ai skill --reference                          # full command reference document
```

## See Also

- [MCP.md](MCP.md) — MCP server setup and tool reference
- [INSTALL.md](INSTALL.md) — Installation and setup
