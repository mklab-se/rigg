# Workstream 1: Environments and infrastructure bindings — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An environment in `rigg.yaml` declares the infrastructure it is made of (bindings), every resource file's infrastructure references are recognised through a registry table, `rigg validate` catches a file that points at another environment's infrastructure, and the bindings can be declared by hand or learned from files and from Azure.

**Architecture:** `rigg-core` gains `binding.rs` (binding types, values, cache, implicit bindings) and `infra.rs` (forms: parse a JSON value into a physical resource, render a new physical resource into the same shape, extract all references of a document, classify against environments). The registry gains an `InfraRef` table per kind. `rigg-client` resolves binding names to ARM ids per tenant/subscription. The CLI gains `rigg env bind|unbind|bind --learn`, richer `env add`/`env show`, validate rules, and learn offers after adopt/pull. The 1.x pin machinery stays until workstream 2 (promote) replaces it.

**Tech Stack:** Rust 2024 (MSRV 1.88), serde/serde_yaml_ng, serde_json, reqwest 0.12, wiremock + assert_cmd, the `ask` primitives from workstream 4a.

**Spec:** `docs/superpowers/specs/2026-09-09-environment-bindings-design.md` (all sections); question protocol from `2026-09-09-interaction-model-design.md`.

## Global Constraints

- Branch `rigg-2`; commit after every task with trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Gate before every commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- `rigg.yaml` 2.0 shape (exact keys): per environment `default`, `tenant`, `subscription`, `search: { service, endpoint?, api-version?, preview-api-version? }`, `foundry: { account, project, endpoint?, api-version? }`, `policy: { protected, strict-bindings }`, `dependencies: { <name>: { <type>: <value> } }` with types `storage | ai-services | function-app | identity | key-vault | api`. Exactly one `search` and one `foundry` target per environment (no lists); `project.yaml` has no connection pins.
- Implicit bindings `search` (the Search service) and `foundry` (the Foundry account) exist in every environment that has the target.
- Binding names: `[a-z0-9][a-z0-9-]*`; `search` and `foundry` are reserved.
- Validation classes: Bound, Shared, Leak (error), Unbound (warning; error when `strict-bindings`, which defaults to `protected`), External (same as Unbound).
- Files stay physical: no templating, no rewriting on push.
- Binding cache: `.rigg/<env>/bindings.json` (gitignored via the existing `.rigg/` ignore).
- Question ids: `binding.<env>.<name>` (choice), `env.<env>.tenant|subscription|search|foundry|protected`, `learn.<env>.<proposed-name>` (text, default = proposed name; answer `skip` skips); register prefixes `binding.`, `env.`, `learn.` in `ask::KNOWN_ID_PREFIXES`.
- ARM test override: `RIGG_ARM_ENDPOINT` replaces `https://management.azure.com` in `ArmClient` (tests only; documented as internal).
- No api-version literals outside the registry (guard test).

---

### Task 1: Workspace model 2.0 — targets, tenant/subscription, policy, dependencies

**Files:**
- Modify: `crates/rigg-core/src/workspace.rs` (whole model), `crates/rigg-core/src/lib.rs` (`pub mod binding;` added in Task 2 — leave for now), `crates/rigg/src/commands/remote.rs:29-33`, `crates/rigg/src/commands/doctor.rs:36-37`, `crates/rigg/src/commands/push.rs:782,796,1363,1377`, `crates/rigg/src/commands/env.rs` (print_env, add writer), `crates/rigg/src/commands/init.rs` (yaml writer), `samples/rigg.yaml`, `crates/rigg/tests/sync.rs` + `cli_surface.rs` fixtures if any use list-form connections (grep `- name:` / `search-connection`)
- Test: `workspace.rs` inline; `cli_surface.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct Environment { pub default: bool, pub tenant: Option<String>, pub subscription: Option<String>, pub search: Option<SearchConnection>, pub foundry: Option<FoundryConnection>, pub policy: Policy, pub dependencies: BTreeMap<String, Binding> }
  pub struct Policy { pub protected: bool, #[serde(rename = "strict-bindings")] pub strict_bindings: Option<bool> }
  impl Policy { pub fn strict_bindings(&self) -> bool { self.strict_bindings.unwrap_or(self.protected) } }
  pub struct Binding { pub kind: BindingType, pub value: String }   // (de)serialized as a one-key map { "<type>": "<value>" }
  pub enum BindingType { Storage, AiServices, FunctionApp, Identity, KeyVault, Api }  // Display/FromStr with the kebab names
  pub struct ProjectManifest { pub description: Option<String> }
  impl ResolvedEnv { pub fn search(&self) -> Option<&SearchConnection>; pub fn foundry(&self) -> Option<&FoundryConnection>; pub fn protected(&self) -> bool; pub fn strict_bindings(&self) -> bool }
  pub fn validate_binding_name(name: &str) -> Result<(), String>;
  ```
- Removes: `ConnectionList`, `WorkspaceError::{AmbiguousConnection, MissingConnection, UnknownConnection}`, `ResolvedEnv::{search_for, foundry_for, has_search, has_foundry}`, `ProjectManifest::{search_connection, foundry_connection}`, `Defaults`/`defaults:`.

- [ ] **Step 1: Write the failing tests**

Replace the workspace tests that exercise multi-connection pins (`parses_multi_connection_env_and_requires_pin`, `missing_connection_errors`) with:

```rust
fn ws_yaml_bindings() -> &'static str {
    r#"
environments:
  dev:
    default: true
    tenant: 45943588-b4fb-4765-ae17-76638c45bb5c
    subscription: 00000000-0000-0000-0000-000000000000
    search: { service: mklabsrch }
    foundry: { account: mklabaifndr, project: proj-default }
    dependencies:
      docs-storage: { storage: mklabstorageacc }
      enrich-fn: { function-app: mklab }
      partner: { api: https://api.partner.example/v1 }
  prod:
    policy: { protected: true }
    search: { service: mklabsrch-prod }
    dependencies:
      docs-storage: { storage: /subscriptions/0b1d/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/mklabstorageprod }
"#
}

#[test]
fn parses_targets_tenant_subscription_and_dependencies() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = make_ws(tmp.path(), ws_yaml_bindings(), &[("p", "{}\n")]);
    let dev = ws.resolve_env(Some("dev")).unwrap();
    assert_eq!(dev.search().unwrap().service, "mklabsrch");
    assert_eq!(dev.foundry().unwrap().project, "proj-default");
    assert_eq!(dev.env.subscription.as_deref(), Some("00000000-0000-0000-0000-000000000000"));
    let b = &dev.env.dependencies["docs-storage"];
    assert_eq!(b.kind, BindingType::Storage);
    assert_eq!(b.value, "mklabstorageacc");
    assert_eq!(dev.env.dependencies["partner"].kind, BindingType::Api);
    let prod = ws.resolve_env(Some("prod")).unwrap();
    assert!(prod.foundry().is_none());
    assert!(prod.protected() && prod.strict_bindings(), "strict-bindings defaults to protected");
    assert!(!dev.strict_bindings());
}

#[test]
fn binding_round_trips_as_a_one_key_map() {
    let b: Binding = serde_yaml::from_str("key-vault: mklabkv").unwrap();
    assert_eq!(b.kind, BindingType::KeyVault);
    assert_eq!(serde_yaml::to_string(&b).unwrap().trim(), "key-vault: mklabkv");
    assert!(serde_yaml::from_str::<Binding>("cosmos: x").is_err(), "unknown type rejected");
    assert!(serde_yaml::from_str::<Binding>("storage: a\nidentity: b").is_err(), "exactly one key");
}

#[test]
fn binding_names_are_validated_and_reserved() {
    assert!(validate_binding_name("docs-storage").is_ok());
    assert!(validate_binding_name("Docs").is_err());
    assert!(validate_binding_name("search").is_err());
    assert!(validate_binding_name("foundry").is_err());
}

#[test]
fn list_form_targets_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(WORKSPACE_FILE), "environments:\n  dev:\n    search:\n      - service: a\n").unwrap();
    assert!(matches!(Workspace::load(tmp.path()), Err(WorkspaceError::Parse { .. })));
}
```

