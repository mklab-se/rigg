# Agent Skills

hoist ships with [agent skills](https://agentskills.io/) — cross-platform workflow instructions that teach AI coding tools *when* and *how* to work with hoist. Skills work with Claude Code, GitHub Copilot, Codex, Cursor, and Gemini CLI.

Skills are independent of the [MCP server](MCP.md). The MCP server gives AI tools *access* to hoist operations through structured tool calls. Skills give AI tools *knowledge* about hoist — file conventions, safety rules, and multi-step workflows. You can use either or both:

| Setup | What you get |
|-------|--------------|
| MCP only | AI can call hoist tools (pull, push, diff, etc.) but doesn't know hoist conventions |
| Skills only | AI understands hoist workflows and can read/edit resource files, but runs CLI commands via shell |
| MCP + Skills | Full integration — AI knows the workflows *and* has structured tool access |

## Slash Commands

User-invoked skills that guide the AI through multi-step hoist workflows:

| Command | What it does |
|---------|--------------|
| `/hoist-status` | Show environment info, auth state, and full resource inventory |
| `/hoist-pull` | Pull resources from Azure with preview and confirmation |
| `/hoist-push` | Safe push: validate, diff, show preview, confirm before pushing |

Slash commands accept an optional environment name as an argument:

```
/hoist-pull test
/hoist-push prod
```

Each slash command orchestrates multiple steps. For example, `/hoist-push` will validate local files, diff against the remote, show you a preview of changes, ask for confirmation, and only then push — whether it uses MCP tools or shell commands under the hood.

## Auto-Loaded Guide

The `hoist-guide` skill loads automatically when the AI detects hoist context (e.g., working with `hoist.yaml`, search indexes, or Foundry agents). It provides the AI with background knowledge about:

- hoist's file structure and naming conventions
- Resource types and their relationships
- Safety rules (e.g., always preview before push)
- How to read and modify resource definitions

No user action needed — the guide activates when relevant context is detected.

## How Skills Work

Skills are markdown instruction files in `.claude/skills/` (and equivalent paths for other AI tools). They're not code — they're structured prompts that tell the AI how to accomplish specific tasks. This makes them portable across AI tools that support the [agent skills](https://agentskills.io/) convention.

## See Also

- [MCP.md](MCP.md) — MCP server setup and tool reference
- [INSTALL.md](INSTALL.md) — Installation and setup
