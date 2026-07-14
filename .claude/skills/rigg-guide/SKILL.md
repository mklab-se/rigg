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
  envs/<env>/
    search/{data-sources,indexes,skillsets,indexers,synonym-maps,aliases,
            knowledge-sources,knowledge-bases}/<name>.json
    foundry/{agents,deployments,connections,guardrails}/<name>.json
    foundry/agents/<name>.instructions.md   # $file sidecar for long text
```

## Environments

Each project keeps a **separate resource tree per environment** under
`envs/<env>/` — dev and prod genuinely diverge (field mappings, agent
instructions), so each gets its own full file tree rather than overlay
patches. A resource's *logical* identity is its file path (kind dir + stem,
e.g. `indexes/docs-index`); the `name` field inside the file is its
*physical* Azure name and may differ per environment. Target an environment
with `-e/--env <name>` (or `RIGG_ENV`, or the `default: true` env). Copy one
environment's tree into another — locally, without touching Azure — with
`rigg promote <project> --from <env> --to <env>` (pinned fields like `name`
and secrets are preserved on the target, not overwritten). Environments can
be marked `policy: { protected: true }` in `rigg.yaml`; mutating pushes and
remote deletes against a protected environment then require an explicit
`--confirm-env <name>` (or an interactive type-to-confirm) — `--yes` alone
never satisfies this gate.

## Key workflows

- **Understand** → `rigg_describe`; **check state** → `rigg_status` / `rigg_diff`.
- **Change**: edit the JSON file (or `rigg new <kind> <name> -p <project>`),
  then `rigg_validate`, then `rigg_push` (preview first, `force: true` to apply).
  Push only touches semantically-changed resources, in dependency order.
- **Adopt existing Azure resources**: `rigg adopt <project> <selector>` CLI
  (selectors: `all`, a kind, or `<kind>/<name>`); via MCP, `rigg_pull` with
  `adopt: true` adopts ALL unmanaged resources into the project.
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
- Portal-created indexed knowledge sources (azureBlob, azureSql, ...) hide an
  Azure-generated pipeline. `rigg migrate knowledge-source <name>` (alias
  `ks`) converts them to explicit `searchIndex` form: `--in-place` keeps all
  names (the next push REPLACES the knowledge source — delete + recreate, the
  index is REBUILT; gated behind `--allow-replace` non-interactively), or
  `--rename <new>` builds a side-by-side pipeline under new names while the
  old one keeps serving (cut over the knowledge base, then delete the old KS
  file and `push --prune`). Push orchestrates knowledge-base unlink/relink
  automatically and resumes an interrupted replace on the next run.
- Index fields cannot be removed in Azure — pushing a field removal fails with
  a clear API error; to recreate: delete the file, `push --prune`, restore, push.
- Exit codes: 0 ok · 1 error · 2 usage · 3 validation · 4 auth · 5 drift/conflict.
