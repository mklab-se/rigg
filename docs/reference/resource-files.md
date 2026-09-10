# Resource files

Every resource a project owns is one JSON file on disk:

```
projects/<project>/envs/<env>/search/<kind-dir>/<name>.json
projects/<project>/envs/<env>/foundry/<kind-dir>/<name>.json
```

The file stem is the resource's logical id; the physical Azure name lives in
the `name` field inside the file. Directory contents are membership — a file
under a project's tree is that project's resource, and no other project may
claim it.

The contents are Azure's own document for that resource — the body of the
REST API's `PUT`, near enough — so Microsoft's reference for each kind is the
reference for what may go in the file. What this page documents is everything
rigg adds around that: where each kind lives, which fields rigg strips in
which direction, what it refuses to store, and which fields it rewrites when
you promote between environments.

## The twelve kinds

| Kind | Directory | Managed through | Channel |
|---|---|---|---|
| Data source | `search/data-sources/` | Azure AI Search data plane | stable |
| Index | `search/indexes/` | Azure AI Search data plane | stable |
| Skillset | `search/skillsets/` | Azure AI Search data plane | stable |
| Indexer | `search/indexers/` | Azure AI Search data plane | stable |
| Synonym map | `search/synonym-maps/` | Azure AI Search data plane | stable |
| Alias | `search/aliases/` | Azure AI Search data plane | stable |
| Knowledge source | `search/knowledge-sources/` | Azure AI Search data plane | stable |
| Knowledge base | `search/knowledge-bases/` | Azure AI Search data plane | **preview** |
| Agent | `foundry/agents/` | Foundry project data plane (`v1`) | stable |
| Model deployment | `foundry/deployments/` | ARM, `Microsoft.CognitiveServices` | stable |
| Connection | `foundry/connections/` | ARM, `Microsoft.CognitiveServices` | stable |
| Guardrail (RAI policy) | `foundry/guardrails/` | ARM, `Microsoft.CognitiveServices` | stable |