`cli_surface.rs`: a test that a `project.yaml` with `search-connection: x` fails to load with a message naming the removed key (serde `deny_unknown_fields` gives it): `rigg status` → failure, stderr contains `search-connection`.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg-core workspace::`
Expected: compile errors.

- [ ] **Step 3: Implement the model**

`workspace.rs`: replace `ConnectionList` uses with `Option<…>`; add `tenant`, `subscription`, `dependencies`; `Policy` as above (`#[serde(deny_unknown_fields)]`, `is_default` updated); `BindingType` with `Display`/`FromStr` over `["storage","ai-services","function-app","identity","key-vault","api"]`; `Binding` with manual serde:

```rust
impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let map = BTreeMap::<String, String>::deserialize(d)?;
        if map.len() != 1 {
            return Err(serde::de::Error::custom("a binding is exactly one `<type>: <value>` pair"));
        }
        let (k, value) = map.into_iter().next().expect("one entry");
        let kind = k.parse::<BindingType>().map_err(serde::de::Error::custom)?;
        if value.trim().is_empty() {
            return Err(serde::de::Error::custom(format!("binding `{k}` has an empty value")));
        }
        Ok(Binding { kind, value })
    }
}
impl Serialize for Binding { /* one-key map */ }
```

Validate binding names in `Workspace::load` (error `WorkspaceError::Parse`-like `InvalidBindingName { env, name, reason }`). `ProjectManifest` keeps `description` only (`deny_unknown_fields` stays, so old pins error clearly). `ResolvedEnv::search()/foundry()` return `self.env.search.as_ref()` etc. Delete `pick_connection`, the three connection errors, `Defaults`.

Callers: `remote.rs` `Remote::for_project(env, _project)` → `search_conn: env.search().cloned()`; `doctor.rs` → `env.search()` / `env.foundry()`; `push.rs` → `env.search().map(|c| c.service.clone())` and `resolve_cross_service_refs(env.search(), …)`; `env.rs::print_env` iterates the options; `env add`/`init` writers unchanged in shape (they already write single mappings) but `init` now also writes `tenant:` and `subscription:` when `AzCliAuth::check_status()` returns them (`AuthStatus.subscription_id`; tenant from `az account show` — add `tenant_id` to `AuthStatus` in `auth.rs`, parsed from the `tenantId` field).

- [ ] **Step 4: Run the gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: green; grep `search_for\|foundry_for\|ConnectionList` in `crates/` returns nothing.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat!(workspace): rigg.yaml 2.0 — single targets, tenant/subscription, policy.strict-bindings, dependencies bindings

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Binding values, implicit bindings and the resolution cache (rigg-core `binding.rs`)

**Files:**
- Create: `crates/rigg-core/src/binding.rs`
- Modify: `crates/rigg-core/src/lib.rs` (`pub mod binding;`), `crates/rigg-core/src/workspace.rs` (move `Binding`/`BindingType` into `binding.rs` and re-export from `workspace` for callers: `pub use crate::binding::{Binding, BindingType};`)
- Test: inline

**Interfaces:**
- Produces:
  ```rust
  pub enum BindingValue { Name(String), ArmId(String), Url(String) }
  impl Binding { pub fn value(&self) -> BindingValue; pub fn physical_name(&self) -> String /* Name → as is (lowercased); ArmId → last path segment; Url → host (lowercased) */; pub fn arm_id(&self) -> Option<&str>; }
  pub fn arm_resource_name(id: &str) -> Option<&str>;               // last segment
  pub fn arm_subscription(id: &str) -> Option<&str>;                // /subscriptions/{s}/…
  pub fn arm_resource_group(id: &str) -> Option<&str>;
  #[derive(Serialize, Deserialize)] pub struct ResolvedBinding { pub name: String, pub kind: BindingType, pub physical_name: String, pub arm_id: Option<String>, pub subscription: Option<String>, pub resource_group: Option<String>, pub location: Option<String>, pub endpoint: Option<String>, pub resolved_at: String /* RFC3339 */ }
  #[derive(Default, Serialize, Deserialize)] pub struct BindingCache { pub bindings: BTreeMap<String, ResolvedBinding> }
  impl BindingCache { pub fn path(ws: &Workspace, env: &str) -> PathBuf; pub fn load(ws, env) -> BindingCache; pub fn save(&self, ws, env) -> io::Result<()>; pub fn get(&self, name: &str) -> Option<&ResolvedBinding>; }
  /// One environment's binding table: declared dependencies + implicit `search`/`foundry`.
  pub struct EnvBindings { pub env: String, entries: BTreeMap<String, BindingEntry> }
  pub struct BindingEntry { pub name: String, pub kind: BindingKind, pub physical_name: String, pub declared: Option<Binding>, pub resolved: Option<ResolvedBinding> }
  pub enum BindingKind { Declared(BindingType), ImplicitSearch, ImplicitFoundry }
  impl EnvBindings {
      pub fn of(ws: &Workspace, env_name: &str, env: &Environment, cache: Option<&BindingCache>) -> EnvBindings;
      pub fn get(&self, name: &str) -> Option<&BindingEntry>;
      pub fn iter(&self) -> impl Iterator<Item = &BindingEntry>;
      /// Entries whose kind accepts `wanted` (BindingType::AiServices also matches ImplicitFoundry; a Search-endpoint lookup matches ImplicitSearch) and whose physical name equals `physical` (case-insensitive).
      pub fn find_physical(&self, wanted: Wanted, physical: &str) -> Option<&BindingEntry>;
  }
  pub enum Wanted { Type(BindingType), SearchService, ModelHost /* ai-services OR implicit foundry */ }
  ```

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn binding_value_forms_and_physical_names() {
    let name = Binding { kind: BindingType::Storage, value: "MKLabStorage".into() };
    assert!(matches!(name.value(), BindingValue::Name(_)));
    assert_eq!(name.physical_name(), "mklabstorage");
    let id = Binding { kind: BindingType::Storage, value: "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct".into() };
    assert_eq!(id.physical_name(), "acct");
    assert_eq!(arm_subscription(id.arm_id().unwrap()), Some("s"));
    assert_eq!(arm_resource_group(id.arm_id().unwrap()), Some("rg"));
    let api = Binding { kind: BindingType::Api, value: "https://Api.Partner.example/v1/".into() };
    assert!(matches!(api.value(), BindingValue::Url(_)));
    assert_eq!(api.physical_name(), "api.partner.example");
}

#[test]
fn env_bindings_include_implicit_search_and_foundry_and_match_model_hosts() {
    let env = Environment { search: Some(SearchConnection { service: "mklabsrch".into(), ..Default::default() }), foundry: Some(FoundryConnection { account: "mklabaifndr".into(), project: "p".into(), ..Default::default() }), dependencies: [("enrichment".to_string(), Binding { kind: BindingType::AiServices, value: "mklabaisrvc".into() })].into_iter().collect(), ..Default::default() };
    let b = EnvBindings::of_env("dev", &env, None);
    assert_eq!(b.get("search").unwrap().physical_name, "mklabsrch");
    assert!(matches!(b.get("foundry").unwrap().kind, BindingKind::ImplicitFoundry));
    assert_eq!(b.find_physical(Wanted::ModelHost, "MKLABAIFNDR").unwrap().name, "foundry");
    assert_eq!(b.find_physical(Wanted::ModelHost, "mklabaisrvc").unwrap().name, "enrichment");
    assert!(b.find_physical(Wanted::Type(BindingType::Storage), "x").is_none());
}

