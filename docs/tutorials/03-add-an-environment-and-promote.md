# Tutorial 3 — Add an environment and promote to it

You have a working `dev` stack. Now you need a `staging` one: the same shape,
pointed at a different Search service and a different storage account, with
the agent instructions you just reviewed.

Copying files would be wrong — every file is full of dev's infrastructure. This
tutorial shows the operation that replaces copying: `rigg promote`, which
*translates* one environment's tree into another.

**Time:** about 25 minutes. **Azure cost:** a second Search service (if you do
not already have one) and a second model deployment. Everything else is
configuration.

## Prerequisites

- A workspace with a working `dev` environment and at least one project — the
  output of [tutorial 1](01-pull-an-existing-solution.md) or
  [tutorial 2](02-build-from-scratch.md). Examples below use the project
  `docs-rag`.
- A second set of targets for staging: a Search service (`contoso-search-stg`)
  and a Foundry project (`contoso-ai`/`rag-stg` — the same account is fine).
- A second storage account (`contosodocsstg`) with the staging corpus.
- **Roles you need**: the same as dev, on the staging resources —
  `Search Service Contributor` on the staging Search service, `Azure AI User`
  on the staging Foundry project, and the ability to grant `Storage Blob Data
  Reader` on the staging storage account (or someone who can).
- `az login` against a tenant that can see both. Staging may live in a
  different subscription — or a different tenant — and rigg supports that;
  pass `--tenant`/`--subscription` in step 1.

## 1. Add the environment

```bash
rigg env add staging --like dev
```

<!-- verify-live -->
```text
# output
Discovering Azure services (via Azure CLI credentials)...
Azure AI Search service:
  contoso-search
> contoso-search-stg
  (skip — none)
Microsoft Foundry project:
> contoso-ai/rag-stg
  (skip — none)
? docs-storage (storage) in 'staging':
  same as dev (contosodocs)
> contosodocsstg
  skip (leave unbound)
? Protect this environment (require typed confirmation for cloud changes)? (y/N) n
Environment 'staging' added.
  search:   contoso-search-stg
  foundry:  contoso-ai/rag-stg
  docs-storage:  storage contosodocsstg
Set as default with: rigg env set-default staging
```

`--like dev` is what makes this short: rigg walks **every binding dev has** and
asks one question each — keep the same physical resource (which makes the
binding *shared*), pick a different one from an ARM list, or skip it. The
questions are the point: a binding rigg copied silently would be exactly the
kind of thing that leaks dev's storage account into staging.

Note what `--like` does *not* copy: tenant and subscription. A second
environment usually lives somewhere else, and guessing would send every
by-name lookup to the wrong place. Pass `--tenant`/`--subscription` when they
really are shared.

The same decisions are available on the command line, which is how you script
this: `--bind <name>=<type>:<value>` sets a binding outright, `--skip <name>`
drops one, and `--same <name>` keeps the model environment's value — the
default for every binding neither `--bind` nor `--skip` names. Off a terminal
nothing is asked at all, so the flags are the whole input:

```bash
rigg env add staging --like dev \
  --search-service contoso-search-stg --foundry-account contoso-ai --foundry-project rag-stg \
  --bind docs-storage=storage:contosodocsstg
```

`docs-storage` is dev's only binding here, so that one `--bind` is the whole
input. To keep staging on dev's storage account instead — a *shared* binding —
name it with `--same docs-storage`; to leave it unbound, `--skip docs-storage`.
A `--same`/`--skip` naming a binding the model environment does not have is a
usage error (exit 2), not a silent no-op.

On a terminal you can also pre-answer a question by its id, which is useful
once you know the ids from a previous run:

```bash
rigg env add staging --like dev --answer binding.staging.docs-storage=contosodocsstg
```

## 2. Look at what you created

```bash
rigg env show staging
```

<!-- verify-live -->
```text
# output
staging
  protected: false
  search: contoso-search-stg → https://contoso-search-stg.search.windows.net (Azure AI Search)
  foundry: contoso-ai/rag-stg → https://contoso-ai.services.ai.azure.com/api/projects/rag-stg (Microsoft Foundry)
  dependencies:
    docs-storage  storage  contosodocsstg
```

