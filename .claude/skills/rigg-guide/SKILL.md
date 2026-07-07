---
name: rigg-guide
description: Reference guide for rigg CLI — configuration-as-code for Azure AI Search and Microsoft Foundry. Auto-loaded when working with rigg.yaml, rigg workspaces/projects, search indexes, knowledge bases, or Foundry agents.
user-invocable: false
---

## rigg overview

rigg manages Azure AI Search and Microsoft Foundry configuration as code. A
**workspace** (`rigg.yaml`) defines environments and service connections; each
**project** under `projects/<name>/` owns its resource definitions as JSON
files. A resource belongs to exactly ONE project, and `pull`/`push`/`diff`
always operate on whole projects — that is what keeps local and cloud
consistent.

## Getting oriented quickly

1. `rigg_describe` (MCP) or `rigg describe --output json` — projects, every
   resource with its file path, the dependency graph, and "APIs to implement".
2. `rigg_status` — per-resource sync state (in sync / local ahead / remote
   ahead / conflict) plus unmanaged remote resources.
3. `rigg_env_list` — configured environments.

## Workspace layout

```
rigg.yaml                     # environments + service connections (YAML)
apis/<name>.json              # shared OpenAPI specs for custom Web API skills
projects/<name>/
  project.yaml                # metadata only — the directory IS the membership
  search/{data-sources,indexes,skillsets,indexers,synonym-maps,aliases,
          knowledge-sources,knowledge-bases}/<name>.json
  foundry/{agents,deployments,connections,guardrails}/<name>.json
  foundry/agents/<name>.instructions.md   # $file sidecar for long text
```

## Key workflows

- **Understand** → `rigg_describe`; **check state** → `rigg_status` / `rigg_diff`.
- **Change**: edit the JSON file (or `rigg new <kind> <name> -p <project>`),
  then `rigg_validate`, then `rigg_push` (preview first, `force: true` to apply).
  Push only touches semantically-changed resources, in dependency order.
- **Adopt existing Azure resources**: `rigg_pull` with `adopt: true`.
- **Delete one resource**: delete its file, then push with `prune: true`.
- **Delete a whole project remotely**: `rigg_delete` (preview → `force: true`).
- **Identity/RBAC problems**: `rigg auth doctor` (add `--fix` to repair).

## Rules

- NEVER put keys/secrets in resource files — validation rejects them. Data
  sources use `ResourceId=` connection strings + managed identity; grant roles
  with `rigg auth doctor --fix`.
- `x-rigg-*` keys are rigg-local annotations (stripped before push):
  `x-rigg-api: <spec>` links a WebApiSkill to `apis/<spec>.json` (validated);
  `x-rigg-ref: knowledge-bases/<kb>` on an agent tool injects the KB's MCP
  endpoint for the target environment at push time.
- Long text (agent instructions) lives in `.md` sidecars via
  `{"$file": "<name>.instructions.md"}` — edit the Markdown, not the JSON.
- Knowledge sources are explicit: they point at an existing index
  (`searchIndex` kind). Build data source → index → (skillset) → indexer →
  knowledge source → knowledge base step by step, pushing and testing per step.
- Index fields cannot be removed in Azure — pushing a field removal fails with
  a clear API error; to recreate: delete the file, `push --prune`, restore, push.
- Exit codes: 0 ok · 1 error · 2 usage · 3 validation · 4 auth · 5 drift/conflict.