#[test]
fn cache_round_trips_under_state_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(WORKSPACE_FILE), "environments:\n  dev:\n    default: true\n    search: { service: s }\n").unwrap();
    let ws = Workspace::load(tmp.path()).unwrap();
    let mut c = BindingCache::default();
    c.bindings.insert("docs".into(), ResolvedBinding { name: "docs".into(), kind: BindingType::Storage, physical_name: "acct".into(), arm_id: Some("/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct".into()), subscription: Some("s".into()), resource_group: Some("rg".into()), location: Some("swedencentral".into()), endpoint: None, resolved_at: "2026-09-10T00:00:00Z".into() });
    c.save(&ws, "dev").unwrap();
    assert!(BindingCache::path(&ws, "dev").ends_with(".rigg/dev/bindings.json"));
    assert_eq!(BindingCache::load(&ws, "dev").get("docs").unwrap().resource_group.as_deref(), Some("rg"));
}
```

(`SearchConnection`/`FoundryConnection`/`Environment` need `Default` derives — add them; `EnvBindings::of_env(name, &Environment, cache)` is the workspace-free constructor; `of(ws, env_name, …)` loads the cache and delegates.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg-core binding::`
Expected: compile errors.

- [ ] **Step 3: Implement**

`binding.rs` as specified. `BindingValue` detection: starts with `/subscriptions/` → `ArmId`; starts with `http://`/`https://` → `Url`; else `Name`. `physical_name` lowercases (Azure resource names are case-insensitive). Cache uses `ws.files_root().join(STATE_DIR).join(env).join("bindings.json")`. `EnvBindings::of_env` inserts the two implicit entries first (physical = service / account), then declared ones; `find_physical` matches by `Wanted`: `Type(t)` → `Declared(t)`; `SearchService` → `ImplicitSearch`; `ModelHost` → `Declared(AiServices)` or `ImplicitFoundry`. Include `resolved` from the cache when present.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "feat(core): binding values, implicit search/foundry bindings, resolution cache

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Registry `InfraRef` table and `infra.rs` — parse, render, extract, classify

**Files:**
- Create: `crates/rigg-core/src/infra.rs`
- Modify: `crates/rigg-core/src/registry.rs` (`InfraRef`, `InfraForm`, `infra_refs(kind)`, table per spec §2.3), `crates/rigg-core/src/lib.rs`
- Test: inline (`infra.rs`, `registry.rs`)

**Interfaces:**
- Produces (registry):
  ```rust
  pub enum InfraForm { StorageResourceId, UserAssignedIdentity, OpenAiEndpoint, AiServicesSubdomain, ApiUri, KeyVaultUri, SearchKbMcpUrl }
  pub struct InfraRef { pub path: &'static str, pub form: InfraForm }
  pub fn infra_refs(kind: ResourceKind) -> &'static [InfraRef];
  ```
- Produces (`infra.rs`):
  ```rust
  pub enum Target { Storage, Identity, ModelHost, AiServices, FunctionApp, Api, KeyVault, SearchService }
  pub struct PhysicalRef { pub target: Target, pub physical: String /* lowercase name or host */, pub original: Value, pub kb_name: Option<String> /* SearchKbMcpUrl */ }
  pub fn parse(form: InfraForm, value: &Value) -> Option<PhysicalRef>;   // None when absent/null/placeholder/unrecognised
  pub fn render(form: InfraForm, original: &Value, target: &RenderTarget) -> Result<Value, String>;
  pub struct RenderTarget { pub physical: String, pub arm_id: Option<String>, pub base_url: Option<String>, pub kb_name: Option<String> }
  pub struct FoundRef { pub path: String /* concrete, e.g. skills[2].uri */, pub form: InfraForm, pub physical: PhysicalRef }
  pub fn extract(kind: ResourceKind, doc: &Value) -> Vec<FoundRef>;
  pub enum Class { Bound(String), Shared(String, Vec<String>), Leak { binding: String, envs: Vec<String> }, Unbound, External }
  pub struct Classified { pub found: FoundRef, pub class: Class }
  pub fn classify(this_env: &EnvBindings, other_envs: &[EnvBindings], refs: Vec<FoundRef>) -> Vec<Classified>;
  ```
  Form rules:
  | form | parse | render |
  |---|---|---|
  | StorageResourceId | `ResourceId=/subscriptions/…/storageAccounts/NAME[/];tail` → Storage NAME (placeholders with `<` → None) | requires `arm_id`; `ResourceId={arm_id};{tail}` keeping the original tail after the first `;` |
  | UserAssignedIdentity | object with `userAssignedIdentity: /…/userAssignedIdentities/NAME` → Identity NAME; null/absent → None | requires `arm_id`; same object with `userAssignedIdentity` replaced |
  | OpenAiEndpoint | `https://NAME.(openai.azure.com|cognitiveservices.azure.com|services.ai.azure.com)[/…]` → ModelHost NAME | same suffix and path with NAME swapped |
  | AiServicesSubdomain | same hosts → AiServices NAME | same |
  | ApiUri | `https://SITE.azurewebsites.net/…` → FunctionApp SITE; any other `http(s)://HOST/…` → Api HOST | FunctionApp: swap host to `{physical}.azurewebsites.net`; Api: replace the matched `base_url` prefix with the target `base_url` |
  | KeyVaultUri | `https://NAME.vault.azure.net[/…]` → KeyVault NAME | swap NAME |
  | SearchKbMcpUrl | `https://SVC.search.windows.net/knowledgebases/KB/mcp?…` (case-insensitive segments) → SearchService SVC, `kb_name = KB` | `https://{physical}.search.windows.net/knowledgebases/{kb_name}/mcp?api-version={SEARCH_PREVIEW_API_VERSION}` |
  `classify`: for each ref, `Wanted` from `Target` (Storage→Type(Storage), Identity→Type(Identity), ModelHost→ModelHost, AiServices→ModelHost, FunctionApp→Type(FunctionApp), Api→Type(Api) with prefix match on `base_url`, KeyVault→Type(KeyVault), SearchService→SearchService). Bound when `this_env.find_physical` hits; Shared additionally when any other env's table has the same physical for the same `Wanted`; Leak when this env misses but another env hits (list those envs and the binding name there); Unbound when nobody hits and the target is not Api; External when target is Api and nobody hits.

- [ ] **Step 1: Write the failing tests**

`infra.rs`:

