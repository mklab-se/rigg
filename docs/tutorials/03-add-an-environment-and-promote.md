# Tutorial 3 — Add an environment and promote to it

You have a working `dev` stack and now need a `staging` one: the same shape,
its own infrastructure, and the agent instructions you reviewed there. Copying
files would be wrong, because every file is full of dev's infrastructure. This
tutorial shows the operation that replaces copying: `rigg promote`, which
*translates* one environment's tree into another.

| | |
|---|---|
| **Time** | About 25 minutes. |
| **Cost** | A second copy of the stack — another index and another indexer run. Everything else is configuration. |
| **You need** | • A workspace with a working `dev` environment and at least one project — the output of [tutorial 1](01-pull-an-existing-solution.md) or [tutorial 2](02-build-from-scratch.md). Examples use the project `docs-rag`<br>• Targets for staging: a Search service and a Foundry project. Reusing dev's is what this page does; a separate `contoso-search-stg` works identically<br>• `az login` against a tenant that can see both. Staging may live in a different subscription — or a different tenant — and rigg supports that; pass `--tenant`/`--subscription` in step 1<br>• Role: `Search Service Contributor` on staging's Search service<br>• Role: `Azure AI User` on staging's Foundry project<br>• Role: the ability to grant `Storage Blob Data Reader` on the storage account, or someone who can |
| **You get** | A staging tree derived from dev, rewired to staging's own infrastructure and pushed. |

> [!NOTE]
> This walkthrough keeps staging on dev's services. Staging is usually a second
> Search service, often in a second subscription, and rigg handles that as the
> same command with different answers in step 1. But the interesting half of
> promote — the rewiring, the renaming, the questions — is visible either way,
> and running staging beside dev on one service costs nothing extra. Where
> sharing changes what you see, it is called out.

## Step 1 — Add the environment

`--like dev` walks every binding dev has and asks one question each.

```bash
rigg env add staging --like dev
```

```text
# output
Discovering Azure services (via Azure CLI credentials)...
> Azure AI Search service: contoso-search
> Microsoft Foundry project: contoso-ai/rag
> docs-storage (storage) in 'staging': same as dev (contosodocs)
  > Protect this environment (require typed confirmation for cloud changes)? No

Environment 'staging' added.
  search:   contoso-search
  foundry:  contoso-ai/rag
  docs-storage:  storage contosodocs
Set as default with: rigg env set-default staging
```

**Why it matters.** Keep the same physical resource (which makes the binding
*shared*), pick a different one from an ARM list, type a value, or skip it —
the questions are the point, because a binding rigg copied silently is exactly
how dev's storage account leaks into staging. Here the answer is deliberately
"same as dev", and rigg records that as sharing rather than pretending the two
are unrelated.

> [!WARNING]
> `--like` does not copy tenant and subscription. A second environment usually
> lives somewhere else, and guessing would send every by-name lookup to the
> wrong place. Pass `--tenant`/`--subscription` when they really are shared.

The same decisions are available on the command line, which is how you script
this — off a terminal nothing is asked at all, so the flags are the whole
input:

```bash
rigg env add staging --like dev \
  --search-service contoso-search --foundry-account contoso-ai --foundry-project rag \
  --same docs-storage
```

`--bind <name>=<type>:<value>` sets a binding outright, `--skip <name>` drops
one, and `--same <name>` keeps the model environment's value — the default for
every binding neither `--bind` nor `--skip` names. To give staging its own
account instead, `--bind docs-storage=storage:contosodocsstg`. A
`--same`/`--skip` naming a binding the model environment does not have is a
usage error (exit 2), not a silent no-op.

> [!TIP]
> On a terminal you can also pre-answer a question by its id, which is useful
> once you know the ids from a previous run:
>
> ```bash
> rigg env add staging --like dev --answer binding.staging.docs-storage=contosodocs
> ```

## Step 2 — Look at what you created

`env show` prints the environment's targets, dependencies and policy.

```bash
rigg env show staging
```

```text
# output
staging
  protected: false
  search: contoso-search → https://contoso-search.search.windows.net (Azure AI Search)
  foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com (Microsoft Foundry)
  dependencies:
    docs-storage  storage  contosodocs (shared with: dev)
```

