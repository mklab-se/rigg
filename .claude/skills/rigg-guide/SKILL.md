---
name: rigg-guide
description: Reference guide for rigg CLI — configuration-as-code for Azure AI Search and Microsoft Foundry. Auto-loaded when working with rigg.yaml, rigg workspaces/projects, search indexes, knowledge bases, or Foundry agents.
user-invocable: false
---

## rigg overview

rigg manages Azure AI Search and Microsoft Foundry configuration as code. A
**workspace** (`rigg.yaml`) defines environments — their targets, dependencies
and policy; each **project** under `projects/<name>/` owns its resource
definitions as JSON files. A resource belongs to exactly ONE project, and
`pull`/`push`/`diff` always operate on whole projects — that is what keeps local and cloud
consistent.

## Getting oriented quickly

1. `rigg_describe` (MCP) or `rigg describe --output json` — projects, every
   resource with its file path, the dependency graph, and "APIs to implement".
2. `rigg_status` — per-resource sync state (in sync / local ahead / remote
   ahead / conflict) plus unmanaged remote resources.
3. `rigg_env_list` — configured environments.

## Workspace layout

```
rigg.yaml                     # environments: targets, dependencies, policy (YAML)
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

An environment is **targets + dependencies + policy**: one Search service,
one Foundry account/project, a `dependencies:` map of infrastructure
**bindings** (`name: { storage|ai-services|function-app|identity|key-vault|api: value }`),
and `policy: { protected, strict-bindings }`. Every environment also has
implicit `search`/`foundry` bindings for free. Each project keeps a
**separate resource tree per environment** under `envs/<env>/` — dev and
prod genuinely diverge (field mappings, agent instructions), so each gets
its own full file tree rather than overlay patches. A resource's *logical*
identity is its file path (kind dir + stem, e.g. `indexes/docs-index`); the
`name` field inside the file is its *physical* Azure name and may differ per
environment. Target an environment with `-e/--env <name>` (or `RIGG_ENV`, or
the `default: true` env). Translate one environment's tree into another —
locally, with online checks unless `--offline` — with `rigg promote
[<project>] --from <A> --to <B> [--dry-run] [--offline] [--yes] [--answer
id=value]…`: infrastructure references are re-pointed at the target's own
binding of the same name (shared bindings unchanged), sibling references
follow renamed physical names, the target's own `name`/`x-rigg-pin`/Web-API
auth carrier are kept, and anything promote can't decide (an unbound
reference, a missing target binding, an external API URL with no binding, a
deployment unavailable or short on quota) becomes a question — non-interactive
callers answer with `--answer id=value` or get exit 6 (`needs-input`). A
target env that doesn't exist is not a question — it's a usage error (exit
2) naming the `rigg env add <to> --like <from>` command to run first (or,
interactively, promote offers to create it inline). Environments can be
marked `policy: { protected:
true }` in `rigg.yaml`; mutating pushes and remote deletes against a
protected environment then require an explicit `--confirm-env <name>` (or an
interactive type-to-confirm) — `--yes` alone never satisfies this gate.

**Bindings**: `rigg env bind <env> <name> <type>:<value>` declares one;
`rigg env bind <env> --learn [--yes]` scans the environment's files and
proposes bindings from the infrastructure references it finds (`adopt`/
`pull` offer this automatically when new unbound references appear); `rigg
env unbind <env> <name>` removes one; `rigg env show <env> [--refresh]`
prints resolved ARM ids and cross-environment sharing; `rigg env add <name>
--like <env>` walks the model environment's bindings asking same/different/
skip. `rigg validate` classifies every infrastructure reference: Bound/
Shared are fine, Leak (bound in another environment) is always an error,
Unbound/External are warnings that become errors under
`strict-bindings: true` (defaults to `protected`).

## Key workflows

- **Understand** → `rigg_describe`; **check state** → `rigg_status` / `rigg_diff`.
- **Change**: edit the JSON file (or `rigg new <kind> <name> -p <project>`),
  then `rigg_validate`, then `rigg_push` (preview first, `force: true` to apply).
  Push only touches semantically-changed resources, in dependency order.
- **Adopt existing Azure resources**: `rigg adopt <project> <selector>` CLI
  (selectors: `all`, a kind, or `<kind>/<name>`); via MCP, `rigg_pull` with
  `adopt: true` adopts ALL unmanaged resources into the project.
- **Promote between environments**: `rigg_promote` (preview via `--dry-run` →
  `force: true` to write); questions come back as `needs-input`, answer via
  `answers`.
- **Delete one resource**: delete its file, then push with `prune: true`.
- **Delete a whole project remotely**: `rigg_delete` (preview → `force: true`).
- **Identity/RBAC**: `rigg auth doctor [-e env]` reports every role, setting
  and network condition the environment's files require — for the service
  identities *and* for the caller — each with principal, role, ARM scope,
  file:path and the exact `az` command. Flags: `--fix` (apply what rigg
  owns, one confirmation for the batch), `--plan` (only what a push would
  create/update), `--live` (also read each indexer's last run), `--principal
  <object-id>` (a CI identity's rights instead of your own). Exit 0 ok / 4
  missing / 6 a fix needs an answer. `rigg status --auth` adds one identity
  line per environment. `rigg auth roles list|remove [-e env]` lists and
  undoes exactly the assignments rigg created (they carry
  `description: rigg:<workspace>:<env>:<reason>`).
- **Prove it works**: `rigg verify <project> [-e env]` (or `rigg push
  --verify`) runs every indexer to completion, retrieves from every
  knowledge base and asks every agent one turn; auth-shaped failures are
  attributed to the identity edge that explains them. Exit 1 on failure, and
  protected environments are gated — the runs cost money. MCP: `rigg_verify`,
  and `rigg_push` takes `verify` / `skip_auth_preflight`.
- **Keyless Web API skills**: `rigg auth easy-auth <function-app binding>
  [-e env] [--client-id <app>]` registers/reuses an Entra app, merges Easy
  Auth into the function app's `authsettingsV2` (never replacing what is
  there), and rewrites the calling skillsets to be keyless on disk. It shows
  a diff, asks, and does NOT push.

## Rules

- NEVER put keys/secrets in resource files — validation rejects them. Data
  sources use `ResourceId=` connection strings + managed identity; grant roles
  with `rigg auth doctor --fix`. Default to the search service's
  system-assigned identity (the only one Azure Storage's trusted-services
  firewall exception accepts); `rigg new <kind> <name> --identity <binding>`
  points a `data-source` or `skillset` scaffold at a user-assigned one
  instead. Where Azure still needs a runtime key, annotate the WebApiSkill
  with a key *source* — `"x-rigg-auth": "function-key"` or
  `"x-rigg-auth": "key-vault:<secret>@<key-vault binding>"` — and rigg
  fetches it at push time into the outgoing body only.
- `rigg push` runs the identity graph over its own plan before the first
  write: missing operator rights refuse (exit 4, with the `az` line), grants
  rigg may make are applied after every gate and waited out.
  `--skip-auth-preflight` opts out.
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
- `rigg az <noun> <verb>` operates the LIVE resources (vs the config
  commands): `indexer run --watch|reset|status`, `index query|stats`,
  `kb ask`, `agent ask`. Physical names, no project ownership needed;
  `--output json` for scripting. The MCP tools rigg_indexer_run/
  rigg_indexer_status/rigg_query/rigg_ask expose the same for post-push
  self-verification.
- Tab completion incl. resource names: `source <(COMPLETE=zsh rigg)` in the
  shell rc (bash/fish equivalents); candidates come from local files.
- Exit codes: 0 ok · 1 error · 2 usage · 3 validation · 4 auth · 5 drift/conflict
  · 6 needs input.
- Guided flows (e.g. the protected-environment gate) ask questions through a
  shared protocol: interactively on a terminal, or via `--answer
  <id>=<value>` (repeatable) / `--answers-file <path>` when scripted or run
  by an AI agent. Unanswered → exits 6 with a `needs-input` JSON document
  (questions: id, prompt, candidates) instead of hanging; re-run with the
  missing answers. `--confirm-env <name>` is shorthand for answering a
  protected environment's `confirm.protected.<env>` question. MCP tools take
  the same answers via an `answers: {id: value}` parameter; a `needs-input`
  result IS the tool result, not an error — answer and call again.
