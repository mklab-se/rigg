---
name: hoist-guide
description: Reference guide for hoist CLI — configuration-as-code for Azure AI Search and Microsoft Foundry. Auto-loaded when working with hoist.yaml, search indexes, Foundry agents, or Azure AI Search configuration.
user-invocable: false
---

## hoist overview
hoist manages Azure AI Search and Microsoft Foundry configuration as code.
Resources are stored as local JSON/YAML files and synced with Azure.

## Getting oriented quickly
Use `hoist_describe` MCP tool first — it returns a complete project summary including
all resources, their dependencies, agent configurations, and knowledge base flows.
This is the fastest way for an AI agent to understand the full project context.

Use `hoist_status` for environment info, auth state, and resource counts.
Use `hoist_env_list` to see all configured environments.

## File structure
- `hoist.yaml` — project config with named environments
- `search/search-management/indexes/` — search index definitions (JSON)
- `search/agentic-retrieval/knowledge-sources/` — knowledge source definitions
- `search/agentic-retrieval/knowledge-bases/` — knowledge base definitions
- `foundry/agents/` — Foundry agent definitions (YAML, matches portal format)

## Key workflows
- Pull: `hoist_pull` MCP tool (preview first without force, then force to execute)
- Push: always validate, diff, then push (use the `/hoist-push` skill)
- Diff: `hoist_diff` MCP tool for comparing local vs remote
- Environments: `hoist_env_list` to see all, pass `env` param to target specific one

## Safety rules
- Always validate before pushing
- Always diff before pushing
- Pull before push to detect conflicts
- Knowledge source changes cascade to managed sub-resources (index, indexer, datasource, skillset)
