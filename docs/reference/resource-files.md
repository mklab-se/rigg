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
