# rigg 2.0 — API refresh, provider table and currency

**Date:** 2026-09-09
**Status:** Design, approved direction. Workstream 0 of
`2026-09-09-rigg-2.0-scope-and-principles-design.md`. Executed first, and
includes the scope reduction (removal of Cosmos/SQL/other data-source
types) so the surface is small before it is upgraded.

## 1. Problem

- Only three API versions are registry constants; every other version is a
  string literal in `arm.rs`, `doctor.rs`, `push.rs`, `arm_resources.rs`.
  The watchdog cannot see them, and one provider (Microsoft.CognitiveServices)
  is called with three different versions.
- Several versions are years behind (Search ARM 2023-11-01, Storage
  2023-05-01, Web 2023-12-01); the Search preview and CognitiveServices ARM
  pins are two releases behind, and knowledge bases live on the preview.
- Version comparison says *that* something changed, never *what*.

State on 2026-09-09 (from the azure-rest-api-specs repository):

| API | rigg 1.7.0 | Newest | 2.0 target |
|---|---|---|---|
| Azure AI Search data plane, stable | 2026-04-01 | 2026-04-01 | 2026-04-01 |
| Azure AI Search data plane, preview | 2026-05-01-preview | 2026-08-01-preview | 2026-08-01-preview |
| Microsoft.CognitiveServices ARM | 2026-05-01 (+2024-10-01, 2025-05-15-preview literals) | 2026-07-01 | 2026-07-01 everywhere |
| Microsoft.Search ARM | 2023-11-01 | 2025-05-01 | 2025-05-01 |
| Microsoft.Storage ARM | 2023-05-01 | 2026-06-01 | 2026-06-01 |
| Microsoft.Web ARM | 2023-12-01 | 2026-07-15 | 2026-07-15 |
| Microsoft.Authorization role assignments / permissions | 2022-04-01 | 2022-04-01 | 2022-04-01 |
| Microsoft.Resources subscriptions / tenants | 2022-12-01 | 2022-12-01 | 2022-12-01 |
| Microsoft.ManagedIdentity ARM | — | 2024-11-30 | 2024-11-30 |
| Microsoft.KeyVault ARM | — | 2026-02-01 | 2026-02-01 |
| Key Vault data plane (secrets) | — | 2025-07-01 (stable) | 2025-07-01 |
| Microsoft Foundry data plane | v1 | v1 | v1 |
| Microsoft Graph | — | v1.0 | v1.0 |

## 2. Provider table (registry)

```rust
pub enum Provider {
    SearchData,          // https://{svc}.search.windows.net
    FoundryData,         // https://{acct}.services.ai.azure.com/api/projects/{p}
    CognitiveServicesArm, SearchArm, StorageArm, WebArm,
    AuthorizationArm, ResourcesArm, ManagedIdentityArm, KeyVaultArm,
    KeyVaultData,        // https://{vault}.vault.azure.net
    Graph,               // https://graph.microsoft.com/v1.0
}

pub struct ProviderMeta {
    pub provider: Provider,
    pub stable: &'static str,            // api-version
    pub preview: Option<&'static str>,   // when rigg uses one
    pub audience: &'static str,          // token scope base
    pub spec_path: Option<&'static str>, // azure-rest-api-specs folder for the watchdog
}
pub fn provider(p: Provider) -> &'static ProviderMeta;
```

Every client builds its URLs through `provider(p).stable` /
`.preview`; no version literal survives outside `registry.rs` (a test greps
the source tree for `api-version=20` outside the registry and fails on any
hit). `rigg.yaml` per-environment overrides (`api-version`,
`preview-api-version` on the Search target; `api-version` on Foundry) are
kept for sovereign clouds and testing.

Kinds keep their `channel` (stable/preview). Knowledge bases stay on
preview (retrieval and output configuration exist only there); the preview
version is now 2026-08-01-preview. The registry's per-kind `volatile`,
`read_only`, `secret`, `write_only`, `reference` and (new) `infra`
tables are re-verified against the refreshed schemas (§4).

## 3. Watchdog over the whole table

`rigg dev api-check` iterates the provider table: for each entry with a
`spec_path`, list the version folders in `Azure/azure-rest-api-specs` and
compare against `stable` (and `preview`). Output stays the one-line-per-API
table; exit 1 when anything is behind. The weekly GitHub Action is unchanged
in shape and now covers all providers. Route-versioned APIs (Foundry `v1`,
Graph `v1.0`) are listed as informational.

## 4. Knowing *what* changed

