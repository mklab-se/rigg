# Tutorial 2 — Build an Agentic RAG stack from scratch

Nothing exists yet except a storage account with some documents in it. By the
end you will have the whole chain — blob container → data source → index →
skillset → indexer → knowledge source → knowledge base → Foundry agent — as
files in Git, running in Azure, answering questions.

**Time:** about 45 minutes, most of it waiting for the first indexer run.
**Azure cost:** this one *does* create resources. An index and its indexer
runs cost storage and ingestion on your Search service; a model deployment
bills per token. Keep the deployment capacity at 1 and delete everything in
[Clean up](#clean-up) when you are done.

## Prerequisites

- Tutorial 1's setup: `rigg` installed, `az login` done. You do not need to
  have completed tutorial 1, but it explains `init`, projects and bindings in
  more detail than this one repeats.
- An Azure AI Search service (`contoso-search` below) and a Microsoft Foundry
  account/project (`contoso-ai`/`rag`).
- An Azure Storage account (`contosodocs`) with a blob container
  (`handbook`) holding a few PDFs, Word files or Markdown documents.
- **Roles you need**, on your own account:
  - `Search Service Contributor` on the Search service.
  - `Azure AI User` on the Foundry project, and `Contributor` on the Foundry
    account if you want rigg to create the model deployment.
  - `User Access Administrator` or `Owner` on the storage account **or** the
    resource group — step 6 grants the Search service's managed identity
    `Storage Blob Data Reader` there. If you cannot grant roles, `rigg auth
    doctor` prints the exact `az` line to hand to someone who can.
- A workspace, from tutorial 1 or fresh:

```bash
mkdir contoso-rag && cd contoso-rag
rigg init .
rigg new project docs-rag
```

## 1. Scaffold the whole retrieval pipeline

```bash
rigg new pipeline docs -p docs-rag --type azureblob
```

<!-- verify-live -->
```text
# output
  created projects/docs-rag/envs/dev/search/data-sources/docs-ds.json
  created projects/docs-rag/envs/dev/search/indexes/docs-index.json
  created projects/docs-rag/envs/dev/search/skillsets/docs-skills.json
  created projects/docs-rag/envs/dev/search/indexers/docs-indexer.json
  created projects/docs-rag/envs/dev/search/knowledge-sources/docs-ks.json
  created projects/docs-rag/envs/dev/search/knowledge-bases/docs-kb.json

Pipeline 'docs' scaffolded in project 'docs-rag':
  1. Edit the data source (connection ResourceId, container)
  2. Shape the index fields for your data
  3. Adjust or remove the skillset, wire the indexer
  4. Push step by step: rigg push docs-rag
```

One command writes six files, already wired to each other: the indexer names
the data source, index and skillset; the knowledge source points at the index;
the knowledge base routes to the knowledge source. Everything is **explicit** —
there is no hidden Azure-generated pipeline behind a knowledge source, so the
whole retrieval chain is reviewable files. `--type` picks the data-source type
(`azureblob` or `adlsgen2`; blob storage is the only source rigg 2.0
supports).

## 2. Point the data source at your container

Open `projects/docs-rag/envs/dev/search/data-sources/docs-ds.json`. The
scaffold left placeholders:

```json
{
  "name": "docs-ds",
  "type": "azureblob",
  "credentials": {
    "connectionString": "ResourceId=/subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs;"
  },
  "container": { "name": "handbook" },
  "dataDeletionDetectionPolicy": {
    "@odata.type": "#Microsoft.Azure.Search.NativeBlobSoftDeleteDeletionDetectionPolicy"
  }
}
```

Fill in your subscription id, resource group, storage account and container.

Note what is *not* there: an account key. `ResourceId=` is the keyless form —
the Search service authenticates to storage with its managed identity, and
rigg's job in step 6 is to make sure that identity actually has the role. A
file that did contain a key would be rejected by `rigg validate`; see
[CONCEPTS → How rigg handles authentication](../../CONCEPTS.md#how-rigg-handles-authentication).

By default the connection uses the search service's **system-assigned**
identity, which is the only one Azure Storage's trusted-services firewall
exception accepts. If your stack spans several services and you would rather
use one user-assigned identity everywhere, bind it and scaffold the data
source against it — delete the one the pipeline wrote first, or use a
different name:

```bash
rigg env bind dev docs-identity identity:contoso-rag-identity
rigg new data-source docs-uami -p docs-rag --type azureblob --identity docs-identity
```

`--identity <binding>` applies to the kinds that carry one (`data-source` and
`skillset`); it writes the bound identity's ARM id into the scaffold instead
of leaving the service's own.

## 3. Shape the index

`docs-index.json` ships with a minimal schema — `id`, `content`, `title`,
`url` and a semantic configuration. Edit the fields to match your documents;
this is the one file worth spending real time on, because **index fields
cannot be removed in Azure** once created. Adding is easy, removing means
recreating the index.

If you do not need the enrichment skillset, delete
`skillsets/docs-skills.json` and remove `"skillsetName"` from the indexer.

## 4. Record the storage binding

```bash
rigg env bind dev docs-storage storage:contosodocs
```

The storage account is now a named dependency of the `dev` environment rather
than a string buried in a connection field. That name is what makes tutorial 3
possible: promoting to staging re-points `docs-storage` at staging's own
account. `rigg env bind dev --learn` would have proposed the same binding by
scanning the files. Every binding type and its syntax is in
[the `dependencies` reference](../reference/rigg-yaml.md#dependencies).

## 5. Validate before touching Azure

```bash
rigg validate docs-rag
```

<!-- verify-live -->
```text
# output
✓ all checks passed
```

`validate` is entirely offline: JSON structure against the pinned Azure
schemas, filename/`name` consistency, exclusive ownership across projects,
every reference resolving to a resource that exists, no key material anywhere,
and every infrastructure reference classified against the bindings. A failure
exits 3 and names the file and the JSON path.

## 6. Fix the identity wiring

```bash
rigg auth doctor -e dev
```

<!-- verify-live -->
```text
# output
auth doctor env: dev
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  ✓ search-system → Search Index Data Contributor @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search
  ✗ search-system → Storage Blob Data Reader @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs
      the indexer reads blobs with the search service's system-assigned identity
      files:  data-sources/docs-ds.json:credentials.connectionString
      fix:    az role assignment create --assignee <search-identity-object-id> --role <role-guid> --scope "/subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs" # Storage Blob Data Reader
  ! blob soft delete is not enabled on contosodocs
      the data source's deletion detection policy requires it
      fix:    az storage account blob-service-properties update --ids "<storage-account-id>" --enable-delete-retention true --delete-retention-days 7

summary: 4 ok, 2 missing, 0 unresolved
```

`auth doctor` derives what the environment *requires* from the files
themselves — every role, every service setting, every network condition — and
checks each against Azure. Each finding names the principal, the role, the ARM
scope, the file and JSON path that caused it, and the `az` command that fixes
it. It exits 4 when anything is missing, so it works as a CI gate.

Let rigg apply the ones it owns:

```bash
rigg auth doctor -e dev --fix
```

<!-- verify-live -->
```text
# output
2 rigg can fix:
  - assign 'Storage Blob Data Reader' to <search-identity-object-id> at /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs
  - enable blob soft delete (7 days) on /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs
Apply 2 fix(es)? (Y/n) y

2 fix(es) applied, 0 failed
✓ re-run `rigg auth doctor` to confirm (role assignments take a moment to propagate)
```

`--fix` only ever changes things rigg owns: role assignments *between
services*, a service identity, the search service's auth options, storage
firewall and soft-delete settings. It will never grant **you** a role — that
would let anyone who can run rigg escalate their own access — so operator gaps
always come back as an `az` line for whoever owns the subscription.

Every assignment rigg creates is tagged (`rigg:<workspace>:<env>:<reason>`) so
`rigg auth roles list` can show them and `rigg auth roles remove` can undo
exactly those and nothing else.

## 7. Push

```bash
rigg push docs-rag
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  create data-sources/docs-ds
  create indexes/docs-index
  create skillsets/docs-skills
  create indexers/docs-indexer
  create knowledge-sources/docs-ks
  create knowledge-bases/docs-kb

Apply 6 change(s)? (y/N) y
  ✓ data-sources/docs-ds
  ✓ indexes/docs-index
  ✓ skillsets/docs-skills
  ✓ indexers/docs-indexer
  ✓ knowledge-sources/docs-ks
  ✓ knowledge-bases/docs-kb
```

The order is not the order you wrote the files in — it is a topological sort
of the reference graph, so nothing is created before what it points at. Before
the first write, push re-ran the auth check *scoped to this plan*: a push that
touches one synonym map is never blocked by an unrelated storage grant.

Creating an indexer makes Azure run it once immediately.

## 8. Run the indexer and watch it

```bash
rigg az indexer run docs-indexer --watch
```

<!-- verify-live -->
```text
# output
  Search:  contoso-search → https://contoso-search.search.windows.net
  ✓ triggered a run of 'docs-indexer'
  … inProgress
  … success
```

`rigg az` is the *runtime* half of the CLI: unlike `push`/`pull`/`diff` it
addresses live resources by their physical name and needs no project
ownership. `--watch` polls until the run reaches a terminal state and exits
non-zero if it failed, which makes it usable in a script.

If the run fails, `rigg az indexer status docs-indexer` prints the per-document
errors, and `rigg auth doctor -e dev --live` reads the same last-run result and
attributes auth-shaped failures to the identity edge that explains them.

Check that documents actually landed:

```bash
rigg az index stats docs-index
rigg az index query docs-index "onboarding"
```

## 9. Ask the knowledge base

```bash
rigg az knowledge-base ask docs-kb "What is the parental leave policy?"
```

<!-- verify-live -->
```text
# output
  Search:  contoso-search → https://contoso-search.search.windows.net

References:
  [1] Employee handbook — Leave  (score 0.87)
  [2] Benefits overview  (score 0.71)

[ref 1] Employees are entitled to ... parental leave, which may be taken in
```

A knowledge base is *agentic retrieval*: rather than a keyword query, it takes
a semantic intent, plans across its knowledge sources, and returns grounding
content plus references. This is exactly what the agent will call in the next
step — testing it here means that if the agent later gives a bad answer, you
already know whether the retrieval layer was at fault.

## 10. Add the Foundry agent

The agent needs a model deployment to run on:

```bash
rigg new deployment gpt-4.1-mini -p docs-rag
rigg new agent docs-agent -p docs-rag
```

Edit `projects/docs-rag/envs/dev/foundry/deployments/gpt-4.1-mini.json` to set
the model version and keep `sku.capacity` small. Then edit
`projects/docs-rag/envs/dev/foundry/agents/docs-agent.json` so the agent names
that deployment and grounds on the knowledge base:

```json
{
  "name": "docs-agent",
  "kind": "prompt",
  "model": "gpt-4.1-mini",
  "instructions": { "$file": "docs-agent.instructions.md" },
  "tools": [
    { "type": "mcp", "x-rigg-ref": "knowledge-bases/docs-kb", "server_url": "" }
  ]
}
```

Two rigg-specific things are happening here.

`{"$file": "docs-agent.instructions.md"}` is a **sidecar**: the instructions
live in a Markdown file next to the JSON, so a prompt change is a readable
diff in a pull request instead of one enormous escaped string. Create that
file and write the agent's instructions in it.

[`x-rigg-ref`](../reference/annotations.md#x-rigg-ref) is an annotation that
says *which* knowledge base, not *where* it is. At push time rigg computes the
knowledge base's MCP endpoint for the environment it is pushing to and fills in
`server_url`. The same file therefore works in dev, staging and prod without
edits — and because `x-rigg-*` keys are stripped before the request, Azure
never sees the annotation.

Push, and let rigg prove the whole stack works end to end:

```bash
rigg push docs-rag --verify
```

<!-- verify-live -->
```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
  create deployments/gpt-4.1-mini
  create agents/docs-agent

Apply 2 change(s)? (y/N) y
  ✓ deployments/gpt-4.1-mini
  ✓ agents/docs-agent
Verify project 'docs-rag' (env: dev)
  ✓ triggered a run of 'docs-indexer'
  … success
  ✓ indexer 'docs-indexer' — 128 processed, 0 failed
  ✓ knowledge base 'docs-kb' retrieved
  ✓ agent 'docs-agent' replied
✓ 3 check(s) passed
```

## 11. Talk to it

```bash
rigg az agent ask docs-agent "How much parental leave do I get?"
```

<!-- verify-live -->
```text
# output
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com/api/projects/rag
Employees are entitled to 480 days of parental leave, of which 390 are paid at
the income-related rate. See the Employee handbook, section 4.2.
```

## What you have now

- A complete Agentic RAG stack, every piece a file you can review and diff.
- The keyless wiring to go with it: managed identity, role assignments made by
  `auth doctor --fix`, no secret anywhere on disk.
- An agent whose grounding endpoint is derived at push time, so the same files
  can target any environment.

Commit it:

```bash
git add . && git commit -m "docs-rag: blob → index → knowledge base → agent"
```

## Clean up

If this was an experiment rather than the start of something:

```bash
rigg delete docs-rag --remote
rigg auth roles remove -e dev
```

`rigg delete` removes the project's resources from Azure in reverse dependency
order and keeps the local files. `auth roles remove` removes exactly the role
assignments rigg created for this environment — never anything that was
already there. The model deployment is the one that keeps billing, so check it
is gone.

## Next

- [Tutorial 3 — Add an environment and promote](03-add-an-environment-and-promote.md)
- [Resource files reference](../reference/resource-files.md) — every kind,
  every field rigg strips, and the infrastructure paths it rewrites.
- [Annotations reference](../reference/annotations.md) — `x-rigg-api`,
  `x-rigg-auth`, `x-rigg-pin`, `x-rigg-ref`, `x-rigg-note`.