"Channel" is the Azure AI Search api-version a kind requires. Knowledge bases
need the preview channel because their retrieval and output configuration
does not exist in the stable api-version — a stable `GET` silently omits it
and a stable `PUT` cannot set it. The api-versions themselves are pinned in
rigg's registry and overridable per environment
([rigg.yaml § The `search` target](rigg-yaml.md#the-search-target)).

Scaffold one of any kind with `rigg new <kind> <name>`, or a whole
blob → index → indexer → knowledge source → knowledge base chain with
`rigg new pipeline <name>`.

## Naming: stem vs. `name`

Two names are in play and they are allowed to differ.

| | Where | What it is |
|---|---|---|
| **Stem** | the file name without `.json` | The resource's *logical id* — how rigg correlates the same resource across environments, and what `x-rigg-ref`, `rigg status` and `--only` use |
| **`name`** | the `name` field inside the file | The *physical* Azure name — what actually exists in the service |

Normally they match. They diverge when a resource is named differently in
different environments: `envs/dev/search/indexes/contoso-docs.json` may hold
`"name": "contoso-docs-dev"` while the `prod` file at the same stem holds
`"name": "contoso-docs"`. Because `rigg promote` correlates by stem and keeps
the target's `name`, the two trees stay aligned without renaming anything in
Azure.

Every file needs a `name`:

```
✗ [projects/contoso-docs/envs/dev/search/indexes/contoso-docs.json] missing "name" field
```

and no two files in one kind directory may claim the same physical name:

```
Error: duplicate physical name 'contoso-docs': both projects/contoso-docs/envs/dev/search/indexes/a.json and projects/contoso-docs/envs/dev/search/indexes/b.json define a resource named 'contoso-docs' — physical (Azure) names must be unique within a kind
```

## What rigg strips, and when

A file is never a verbatim copy of what Azure returns, and what is pushed is
never a verbatim copy of the file. Four filters, all driven by rigg's
registry:

| Class | Removed on pull (never on disk) | Removed on push | Removed before comparison | Why |
|---|---|---|---|---|
| **Volatile** | yes | yes | yes | Azure rewrites them on every read — `@odata.etag`, `@odata.context`, `etag`, and per-kind equivalents. Keeping them would make every `rigg status` show drift |
| **Read-only** | yes | yes | yes | Returned by `GET` but rejected by `PUT`. Writing them to disk would guarantee a failed push |
| **`x-rigg-*`** | no — kept | **yes** | yes | rigg-local [annotations](annotations.md). Yours, never Azure's |
| **Write-only** | no — kept | no — sent | **yes** | Accepted by `PUT` but redacted on `GET` (a data source's `credentials.connectionString`). Comparing them would show permanent phantom drift |

Concretely, per kind:

| Kind | Volatile | Read-only | Write-only |
|---|---|---|---|
| Data source | `@odata.etag`, `@odata.context`, `e_tag`, `etag` | — | `credentials.connectionString` |
| Index, skillset, indexer, synonym map, alias, knowledge base | `@odata.etag`, `@odata.context`, `e_tag`, `etag` | — | — |
| Knowledge source | `@odata.etag`, `@odata.context`, `e_tag`, `etag` | `azureBlobParameters.createdResources`, `indexedOneLakeParameters.createdResources` | — |
| Agent | `@odata.etag`, `@odata.context`, `id`, `object`, `created_at`, `updated_at`, `version`, `metadata.modified_at` | — | — |
| Model deployment | `id`, `type`, `systemData`, `etag`, `properties.provisioningState`, `properties.capabilities`, `properties.rateLimits`, `properties.model.callRateLimit`, `properties.currentCapacity`, `properties.deploymentState` | — | — |
| Connection | `id`, `type`, `systemData`, `etag`, `properties.provisioningState` | — | — |
| Guardrail | `id`, `type`, `systemData`, `etag` | — | — |

A knowledge source's `createdResources` is read-only because rigg's model is
explicit-only: resources Azure creates for you are Azure's to manage, and
rigg never adopts them into your files.

An indexer's execution history is not in this table because it is not part of
the indexer document at all — it lives on the separate `/status` resource,
which `rigg az indexer status` fetches and rigg never merges into the file.

### Immutable fields

Some fields the service will not change in place. When a local value differs
from the remote one, an in-place `PUT` cannot reconcile the two and the
resource has to be deleted and re-created — `rigg push` shows this as
`replace` rather than `update`, and gates it separately from the ordinary
apply prompt: interactively it asks (defaulting to No), and non-interactively
`--yes` is deliberately not enough —

```
Error: push plan contains replace(s); pass --allow-replace (in addition to --yes) to proceed
```

| Kind | Immutable field |
|---|---|
| Knowledge source | `kind` (`azureBlob`, `searchIndex`, …) |

Replacing a knowledge source means unlinking every knowledge base that points
at it first and relinking afterwards, including knowledge bases in other
projects. rigg writes a [recovery file](state.md#replace-recovery-files) so an
interrupted replace can be finished by the next push.

## Secrets are never stored locally

`rigg validate` refuses to let key material into a file. Each kind declares
the paths that could carry a credential:

| Kind | Fields checked |
|---|---|
| Data source | `credentials.connectionString` |
| Index | `encryptionKey.accessCredentials.applicationSecret`, `vectorSearch.vectorizers[].azureOpenAIParameters.apiKey` |
| Skillset | `cognitiveServices.key`, `skills[].apiKey`, `encryptionKey.accessCredentials.applicationSecret` |
| Synonym map | `encryptionKey.accessCredentials.applicationSecret` |
| Knowledge source | `searchIndexParameters.apiKey`, `azureBlobParameters.connectionString` |
| Knowledge base | `models[].apiKey`, `models[].azureOpenAIParameters.apiKey` |
| Connection | `properties.credentials.key`, `.keys`, `.secret`, `.clientSecret`, `.pat`, `.sas` |

A value in one of these is accepted only when it is an identity-based
placeholder — a `ResourceId=` connection string, or a `<…>` scaffold
placeholder you have not filled in yet. Anything else:

```
✗ [projects/contoso-docs/envs/dev/search/data-sources/contoso-docs.json] field 'credentials.connectionString' contains a credential — rigg never stores secrets locally. Use a managed identity (connection string 'ResourceId=/subscriptions/...') and grant the identity RBAC access instead; secrets belong in Azure Key Vault, never in files
```

Two further checks are not path-based, because they cannot be: any document
containing `AccountKey=` anywhere is rejected outright, and an
`x-functions-key` header on a Web API skill is matched case-insensitively.

```
✗ [projects/contoso-docs/envs/dev/search/data-sources/contoso-docs.json] contains an 'AccountKey=' connection string — replace it with an identity-based 'ResourceId=...' connection and delete/rotate the leaked key
```

The identity-based form for a blob data source looks like this — note that
there is no key anywhere, only a resource id and, optionally, the identity to
use:

```json
{
  "name": "contoso-docs",
  "type": "azureblob",
  "credentials": {
    "connectionString": "ResourceId=/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/contoso-rg/providers/Microsoft.Storage/storageAccounts/contosostorage;"
  },
  "container": { "name": "docs" },
  "identity": {
    "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
    "userAssignedIdentity": "/subscriptions/00000000-0000-0000-0000-000000000000/resourceGroups/contoso-rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/contoso-indexer-id"
  }
}
```

Omit the `identity` block to use the search service's own system-assigned
identity. `rigg new data-source <name> --identity <binding>` scaffolds the
user-assigned form; `rigg auth doctor` reports whichever principal a resource
ends up using and whether it has the roles it needs.

When a key genuinely cannot be avoided — an Azure Function that will not take
a token — name its *source* rather than its value with
[`x-rigg-auth`](annotations.md#x-rigg-auth), and rigg fetches it at push time.

Data sources are limited to Azure Blob Storage: `azureblob` and `adlsgen2`
(the same type with hierarchical namespace enabled) are the only accepted
`type` values.

## `$file` sidecars

A long string field is miserable to review as a JSON one-liner with `\n`
escapes. Any string field may instead be an object naming a file next to the
JSON:

```json
{
  "name": "contoso-assistant",
  "model": "gpt-5.2-chat",
  "instructions": { "$file": "contoso-assistant.instructions.md" }
}
```

```
foundry/agents/contoso-assistant.json
foundry/agents/contoso-assistant.instructions.md
```

The path is relative to the JSON file's own directory. On load — for
validate, diff, push, everything — the file's content is inlined as the
string value; on pull the value is extracted back out to the sidecar and
replaced with the `$file` reference. Azure only ever sees the string.

Extraction on pull happens for a field when either of two things is true:

- the kind declares it as a sidecar field by default — today that is exactly
  **`instructions` on an agent**; or
- a sidecar file for that field already exists on disk **at the name rigg
  looks for**, in which case rigg keeps using it.

That second rule is what lets you opt any field in, and it has two conditions
that are easy to miss:

- **The filename must be `<json-stem>.<field>.md`, exactly.** For
  `search/skillsets/contoso-enrich.json` and a field `description`, that is
  `contoso-enrich.description.md` in the same directory. rigg does not follow
  the name inside the `$file` object when it decides whether to extract — it
  builds the expected name and checks whether that file exists. A `$file`
  pointing at `notes.md` is read fine on load, but the next `rigg pull`
  extracts nothing, inlines the prose back into the JSON, and leaves
  `notes.md` orphaned.
- **The field must be top-level.** Extraction looks at the document's own
  keys only; a long string nested inside an object or array — a skill's
  `description`, a scoring function's text — cannot be a sidecar, and a
  `$file` there survives only until the next pull rewrites the document.

Get both right and every future pull writes the prose back to the Markdown
rather than into the JSON. Diffs then show the prose line by line, which is
the point.

A `$file` pointing nowhere is an error rather than an empty string:

```
Error: sidecar file not found: projects/contoso-assistant/envs/dev/foundry/agents/contoso-assistant.instructions.md (referenced from projects/contoso-assistant/envs/dev/foundry/agents/contoso-assistant.json)
```

## References between resources

rigg reads the reference fields out of its registry, which is how it knows to
create a data source before the indexer that names it, and to delete them in
the opposite order. `rigg describe` draws the same graph.

| Kind | Field | Points at |
|---|---|---|
| Skillset | `knowledgeStore.projections[].objects[].storageContainer` | index |
| Skillset | `indexProjections.selectors[].targetIndexName` | index |
| Indexer | `dataSourceName` | data source |
| Indexer | `targetIndexName` | index |
| Indexer | `skillsetName` | skillset |
| Alias | `indexes[]` | index |
| Knowledge source | `searchIndexParameters.searchIndexName` | index |
| Knowledge base | `knowledgeSources[].name` | knowledge source |
| Agent | `model` | model deployment |
| Agent | `tools[].project_connection_id` | connection |
| Model deployment | `properties.raiPolicyName` | guardrail |

References that have no such field — above all a Foundry agent grounded on a
Search knowledge base — are declared with
[`x-rigg-ref`](annotations.md#x-rigg-ref), and count for ordering in exactly
the same way.

A reference to a resource that is not in the workspace is a warning by
default (it may legitimately be a pre-existing Azure resource);
`rigg validate --strict` makes it an error:

```
warning: [projects/contoso-assistant/envs/dev/foundry/agents/contoso-assistant.json] references deployments/text-embedding-3-large — not in this workspace (must already exist in Azure)
```

A scaffold placeholder left unfilled is always an error:

```
✗ [projects/contoso-docs/envs/dev/search/indexers/contoso-docs.json] placeholder reference '<index-name>' — replace the scaffold placeholder
```

## Infrastructure reference fields

Some fields inside a resource file do not name another rigg-managed
resource: they name supporting Azure infrastructure — a storage account, a
user-assigned identity, a model host, a function app, a key vault, an
external API, or the search service itself. rigg knows exactly which fields
those are, which is how `rigg promote` can re-point each one at the target
environment's binding of the same name instead of copying the source value.

The tables below list every such field per resource kind, together with the
`dependencies:` binding type in `rigg.yaml` it resolves to and the internal
form used to recognise and rewrite the value. `Only for @odata.type`
restricts a rule to array elements of one Azure type (for example, only
`WebApiSkill` entries inside `skills[]`).

The tables are generated from rigg's registry — run
`rigg dev infra-table` and replace the text between the markers to refresh
them.

<!-- generated:infra-table:start -->
### data-sources

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `credentials.connectionString` | storage | `StorageResourceId` | — |
| `identity` | identity | `UserAssignedIdentity` | — |
| `encryptionKey.keyVaultUri` | key-vault | `KeyVaultUri` | — |
| `encryptionKey.identity` | identity | `UserAssignedIdentity` | — |

### indexes

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `vectorSearch.vectorizers[].azureOpenAIParameters.resourceUri` | ai-services | `OpenAiEndpoint` | — |
| `vectorSearch.vectorizers[].azureOpenAIParameters.authIdentity` | identity | `UserAssignedIdentity` | — |
| `encryptionKey.keyVaultUri` | key-vault | `KeyVaultUri` | — |
| `encryptionKey.identity` | identity | `UserAssignedIdentity` | — |

### skillsets

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `skills[].resourceUri` | ai-services | `OpenAiEndpoint` | `#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill` |
| `skills[].authIdentity` | identity | `UserAssignedIdentity` | — |
| `skills[].uri` | function-app or api | `ApiUri` | `#Microsoft.Skills.Custom.WebApiSkill` |
| `cognitiveServices.subdomainUrl` | ai-services | `AiServicesSubdomain` | — |
| `cognitiveServices.identity` | identity | `UserAssignedIdentity` | — |
| `knowledgeStore.storageConnectionString` | storage | `StorageResourceId` | — |
| `knowledgeStore.identity` | identity | `UserAssignedIdentity` | — |
| `encryptionKey.keyVaultUri` | key-vault | `KeyVaultUri` | — |
| `encryptionKey.identity` | identity | `UserAssignedIdentity` | — |

### indexers

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `encryptionKey.keyVaultUri` | key-vault | `KeyVaultUri` | — |
| `encryptionKey.identity` | identity | `UserAssignedIdentity` | — |

### knowledge-sources

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `azureBlobParameters.connectionString` | storage | `StorageResourceId` | — |
| `azureBlobParameters.ingestionParameters.identity` | identity | `UserAssignedIdentity` | — |
| `azureBlobParameters.ingestionParameters.embeddingModel.azureOpenAIParameters.resourceUri` | ai-services | `OpenAiEndpoint` | — |
| `azureBlobParameters.ingestionParameters.embeddingModel.azureOpenAIParameters.authIdentity` | identity | `UserAssignedIdentity` | — |
| `azureBlobParameters.ingestionParameters.chatCompletionModel.azureOpenAIParameters.resourceUri` | ai-services | `OpenAiEndpoint` | — |
| `azureBlobParameters.ingestionParameters.chatCompletionModel.azureOpenAIParameters.authIdentity` | identity | `UserAssignedIdentity` | — |
| `azureBlobParameters.ingestionParameters.aiServices.uri` | ai-services | `AiServicesSubdomain` | — |
| `azureBlobParameters.ingestionParameters.assetStore.connectionString` | storage | `StorageResourceId` | — |
| `encryptionKey.keyVaultUri` | key-vault | `KeyVaultUri` | — |
| `encryptionKey.identity` | identity | `UserAssignedIdentity` | — |

### knowledge-bases

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `models[].azureOpenAIParameters.resourceUri` | ai-services | `OpenAiEndpoint` | — |
| `models[].azureOpenAIParameters.authIdentity` | identity | `UserAssignedIdentity` | — |
| `encryptionKey.keyVaultUri` | key-vault | `KeyVaultUri` | — |
| `encryptionKey.identity` | identity | `UserAssignedIdentity` | — |

### agents

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `tools[].server_url` | search, ai-services, function-app or api | `Endpoint` | — |

### connections

| Path | Binding type | Form | Only for `@odata.type` |
|---|---|---|---|
| `properties.target` | search, ai-services, function-app or api | `Endpoint` | — |
<!-- generated:infra-table:end -->

### How a binding gets used

When rigg sees one of the fields above, it parses the value into the "form"
named in the table — a storage `ResourceId=` connection string, an Azure
OpenAI endpoint, a key vault URI, a plain endpoint — and looks for a binding
of the matching type whose resolved resource is the one named. What it finds
decides what happens:

| Outcome | `strict-bindings: false` | `strict-bindings: true` |
|---|---|---|
| Bound in this environment | fine | fine |
| Bound in a *different* environment only | error (a leak) | error |
| Bound nowhere, an Azure resource | warning | error |
| Bound nowhere, an external URL | warning | error |

`rigg validate` and `rigg push`'s preflight run the same machinery and print
the same messages:

```
! [projects/contoso-docs/envs/dev/search/data-sources/contoso-docs.json] credentials.connectionString references storage 'contosostorage', which no environment binds — run `rigg env bind dev --learn` to record it
```

`rigg env bind <env> --learn` is the fast way out: it walks these very fields,
proposes a binding name for each unbound reference, and writes the ones you
accept into `rigg.yaml`.

`rigg promote --from dev --to prod` uses the same table in the other
direction: for each field it finds the source environment's binding, looks up
the *target's* binding of the same name, and rewrites the value into the
target's world — a different storage account, a different function app, a
different vault. That is why an unbound infrastructure reference blocks a
promote into a strict environment: rigg has no name to translate through.

## Common mistakes

**Keeping `@odata.etag` (or any volatile field) in the file.** They are
stripped on pull, so a hand-added one only ever causes a diff. If a file has
them, it was hand-written or copied from a portal export — one `rigg pull`
cleans it up.

**A resource file in the wrong directory.** `search/` vs. `foundry/` and the
exact kind directory are how rigg knows what a file *is*. A knowledge base in
`search/knowledge-sources/` is not a misconfigured knowledge base; it is an
invalid knowledge source. The directory names are in [the table
above](#the-twelve-kinds) and are always kebab-case.

**A file that is not `.json`.** Only `.json` files are resources. Sidecars are
Markdown and are found through `$file`, not by scanning.

**Renaming the file to rename the resource.** The stem is the logical id;
Azure's name is the `name` field. Changing the stem re-correlates the file
across environments and orphans its baseline; changing `name` renames the
resource in Azure (which, for most kinds, means create-new and prune-old).
Decide which one you meant.

**Pasting a portal connection string.** It carries `AccountKey=` and is
rejected. Use the `ResourceId=` form and grant the identity a role —
`rigg auth doctor --fix` will offer to grant it.

**Expecting an indexer file to show run history.** It never does; that is
`rigg az indexer status`.

## See also

- [project.yaml](project-yaml.md) — the tree these files live in, and exclusive ownership.
- [Annotations](annotations.md) — `x-rigg-api`, `x-rigg-auth`, `x-rigg-pin`, `x-rigg-ref`.
- [rigg.yaml § Dependencies](rigg-yaml.md#dependencies) — the bindings the infrastructure fields resolve through.
- [State](state.md) — baselines, and why a stripped field never shows as drift.
- [`CONCEPTS.md`](../../CONCEPTS.md) — logical identity vs. physical name, validation classes.
- [CLI reference](cli.md#rigg-new) — `rigg new`, `rigg validate`, `rigg az indexer status`.
