# Workstream 3: Identity and authentication — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** rigg computes, verifies, grants and proves every identity requirement of a configuration before anything is pushed: service-identity roles on the right scopes (from bindings), the operator's own rights, network and setting constraints, Easy Auth for Web API skills (with Graph), push-time key sources without secrets on disk, and an end-to-end verification (`push --verify`).

**Architecture:** `rigg-core::identity` becomes a graph builder: `Edge`s (principal, role, scope from resolved bindings, sources, constraints) and `Check`s (settings/network) derived from documents + `EnvBindings`; operator edges derived from a plan. `rigg-client` gains the ARM reads the checks need, RBAC helpers with `description`/`atScope()`, a per-tenant/audience token provider, a Graph client and a Key Vault secret reader. The CLI's `auth doctor` verifies and fixes through those; `push` runs the plan-scoped preflight before the first mutation and can verify afterwards; `auth easy-auth` wires Entra for a function app; `x-rigg-auth` gains the Key Vault source.

**Tech Stack:** Rust 2024 (MSRV 1.88), reqwest 0.12, serde_json, wiremock ARM/Graph/KeyVault fakes, assert_cmd, the `ask` primitives.

**Spec:** `docs/superpowers/specs/2026-09-09-identity-and-auth-design.md` (all sections); scope from `2026-09-09-rigg-2.0-scope-and-principles-design.md` §4–§5.

## Global Constraints

