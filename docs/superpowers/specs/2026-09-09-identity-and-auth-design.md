# rigg 2.0 — Identity and authentication

**Date:** 2026-09-09
**Status:** Design, approved direction. Workstream 3 of
`2026-09-09-rigg-2.0-scope-and-principles-design.md`. Depends on the
bindings spec (resolved bindings supply every scope) and the interaction
spec (question protocol). Facts below were verified against Microsoft Learn
on 2026-09-09 (research reports in the session scratchpad; every role GUID
below was read from the built-in-roles reference or the feature's own
documentation page).

## 1. Problem (1.x)

- Auth is handled *after* the first failure: push PUTs, gets a 403, then
  diagnoses. Indexer creation validates connections and fails with a 400
  that the RBAC path does not even catch.
- Doctor scopes model-access edges to the environment's Foundry account
  instead of the `resourceUri` in the file; Key Vault scopes never resolve.
- The operator's own rights are never checked; the hint text for them is
  dead code. Every new dependency needs two grants, rigg handles one.
- Network causes of 403 (storage firewall, function access restrictions) are
  not distinguished from RBAC.
- The identity constant labelled "Storage Blob Data Reader" in
  `identity.rs` is the GUID of Storage Blob Data **Contributor**
  (`ba92f5b4-…`); Reader is `2a2b9908-6ea1-4ae2-8e65-a410df84e7d1`. rigg 1.x
  over-grants.
- User-assigned identities are recommended by the 1.0 spec but unsupported
  in scaffolds, promote and doctor. And the recommendation is wrong for one
  common case: the storage trusted-services exception works **only** with
  the search service's system-assigned identity.

## 2. Principles (from the scope spec, made concrete)

1. No secrets on disk, none typed, none printed. Runtime secrets that Azure
   still requires are fetched at push time from a declared *key source*.
2. Two-sided: for every edge rigg computes the service-identity role **and**
   the operator's ability to grant it and to manage the target.
3. Before, not after: the auth plan is computed from the push plan and
   resolved before the first mutation; propagation is waited out; the PUT
   retry stays as the safety net.
4. Explain: every finding names the principal, the role, the scope (ARM id),
   the file and path that require it, and the exact `az` command.
5. Verify: a green doctor is a claim; a clean indexer run, a knowledge-base
   retrieval and an agent answer are proof. `rigg push --verify` runs them.

## 3. The identity graph

### 3.1 Principals

| Principal | Identity | How rigg finds it |
|---|---|---|
| `search-system` | Search service system-assigned MI | ARM `searchServices/{n}` `identity.principalId` |
| `search-user:<binding>` | user-assigned MI named by an `identity` binding, used via `identity`/`authIdentity` fields | ARM `userAssignedIdentities/{n}` `principalId`, `clientId` |
| `foundry-project` | Foundry project system-assigned MI | ARM `accounts/{a}/projects/{p}` `identity.principalId` |
| `operator` | the caller (az login user, or SP/OIDC in CI) | token claims (`oid`, `appid`) + ARM `permissions` |
| `principal:<id>` | any named principal (`--principal`), e.g. the CI identity | given |

### 3.2 Edges (derived from files + resolved bindings)

`identity::edges(env, project?, plan?) -> Vec<Edge>` where
`Edge { principal, kind, scope: ArmId, role: Role, reason, sources: [(file, path)], constraints }`.

