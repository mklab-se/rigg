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
    tenant: 11111111-1111-1111-1111-111111111111        # optional: az login's default tenant
    subscription: 00000000-0000-0000-0000-000000000000  # optional: discovery scope
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
The translated document is written exactly as translation produced it — a
target-only `x-rigg-*` annotation other than `x-rigg-pin` is not carried
over, because carrying anything over from the file being replaced would
silently undo the translation.
Sidecars are promoted as content (inline on read, extract on write). A→B and
B→A are the same operation — you choose the direction with `--from`/`--to`,
not a fixed "deploy" direction.

**Questions.** Translation stops on anything it cannot decide — a source
value that matches no binding in `A`, a binding that exists in `A` but not
`B`, an external `api` URL with no `api` binding in either environment, or a
new-in-`B` deployment that Azure reports as unavailable or short on quota in
`B`'s region. Interactively these are asked inline (answers that create
bindings are written to `rigg.yaml` once the run proceeds past the preview —
a run aborted at the confirmation loses them); non-interactively every
pending question comes back as a `needs-input` document (exit 6, nothing
written) — answer with `--answer <id>=<value>` (repeatable) or
`--answers-file <path>`. `--yes` applies a plan that has no pending
questions.

A target environment `B` that doesn't exist yet is **not** one of these
questions — it's a usage error (exit 2) up front, before translation runs,
naming the exact `rigg env add <to> --like <from>` command to create it
first (filled in with `A`'s own search service / Foundry account and
project). Interactively, `rigg promote` offers to run that wizard inline
instead of failing.

**Preview.** Always shown before writing — and the whole output for
`--dry-run`: the search/Foundry targets, a rewiring table per binding
(shared vs. changed, with reference counts), renamed siblings with the
number of references rewritten, a resource summary (changed / new /
unchanged / kept-only-in-target) with per-resource semantic diffs, and
checks (deployment availability/quota). `--dry-run`
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

## How rigg handles authentication

rigg is identity-first: **no file rigg writes ever contains a credential**,
and `rigg validate` rejects one that does. Every connection it manages is
made with a managed identity, which means the wiring that used to be a
connection string is now a *role assignment* — something rigg can derive
from your files, check against Azure, and, where it is allowed to, create
for you.

### The principals

Four kinds of identity show up in a rigg workspace:

| Principal | What it is | Where rigg finds it |
|---|---|---|
| `search-system` | the Search service's **system-assigned** managed identity — the default for every Search-side connection | ARM, on the service named by the environment's `search` target |
| `search-user:<binding>` | a **user-assigned** managed identity a file names in an `identity` / `authIdentity` / `cognitiveServices.identity` field | the environment's `identity` binding of that name |
| `foundry-project` | the Foundry **project's** system-assigned managed identity — what an agent uses to reach a knowledge base | ARM, on `<account>/projects/<project>` |
| `operator` | **you** — your `az login` user, or the service principal a CI job runs as | the access token's own claims |