**Why it matters.** An environment is exactly three things: **targets** (one
Search service, one Foundry account/project), **dependencies** (the named
infrastructure bindings) and **policy** — every key is in
[the environments reference](../reference/rigg-yaml.md#environments). The
`(shared with: dev)` is rigg reporting the consequence of your answer, not a
warning: sharing is legitimate, and it has to be visible.

Resolve the bindings against ARM before promoting:

```bash
rigg env show staging --refresh
```

```text
# output
staging
  protected: false
  search: contoso-search → https://contoso-search.search.windows.net (Azure AI Search) → /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search
  foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com (Microsoft Foundry) → /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.CognitiveServices/accounts/contoso-ai
  dependencies:
    docs-storage  storage  contosodocs → /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs (shared with: dev)
```

> [!NOTE]
> `--refresh` re-resolves every binding against ARM and caches the ids; a
> binding that names a resource you cannot see is reported here rather than at
> push time. Skip it and promote still works, but it reports what it could not
> resolve:
>
> ```text
> Checks
>   ! binding 'docs-storage' in 'staging' is declared by name only — run `rigg env show staging --refresh` (or declare the full ARM id)
>   ! data-sources/docs-ds credentials.connectionString: kept from 'dev' (binding 'docs-storage' unresolved in 'staging')
> ```

"Kept from dev" is the honest failure mode: rather than guess a staging
resource id, promote leaves dev's and tells you it did. The staging tree is
still empty — `projects/docs-rag/envs/staging/` does not exist yet, and that is
what promote is for.

## Step 3 — Preview the promotion

A dry run shows the translation without writing a file.

```bash
rigg promote docs-rag --from dev --to staging --dry-run
```

```text
# output
Promote project 'docs-rag': dev → staging
  Targets: Search contoso-search → contoso-search, Foundry contoso-ai/rag → contoso-ai/rag

Rewiring (bindings)
  docs-storage   storage        contosodocs                = contosodocs                shared
  foundry        model host     contoso-ai                 = contoso-ai                 shared
  search         search         contoso-search             = contoso-search             shared (2 references)

Resources
  0 changed, 8 new, 0 unchanged, 0 kept (only in 'staging')

new (will be created in 'staging'):
  data-sources/docs-ds
  indexes/docs-index
…
  connections/docs-kb-conn

(dry run — nothing written)
```

**Why it matters.** Read the **Rewiring** table first: every infrastructure
reference was parsed to its physical resource, matched to the binding name it
has in dev, and re-rendered from staging's binding of the same name. Here every
line says `=` and `shared`, because this walkthrough pointed staging at dev's
resources; with a separate staging account the same line reads `docs-storage
storage contosodocs → contosodocsstg`. Either way the table is a statement
about your infrastructure that you get to check before anything is written.

> [!TIP]
> `--dry-run` still performs the online checks (deployment availability and
> quota in the target region, Web API auth re-derivation), so it can still ask
> questions. Add `--offline` for a completely network-free preview.

## Step 4 — Answer whatever promote cannot decide

Anything ambiguous stops the run and becomes a question rather than a guess.

| Question id | When |
|---|---|
| `binding.staging.<name>` | dev has a binding staging lacks — use dev's value (shared), pick another, or skip |
| `promote.bind.dev.<physical>` | a dev file references infrastructure that is not bound in dev at all |
| `promote.external.<host>` | an external API URL bound in neither environment — keep it verbatim? |

On a terminal they are prompted inline. Non-interactively the run writes
nothing, exits 6, and prints a `needs-input` document listing every outstanding
question with its candidates. Answer and re-run:

```bash
rigg promote docs-rag --from dev --to staging --answer binding.staging.docs-storage=contosodocs --yes
```

**Why it matters.** Answers that create a binding are written into `rigg.yaml`
once the run gets past the confirmation, so the same question is never asked
twice.

## Step 5 — Promote for real

Drop `--dry-run` and confirm the same plan.

```bash
rigg promote docs-rag --from dev --to staging
```

```text
# output
Promote project 'docs-rag': dev → staging
  Targets: Search contoso-search → contoso-search, Foundry contoso-ai/rag → contoso-ai/rag

Rewiring (bindings)
…
Resources
  0 changed, 8 new, 0 unchanged, 0 kept (only in 'staging')
…
? Proceed? (Y/n) y

Promoted 8 resource(s) into 'staging'.
hint: rigg validate docs-rag
      rigg auth doctor -e staging
      rigg push docs-rag -e staging --dry-run
      rigg push docs-rag -e staging
```

**Why it matters.** Nothing has touched Azure yet: promote is a **local**
operation that writes `projects/docs-rag/envs/staging/`, so you can diff it,
review it and commit it. Resources that exist only in staging are never touched
and nothing is deleted, so promote is safe to re-run. And `--from`/`--to` is a
direction you choose, not a fixed deploy pipeline: a hotfix promoted back from
staging to dev is the same command with the arguments swapped.

> [!TIP]
> Why translation beats copying is argued in full in
> [How rigg works → Promote is translation](../how-rigg-works.md#promote-is-translation-not-copy)
> and
> [CONCEPTS → Promoting between environments](../../CONCEPTS.md#promoting-between-environments).

## Step 6 — Give staging its own names

Promote copied dev's physical names, so rename them before pushing anything:
edit the `"name"` field in each file under
`projects/docs-rag/envs/staging/` — leave the *file* names alone — and re-run
the promotion.

```bash
# in projects/docs-rag/envs/staging/: "name": "docs-ds" → "staging-docs-ds", …
rigg promote docs-rag --from dev --to staging
```

```text
# output
Promote project 'docs-rag': dev → staging
  Targets: Search contoso-search → contoso-search, Foundry contoso-ai/rag → contoso-ai/rag
…
Renamed siblings
  data-sources/docs-ds docs-ds → staging-docs-ds  (1 reference(s) rewritten)
  indexes/docs-index   docs-index → staging-docs-index  (2 reference(s) rewritten)
  skillsets/docs-skills docs-skills → staging-docs-skills  (1 reference(s) rewritten)
  knowledge-sources/docs-ks docs-ks → staging-docs-ks  (1 reference(s) rewritten)
  knowledge-bases/docs-kb docs-kb → staging-docs-kb  (3 reference(s) rewritten)
  connections/docs-kb-conn docs-kb-conn → staging-docs-kb-conn  (1 reference(s) rewritten)

Resources
  0 changed, 0 new, 8 unchanged, 0 kept (only in 'staging')
```

**Why it matters.** You edited eight `"name"` fields; promote rewrote every
*reference* to them — the indexer's `dataSourceName`, `targetIndexName` and
`skillsetName`, the knowledge source's `searchIndexName`, the knowledge base's
`knowledgeSources`, and on the Foundry side the agent's `x-rigg-ref`, its
`project_connection_id` and both MCP URLs (the tool's `server_url` and the
connection's `properties.target`). That is the **Renamed siblings**
table, and it is why resources correlate by file path rather than by name.

> [!WARNING]
> **Two environments on one Search service cannot share physical names**:
> pushing staging would land on dev's resources and quietly take them over. On
> staging's own service, keeping dev's names is normal and this step is
> unnecessary. Run promote once more and it reports `0 changed, 0 new, 8
> unchanged`: it is idempotent, so re-running it after every dev change is a
> diff, not a merge.

## Step 7 — Wire up staging's identities

Derive staging's own requirement graph and satisfy it.

```bash
rigg auth doctor -e staging --fix
```

```text
# output
auth doctor env: staging
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  ✓ search-system → Storage Blob Data Reader @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs
      search service 'contoso-search' (system-assigned) holds it
      files:  data-sources/docs-ds.json:credentials.connectionString
…
  operator: you@contoso.com
  ✓ operator → Search Service Contributor @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search
  ✓ operator → Foundry User @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.CognitiveServices/accounts/contoso-ai/projects/rag
…
summary: 12 ok, 0 missing, 0 unresolved
✓ identity wiring is complete
```

**Why it matters.** Everything is already satisfied here, because staging
points at the same services as dev and dev's grants cover them. On a *separate*
staging service none of them would be: a new Search service has a different
managed identity, and no role assignment carries over — nor should one. The
graph is derived per environment from that environment's own files and
bindings, so `--fix` would create staging's own assignments, tagged
`rigg:<workspace>:staging:<reason>`, which `rigg auth roles remove -e staging`
would undo exactly.

## Step 8 — Push and verify

Preview staging's plan, then apply it.

```bash
rigg push docs-rag -e staging --dry-run
```

```text
# output
Push project 'docs-rag' (env: staging)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  create data-sources/staging-docs-ds
  create indexes/staging-docs-index
  create skillsets/staging-docs-skills
  create connections/staging-docs-kb-conn
  create indexers/staging-docs-indexer
  create knowledge-sources/staging-docs-ks
  create knowledge-bases/staging-docs-kb
  create agents/staging-docs-agent
  (dry run — nothing pushed)
```

**Why it matters.** Read the names in that plan before you answer the real one
— they are the proof that step 6 landed, and the difference between creating
staging and overwriting dev.

```bash
rigg push docs-rag -e staging
```

```text
# output
Push project 'docs-rag' (env: staging)
…
? Apply 8 change(s)? (y/N) y
  ✓ data-sources/staging-docs-ds
…
  ✓ agents/staging-docs-agent
```

Then prove staging actually runs — this triggers a real indexer run and one
agent turn, so it costs ingestion and tokens:

```bash
rigg verify docs-rag -e staging
```

```text
# output
Verify project 'docs-rag' (env: staging)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  ✓ triggered a run of 'staging-docs-indexer'
  … success
  ✓ indexer 'staging-docs-indexer' — 0 processed, 0 failed
  ✓ knowledge base 'staging-docs-kb' retrieved
  ✓ agent 'staging-docs-agent' replied
✓ 3 check(s) passed
```

`0 processed` again: Azure ran the new indexer the moment push created it, and
it read the corpus then. What verify proves is that the run reached storage and
finished, that the knowledge base answers, and that the agent can reach it
through staging's own connection.

> [!TIP]
> `-e staging` selects the environment for one command. `RIGG_ENV=staging` sets
> it for a shell, and `rigg env set-default staging` changes the workspace
> default; the precedence is flag > `RIGG_ENV` > `default: true`.

## Step 9 — Keep the two honest

From now on the useful question is not "what is in staging?" but "how do the
two differ?".

```bash
rigg status
```

```text
# output
env: dev (default)
  docs-rag
    data-sources/docs-ds                               in sync
    indexes/docs-index                                 in sync
…
    connections/docs-kb-conn                           in sync

env: staging
  docs-rag
    data-sources/staging-docs-ds                       in sync
    indexes/staging-docs-index                         in sync
…
    connections/staging-docs-kb-conn                   in sync
```

**Why it matters.** `rigg status` with no argument reports every environment,
so drift in staging shows up while you are working in dev. For "do the two
still mean the same thing?", re-run `rigg promote docs-rag --from dev --to
staging --dry-run`: `0 changed` is the answer, and it is the only comparison
that knows which differences are *supposed* to be there.

> [!NOTE]
> `rigg diff docs-rag --compare-env staging` is the raw alternative — it
> compares the two environments' live services field by field, which is
> informative when they are different services and vacuous when, as here, they
> are the same one.

## What you have now

- [x] Two environments in one `rigg.yaml`, each with its own targets, bindings
      and policy — with sharing recorded rather than hidden.
- [x] A staging tree that was *derived* from dev, not copied, with the
      derivation visible in the rewiring and renaming tables.
- [x] Staging's own resources in Azure, pushed and verified.
- [x] A repeatable promotion: re-running it after every dev change is a diff,
      not a merge.

## Clean up

Delete everything the project owns in *that* environment from Azure — `-e
staging` is what keeps it away from dev, and it leaves the local files alone.
Remove the environment together with the role assignments rigg created for it,
so they do not outlive the environment that explains them. Then delete the
promoted tree, which both commands leave on disk because `rigg env remove` only
edits `rigg.yaml`.

```bash
rigg delete docs-rag --remote -e staging
rigg env remove staging --clean-roles
rm -r projects/docs-rag/envs/staging
```

`--clean-roles` does the same job as `rigg auth roles remove -e staging`, so
run one or the other, not both.

## Next

[Tutorial 4 — Push to protected production](04-push-to-protected-production.md)
