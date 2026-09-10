# Concepts

rigg has two levels: a **workspace** and its **projects**. Understanding the
split is the key to using rigg well.

## Workspace vs project

- A **workspace** (`rigg.yaml`) is the top level. It declares your
  **environments** (dev, test, prod) and, per environment, its **targets,
  dependencies and policy** — which Azure AI Search service and Microsoft
  Foundry account/project it points at, the infrastructure its files may
  reference, and how carefully rigg must treat mutations against it — plus
  shared assets like `apis/`. A workspace holds *no* resource definitions
  itself.
- A **project** (`projects/<name>/`) is a **named group of resource
  definitions you pull, push, diff, review, and deploy as one unit**. Indexes,
  indexers, skillsets, knowledge bases, agents, and model deployments live as
  files inside a project.
- **A resource belongs to exactly one project.** rigg enforces this. It is what
  makes sync unambiguous: when you push a project, rigg knows exactly which
  remote resources that project owns — so it never half-syncs or fights another
  project over the same resource.

## Why two levels?

The workspace answers *"where do things go?"* — which services and
environments, shared across everything. Projects answer *"what do I manage
together?"* — the unit of change, review, and deployment.

Separating them means you can promote one coherent project from dev to prod
without dragging along unrelated resources, and different projects can be owned
and reviewed independently while sharing the same service and environment
configuration.

## One or many projects? Choosing boundaries

Use **one** project when your whole stack ships and is reviewed together — for
example, a single agent plus the retrieval pipeline it depends on.

Use **several** projects to draw boundaries you care about:

- **By deployable unit** — each agent or app that ships independently.
- **By ownership / review scope** — a team owns its project; pull requests stay
  focused on one project's files.
- **By lifecycle** — group things that change on the same cadence; separate
  things that don't.

Rule of thumb: **if you would pull, push, and review it as a unit, it is a
project.** If two things never need to deploy together, they can be separate
projects.

Because a resource lives in exactly one project, a *shared* resource goes in
the project that owns it; other projects refer to it by name and environment
rather than co-owning it.

