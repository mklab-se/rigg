# Workstream 0: API refresh, provider table, scope reduction — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every Azure API version rigg uses lives in one registry table at the newest version, the watchdog checks all of them, and the unsupported data-source types (Cosmos, SQL, OneLake, SharePoint, MySQL, Table, Files) are gone from the code.

**Architecture:** `rigg-core::registry` gains a `Provider` enum + `ProviderMeta` table; `rigg-client` builds every URL from it. The Search client follows `@odata.nextLink`. `rigg dev api-check` iterates the table; `rigg dev api-diff` and `rigg dev api-fixture` download OpenAPI documents from `Azure/azure-rest-api-specs`; a schema fixture per pinned version feeds an unknown-field canary in `pull`/`adopt`.

**Tech Stack:** Rust 2024 edition (MSRV 1.88), serde_json, reqwest 0.12, wiremock + assert_cmd for tests, clap 4.

**Spec:** `docs/superpowers/specs/2026-09-09-api-refresh-and-currency-design.md` (and §2 of `2026-09-09-rigg-2.0-scope-and-principles-design.md` for the scope reduction).

## Global Constraints

- Branch: `rigg-2`. Commit after every task with the trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Gate before every commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Target versions (copy verbatim): Search data plane stable `2026-04-01`, preview `2026-08-01-preview`; Microsoft.CognitiveServices `2026-07-01`; Microsoft.Search `2025-05-01`; Microsoft.Storage `2026-06-01`; Microsoft.Web `2026-07-15`; Microsoft.Authorization `2022-04-01`; Microsoft.Resources `2022-12-01`; Microsoft.ManagedIdentity `2024-11-30`; Microsoft.KeyVault ARM `2026-02-01`; Key Vault secrets `2025-07-01`; Foundry `v1`; Graph `v1.0`.
- No api-version literal outside `crates/rigg-core/src/registry.rs` (a test enforces it).
- Supported data-source types: `azureblob`, `adlsgen2` only.
- No secrets: the storage `listKeys` client functions are deleted, not kept.
- Documents stay pass-through; the canary only reports.

---

### Task 1: Remove unsupported data-source types and the Cosmos client

**Files:**
- Delete: `crates/rigg-client/src/cosmos.rs`, `samples/projects/cosmos-sql-patterns/` (whole directory), `crates/rigg-core/src/config.rs`
- Modify: `crates/rigg-client/src/lib.rs`, `crates/rigg-client/Cargo.toml`, `crates/rigg-client/src/auth.rs:129-135` and its test near line 645, `crates/rigg-client/src/client.rs:9,71-84`, `crates/rigg-client/src/foundry.rs:12,56-70`, `crates/rigg-core/src/lib.rs:9,23-26`, `crates/rigg-core/src/registry.rs:385-420` and tests ~1050-1060, `crates/rigg-core/src/scaffold.rs:35-113` and tests ~421-432,478, `crates/rigg-core/src/identity.rs:295-340` and test `cosmos_and_sql_are_informational`, `crates/rigg/src/commands/validate.rs:236-260`, `crates/rigg/tests/cli_surface.rs:240-275`, `samples/README.md`, `README.md:255`, `GETTING_STARTED.md:178`
- Test: registry/scaffold/identity inline tests; `crates/rigg/tests/cli_surface.rs`

**Interfaces:**
- Produces: `registry::valid_datasource_types(channel) -> &'static [&'static str]` returning `["azureblob", "adlsgen2"]` for both channels; `registry::preview_only_datasource_types` removed; `scaffold::check_datasource_type(t) -> Result<(), String>` (no warning variant any more).

- [ ] **Step 1: Write the failing registry and scaffold tests**

In `crates/rigg-core/src/registry.rs` replace the existing `valid_datasource_types` test with:

```rust
#[test]
fn datasource_types_are_blob_only_on_both_channels() {
    assert_eq!(valid_datasource_types(Channel::Stable), &["azureblob", "adlsgen2"]);
    assert_eq!(valid_datasource_types(Channel::Preview), &["azureblob", "adlsgen2"]);
}
```

In `crates/rigg-core/src/scaffold.rs` replace `datasource_type_validation` with:

```rust
#[test]
fn datasource_type_validation() {
    assert!(check_datasource_type("azureblob").is_ok());
    assert!(check_datasource_type("adlsgen2").is_ok());
    let err = check_datasource_type("cosmosdb").unwrap_err();
    assert!(err.contains("azureblob, adlsgen2"), "{err}");
    assert!(scaffold(ResourceKind::DataSource, "x", Some("azuresql")).is_err());
}
```

and change the pipeline test at line ~478 from `scaffold_pipeline("p", "cosmosdb", false)` to `scaffold_pipeline("p", "adlsgen2", false)`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rigg-core datasource_type`
Expected: FAIL (`preview_only_datasource_types` still exists; cosmosdb still accepted).

- [ ] **Step 3: Cut the registry and scaffold down**

`registry.rs`: replace the body of `valid_datasource_types` with

```rust
pub fn valid_datasource_types(_channel: Channel) -> &'static [&'static str] {
    &["azureblob", "adlsgen2"]
}
```

and delete `preview_only_datasource_types` entirely. Fix the doc comment (Azure Files spelling note goes away).

`scaffold.rs`: `check_datasource_type` becomes

```rust
pub fn check_datasource_type(ds_type: &str) -> Result<(), String> {
    let valid = registry::valid_datasource_types(Channel::Stable);
    if valid.contains(&ds_type) {
        Ok(())
    } else {
        Err(format!(
            "unsupported data source type '{ds_type}' — rigg supports Azure Blob Storage only (valid: {})",
            valid.join(", ")
        ))
    }
}
```

`scaffold_datasource`: keep only the `"azureblob" | "adlsgen2"` arm for the connection string/container and the deletion policy; delete the `cosmosdb`, `azuresql`, `onelake` and `_` arms (the `check_datasource_type` call at the top makes them unreachable). Update `new.rs` and `new_pipeline` callers: `check_datasource_type(..)` no longer returns `Option<String>`; drop the `if let Some(warning)` blocks (lines ~103-108 and ~140-147 in `crates/rigg/src/commands/new.rs`) and just `?` the result.

- [ ] **Step 4: Remove Cosmos/SQL identity edges**

In `crates/rigg-core/src/identity.rs::datasource_edges` keep only the `"azureblob" | "adlsgen2"` arm; delete the `cosmosdb` and `azuresql` arms and the `cosmos_and_sql_are_informational` test. Keep `EdgeKind::Informational` (still used for Web API skills). Update the `EdgeKind::Informational` doc comment to drop the Cosmos/SQL mention.

- [ ] **Step 5: Remove the Cosmos client, legacy config and the crypto dependencies**

```bash
git rm crates/rigg-client/src/cosmos.rs crates/rigg-core/src/config.rs
git rm -r samples/projects/cosmos-sql-patterns
```

- `crates/rigg-client/src/lib.rs`: delete `pub mod cosmos;`.
- `crates/rigg-client/Cargo.toml`: delete the `hmac`, `sha2`, `base64` lines.
- `crates/rigg-client/src/auth.rs`: delete `for_cosmos()` and `test_for_cosmos_uses_cosmos_scope`.
- `crates/rigg-client/src/client.rs`: delete `use rigg_core::config::SearchServiceConfig;` and `from_service_config`.
- `crates/rigg-client/src/foundry.rs`: delete `use rigg_core::config::FoundryServiceConfig;` and the `new(config: &FoundryServiceConfig)` constructor (keep `from_connection` and the test constructor).
- `crates/rigg-core/src/lib.rs`: delete `pub mod config;` and the `pub use config::{…};` block; update the module doc bullet "Configuration management".
- `crates/rigg/src/commands/validate.rs::warn_missing_deletion_tracking`: delete the `"cosmosdb"` and `"azuresql"` arms; keep the blob arm.

- [ ] **Step 6: Update CLI tests, samples and docs**

`crates/rigg/tests/cli_surface.rs::new_datasource_type_validation` becomes:

```rust
#[test]
fn new_datasource_type_validation() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "data-source", "ds1", "-p", "demo", "--type", "adlsgen2"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["new", "data-source", "ds2", "-p", "demo", "--type", "cosmosdb"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("azureblob, adlsgen2"));
}
```

`samples/README.md`: delete the `cosmos-sql-patterns` table row and change "three projects" to "two projects" (two places). `README.md:255`: change `--type cosmosdb` to `--type adlsgen2`; `README.md:291` and `GETTING_STARTED.md:178`: drop the Cosmos mention and the count. `docs/superpowers/specs/2026-05-08-cosmos-ks-wizard-design.md` and `docs/superpowers/plans/2026-05-10-cosmos-ks-wizard-phase-1.md`: add a first line `**Superseded (2026-09-09): Cosmos DB support was removed in rigg 2.0 — see 2026-09-09-rigg-2.0-scope-and-principles-design.md.**`.

- [ ] **Step 7: Build, run the full gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all green; `grep -rn -i cosmos crates --include='*.rs'` prints nothing.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat!: blob storage is the only data source — remove Cosmos/SQL/OneLake/SharePoint/Table/Files support and the legacy config module

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Provider table in the registry

**Files:**
- Modify: `crates/rigg-core/src/registry.rs:16-22` (constants) + new `Provider` section after the `Channel` enum
- Test: inline in `registry.rs`; new `crates/rigg-core/tests/no_version_literals.rs`

**Interfaces:**
- Produces:
  ```rust
  pub enum Provider { SearchData, FoundryData, CognitiveServicesArm, SearchArm, StorageArm, WebArm, AuthorizationArm, ResourcesArm, ManagedIdentityArm, KeyVaultArm, KeyVaultData, Graph }
  pub struct ProviderMeta { pub provider: Provider, pub label: &'static str, pub stable: &'static str, pub preview: Option<&'static str>, pub audience: &'static str, pub spec_path: Option<&'static str>, pub route_versioned: bool }
  pub fn provider(p: Provider) -> &'static ProviderMeta;
  pub fn providers() -> &'static [ProviderMeta];
  ```
  and constants `SEARCH_STABLE_API_VERSION`, `SEARCH_PREVIEW_API_VERSION`, `FOUNDRY_API_VERSION`, `ARM_COGNITIVE_API_VERSION`, `ARM_SEARCH_API_VERSION`, `ARM_STORAGE_API_VERSION`, `ARM_WEB_API_VERSION`, `ARM_AUTHORIZATION_API_VERSION`, `ARM_RESOURCES_API_VERSION`, `ARM_MANAGED_IDENTITY_API_VERSION`, `ARM_KEYVAULT_API_VERSION`, `KEYVAULT_SECRETS_API_VERSION`, `GRAPH_API_VERSION`.

- [ ] **Step 1: Write the failing tests**

Inline in `registry.rs` tests module:

```rust
#[test]
fn provider_table_is_complete_and_current() {
    for p in [Provider::SearchData, Provider::FoundryData, Provider::CognitiveServicesArm,
              Provider::SearchArm, Provider::StorageArm, Provider::WebArm, Provider::AuthorizationArm,
              Provider::ResourcesArm, Provider::ManagedIdentityArm, Provider::KeyVaultArm,
              Provider::KeyVaultData, Provider::Graph] {
        let m = provider(p);
        assert_eq!(m.provider, p);
        assert!(!m.stable.is_empty());
        assert!(m.audience.starts_with("https://"));
    }
    assert_eq!(provider(Provider::SearchData).stable, "2026-04-01");
    assert_eq!(provider(Provider::SearchData).preview, Some("2026-08-01-preview"));
    assert_eq!(provider(Provider::CognitiveServicesArm).stable, "2026-07-01");
    assert_eq!(provider(Provider::SearchArm).stable, "2025-05-01");
    assert_eq!(provider(Provider::StorageArm).stable, "2026-06-01");
    assert_eq!(provider(Provider::WebArm).stable, "2026-07-15");
    assert_eq!(provider(Provider::KeyVaultData).stable, "2025-07-01");
    assert!(provider(Provider::FoundryData).route_versioned);
    assert!(provider(Provider::Graph).route_versioned);
    assert_eq!(providers().len(), 12);
}
```

New file `crates/rigg-core/tests/no_version_literals.rs`:

```rust
//! Guard: every Azure api-version lives in the registry provider table.
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() { walk(&p, out); } else if p.extension().is_some_and(|e| e == "rs") { out.push(p); }
    }
}