Two mechanisms, both cheap:

1. **`rigg dev api-diff <provider> [--from v] [--to v]`** — downloads the
   OpenAPI documents for two versions of a provider from the specs
   repository and prints, for the definitions rigg's kinds map to (a small
   static list per provider: e.g. `SearchIndexerDataSource`,
   `SearchIndexerSkillset`, `KnowledgeBase`, `KnowledgeSource`, `Account`,
   `Project`, `Deployment`, `ConnectionPropertiesV2`, `RaiPolicy`,
   `StorageAccount`, `Site`, `SiteAuthSettingsV2`), the added, removed and
   type-changed properties, plus new/removed enum values (e.g. new
   `@odata.type` skills, new data-source types). This is how the maintainer
   decides what to support when the watchdog fires.
2. **Unknown-field canary** — on `pull` and `adopt`, after normalization,
   rigg compares each document's keys (recursively, per known `@odata.type`
   or kind) against the schema fixture captured for the pinned version
   (`crates/rigg-core/fixtures/schema/<provider>-<version>.json`, a trimmed
   extraction of property names produced by `rigg dev api-fixture`). Keys
   not in the fixture are reported once per run: "`knowledge-bases/kb`: field
   `retrievalMode` not in rigg's 2026-08-01-preview schema — Azure may have
   shipped a newer API; run `rigg dev api-check`". Never an error; never
   strips anything (documents remain pass-through).

## 5. Scope reduction (done in this workstream)

Delete: `rigg-client/src/cosmos.rs` and its dependencies (`hmac`, `sha2`,
`base64` if unused after); the Cosmos knowledge-source wizard
(`2026-05-08-cosmos-ks-wizard-design.md` is superseded); scaffold arms for
`cosmosdb`, `azuresql`, `onelake`, `sharepoint`, `mysql`, `azuretable`,
`azurefile(s)`; `valid_datasource_types` becomes `["azureblob", "adlsgen2"]`
on both channels; identity edges for Cosmos/SQL; docs and samples that
mention them (`samples/projects/cosmos-sql-patterns` removed). Validation
rejects other `type` values with a message naming the two supported ones.

## 6. Changes to absorb per API (from the 2026-09-09 research report)

Verified by diffing the OpenAPI documents in `Azure/azure-rest-api-specs`
and checking ARM registration with `az provider show`.

### 6.1 Azure AI Search data plane → 2026-08-01-preview (stable stays 2026-04-01)

- **List paging (breaking).** `$top/$skip/$count` are gone; list results
  carry `@odata.nextLink` and may page. The Search client follows
  `@odata.nextLink` verbatim for every list operation (data sources,
  indexers, indexes, skillsets, synonym maps, aliases, knowledge sources,
  knowledge bases). Wiremock tests cover a two-page listing.
- **Knowledge base (additive).** `retrieveDefaults { maxRuntimeInSeconds,
  maxOutputDocuments, maxOutputSizeInTokens }`, `tags`, reasoning effort
  `kind: auto`. Pass-through; nothing in the registry changes except the
  schema fixture.
- **Knowledge source.** `resultsProcessing` (`rerank`|`none`) on the base;
  `queryHints` on index-backed kinds; `ingestionParameters.networkAccessMode`
  is **create-time only** → added to `immutable_fields` for KnowledgeSource
  so a differing local value shows `replace`. `McpServerTool.inclusionMode` →
  `resultsProcessing` and WorkIQ `entraAppAuthentication` are outside rigg's
  supported kinds (knowledge sources of those kinds are neither scaffolded
  nor migrated); pull still round-trips them as opaque documents.
- **Identity shapes: unchanged.** `DataUserAssignedIdentity` gains a
  preview-only `federatedIdentityClientId` (pass-through).
- **Retrieve response.** Activity records use `model { modelName,
  deploymentId }` instead of `modelName`; `rigg az kb ask` rendering accepts
  both.
- **MCP endpoint.** Not in the swagger; documentation now prescribes
  `?api-version=2026-08-01-preview` on `/knowledgebases/<kb>/mcp`. The
  `SearchKbMcpUrl` renderer, the agent scaffold and the registry sample move
  to it (1.x had `2025-11-01-Preview` in a sample and `2025-11-01-preview` as
  a default in `config.rs`).
- Data-source types: the enum still lists eight; rigg accepts `azureblob`
  and `adlsgen2` (scope spec).

### 6.2 Microsoft.CognitiveServices ARM → 2026-07-01