The system-assigned identity is the default on purpose. It is the only
identity Azure Storage's trusted-services exception accepts (see
[below](#the-trusted-services-caveat)), and it needs no binding. Use a
user-assigned identity when you want role assignments to survive re-creating
the service, or one identity shared across environments: bind it
(`rigg env bind dev shared-mi identity:<name>`) and point a scaffold at it
with `rigg new <kind> <name> --identity shared-mi` (`data-source` and
`skillset`; other kinds get their identities from `pull` or `rigg env bind
--learn`, not from a fresh scaffold).

### The requirement graph

rigg reads every file in an environment's tree, extracts each infrastructure
reference the registry knows about, resolves it through the environment's
[bindings](#dependencies-and-bindings), and produces two things:

- **Edges** — "this principal needs this role at this ARM scope, because of
  this field in this file". An edge carries its evidence: the resource, the
  JSON path, and a sentence saying why.
- **Checks** — the settings and network conditions that are not roles but
  still gate the connection.

What the files imply, today:

| File evidence | Principal | Role |
|---|---|---|
| Data source `credentials.connectionString` (blob) | the data source's identity | Storage Blob Data Reader |
| Knowledge source `azureBlobParameters.connectionString` | its ingestion identity | Storage Blob Data Reader |
| Knowledge source `…assetStore.connectionString` | same | Storage Blob Data Contributor |
| Skillset `knowledgeStore.storageConnectionString` | the knowledge store's identity | Storage Blob Data Contributor (plus Storage Table Data Contributor and Reader and Data Access when it has table projections) |
| An embedding `resourceUri` (index vectorizer, AzureOpenAIEmbeddingSkill, knowledge-source embedding model) | that element's `authIdentity`, else system | Cognitive Services OpenAI User |
| A chat-completion `resourceUri` (knowledge-base `models[]`, knowledge-source verbalization) | same | Cognitive Services User |
| Skillset `cognitiveServices` with an `AIServicesByIdentity` `subdomainUrl` | its `identity`, else system | Cognitive Services User (the account must be of kind `AIServices`) |
| Skillset WebApiSkill with `authResourceId` | that skill's `authIdentity`, else system | *not a role* — the app must accept the audience (see [Easy Auth](#easy-auth-the-edge-rbac-cannot-cover)) |
| Agent `tools[].project_connection_id` on an MCP tool whose connection uses `ProjectManagedIdentity` | `foundry-project` | Search Index Data Reader, on the search service |
| `encryptionKey.keyVaultUri` with no explicit credential | `search-system` | Key Vault Crypto Service Encryption User |

And the checks, alongside them: the Search SKU (Free has no managed
identity; knowledge bases need Basic or higher), whether the service has an
identity at all, whether it accepts Entra tokens (`authOptions.aadOrApiKey`
or `disableLocalAuth`), the storage firewall, blob soft delete when a data
source uses `NativeBlobSoftDeleteDeletionDetectionPolicy`, whether shared-key
access is disabled (reported for context — identity-based access works
either way), the AI Services account kind, a function app's access
restrictions and its Easy Auth settings. (Model deployment availability and
quota are checked by `push`/`promote`, where the deployment is actually
written; doctor lists the row as skipped rather than repeating a check whose
answer only matters at write time.)

The **operator's** own edges come from the plan rather than from a single
field: Search Service Contributor on the search service for any Search
resource, Foundry User on the Foundry *project* for agents, Foundry Project
Manager (or Cognitive Services Contributor) on the account for connections,
and Foundry Account Owner (or Cognitive Services Contributor) for deployments
and guardrails — plus, for every edge rigg might have to grant, whether you
can create a role assignment at that scope at all. A push that will also read
the data plane (`push --verify`) adds Search Index Data Reader to its own
preflight.

An operator edge is satisfied by an assignment of that exact role, by an
assignment of one of the alternatives the edge lists, or by your *effective*
permissions at the scope covering everything one of those role definitions
grants. A subscription Owner or Contributor therefore already counts for
Search Service Contributor, Foundry Account Owner and — through the
Cognitive Services Contributor alternative — project connections. It never
counts for a role whose permissions live in `dataActions`: Foundry User,
Search Index Data Reader, and Foundry Project Manager itself all carry data
actions that `actions: ["*"]` does not cover. (Managed-identity edges keep to
the exact role: ARM will only report effective permissions for the caller.)

### Where the graph is used

The same graph runs in three places, so the answer never depends on which
command you happened to run:

```bash
rigg auth doctor -e dev                 # the whole environment, verified
rigg auth doctor -e dev --fix           # …and repair what rigg owns
rigg auth doctor -e dev --plan          # only what a push would create/update
rigg auth doctor -e dev --live          # …plus each indexer's last run
rigg auth doctor -e dev --principal <object-id>   # a CI identity's rights, not yours
rigg status --auth                      # one identity line per environment
rigg push my-rag                        # plan-scoped preflight, before the first write
rigg verify my-rag                      # proof, after the fact
```

- **`rigg auth doctor`** reports every edge and check — `✓` in place, `✗` a
  missing role, `!` a setting or network condition that does not hold, `?`
  something it could not judge, `-` deliberately checked elsewhere — each
  with its principal, role, scope, reason, the file and path that require it,
  and the exact `az` command.
  Exit **0** when everything is in place, **4** when anything is missing or
  could not be judged, **6** when `--fix` needs a confirmation it cannot ask
  for (a script, or `--output json`). `--output json` prints
  `{env, edges[], checks[], operator[], summary}`.
- **`rigg push`** runs `doctor --plan` against exactly the documents it is
  about to send, before the first mutation. Anything only a human may grant
  refuses right there (exit 4, with the `az` line); what rigg may grant is
  applied only after every gate has been cleared, and then **waited out** —
  rigg polls until the assignment is visible before it continues. `--dry-run`
  reports the whole remediation and refuses nothing. `--skip-auth-preflight`
  opts out entirely, for a caller who knows the wiring is fine and cannot
  read ARM.
- **`rigg verify <project>`** (also `rigg push --verify`) is the proof a
  green doctor is not: every indexer is run and watched to completion, every
  knowledge base gets a retrieve, every agent a one-turn question. A failure
  that looks like an authorization problem is attributed to the edge that
  would explain it. It exits 1 on any failure — and because indexer runs cost
  money, a protected environment gates it like any other mutation.

### What rigg grants, and what it never grants

`rigg auth doctor --fix` (and push's preflight) will, after one confirmation
for the whole batch:

- create role assignments **for service identities**, stamped with
  `description: "rigg:<workspace>:<env>:<reason>"` and an explicit
  `principalType`;
- enable a system-assigned identity on a service that has none;
- turn on Entra token acceptance on the search service;
- add the `AzureServices` firewall bypass or a resource-instance rule on a
  storage account, and enable blob soft delete where a policy requires it.

Because every assignment rigg makes is tagged, rigg can also take them back:

```bash
rigg auth roles list -e dev       # exactly the assignments rigg created here
rigg auth roles remove -e dev     # …and remove them (env remove --clean-roles does this too)
```

Two things rigg will **never** do:

- **Grant you your own rights.** An operator edge is always reported, never
  fixed — if `--fix` could grant the caller their own access, anyone able to
  run a push could escalate themselves. You get the `az` line and someone
  with User Access Administrator runs it.
- **Manage keys or passwords.** rigg never creates or rotates a credential,
  never reads a storage account key, and never writes one to disk.

### Easy Auth: the edge RBAC cannot cover

A custom Web API skill calling your Azure Function is not an ARM role — the
function app itself has to accept the search identity's token. `rigg auth
easy-auth <function-app binding>` wires that end to end:

```bash
rigg auth easy-auth enrich-fn -e dev
rigg auth easy-auth enrich-fn -e dev --client-id <existing app registration>
```

It registers (or reuses) an Entra application with `api://<app-id>` and a
`Caller` app role, creates the enterprise application, and PUTs a **merged**
`authsettingsV2` on the function app — other identity providers and unrelated
settings are kept, `allowedAudiences` and `allowedApplications` are unioned,
never replaced. The caller it admits is the search service's system-assigned
identity, or the user-assigned identity a skillset declares in
`authIdentity`. The merged document is shown as a diff and confirmed before
anything is written. Every skillset in the environment that calls that app is
then rewritten to be keyless on disk — `authResourceId` set, the `code=`
parameter, the `x-functions-key` header and any `x-rigg-auth` carrier removed.
Nothing is pushed: `rigg push` is still yours to run.

### When Azure still wants a key

A few things Azure has no keyless form for. Rather than storing the secret,
name the **source** it should be fetched from, on the WebApiSkill:

| Annotation | What push does |
|---|---|
| `"x-rigg-auth": "function-key"` | reads the key from ARM `listkeys` on the function app (function-level key first, host key as fallback) |
| `"x-rigg-auth": "key-vault:<secret>@<key-vault binding>"` | reads the secret from that vault's data plane with your own token (you need Key Vault Secrets User) |
| neither, and `authResourceId` set | keyless — nothing is injected |

Either way the value exists only in the outgoing request body: the file keeps
`<redacted>`, and the key never reaches disk, stdout, or a log.
`rigg push --refresh-credentials` re-injects for skillsets that are otherwise
in sync.

### The trusted-services caveat

If a storage account's firewall is set to `defaultAction: Deny`, Azure AI
Search reaches it in one of two ways: the trusted-services exception
(`bypass` including `AzureServices`), or a resource-instance rule naming the
search service. **The trusted-services exception works only with the search
service's system-assigned identity** — a user-assigned identity cannot use
it. rigg's doctor knows this: a user-assigned identity against firewalled
storage is reported as unsupported, with the two ways out (switch that
connection to the system identity, or add a resource-instance rule, which
`--fix` can do). It is the main reason the system-assigned identity, not a
shared user-assigned one, is rigg's default.

### Tokens

rigg acquires one token per (tenant, audience) — ARM, Search, the Foundry
data plane, Cognitive Services, Key Vault, Microsoft Graph — and caches it
for five minutes. The chain, highest first:

1. `RIGG_ACCESS_TOKEN` — a pre-minted bearer token, honoured for **every**
   audience. Intended for CI and test rigs.
2. Service-principal environment variables — `AZURE_CLIENT_ID` and
   `AZURE_TENANT_ID` plus either `AZURE_CLIENT_SECRET` or
   `AZURE_FEDERATED_TOKEN_FILE` (OIDC). Tokens are minted directly from
   Entra ID; the Azure CLI does not have to be installed.
3. Your Azure CLI login (`az login`), per tenant. An environment that names
   a `tenant` you are not signed in to says so, and names the
   `az login --tenant <t>` that fixes it.

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
