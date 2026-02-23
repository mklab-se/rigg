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

## Knowledge sources — managed sub-resources (IMPORTANT)
Knowledge sources automatically provision and manage sub-resources: an index, indexer,
data source, and skillset. These are created by Azure as part of the knowledge source lifecycle.

**Do NOT create or push managed sub-resources separately.** When pushing knowledge sources,
use `--knowledgesources` (or resource_type='knowledgesources' in MCP). hoist handles the
entire cascade automatically — KS definition plus all managed sub-resources in the correct order.

If you push the sub-resources (index, skillset, etc.) manually before pushing the knowledge
source, the KS creation will fail because those resources already exist.

### Knowledge source updates (known Azure limitation)
Azure has a known bug where updating a knowledge source triggers recreation of its managed
sub-resources (index, indexer, data source, skillset). This fails if sub-resources already exist.

Workaround (via MCP):
1. `hoist_delete` with `resource_type='knowledgesources'`, `name='<name>'`, `target='remote'`, `force=true`
   (pass `env='<name>'` to target a specific environment, e.g. `env='prod'`)
2. `hoist_push` with `resource_type='knowledgesources'`, `force=true`

Workaround (via CLI):
1. `hoist delete --knowledgesource <name> --target remote` (use `--env <name>` for specific env)
2. `hoist push --knowledgesources`

### Deleting resources
`hoist delete` (CLI) and `hoist_delete` (MCP) require specifying where to operate:
- `--target remote` / `target='remote'` — deletes from the Azure service only. Local files are NOT affected.
- `--target local` / `target='local'` — removes local files only. Azure resources are NOT affected.
  Local files are shared across all environments — removing them affects all envs.

After deleting, use push or pull to sync the other side.

WARNING: Deleting a knowledge source from Azure removes the search index and all its data.
Re-indexing occurs automatically but takes time and may incur costs. To change managed
sub-resources (index schema, skillset skills), edit those files directly and push with
`--indexes`, `--skillsets`, etc.
