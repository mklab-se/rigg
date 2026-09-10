# Tutorial 1 — Put an existing Azure solution under version control

You already have an Agentic RAG stack in Azure: an index, an indexer, maybe a
knowledge base and a Foundry agent. It was built in the portal, nobody
remembers who changed the agent instructions last week, and your AI coding
tools cannot see any of it.

By the end of this tutorial that whole stack is a Git repository, and you have
proved the round trip works: you will delete a resource from Azure and put it
back with one command.

**Time:** about 20 minutes.

**New Azure cost:** small, but not zero. Adopting, committing and describing
create nothing. Step 8 creates one throwaway synonym map (free) and deletes it
again. Step 10 runs `rigg verify`, which triggers a full run of every indexer
in the project and asks every agent one question — that is real ingestion,
embedding and token spend on the stack you already have. Skip step 10 if you
would rather not pay for it; nothing later depends on it.

## Prerequisites

- `rigg` on your PATH — see [INSTALL.md](../../INSTALL.md).
- The Azure CLI, logged in: `az login`. rigg borrows the CLI's token; see
  [CONCEPTS → Tokens](../../CONCEPTS.md#tokens) for the full chain.
- An Azure AI Search service with at least one index, and optionally a
  Microsoft Foundry project with an agent. Everything below uses the
  placeholder names `contoso-search` (Search service) and `contoso-ai`/`rag`
  (Foundry account/project) — substitute your own.
- **Roles you need**, on your own account:
  - `Search Service Contributor` on the Search service (read and write
    definitions), and `Reader` on the resource group so ARM discovery can see
    it.
  - `Azure AI User` on the Foundry project, if you are managing agents.
  - You do **not** need `Owner` or `User Access Administrator` for this
    tutorial: nothing here creates a role assignment.
- Git, for the commit in step 6.

If a step fails with exit code 4, run `rigg auth doctor` — it names the
missing role, its scope, and the `az` command that grants it.

## 1. Make a workspace directory

```bash
mkdir contoso-rag
cd contoso-rag
```

rigg is happiest as its own repository: one workspace directory, one
`rigg.yaml`, one Git history. Nothing in it is machine-specific, so this
directory is exactly what your teammates will clone.

## 2. Initialize the workspace

```bash
rigg init .
```

<!-- verify-live -->
```text
# output
Discovering Azure services (via Azure CLI credentials)...
Azure AI Search service:
> contoso-search
  contoso-search-test
  (skip — none)
Microsoft Foundry project:
> contoso-ai/rag
  (skip — none)

Identity guidance
  For stacks spanning services (search + storage + foundry), a USER-ASSIGNED managed
  identity is recommended: one identity for the whole pipeline, role assignments
  survive service re-creation, and it works across environments.

✓ rigg workspace initialized
  config:   /Users/you/contoso-rag/rigg.yaml
  search:   contoso-search
  foundry:  contoso-ai/rag
  environment: dev (default) — rigg commands target it unless -e/RIGG_ENV say otherwise; add more with `rigg env add`

Next steps:
  rigg new project <name>           # create your first project
  rigg new pipeline <name> -p <p>   # scaffold an explicit RAG pipeline
  rigg adopt <project> <selector>   # or adopt existing Azure resources
```

`init` asks ARM what you have rather than making you type resource ids: it
lists every Search service and Foundry project visible to your login and lets
you pick. It writes `rigg.yaml` with one environment called `dev`, creates
`projects/` and `apis/`, and adds `.rigg/` to `.gitignore` — that directory is
a cache, never a source of truth.

On a machine with no terminal (CI, a container build) discovery cannot prompt,
so name the services on the command line instead:

```bash
rigg init . --search-service contoso-search --foundry-account contoso-ai --foundry-project rag
```

Have a look at what it wrote — every key is documented in
[the rigg.yaml reference](../reference/rigg-yaml.md):

```bash
cat rigg.yaml
```

<!-- verify-live -->
```text
# output
# Rigg workspace configuration.
# Resource definitions live in projects/<name>/ — see `rigg new project`.
environments:
  dev:
    default: true
    tenant: <tenant-id>
    subscription: <subscription-id>
    search: { service: contoso-search }
    foundry: { account: contoso-ai, project: rag }
```

## 3. Create a project

```bash
rigg new project docs-rag
```

<!-- verify-live -->
```text
# output
Created project 'docs-rag' at /Users/you/contoso-rag/projects/docs-rag
Next steps:
  rigg adopt docs-rag                    # adopt existing Azure resources (interactive)
  rigg new <kind> <name> -p docs-rag     # or scaffold new ones
```

A **project** is the unit rigg syncs: `pull`, `push` and `diff` always operate
on a whole project, never on half of one. Every resource belongs to exactly
one project — that single rule is what makes sync unambiguous. Name a project
after the thing it owns; `rigg concepts` explains when to use several.

## 4. Adopt the existing Azure resources

```bash
rigg adopt docs-rag all
```

<!-- verify-live -->
```text
# output
Would adopt 7 resource(s) into 'docs-rag':
  data-sources/docs-ds
  indexes/docs-index
  skillsets/docs-skills
  indexers/docs-indexer
  knowledge-sources/docs-ks
  knowledge-bases/docs-kb
  agents/docs-agent
Adopt these? (Y/n) y
  + adopted data-sources/docs-ds
  + adopted indexes/docs-index
  + adopted skillsets/docs-skills
  + adopted indexers/docs-indexer
  + adopted knowledge-sources/docs-ks
  + adopted knowledge-bases/docs-kb
  + adopted agents/docs-agent

Infrastructure references not yet bound in 'dev':
  name                 type         value
  contosodocs          storage      /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs  (from data-sources/docs-ds.json:credentials.connectionString)
Record these bindings in rigg.yaml? (Y/n) y
```

`adopt` claims *unmanaged* remote resources into a project: it downloads each
definition, normalizes it (stripping etags, timestamps and server-set
defaults) and records a sync baseline. `all` takes everything unmanaged; you
can also name a kind (`indexes`) or one resource
(`agents/docs-agent`), and `--with-deps` pulls a resource's upstream
dependencies along with it. Run it without a selector on a terminal for a
pick-list wizard.

## 5. Record the infrastructure bindings

Answer `y` to the prompt above, or run the learn step explicitly:

```bash
rigg env bind dev --learn
```

<!-- verify-live -->
```text
# output
Found 1 unbound infrastructure reference(s) in 'dev':
  contosodocs  storage  contosodocs  (1 reference(s), e.g. data-sources/docs-ds.json:credentials.connectionString)
Name for the storage 'contosodocs' (or 'skip'): [contosodocs]
Bound 'contosodocs' in environment 'dev': storage contosodocs
```

Your data source's connection string names a storage account; your skillset
may name a function app. Those are **bindings**: rigg gives each one a name in
`rigg.yaml`, so the same file can later be translated to a staging or prod
environment that uses a different account. `--learn` scans the environment's
files, groups every infrastructure reference by physical resource, and
proposes a name per group — rename any of them before confirming.

Check what it recorded:

```bash
rigg env show dev
```

<!-- verify-live -->
```text
# output
dev
  protected: false
  tenant: <tenant-id>
  subscription: <subscription-id>
  search: contoso-search → https://contoso-search.search.windows.net (Azure AI Search)
  foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag (Microsoft Foundry)
  dependencies:
    contosodocs  storage  contosodocs
```

## 6. Look at the state, then commit it

```bash
rigg status
```

<!-- verify-live -->
```text
# output
env: dev (default)
  docs-rag
    data-sources/docs-ds                               in sync
    indexes/docs-index                                 in sync
    skillsets/docs-skills                              in sync
    indexers/docs-indexer                              in sync
    knowledge-sources/docs-ks                          in sync
    knowledge-bases/docs-kb                            in sync
    agents/docs-agent                                  in sync
```

`in sync` means all three of local file, live Azure document and the recorded
baseline agree. rigg compares three things rather than two, which is what lets
it tell "you edited this" apart from "someone edited it in the portal" — see
[How rigg works](../how-rigg-works.md#three-states-not-two-sync-classes-and-baselines).

Now it is a repository:

```bash
git init
git add .
git commit -m "Adopt the docs-rag stack from Azure"
```

`.rigg/` is already gitignored, and no file rigg wrote contains a secret —
that is enforced, not a convention: `rigg validate` rejects key material.

## 7. See the whole graph

```bash
rigg describe docs-rag
```

<!-- verify-live -->
```text
# output
docs-rag (env: dev)
  data-sources/docs-ds
  indexes/docs-index
  skillsets/docs-skills -> apis/doc-enrichment
  indexers/docs-indexer -> data-sources/docs-ds, indexes/docs-index, skillsets/docs-skills
  knowledge-sources/docs-ks -> indexes/docs-index
  knowledge-bases/docs-kb -> knowledge-sources/docs-ks
  agents/docs-agent -> knowledge-bases/docs-kb

  APIs to implement (specs in apis/):
    doc-enrichment (used by skillsets/docs-skills)

  Infrastructure:
    contosodocs  storage  contosodocs
```

This is the picture that used to be spread over half a dozen portal blades.
It is also what an AI coding tool gets in a single call through
[the MCP server](../../MCP.md) — the dependency graph, every file path, and
the OpenAPI specs a custom skill expects you to implement.

## 8. Prove the round trip

Version control is only worth something if you can restore from it. Do the
scariest possible test on the least scary possible resource: scaffold a
throwaway synonym map, push it, delete it from Azure, and push it back.
Nothing in your stack references it, so every command below is scoped to that
one file.

> **Warning:** `rigg delete <project> --remote` is a *different* command, and
> it is deliberately not part of this tutorial. It deletes **every** resource
> the project owns from Azure — after step 4 that is your entire real stack —
> and deleting a Search index destroys the documents in it, which come back
> only once an indexer has re-ingested the whole corpus. The round trip below
> uses `push --prune`, which touches only the resources whose local file you
> removed.

Scaffold the throwaway resource:

```bash
rigg new synonym-map roundtrip-demo -p docs-rag
```

<!-- verify-live -->
```text
# output
Created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/synonym-maps/roundtrip-demo.json
```

Push it, and commit it — you can only restore from a repository that holds the
version you mean to restore:

```bash
rigg push docs-rag
git add projects/docs-rag/envs/dev/search/synonym-maps/roundtrip-demo.json
git commit -m "Add a throwaway synonym map"
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  create synonym-maps/roundtrip-demo

Apply 1 change(s)? (y/N) y
  ✓ synonym-maps/roundtrip-demo
```

Now delete it from Azure — by deleting the *file*. A resource with a recorded
baseline and no file is an **orphan**, and orphans are removed from Azure only
when you say `--prune`. Preview first; a dry run writes nothing:

```bash
rm projects/docs-rag/envs/dev/search/synonym-maps/roundtrip-demo.json
rigg push docs-rag --prune --dry-run
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  delete synonym-maps/roundtrip-demo
  (dry run — nothing pushed)
```

Everything else is in sync, so the plan is one line long — that one line is
the whole blast radius. Apply it:

```bash
rigg push docs-rag --prune --yes
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  delete synonym-maps/roundtrip-demo
  ✓ deleted synonym-maps/roundtrip-demo
```

`roundtrip-demo` is gone from Azure. Check the portal if you want to see it
for yourself. Without `--prune` the same plan would have printed
`orphan synonym-maps/roundtrip-demo (file deleted locally; pass --prune to
delete remotely)` and changed nothing: deletes are always explicit.

## 9. Push it back

Restore the file from Git and push:

```bash
git checkout -- projects/docs-rag/envs/dev/search/synonym-maps/roundtrip-demo.json
rigg push docs-rag
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  create synonym-maps/roundtrip-demo

Apply 1 change(s)? (y/N) y
  ✓ synonym-maps/roundtrip-demo
```

That is the round trip: Azure lost a resource, Git had it, one command put it
back. The same command restores eight resources, or eighty, and at that scale
two things it does on every push become visible. Push orders the writes from the
reference graph, so nothing is created before what it points at (the data
source before the indexer, the knowledge base before the agent). And before
the first write it runs the **auth preflight**: it derives, from these very
documents, which managed identity needs which role on which resource, and
checks each one against Azure. Were the search service's identity to lack
`Storage Blob Data Reader` on `contosodocs`, the push would refuse with
exit 4 and print the `az` line — before writing anything.

After each PUT, rigg reads the server's copy back, normalizes it, and rewrites
the local file and the baseline. That is why `rigg status` says `in sync`
immediately afterwards instead of inventing a diff out of a default Azure
filled in for you.

## 10. Prove it actually works

This is the step that costs money: `verify` runs every indexer in the project
to completion and asks every agent one question, so it bills ingestion,
embedding and tokens on your existing stack. Skip it if that is not what you
want today.

```bash
rigg verify docs-rag
```

<!-- verify-live -->
```text
# output
Verify project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  ✓ triggered a run of 'docs-indexer'
  … inProgress
  … success
  ✓ indexer 'docs-indexer' — 128 processed, 0 failed
  ✓ knowledge base 'docs-kb' retrieved
  ✓ agent 'docs-agent' replied
✓ 3 check(s) passed
```

`push` proves the *definitions* landed; `verify` proves the *stack runs*. It
runs every indexer to completion, retrieves from every knowledge base and asks
every agent one question. A failure that smells like authorization is
attributed to the identity edge that would explain it, so you get "the search
identity cannot read `contosodocs`" instead of a raw 403.

## What you have now

- A Git repository whose history is the history of your RAG configuration.
- A `rigg.yaml` naming your services and the infrastructure your files depend
  on, and a `projects/docs-rag/envs/dev/` tree of reviewable JSON.
- A verified round trip: you can restore this stack from the repository.
- The whole graph available to Claude Code, Copilot or any MCP client.

## Clean up

Step 9 put `roundtrip-demo` back. Remove it for good — the same two commands
as step 8, plus a commit so the repository agrees:

```bash
rm projects/docs-rag/envs/dev/search/synonym-maps/roundtrip-demo.json
rigg push docs-rag --prune
git commit -am "Remove the throwaway synonym map"
```

Your adopted stack is untouched: `--prune` deletes only the resources whose
local file you removed.

## Next

- [Tutorial 2 — Build a RAG stack from scratch](02-build-from-scratch.md)
- [How rigg works](../how-rigg-works.md) — what `status`, `push` and the auth
  preflight actually compute.
- [The CLI reference](../reference/cli.md) — every flag on every command.
