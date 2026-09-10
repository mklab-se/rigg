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
again. Step 10 runs `rigg verify`, which triggers a run of every indexer in
the project and asks every agent one question — real ingestion, embedding and
token spend on the stack you already have. An indexer whose corpus has not
changed re-reads nothing, so the usual bill is a handful of tokens; an indexer
you have just reset is a full re-ingestion. Skip step 10 if you would rather
not pay for it at all; nothing later depends on it.

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

```text
# output
Discovering Azure services (via Azure CLI credentials)...
? Azure AI Search service:
> contoso-search
  (skip — none)
[↑↓ to move, enter to select, type to filter]
> Azure AI Search service: contoso-search

? Microsoft Foundry project:
> contoso-ai/rag
  (skip — none)
[↑↓ to move, enter to select, type to filter]
> Microsoft Foundry project: contoso-ai/rag

Identity guidance
  For stacks spanning services (search + storage + foundry), a USER-ASSIGNED managed
  identity is recommended: one identity for the whole pipeline, role assignments
  survive service re-creation, and it works across environments. Use system-assigned
  for simple single-service setups. `rigg auth doctor` verifies the wiring; `rigg new
  <kind> <name> --identity <binding>` writes the identity into new resources.

✓ rigg workspace initialized
  config:   ./rigg.yaml
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

```text
# output
Would adopt 8 resource(s) into 'docs-rag':
  data-sources/docs-ds
  indexes/docs-index
  skillsets/docs-skills
  indexers/docs-indexer
  knowledge-sources/docs-ks
  knowledge-bases/docs-kb
  agents/docs-agent
  deployments/gpt-4.1-mini
? Adopt these? (Y/n) y
  + adopted data-sources/docs-ds
  + adopted indexes/docs-index
  + adopted skillsets/docs-skills
  + adopted indexers/docs-indexer
  + adopted knowledge-sources/docs-ks
  + adopted knowledge-bases/docs-kb
  + adopted agents/docs-agent
  + adopted deployments/gpt-4.1-mini

Infrastructure references not yet bound in 'dev':
  name                 type         value
  docs-enrich          function-app docs-enrich  (from projects/docs-rag/envs/dev/search/skillsets/docs-skills.json:skills[1].uri)
? Record these bindings in rigg.yaml? (Y/n) y
```

`adopt` claims *unmanaged* remote resources into a project: it downloads each
definition, normalizes it (stripping etags, timestamps and server-set
defaults) and records a sync baseline. `all` takes everything unmanaged; you
can also name a kind (`indexes`) or one resource
(`agents/docs-agent`), and `--with-deps` pulls a resource's upstream
dependencies along with it. Run it without a selector on a terminal for a
pick-list wizard.

> **Note:** `all` really does mean all — on the Foundry side that includes
> every model deployment, connection and guardrail in the project, not just
> the ones your RAG stack uses. On an account you share with other teams,
> name what you want instead (`rigg adopt docs-rag indexes`,
> `rigg adopt docs-rag agents/docs-agent --with-deps`). Adoption itself only
> writes local files, but a project that owns a resource is a project that
> can later delete it.

Adoption is also where rigg tells you it may be behind Azure. A field the
pinned schema does not know prints as a note —
`field 'subtype' is not in rigg's 2026-04-01 schema — Azure may have shipped
a newer API; run 'rigg dev api-check'` — and the field is kept, not dropped.

## 5. Record the infrastructure bindings

Answer `y` to the prompt above, or run the learn step explicitly:

```bash
rigg env bind dev --learn
```

```text
# output
Found 1 unbound infrastructure reference(s) in 'dev':
  docs-enrich  function-app  docs-enrich  (1 reference(s), e.g. projects/docs-rag/envs/dev/search/skillsets/docs-skills.json:skills[1].uri)