```rust
#[test]
fn parse_and_render_every_form() {
    // storage
    let v = json!("ResourceId=/subscriptions/S/resourceGroups/RG/providers/Microsoft.Storage/storageAccounts/MKLabAcct/;Database=x");
    let p = parse(InfraForm::StorageResourceId, &v).unwrap();
    assert_eq!((p.target, p.physical.as_str()), (Target::Storage, "mklabacct"));
    let out = render(InfraForm::StorageResourceId, &v, &RenderTarget { physical: "prodacct".into(), arm_id: Some("/subscriptions/P/resourceGroups/PRG/providers/Microsoft.Storage/storageAccounts/prodacct".into()), base_url: None, kb_name: None }).unwrap();
    assert_eq!(out, json!("ResourceId=/subscriptions/P/resourceGroups/PRG/providers/Microsoft.Storage/storageAccounts/prodacct;Database=x"));
    assert!(parse(InfraForm::StorageResourceId, &json!("ResourceId=/subscriptions/<sub>/…")).is_none());
    // identity
    let id = json!({"@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity", "userAssignedIdentity": "/subscriptions/S/resourcegroups/RG/providers/Microsoft.ManagedIdentity/userAssignedIdentities/Rigg-Dev"});
    assert_eq!(parse(InfraForm::UserAssignedIdentity, &id).unwrap().physical, "rigg-dev");
    assert!(parse(InfraForm::UserAssignedIdentity, &json!(null)).is_none());
    // openai endpoint keeps path
    let e = json!("https://MKLabAIFNDR.openai.azure.com/");
    let p = parse(InfraForm::OpenAiEndpoint, &e).unwrap();
    assert_eq!((p.target, p.physical.as_str()), (Target::ModelHost, "mklabaifndr"));
    assert_eq!(render(InfraForm::OpenAiEndpoint, &e, &RenderTarget { physical: "prodaifndr".into(), arm_id: None, base_url: None, kb_name: None }).unwrap(), json!("https://prodaifndr.openai.azure.com/"));
    // function vs external api
    let f = json!("https://mklab.azurewebsites.net/api/enrich?code=<redacted>");
    let p = parse(InfraForm::ApiUri, &f).unwrap();
    assert_eq!((p.target, p.physical.as_str()), (Target::FunctionApp, "mklab"));
    assert_eq!(render(InfraForm::ApiUri, &f, &RenderTarget { physical: "mklab-prod".into(), arm_id: None, base_url: None, kb_name: None }).unwrap(), json!("https://mklab-prod.azurewebsites.net/api/enrich?code=<redacted>"));
    let x = json!("https://api.partner.example/v1/enrich");
    assert_eq!(parse(InfraForm::ApiUri, &x).unwrap().target, Target::Api);
    assert_eq!(render(InfraForm::ApiUri, &x, &RenderTarget { physical: "api.partner-prod.example".into(), arm_id: None, base_url: Some("https://api.partner-prod.example/v2".into()), kb_name: None }).unwrap_or(json!(null)), json!("https://api.partner-prod.example/v2/enrich"));
    // key vault
    assert_eq!(parse(InfraForm::KeyVaultUri, &json!("https://mklabkv.vault.azure.net/keys/k/1")).unwrap().physical, "mklabkv");
    // kb mcp
    let m = json!("https://mklabsrch.search.windows.net/knowledgeBases/regulatory-kb/mcp?api-version=old");
    let p = parse(InfraForm::SearchKbMcpUrl, &m).unwrap();
    assert_eq!((p.target, p.physical.as_str(), p.kb_name.as_deref()), (Target::SearchService, "mklabsrch", Some("regulatory-kb")));
    let r = render(InfraForm::SearchKbMcpUrl, &m, &RenderTarget { physical: "mklabsrch-prod".into(), arm_id: None, base_url: None, kb_name: Some("regulatory-kb".into()) }).unwrap();
    assert_eq!(r, json!(format!("https://mklabsrch-prod.search.windows.net/knowledgebases/regulatory-kb/mcp?api-version={}", crate::registry::SEARCH_PREVIEW_API_VERSION)));
}

#[test]
fn extract_walks_arrays_with_concrete_paths() {
    let skillset = json!({"name": "ss", "skills": [
        {"@odata.type": "#Microsoft.Skills.Text.SplitSkill"},
        {"@odata.type": "#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill", "resourceUri": "https://mklabaifndr.openai.azure.com"},
        {"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill", "uri": "https://mklab.azurewebsites.net/api/x"}
    ], "cognitiveServices": {"@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity", "subdomainUrl": "https://mklabaisrvc.cognitiveservices.azure.com/"}});
    let refs = extract(ResourceKind::Skillset, &skillset);
    let paths: Vec<&str> = refs.iter().map(|r| r.path.as_str()).collect();
    assert!(paths.contains(&"skills[1].resourceUri"), "{paths:?}");
    assert!(paths.contains(&"skills[2].uri"), "{paths:?}");
    assert!(paths.contains(&"cognitiveServices.subdomainUrl"), "{paths:?}");
    assert_eq!(refs.len(), 3);
}

#[test]
fn classify_bound_shared_leak_unbound_external() {
    let dev = EnvBindings::of_env("dev", &env_with(&[("docs", BindingType::Storage, "acct"), ("fn", BindingType::FunctionApp, "mklab")], "mklabsrch", "mklabaifndr"), None);
    let prod = EnvBindings::of_env("prod", &env_with(&[("docs", BindingType::Storage, "acct"), ("fn", BindingType::FunctionApp, "mklab-prod")], "mklabsrch-prod", "mklabaifndr-prod"), None);
    let refs = vec![
        found(InfraForm::StorageResourceId, "credentials.connectionString", Target::Storage, "acct"),
        found(InfraForm::ApiUri, "skills[0].uri", Target::FunctionApp, "mklab-prod"),
        found(InfraForm::OpenAiEndpoint, "skills[1].resourceUri", Target::ModelHost, "mklabaifndr"),
        found(InfraForm::KeyVaultUri, "encryptionKey.keyVaultUri", Target::KeyVault, "kv"),
        found(InfraForm::ApiUri, "skills[2].uri", Target::Api, "api.partner.example"),
    ];
    let out = classify(&dev, &[prod], refs);
    assert!(matches!(&out[0].class, Class::Shared(b, envs) if b == "docs" && envs == &vec!["prod".to_string()]));
    assert!(matches!(&out[1].class, Class::Leak { binding, envs } if binding == "fn" && envs == &vec!["prod".to_string()]));
    assert!(matches!(&out[2].class, Class::Bound(b) if b == "foundry"));
    assert!(matches!(out[3].class, Class::Unbound));
    assert!(matches!(out[4].class, Class::External));
}
```

(`env_with(..)` and `found(..)` are small test helpers you write in the test module.)

`registry.rs` test:

```rust
#[test]
fn infra_ref_table_matches_the_spec() {
    let ks: Vec<&str> = infra_refs(ResourceKind::KnowledgeSource).iter().map(|r| r.path).collect();
    for p in ["azureBlobParameters.connectionString", "azureBlobParameters.ingestionParameters.identity", "azureBlobParameters.ingestionParameters.embeddingModel.azureOpenAIParameters.resourceUri", "azureBlobParameters.ingestionParameters.chatCompletionModel.azureOpenAIParameters.resourceUri", "azureBlobParameters.ingestionParameters.aiServices.uri", "azureBlobParameters.ingestionParameters.assetStore.connectionString", "encryptionKey.keyVaultUri"] {
        assert!(ks.contains(&p), "missing {p}");
    }
    assert!(infra_refs(ResourceKind::Deployment).is_empty());
    assert_eq!(infra_refs(ResourceKind::Agent)[0].path, "tools[].server_url");
}
```