#[test]
fn no_api_version_literals_outside_registry() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join("crates");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let re = regex_lite::Regex::new(r#"api-version=20\d\d-\d\d-\d\d|"20\d\d-\d\d-\d\d(-preview)?"\s*(,|\)|;|$)"#).unwrap();
    let mut offenders = Vec::new();
    for f in files {
        if f.ends_with("registry.rs") { continue; }
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") { continue; }
            if re.is_match(line) { offenders.push(format!("{}:{}: {}", f.display(), i + 1, line.trim())); }
        }
    }
    assert!(offenders.is_empty(), "api-version literals outside registry.rs:\n{}", offenders.join("\n"));
}
```

Add `regex-lite = "0.1"` to `[dev-dependencies]` of `crates/rigg-core/Cargo.toml` (and to `[workspace.dependencies]`). Test files under `crates/*/tests` are included in the walk on purpose: wiremock tests must assert versions through the registry constants too (Task 4 fixes the two in `sync.rs`).

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rigg-core provider_table no_api_version`
Expected: compile error (`Provider` undefined); the literal guard lists `arm.rs`, `doctor.rs`, `push.rs`, `sync.rs`, `client.rs` tests.

- [ ] **Step 3: Add the table**

In `registry.rs`, replace the constants block (lines 16-22) with:

```rust
/// Azure AI Search data plane. Overridable per environment in `rigg.yaml`.
pub const SEARCH_STABLE_API_VERSION: &str = "2026-04-01";
pub const SEARCH_PREVIEW_API_VERSION: &str = "2026-08-01-preview";
/// Microsoft Foundry data plane (route-versioned).
pub const FOUNDRY_API_VERSION: &str = "v1";
/// ARM: Microsoft.CognitiveServices (accounts, projects, deployments, connections, RAI policies).
pub const ARM_COGNITIVE_API_VERSION: &str = "2026-07-01";
/// ARM: Microsoft.Search (search services, identity, network, shared private links).
pub const ARM_SEARCH_API_VERSION: &str = "2025-05-01";
/// ARM: Microsoft.Storage (accounts, blob services, containers).
pub const ARM_STORAGE_API_VERSION: &str = "2026-06-01";
/// ARM: Microsoft.Web (sites, function keys, auth settings, site config).
pub const ARM_WEB_API_VERSION: &str = "2026-07-15";
/// ARM: Microsoft.Authorization (role assignments, permissions).
pub const ARM_AUTHORIZATION_API_VERSION: &str = "2022-04-01";
/// ARM: Microsoft.Resources (subscriptions, tenants).
pub const ARM_RESOURCES_API_VERSION: &str = "2022-12-01";
/// ARM: Microsoft.ManagedIdentity (user-assigned identities).
pub const ARM_MANAGED_IDENTITY_API_VERSION: &str = "2024-11-30";
/// ARM: Microsoft.KeyVault (vaults).
pub const ARM_KEYVAULT_API_VERSION: &str = "2026-02-01";
/// Key Vault data plane (secrets).
pub const KEYVAULT_SECRETS_API_VERSION: &str = "2025-07-01";
/// Microsoft Graph (route-versioned).
pub const GRAPH_API_VERSION: &str = "v1.0";
pub const ARM_BASE_URL: &str = "https://management.azure.com";
pub const GRAPH_BASE_URL: &str = "https://graph.microsoft.com/v1.0";

/// Every remote API rigg talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Provider {
    SearchData, FoundryData, CognitiveServicesArm, SearchArm, StorageArm, WebArm,
    AuthorizationArm, ResourcesArm, ManagedIdentityArm, KeyVaultArm, KeyVaultData, Graph,
}

#[derive(Debug, Clone, Copy)]
pub struct ProviderMeta {
    pub provider: Provider,
    /// Human label for `rigg dev api-check`.
    pub label: &'static str,
    pub stable: &'static str,
    pub preview: Option<&'static str>,
    /// Token audience (scope base, without `/.default`).
    pub audience: &'static str,
    /// Folder in Azure/azure-rest-api-specs whose entries are version folders
    /// (`None` for route-versioned APIs and APIs not in that repository).
    pub spec_path: Option<&'static str>,
    /// Preview folder in the specs repository, when rigg uses a preview.
    pub preview_spec_path: Option<&'static str>,
    pub route_versioned: bool,
}

static PROVIDERS: &[ProviderMeta] = &[
    ProviderMeta { provider: Provider::SearchData, label: "Azure AI Search data plane", stable: SEARCH_STABLE_API_VERSION, preview: Some(SEARCH_PREVIEW_API_VERSION), audience: "https://search.azure.com", spec_path: Some("specification/search/data-plane/Search/stable"), preview_spec_path: Some("specification/search/data-plane/Search/preview"), route_versioned: false },
    ProviderMeta { provider: Provider::FoundryData, label: "Microsoft Foundry data plane", stable: FOUNDRY_API_VERSION, preview: None, audience: "https://ai.azure.com", spec_path: None, preview_spec_path: None, route_versioned: true },
    ProviderMeta { provider: Provider::CognitiveServicesArm, label: "Microsoft.CognitiveServices ARM", stable: ARM_COGNITIVE_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/cognitiveservices/resource-manager/Microsoft.CognitiveServices/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::SearchArm, label: "Microsoft.Search ARM", stable: ARM_SEARCH_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/search/resource-manager/Microsoft.Search/Search/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::StorageArm, label: "Microsoft.Storage ARM", stable: ARM_STORAGE_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/storage/resource-manager/Microsoft.Storage/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::WebArm, label: "Microsoft.Web ARM", stable: ARM_WEB_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/web/resource-manager/Microsoft.Web/AppService/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::AuthorizationArm, label: "Microsoft.Authorization ARM", stable: ARM_AUTHORIZATION_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/authorization/resource-manager/Microsoft.Authorization/Authorization/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::ResourcesArm, label: "Microsoft.Resources ARM", stable: ARM_RESOURCES_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/resources/resource-manager/Microsoft.Resources/subscriptions/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::ManagedIdentityArm, label: "Microsoft.ManagedIdentity ARM", stable: ARM_MANAGED_IDENTITY_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/msi/resource-manager/Microsoft.ManagedIdentity/ManagedIdentity/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::KeyVaultArm, label: "Microsoft.KeyVault ARM", stable: ARM_KEYVAULT_API_VERSION, preview: None, audience: "https://management.azure.com", spec_path: Some("specification/keyvault/resource-manager/Microsoft.KeyVault/KeyVault/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::KeyVaultData, label: "Key Vault data plane (secrets)", stable: KEYVAULT_SECRETS_API_VERSION, preview: None, audience: "https://vault.azure.net", spec_path: Some("specification/keyvault/data-plane/Secrets/stable"), preview_spec_path: None, route_versioned: false },
    ProviderMeta { provider: Provider::Graph, label: "Microsoft Graph", stable: GRAPH_API_VERSION, preview: None, audience: "https://graph.microsoft.com", spec_path: None, preview_spec_path: None, route_versioned: true },
];

pub fn providers() -> &'static [ProviderMeta] { PROVIDERS }

pub fn provider(p: Provider) -> &'static ProviderMeta {
    PROVIDERS.iter().find(|m| m.provider == p).expect("every Provider has a table entry")
}
```

- [ ] **Step 4: Run the registry test**

Run: `cargo test -p rigg-core provider_table`
Expected: PASS. (`no_api_version_literals` still fails until Tasks 3–4.)

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-core Cargo.toml
git commit -m "feat(registry): provider table — every Azure API version in one place

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: ARM clients on the provider table (Search, CognitiveServices, Storage, Web, Authorization, Resources)

**Files:**
- Modify: `crates/rigg-client/src/arm.rs` (all `api-version=` literals: lines ~353, 393, 465, 496, 551, 613, 650, 679, 719, 738, 765, 798, 827, 855, 886, 957, 1000, 1037; plus `ARM_BASE_URL` const), `crates/rigg-client/src/arm_resources.rs:17,60-66`, `crates/rigg/src/commands/doctor.rs:13-14,45,59,79,87`, `crates/rigg/src/commands/push.rs:862,883`
- Test: `crates/rigg-client/src/arm.rs` inline tests

**Interfaces:**
- Produces: `ArmClient::url(&self, path: &str, provider: Provider) -> String` (builds `https://management.azure.com{path}?api-version={provider(p).stable}`; `path` starts with `/`); `ArmClient::get_resource_identity(&self, resource_id: &str, provider: Provider)`; `ArmClient::site_config(&self, site_id: &str) -> Result<Value, ClientError>` (`GET {site_id}/config/web`).
- Removes: `ArmClient::get_storage_account_key`, `ArmClient::get_storage_connection_string`.

- [ ] **Step 1: Write the failing test**

In `arm.rs` tests:

```rust
#[test]
fn arm_urls_come_from_the_provider_table() {
    use rigg_core::registry::{Provider, provider};
    let c = ArmClient::with_token("t".to_string());
    assert_eq!(
        c.url("/subscriptions/s/providers/Microsoft.Search/searchServices", Provider::SearchArm),
        format!("https://management.azure.com/subscriptions/s/providers/Microsoft.Search/searchServices?api-version={}", provider(Provider::SearchArm).stable)
    );
    assert!(c.url("/subscriptions", Provider::ResourcesArm).ends_with("?api-version=2022-12-01"));
}
```

Add a test-only constructor `pub fn with_token(token: String) -> Self` (builds the reqwest client, stores the token) if one does not exist.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rigg-client arm_urls`
Expected: FAIL (`url` not defined).

- [ ] **Step 3: Implement `url` and replace every literal**

```rust
/// ARM URL for `path` (leading `/`) on `provider`'s pinned api-version.
pub fn url(&self, path: &str, provider: Provider) -> String {
    format!("{}{}?api-version={}", rigg_core::registry::ARM_BASE_URL, path, rigg_core::registry::provider(provider).stable)
}
```

Then rewrite each `format!("{}...?api-version=...", ARM_BASE_URL, ...)` in `arm.rs` as `self.url(&format!("/subscriptions/{}/providers/Microsoft.Search/searchServices", subscription_id), Provider::SearchArm)`, mapping:

| old literal | provider |
|---|---|
| `Microsoft.Authorization/roleAssignments…2022-04-01` (2 sites) | `AuthorizationArm` |
| `/subscriptions?api-version=2022-12-01` | `ResourcesArm` |
| `Microsoft.Search/searchServices…2023-11-01` | `SearchArm` |
| `Microsoft.CognitiveServices/accounts…2024-10-01` (2), `…/projects…2025-05-15-preview`, deployments (2, `2024-10-01`) | `CognitiveServicesArm` |
| `Microsoft.Web/sites…2023-12-01` (2), `functions/{f}/listkeys`, `host/default/listkeys`, `config/authsettingsV2/list` | `WebArm` |
| `Microsoft.Storage/storageAccounts…2023-05-01` (3), `blobServices/default/containers/{c}` | `StorageArm` |

Delete `get_storage_account_key` and `get_storage_connection_string` (no callers; keys never leave Azure). Delete the local `ARM_BASE_URL` const in `arm.rs` and `arm_resources.rs` and import `rigg_core::registry::ARM_BASE_URL`.

`get_resource_identity`: change the second parameter from `api_version: &str` to `provider: Provider` and build with `self.url(resource_id, provider)`. Update callers: `doctor.rs` (delete both local consts; pass `Provider::SearchArm` / `Provider::CognitiveServicesArm`), `push.rs:862-883` (delete `SEARCH_ARM_API`; pass `Provider::SearchArm`).

Add:

```rust
/// Site configuration (`ipSecurityRestrictions`, `publicNetworkAccess`, …):
/// not returned by `GET sites/{name}`; lives under `config/web`.
pub async fn site_config(&self, site_id: &str) -> Result<Value, ClientError> {
    let url = self.url(&format!("{site_id}/config/web"), Provider::WebArm);
    let response = self.http.get(&url).header("Authorization", format!("Bearer {}", self.token)).send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await?;
        return Err(ClientError::from_response(status.as_u16(), &body));
    }
    Ok(response.json().await?)
}
```

`arm_resources.rs::arm_url`: keep using `registry::ARM_COGNITIVE_API_VERSION` (already a constant). Search ARM enum casing: `list_search_services` deserializes `SearchService`; if it deserializes `publicNetworkAccess` or `hostingMode` into an enum, make those `String` and compare with `eq_ignore_ascii_case` where read. Check `struct SearchService` (arm.rs ~36-62) and adjust.

- [ ] **Step 4: Run tests and the literal guard**

Run: `cargo test -p rigg-client && cargo test -p rigg-core no_api_version`
Expected: rigg-client green; the guard now lists only `client.rs` tests and `tests/sync.rs`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(client): ARM calls on the provider table — Search 2025-05-01, CognitiveServices 2026-07-01, Storage 2026-06-01, Web 2026-07-15; drop storage listKeys

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Search data plane 2026-08-01-preview — paging, MCP URL, immutable networkAccessMode

**Files:**
- Modify: `crates/rigg-client/src/client.rs:236-255` (`list`) and tests ~425-475, `crates/rigg/src/commands/remote.rs:285-300`, `crates/rigg-core/src/registry.rs` (KnowledgeSource `immutable_fields`, sample at ~907), `crates/rigg/tests/sync.rs:2824,2830`
- Test: `crates/rigg/tests/sync.rs` (new two-page listing test), registry inline test, `remote.rs` inline test

**Interfaces:**
- Produces: `AzureSearchClient::list` follows `@odata.nextLink`; `remote::kb_mcp_url(search_service: &str, kb: &str) -> String` = `https://{svc}.search.windows.net/knowledgebases/{kb}/mcp?api-version={SEARCH_PREVIEW_API_VERSION}`.

- [ ] **Step 1: Write the failing tests**

`client.rs` tests: change `make_client` to use `rigg_core::registry::{SEARCH_STABLE_API_VERSION, SEARCH_PREVIEW_API_VERSION}` instead of the two literals, and the three `assert_eq!` URLs to be built with `format!` from the constants.

`tests/sync.rs`: change the two `query_param("api-version", "2026-05-01-preview")` to `query_param("api-version", rigg_core::registry::SEARCH_PREVIEW_API_VERSION)` (add `rigg-core` to `[dev-dependencies]` of `crates/rigg/Cargo.toml` if missing), and add:

```rust
#[tokio::test]
async fn pull_follows_odata_next_link_across_pages() {
    let server = MockServer::start().await;
    let page2 = format!("{}/indexes?api-version=2026-04-01&$skiptoken=abc", server.uri());
    Mock::given(method("GET")).and(path("/indexes")).and(query_param("$skiptoken", "abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"name": "idx-b", "fields": []}]})))
        .mount(&server).await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"name": "idx-a", "fields": []}], "@odata.nextLink": page2})))
        .mount(&server).await;
    mount_empty_lists_except(&server, "indexes").await; // helper: 200 {"value":[]} for every other kind
    let ws = workspace(&server.uri());
    rigg(ws.path()).args(["adopt", "demo", "all", "--yes"]).assert().success();
    assert!(ws.path().join("projects/demo/envs/dev/search/indexes/idx-a.json").exists());
    assert!(ws.path().join("projects/demo/envs/dev/search/indexes/idx-b.json").exists());
}
```

Write `mount_empty_lists_except` next to the other helpers: iterate `["datasources","indexers","skillsets","synonymmaps","aliases","knowledgeSources","knowledgeBases"]`, mount `GET /{path}` → `{"value": []}` (skip the named one). Wiremock is first-match-wins: mount the `$skiptoken` mock **before** the generic `/indexes` mock, as above.

`registry.rs` test:

```rust
#[test]
fn knowledge_source_network_access_mode_is_immutable() {
    let a = json!({"name": "ks", "kind": "azureBlob", "azureBlobParameters": {"ingestionParameters": {"networkAccessMode": "public"}}});
    let b = json!({"name": "ks", "kind": "azureBlob", "azureBlobParameters": {"ingestionParameters": {"networkAccessMode": "private"}}});
    assert!(!immutable_diff(ResourceKind::KnowledgeSource, &a, &b).is_empty());
}
```

`remote.rs` test:

```rust
#[test]
fn kb_mcp_url_uses_the_documented_form_and_preview_version() {
    assert_eq!(
        kb_mcp_url("mklabsrch", "regulatory-kb"),
        format!("https://mklabsrch.search.windows.net/knowledgebases/regulatory-kb/mcp?api-version={}", rigg_core::registry::SEARCH_PREVIEW_API_VERSION)
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg --test sync pull_follows && cargo test -p rigg-core network_access_mode && cargo test -p rigg kb_mcp_url`
Expected: FAIL (only page 1 adopted; immutable diff empty; `kb_mcp_url` undefined).

- [ ] **Step 3: Implement**

`client.rs::list`:

```rust
pub async fn list(&self, kind: ResourceKind) -> Result<Vec<Value>, ClientError> {
    let mut url = self.collection_url(kind);
    let mut items = Vec::new();
    loop {
        let Some(page) = self.request_with_retry(Method::GET, &url, None).await? else { break };
        if let Some(arr) = page.get("value").and_then(Value::as_array) {
            items.extend(arr.iter().cloned());
        }
        // 2026-08-01-preview pages list results; the link must be used verbatim.
        match page.get("@odata.nextLink").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => url = next.to_string(),
            _ => break,
        }
    }
    Ok(items)
}
```

`registry.rs`: KnowledgeSource `immutable_fields: &["kind", "azureBlobParameters.ingestionParameters.networkAccessMode"]`. Check `immutable_diff` (line ~684) walks paths with `collect_path` (dot syntax); if it only reads top-level keys, switch it to `collect_path` and keep the existing `kind` behaviour.

`remote.rs`: extract

```rust
/// The knowledge base's MCP endpoint as Foundry expects it (documented form;
/// answers synthesized on the preview api-version).
pub fn kb_mcp_url(search_service: &str, kb: &str) -> String {
    format!(
        "https://{search_service}.search.windows.net/knowledgebases/{kb}/mcp?api-version={}",
        rigg_core::registry::SEARCH_PREVIEW_API_VERSION
    )
}
```

and use it in `resolve_walk`. Update the registry sample at ~907 to `…/mcp?api-version=2026-08-01-preview` (a test fixture string; the parser ignores the query).

- [ ] **Step 4: Run the whole gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: green, including `no_api_version_literals`.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(search): 2026-08-01-preview — follow @odata.nextLink, documented KB MCP URL, networkAccessMode immutable

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: `rigg dev api-check` over the whole provider table

**Files:**
- Modify: `crates/rigg/src/commands/dev.rs` (replace `CHECKS`), `.claude/skills/api-watchdog/SKILL.md`, `.github/workflows/api-watchdog.yml` (issue body text only)
- Test: `dev.rs` inline

**Interfaces:**
- Produces: `dev::latest_from_entries(entries: &[Value]) -> Option<String>` (pure; extracted from `fetch_latest_version`); JSON output rows `{api, channel: "stable"|"preview", supported, latest_upstream, status}`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn latest_from_entries_picks_newest_version_folder() {
    let entries: Vec<serde_json::Value> = ["2023-11-01", "2025-05-01", "README.md", "2022-09-01"]
        .iter().map(|n| serde_json::json!({"name": n})).collect();
    assert_eq!(latest_from_entries(&entries).as_deref(), Some("2025-05-01"));
    assert_eq!(latest_from_entries(&[]), None);
}

#[test]
fn every_spec_backed_provider_is_checked() {
    let checks = checks();
    let backed = rigg_core::registry::providers().iter().filter(|p| p.spec_path.is_some()).count();
    let previews = rigg_core::registry::providers().iter().filter(|p| p.preview_spec_path.is_some()).count();
    assert_eq!(checks.len(), backed + previews);
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p rigg latest_from_entries every_spec_backed`
Expected: FAIL (functions undefined).

- [ ] **Step 3: Implement**

Replace the static `CHECKS` with:

```rust
struct Check { label: String, channel: &'static str, spec_path: &'static str, supported: &'static str }

fn checks() -> Vec<Check> {
    let mut out = Vec::new();
    for p in rigg_core::registry::providers() {
        if let Some(path) = p.spec_path {
            out.push(Check { label: format!("{} (stable)", p.label), channel: "stable", spec_path: path, supported: p.stable });
        }
        if let (Some(path), Some(preview)) = (p.preview_spec_path, p.preview) {
            out.push(Check { label: format!("{} (preview)", p.label), channel: "preview", spec_path: path, supported: preview });
        }
    }
    out
}

fn latest_from_entries(entries: &[serde_json::Value]) -> Option<String> {
    let mut versions: Vec<String> = entries.iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .filter(|n| n.len() >= 10 && n.as_bytes()[4] == b'-')
        .map(str::to_string).collect();
    versions.sort();
    versions.pop()
}
```

`fetch_latest_version` calls `latest_from_entries`. The informational Foundry line becomes a loop over `providers().iter().filter(|p| p.route_versioned)`. The JSON row gains `"channel"`. Keep exit semantics (behind → exit 1; lookup failures never fail).

`.claude/skills/api-watchdog/SKILL.md`: replace "rigg pins Azure api-versions as constants … (search stable/preview, foundry data plane, CognitiveServices ARM)" with "rigg pins every Azure api-version in the registry provider table (`providers()` in `crates/rigg-core/src/registry.rs`)" and add `rigg dev api-diff <provider> --from <old> --to <new>` as step 1 of the upgrade procedure. Workflow issue body: mention `rigg dev api-diff`.

- [ ] **Step 4: Run tests and the live check**

Run: `cargo test -p rigg dev:: && cargo run -q --bin rigg -- dev api-check`
Expected: tests pass; api-check prints one row per spec-backed provider (+ preview rows), all `current` except possibly Storage/KeyVault if Azure moved again (report, do not fail the task on that).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(dev): api-check covers every provider in the registry table

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Schema fixtures, `api-fixture`, `api-diff`, and the unknown-field canary

**Files:**
- Create: `crates/rigg-core/src/schema.rs`, `crates/rigg-core/fixtures/schema/search-data-2026-04-01.json`, `…/search-data-2026-08-01-preview.json`, `…/cognitiveservices-arm-2026-07-01.json`, `crates/rigg/src/commands/dev_spec.rs`
- Modify: `crates/rigg-core/src/lib.rs` (`pub mod schema;`), `crates/rigg-core/src/registry.rs` (per-kind `schema_definition: &'static str` in `KindMeta`), `crates/rigg/src/cli.rs:722-725` (`DevCommands::{ApiDiff, ApiFixture}`), `crates/rigg/src/commands/dev.rs` (dispatch), `crates/rigg/src/commands/pull.rs:120-135`, `crates/rigg/src/commands/adopt.rs` (after each `store.write`)
- Test: `schema.rs` inline; registry inline; `crates/rigg/tests/sync.rs` canary test

**Interfaces:**
- Produces:
  ```rust
  // rigg-core::schema
  pub struct SchemaFixture { pub provider: &'static str, pub version: &'static str, definitions: BTreeMap<String, BTreeSet<String>> }
  pub fn fixture_for(kind: ResourceKind) -> &'static SchemaFixture;      // embedded via include_str!
  pub fn unknown_top_level_fields(kind: ResourceKind, doc: &Value) -> Vec<String>;
  pub fn extract_fixture(openapi: &Value) -> BTreeMap<String, BTreeSet<String>>; // definitions → property names (allOf/$ref resolved one level)
  pub fn diff_definitions(old: &Value, new: &Value, names: &[&str]) -> Vec<DefinitionDiff>;
  pub struct DefinitionDiff { pub definition: String, pub added: Vec<String>, pub removed: Vec<String>, pub enum_added: Vec<(String, String)>, pub enum_removed: Vec<(String, String)>, pub missing_in: Option<&'static str> }
  ```
  `KindMeta.schema_definition`: DataSource→`SearchIndexerDataSource`, Index→`SearchIndex`, Indexer→`SearchIndexer`, Skillset→`SearchIndexerSkillset`, SynonymMap→`SynonymMap`, Alias→`SearchAlias`, KnowledgeSource→`KnowledgeSource`, KnowledgeBase→`KnowledgeBase`, Agent→`""` (Foundry: no fixture), Deployment→`Deployment`, Connection→`ConnectionPropertiesV2`, Guardrail→`RaiPolicy`.

- [ ] **Step 1: Capture the fixtures (one-off, committed)**

Download the three OpenAPI documents with `curl` from `https://raw.githubusercontent.com/Azure/azure-rest-api-specs/main/specification/search/data-plane/Search/stable/2026-04-01/search.json`, `…/preview/2026-08-01-preview/search.json`, `…/cognitiveservices/resource-manager/Microsoft.CognitiveServices/stable/2026-07-01/cognitiveservices.json` into the scratchpad. Fixtures are produced by `rigg dev api-fixture` in Step 5; for now write `extract_fixture` (Step 3) and a tiny `examples/` -free path: a `#[test] #[ignore]` in `schema.rs` named `regenerate_fixtures` that reads `RIGG_OPENAPI_DIR` and writes the three files. Run it once after Step 3 with `RIGG_OPENAPI_DIR=<scratchpad> cargo test -p rigg-core regenerate_fixtures -- --ignored`.

Fixture format (small — property names only):

```json
{ "provider": "search-data", "version": "2026-08-01-preview",
  "definitions": { "KnowledgeBase": ["@odata.etag", "answerInstructions", "corsOptions", "description", "encryptionKey", "knowledgeSources", "models", "name", "outputMode", "retrievalInstructions", "retrievalReasoningEffort", "retrieveDefaults", "tags"], "...": [] } }
```

- [ ] **Step 2: Write the failing tests**

`schema.rs`:

```rust
#[test]
fn extract_resolves_allof_one_level() {
    let doc = json!({"definitions": {
        "Base": {"properties": {"name": {}, "description": {}}},
        "Child": {"allOf": [{"$ref": "#/definitions/Base"}], "properties": {"extra": {}}}
    }});
    let f = extract_fixture(&doc);
    assert_eq!(f["Child"].iter().cloned().collect::<Vec<_>>(), vec!["description", "extra", "name"]);
}

#[test]
fn unknown_fields_reports_only_keys_missing_from_the_fixture() {
    let doc = json!({"name": "kb", "knowledgeSources": [], "retrievalMode": "x", "@odata.etag": "e"});
    let unknown = unknown_top_level_fields(ResourceKind::KnowledgeBase, &doc);
    assert_eq!(unknown, vec!["retrievalMode"]);
}

#[test]
fn diff_definitions_lists_added_removed_and_enum_changes() {
    let old = json!({"definitions": {"A": {"properties": {"x": {}, "y": {"type": "string", "enum": ["p"]}}}}});
    let new = json!({"definitions": {"A": {"properties": {"x": {}, "z": {}, "y": {"type": "string", "enum": ["p", "q"]}}}}});
    let d = &diff_definitions(&old, &new, &["A"])[0];
    assert_eq!(d.added, vec!["z"]);
    assert!(d.removed.is_empty());
    assert_eq!(d.enum_added, vec![("y".to_string(), "q".to_string())]);
}
```

`registry.rs`:

```rust
#[test]
fn registry_paths_exist_in_the_pinned_schema() {
    use crate::schema::fixture_for;
    for kind in ResourceKind::search_kinds() {
        let m = meta(kind);
        let f = fixture_for(kind);
        let props = f.definition(m.schema_definition).expect(m.schema_definition);
        for path in m.volatile_fields.iter().chain(m.read_only_fields).chain(m.secret_fields).chain(m.write_only_fields).chain(m.immutable_fields).chain(m.reference_fields.iter().map(|r| &r.path)) {
            let head = path.split('.').next().unwrap().trim_end_matches("[]");
            if head.starts_with("@odata") || head == "etag" || head == "e_tag" { continue; }
            assert!(props.contains(head), "{kind:?}: `{path}` not in {} ({})", m.schema_definition, f.version);
        }
    }
}
```

`tests/sync.rs`:

```rust
#[tokio::test]
async fn pull_reports_fields_unknown_to_the_pinned_schema() {
    let server = MockServer::start().await;
    mount_empty_lists_except(&server, "indexes").await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"name": "idx", "fields": [], "brandNewSetting": true}]})))
        .mount(&server).await;
    let ws = workspace(&server.uri());
    rigg(ws.path()).args(["adopt", "demo", "all", "--yes"]).assert().success()
        .stderr(predicate::str::contains("brandNewSetting").and(predicate::str::contains("rigg dev api-check")));
    let doc: Value = serde_json::from_str(&std::fs::read_to_string(ws.path().join("projects/demo/envs/dev/search/indexes/idx.json")).unwrap()).unwrap();
    assert_eq!(doc["brandNewSetting"], json!(true), "documents stay pass-through");
}
```

- [ ] **Step 3: Run to verify they fail, then implement `schema.rs`**

Run: `cargo test -p rigg-core schema:: registry_paths_exist`
Expected: compile errors.

Implement:

```rust
//! Pinned-version schema fixtures (property names per OpenAPI definition):
//! the registry is checked against them, and pull/adopt report fields Azure
//! returns that the pinned version does not know (an API-drift canary).
use std::collections::{BTreeMap, BTreeSet};
use serde_json::Value;
use crate::{registry, resources::ResourceKind};

pub struct SchemaFixture { pub provider: &'static str, pub version: &'static str, definitions: BTreeMap<String, BTreeSet<String>> }

impl SchemaFixture {
    fn parse(provider: &'static str, version: &'static str, text: &str) -> Self {
        let v: Value = serde_json::from_str(text).expect("fixture is valid JSON");
        let definitions = v["definitions"].as_object().expect("definitions").iter()
            .map(|(k, arr)| (k.clone(), arr.as_array().unwrap().iter().filter_map(Value::as_str).map(str::to_string).collect()))
            .collect();
        Self { provider, version, definitions }
    }
    pub fn definition(&self, name: &str) -> Option<&BTreeSet<String>> { self.definitions.get(name) }
}

static SEARCH_STABLE: std::sync::OnceLock<SchemaFixture> = std::sync::OnceLock::new();
static SEARCH_PREVIEW: std::sync::OnceLock<SchemaFixture> = std::sync::OnceLock::new();
static COGNITIVE_ARM: std::sync::OnceLock<SchemaFixture> = std::sync::OnceLock::new();

pub fn fixture_for(kind: ResourceKind) -> &'static SchemaFixture {
    match registry::meta(kind).domain {
        registry::Domain::Search => match registry::meta(kind).channel {
            registry::Channel::Stable => SEARCH_STABLE.get_or_init(|| SchemaFixture::parse("search-data", registry::SEARCH_STABLE_API_VERSION, include_str!("../fixtures/schema/search-data-2026-04-01.json"))),
            registry::Channel::Preview => SEARCH_PREVIEW.get_or_init(|| SchemaFixture::parse("search-data", registry::SEARCH_PREVIEW_API_VERSION, include_str!("../fixtures/schema/search-data-2026-08-01-preview.json"))),
        },
        _ => COGNITIVE_ARM.get_or_init(|| SchemaFixture::parse("cognitiveservices-arm", registry::ARM_COGNITIVE_API_VERSION, include_str!("../fixtures/schema/cognitiveservices-arm-2026-07-01.json"))),
    }
}

/// Top-level keys of `doc` that the pinned schema does not declare for
/// `kind`. Empty for kinds without a fixture definition (agents).
pub fn unknown_top_level_fields(kind: ResourceKind, doc: &Value) -> Vec<String> {
    let name = registry::meta(kind).schema_definition;
    if name.is_empty() { return Vec::new(); }
    let Some(props) = fixture_for(kind).definition(name) else { return Vec::new(); };
    doc.as_object().map(|m| m.keys().filter(|k| !k.starts_with("x-rigg-") && !k.starts_with("@odata") && !props.contains(*k)).cloned().collect()).unwrap_or_default()
}

/// `definitions` → property names, resolving `allOf` `$ref`s one level.
pub fn extract_fixture(openapi: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let defs = openapi["definitions"].as_object().cloned().unwrap_or_default();
    let props_of = |d: &Value| -> BTreeSet<String> { d["properties"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default() };
    defs.iter().map(|(name, d)| {
        let mut set = props_of(d);
        if let Some(all) = d["allOf"].as_array() {
            for part in all {
                if let Some(r) = part["$ref"].as_str().and_then(|r| r.strip_prefix("#/definitions/")) {
                    if let Some(base) = defs.get(r) { set.extend(props_of(base)); }
                }
                set.extend(props_of(part));
            }
        }
        (name.clone(), set)
    }).collect()
}

pub struct DefinitionDiff { pub definition: String, pub added: Vec<String>, pub removed: Vec<String>, pub enum_added: Vec<(String, String)>, pub enum_removed: Vec<(String, String)>, pub missing_in: Option<&'static str> }

pub fn diff_definitions(old: &Value, new: &Value, names: &[&str]) -> Vec<DefinitionDiff> {
    let (fo, fn_) = (extract_fixture(old), extract_fixture(new));
    let enums = |doc: &Value, def: &str| -> BTreeMap<String, BTreeSet<String>> {
        doc["definitions"][def]["properties"].as_object().map(|m| m.iter().filter_map(|(k, v)| v["enum"].as_array().map(|e| (k.clone(), e.iter().filter_map(Value::as_str).map(str::to_string).collect()))).collect()).unwrap_or_default()
    };
    names.iter().map(|n| {
        let (a, b) = (fo.get(*n), fn_.get(*n));
        let missing_in = match (a, b) { (None, _) => Some("old"), (_, None) => Some("new"), _ => None };
        let (a, b) = (a.cloned().unwrap_or_default(), b.cloned().unwrap_or_default());
        let (eo, en) = (enums(old, n), enums(new, n));
        let mut enum_added = Vec::new(); let mut enum_removed = Vec::new();
        for (k, vals) in &en { for v in vals { if !eo.get(k).is_some_and(|s| s.contains(v)) { enum_added.push((k.clone(), v.clone())); } } }
        for (k, vals) in &eo { for v in vals { if !en.get(k).is_some_and(|s| s.contains(v)) { enum_removed.push((k.clone(), v.clone())); } } }
        DefinitionDiff { definition: n.to_string(), added: b.difference(&a).cloned().collect(), removed: a.difference(&b).cloned().collect(), enum_added, enum_removed, missing_in }
    }).collect()
}
```

Add `schema_definition` to `KindMeta` and to every entry in `KINDS` per the Interfaces mapping. Register `pub mod schema;` in `lib.rs`. Then run the ignored `regenerate_fixtures` test to write the three fixture files, and `cargo test -p rigg-core` — `registry_paths_exist_in_the_pinned_schema` must pass; if a path fails, the registry path is wrong for this version — fix the registry, not the test.

- [ ] **Step 4: Wire the canary into pull and adopt**

In `pull.rs` after every `store.write(r, doc)?` that returns true, and in `adopt.rs` after each write, add:

```rust
for field in rigg_core::schema::unknown_top_level_fields(r.kind, doc) {
    eprintln!(
        "{} {r}: field `{field}` is not in rigg's {} schema — Azure may have shipped a newer API; run `rigg dev api-check`",
        "note:".dimmed(),
        rigg_core::schema::fixture_for(r.kind).version
    );
}
```

Deduplicate per run with a `BTreeSet<(String, String)>` (resource, field) held in the command function so a field is reported once.

- [ ] **Step 5: `rigg dev api-fixture` and `rigg dev api-diff`**

`cli.rs`:

```rust
pub enum DevCommands {
    /// Check whether newer Azure API versions are available
    ApiCheck,
    /// Show what changed between two versions of a provider's OpenAPI definitions
    ApiDiff { provider: String, #[arg(long)] from: Option<String>, #[arg(long)] to: Option<String> },
    /// Regenerate the pinned schema fixtures under crates/rigg-core/fixtures/schema
    ApiFixture { provider: String },
}
```

`commands/dev_spec.rs`:

```rust
//! `rigg dev api-diff` / `api-fixture`: fetch OpenAPI documents from
//! Azure/azure-rest-api-specs and diff or extract them.
use anyhow::{Context, Result, bail};
use rigg_core::registry::{Provider, provider, providers};

const RAW: &str = "https://raw.githubusercontent.com/Azure/azure-rest-api-specs/main";
const API: &str = "https://api.github.com/repos/Azure/azure-rest-api-specs/contents";

/// Definitions rigg cares about, per provider.
fn definitions(p: Provider) -> &'static [&'static str] {
    match p {
        Provider::SearchData => &["SearchIndexerDataSource", "SearchIndex", "SearchIndexer", "SearchIndexerSkillset", "SynonymMap", "SearchAlias", "KnowledgeSource", "KnowledgeBase", "WebApiSkill", "AzureOpenAIVectorizerParameters", "AIServicesAccountIdentity", "SearchIndexerDataUserAssignedIdentity", "SearchIndexerDataSourceType", "KnowledgeSourceKind"],
        Provider::CognitiveServicesArm => &["Account", "Project", "Identity", "DeploymentProperties", "ConnectionPropertiesV2", "ConnectionAuthType", "ConnectionCategory", "RaiPolicyProperties"],
        Provider::SearchArm => &["SearchService", "SearchServiceProperties", "Identity", "NetworkRuleSet", "SharedPrivateLinkResourceProperties"],
        Provider::StorageArm => &["StorageAccount", "StorageAccountProperties", "NetworkRuleSet", "BlobServiceProperties"],
        Provider::WebArm => &["Site", "SiteProperties", "SiteConfig", "SiteAuthSettingsV2", "SiteAuthSettingsV2Properties"],
        _ => &[],
    }
}

fn parse_provider(s: &str) -> Result<&'static rigg_core::registry::ProviderMeta> {
    providers().iter().find(|m| m.label.to_ascii_lowercase().replace(' ', "-").contains(&s.to_ascii_lowercase()) || format!("{:?}", m.provider).eq_ignore_ascii_case(s))
        .with_context(|| format!("unknown provider '{s}' (one of: {})", providers().iter().map(|m| format!("{:?}", m.provider)).collect::<Vec<_>>().join(", ")))
}

/// The OpenAPI document of `version` under `spec_path`: the first `.json` in the version folder.
async fn fetch_openapi(http: &reqwest::Client, spec_path: &str, version: &str) -> Result<serde_json::Value> {
    let listing: Vec<serde_json::Value> = http.get(format!("{API}/{spec_path}/{version}")).send().await?.error_for_status()?.json().await?;
    let file = listing.iter().filter_map(|e| e["name"].as_str()).find(|n| n.ends_with(".json"))
        .with_context(|| format!("no .json in {spec_path}/{version}"))?;
    Ok(http.get(format!("{RAW}/{spec_path}/{version}/{file}")).send().await?.error_for_status()?.json().await?)
}

pub async fn api_diff(provider_name: &str, from: Option<String>, to: Option<String>) -> Result<()> {
    let m = parse_provider(provider_name)?;
    let Some(spec_path) = m.spec_path else { bail!("{} is route-versioned; nothing to diff", m.label) };
    let http = reqwest::Client::builder().user_agent("rigg-api-diff").build()?;
    let from = from.unwrap_or_else(|| m.stable.to_string());
    let to = match to { Some(t) => t, None => { let l: Vec<serde_json::Value> = http.get(format!("{API}/{spec_path}").header("User-Agent", "rigg").send().await?.json().await?; crate::commands::dev::latest_from_entries(&l).context("no versions upstream")? } };
    let (old, new) = (fetch_openapi(&http, spec_path, &from).await?, fetch_openapi(&http, spec_path, &to).await?);
    println!("{}: {from} → {to}", m.label);
    for d in rigg_core::schema::diff_definitions(&old, &new, definitions(m.provider)) {
        if let Some(side) = d.missing_in { println!("  {}: missing in {side}", d.definition); continue; }
        if d.added.is_empty() && d.removed.is_empty() && d.enum_added.is_empty() && d.enum_removed.is_empty() { continue; }
        println!("  {}", d.definition);
        for f in &d.added { println!("    + {f}"); }
        for f in &d.removed { println!("    - {f}"); }
        for (f, v) in &d.enum_added { println!("    + {f}: {v}"); }
        for (f, v) in &d.enum_removed { println!("    - {f}: {v}"); }
    }
    Ok(())
}

pub async fn api_fixture(provider_name: &str) -> Result<()> {
    let m = parse_provider(provider_name)?;
    let Some(spec_path) = m.spec_path else { bail!("{} has no OpenAPI document", m.label) };
    let http = reqwest::Client::builder().user_agent("rigg-api-fixture").build()?;
    let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rigg-core/fixtures/schema");
    let slug = match m.provider { Provider::SearchData => "search-data", Provider::CognitiveServicesArm => "cognitiveservices-arm", other => bail!("no fixture is kept for {other:?}") };
    let mut versions = vec![(spec_path, m.stable)];
    if let (Some(p), Some(v)) = (m.preview_spec_path, m.preview) { versions.push((p, v)); }
    for (path, version) in versions {
        let doc = fetch_openapi(&http, path, version).await?;
        let defs = rigg_core::schema::extract_fixture(&doc);
        let value = serde_json::json!({ "provider": slug, "version": version, "definitions": defs });
        let file = out_dir.join(format!("{slug}-{version}.json"));
        std::fs::write(&file, serde_json::to_string_pretty(&value)?)?;
        println!("wrote {}", file.display());
    }
    Ok(())
}
```

Dispatch both from `dev.rs::run`. Make `latest_from_entries` `pub(crate)`.

- [ ] **Step 6: Run the gate and one live diff**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo run -q --bin rigg -- dev api-diff SearchData --from 2026-05-01-preview --to 2026-08-01-preview`
Expected: green; the diff prints `KnowledgeBase + retrieveDefaults`, `+ tags` and `KnowledgeSource + resultsProcessing` (the live diff is a smoke check, not a test).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(dev): schema fixtures, api-diff/api-fixture, unknown-field canary on pull and adopt

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Changelog, docs, live smoke

**Files:**
- Modify: `CHANGELOG.md` (new `[Unreleased]` → 2.0.0 opening), `README.md` (data-source sentence; `rigg dev` commands), `CLAUDE.md` (registry provider table sentence), `.claude/skills/api-watchdog/SKILL.md` (done in Task 5, re-check)

- [ ] **Step 1: Changelog**

Add at the top of `CHANGELOG.md`:

```markdown
## [Unreleased] — 2.0.0

rigg 2.0 narrows to the Agentic RAG stack it does best — Microsoft Foundry
agents grounded on Azure AI Search, fed from Azure Blob Storage, enriched by
Azure Functions — and rebuilds environments, promotion and authentication
around that. No compatibility with 1.x workspaces.

### Changed (breaking)

- **Blob Storage is the only data source** (`azureblob`, `adlsgen2`). Cosmos
  DB, Azure SQL, OneLake, SharePoint, MySQL, Table and Files support is removed,
  including the Cosmos client and the `cosmos-sql-patterns` sample.
- **Every Azure API version now lives in the registry provider table** and is
  the newest available: Search data plane 2026-04-01 / 2026-08-01-preview,
  Microsoft.CognitiveServices 2026-07-01, Microsoft.Search 2025-05-01,
  Microsoft.Storage 2026-06-01, Microsoft.Web 2026-07-15. Search list
  operations follow `@odata.nextLink`. Knowledge-base MCP endpoints are written
  in the documented `knowledgebases/<kb>/mcp?api-version=2026-08-01-preview` form.
- A knowledge source's `ingestionParameters.networkAccessMode` is immutable:
  changing it locally shows `replace`.
- Storage account `listKeys` is no longer called anywhere.

### Added

- `rigg dev api-check` covers every provider; `rigg dev api-diff <provider>`
  shows added/removed properties and enum values between two versions;
  `rigg dev api-fixture` refreshes the pinned schema fixtures.
- Pull and adopt report fields Azure returns that rigg's pinned schema does
  not know (API-drift canary; documents stay untouched).
```

- [ ] **Step 2: README and CLAUDE.md**

`README.md`: in the features list add "Blob Storage only" wording where data sources are described; document the two new `rigg dev` commands next to `api-check`. `CLAUDE.md`: change "Supported versions are constants in `crates/rigg-core/src/registry.rs`" to "Every API version is in the registry provider table (`providers()`)".

- [ ] **Step 3: Live smoke against Azure (Kristofer's login)**

Run, from `e2e-test/`:

```bash
cargo build -q && ~/.local/bin/rigg status && ~/.local/bin/rigg pull regulus --dry-run
```

Expected: status lists dev and staging; pull dry-run shows no unexpected drift and no canary notes (if a canary note appears, record the field in the task result — it is information, not a failure).

- [ ] **Step 4: Full gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "docs: 2.0.0 changelog opening, api tooling docs

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: ARM-registered versions — hold CognitiveServices at 2026-05-01, teach api-check about ARM registration

Added 2026-09-10 after the Task 7 live smoke: Azure rejects Microsoft.CognitiveServices `2026-07-01` for `accounts/projects/connections` (ARM registers at most `2026-05-01` stable / `2026-05-15-preview` for connections, while `accounts` and `accounts/projects` accept `2026-07-01`). The 2026-05-01 → 2026-07-01 diff had no changes for rigg's kinds, so one uniform version is correct. The watchdog must know why a pin is held so it does not report BEHIND forever, and it must be able to tell when the hold can be lifted.

**Files:**
- Modify: `crates/rigg-core/src/registry.rs` (`ARM_COGNITIVE_API_VERSION`, `ProviderMeta`, `PROVIDERS`, tests), `crates/rigg/src/commands/dev.rs` (`checks()`, status logic, output), `crates/rigg-client/src/arm.rs` (new `provider_api_versions`), `CHANGELOG.md` (CognitiveServices line), `README.md` ("Resource Kinds" API-version sentence), `crates/rigg-client/src/arm_resources.rs` test asserting the version
- Test: registry inline; `dev.rs` inline; `arm.rs` inline

**Interfaces:**
- Produces:
  ```rust
  pub struct ArmRegistration { pub namespace: &'static str, pub resource_types: &'static [&'static str] }
  pub struct Hold { pub newer: &'static str, pub reason: &'static str }
  // ProviderMeta gains: pub arm: Option<ArmRegistration>, pub hold: Option<Hold>
  // ArmClient:
  pub async fn provider_api_versions(&self, subscription_id: &str, namespace: &str) -> Result<BTreeMap<String, Vec<String>>, ClientError> // resourceType → apiVersions
  ```
- api-check statuses: `current`, `BEHIND`, `held` (spec repo newest == `hold.newer`), `held — ARM now registers <v> for every type: lift the hold` (when ARM access is available and confirms), `?`.

- [ ] **Step 1: Write the failing tests**

`registry.rs`:

```rust
#[test]
fn cognitive_services_is_held_at_the_version_arm_registers_for_connections() {
    let m = provider(Provider::CognitiveServicesArm);
    assert_eq!(m.stable, "2026-05-01");
    let hold = m.hold.expect("hold documented");
    assert_eq!(hold.newer, "2026-07-01");
    let arm = m.arm.expect("arm registration");
    assert_eq!(arm.namespace, "Microsoft.CognitiveServices");
    assert!(arm.resource_types.contains(&"accounts/projects/connections"));
}

#[test]
fn every_arm_provider_declares_its_registration() {
    for m in providers() {
        if m.audience == "https://management.azure.com" && m.spec_path.is_some() {
            assert!(m.arm.is_some(), "{} lacks ArmRegistration", m.label);
        }
    }
}
```

`dev.rs`:

```rust
#[test]
fn status_is_held_when_upstream_equals_the_documented_hold() {
    let m = rigg_core::registry::provider(rigg_core::registry::Provider::CognitiveServicesArm);
    assert_eq!(status_for(m.stable, "2026-07-01", m.hold.as_ref(), None), "held");
    assert_eq!(status_for(m.stable, "2026-09-01", m.hold.as_ref(), None), "BEHIND");
    assert_eq!(status_for(m.stable, m.stable, m.hold.as_ref(), None), "current");
    // ARM confirms the newer version for every resource type → the hold can be lifted
    let arm_ok = Some(true);
    assert!(status_for(m.stable, "2026-07-01", m.hold.as_ref(), arm_ok).starts_with("held — ARM now registers"));
}
```

`arm.rs`:

```rust
#[test]
fn provider_api_versions_url_uses_resources_arm_version() {
    let c = ArmClient::with_token("t".into());
    assert_eq!(
        c.url("/subscriptions/s/providers/Microsoft.CognitiveServices", rigg_core::registry::Provider::ResourcesArm),
        format!("https://management.azure.com/subscriptions/s/providers/Microsoft.CognitiveServices?api-version={}", rigg_core::registry::ARM_RESOURCES_API_VERSION)
    );
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg-core cognitive_services_is_held every_arm_provider && cargo test -p rigg status_is_held`
Expected: FAIL (fields/functions missing; stable still 2026-07-01).

- [ ] **Step 3: Registry**

`ARM_COGNITIVE_API_VERSION = "2026-05-01"` with the doc comment: "Newest version ARM registers for every CognitiveServices resource type rigg uses; `accounts/projects/connections` caps it (2026-07-01 is registered for accounts and projects only, and changed nothing rigg reads)." Add the two structs and the two `ProviderMeta` fields. Table values:

| provider | arm | hold |
|---|---|---|
| CognitiveServicesArm | `Microsoft.CognitiveServices`, `["accounts", "accounts/projects", "accounts/projects/connections"]` | `newer: "2026-07-01", reason: "not registered for accounts/projects/connections (max 2026-05-01 stable)"` |
| SearchArm | `Microsoft.Search`, `["searchServices"]` | None |
| StorageArm | `Microsoft.Storage`, `["storageAccounts"]` | None |
| WebArm | `Microsoft.Web`, `["sites"]` | None |
| AuthorizationArm | `Microsoft.Authorization`, `["roleAssignments"]` | None |
| ResourcesArm | None (it is the registration API itself) | None |
| ManagedIdentityArm | `Microsoft.ManagedIdentity`, `["userAssignedIdentities"]` | None |
| KeyVaultArm | `Microsoft.KeyVault`, `["vaults"]` | None |
| others | None | None |

Update `provider_table_is_complete_and_current` (2026-05-01) and the `arm_resources.rs` test string.

- [ ] **Step 4: ARM registration lookup**

`arm.rs`:

```rust
/// `resourceType → apiVersions` as ARM registers them for `namespace` in
/// `subscription_id` (what `az provider show` prints). The ground truth for
/// which api-version a call may use — the specs repository can be ahead of it.
pub async fn provider_api_versions(&self, subscription_id: &str, namespace: &str) -> Result<BTreeMap<String, Vec<String>>, ClientError> {
    let url = self.url(&format!("/subscriptions/{subscription_id}/providers/{namespace}"), Provider::ResourcesArm);
    let response = self.http.get(&url).header("Authorization", format!("Bearer {}", self.token)).send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await?;
        return Err(ClientError::from_response(status.as_u16(), &body));
    }
    let value: Value = response.json().await?;
    let mut out = BTreeMap::new();
    for rt in value["resourceTypes"].as_array().into_iter().flatten() {
        let name = rt["resourceType"].as_str().unwrap_or_default().to_string();
        let versions = rt["apiVersions"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
        out.insert(name, versions);
    }
    Ok(out)
}
```

- [ ] **Step 5: api-check**

In `dev.rs` add a pure status function and use it:

```rust
/// `arm_confirms`: Some(true) when ARM registers `latest` for every resource
/// type of the provider, Some(false) when it does not, None without ARM access.
fn status_for(supported: &str, latest: &str, hold: Option<&Hold>, arm_confirms: Option<bool>) -> String {
    if !version_newer(latest, supported) { return "current".to_string(); }
    match hold {
        Some(h) if h.newer == latest => match arm_confirms {
            Some(true) => format!("held — ARM now registers {latest} for every type: lift the hold"),
            _ => "held".to_string(),
        },
        _ => "BEHIND".to_string(),
    }
}
```

`Check` carries the `ProviderMeta` (or its `arm` and `hold`). After fetching `latest`, if `arm` is `Some` and an `ArmClient::new()` succeeded once (lazily, first use; failure → `None` for all), call `provider_api_versions(first enabled subscription, namespace)` and compute `arm_confirms = Some(resource_types.iter().all(|t| versions.get(*t).is_some_and(|v| v.iter().any(|x| x == latest))))`. Text output prints the hold reason on a second indented line for `held` rows; JSON rows gain `"hold_reason"` when held. `held` never sets `behind`.

- [ ] **Step 6: Docs and smoke**

CHANGELOG: change the CognitiveServices line to `Microsoft.CognitiveServices 2026-05-01 (the newest version Azure registers for project connections; 2026-07-01 changed nothing rigg uses and is tracked as a documented hold by \`rigg dev api-check\`)`. README "Resource Kinds" section: replace the single-version sentence with one pointing at the provider table and `rigg dev api-check`. Then the live smoke, from `e2e-test/`: `cargo build -q --manifest-path ../Cargo.toml && ../target/debug/rigg status && ../target/debug/rigg diff regulus && ../target/debug/rigg dev api-check` — all read-only. Expected: status lists dev and staging with resources in sync; diff shows no drift (or only known content drift); api-check shows CognitiveServices `held` with the reason line.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "fix(registry): hold CognitiveServices ARM at 2026-05-01 (connections cap); api-check verifies against ARM registration

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Execution record (2026-09-10)

Executed on branch `rigg-2`, commits 3ab15aa..e5e50aa (15 commits). Rulings made by the controller during execution:

- Workspace: in-place on branch `rigg-2` (not main). Ruling: no separate worktree — Kristofer's `rigg` symlink points at this checkout's target/debug, and a worktree would build elsewhere; cost if wrong: none beyond convention.
- | T2 guard test vs Global gate | `no_api_version_literals` fails until T3+T4 land, but every commit must pass the gate | CONFLICT — Ruling: T2 adds the guard with `#[ignore = "enabled in Task 4 once client.rs and sync.rs are migrated"]`; T4 removes the ignore. Cost if wrong: one extra edit. |
- Task 1: implemented 3ab15aa (DONE_WITH_CONCERNS: test literals 'cosmosdb' kept as rejected inputs — Ruling: acceptable, the grep expectation targeted implementation code; README auth-doctor sentence updated beyond file list — accepted)
- Task 4: BASE 88eaa2d. Ruling: the literal guard's regex is narrowed to the URL form `api-version=20\d\d-\d\d-\d\d` (the spec's wording) so bare date strings in dev.rs's version_ordering test stop matching; remaining URL-form hits (arm.rs test assertion, error.rs) are rewritten to use registry constants. Cost if wrong: a bare-date literal could slip in unguarded — the provider_table test still pins values.
- Task 4: review — Important (plan-mandated): list() has no guard against a non-terminating @odata.nextLink chain. Ruling: real and cheap — add a max-pages cap (1000) and a same-link cycle check, error out with a clear message; fix round 1. Cost if wrong: none (a legit listing never hits 1000 pages).
- Task 6: review — Important (plan-mandated): canary misfires on every Foundry ARM kind (schema_definition names sub-objects; ARM envelope keys never in fixture). Ruling: restrict the canary to Domain::Search kinds (return empty for others) and add a unit test with scaffold_deployment; also apply Minor 2 (assert fixture "version" matches the registry constant in SchemaFixture::parse) in the same fix round because it is one line and protects the drift detector itself. Cost if wrong: Foundry kinds lose the canary (acceptable — their ARM schema rarely adds top-level keys).
- Ruling: ARM_COGNITIVE_API_VERSION = 2026-05-01 — the newest version ARM registers for EVERY CognitiveServices resource type rigg uses (connections cap it; the 2026-05-01→2026-07-01 diff had no changes for rigg). The watchdog must not report this as BEHIND forever: new Task 8 adds an ARM-registration check to api-check (per provider: namespace + resource types; "held" when the spec repo is ahead but ARM does not register the newer version for all listed types). Smoke test command corrected to `rigg status` + `rigg diff regulus` (both read-only). README "Resource Kinds" API-version sentence to be fixed in Task 8. Cost if wrong: a version bump is one constant edit later.
- Task 8: implemented e2ed477 (DONE; live smoke green: status/diff/api-check). Deviation: ResourcesArm.spec_path set to None to satisfy every_arm_provider_declares_its_registration — Ruling: restore the spec_path (keep watchdog coverage of the subscriptions API) and exempt ResourcesArm in that test with a comment; handled in the fix round together with review findings. Cost if wrong: none.
- Final review (opus): mergeable after fixes. Important 1: CS schema fixture pinned at 2026-07-01 while constant is 2026-05-01 → latent panic. Important 2: KnowledgeSource networkAccessMode immutable entry on a stable-channel kind where the field cannot exist. Ruling: single fix wave covering I1 (regenerate fixture at 2026-05-01 from scratchpad cs-2026-05-01.json, rename include_str + regenerate list), I2 (drop the immutable entry; KnowledgeSource stays on the stable channel; changelog line removed), Minor 3 (CLAUDE.md:57), Minor 5 (migrate guard + help text), Minor 6 (CHANGELOG removed library API), Minor 9 (forward-looking comments), Minor 10 (arm_resources doc comment). Deferred to the polish pass: Minor 4 (deep-path registry guard), 7, 8 and the can-wait ledger items. Cost if wrong: none of the deferred items affect the supported path.

Deferred to a later polish pass (final review: can wait):

- Task 1: minor (deferred): validate.rs warn_missing_deletion_tracking `integrated_sql` branch is now unreachable dead logic — clean up in a later pass.
- Task 2: minor (deferred): stale #[ignore] reason on no_version_literals guard — Task 4 removes the ignore anyway.
- Task 3: minor (deferred): list_web_sites/find_web_site_id duplicate the per-subscription sites URL loop (pre-existing).
- Task 4: minor (deferred): mount_empty_lists_except duplicates mock_empty_lists' kind list.
- Task 5: minor (deferred): route-versioned JSON rows hardcode channel "stable".
- Task 6: minor (deferred): KS read_only_fields completeness unguarded (registry test idea); Indexer read_only_fields now empty — hand-pasted status fields would reach PUT; parse_provider substring match too loose; api_fixture uses compile-time CARGO_MANIFEST_DIR; SchemaFixture::parse panics on malformed fixture; diff_definitions subtype folding undocumented.
- Task 7: minor (deferred): CLAUDE.md sentence carries an extra file-path clause (accurate).