No change for accounts, projects, deployments, connections, RAI policies or
identity. All literals (`2024-10-01`, `2025-05-15-preview`, `2025-06-01`)
collapse to the provider table entry. Note: the Foundry IQ documentation's
connection example carries a `properties.audience` field that is absent
from the swagger; rigg sends the connection body as the user's file has it
(pass-through) and the live test in workstream 3 records whether ARM accepts
it on 2026-07-01.

### 6.3 Microsoft.Search ARM → 2025-05-01

- **Enum casing (breaking):** `publicNetworkAccess` is `Enabled | Disabled |
  SecuredByPerimeter`, `hostingMode` is `Default | HighDensity`; compare
  case-insensitively.
- Identity supports `UserAssigned` and `SystemAssigned, UserAssigned` with
  `userAssignedIdentities`; PATCH `SearchServiceUpdate` enables/attaches.
- `networkRuleSet.bypass` (`None | AzureServices`), `ipRules[]`; new
  `properties.endpoint`.
- `sharedPrivateLinkResources` list: `properties { privateLinkResourceId,
  groupId, requestMessage, status: Pending|Approved|Rejected|Disconnected,
  provisioningState }` — used by the auth spec's network checks.

### 6.4 Microsoft.Storage ARM → 2026-06-01

No change to the shapes rigg reads (`networkAcls { bypass, defaultAction,
ipRules, virtualNetworkRules, resourceAccessRules }`, `publicNetworkAccess`,
`allowSharedKeyAccess`, `isHnsEnabled`, `blobServices/default
{ deleteRetentionPolicy, isVersioningEnabled }`, containers). New
`allowSharedKeyAccessForServices { blob, … }` is read by the auth doctor as
well. `listKeys` is removed from rigg.

### 6.5 Microsoft.Web ARM → 2026-07-15

No change to sites, `functions/{f}/listkeys`, `host/default/listkeys`,
`config/authsettingsV2` (PUT and `/list`). `siteConfig` is not returned by
`GET sites/{name}`: access restrictions are read from
`GET sites/{name}/config/web` (`ipSecurityRestrictions`,
`ipSecurityRestrictionsDefaultAction`, `publicNetworkAccess`).

### 6.6 New providers

- Microsoft.ManagedIdentity 2024-11-30: `PUT userAssignedIdentities/{name}
  { location, tags }` → `properties { principalId, clientId, tenantId }`.
- Microsoft.KeyVault ARM 2026-02-01: vault by URI = list vaults in the
  subscription and match `name` / `properties.vaultUri` (the resource group
  is not in the URI). ARM already serves 2026-05-15 without a public spec;
  2026-02-01 is used.
- Key Vault secrets data plane 2025-07-01: `GET /secrets/{name}/` →
  `SecretBundle { value, … }`, scope `https://vault.azure.net/.default`.
- Microsoft Graph v1.0: `POST /applications` (then `PATCH` `identifierUris:
  ["api://<appId>"]`), `POST /servicePrincipals { appId }`, `PATCH
  /servicePrincipals/{id} { appRoleAssignmentRequired }`, `POST
  /servicePrincipals/{resourceSp}/appRoleAssignedTo`. rigg defines an explicit
  app role (`allowedMemberTypes: ["Application"]`) on the application
  instead of relying on the all-zeros default. The Azure CLI Graph token
  carries `Application.ReadWrite.All` and `AppRoleAssignment.ReadWrite.All`;
  effective rights are bounded by the user's directory role.

### 6.7 Microsoft Foundry data plane v1

Unchanged: agents, versions, the `mcp` tool shape (`server_url`,
`project_connection_id`, `allowed_tools`, `require_approval`, `headers`).
Additive `agents:import` and `versions:import` are not used.

## 7. Testing

- Registry: provider table completeness (every `Provider` has an entry);
  source-tree grep test for stray `api-version=` literals; per-kind
  schema-path test (`InfraRef`, `RefField`, `secret_fields`, etc. paths exist
  in the pinned fixture).
- `api-check`: wiremock-free test against a recorded GitHub contents
  listing fixture; `api-diff` against two local fixture documents;
  unknown-field canary on a document with an extra key.
- All existing wiremock sync tests updated to the new versions (query
  strings asserted).
- Live smoke (Kristofer's subscription): `rigg pull` and `rigg status` on
  `e2e-test` with the new versions; a temporary knowledge base created and
  adopted on 2026-08-01-preview and deleted.

## 8. Release note

CHANGELOG `[2.0.0]` opens with the version table above and the removed
data-source types.
