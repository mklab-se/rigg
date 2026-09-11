# Tutorial 2 — Build an Agentic RAG stack from scratch

Nothing exists yet except a storage account with some documents in it. By the
end you will have the whole chain — blob container → data source → index →
skillset → indexer → knowledge source → knowledge base → Foundry agent — as
files in Git, running in Azure, answering questions.

| | |
|---|---|
| **Time** | About 45 minutes, most of it waiting for the first indexer run. |
| **Cost** | This one *does* create resources. An index and its indexer runs cost storage and ingestion; the model deployment the knowledge base and the agent share bills per token. Reuse a deployment you already have where you can, and [clean up](#clean-up) when you are done. |
| **You need** | • Tutorial 1's setup: `rigg` installed, `az login` done<br>• An Azure AI Search service (`contoso-search`) and a Microsoft Foundry account/project (`contoso-ai`/`rag`) with at least one chat model deployment — `gpt-4.1-mini` in the examples. Both the knowledge base and the agent need one<br>• An Azure Storage account (`contosodocs`) with a blob container (`handbook`) holding a few PDFs, Word files or Markdown documents<br>• Role: `Search Service Contributor` on the Search service<br>• Role: `Azure AI User` on the Foundry project, and `Contributor` on the Foundry account if you want rigg to create a model deployment<br>• Role: `Storage Blob Data Contributor` on the storage account if you are the one uploading the documents — that is a data-plane role, and `Contributor` alone does not include it<br>• Role: `User Access Administrator` or `Owner` on the storage account **or** the resource group, so rigg can grant the service identities the roles the stack needs |
| **You get** | A complete Agentic RAG stack, every piece a reviewable file, wired keylessly. |

> [!TIP]
> You do not need to have completed tutorial 1, but it explains `init`,
> projects and bindings in more detail than this one repeats. If you cannot
> grant roles yourself, `rigg auth doctor` prints the exact `az` line to hand
> to someone who can.

Start from a workspace, from tutorial 1 or fresh:

```bash
mkdir contoso-rag && cd contoso-rag
rigg init .
rigg new project docs-rag
```

## Step 1 — Scaffold the whole retrieval pipeline

One command writes the six files of the retrieval chain, already wired to each
other.

```bash
rigg new pipeline docs -p docs-rag --type azureblob
```

```text
# output
  created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/data-sources/docs-ds.json
  created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/indexes/docs-index.json
  created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/skillsets/docs-skills.json
  created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/indexers/docs-indexer.json
  created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/knowledge-sources/docs-ks.json
  created /Users/you/contoso-rag/projects/docs-rag/envs/dev/search/knowledge-bases/docs-kb.json

Pipeline 'docs' scaffolded in project 'docs-rag':
  1. Edit the data source (connection ResourceId, container)
  2. Shape the index fields for your data
  3. Adjust or remove the skillset, wire the indexer
  4. Push step by step: rigg push docs-rag
```

**Why it matters.** The indexer names the data source, index and skillset; the
knowledge source points at the index; the knowledge base routes to the
knowledge source. Everything is **explicit** — there is no hidden
Azure-generated pipeline behind a knowledge source, so the whole retrieval
chain is reviewable files.

> [!NOTE]
> `--type` picks the data-source type (`azureblob` or `adlsgen2`; blob storage
> is the only source rigg 2.0 supports). Three of the six files ship with `<…>`
> placeholders, and all three have to be replaced before the stack works: the
> data source's connection string and container (step 2), the indexer's field
> mappings (step 3) and the knowledge base's model (step 4). Every kind and
> every field rigg strips is in
> [the resource files reference](../reference/resource-files.md).

## Step 2 — Point the data source at your container

Open `projects/docs-rag/envs/dev/search/data-sources/docs-ds.json` and replace
the four `<…>` placeholders the scaffold left.

```json
{
  "name": "docs-ds",
  "type": "azureblob",
  "credentials": {
    "connectionString": "ResourceId=/subscriptions/<subscription-id>/resourceGroups/<rg>/providers/Microsoft.Storage/storageAccounts/<storage-account>;"
  },
  "container": { "name": "<container-name>" },
  "dataChangeDetectionPolicy": null,
  "dataDeletionDetectionPolicy": {
    "@odata.type": "#Microsoft.Azure.Search.NativeBlobSoftDeleteDeletionDetectionPolicy"
  }
}
```

**Why it matters.** Note what is *not* there: an account key. `ResourceId=` is
the keyless form — the Search service authenticates to storage with its managed
identity, and rigg's job in step 6 is to make sure that identity actually has
the role. A file that did contain a key would be rejected by `rigg validate`;
see
[CONCEPTS → How rigg handles authentication](../../CONCEPTS.md#how-rigg-handles-authentication).

> [!NOTE]
> The examples that follow use `contoso-rg`, `contosodocs` and `handbook`.
> `dataChangeDetectionPolicy: null` means "use the built-in policy"; a blob
> indexer tracks a high-water mark either way, which is why a re-run over
> unchanged documents reports `0 processed`. The deletion policy is filled in
> for you because without one, documents deleted from the container stay in the
> index forever.

> [!TIP]
> By default the connection uses the search service's **system-assigned**
> identity, which is the only one Azure Storage's trusted-services firewall
> exception accepts. If your stack spans several services and you would rather
> use one user-assigned identity everywhere, bind it and scaffold the data
> source against it — delete the one the pipeline wrote first, or use a
> different name:
>
> ```bash
> rigg env bind dev docs-identity identity:contoso-rag-identity
> rigg new data-source docs-uami -p docs-rag --type azureblob --identity docs-identity
> ```

`--identity <binding>` applies to the kinds that carry one (`data-source` and
`skillset`); it writes the bound identity's ARM id into the scaffold instead of
leaving the service's own.

## Step 3 — Shape the index, and map the fields into it

`docs-index.json` ships with a minimal schema — `id`, `content`, `title`, `url`
and a semantic configuration. Edit the fields to match your documents, then
tell the indexer which blob metadata fills them, in
`indexers/docs-indexer.json`:

```json
{
  "fieldMappings": [
    { "sourceFieldName": "metadata_storage_path", "targetFieldName": "id",
      "mappingFunction": { "name": "base64Encode" } },
    { "sourceFieldName": "metadata_storage_name", "targetFieldName": "title" },
    { "sourceFieldName": "metadata_storage_path", "targetFieldName": "url" }
  ]
}
```

**Why it matters.** A blob indexer populates `content` on its own and will
invent a key for you, but `title` and `url` arrive empty unless you say where
they come from. The key field has to be a valid Azure Search document key, and
a blob path is not one — `base64Encode` is what makes it legal.

> [!WARNING]
> The index is the one file worth spending real time on, because **index fields
> cannot be removed in Azure** once created. Adding is easy; removing means
> recreating the index. If you do not need the enrichment skillset, delete
> `skillsets/docs-skills.json` and remove `"skillsetName"` from the indexer.

## Step 4 — Give the knowledge base a model

A knowledge base plans its retrieval with a chat model, so fill in the
placeholders in `knowledge-bases/docs-kb.json`.

```json
{
  "models": [
    {
      "kind": "azureOpenAI",
      "azureOpenAIParameters": {
        "resourceUri": "https://<foundry-account>.openai.azure.com",
        "deploymentId": "<deployment-name>",
        "modelName": "<model-name>"
      }
    }
  ]
}
```

**Why it matters.** Without a model, Azure rejects every call to the knowledge
base — `A Knowledge Base model must be specified to use any reasoning effort
other than 'Minimal'`. Point it at a deployment that already exists —
`contoso-ai` and `gpt-4.1-mini` here. The Search service calls that model with
its own managed identity, which is a role you do not have to remember: step 6
derives the requirement from this very field.

## Step 5 — Record the storage binding, then validate

Name the storage account as a dependency of the environment, then check the
files offline.

```bash
rigg env bind dev docs-storage storage:contosodocs
rigg validate docs-rag
```

```text
# output
✓ all checks passed
```

**Why it matters.** The storage account is now a named dependency rather than a
string buried in a connection field, and that name is what makes tutorial 3
possible: promoting to staging re-points `docs-storage` at staging's own
account. `validate` is entirely offline — JSON structure against the pinned
Azure schemas, filename/`name` consistency, exclusive ownership across
projects, every reference resolving, no key material anywhere, and every
infrastructure reference classified against the bindings. A failure exits 3 and
names the file and the JSON path.

> [!TIP]
> `rigg env bind dev --learn` would have proposed the same binding by scanning
> the files. Every binding type and its syntax is in
> [the `dependencies` reference](../reference/rigg-yaml.md#dependencies).

## Step 6 — Check the identity wiring

`auth doctor` derives what the environment requires from the files themselves
and checks each requirement against Azure.

```bash
rigg auth doctor -e dev
```

```text
# output
auth doctor env: dev
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  ✓ search-system → Storage Blob Data Reader @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs
      search service 'contoso-search' (system-assigned) holds it
      files:  data-sources/docs-ds.json:credentials.connectionString
  ✓ search-system → Cognitive Services User @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.CognitiveServices/accounts/contoso-ai
      search service 'contoso-search' (system-assigned) holds it
      files:  knowledge-bases/docs-kb.json:models[0].azureOpenAIParameters.resourceUri
…
  operator: you@contoso.com
  ✓ operator → Search Service Contributor @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search
…
summary: 9 ok, 0 missing, 0 unresolved
✓ identity wiring is complete
```

**Why it matters.** Nobody told rigg the search service would call a model: it
read `models[0].azureOpenAIParameters.resourceUri` out of the knowledge base
you edited in step 4 and concluded that the search service's identity needs
`Cognitive Services User` on the Foundry account. The elided lines check the
storage firewall, shared-key access, blob soft delete, the search SKU, its
identity and its Entra tokens. Every finding names the principal, the role, the
ARM scope, the file and JSON path that caused it — and, when it is missing, the
`az` command that fixes it.

A missing finding reads like this, and `auth doctor` exits 4 when anything is
missing, so it works as a CI gate:

```text
  ✗ search-system → Storage Blob Data Reader @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosodocs
      the indexer reads blobs with the search service's system-assigned identity
      files:  data-sources/docs-ds.json:credentials.connectionString
```

Let rigg apply the ones it owns:

```bash
rigg auth doctor -e dev --fix
```

> [!NOTE]
> `--fix` only ever changes things rigg owns: role assignments *between
> services*, a service identity, the search service's auth options, storage
> firewall and soft-delete settings. It will never grant **you** a role — that
> would let anyone who can run rigg escalate their own access — so operator
> gaps always come back as an `az` line. Every assignment rigg creates is
> tagged (`rigg:<workspace>:<env>:<reason>`) so `rigg auth roles list` can show
> them and `rigg auth roles remove` can undo exactly those and nothing else.

## Step 7 — Push

Send the six definitions to Azure.

```bash
rigg push docs-rag
```

```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  create data-sources/docs-ds
  create indexes/docs-index
  create skillsets/docs-skills
  create indexers/docs-indexer
  create knowledge-sources/docs-ks
  create knowledge-bases/docs-kb
? Apply 6 change(s)? (y/N) y
  ✓ data-sources/docs-ds
…
  ✓ knowledge-bases/docs-kb
```

**Why it matters.** The order is not the order you wrote the files in — it is a
topological sort of the reference graph, so nothing is created before what it
points at. Before the first write, push re-ran the auth check *scoped to this
plan*: a push that touches one synonym map is never blocked by an unrelated
storage grant. Creating an indexer makes Azure run it once immediately.

> [!NOTE]
> Run `rigg status` after this push and the skillset may say `remote ahead
> (pull pending)`. That is not drift you caused: Azure fills in a skill's
> optional defaults (`defaultLanguageCode: "en"`, `pageOverlapLength: 0`) some
> time *after* the create, so the copy rigg read back at push time was still
> all-nulls. `rigg pull docs-rag` takes Azure's version and the two agree from
> then on. A push in the meantime is safe — it prints `skip
> skillsets/docs-skills (remote changed since last sync — pull first)` and
> leaves it alone.

## Step 8 — Run the indexer and watch it

Clear the change-tracking state, then run the indexer to see the whole corpus
go through.

```bash
rigg az indexer reset docs-indexer
rigg az indexer run docs-indexer --watch
```

```text
# output
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  ✓ reset docs-indexer — run it with: rigg az indexer run docs-indexer
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  ✓ triggered a run of 'docs-indexer'
  … success
  ✓ run completed: 2 processed, 0 failed
```

**Why it matters.** `rigg az` is the *runtime* half of the CLI: unlike
`push`/`pull`/`diff` it addresses live resources by their physical name and
needs no project ownership. `--watch` polls until the run reaches a terminal
state and exits non-zero if it failed, which makes it usable in a script.

> [!TIP]
> If the run fails, `rigg az indexer status docs-indexer` prints the
> per-document errors, and `rigg auth doctor -e dev --live` reads the same
> last-run result and attributes auth-shaped failures to the identity edge that
> explains them.

Check that documents actually landed:

```bash
rigg az index stats docs-index
rigg az index query docs-index "onboarding"
```

```text
# output
…
Index 'docs-index'
  documents: 2
  storage:   16.7 KiB
  vectors:   0 B
…
1 match(es) in 'docs-index' (showing 1)

[1] score 0.282
  id: aHR0cHM6Ly9jb250b3NvZG9jcy5ibG9iLmNvcmUud2luZG93cy5uZXQvaGFuZGJvb2svb25ib2FyZGluZy5tZA4
  content: # Onboarding checklist
…
  title: onboarding.md
  url: https://contosodocs.blob.core.windows.net/handbook/onboarding.md
```

The base64 `id` and the populated `title`/`url` are the field mappings from
step 3 doing their job.

## Step 9 — Ask the knowledge base

Test retrieval on its own, before an agent is in the picture.

```bash
rigg az knowledge-base ask docs-kb "What is the parental leave policy?"
```

```text
# output
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com

[ref 0] # Leave policy

## Parental leave

Employees are entitled to 480 days of parental leave per child. 390 of those
days are paid at the income-related rate; the remaining 90 are paid at the
flat rate. Leave may be taken until the child turns twelve.

(chunks truncated for reading — full text via --output json)

References:
  [1] aHR0cHM6Ly9jb250b3NvZG9jcy5ibG9iLmNvcmUud2luZG93cy5uZXQvaGFuZGJvb2svbGVhdmUtcG9saWN5Lm1k6 (score 3.43)
```

**Why it matters.** A knowledge base is *agentic retrieval*: rather than a
keyword query, it takes a semantic intent, plans across its knowledge sources,
and returns grounding **passages** plus the document keys they came from — not
a synthesized answer. Synthesis is the agent's job in the next step. Testing
retrieval here means that if the agent later gives a bad answer, you already
know whether the retrieval layer was at fault.

## Step 10 — Add the Foundry agent

The agent needs two things beyond its own file: a model deployment to run on,
and a Foundry **connection** that lets it call the knowledge base.

```bash
rigg new connection docs-kb-conn -p docs-rag
rigg new agent docs-agent -p docs-rag
```

```text
# output
Created /Users/you/contoso-rag/projects/docs-rag/envs/dev/foundry/connections/docs-kb-conn.json
Created /Users/you/contoso-rag/projects/docs-rag/envs/dev/foundry/agents/docs-agent.json
```

> [!NOTE]
> Reuse a deployment that already exists in the project — name it in the agent
> file and rigg will warn, once, that it is not a workspace file: `warning:
> [projects/docs-rag/envs/dev/foundry/agents/docs-agent.json] references
> deployments/gpt-4.1-mini — not in this workspace (must already exist in
> Azure)`. To manage the deployment as code instead, `rigg new deployment
> gpt-4.1-mini -p docs-rag` writes a file for it; push creates it, and it bills
> per token from then on, so keep `sku.capacity` small.

Foundry will not call the knowledge base's MCP endpoint without a connection
that says how to authenticate to it. Fill in `connections/docs-kb-conn.json`:

```json
{
  "name": "docs-kb-conn",
  "properties": {
    "category": "RemoteTool",
    "group": "GenericProtocol",
    "authType": "ProjectManagedIdentity",
    "audience": "https://search.azure.com",
    "target": "https://contoso-search.search.windows.net/knowledgebases/docs-kb/mcp?api-version=2026-08-01-preview",
    "metadata": { "knowledgeBaseName": "docs-kb" }
  }
}
```

`ProjectManagedIdentity` means the Foundry project calls Search as itself — no
key, no secret. Then wire the agent to both:

```json
{
  "name": "docs-agent",
  "kind": "prompt",
  "model": "gpt-4.1-mini",
  "instructions": { "$file": "docs-agent.instructions.md" },
  "tools": [
    {
      "type": "mcp",
      "x-rigg-ref": "knowledge-bases/docs-kb",
      "server_url": "",
      "require_approval": "never",
      "project_connection_id": "docs-kb-conn"
    }
  ]
}
```

Two rigg-specific things are happening here:

- `{"$file": "docs-agent.instructions.md"}` is a **sidecar**: the instructions
  live in a Markdown file next to the JSON, so a prompt change is a readable
  diff in a pull request instead of one enormous escaped string. `rigg new
  agent` created that file — write the agent's instructions in it.
- [`x-rigg-ref`](../reference/annotations.md#x-rigg-ref) says *which* knowledge
  base, not *where* it is. At push time rigg computes the knowledge base's MCP
  endpoint for the target environment, fills in `server_url`, and derives the
  `server_label` Foundry insists on — so the same file works in dev, staging
  and prod. `x-rigg-*` keys are stripped before the request, so Azure never
  sees the annotation. The
  [annotations reference](../reference/annotations.md) covers `x-rigg-api`,
  `x-rigg-auth`, `x-rigg-pin`, `x-rigg-ref` and `x-rigg-note`.

Push, and let rigg prove the whole stack works end to end:

```bash
rigg push docs-rag --verify
```

```text
# output
Push project 'docs-rag' (env: dev)
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
  create connections/docs-kb-conn
  create agents/docs-agent

  ! auth preflight: 1 requirement(s) missing for this plan
    ✗ foundry-project → Search Index Data Reader @ /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search — Foundry project '/subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.CognitiveServices/accounts/contoso-ai/projects/rag' lacks 'Search Index Data Reader' here
…
  fix assign 'Search Index Data Reader' to <foundry-project-object-id> at /subscriptions/<subscription-id>/resourceGroups/contoso-rg/providers/Microsoft.Search/searchServices/contoso-search
      ✓ applied
  waiting for 1 role assignment(s) to become visible (up to ~3 min)
      ✓ 'Search Index Data Reader' is visible
  ✓ connections/docs-kb-conn
  ✓ agents/docs-agent

Verify project 'docs-rag' (env: dev)
…
  ✓ indexer 'docs-indexer' — 0 processed, 0 failed
  ✓ knowledge base 'docs-kb' retrieved
  ✓ agent 'docs-agent' replied
✓ 3 check(s) passed
```

**Why it matters.** That preflight block is the tutorial in miniature. Nobody
wrote down that a Foundry project calling a knowledge base needs `Search Index
Data Reader` on the Search service; rigg derived it from the connection and the
agent tool you wrote, found it missing, fixed it, waited for Entra to make the
assignment visible, and only then pushed. `0 processed` on the indexer is
the high-water mark from step 8 — nothing has changed in the container since.

## Step 11 — Talk to it

Ask the agent the question its grounding data can answer.

```bash
rigg az agent ask docs-agent "How much parental leave do I get?"
```

```text
# output
  Search:  contoso-search → https://contoso-search.search.windows.net
  Foundry: contoso-ai/rag → https://contoso-ai.services.ai.azure.com
You are entitled to 480 days of parental leave per child. Of these, 390 days are paid at the income-related rate, and the remaining 90 days are paid at the flat rate. The leave may be taken until the child turns twelve.【4:0†leave-policy.md】
```

**Why it matters.** The citation at the end is the agent naming the blob its
answer came from — the chain from step 2's container to this sentence, closed.

Commit it:

```bash
git add . && git commit -m "docs-rag: blob → index → knowledge base → agent"
```

## What you have now

- [x] A complete Agentic RAG stack, every piece a file you can review and diff.
- [x] Keyless wiring: managed identities, role assignments made by the auth
      preflight, no secret anywhere on disk.
- [x] An indexer that has ingested your corpus, and an index you can query.
- [x] An agent whose grounding endpoint is derived at push time, so the same
      files can target any environment.

## Clean up

If this was an experiment rather than the start of something, remove the
project's resources from Azure in reverse dependency order — the local files
stay — and withdraw exactly the role assignments rigg created for this
environment.

```bash
rigg delete docs-rag --remote
rigg auth roles remove -e dev
```

> [!WARNING]
> A model deployment is the one thing that keeps billing after you stop
> looking, so check it is gone — or, if you reused an existing one as this
> tutorial did, that it is *still there*: `rigg delete --remote` removes what
> the project owns, and a deployment you never adopted was never the project's
> to delete.

## Next

[Tutorial 3 — Add an environment and promote](03-add-an-environment-and-promote.md)
