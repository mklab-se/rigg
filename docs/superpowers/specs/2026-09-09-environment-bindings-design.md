# rigg 2.0 — Environments and infrastructure bindings

**Date:** 2026-09-09
**Status:** Design, approved direction. Workstream 1 of
`2026-09-09-rigg-2.0-scope-and-principles-design.md`.
**Supersedes:** the environment model of `2026-07-10-environments-design.md`
(per-env trees, logical vs physical identity and protected envs are kept;
the value-level pin machinery is replaced).

## 1. Problem

An environment in rigg 1.x is a Search service plus a Foundry project. The
infrastructure those services reach — storage accounts, model hosts, AI
services accounts, function apps, identities, key vaults — exists only as
opaque strings inside resource files. Consequences:

- `promote` cannot know what should differ between environments; it keeps
  whatever the target file already had and copies the source's infrastructure
  verbatim into files that are new in the target.
- `validate` cannot tell that a `prod` file points at `dev`'s storage.
- `auth doctor` derives scopes by parsing strings and falls back to
  environment defaults, so it grants roles on the wrong resource.
- Scaffolds emit `<subscription-id>` placeholders the user must hand-edit.

## 2. Model

### 2.1 Environment = targets + dependencies + policy

```yaml
# rigg.yaml (2.0)
name: my-rag
environments:
  dev:
    default: true
    tenant: 72f988bf-86f1-41af-91ab-2d7cd011db47        # optional: az login's default tenant
    subscription: fa354123-c4ee-4b2e-a700-bf01decf803a  # optional: discovery scope; recommended
    search:  { service: mklabsrch }
    foundry: { account: mklabaifndr, project: proj-default }
    policy:  { protected: false }
    dependencies:
      docs-storage: { storage: mklabstorageacc }
      enrichment:   { ai-services: mklabaisrvc }
      enrich-fn:    { function-app: mklab-enrich-fn }
      pipeline-mi:  { identity: rigg-dev-mi }
      cmk:          { key-vault: mklab-kv }
      partner-api:  { api: https://api.partner.example }
  prod:
    tenant: 9a3c…                                       # a different tenant is allowed
    subscription: 0b1d…
    search:  { service: mklabsrch-prod }
    foundry: { account: mklabaifndr-prod, project: proj-prod }
    policy:  { protected: true }
    dependencies:
      docs-storage: { storage: mklabstorageacc }          # same physical value ⇒ shared
      enrichment:   { ai-services: mklabaisrvc-prod }
      enrich-fn:    { function-app: mklab-enrich-fn }     # shared
      pipeline-mi:  { identity: rigg-prod-mi }
      cmk:          { key-vault: mklab-kv-prod }
      partner-api:  { api: https://api.partner.example }
```

- **Targets** — `search` and `foundry`. Exactly one of each per environment
  (either may be absent when a project only uses one service). The 1.x
  multi-connection lists and `project.yaml` connection pins are removed: one
  Search service and one Foundry project per environment is the supported
  configuration. Two Search services are two environments.
- **Dependencies** — a map of *binding name* → `{ <type>: <value> }`. Binding
  names correlate across environments exactly as file paths correlate
  resources: the same name in `dev` and `prod` is the same *role* played by
  possibly different physical resources. Physical values are free. "Shared"
  is simply the same physical value in both environments — explicit, never
  inferred.
- **Implicit bindings** — every environment also has `search` (its Search
  service) and `foundry` (its Foundry account), usable wherever a binding of
  type `ai-services` (model host) or the Search endpoint is expected. Most
  workspaces therefore need no `ai-services` binding at all: the Foundry
  account that hosts the project also hosts the models.
- **Tenant / subscription** — optional. When present, ARM discovery and
  token acquisition are scoped to them; when absent, rigg uses the Azure CLI
  default tenant and searches all subscriptions visible in it. Environments in
  different subscriptions or tenants are fully supported.
- **Policy** — `protected` (unchanged from 1.x) and `strict-bindings`
  (see §4; defaults to the value of `protected`).

### 2.2 Binding types and values

