# `rigg.yaml`

`rigg.yaml` is the workspace file. It sits at the root of your repository and
declares **environments** — named deployments, each with one Azure AI Search
target, one Microsoft Foundry target, a policy, and a table of *dependency
bindings* naming the supporting Azure resources (storage accounts, function
apps, key vaults, identities) that this environment's resource definitions
point at. It holds no resource definitions of its own and never any secrets;
finding `rigg.yaml` is also how rigg decides it is inside a workspace at all
(every command walks up from the current directory until it finds one).

## Complete example

```yaml
name: contoso-rag                 # optional label, shown by `rigg describe`

environments:
  dev:
    default: true                 # used when neither --env nor RIGG_ENV says otherwise
    tenant: 00000000-0000-0000-0000-000000000000
    subscription: 11111111-1111-1111-1111-111111111111
    search:
      service: contoso-search     # https://contoso-search.search.windows.net
    foundry:
      account: contoso-foundry    # https://contoso-foundry.services.ai.azure.com
      project: proj-default
    dependencies:
      docs-storage:  { storage: contosostorage }
      enrich-fn:     { function-app: contoso-enrich-fn }
      secrets:       { key-vault: contoso-kv }
      indexer-identity: { identity: contoso-indexer-id }
      partner-api:   { api: https://api.contoso-partner.example/v1 }

  prod:
    tenant: 00000000-0000-0000-0000-000000000000
    subscription: 22222222-2222-2222-2222-222222222222
    search:
      service: contoso-search-prod
    foundry:
      account: contoso-foundry-prod
      project: proj-default
    policy:
      protected: true             # every cloud mutation needs a typed confirmation
      strict-bindings: true       # (already implied by `protected: true`)
    dependencies:
      # A full ARM id is the unambiguous form across subscriptions.
      docs-storage:
        storage: /subscriptions/22222222-2222-2222-2222-222222222222/resourceGroups/contoso-prod-rg/providers/Microsoft.Storage/storageAccounts/contosostorageprod
      enrich-fn: { function-app: contoso-enrich-fn-prod }
      secrets:   { key-vault: contoso-kv-prod }
```