**Naming:** Name a project after the thing it owns — a project holding the
`regulus` agent and its retrieval stack is naturally called `regulus`. Names
follow the same rules as resource names (no `/` or `\`, at most 260
characters).

## Workspace layout

```
rigg.yaml                     # workspace: environments (targets, dependencies, policy)
apis/<name>.json              # shared OpenAPI specs for custom Web API skills
projects/<name>/
  project.yaml                # metadata only — the directory IS the membership
  envs/<env>/
    search/{data-sources,indexes,skillsets,indexers,synonym-maps,aliases,
            knowledge-sources,knowledge-bases}/<name>.json
    foundry/{agents,deployments,connections,guardrails}/<name>.json
.rigg/<env>/<project>/...     # per-environment sync state (gitignored)
```

Platform-provided resources (such as Microsoft's built-in guardrail policies)
are never adopted or listed as unmanaged — rigg only tracks configuration you
can actually change. Your resources reference them by name instead. The same
applies to sub-resources that Azure creates automatically (for example the
index and indexer behind a managed-ingestion knowledge source) — manage the
knowledge source; Azure manages what it generates.

## Environments

An **environment** is **targets + dependencies + policy**: which Azure AI
Search service and Microsoft Foundry account/project it points at, which
other pieces of Azure infrastructure its resources are allowed to reference,
and how carefully rigg must treat mutations against it. Environments are
declared under `environments:` in `rigg.yaml`:

```yaml
environments:
  dev:
    default: true
    tenant: 72f988bf-86f1-41af-91ab-2d7cd011db47        # optional: az login's default tenant
    subscription: fa354123-c4ee-4b2e-a700-bf01decf803a  # optional: discovery scope
    search:  { service: my-search-dev }
    foundry: { account: my-foundry, project: my-project-dev }
    policy:  { protected: false }
    dependencies:
      docs-storage: { storage: my-storage-dev }
      enrich-fn:    { function-app: my-enrich-fn }
  prod:
    tenant: 9a3c…                                       # a different tenant is allowed
    subscription: 0b1d…
    search:  { service: my-search-prod }
    foundry: { account: my-foundry, project: my-project-prod }
    policy:  { protected: true }
    dependencies:
      docs-storage: { storage: my-storage-prod }
      enrich-fn:    { function-app: my-enrich-fn }        # same value in both ⇒ shared
```

Every project keeps a **separate resource tree per environment**, rooted at
`envs/<env>/` (see the layout above). This is deliberate: dev and prod
genuinely diverge — different field mappings while you're testing, different
agent instructions before a rollout — and a full tree, rather than a shared
file with overlay patches, makes that divergence something you can see and
diff instead of logic hidden behind a merge step.

- **Targets** — exactly one `search` and one `foundry` per environment
  (either may be absent when a project only uses one service). Two Search
  services means two environments, not a list.
- **Tenant / subscription** — optional. When present, ARM discovery and
  token acquisition are scoped to them; when absent, rigg uses the Azure CLI
  default tenant and searches every subscription visible in it. Environments
  in different subscriptions or tenants are fully supported.
- **Policy** — `protected` (see [below](#protected-environments)) and
  `strict-bindings` (see [Validation classes](#validation-classes)).

### Logical identity vs. physical name

A resource is identified by **where its file lives**: the kind directory and
file stem (e.g. `envs/dev/search/indexes/docs-index.json` → logical id
`indexes/docs-index`) — that file path is its identity across environments.
The `name` field inside the file is a different thing: the resource's
**physical** name, what Azure actually calls it. The two usually match, but
don't have to — `envs/dev/search/indexes/docs-index.json` can have
`"name": "docs-index-dev"` while its `prod` counterpart, at the same logical
path, has `"name": "docs-index"`. rigg correlates the two files by path, not
by name, so renaming a resource in one environment never breaks its link to
the same resource in another.

### Dependencies and bindings

Resource files reference infrastructure outside Search and Foundry — a
storage account in a data source's connection string, a Key Vault in an
encryption key, a function app in a custom skill's URL. A **binding** gives
that infrastructure a name in `dependencies:`, mapping a *binding name* to
`{ <type>: <value> }`. The recognized types are `storage`, `ai-services`,
`function-app`, `identity`, `key-vault`, and `api` (an external REST base
URL, matched by prefix — no ARM lookup). A name is resolved through ARM
(account/site/vault/identity name, or you can write the full ARM id
directly) in the environment's subscription, and the resolved id is cached
in `.rigg/<env>/bindings.json` (gitignored).

Binding names correlate across environments exactly as file paths correlate
resources: the same name in `dev` and `prod` is the same *role*, possibly
played by a different physical resource. **Shared** is simply the same
physical resource bound in two environments — the binding names may differ,
and an `ai-services` binding counts as sharing another environment's implicit
`foundry` target when both name the same account. It is explicit, never
inferred; **different** just means the values differ, as `docs-storage` does
above.

Every environment also has two **implicit bindings** for free: `search` (its
own Search service) and `foundry` (its own Foundry account), usable wherever
a binding of type `ai-services` or the Search endpoint is expected. Most
workspaces need no separate `ai-services` binding at all — the Foundry
account that hosts the project usually also hosts the models.

**Declare or learn — both are first-class.** Write `dependencies:` by hand
(or let an AI write it) and rigg resolves and validates against it on first
use — configuration first. Or run `rigg env bind <env> --learn`, which scans
that environment's files, extracts every infrastructure reference, groups it
by (type, physical resource), proposes a binding name per group (the
resource's name, lower-kebab-cased — rename any of them before confirming),
and writes the result to `rigg.yaml` — discovery first. `adopt` and `pull`
run the same scan after writing files and offer the learn step when new
unbound references appear.

```bash
rigg env bind dev docs-storage storage:my-storage-dev   # declare one binding
rigg env bind dev --learn                                # propose bindings from the files
rigg env bind dev --learn --yes                          # accept the proposal non-interactively
rigg env unbind dev docs-storage                          # remove a binding
rigg env show dev                                         # targets, policy, every binding + resolved id
rigg env show dev --refresh                               # re-resolve every binding against Azure
rigg env add prod --like dev                              # walk dev's bindings: same, pick another, or skip
```

`rigg env add <name> --like <env>` is the fastest way to stand up a new
environment: it asks, per binding in the model environment, whether the new
environment shares the same physical resource, points at a different one
(ARM pick-list), or should skip that binding entirely (`--same`/`--skip` on
the command line answer the same questions non-interactively). `rigg
describe` also prints an infrastructure section per environment.

### Validation classes

`rigg validate` classifies every infrastructure reference it finds in every
file:

| Class | Meaning | Severity |
|---|---|---|
| Bound | matches a binding of this environment (or the implicit `search`/`foundry`) | ok |
| Shared | bound here and also in another environment with the same value | ok (listed with `--show-bindings`) |
| Leak | bound in **another** environment, not this one | **error** — the file points at another environment's infrastructure |
| Unbound | matches no binding anywhere | warning; **error** when `policy.strict-bindings: true` |
| External | an `api`-form reference with no matching `api` binding | warning (same strictness as Unbound) |

`strict-bindings` defaults to the value of `protected`, so a protected
environment is strict by default; set it explicitly to change that. An
error names the file, the path, the physical value, and (for a leak) the
environment that owns it, plus the fix: `rigg env bind <env> --learn` or
`rigg env bind <env> <name> <type>:<value>`. `push` runs the same
classification on its plan as a preflight and refuses on error before any
mutation.

### Promoting between environments

`rigg promote` produces, for every logical resource in environment `A`, the
document it should have in `B` — by **translation**, not by copying:

```bash
rigg promote --from dev --to prod --dry-run          # preview only (project optional when there is exactly one)
rigg promote my-rag --from dev --to prod             # write prod's tree
rigg push my-rag --env prod                          # then sync it to Azure
```

For each field, translation picks exactly one of:

1. **Infrastructure translation** — every infrastructure reference (a
   registry-recognized `InfraRef`: storage, ai-services, function-app,
   identity, key-vault, api) is parsed to its physical resource, mapped to
   the binding name it has in `A`, and rendered from `B`'s binding of the
   same name. The same physical value in both environments is **shared** —
   no change, but still listed in the preview.
2. **Sibling translation** — every reference to another resource by physical
   name (an indexer's data source/index/skillset, a knowledge base's
   knowledge sources, an agent's deployment/connection, `x-rigg-ref`, and the
   knowledge-base name inside a `SearchKbMcpUrl`) is rewritten to that
   sibling's physical name in `B`, correlated by logical id (file stem). If
   the sibling doesn't exist in `B` yet, it is created in this same promote
   under `A`'s physical name, so the reference is already correct.
3. **Kept from the target** — the resource's own `name` (physical identity is
   never promoted), any path listed in the target file's `x-rigg-pin`
   annotation (1.x array semantics apply: target-only array elements along a
   pinned path survive), and the target file's own `x-rigg-pin`.
4. **Re-derived from the target's infrastructure** — a Web API skill's auth
   carrier. Once its URI is translated to `B`'s function app: if `B`'s
   skillset file already carries an auth carrier for that skill
   (`authResourceId`, `x-rigg-auth`, or an `x-functions-key` header), that
   carrier is kept; otherwise it is derived from the target function app's
   Easy Auth state (ARM `authsettingsV2`, online): Easy Auth on →
   `authResourceId` set, key carrier removed; off → `x-rigg-auth:
   function-key` when `A` used a key, else anonymous. `A`'s `x-rigg-auth`,
   `authResourceId` and key header never cross as-is. With `--offline`, the
   carrier is left unresolved and reported; `push`'s auth gate handles it.
5. **Everything else** comes from `A` — that is the promotion.

Resources that only exist in `B` are never touched, and nothing is deleted.
Sidecars are promoted as content (inline on read, extract on write). A→B and
B→A are the same operation — you choose the direction with `--from`/`--to`,
not a fixed "deploy" direction.

**Questions.** Translation stops on anything it cannot decide — a source
value that matches no binding in `A`, a binding that exists in `A` but not
`B`, a target environment that doesn't exist yet, a new-in-`B` deployment
that Azure reports as unavailable or short on quota in `B`'s region, or an
external `api` URL with no `api` binding. Interactively these are asked
inline (answers that create bindings are written to `rigg.yaml`
immediately); non-interactively every pending question comes back as a
`needs-input` document (exit 6, nothing written) — answer with `--answer
<id>=<value>` (repeatable) or `--answers-file <path>`. `--yes` applies a plan
that has no pending questions.

**Preview.** Always shown before writing — and the whole output for
`--dry-run`: the search/Foundry targets, a rewiring table per binding
(shared vs. changed, with reference counts), renamed siblings with the
number of references rewritten, a resource summary (changed / new /
unchanged / kept-only-in-target) with per-resource semantic diffs, and
checks (deployment availability/quota, sidecar content changes). `--dry-run`
still runs the online checks — pass `--offline` too for a network-free
preview, which may still ask questions from what's already on disk.
`--output json` carries the same sections as documented keys: `targets`,
`rewiring[]`, `renamed[]`,
`resources{changed,new,unchanged,kept_only_in_to}`, `checks[]`,
`questions[]`, `dry_run`.

**`--offline`** skips every Azure lookup (candidate lists for binding
questions, Web API auth re-derivation, deployment availability/quota);
unresolved items are reported in the preview instead of guessed at.

After a successful (non-dry-run) promote, rigg hints at the next steps in
order: `rigg validate <project>`, `rigg auth doctor -e <to>`, `rigg push
<project> -e <to> --dry-run`, `rigg push <project> -e <to>`.

`rigg diff --compare-env` remains a *raw* environment-vs-environment
comparison — "equal modulo infrastructure" is what `promote --dry-run`
shows, so there is no separate compare mode for it.

### Protected environments

Marking an environment `policy: { protected: true }` requires an explicit,
per-invocation confirmation before rigg mutates it: `push` (create/update or
`--prune`) and `delete --remote`.

```bash
rigg push my-rag --env prod --yes                        # exits 6: prod is protected, asks to be named
rigg push my-rag --env prod --yes --confirm-env prod     # proceeds
```

Interactively, rigg instead prompts you to type the environment's name.
`--yes` alone never satisfies a protected environment's gate — it only skips
the routine "apply N changes?" prompt, and scripts reach for it reflexively;
if it also cleared this gate, a protected environment would be no safer than
an ordinary one.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | error |
| 2 | usage error |
| 3 | validation failed |
| 4 | auth / permission denied |
| 5 | drift or conflict detected |
| 6 | needs input |

**When rigg needs an answer.** Guided flows ask questions. On a terminal rigg
prompts. In scripts and from AI agents rigg cannot prompt, so it prints a
`needs-input` JSON document listing the questions (id, prompt, candidates)
and exits 6. Re-run with `--answer <id>=<value>` (repeatable) or
`--answers-file <path>`; answered questions are never asked again. A
protected environment's typed confirmation is such a question
(`confirm.protected.<env>`); `--confirm-env <env>` remains as shorthand.

rigg treats a session as non-interactive when stdin or stdout is not a
terminal, with `--non-interactive`, `--yes` or `--output json`, or when
`RIGG_NON_INTERACTIVE=1` is set — the environment variable is the way to
force script behaviour while still sitting at a terminal. Answers supplied
up front are always used, in either mode: a value that is *wrong* (a
`--confirm-env` that doesn't equal the environment's name, an `--answer`
outside a question's candidates) is a usage error (exit 2) naming what was
expected — on a terminal too, where rigg deliberately does not fall back to
prompting for a value you already tried to give.

## See also

- **Getting Started** (`GETTING_STARTED.md`) — build a stack from scratch.
- Run `rigg describe` to see how your resources connect, and `rigg status` to
  see what is in sync.