| Type | Value forms accepted | Resolves to (ARM) |
|---|---|---|
| `storage` | account name, or full ARM id | `Microsoft.Storage/storageAccounts/{name}` |
| `ai-services` | account name, or full ARM id | `Microsoft.CognitiveServices/accounts/{name}` (any kind; identity billing requires kind `AIServices`, checked by doctor) |
| `function-app` | site name, or full ARM id | `Microsoft.Web/sites/{name}` |
| `identity` | user-assigned identity name, or full ARM id | `Microsoft.ManagedIdentity/userAssignedIdentities/{name}` |
| `key-vault` | vault name, or full ARM id | `Microsoft.KeyVault/vaults/{name}` |
| `api` | base URL (`https://host[/path]`) | none — an external REST API; matched by URL prefix; no ARM lookup |

A name is resolved through ARM in the environment's subscription (or all
visible subscriptions when none is declared). Ambiguity (two matches) is an
error that names both ids and asks the user to write the full id. Resolution
results are cached in `.rigg/<env>/bindings.json` (gitignored) with the
resolved id, subscription, resource group, region and the timestamp;
`rigg env show --refresh` re-resolves. Offline commands (`validate`) use the
cache when present and raw names otherwise.

### 2.3 Infrastructure reference fields (registry)

The registry gains, per kind, a table of **infrastructure reference fields**:
JSON paths whose value names infrastructure, and the *form* the value takes.
This replaces `env_pinned_extra` and the secret/write-only pin union.