## Top level

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `name` | string | no | — | A label for the workspace. Cosmetic. |
| `root` | string | no | alongside `rigg.yaml` | Subdirectory (relative to `rigg.yaml`) that holds `projects/`, `apis/` and `.rigg/`. Set by `rigg init <folder>` when you keep rigg's trees in a subfolder of a larger repository. |
| `environments` | map | no | `{}` | Environment name → [environment](#environments). |

Unknown keys are rejected: every struct in `rigg.yaml` is parsed with
`deny_unknown_fields`, so a typo is a load error naming the accepted keys
rather than a silently ignored setting.

## Environments

An environment is one deployment target. Its name is the map key (`dev`,
`staging`, `prod`, …) and is what you pass to `--env`.

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `default` | bool | no | `false` | Marks this environment as the one used when no selection is made. At most one environment should set it. |
| `tenant` | string (GUID or domain) | no | the Azure CLI's current tenant | Entra ID tenant the environment's resources live in. Tokens are minted for this tenant; a tenant you are not signed in to produces an error naming the `az login --tenant <t>` that fixes it. |
| `subscription` | string (GUID) | no | discovered | Azure subscription the environment's resources live in. Used to scope ARM lookups (binding resolution, role assignments, deployments, connections). |
| `search` | map | no | — | The [Azure AI Search target](#the-search-target). Omit it for a Foundry-only environment. |
| `foundry` | map | no | — | The [Foundry target](#the-foundry-target). Omit it for a Search-only environment. |
| `policy` | map | no | `{}` | [Policy gates](#policy). |
| `dependencies` | map | no | `{}` | [Dependency bindings](#dependencies). |

### Environment resolution

Exactly one environment is selected per invocation, in this order:

1. an explicit `--env <name>` / `-e <name>` flag;
2. the `RIGG_ENV` environment variable;
3. the environment with `default: true`.

If none of the three yields a name:

```
Error: no default environment configured; pass --env or set `default: true` on one environment
```

A name that is not in the file:

```
Error: unknown environment 'staging' (available: dev, prod)
```

Two commands deliberately do **not** accept `default: true` as good enough,
because acting on the wrong environment there is expensive. `rigg adopt`
(and any flow that adopts) requires an explicit `--env` / `RIGG_ENV` when the
workspace has more than one environment — interactively it asks which one,
otherwise:

```
Error: multiple environments configured (dev, prod); pass --env <name> (or set RIGG_ENV) to say which one to adopt from
```

### The `search` target

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `service` | string | **yes** | — | Azure AI Search service name. |
| `name` | string | no | — | A label for the target. Cosmetic. |
| `endpoint` | string (URL) | no | `https://{service}.search.windows.net` | Full base URL override — sovereign clouds, and the wiremock fake used by rigg's own tests. |
| `api-version` | string | no | `2026-04-01` | Override for the stable data-plane api-version. |
| `preview-api-version` | string | no | `2026-08-01-preview` | Override for the preview data-plane api-version (knowledge bases require the preview channel). |

Change an api-version only when you know why. rigg's registry pins the pair
it has schema fixtures for; `rigg dev api-check` reports when Azure has moved
on.

### The `foundry` target

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `account` | string | **yes** | — | Foundry (Azure AI Services) account name. |
| `project` | string | **yes** | — | Foundry project name, e.g. `proj-default`. |
| `name` | string | no | — | A label for the target. Cosmetic. |
| `endpoint` | string (URL) | no | `https://{account}.services.ai.azure.com` | Full base URL override. |
| `api-version` | string | no | `v1` | Override for the Foundry data-plane api-version. |

Model deployments, connections and guardrails are ARM resources under the
same account and use the `Microsoft.CognitiveServices` api-version pinned in
the registry (`2026-05-01`), not this one.

## Policy

Per-environment gates. Omit the block entirely for an unguarded environment.

| Key | Type | Required | Default | Meaning |
|---|---|---|---|---|
| `protected` | bool | no | `false` | Every cloud-mutating operation against this environment (`push` apply, `push --prune`, `delete --remote`, `az indexer run`/`reset`) requires the environment's name to be typed back. |
| `strict-bindings` | bool | no | the value of `protected` | Every infrastructure reference in the environment's files must resolve to a declared `dependencies` binding before push. When false, an unbound reference is a warning. |

`--yes` deliberately does **not** satisfy the protected gate. `--yes` exists
to skip the routine "apply N change(s)?" prompt and scripts reach for it
reflexively; if it also unlocked protection, a protected environment would be
no safer than an unprotected one. The confirmation must be given per
invocation, as a typed name at the prompt or as
`--confirm-env <env>` / `--answer confirm.protected.<env>=<env>` in a script.
See [Exit codes and questions](exit-codes-and-questions.md#needs-input).

## Dependencies

A `dependencies` entry gives a **name** to a supporting Azure resource that
this environment's resource definitions point at. Resource files carry the
physical value (a connection string, a function URL, a vault URI); the
binding is what lets rigg recognise that value, check you have access to it,
and — during `rigg promote` — swap it for the target environment's equivalent
of the same name.

Each entry is exactly one `<type>: <value>` pair:

```yaml
dependencies:
  docs-storage: { storage: contosostorage }
```

| Type | Points at | Example value |
|---|---|---|
| `storage` | A storage account (blob / ADLS Gen2) | `contosostorage` |
| `ai-services` | An Azure AI Services / OpenAI account hosting models | `contoso-foundry` |
| `function-app` | An Azure Functions app hosting Web API skills | `contoso-enrich-fn` |
| `identity` | A user-assigned managed identity | `contoso-indexer-id` |
| `key-vault` | A key vault holding secrets rigg reads at push time | `contoso-kv` |
| `api` | An external HTTP API rigg does not own | `https://api.contoso-partner.example/v1` |

**Binding values** may be written three ways, and rigg classifies them by
shape:

| Shape | Recognised when it | Resolves to |
|---|---|---|
| Bare name | is neither of the other two | the resource of that name, discovered via ARM in the environment's subscription |
| ARM resource id | starts with `/subscriptions/` | exactly that resource — the unambiguous form for multi-subscription workspaces |
| URL | starts with `http://` or `https://` | the host; used for `api` bindings and for endpoint matching |

**Binding names** must be lowercase kebab-case: ASCII letters, digits and
single hyphens, no leading or trailing hyphen, no double hyphen. `search` and
`foundry` are reserved — they already name the environment's own targets:

```
Error: rigg.yaml found at /repo/rigg.yaml but could not be read: environment 'dev' has an invalid dependency binding name 'Docs_Storage': binding name 'Docs_Storage' must be lowercase kebab-case (letters, digits, single hyphens; no leading/trailing hyphen)
```

### Implicit bindings

Every environment has two bindings you do not declare:

| Name | Comes from | ARM type |
|---|---|---|
| `search` | `search.service` | `Microsoft.Search/searchServices` |
| `foundry` | `foundry.account` | `Microsoft.CognitiveServices/accounts` |

They appear in `rigg env show` alongside the declared ones, and an
infrastructure reference pointing at the search service or the Foundry
account resolves through them without any `dependencies` entry. An
`ai-services` binding and the implicit `foundry` binding compete for the same
role (both can host a model deployment), which is why an environment may
legitimately bind both to the same account.

### Managing bindings from the CLI

```bash
rigg env list
rigg env show dev
rigg env show dev --refresh
rigg env add staging --search-service contoso-search-staging --protected
rigg env add staging --like dev --skip docs-storage
rigg env bind dev docs-storage storage:contosostorage
rigg env bind dev --learn
rigg env unbind dev docs-storage
rigg env set-default dev
rigg env remove staging
```

`rigg env add` writes a new environment block. `rigg env bind` adds or
replaces one `dependencies` entry; `--learn` instead scans the environment's
resource files for infrastructure references that no binding covers and
proposes a name for each (question ids `learn.<env>.<resource>`, then
`learn.<env>.record` to write them). `rigg env show --refresh` re-resolves
every binding against Azure and rewrites the [bindings
cache](state.md#bindings-cache); it never edits `rigg.yaml`.

## Multi-subscription and multi-tenant workspaces

Environments are independent: each carries its own `tenant` and
`subscription`, and tokens are minted per `(tenant, audience)` pair. Two
patterns are common.

**Same tenant, one subscription per stage** — the usual case. Set
`subscription` per environment and use bare binding names; ARM discovery
stays inside the right subscription:

```yaml
environments:
  dev:
    subscription: 11111111-1111-1111-1111-111111111111
    search: { service: contoso-search }
    dependencies:
      docs-storage: { storage: contosostorage }
  prod:
    subscription: 22222222-2222-2222-2222-222222222222
    search: { service: contoso-search-prod }
    dependencies:
      docs-storage: { storage: contosostorageprod }
```

**Separate tenants** — set `tenant` per environment and sign in to both
(`az login --tenant <t>`). Prefer full ARM ids for bindings here: a bare name
is resolved by search, and searching the wrong tenant is the failure mode you
want to design out.

## Common mistakes

**A key on the wrong level.** Every block rejects unknown fields, and the
error names the ones it accepts:

```
Error: rigg.yaml found at /repo/rigg.yaml but could not be read: failed to parse /repo/rigg.yaml: environments.dev.search: unknown field `protected`, expected one of `name`, `service`, `endpoint`, `api-version`, `preview-api-version` at line 7 column 7
```

`protected` belongs under `policy:`, not under `search:`.

**A binding written as several keys.** A binding is one pair, so this is a
parse error — *a binding is exactly one `<type>: <value>` pair*:

```yaml
dependencies:
  docs-storage:
    storage: contosostorage
    identity: contoso-indexer-id   # wrong: declare a second binding instead
```

**A list of targets.** `search:` and `foundry:` are single maps, not lists.
One environment has one Search service and one Foundry project; model a
second service as a second environment.

**No environment marked default, and no `--env`.** Every command that touches
a target needs one:

```
Error: no default environment configured; pass --env or set `default: true` on one environment
```

**Naming a binding `search` or `foundry`.** Those names are taken by the
implicit bindings; pick a descriptive name (`docs-storage`, `enrich-fn`).

**Expecting `--yes` to unlock a protected environment.** It does not; see
[Policy](#policy).

## See also

- [`CONCEPTS.md`](../../CONCEPTS.md) — environments, dependencies and bindings in prose.
- [Resource files](resource-files.md) — the infrastructure fields a binding resolves.
- [State](state.md#bindings-cache) — where resolved bindings are cached.
- [Exit codes and questions](exit-codes-and-questions.md) — the `binding.` / `env.` / `learn.` question ids these flows ask.
- [CLI reference](cli.md#rigg-env) — every `rigg env` flag.
- [Tutorial 3 — add an environment and promote](../tutorials/03-add-an-environment-and-promote.md).