? Name for the function-app 'docs-enrich' (or 'skip'): (docs-enrich)
Bound 'docs-enrich' in environment 'dev': function-app docs-enrich
```

Your skillset names a function app; your data source's connection string
names a storage account; a Foundry connection names whatever it targets.
Those are **bindings**: rigg gives each one a name in `rigg.yaml`, so the same
file can later be translated to a staging or prod environment that uses a
different account. `--learn` scans the environment's files, groups every
infrastructure reference by physical resource, and proposes a name per group —
rename any of them before confirming. It proposes one per group, so a real
stack usually yields several lines rather than the single one above.

Note what the skillset file does *not* contain. Azure redacts a Web API
skill's function key on every GET, so what landed on disk is
`...?code=<redacted>` — a URL you can read, without the secret. That is why
`rigg validate` can promise no key material on disk, and why
`rigg push --refresh-credentials` exists for the day the key needs
re-supplying.

Check what it recorded:

```bash
rigg env show dev
```

```text
# output
dev
  protected: false
  tenant: <tenant-id>
  subscription: <subscription-id>
  search: contoso-search → https://contoso-search.search.windows.net (Azure AI Search)
  foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com (Microsoft Foundry)
  dependencies:
    docs-enrich  function-app  docs-enrich
```

## 6. Look at the state, then commit it

```bash
rigg status
```

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
    deployments/gpt-4.1-mini                           in sync
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

```text
# output
docs-rag (env: dev)
  data-sources/docs-ds
  indexes/docs-index
  skillsets/docs-skills -> indexes/docs-index
  indexers/docs-indexer -> data-sources/docs-ds, indexes/docs-index, skillsets/docs-skills
  knowledge-sources/docs-ks -> indexes/docs-index
  knowledge-bases/docs-kb -> knowledge-sources/docs-ks
  agents/docs-agent -> knowledge-bases/docs-kb, deployments/gpt-4.1-mini
  deployments/gpt-4.1-mini -> guardrails/Microsoft.DefaultV2

  Infrastructure:
    docs-enrich  function-app  docs-enrich
```

This is the picture that used to be spread over half a dozen portal blades.
It is also what an AI coding tool gets in a single call through
[the MCP server](../../MCP.md) — the dependency graph and every file path.
A skillset that implements one of the OpenAPI specs in `apis/` adds an
`APIs to implement` section here; an adopted skillset that calls a function
app directly, like this one, shows up under `Infrastructure` instead.

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

```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  create synonym-maps/roundtrip-demo
  note: skillsets/docs-skills has a server-redacted Web API key (normal on Azure GETs, says nothing about the stored key) — if enrichment is failing, run `rigg push --refresh-credentials` to re-authorize
? Apply 1 change(s)? (y/N) y
  ✓ synonym-maps/roundtrip-demo
```

Now delete it from Azure — by deleting the *file*. A resource with a recorded
baseline and no file is an **orphan**, and orphans are removed from Azure only
when you say `--prune`. Preview first; a dry run writes nothing:

```bash
rm projects/docs-rag/envs/dev/search/synonym-maps/roundtrip-demo.json
rigg push docs-rag --prune --dry-run
```

```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  delete synonym-maps/roundtrip-demo
  (dry run — nothing pushed)
```

Everything else is in sync, so the plan is one line long — that one line is
the whole blast radius. Apply it:

```bash
rigg push docs-rag --prune --yes
```

```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
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

```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  create synonym-maps/roundtrip-demo
? Apply 1 change(s)? (y/N) y
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

```text
# output
Verify project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  ✓ triggered a run of 'docs-indexer'
  … success
  ✓ indexer 'docs-indexer' — 0 processed, 0 failed
  ✓ knowledge base 'docs-kb' retrieved
  ✓ agent 'docs-agent' replied
✓ 3 check(s) passed
```

`push` proves the *definitions* landed; `verify` proves the *stack runs*. It
runs every indexer to completion, retrieves from every knowledge base and asks
every agent one question. A failure that smells like authorization is
attributed to the identity edge that would explain it, so you get "the search
identity cannot read `contosodocs`" instead of a raw 403.

`0 processed` above is the expected answer for a corpus that has not changed:
a blob indexer tracks a high-water mark, so a re-run over the same documents
re-reads nothing and costs nothing. What it proves is that the run *reached*
the storage account and finished — which is the thing an expired role
assignment breaks. Ask for the whole corpus back with
`rigg az indexer reset <name>`, which makes the next run reprocess everything.

Verify is also honest about a stack that is only mostly working: an agent
with no model deployment fails here with `API error (400)` while everything
around it passes, and the command exits 1 naming exactly which check failed.

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