```rust
pub struct InfraRef { pub path: &'static str, pub form: InfraForm }

pub enum InfraForm {
    /// `ResourceId=/subscriptions/…/storageAccounts/X;` → storage
    StorageResourceId,
    /// `{ "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
    ///    "userAssignedIdentity": "/subscriptions/…/userAssignedIdentities/X" }` → identity
    /// (the path addresses the object; null means "system-assigned")
    UserAssignedIdentity,
    /// `https://X.openai.azure.com`, `https://X.cognitiveservices.azure.com`,
    /// `https://X.services.ai.azure.com` → ai-services or the implicit `foundry`
    OpenAiEndpoint,
    /// `https://X.cognitiveservices.azure.com/` on AIServicesByIdentity → ai-services
    AiServicesSubdomain,
    /// `https://X.azurewebsites.net/api/…` → function-app; any other URL → api (prefix match)
    ApiUri,
    /// `https://X.vault.azure.net/…` → key-vault
    KeyVaultUri,
    /// `https://X.search.windows.net/knowledgebases/<kb>/mcp?…` → implicit `search`
    /// + a sibling reference to knowledge-bases/<kb>
    SearchKbMcpUrl,
}
```

The complete table for 2.0 (paths use the registry's `a.b[].c` syntax):

| Kind | Path | Form |
|---|---|---|
| DataSource | `credentials.connectionString` | StorageResourceId |
| DataSource | `identity` | UserAssignedIdentity |
| DataSource | `encryptionKey.keyVaultUri` | KeyVaultUri |
| Index | `vectorSearch.vectorizers[].azureOpenAIParameters.resourceUri` | OpenAiEndpoint |
| Index | `vectorSearch.vectorizers[].azureOpenAIParameters.authIdentity` | UserAssignedIdentity |
| Index | `encryptionKey.keyVaultUri` | KeyVaultUri |
| Skillset | `skills[].resourceUri` (AzureOpenAIEmbeddingSkill) | OpenAiEndpoint |
| Skillset | `skills[].authIdentity` | UserAssignedIdentity |
| Skillset | `skills[].uri` (WebApiSkill) | ApiUri |
| Skillset | `cognitiveServices.subdomainUrl` | AiServicesSubdomain |
| Skillset | `cognitiveServices.identity` | UserAssignedIdentity |
| Skillset | `knowledgeStore.storageConnectionString` | StorageResourceId |
| Skillset | `knowledgeStore.identity` | UserAssignedIdentity |
| Skillset | `encryptionKey.keyVaultUri` | KeyVaultUri |
| Indexer | `cache.storageConnectionString` | StorageResourceId |
| Indexer | `cache.identity` | UserAssignedIdentity |
| Indexer | `encryptionKey.keyVaultUri` | KeyVaultUri |
| KnowledgeSource | `azureBlobParameters.connectionString` | StorageResourceId |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.identity` | UserAssignedIdentity |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.embeddingModel.azureOpenAIParameters.resourceUri` | OpenAiEndpoint |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.embeddingModel.azureOpenAIParameters.authIdentity` | UserAssignedIdentity |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.chatCompletionModel.azureOpenAIParameters.resourceUri` | OpenAiEndpoint |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.chatCompletionModel.azureOpenAIParameters.authIdentity` | UserAssignedIdentity |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.aiServices.uri` | AiServicesSubdomain (secret sibling `apiKey` rejected) |
| KnowledgeSource | `azureBlobParameters.ingestionParameters.assetStore.connectionString` | StorageResourceId |
| KnowledgeSource | `encryptionKey.keyVaultUri` | KeyVaultUri |
| KnowledgeBase | `models[].azureOpenAIParameters.resourceUri` | OpenAiEndpoint |
| KnowledgeBase | `models[].azureOpenAIParameters.authIdentity` | UserAssignedIdentity |
| KnowledgeBase | `encryptionKey.keyVaultUri` | KeyVaultUri |
| Agent | `tools[].server_url` | SearchKbMcpUrl |
| Connection | `properties.target` | SearchKbMcpUrl |

Paths were checked against the 2026-08-01-preview Search schema on
2026-09-09 (`SearchIndexerDataSource.identity`, `SearchIndexerKnowledgeStore.identity`,
`SearchIndexerCache.identity`, `KnowledgeSourceIngestionParameters.{identity,
embeddingModel, chatCompletionModel, aiServices, assetStore}`,
`KnowledgeBaseAzureOpenAIModel.azureOpenAIParameters`, `WebApiSkill.{uri,
authResourceId, authIdentity}`, `AIServicesAccountIdentity.{subdomainUrl,
identity}` all exist). Deployments, guardrails, synonym maps and aliases have
no infrastructure references. (Deployments have per-environment *constraints* — region model
availability, quota, capacity — handled in the promote spec.) Workstream 0
re-verifies every path against the refreshed API schemas before this table is
frozen.

Derived, per-environment fields that are **not** bindings but follow from
one: a WebApiSkill's `authResourceId`, `httpHeaders.x-functions-key` and
`x-rigg-auth` annotation follow from the function-app binding's Easy Auth
state (identity & auth spec). The promote spec says how they are re-derived.

### 2.4 Core API (rigg-core)

- `workspace::Environment { default, tenant, subscription, search: Option<SearchTarget>, foundry: Option<FoundryTarget>, policy, dependencies: BTreeMap<String, Binding> }`.
- `binding::{Binding, BindingType, ResolvedBinding, BindingCache}`.
- `registry::infra_refs(kind) -> &[InfraRef]`; `infra::parse(form, value) -> Option<PhysicalRef>`; `infra::render(form, &ResolvedBinding, original_value) -> Value` (keeps everything in the original that is not the infrastructure part — e.g. the path and query of a function URI, the `Database=` tail of a connection string).
- `infra::extract(kind, doc) -> Vec<(path, PhysicalRef)>` and `infra::classify(env, refs) -> Vec<Classified>` with variants `Bound(name)`, `Shared(name)`, `LeakFrom(other_env, name)`, `Unbound`, `External`.
- `Store` and baselines are unchanged.

## 3. Discovery and learning (configuration first *and* discovery first)

Both entry points must be effortless:

- **Configuration first.** Write `dependencies:` by hand (or let an AI write
  it); rigg validates files against it and resolves names via ARM on first
  use.
- **Discovery first.** `rigg env bind <env> --learn` scans the environment's
  tree, extracts every infrastructure reference, groups by (type, physical
  resource), proposes a binding name per group (the resource name, lower-
  kebab-cased; the user can rename), shows the table, and writes the bindings
  to `rigg.yaml` on confirmation. `adopt` and `pull` run the same scan after
  writing files and, interactively, offer the learn step when new unbound
  references appeared; non-interactively they print the hint.
- **New environment from an existing one.** `rigg env add prod --like dev`
  walks `dev`'s bindings and, per binding, asks: same as `dev` (value shown),
  pick another (ARM list of that type in the chosen subscription), or skip.
  Targets are asked first (tenant, subscription, Search service, Foundry
  project — ARM pick-lists, same as `init`), then dependencies, then
  `protected`. Flag form: `--search-service`, `--foundry-account`,
  `--foundry-project`, `--tenant`, `--subscription`, `--bind name=type:value`
  (repeatable), `--like <env>` with `--same name` / `--skip name`.
- `rigg env bind <env> <name> <type>:<value>` and `rigg env unbind <env>
  <name>` for single edits. `rigg env show <env>` prints targets, policy and
  every binding with its resolved ARM id, region, and which other
  environments share the same physical resource. `rigg describe` gains an
  infrastructure section per environment.

`rigg.yaml` is re-serialized by these commands; a header comment is
regenerated, other comments are not preserved (as in 1.x).

## 4. Validation rules

`rigg validate` (offline, all environments) classifies every infrastructure
reference in every file:

| Class | Meaning | Severity |
|---|---|---|
| Bound | matches a binding of this environment (or an implicit one) | ok |
| Shared | bound here and also in other environments with the same value | ok (listed in `--verbose`) |
| Leak | bound in **another** environment, not in this one | **error** — the file points at another environment's infrastructure |
| Unbound | matches no binding anywhere | warning; **error** when `policy.strict-bindings: true` (default `true` when `protected`) |
| External | `api` form with no matching `api` binding | warning (same strictness as Unbound) |

Errors exit 3 and name the file, the path, the physical value, and the
environments that bind it, followed by the hint (`rigg env bind <env> --learn`
or `rigg env bind <env> <name> storage:<value>`). Push runs the same
classification on its plan as a preflight and refuses on error before any
mutation. Existing 1.x validation (no secrets; identity-based forms only;
references resolve; sidecars; API links) is kept.

## 5. Interaction with other workstreams

- **Promote** (workstream 2) translates by binding name.
- **Identity & auth** (workstream 3) derives scopes and roles from resolved
  bindings, and reads network and identity facts from them.
- **Interaction model** (workstream 4) supplies the wizards (`env add`,
  learn, pick-lists) and the non-interactive question protocol.
- **API refresh** (workstream 0) supplies the ARM provider table used for
  resolution.

## 6. Removed from 1.x

- `ConnectionList` (multiple Search/Foundry connections per environment) and
  the `search-connection`/`foundry-connection` pins in `project.yaml`.
- `registry::env_pinned`, `env_pinned_extra`, and the `restore_path`-based
  promote merge (replaced in workstream 2; `x-rigg-pin` survives as the
  user's escape hatch for content that must stay per-environment).
- `defaults.identity` in `rigg.yaml` (replaced by `identity` bindings).

## 7. Testing

- rigg-core unit tests: parse/render round-trips for every `InfraForm`
  (including the non-infrastructure tail preservation), `classify` for all
  five classes, binding name/ARM-id resolution with a fake resolver, cache
  read/write, `Environment` YAML round-trip incl. tenant/subscription.
- Registry test: every `InfraRef` path exists in the refreshed API schema
  fixtures for its kind (fixtures captured in workstream 0).
- CLI tests (assert_cmd, no network): `validate` leak error, unbound warning
  vs strict error, `env bind --learn` proposals from a sample tree, `env add`
  flag form writes the expected YAML, `env show` output.
- Wiremock ARM fake (introduced here, reused by workstream 3): name → id
  resolution across two subscriptions, ambiguity error.
- Live (Kristofer): `env bind dev --learn` on `e2e-test`, `env add staging
  --like dev`, `validate` on a deliberately mispointed prod file.

## 8. Open decisions

None blocking. One flagged choice: single Search/Foundry target per
environment (§2.1). Reverting to lists is possible later without breaking the
binding model.