Also extend the existing `registry_paths_exist_in_the_pinned_schema` test to include `infra_refs(kind)` paths (head segment check, like the others).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg-core infra:: registry::infra_ref`
Expected: compile errors.

- [ ] **Step 3: Implement**

Registry: the full table from spec §2.3 (27 rows across DataSource, Index, Skillset, Indexer, KnowledgeSource, KnowledgeBase, Agent, Connection; Skillset rows are conditional on `@odata.type` — `extract` applies `skills[].resourceUri` only to `AzureOpenAIEmbeddingSkill`, `skills[].uri`/`authIdentity` only to `WebApiSkill`; encode that as `InfraRef { path, form, only_odata_type: Option<&'static str> }`).

`infra.rs`: `extract` walks paths with `[]` expanding to concrete indices (write a small walker returning `(concrete_path, &Value)` pairs; do not reuse `collect_path`, which loses indices). Host parsing: lowercase the host; strip a trailing `/`. `render` for `StorageResourceId` preserves the original's `;`-tail; for URL forms, rebuild with the original path+query. `classify` as in Interfaces.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "feat(core): infrastructure reference table and parse/render/extract/classify

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: `rigg validate` classifies infrastructure references

**Files:**
- Modify: `crates/rigg/src/commands/validate.rs` (warnings vector; per-env classification), `crates/rigg/src/cli.rs` (`ValidateArgs`: `--show-bindings` shows Bound/Shared rows)
- Test: `crates/rigg/tests/cli_surface.rs`

**Interfaces:**
- `validate` JSON: `{ "valid": bool, "problems": [..], "warnings": [..] }`; text prints `✗` problems, `!` warnings, and with `--show-bindings` `✓ bound` / `= shared (prod)` rows.
- Messages (exact prefixes, tested):
  - Leak: `[<file>] <path> references storage 'X', which is bound in environment 'prod' as 'docs-storage' but not in 'dev' — bind it (rigg env bind dev docs-storage storage:X) or fix the file`
  - Unbound: `[<file>] <path> references function-app 'Y', which no environment binds — run \`rigg env bind dev --learn\` to record it`
  - External: `[<file>] <path> calls external API 'https://host' — bind it as an api dependency to track it across environments`

- [ ] **Step 1: Write the failing tests**

```rust
fn workspace_two_envs_with_bindings() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("rigg.yaml"), "environments:\n  dev:\n    default: true\n    search: { service: s-dev }\n    dependencies:\n      docs: { storage: devacct }\n  prod:\n    policy: { protected: true }\n    search: { service: s-prod }\n    dependencies:\n      docs: { storage: prodacct }\n").unwrap();
    let proj = tmp.path().join("projects/demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

fn write_ds(ws: &std::path::Path, env: &str, name: &str, account: &str) {
    let d = ws.join(format!("projects/demo/envs/{env}/search/data-sources"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{name}.json")), format!(r#"{{"name":"{name}","type":"azureblob","credentials":{{"connectionString":"ResourceId=/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/{account};"}},"container":{{"name":"c"}}}}"#)).unwrap();
}

#[test]
fn validate_flags_a_prod_file_pointing_at_dev_storage_as_a_leak() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "prod", "ds", "devacct");
    rigg().current_dir(ws.path()).args(["validate"]).assert().code(3)
        .stdout(predicate::str::contains("bound in environment 'dev' as 'docs' but not in 'prod'"));
}

#[test]
fn validate_warns_on_unbound_in_dev_but_errors_in_protected_prod() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "otheracct");
    rigg().current_dir(ws.path()).args(["validate"]).assert().success()
        .stdout(predicate::str::contains("no environment binds").and(predicate::str::contains("rigg env bind dev --learn")));
    write_ds(ws.path(), "prod", "ds2", "otheracct");
    rigg().current_dir(ws.path()).args(["validate", "--output", "json"]).assert().code(3)
        .stdout(predicate::str::contains("\"valid\": false"));
}

#[test]
fn validate_show_bindings_lists_bound_and_shared() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "devacct");
    rigg().current_dir(ws.path()).args(["validate", "--show-bindings"]).assert().success()
        .stdout(predicate::str::contains("bound 'docs'"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg --test cli_surface validate_`
Expected: FAIL.

- [ ] **Step 3: Implement**

In `validate::run`, build `EnvBindings` for every environment in `ws.config.environments` once (with the cache), then per env/project/file: `infra::extract` + `infra::classify(this, others, refs)`; map classes to problems/warnings with the exact messages above (Unbound/External → problems when `env.policy.strict_bindings()`, else warnings). Keep every existing check. `--show-bindings` (a new `ValidateArgs` flag, not the global `-v`) prints `✓ bound 'docs' (storage devacct)` / `= shared 'docs' with prod` lines.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "feat(validate): classify infrastructure references — leaks are errors, unbound warns (strict in protected envs)

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: ARM resolution per tenant/subscription and the test-only ARM endpoint override

**Files:**
- Modify: `crates/rigg-client/src/auth.rs` (`get_arm_token_for_tenant`, `AuthStatus.tenant_id`), `crates/rigg-client/src/arm.rs` (`ArmClient::for_tenant`, `base_url` field honouring `RIGG_ARM_ENDPOINT`, `resolve_binding`, list helpers for MSI and Key Vault), `crates/rigg-core/src/registry.rs` (nothing — versions already there)
- Create: `crates/rigg/tests/arm_fake.rs` (wiremock ARM fake helpers, reused by later workstreams) — a `pub fn mount_arm_fake(server: &MockServer, subs: &[&str], resources: &[(&str /*type*/, &str /*name*/, &str /*rg*/, &str /*location*/)])` that serves `/subscriptions`, and per-subscription provider listings for `Microsoft.Storage/storageAccounts`, `Microsoft.CognitiveServices/accounts`, `Microsoft.Web/sites`, `Microsoft.ManagedIdentity/userAssignedIdentities`, `Microsoft.KeyVault/vaults`, `Microsoft.Search/searchServices`
- Test: `arm.rs` inline (URL building), `crates/rigg/tests/arm_fake.rs` (`ArmClient` against the fake)

**Interfaces:**
- Produces:
  ```rust
  impl AzCliAuth { pub fn get_arm_token_for_tenant(tenant: Option<&str>) -> Result<String, AuthError> /* az account get-access-token [--tenant T] --resource https://management.azure.com; cache key (tenant, audience) */ }
  impl ArmClient {
      pub fn new() -> Result<Self, ClientError>;                       // default tenant, as today
      pub fn for_tenant(tenant: Option<&str>) -> Result<Self, ClientError>;
      pub fn with_token(token: String) -> Self;                       // exists; now also reads RIGG_ARM_ENDPOINT
      pub fn url(&self, path: &str, provider: Provider) -> String;   // uses self.base_url
      pub async fn resolve_binding(&self, kind: BindingType, value: &str, subscription: Option<&str>) -> Result<ResolvedBinding, ClientError>;
      pub async fn list_user_assigned_identities(&self, subscription_id: &str) -> Result<Vec<ArmResource>, ClientError>;
      pub async fn list_key_vaults(&self, subscription_id: &str) -> Result<Vec<ArmResource>, ClientError>;
  }
  pub struct ArmResource { pub name: String, pub id: String, pub location: String, pub kind: Option<String>, pub endpoint: Option<String> }
  ```
  `resolve_binding`: `ArmId` value → `GET {id}` with the type's provider version to confirm and fill location/endpoint (endpoint: storage `properties.primaryEndpoints.blob`, cognitive `properties.endpoint`, sites `https://{name}.azurewebsites.net`, vault `properties.vaultUri`, MSI: `properties.clientId` in `endpoint`? — no: put MSI `principalId` in a new `principal_id: Option<String>` field of `ResolvedBinding` (add it to the core struct now, `#[serde(default)]`); `Name` value → list the type in `subscription` (or every enabled subscription when None), match case-insensitively; zero → `ClientError::NotFound`; more than one → `ClientError::Api { status: 409, message: "ambiguous: <id1>, <id2> — use the full ARM id" }`. `Url` (api) → no ARM: `ResolvedBinding` with `arm_id: None`, `endpoint: Some(url)`.

- [ ] **Step 1: Write the failing tests**

`arm.rs`:

```rust
#[test]
fn arm_base_url_can_be_overridden_for_tests() {
    temp_env::with_var("RIGG_ARM_ENDPOINT", Some("http://127.0.0.1:1"), || {
        let c = ArmClient::with_token("t".into());
        assert!(c.url("/subscriptions", Provider::ResourcesArm).starts_with("http://127.0.0.1:1/subscriptions?api-version="));
    });
}
```

(add `temp-env = "0.3"` to workspace dev-dependencies, or read the env var in a small `fn arm_base_url() -> String` and test that function with an explicit argument instead — either is fine; prefer the latter to avoid a dependency: `fn base_url_from(env: Option<&str>) -> String`.)

`crates/rigg/tests/arm_fake.rs`:

```rust
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path, path_regex}};
use serde_json::json;
use rigg_client::arm::ArmClient;
use rigg_core::binding::BindingType;

pub async fn mount_arm_fake(server: &MockServer, subs: &[&str], resources: &[(&str, &str, &str, &str)]) {
    Mock::given(method("GET")).and(path("/subscriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": subs.iter().map(|s| json!({"subscriptionId": s, "displayName": s, "state": "Enabled"})).collect::<Vec<_>>()})))
        .mount(server).await;
    for (rt, ns) in [("storageAccounts", "Microsoft.Storage"), ("accounts", "Microsoft.CognitiveServices"), ("sites", "Microsoft.Web"), ("userAssignedIdentities", "Microsoft.ManagedIdentity"), ("vaults", "Microsoft.KeyVault"), ("searchServices", "Microsoft.Search")] {
        for sub in subs {
            let items: Vec<_> = resources.iter().filter(|(t, ..)| *t == rt).map(|(t, name, rg, loc)| json!({
                "name": name, "location": loc, "kind": if *t == "accounts" { "AIServices" } else { "" },
                "id": format!("/subscriptions/{sub}/resourceGroups/{rg}/providers/{ns}/{t}/{name}"),
                "properties": {"endpoint": format!("https://{name}.cognitiveservices.azure.com/"), "vaultUri": format!("https://{name}.vault.azure.net/"), "principalId": "00000000-0000-0000-0000-00000000aaaa", "primaryEndpoints": {"blob": format!("https://{name}.blob.core.windows.net/")}}
            })).collect();
            Mock::given(method("GET")).and(path(format!("/subscriptions/{sub}/providers/{ns}/{rt}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": items}))).mount(server).await;
        }
    }
    Mock::given(method("GET")).and(path_regex(r"^/subscriptions/[^/]+/resourceGroups/[^/]+/providers/.+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "by-id", "location": "swedencentral", "properties": {}})))
        .mount(server).await;
}

#[tokio::test]
async fn resolve_binding_by_name_and_by_id() {
    let server = MockServer::start().await;
    mount_arm_fake(&server, &["sub-a", "sub-b"], &[("storageAccounts", "acct", "rg", "swedencentral"), ("storageAccounts", "dup", "rg1", "x"), ("storageAccounts", "dup", "rg2", "x")]).await;
    unsafe { std::env::set_var("RIGG_ARM_ENDPOINT", server.uri()); }
    let arm = ArmClient::with_token("t".into());
    let r = arm.resolve_binding(BindingType::Storage, "ACCT", Some("sub-a")).await.unwrap();
    assert_eq!(r.resource_group.as_deref(), Some("rg"));
    assert!(r.arm_id.as_deref().unwrap().starts_with("/subscriptions/sub-a/"));
    let err = arm.resolve_binding(BindingType::Storage, "dup", Some("sub-a")).await.unwrap_err();
    assert!(err.to_string().contains("ambiguous"));
    assert!(arm.resolve_binding(BindingType::Storage, "nope", None).await.is_err());
}
```

(Setting a process env var in a test is acceptable here because this test binary owns the process; keep all ARM-fake tests in this one file so they share the variable, and use `#[serial]`-free sequencing by setting it once in each test to the same server per test — wiremock servers are per test, so the tests must not run in parallel: add `--test-threads=1` via a `[[test]]` entry? Simpler: put the override on the client — `ArmClient::with_token_and_base(token, base_url)` — and have `with_token` read the env var. Use `with_token_and_base` in tests; keep the env var for the CLI binary tests in Task 6.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg-client arm_base_url && cargo test -p rigg --test arm_fake`
Expected: FAIL / compile errors.

- [ ] **Step 3: Implement**

`auth.rs`: `get_arm_token_for_tenant(tenant)` appends `--tenant <t>` when given; cache key `format!("{}|{}", tenant.unwrap_or("-"), audience)`; on failure with a tenant, the error message says `run: az login --tenant <t>`. `AuthStatus.tenant_id` from `tenantId`.

`arm.rs`: `base_url: String` field (from `RIGG_ARM_ENDPOINT` or `registry::ARM_BASE_URL`), `for_tenant`, `with_token_and_base`, `url()` uses `self.base_url`; `ArmResource` and the two new list functions (MSI `Provider::ManagedIdentityArm`, KV `Provider::KeyVaultArm`); `resolve_binding` per the Interfaces text; also make `arm_resources.rs` use `ArmClient::url` instead of the constant so the override applies there too.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "feat(client): per-tenant ARM client, binding resolution by name or id, RIGG_ARM_ENDPOINT test override, ARM fake

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: `rigg env` bindings commands, `env add --like`, `env show`, `describe`

**Files:**
- Modify: `crates/rigg/src/cli.rs` (`EnvCommands::{Add, Show, Bind, Unbind}` args), `crates/rigg/src/commands/env.rs`, `crates/rigg/src/commands/discovery.rs` (per-type pick-lists via ARM), `crates/rigg/src/commands/describe.rs` (infrastructure section), `crates/rigg/src/commands/ask.rs` (`KNOWN_ID_PREFIXES` += `binding.`, `env.`, `learn.`)
- Create: `crates/rigg/src/commands/bindings.rs` (learn: scan a tree, group, propose names; shared helpers used by Task 7)
- Test: `cli_surface.rs` (flag forms, learn on a fixture tree, show output), `crates/rigg/tests/sync.rs` or a new `crates/rigg/tests/env_arm.rs` (env show `--refresh` and `env add --like` against the ARM fake via `RIGG_ARM_ENDPOINT`)

**Interfaces:**
- CLI:
  ```
  rigg env add <name> [--tenant T] [--subscription S] [--search-service X] [--foundry-account A --foundry-project P] [--protected] [--bind <name>=<type>:<value>]… [--like <env> [--same <name>]… [--skip <name>]…]
  rigg env bind <env> <name> <type>:<value>          # add/replace one binding
  rigg env bind <env> --learn [--yes]                # propose from files; interactive confirm / questions learn.<env>.<name>
  rigg env unbind <env> <name>
  rigg env show [<env>] [--refresh]                  # targets, policy, bindings with cached ARM ids + "shared with: …"
  ```
- `bindings::learn(ws, env) -> Vec<Proposal { name, kind: BindingType, value: String /* physical name; ARM id when the file carries one */, sources: Vec<(file, path)> }>`; name proposal = physical name lower-kebab (strip non-alnum → `-`), de-duplicated, never `search`/`foundry`.
- `bindings::write_binding(ws_root, env, name, binding)`, `remove_binding(..)` — edit `rigg.yaml` via the existing `edit_workspace_yaml` helper (moved from env.rs into bindings.rs).
- Interactive `env add` without flags: tenant/subscription pick-lists (from `az account list` equivalent: ARM `/tenants` + `/subscriptions`), Search/Foundry pick-lists (existing discovery), then for `--like`: per binding a `Question::choice("binding.<new>.<name>", …, [same as <env> (<value>) | pick from ARM list… | skip])` through `ctx.asker("env add", json!({"env": name, "like": like}))` using `ask_all` where possible (all bindings at once), `protected` as `Question::confirm("env.<name>.protected", …, false)`.
- `describe` text gains, per environment printed, an `Infrastructure:` block listing bindings (`name  type  physical  [shared with prod]`); JSON gains `"infrastructure": [{name, type, value, physical_name, shared_with: []}]`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn env_bind_and_unbind_edit_rigg_yaml() {
    let ws = workspace();
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "docs", "storage:mklabstorageacc"]).assert().success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(yaml.contains("docs:") && yaml.contains("storage: mklabstorageacc"), "{yaml}");
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "search", "storage:x"]).assert().code(2);
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "docs", "cosmos:x"]).assert().code(2);
    rigg().current_dir(ws.path()).args(["env", "unbind", "dev", "docs"]).assert().success();
    assert!(!std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap().contains("docs:"));
}

#[test]
fn env_bind_learn_proposes_from_files_and_writes_with_yes() {
    let ws = workspace();
    write_ds(ws.path(), "dev", "ds", "mklabstorageacc");   // helper from Task 4
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "--learn", "--yes"]).assert().success()
        .stdout(predicate::str::contains("mklabstorageacc").and(predicate::str::contains("storage")));
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(yaml.contains("mklabstorageacc: { storage: mklabstorageacc }") || yaml.contains("storage: mklabstorageacc"), "{yaml}");
    // non-interactive without --yes: needs-input with learn.dev.mklabstorageacc
    write_ds(ws.path(), "dev", "ds2", "otheracct");
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "--learn", "--output", "json"]).assert().code(6)
        .stdout(predicate::str::contains("learn.dev.otheracct"));
}

#[test]
fn env_add_with_like_flags_copies_and_overrides_bindings() {
    let ws = workspace();
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "docs", "storage:devacct"]).assert().success();
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "fn", "function-app:mklab"]).assert().success();
    rigg().current_dir(ws.path()).args(["env", "add", "prod", "--search-service", "s-prod", "--like", "dev", "--same", "fn", "--bind", "docs=storage:prodacct", "--protected"]).assert().success();
    rigg().current_dir(ws.path()).args(["env", "show", "prod"]).assert().success()
        .stdout(predicate::str::contains("docs").and(predicate::str::contains("prodacct")).and(predicate::str::contains("fn")).and(predicate::str::contains("shared with: dev")).and(predicate::str::contains("protected: true")));
}

#[test]
fn describe_lists_infrastructure() {
    let ws = workspace();
    rigg().current_dir(ws.path()).args(["env", "bind", "dev", "docs", "storage:devacct"]).assert().success();
    rigg().current_dir(ws.path()).args(["describe", "--output", "json"]).assert().success()
        .stdout(predicate::str::contains("\"infrastructure\"").and(predicate::str::contains("devacct")));
}
```

ARM-backed (new test file `crates/rigg/tests/env_arm.rs`, reusing `mount_arm_fake` — move the helper into `crates/rigg/tests/common/arm_fake.rs` with `#[path]` includes, or duplicate minimally):

```rust
#[tokio::test]
async fn env_show_refresh_resolves_bindings_against_arm() {
    let server = MockServer::start().await;
    mount_arm_fake(&server, &["sub-a"], &[("storageAccounts", "devacct", "rg", "swedencentral")]).await;
    let ws = workspace(); // from cli_surface-style helper, copied here
    rigg(ws.path()).env("RIGG_ARM_ENDPOINT", server.uri()).env("RIGG_ACCESS_TOKEN", "t")
        .args(["env", "bind", "dev", "docs", "storage:devacct"]).assert().success();
    rigg(ws.path()).env("RIGG_ARM_ENDPOINT", server.uri()).env("RIGG_ACCESS_TOKEN", "t")
        .args(["env", "show", "dev", "--refresh"]).assert().success()
        .stdout(predicate::str::contains("/subscriptions/sub-a/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/devacct"));
    assert!(ws.path().join(".rigg/dev/bindings.json").exists());
}
```

(`ArmClient` must accept `RIGG_ACCESS_TOKEN` as the ARM token when set — check `auth.rs`'s static-token path; if `ArmClient::new()` only uses az CLI, make it honour `RIGG_ACCESS_TOKEN` first, like the data-plane clients.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg --test cli_surface env_ describe_lists && cargo test -p rigg --test env_arm`
Expected: FAIL.

- [ ] **Step 3: Implement**

`cli.rs` per the CLI block. `bindings.rs`: `learn` (uses `infra::extract` over every project's tree for the env; group by `(Target→BindingType, physical)`; skip targets that already resolve to a binding in this env; `Api` proposals use the URL origin as value), `write_binding`, `remove_binding`, `proposals_to_questions(env, proposals) -> Vec<Question>` (text with default = proposed name; `skip` skips). `env.rs`: `bind` (validate name, parse `type:value`, `edit_workspace_yaml`), `unbind`, `show` (build `EnvBindings` with the cache; `--refresh` resolves every declared binding via `ArmClient::for_tenant(env.tenant)` + `resolve_binding(kind, value, env.subscription)` and saves the cache; prints one row per binding `name  type  value → <arm id or physical>` and `shared with: <envs>` computed from the other environments' physical names), `add` with the new flags and the interactive path via `ctx.asker`. `describe`: infrastructure block/JSON. `KNOWN_ID_PREFIXES` extended.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "feat(env): bind/unbind/learn, env add --like, env show with resolved ids and sharing, describe infrastructure

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Learn offers after adopt and pull; `init` records tenant and subscription

**Files:**
- Modify: `crates/rigg/src/commands/adopt.rs` (after the adoption summary), `crates/rigg/src/commands/pull.rs` (after the per-project summary), `crates/rigg/src/commands/init.rs` (tenant/subscription lines)
- Test: `crates/rigg/tests/sync.rs` (adopt then hint / `--yes` learn), `cli_surface.rs` (init flags path writes `subscription:` when `--subscription` given; add `--tenant`/`--subscription` flags to `InitArgs`)

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn adopt_offers_to_learn_bindings_non_interactively_as_a_hint() {
    let server = MockServer::start().await;
    mount_empty_lists_except(&server, "datasources").await;
    Mock::given(method("GET")).and(path("/datasources")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{"name": "ds", "type": "azureblob", "credentials": {"connectionString": "ResourceId=/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct;"}, "container": {"name": "c"}}]}))).mount(&server).await;
    let ws = workspace(&server.uri());
    rigg(ws.path()).args(["adopt", "demo", "all", "--yes"]).assert().success()
        .stderr(predicate::str::contains("rigg env bind dev --learn"));
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(!yaml.contains("dependencies"), "non-interactive adopt does not write bindings");
}
```

`cli_surface.rs`:

```rust
#[test]
fn init_records_tenant_and_subscription_flags() {
    let tmp = tempfile::tempdir().unwrap();
    rigg().current_dir(tmp.path()).args(["init", "--search-service", "s", "--tenant", "t-1", "--subscription", "sub-1"]).assert().success();
    let yaml = std::fs::read_to_string(tmp.path().join("rigg.yaml")).unwrap();
    assert!(yaml.contains("tenant: t-1") && yaml.contains("subscription: sub-1"), "{yaml}");
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg --test sync adopt_offers && cargo test -p rigg --test cli_surface init_records`
Expected: FAIL.

- [ ] **Step 3: Implement**

`adopt.rs`/`pull.rs`: after writing, `bindings::learn(&ws, &env.name)`; if non-empty: interactive → print the proposal table and ask `Question::confirm("learn.<env>.record", "Record these bindings in rigg.yaml?", true)` then write; non-interactive → `eprintln!("hint: {n} infrastructure reference(s) are not bound in '{env}' — run `rigg env bind {env} --learn` to record them")`. `init.rs`: `--tenant`/`--subscription` flags; when absent and az is logged in, take them from `AzCliAuth::check_status()` (`tenant_id`, `subscription_id`); write the two lines under the environment when known.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A
git commit -m "feat: learn bindings after adopt/pull; init records tenant and subscription

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: Docs, samples, live workspace, smoke

**Files:**
- Modify: `CONCEPTS.md` + `crates/rigg/CONCEPTS.md` ("Environments" chapter rewritten around bindings: targets, dependencies, implicit bindings, shared vs different, learn vs declare, validation classes), `README.md` (environments section; `rigg env` commands; `rigg.yaml` example with `dependencies`), `GETTING_STARTED.md` (the "point it at your storage account" step becomes `rigg env bind dev docs storage:<account>` + `rigg new data-source … --storage docs` is Task-later — for now describe the binding then the file edit), `.claude/skills/rigg-guide/SKILL.md`, `CHANGELOG.md`, `samples/rigg.yaml` (a `dependencies:` block with placeholders), `samples/projects/*/envs/demo/**` (unchanged files; verify `rigg validate` on samples passes with warnings only)
- Live: `e2e-test/rigg.yaml` (untracked): run `rigg env bind dev --learn --yes` and `rigg env bind staging --learn --yes` there, then `rigg validate`, `rigg env show dev --refresh`, `rigg status` — all read-only against Azure except the local YAML/cache writes

- [ ] **Step 1: Docs**

Write the chapter and README/GETTING_STARTED updates (copy CONCEPTS to the crate). CHANGELOG `### Added`: bindings, `rigg env bind|unbind|--learn`, `env add --like`, `env show --refresh`, validate classes; `### Changed (breaking)`: single Search/Foundry target per environment, `project.yaml` connection pins removed, `defaults.identity` removed, `rigg.yaml` `tenant`/`subscription`/`policy.strict-bindings`.

- [ ] **Step 2: Samples**

`samples/rigg.yaml` gains:

```yaml
    dependencies:
      docs-storage: { storage: your-storage-account }
      enrich-fn:    { function-app: your-function-app }
```

Run `cargo run -q --bin rigg -- validate` inside `samples/` — must exit 0 (warnings allowed for placeholder values that do not match: the samples' files carry `<subscription-id>` placeholders, which `parse` ignores).

- [ ] **Step 3: Live smoke (authorised; read-only against Azure)**

From `e2e-test/`: `cargo build -q --manifest-path ../Cargo.toml && ../target/debug/rigg env bind dev --learn --yes && ../target/debug/rigg env bind staging --learn --yes && ../target/debug/rigg validate && ../target/debug/rigg env show dev --refresh && ../target/debug/rigg status`. Paste the output in the report. Expected: learn records `mklabstorageacc` (storage) and the model host resolves to the implicit `foundry`; validate passes; show prints resolved ARM ids; status unchanged from before.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add -A   # never e2e-test/
git commit -m "docs: environments and infrastructure bindings — concepts, README, getting started, samples

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Execution record (2026-09-10)

Executed on branch `rigg-2`, commits 6cbe28b..a0fefd8. Rulings:

- | T5 → T6 | ArmClient::{for_tenant, resolve_binding, with_token_and_base}, RIGG_ARM_ENDPOINT, mount_arm_fake | consistent; T5 test env-var ambiguity — Ruling: implement `base_url_from(env: Option<&str>)` (no temp-env dependency) and `with_token_and_base`; the CLI binary honours RIGG_ARM_ENDPOINT via with_token → base_url_from(std::env::var(..).ok()). Cost if wrong: none |
- | T6 ARM token in tests | ArmClient must honour RIGG_ACCESS_TOKEN — Ruling: yes, static token first (as the data-plane clients), then az CLI | noted in T6 brief |
- Task 3: implemented 543be67 (DONE_WITH_CONCERNS: Indexer channel bumped to Preview because `cache.*` exists only in the preview schema). Ruling: REVERT — Indexer stays on the stable channel; the indexer enrichment cache is a preview-only feature rigg 2.0 does not model: drop the two `cache.*` InfraRef rows (and note it for the auth spec's cache edge). Cost if wrong: users of incremental enrichment get no binding tracking for the cache account — acceptable, documented.
- Task 3: review Needs fixes — C1 extract path separator bug (nested arrays); C2 Indexer channel revert + drop cache rows (ruling); I3 odata suffix equality; I4 authIdentity gating — Ruling: spec wins, `only_odata_type: None`; minors 5 (case-insensitive ResourceId= anywhere, aligned with identity.rs), 6 (URL placeholder rejection), 7 (api prefix boundary), 9 (row-count test) folded into fix round 1; minor 8 (Shared name divergence) deferred to the reporting UX.
- Task 4: implemented ebb6d56 (DONE_WITH_CONCERNS: global --verbose long form removed to free the name). Ruling: restore the global `--verbose`; validate's flag becomes `--show-bindings` (plan said --verbose; the global flag wins). Fix round after review. Cost if wrong: none.
- Task 5: review Approved; Important: by-name fan-out aborts on the first subscription error. Ruling: carried into Task 6 (its consumer) — mirror find_storage_accounts_with_container's skip-and-debug-continue. Cost if wrong: none.
- Task 6: review Approved. Important 1 (function-app candidates ignore --subscription, N×N fan-out) + Important 2 (zero-match reports first_error even when some subscriptions listed) + Minors 3 (targets before dependencies), 4 (--same not asked), 5 (bind/unbind say!), 8 (location in JSON), 9 (tests: rebind replace; yaml round-trip with tenant/subscription/policy), 10 (duplicate renamed proposals), 11 (--like with no targets creates empty env) — Ruling: carried into Task 7. Minors 6 (cache drop on refresh failure) and 7 (describe text prints value) deferred. Cost if wrong: none.
- Final review (opus): mergeable after fixes. Ruling: ONE fix wave — C1 implement the push binding preflight (spec §4; refuse on Leak/strict errors before any mutation) so the docs are true; I2 hint keywords (ai-services; implicit targets get a target-change hint); I3 hint names the validated env; I4 render symmetry (case-insensitive ResourceId= anywhere; api boundary in render); I5 refresh resolves implicit search/foundry under reserved keys; I6 learn keeps ARM ids for storage/identity; M7 one definition of Shared = same physical value for the same Wanted (names may differ) in classify, env show, describe, CONCEPTS; M8 header comment regenerated + accurate doc comment; M9 render tests for UAMI/AiServicesSubdomain/KeyVault; M10/M11 test assertions; M12 "found but unreadable" error; M13 terminology sweep (incl. CLAUDE.md); M14 example; M15 drop `_ws`, document ResolvedBinding.name, answers_to_bindings length check; CHANGELOG notes (strict-by-default on protected envs; leak errors without bindings; RIGG_ACCESS_TOKEN now used for ARM). Deferred: describe repeats per project; refresh drops cache on transient failure; describe text prints value; pull learn offer before conflict check; Remote::for_project unused param. Task 1 minor (empty ids) already fixed in Task 7.
- Re-review: 16/16 addressed; open Important: push --dry-run skips the binding preflight entirely. Ruling: one targeted fix — run the preflight before the dry-run return (report at say! level; refuse only on a real push); cover the pending_relinks path; route WorkspaceError::InvalidBindingName through the "found but unreadable" message. Deferred: leak hint for implicit targets could also offer `env bind <env> <name> ai-services:<account>`; hint uses bare names not ARM ids; target-level "shared with"; push/validate prefix difference. Cost if wrong: none.

Deferred (can wait):

- Task 1: minor (deferred): empty tenantId/subscriptionId not filtered in init; Remote::for_project keeps an unused project param.
- Task 2: minor (deferred): EnvBindings::of_env does not re-validate reserved names (Workspace::load does).
- Task 3: minor (deferred): Class::Shared hides a name divergence across envs (reporting UX).
- Task 4: minor (deferred): too_many_arguments allows on record_classified/validate_project.
- Task 7: minor (deferred): pull's learn offer fires before the conflict check.
- Task 8: minor (deferred): CONCEPTS/SKILL intro still says "service connections".
- Final review (opus): mergeable after fixes. Ruling: ONE fix wave — C1 implement the push binding preflight (spec §4; refuse on Leak/strict errors before any mutation) so the docs are true; I2 hint keywords (ai-services; implicit targets get a target-change hint); I3 hint names the validated env; I4 render symmetry (case-insensitive ResourceId= anywhere; api boundary in render); I5 refresh resolves implicit search/foundry under reserved keys; I6 learn keeps ARM ids for storage/identity; M7 one definition of Shared = same physical value for the same Wanted (names may differ) in classify, env show, describe, CONCEPTS; M8 header comment regenerated + accurate doc comment; M9 render tests for UAMI/AiServicesSubdomain/KeyVault; M10/M11 test assertions; M12 "found but unreadable" error; M13 terminology sweep (incl. CLAUDE.md); M14 example; M15 drop `_ws`, document ResolvedBinding.name, answers_to_bindings length check; CHANGELOG notes (strict-by-default on protected envs; leak errors without bindings; RIGG_ACCESS_TOKEN now used for ARM). Deferred: describe repeats per project; refresh drops cache on transient failure; describe text prints value; pull learn offer before conflict check; Remote::for_project unused param. Task 1 minor (empty ids) already fixed in Task 7.
- Re-review: 16/16 addressed; open Important: push --dry-run skips the binding preflight entirely. Ruling: one targeted fix — run the preflight before the dry-run return (report at say! level; refuse only on a real push); cover the pending_relinks path; route WorkspaceError::InvalidBindingName through the "found but unreadable" message. Deferred: leak hint for implicit targets could also offer `env bind <env> <name> ai-services:<account>`; hint uses bare names not ARM ids; target-level "shared with"; push/validate prefix difference. Cost if wrong: none.