| File evidence (kind / path) | Principal | Role (GUID) | Scope |
|---|---|---|---|
| DataSource `credentials.connectionString` (`azureblob`/`adlsgen2`) | the data source's identity (system, or `identity` UAMI) | Storage Blob Data Reader `2a2b9908-6ea1-4ae2-8e65-a410df84e7d1` | storage account from the binding |
| KnowledgeSource `azureBlobParameters` | its identity | Storage Blob Data Reader; Contributor `ba92f5b4-2d11-453d-a403-e96b0029c9fe` when an `assetStore` is declared | storage account |
| Skillset `knowledgeStore.storageConnectionString` | its identity | Storage Blob Data Contributor (+ Reader and Data Access `c12c1c16-33a1-487b-954d-41c89c60f349` for table projections) | storage account |
| Indexer `cache.storageConnectionString` | its identity | Storage Blob Data Contributor + Storage Table Data Contributor `0a9a7e1f-b9d0-4cc4-a60d-0319b160aaa3` | storage account |
| Index vectorizer / Skillset AzureOpenAIEmbeddingSkill / KnowledgeSource embedding `resourceUri` | its `authIdentity` or system | Cognitive Services OpenAI User `5e0bd9bd-7b93-4f28-af87-19fc36ad61bd` | Cognitive Services account resolved from `resourceUri` host (`customSubDomainName` / `properties.endpoints` join) |
| KnowledgeBase `models[]` / KnowledgeSource chat-completion model `resourceUri` | search identity | Cognitive Services User `a97b65f3-24c7-4388-baec-2e87135dc908` | same resolution |
| Skillset `cognitiveServices` AIServicesByIdentity `subdomainUrl` | its `identity` or system | Cognitive Services User | account; **constraint**: account kind must be `AIServices` |
| Skillset WebApiSkill `authResourceId` | search identity | *app authorization*: Easy Auth on the function app must accept the audience; optional app-role assignment (Graph) when the enterprise app requires assignment | function app from the `uri` binding |
| Agent tool `type: mcp` with `project_connection_id` → Connection `authType: ProjectManagedIdentity`, `category: RemoteTool` | `foundry-project` | Search Index Data Reader `1407120a-92aa-4202-b7e9-c0e197c71c8f` | Search service (from the MCP URL, i.e. the environment's `search`) |
| Search kinds' `encryptionKey.keyVaultUri` (identity-based) | search identity | Key Vault Crypto Service Encryption User `e147488a-f6f5-4113-8e2d-b22465e65bf6` | key vault from the binding |
| Foundry account CMK (reported only; rigg does not manage account settings) | Foundry account identity | Key Vault Crypto User `12338af0-0e69-4776-bea7-57ae8d297424` or Crypto Service Encryption User (either accepted) | key vault |

**Not implemented in 2.0.0:** the Indexer `cache.storageConnectionString`
row (the incremental-enrichment cache is preview-only, and `cache.*` is
deliberately absent from the registry's infra-reference table), and the
Foundry account CMK row (rigg does not manage account settings, and
`roles::KEY_VAULT_CRYPTO_USER` is defined but unused). Both were ruled out
of scope; the rest of the table is implemented. The **Easy Auth wiring**
operator row (Graph directory roles) is not verified either: `rigg auth
easy-auth` surfaces Microsoft Graph's own error when the caller lacks
Application Developer / Cloud Application Administrator, rather than
pre-checking directory role membership.

Operator edges (always, per environment, scoped to what the plan touches):

| Need | Role(s) | Scope |
|---|---|---|
| manage Search configuration (all Search kinds; run/reset indexers) | Search Service Contributor `7ca78c08-252a-4471-8644-bb5ff32d4ba0` | search service |
| `rigg az` query / KB ask / `--verify` | Search Index Data Reader (or Contributor) | search service |
| Search accepts bearer tokens | setting: `authOptions.aadOrApiKey` or `disableLocalAuth: true` (ARM 2025-05-01) | search service |
| create/update agents | Foundry User `53ca6127-db72-4b80-b1b0-d745d6d5456d` (Owner/Contributor do **not** suffice) | Foundry project |
| create project connections | Foundry Project Manager `eadc314b-1a2d-4efa-be10-5d325db5065e` or Cognitive Services Contributor `25fbc0a9-bd7c-42a3-aa1a-3b75d497ee68` (a control-plane write; Owner/Contributor cover it via effective permissions) | Foundry account |
| create model deployments | Foundry Account Owner `e47c6f54-e4a2-4754-9501-8e0985b135e1` or Cognitive Services Contributor `25fbc0a9-bd7c-42a3-aa1a-3b75d497ee68` | Foundry account |
| grant a missing role | `Microsoft.Authorization/roleAssignments/write` at the edge's scope (ARM `permissions` list, wildcard-matched) | each edge scope |
| Easy Auth wiring | Graph: Application Developer suffices to create the application and its service principal; Cloud Application Administrator (or Application Administrator) is needed to PATCH an application the user does not own and for app-role assignments | tenant |

### 3.3 Constraints and settings (not roles, still required)

| Check | Rule | Fix |
|---|---|---|
| Search SKU | Free has no managed identity; knowledge bases need Basic+ (S3 HD: none) | report |
| Search identity exists | system-assigned enabled when the files name no identity, and every UAMI they *do* name attached | `--fix`: PATCH identity (ARM Search 2025-05-01) |
| Storage network: firewall (`networkAcls.defaultAction: Deny`) | needs `bypass` ⊇ `AzureServices` **and** the connection must use the system-assigned identity (UAMI unsupported for the trusted-service exception); or a resource-instance rule for the search service; IP rules never work same-region | `--fix` (with confirmation): add `AzureServices` bypass or a resource-instance rule; rewrite the file to system identity when it used a UAMI (question) |
| — 2.0.0 note | Deny + UAMI is reported with a hint and repaired with the **resource-instance rule** only; the "rewrite the file to system identity" question, and the Global Constraint question id `auth.identity.<env>` it reserves, are **not asked in 2.0.0** | — |
| Storage `publicNetworkAccess: Disabled` | only a shared private link works; needs Basic+ (S1+ when the indexer has a skillset beyond embedding); indexer `executionEnvironment: private` | report existing `sharedPrivateLinkResources` (ARM Search) or print the exact creation command |
| — 2.0.0 note | the condition is **reported only**; rigg does not list or create shared private links (`list_shared_private_links` exists in the client and has no caller) | — |
| Blob soft delete | required by `NativeBlobSoftDeleteDeletionDetectionPolicy`; blob **versioning must be off** | `--fix` (confirmation): enable soft delete (7 days default); versioning: report |
| `allowSharedKeyAccess: false` | fine for blobs with identity | none |
| AIServicesByIdentity account kind | must be `AIServices` | question: switch `subdomainUrl` to a bound AIServices account — 2.0.0 **reports** the wrong kind and names the change; the question is not asked |
| Function app access restrictions | must admit the search service (`AzureCognitiveSearch` service tag or search IP) | report + exact command |
| Easy Auth on the function app | `authsettingsV2`: AAD provider enabled, `allowedAudiences` contains the skill's `authResourceId`, `unauthenticatedClientAction: Return401`; optionally `allowedApplications` contains the search MI client id | `--fix`: see §5 |
| Deployment availability | model/version available in the account's region; quota headroom for `sku.capacity` | question (promote) / report (doctor) — in 2.0.0 this is **checked by `push`/`promote`**, where the deployment is written; `doctor` marks the row skipped rather than repeating it |

Cosmos DB and Azure SQL edges are removed with their data-source types.

## 4. Commands

### 4.1 `rigg auth doctor [-e env] [--fix] [--principal <id>] [--plan] [--live] [--output json]`

1. Resolve bindings; load the environment's tree (or, with `--plan`, only the
   resources a push would create/update, using the same plan builder as
   push).
2. Derive edges (§3.2) and checks (§3.3).
3. Verify each through ARM/Graph, in this order: identity existence → operator
   rights → settings and network → role assignments (using
   `$filter=atScope() and assignedTo('<oid>')` so inherited assignments
   count).
4. Report per edge: `✓` / `✗ missing` / `!` setting / `?` unresolved, each
   with principal, role, scope, reason, the source file:path, and the fix
   command. With `--fix`, apply fixes that are (a) rigg-owned and (b)
   confirmed interactively or pre-answered; role assignments carry
   `description: "rigg:<workspace>:<env>:<reason>"` and `principalType:
   ServicePrincipal` for managed identities.
5. `--principal` checks operator edges for a named principal instead of the
   caller (the CI identity). `rigg ci init` prints the resulting role list
   with real scopes.
6. `--live` additionally reads each indexer's last execution result and
   attributes auth-shaped failures (403/401 from storage, model, function) to
   the edge that would explain them.
7. Exit codes: 0 all good; 4 when anything is missing (also with `--fix` when
   something could not be fixed); 6 when a fix needs an answer
   non-interactively.

`rigg status --auth` runs the doctor's verification per environment and
adds one line per environment ("identity: ok" / "identity: 2 missing — rigg
auth doctor -e prod").

### 4.2 Push integration

Before the plan is applied:

1. **Plan-scoped auth preflight** = doctor `--plan` on the push plan.
   Interactive: missing items are listed and fixes offered inline (one
   confirmation per fix class, not per edge). Non-interactive: exit 4 with
   the list unless `--skip-auth-preflight`.
2. **Grant, then wait.** After grants, rigg polls until the role assignment
   is visible through `atScope()` for the principal, then proceeds; the
   existing PUT-retry loop (`RIGG_RBAC_RETRY_SECS`/`MAX_RETRIES`) remains for
   data-plane propagation lag.
3. **Key injection at push time** for declared key sources (§6).
4. **`--verify`** (also `rigg verify <project> -e env` standalone): after a
   successful push, for each indexer: run and watch to completion, fail on
   errors; for each knowledge base: a retrieve smoke call; for each agent: a
   one-turn ask. Non-zero on any failure with the attributed edge when one
   matches.

### 4.3 `rigg auth easy-auth <function-app binding> [-e env] [--client-id <existing app>]`

Enables Microsoft Entra authentication on a function app so Web API skills
can be keyless (§5). Also reachable from push's Web API auth resolution
("identity-based — set it up now").

### 4.4 `rigg auth roles list|remove [-e env]`

Lists role assignments rigg created (by description prefix) and removes
them; `rigg env remove --clean-roles` uses it.

## 5. Easy Auth wiring (Graph in scope)

Given a bound function app and the search identity:

1. Application: reuse `--client-id` if given; else Graph `POST /applications`
   `{ displayName: "rigg-<site>", signInAudience: "AzureADMyOrg", api: { requestedAccessTokenVersion: 2 } }`,
   then `PATCH /applications/{id}` `{ identifierUris: ["api://<appId>"] }`.
2. Service principal: `POST /servicePrincipals { appId }` (if absent).
3. Function app: ARM `PUT .../config/authsettingsV2` with
   `platform.enabled: true`, `globalValidation { requireAuthentication: true, unauthenticatedClientAction: "Return401" }`,
   `identityProviders.azureActiveDirectory { enabled: true, registration { clientId, openIdIssuer: "https://login.microsoftonline.com/<tenant>/v2.0" }, validation { allowedAudiences: ["api://<appId>"], defaultAuthorizationPolicy { allowedApplications: [<search MI clientId>] } } }`.
   Existing settings are merged, not replaced; the current document is shown
   as a diff before writing.
4. If the enterprise app has `appRoleAssignmentRequired: true`: Graph
   `POST /servicePrincipals/{resourceSpObjectId}/appRoleAssignedTo`
   `{ principalId: <caller MI object id>, resourceId: <resourceSpObjectId>, appRoleId: <Caller role id> }`.
   The role is a real one rigg defines on the application in step 1 —
   `value: "Caller"`, `allowedMemberTypes: ["Application"]`, its id
   deterministic in the identifier URI so a re-run finds the same role —
   rather than the all-zeros default role: an explicit application-only role
   is auditable and revocable on its own. Posting to the *resource* service
   principal's `appRoleAssignedTo` and posting to the *principal's*
   `appRoleAssignments` create the same directory object; the resource-side
   form is used because rigg already holds the resource SP's object id. The
   call is idempotent: existing assignments for the principal are listed
   first, and Graph's "Permission being assigned already exists" 400 is
   treated as success, so a second run cannot fail after step 3's PUT has
   landed.
5. Skill file: `authResourceId: "api://<appId>"`, key carrier removed,
   `x-rigg-auth` removed.

Graph tokens come from the operator's Azure CLI login
(`--resource-type ms-graph`, per tenant). Insufficient directory rights
produce a message naming the required directory role and the manual
portal path.

## 6. Key sources (`x-rigg-auth`)

For the cases Azure still needs a secret at runtime:

| Annotation on the WebApiSkill | Push-time behaviour |
|---|---|
| `"x-rigg-auth": "function-key"` | ARM `listkeys` on the function (function-level key preferred, host `default` key fallback); placed in the skill's own carrier (`x-functions-key` header if present, else `code=`); file keeps `<redacted>` |
| `"x-rigg-auth": "key-vault:<secret-name>@<key-vault binding>"` | Key Vault data plane `GET /secrets/<name>` with the operator's token (api-version 2025-07-01, scope `https://vault.azure.net/.default`, operator needs Key Vault Secrets User `4633458b-17de-408a-b874-0445c86b69e6`); placed the same way |
| absent, `authResourceId` set | keyless; nothing injected |
| absent, no `authResourceId`, key redacted | push gate: question (Entra via §5 / function-key / key-vault / skip) |

`rigg push --refresh-credentials` re-injects for in-sync skillsets (kept
from 1.6.4). Keys never appear in output; the injected body is never logged.

## 7. Identity choice

- Default: the search service's **system-assigned** identity for every
  Search-side edge. It is the only identity that works with the storage
  trusted-services exception and with debug sessions, and it needs no
  binding.
- An `identity` binding (UAMI) is used when the file says so
  (`identity`/`authIdentity` objects), typically for portability across
  service re-creation or for a shared identity across environments. Doctor
  enforces the constraint table: a UAMI against firewalled storage with the
  trusted-services bypass is reported as unsupported with the two ways out
  (system identity, or a resource-instance rule).
- Scaffolds and `rigg new` set the identity fields from the environment's
  `identity` binding when `--identity <binding>` is given, else leave them
  null (system). The flag applies to the kinds whose *scaffold* has a place
  for a single identity object — `data-source` and `skillset`; for a
  skillset it also writes the `cognitiveServices` discriminator
  (`AIServicesByIdentity` + a `subdomainUrl` placeholder) that the identity
  is only legal alongside. Array-element identities (vectorizers, models,
  individual skills) and the blob forms of a knowledge source come from
  `rigg env bind <env> --learn` / pull, not from a fresh scaffold.

## 8. Tokens and tenancy

`rigg-client::auth` becomes a per-`(tenant, audience)` token provider:
audiences ARM, Search (`https://search.azure.com/.default`), Foundry data
plane, Cognitive Services, Key Vault, Graph. Azure CLI path:
`az account get-access-token --tenant <t> --scope <audience>/.default` (or
`--resource-type ms-graph`); on failure for a non-home tenant the error tells
the user to `az login --tenant <t>`. Service-principal and
`RIGG_ACCESS_TOKEN` paths are per audience as today, tenant taken from the
environment. The 5-minute token cache is keyed by (tenant, audience).

## 9. Removed from 1.x

Cosmos/SQL edges; `error.rs` suggestion text (replaced by doctor output);
the fixed default-scope fallbacks in doctor; `SEARCH_ARM_API` literals
(registry provider table).

## 10. Testing

- Unit: edge derivation for every row in §3.2 from fixture documents (system
  and UAMI variants); operator edge selection by plan contents; constraint
  evaluation from fixture ARM documents (storage firewall matrix: open /
  deny+bypass / deny+resource-rule / disabled; soft delete on/off/versioning;
  SKU; account kind); `resourceUri` → account resolution via `endpoints`;
  permissions wildcard matching; role-assignment filter and description.
- Wiremock ARM + Graph fakes (shared with workstream 1): doctor end to end on
  a two-environment workspace — missing role → `--fix` creates it with the
  description → second run green; operator lacking `roleAssignments/write`
  → exit 4 with the az command; Easy Auth wiring creates application,
  service principal and authsettingsV2; key-vault key source injected on
  push (fake vault) and never written to disk or stdout.
- CLI: exit codes 0/4/6, JSON shape, `--principal`, `status --auth` line.
- Live (Kristofer's subscription; minimal, cleaned up): doctor on
  `e2e-test` dev; a temporary UAMI + firewalled temporary storage account
  (Standard_LRS, deleted afterwards) to exercise the trusted-services and
  resource-rule paths; Easy Auth on the existing `mklab` function app only
  if he confirms (it changes a shared app); `push --verify` on `regulus`.

## 11. Facts still unverifiable from documentation (verify live in workstream 3)

- Whether Easy Auth honours "assignment required" for managed identities
  (rigg assigns the app role whenever the flag is set, which is safe either
  way).
- Whether ARM accepts `properties.audience` on a `RemoteTool` connection on
  api-version 2026-07-01 (rigg passes the file through; the live test records
  the answer).
- `identity.principalId` on `GET accounts/{a}/projects/{p}` (inferred from
  the ARM schema).