- Branch `rigg-2`; commit after every task with trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Gate before every commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Role GUIDs (exact): Storage Blob Data Reader `2a2b9908-6ea1-4ae2-8e65-a410df84e7d1`; Storage Blob Data Contributor `ba92f5b4-2d11-453d-a403-e96b0029c9fe`; Storage Table Data Contributor `0a9a7e1f-b9d0-4cc4-a60d-0319b160aaa3`; Reader and Data Access `c12c1c16-33a1-487b-954d-41c89c60f349`; Cognitive Services OpenAI User `5e0bd9bd-7b93-4f28-af87-19fc36ad61bd`; Cognitive Services User `a97b65f3-24c7-4388-baec-2e87135dc908`; Cognitive Services Contributor `25fbc0a9-bd7c-42a3-aa1a-3b75d497ee68`; Search Index Data Reader `1407120a-92aa-4202-b7e9-c0e197c71c8f`; Search Index Data Contributor `8ebe5a00-799e-43f5-93ac-243d3dce84a7`; Search Service Contributor `7ca78c08-252a-4471-8644-bb5ff32d4ba0`; Key Vault Crypto Service Encryption User `e147488a-f6f5-4113-8e2d-b22465e65bf6`; Key Vault Crypto User `12338af0-0e69-4776-bea7-57ae8d297424`; Key Vault Secrets User `4633458b-17de-408a-b874-0445c86b69e6`; Foundry User `53ca6127-db72-4b80-b1b0-d745d6d5456d`; Foundry Project Manager `eadc314b-1a2d-4efa-be10-5d325db5065e`; Foundry Account Owner `e47c6f54-e4a2-4754-9501-8e0985b135e1`.
- Role assignments rigg creates carry `properties.description = "rigg:<workspace-name-or-dir>:<env>:<reason>"` and `principalType: ServicePrincipal` for managed identities (`User`/`ServicePrincipal` per the operator's token for operator grants — rigg never grants itself).
- Role checks use `$filter=atScope() and assignedTo('<oid>')` so inherited assignments count.
- Storage trusted-services exception works only with the search service's **system-assigned** identity.
- No secrets on disk, none printed; Key Vault values and function keys exist only in the outgoing request body.
- Question ids: `auth.fix.<edge-id>` (confirm), `auth.easyauth.<site>` (confirm), `auth.identity.<env>` (choice system/uami binding); prefix `auth.` registered.
- Exit codes: doctor 0 ok / 4 missing (also with `--fix` when unfixed) / 6 needs input; push preflight refusal exit 4 (`--skip-auth-preflight` bypasses).
- No api-version literals outside the registry; new providers' versions come from the table (`Provider::{ManagedIdentityArm, KeyVaultArm, KeyVaultData, Graph, SearchArm, StorageArm, WebArm}`).
- Tokens: `auth::token_for(tenant: Option<&str>, audience: &str)` with the existing 5-minute cache keyed by `(tenant, audience)`; Azure CLI path uses `--scope <audience>/.default` (Graph: `--resource-type ms-graph`); `RIGG_ACCESS_TOKEN` is used for every audience when set (test rigs).

---

### Task 1: Identity graph v2 in `rigg-core::identity`

**Files:**
- Modify: `crates/rigg-core/src/identity.rs` (rewrite), `crates/rigg-core/src/lib.rs` (no change expected)
- Test: inline

**Interfaces:**
```rust
pub mod roles { pub struct Role { pub id: &'static str, pub name: &'static str } pub const STORAGE_BLOB_DATA_READER: Role; /* … every GUID from Global Constraints … */ }
pub enum Principal { SearchSystem, SearchUser { binding: String }, FoundryProject, Operator, Named { object_id: String } }
pub enum Scope { Resolved(String /* ARM id */), Unresolved { binding: String, kind: Option<BindingType>, physical: String } }
pub enum Constraint { AiServicesKindRequired, TrustedServiceNeedsSystemIdentity, PreviewOnly(&'static str) }
pub struct Source { pub kind: ResourceKind, pub name: String, pub path: String }
pub struct Edge { pub id: String /* stable: "<principal>|<role>|<scope>" */, pub principal: Principal, pub role: roles::Role, pub scope: Scope, pub reason: String, pub sources: Vec<Source>, pub constraints: Vec<Constraint>, pub kind: EdgeKind }
pub enum EdgeKind { Rbac, AppAuthorization /* Easy Auth audience */, Informational }
pub enum CheckKind { SearchSku, SearchIdentity, SearchRbacEnabled, StorageNetwork { account: Scope }, StorageSoftDelete { account: Scope }, StorageSharedKey { account: Scope }, AiServicesKind { account: Scope }, FunctionAppNetwork { site: Scope }, EasyAuth { site: Scope, audience: String }, DeploymentAvailability { stem: String } }
pub struct Check { pub id: String, pub kind: CheckKind, pub reason: String, pub sources: Vec<Source> }
pub struct Graph { pub edges: Vec<Edge>, pub checks: Vec<Check>, pub operator: Vec<Edge> }
pub fn graph_for_docs(env: &EnvBindings, docs: &[(ResourceKind, String, Value)]) -> Graph;   // service edges + checks, per spec §3.2/§3.3
pub fn operator_edges(env: &EnvBindings, kinds_in_plan: &[ResourceKind], needs_grants: &[&Edge]) -> Vec<Edge>; // spec §3.2 operator table
pub fn parse_resource_id(conn: &str) -> Option<String>; // kept
```
Derivation table (implement exactly; every row a test):
| evidence | principal | role | scope |
|---|---|---|---|
| DataSource `credentials.connectionString` (storage) | `identity` object → `SearchUser{binding}` (matched via `env.find_physical(Type(Identity), name)`), else `SearchSystem` | Blob Data Reader | storage binding (Resolved when `resolved.arm_id` present; else Unresolved) |
| KnowledgeSource `azureBlobParameters.connectionString` | ingestionParameters.identity or system | Blob Data Reader; + Contributor on `assetStore.connectionString`'s account when present | storage |
| Skillset `knowledgeStore.storageConnectionString` | knowledgeStore.identity or system | Blob Data Contributor (+ Reader and Data Access when `projections[].tables` non-empty) | storage |
| Index vectorizer / Skillset AzureOpenAIEmbeddingSkill / KS embedding `resourceUri` | authIdentity or system | Cognitive Services OpenAI User | model host (binding `foundry` or ai-services) |
| KnowledgeBase `models[]` / KS chatCompletionModel `resourceUri` | system | Cognitive Services User | model host |
| Skillset `cognitiveServices` AIServicesByIdentity `subdomainUrl` | identity or system | Cognitive Services User + `Constraint::AiServicesKindRequired` | ai-services |
| Skillset WebApiSkill with `authResourceId` | SearchSystem/authIdentity | `EdgeKind::AppAuthorization` (audience = authResourceId) | function app from `uri` |
| Agent tool `mcp` with `project_connection_id` (+ Connection `authType: ProjectManagedIdentity`) | FoundryProject | Search Index Data Reader | implicit `search` binding |
| any `encryptionKey.keyVaultUri` without accessCredentials | SearchSystem | Key Vault Crypto Service Encryption User | key vault |
Checks: `SearchSku`, `SearchIdentity`, `SearchRbacEnabled` always when the env has a search target; `StorageNetwork`/`StorageSoftDelete`(only when a data source uses `NativeBlobSoftDeleteDeletionDetectionPolicy`)/`StorageSharedKey` per storage binding used; `AiServicesKind` per AIServicesByIdentity; `FunctionAppNetwork` + `EasyAuth` per WebApiSkill with `authResourceId`; `DeploymentAvailability` per Deployment doc. `TrustedServiceNeedsSystemIdentity` constraint attached to storage edges whose principal is `SearchUser`.
Operator table (spec §3.2): Search Service Contributor @ search when any Search kind in plan; Search Index Data Reader @ search when `--verify`/az ops requested (flag param); Foundry User @ project when Agent in plan; Foundry Project Manager @ account when Connection in plan; Foundry Account Owner **or** Cognitive Services Contributor @ account when Deployment/Guardrail in plan (an `Edge` may list `alternatives: Vec<roles::Role>` — add the field); `roleAssignments/write` permission check per scope of every edge in `needs_grants` (represented as `CheckKind::CanGrant { scope }` — add it).

- [ ] Steps: tests first (one per table row + operator table + constraint attachment + Unresolved scope when the binding has no ARM id), RED → implement → GREEN → gate → commit `feat(core): identity graph v2 — edges from bindings, constraints, checks, operator edges`.

---

### Task 2: Client capabilities — tokens per audience, ARM reads, RBAC helpers, Graph, Key Vault

**Files:**
- Modify: `crates/rigg-client/src/auth.rs` (`token_for`), `crates/rigg-client/src/arm.rs`, `crates/rigg-client/src/lib.rs`
- Create: `crates/rigg-client/src/graph.rs`, `crates/rigg-client/src/keyvault.rs`
- Test: inline URL/parsing tests; `crates/rigg/tests/arm_fake.rs` gains mounts: `mount_search_service(server, sub, rg, name, sku, identity_type, principal_id, rbac_enabled, public_network)`, `mount_storage_account(server, id, network_default_action, bypass, public_network, shared_key, hns, soft_delete, versioning)`, `mount_permissions(server, scope, can_write_role_assignments)`, `mount_role_assignments(server, scope, principal, role_ids)` (+ PUT recorder), `mount_cognitive_account(server, id, kind, location)`; new `crates/rigg/tests/graph_fake.rs` (`mount_graph(server)`: POST applications → 201 with appId, PATCH applications/{id} → 204, POST servicePrincipals → 201, POST servicePrincipals/{id}/appRoleAssignedTo → 201, GET servicePrincipals?$filter=appId → list) and `mount_keyvault_secret(server, name, value)`.

**Interfaces:**
```rust
// auth.rs
pub fn token_for(tenant: Option<&str>, audience: &str) -> Result<String, AuthError>; // RIGG_ACCESS_TOKEN > service principal env > az CLI (--scope audience/.default; ms-graph via --resource-type)
// arm.rs (all via self.url + Provider)
pub async fn get_search_service(&self, id: &str) -> Result<SearchServiceInfo, ClientError>;   // sku.name, identity{type, principalId, userAssignedIdentities}, authOptions/disableLocalAuth, publicNetworkAccess, networkRuleSet
pub async fn set_search_auth_options(&self, id: &str) -> Result<(), ClientError>;              // PATCH aadOrApiKey http401WithBearerChallenge
pub async fn attach_user_assigned_identity(&self, id: &str, provider: Provider, uami_id: &str) -> Result<(), ClientError>; // PATCH identity type SystemAssigned, UserAssigned + map
pub async fn get_storage_account(&self, id: &str) -> Result<StorageAccountInfo, ClientError>; // networkAcls{defaultAction,bypass,resourceAccessRules[]}, publicNetworkAccess, allowSharedKeyAccess, isHnsEnabled, location
pub async fn get_blob_service_properties(&self, account_id: &str) -> Result<BlobServiceInfo, ClientError>; // deleteRetentionPolicy{enabled,days}, isVersioningEnabled
pub async fn set_blob_soft_delete(&self, account_id: &str, days: u32) -> Result<(), ClientError>;
pub async fn add_storage_bypass_azure_services(&self, id: &str) -> Result<(), ClientError>;   // PATCH networkAcls.bypass += AzureServices
pub async fn add_storage_resource_instance_rule(&self, id: &str, tenant: &str, resource_id: &str) -> Result<(), ClientError>;
pub async fn list_shared_private_links(&self, search_id: &str) -> Result<Vec<Value>, ClientError>;
pub async fn get_cognitive_account_by_id(&self, id: &str) -> Result<AiServicesAccount, ClientError>;
pub async fn can_write_role_assignments(&self, scope: &str) -> Result<bool, ClientError>;       // GET {scope}/providers/Microsoft.Authorization/permissions, wildcard match
pub async fn role_assignments_for(&self, scope: &str, principal_id: &str) -> Result<Vec<RoleAssignmentInfo{role_definition_id, description, id}>, ClientError>; // $filter=atScope() and assignedTo('{pid}')
pub async fn create_role_assignment_described(&self, scope: &str, principal_id: &str, role_guid: &str, principal_type: &str, description: &str) -> Result<(), ClientError>;
pub async fn delete_role_assignment(&self, id: &str) -> Result<(), ClientError>;
pub async fn list_rigg_role_assignments(&self, scope: &str, description_prefix: &str) -> Result<Vec<RoleAssignmentInfo>, ClientError>;
pub async fn create_user_assigned_identity(&self, subscription: &str, rg: &str, name: &str, location: &str) -> Result<ArmResource, ClientError>;
pub async fn caller_object_id(&self) -> Result<CallerIdentity{object_id, principal_type: "User"|"ServicePrincipal", display: String}, ClientError>; // decode the bearer token's oid/appid/upn claims (base64url JSON, no signature check)
// graph.rs
pub struct GraphClient { … } impl GraphClient { pub fn for_tenant(tenant: Option<&str>) -> Result<Self>; pub async fn create_application(&self, display_name: &str) -> Result<Application{id, app_id}>; pub async fn set_identifier_uri_and_role(&self, app_object_id: &str, uri: &str) -> Result<String /* app role id */>; pub async fn ensure_service_principal(&self, app_id: &str) -> Result<ServicePrincipal{id, app_role_assignment_required: bool}>; pub async fn assign_app_role(&self, resource_sp_id: &str, principal_object_id: &str, app_role_id: &str) -> Result<()>; }
// keyvault.rs
pub async fn get_secret(tenant: Option<&str>, vault_uri: &str, name: &str) -> Result<String, ClientError>; // api-version KEYVAULT_SECRETS_API_VERSION; value never logged
```
`RIGG_GRAPH_ENDPOINT` and `RIGG_KEYVAULT_ENDPOINT` test overrides mirror `RIGG_ARM_ENDPOINT`.

- [ ] Steps: tests (URL building; token cache key; `can_write_role_assignments` wildcard cases `*`, `Microsoft.Authorization/*`, exact, notActions exclusion; Graph create/patch/sp flow against the fake; Key Vault secret against the fake) RED → implement → GREEN → gate → commit `feat(client): per-audience tokens, RBAC helpers with description/atScope, storage/search/cognitive reads, Graph and Key Vault clients`.

---

### Task 3: `rigg auth doctor` v2 and `rigg auth roles`

**Files:**
- Modify: `crates/rigg/src/commands/doctor.rs` (rewrite), `crates/rigg/src/cli.rs` (`Doctor { fix, principal, plan, live, env }`, `AuthCommands::Roles { List, Remove }`), `crates/rigg/src/commands/status.rs` (`--auth` line), `crates/rigg/src/commands/ask.rs` (`auth.` prefix)
- Create: `crates/rigg/src/commands/auth_report.rs` (rendering shared with push)
- Test: `crates/rigg/tests/auth_fake.rs` (doctor end to end on a two-env workspace with the fakes)

**Interfaces / behaviour (spec §4.1):**
1. Resolve bindings (cache; `--refresh`-less: use cached ARM ids, resolve missing ones through `ArmClient::resolve_binding` and save the cache).
2. `identity::graph_for_docs` over the env's tree (or the push plan with `--plan`) + `operator_edges`.
3. Verify in order: identities (search: `get_search_service`.identity; UAMI bindings: resolved principal_id; foundry project: `get_resource_identity`) → operator rights (caller via `caller_object_id`, or `--principal`) → settings/network checks → role assignments (`role_assignments_for`). Unresolved scopes are reported `?` with `rigg env show <env> --refresh`.
4. Report per edge/check with `✓`/`✗`/`!`/`?`, principal, role, scope, reason, `file:path`, fix command (`az role assignment create --assignee <oid> --role "<name>" --scope <id>` etc.). `--fix`: for each fixable item, a question `auth.fix.<edge-id>` (confirm, default yes) → apply (`create_role_assignment_described` with the rigg description and `principalType`; `enable_system_identity`; `set_search_auth_options`; `set_blob_soft_delete`; `add_storage_bypass_azure_services` — the storage network fix is offered only when `defaultAction == Deny` and the search identity is system). Non-interactive `--fix` with `--yes` applies all; without → exit 6 listing the questions.
5. `--principal <object-id>`: operator edges checked for that principal. `--live`: for each indexer, `indexer_status` last result; on an auth-shaped error (403/401/`Unauthorized`/`Forbidden`/`AuthorizationPermissionMismatch`/`AADSTS`) attribute to the edge whose scope host appears in the message.
6. JSON: `{ env, edges: [{id, principal, role, scope, status, reason, sources, fix}], checks: [...], operator: [...], summary: {ok, missing, unresolved} }`.
7. `rigg auth roles list [-e]` lists assignments with description prefix `rigg:<ws>:<env>` at every scope the graph knows; `remove` deletes them (confirm; `--yes`). `rigg env remove --clean-roles` calls it.
8. `rigg status --auth`: one line per env from a doctor run without `--fix`.

- [ ] Steps: fake-backed tests (green workspace → exit 0; missing role → exit 4 with az command; `--fix --yes` creates the assignment with description → second run green; no search identity → `--fix` enables it; storage Deny without bypass → `!` line + fix offer; UAMI on firewalled storage → constraint reported; `--principal` lacking Search Service Contributor → exit 4; `--plan` limits edges to the plan; JSON shape) RED → implement → GREEN → gate → commit `feat(auth): doctor v2 — binding scopes, operator rights, settings and network checks, tagged fixes, roles list/remove`.

---

### Task 4: Push integration — preflight, grant-then-wait, `--verify`

**Files:**
- Modify: `crates/rigg/src/commands/push.rs` (preflight after the binding preflight; grant + wait; `--verify`), `crates/rigg/src/cli.rs` (`--verify`, `--skip-auth-preflight`; new `Verify(VerifyArgs)` command), `crates/rigg/src/commands/remote.rs` (nothing new expected), `crates/rigg/src/mcp/tools.rs` (`rigg_push` gains `verify`, `skip_auth_preflight`)
- Create: `crates/rigg/src/commands/verify.rs`
- Test: `crates/rigg/tests/auth_fake.rs` + `sync.rs` (push preflight refusal exit 4 with zero PUTs; `--skip-auth-preflight` proceeds; grant-then-wait: role assignment PUT recorded, then role list shows it, then PUT proceeds; `--verify` runs indexer status polling + KB retrieve + agent ask against wiremock)

**Behaviour:** after the binding preflight and before the protected gate: `identity::graph_for_docs` over the plan bodies + `operator_edges(kinds_in_plan)` → verify (Task 3's engine, extracted into `auth_engine::verify(...) -> Report`) → missing fixable items: interactive → one confirmation `auth.fix.all` (default yes) then apply and **wait**: poll `role_assignments_for` until every granted role is visible (max `RIGG_RBAC_RETRY_SECS`×`MAX_RETRIES`), then continue; non-interactive → exit 4 listing items (unless `--skip-auth-preflight`). Existing `put_with_rbac_help` stays. `--verify` (and `rigg verify <project> -e <env>`): for each indexer in the project run + poll status until success/error (reuse `az/indexer.rs` logic; factor `run_and_watch(remote, name, ctx)`), each KB `kb_retrieve` smoke (`{"messages":[{"role":"user","content":[{"type":"text","text":"ping"}]}]}` shape per current client), each agent `agent_ask("Reply with OK")`; failures attributed via the doctor's error matcher; exit 1 on any failure.

- [ ] Steps: tests RED → implement → GREEN → gate → commit `feat(push): auth preflight before mutation, grant-then-wait, push --verify / rigg verify`.

---

### Task 5: Easy Auth wiring and key sources

**Files:**
- Modify: `crates/rigg/src/cli.rs` (`AuthCommands::EasyAuth { function_app: String, client_id: Option<String>, env }`), `crates/rigg/src/commands/credentials.rs` (`resolve_webapi_auth` gains "identity-based — set it up now" → calls the wiring; `inject_function_keys` handles `key-vault:<secret>@<binding>`; `webapi_skills_missing_auth` recognises the key-vault annotation), `crates/rigg/src/commands/validate.rs` (accept the key-vault annotation; the vault must be a bound key-vault), `crates/rigg/src/commands/new.rs` + `crates/rigg-core/src/scaffold.rs` (`--identity <binding>` sets `identity`/`authIdentity` objects)
- Create: `crates/rigg/src/commands/easy_auth.rs`
- Test: `crates/rigg/tests/auth_fake.rs` (Graph + ARM fakes: `auth easy-auth enrich-fn -e dev --yes` creates app + SP, PATCHes authsettingsV2 with merged settings, sets `authResourceId` in the skillset file; key-vault key injected on push from the fake vault and never written to disk or stdout; `new data-source --identity pipeline-mi` writes the identity object)

**Behaviour (spec §5, §6, §7):** `easy_auth`: binding → site id (resolve_binding) → `site_auth_settings` current → plan (show a diff of the settings document) → confirm `auth.easyauth.<site>` → Graph: application (reuse `--client-id`), identifier URI `api://<appId>` + one app role, service principal → ARM PUT authsettingsV2 (merge: keep other providers; set platform.enabled, globalValidation, azureActiveDirectory registration/validation with allowedAudiences + allowedApplications = search MI clientId) → if `appRoleAssignmentRequired` assign the search MI → update every skillset in the env whose WebApi uri host is that site: `authResourceId = api://<appId>`, strip key carriers. Key source: `"x-rigg-auth": "key-vault:<secret>@<binding>"` → at push, `keyvault::get_secret(tenant, vault_uri from the resolved binding's endpoint, secret)` → `place_function_key`. Identity choice: `rigg new … --identity <binding>` writes `{"@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity", "userAssignedIdentity": "<arm id>"}` (needs the resolved id → resolve on demand).

- [ ] Steps: tests RED → implement → GREEN → gate → commit `feat(auth): Easy Auth wiring via Graph, Key Vault key source, --identity scaffolds`.

---

### Task 6: Docs, CHANGELOG, live smoke

**Files:** `CONCEPTS.md` (+crate copy: "How rigg handles authentication"), `README.md` (auth section rewrite; doctor flags; `push --verify`; `auth easy-auth`; `auth roles`), `GETTING_STARTED.md` (step 4 rewrite), `.claude/skills/rigg-guide/SKILL.md`, `MCP.md` (rigg_push params), `CHANGELOG.md` (Added; Changed(breaking): Storage Blob Data Reader GUID corrected — 1.x granted Contributor; doctor exit codes; `RIGG_ACCESS_TOKEN` used for all audiences), `crates/rigg/src/commands/ci.rs` (role list from `operator_edges` with real scopes).
**Live smoke (authorised; read-only against Azure except role assignments on rigg's own temp resources):** from `e2e-test/`: `rigg auth doctor -e dev` (no --fix; paste), `rigg auth doctor -e dev --output json`, `rigg verify regulus -e dev` (KB retrieve + agent ask; indexer run only if the regulatory indexer exists and Kristofer's earlier notes say running it is cheap — otherwise skip with a note), `rigg auth roles list -e dev`. Optional if time allows and cheap: create a temporary Standard_LRS storage account `riggtmp<rand>` in `mklab-rg` with `defaultAction: Deny`, bind it in a throwaway env, run doctor to see the network check, delete the account.

- [ ] Steps: docs → smoke (paste) → gate → commit `docs(auth): identity and authentication chapter; doctor, verify, easy-auth, roles`.