An environment is exactly three things: **targets** (one Search service, one
Foundry account/project), **dependencies** (the named infrastructure bindings),
and **policy** — every key is in
[the environments reference](../reference/rigg-yaml.md#environments). `rigg env show staging --refresh` re-resolves every binding
against ARM and caches the ids; a binding that names a resource you cannot see
is reported here rather than at push time.

The staging tree is still empty — `projects/docs-rag/envs/staging/` does not
exist yet. That is what promote is for.

## 3. Preview the promotion

```bash
rigg promote docs-rag --from dev --to staging --dry-run
```

<!-- verify-live -->
```text
# output
Promote project 'docs-rag': dev → staging
  Targets: Search contoso-search → contoso-search-stg, Foundry contoso-ai/rag → contoso-ai/rag-stg

Rewiring (bindings)
  docs-storage   storage        contosodocs                → contosodocsstg
  search         search         contoso-search             → contoso-search-stg               (2 references)

Resources
  0 changed, 8 new, 0 unchanged, 0 kept (only in 'staging')

new (will be created in 'staging'):
  data-sources/docs-ds
  indexes/docs-index
  skillsets/docs-skills
  indexers/docs-indexer
  knowledge-sources/docs-ks
  knowledge-bases/docs-kb
  deployments/gpt-4.1-mini
  agents/docs-agent

Checks
  ✓ deployment 'gpt-4.1-mini' is available in the target region

(dry run — nothing written)
```

Read the **Rewiring** table first: it is the whole argument for promote
existing. Every infrastructure reference was parsed to its physical resource,
matched to the binding name it has in dev, and re-rendered from staging's
binding of the same name. An `=` instead of `→` in that table would mean the
two environments *share* that resource — reported, never silently assumed.

A **Renamed siblings** section appears when a resource has a different
physical `name` in the target; every reference to it is rewritten to follow.
Resources correlate by file path, not by name, precisely so that renaming is
possible.

`--dry-run` still performs the online checks (deployment availability and
quota in the target region, Web API auth re-derivation), so it can still ask
questions. Add `--offline` for a completely network-free preview.

## 4. Answer whatever promote cannot decide

Anything ambiguous stops the run and becomes a question rather than a guess.
The three you are most likely to see:

| Question id | When |
|---|---|
| `binding.staging.<name>` | dev has a binding staging lacks — use dev's value (shared), pick another, or skip |
| `promote.bind.dev.<physical>` | a dev file references infrastructure that is not bound in dev at all |
| `promote.external.<host>` | an external API URL bound in neither environment — keep it verbatim? |

On a terminal they are prompted inline. Non-interactively the run writes
nothing, exits 6, and prints a `needs-input` document listing every
outstanding question with its candidates. Answer and re-run:

```bash
rigg promote docs-rag --from dev --to staging --answer binding.staging.docs-storage=contosodocsstg --yes
```

Answers that create a binding are written into `rigg.yaml` once the run gets
past the confirmation, so the same question is never asked twice.

## 5. Promote for real

```bash
rigg promote docs-rag --from dev --to staging
```

<!-- verify-live -->
```text
# output
Promote project 'docs-rag': dev → staging
  Targets: Search contoso-search → contoso-search-stg, Foundry contoso-ai/rag → contoso-ai/rag-stg

Rewiring (bindings)
  docs-storage   storage        contosodocs                → contosodocsstg
  search         search         contoso-search             → contoso-search-stg               (2 references)

Resources
  0 changed, 8 new, 0 unchanged, 0 kept (only in 'staging')

Proceed? (Y/n) y

Promoted 8 resource(s) into 'staging'.
hint: rigg validate docs-rag
      rigg auth doctor -e staging
      rigg push docs-rag -e staging --dry-run
      rigg push docs-rag -e staging
```

Nothing has touched Azure yet: promote is a **local** operation that writes
`projects/docs-rag/envs/staging/`. Diff it, review it, commit it — the
translation is now something a colleague can read in a pull request.

Two properties worth internalising. Resources that exist only in staging are
never touched and nothing is deleted, so promote is safe to re-run. And
`--from`/`--to` is a direction you choose, not a fixed deploy pipeline: a
hotfix promoted back from staging to dev is the same command with the
arguments swapped.

## 6. Wire up staging's identities

```bash
rigg auth doctor -e staging --fix
```

<!-- verify-live -->
```text
# output
auth doctor env: staging
  Search:  contoso-search-stg → https://contoso-search-stg.search.windows.net
  Foundry: contoso-ai/rag-stg → https://contoso-ai.services.ai.azure.com/api/projects/rag-stg
  ✗ search-system → Storage Blob Data Reader @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocsstg
      the indexer reads blobs with the search service's system-assigned identity
      files:  data-sources/docs-ds.json:credentials.connectionString

1 rigg can fix:
  - assign 'Storage Blob Data Reader' to <search-identity-object-id> at /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocsstg
Apply 1 fix(es)? (Y/n) y

1 fix(es) applied, 0 failed
✓ re-run `rigg auth doctor` to confirm (role assignments take a moment to propagate)
```

A new environment means a new Search service with a *different* managed
identity, so none of dev's role assignments carry over — and none should. The
requirement graph is derived per environment from that environment's own
files and bindings.

## 7. Push and verify

```bash
rigg push docs-rag -e staging --dry-run
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: staging)
  Search:  contoso-search-stg → https://contoso-search-stg.search.windows.net
  Foundry: contoso-ai/rag-stg → https://contoso-ai.services.ai.azure.com/api/projects/rag-stg
  create data-sources/docs-ds
  create indexes/docs-index
  create skillsets/docs-skills
  create indexers/docs-indexer
  create knowledge-sources/docs-ks
  create knowledge-bases/docs-kb
  create deployments/gpt-4.1-mini
  create agents/docs-agent
  (dry run — nothing pushed)
```

```bash
rigg push docs-rag -e staging
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: staging)
  Search:  contoso-search-stg → https://contoso-search-stg.search.windows.net
  Foundry: contoso-ai/rag-stg → https://contoso-ai.services.ai.azure.com/api/projects/rag-stg
  create data-sources/docs-ds
  create indexes/docs-index
  create skillsets/docs-skills
  create indexers/docs-indexer
  create knowledge-sources/docs-ks
  create knowledge-bases/docs-kb
  create deployments/gpt-4.1-mini
  create agents/docs-agent

Apply 8 change(s)? (y/N) y
  ✓ data-sources/docs-ds
  ✓ indexes/docs-index
  ✓ skillsets/docs-skills
  ✓ indexers/docs-indexer
  ✓ knowledge-sources/docs-ks
  ✓ knowledge-bases/docs-kb
  ✓ deployments/gpt-4.1-mini
  ✓ agents/docs-agent
```

Then prove staging actually runs — this triggers a real indexer run and one
agent turn, so it costs ingestion and tokens:

```bash
rigg verify docs-rag -e staging
```

<!-- verify-live -->
```text
# output
Verify project 'docs-rag' (env: staging)
  Search:  contoso-search-stg → https://contoso-search-stg.search.windows.net
  Foundry: contoso-ai/rag-stg → https://contoso-ai.services.ai.azure.com/api/projects/rag-stg
  ✓ triggered a run of 'docs-indexer'
  … success
  ✓ indexer 'docs-indexer' — 64 processed, 0 failed
  ✓ knowledge base 'docs-kb' retrieved
  ✓ agent 'docs-agent' replied
✓ 3 check(s) passed
```

`-e staging` selects the environment for one command. `RIGG_ENV=staging` sets
it for a shell, and `rigg env set-default staging` changes the workspace
default; the precedence is flag > `RIGG_ENV` > `default: true`.

## 8. Keep the two honest

From now on, the useful question is not "what is in staging?" but "how do the
two differ?":

```bash
rigg status
rigg diff docs-rag --compare-env staging
```

`rigg status` with no argument reports every environment, so drift in staging
shows up while you are working in dev. `diff --compare-env` is a **raw**
comparison — it will show every infrastructure difference, because those are
real. "Equal modulo infrastructure" is what `promote --dry-run` tells you: if
it reports `0 changed`, the two trees mean the same thing.

## What you have now

- Two environments in one `rigg.yaml`, each with its own targets, bindings and
  policy.
- A staging tree that was *derived* from dev, not copied — with the derivation
  visible in the rewiring table and reviewable in Git.
- A repeatable promotion: re-running it after every dev change is a diff, not
  a merge.

## Clean up

```bash
rigg delete docs-rag --remote -e staging
rigg env remove staging --clean-roles
rm -r projects/docs-rag/envs/staging
```

`rigg delete --remote` removes everything the project owns in *that*
environment from Azure; `-e staging` is what keeps it away from dev. It leaves
the local files alone, which is why the `rm -r` is a separate line:
`rigg env remove` only edits `rigg.yaml`, so the promoted tree under
`projects/<project>/envs/staging/` stays on disk until you delete it.

`--clean-roles` deletes the role assignments rigg created for the environment
as it removes it — otherwise they outlive the environment that explains them,
on infrastructure other environments may share. It does the same job as
`rigg auth roles remove -e staging`, so run one or the other, not both.

## Next

- [Tutorial 4 — Push to protected production](04-push-to-protected-production.md)
- [How rigg works → Promote is translation](../how-rigg-works.md#promote-is-translation-not-copy)
- [CONCEPTS → Promoting between environments](../../CONCEPTS.md#promoting-between-environments)
