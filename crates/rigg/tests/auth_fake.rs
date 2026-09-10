//! `rigg auth doctor` / `rigg auth roles` / `rigg status --auth` end to end
//! against the wiremock ARM fake.
//!
//! One mock server serves both planes: ARM lives under `/subscriptions/…`
//! and the Search data plane under `/datasources`, `/indexers/…`, so a single
//! `RIGG_ARM_ENDPOINT` + `endpoint:` pair covers `--plan` and `--live` too.

#[path = "arm_fake.rs"]
mod arm_fake;
#[path = "graph_fake.rs"]
mod graph_fake;

use arm_fake::{
    last_auth_settings_put, mount_arm_fake, mount_easy_auth, mount_easy_auth_write,
    mount_permissions, mount_search_service, mount_storage_account, search_service_id,
};
use assert_cmd::Command;
use graph_fake::{
    FAKE_APP_ID, mount_graph, mount_graph_application_lookup, mount_graph_service_principal,
    mount_keyvault_secret,
};
use predicates::prelude::*;
use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const SUB: &str = "sub-a";
const RG: &str = "rg";
const SEARCH: &str = "srch";
const SEARCH_PID: &str = "00000000-0000-0000-0000-0000000000s1";
const UAMI_PID: &str = "00000000-0000-0000-0000-00000000aaaa";
const OPERATOR_OID: &str = "00000000-0000-0000-0000-0000000000op";
const OTHER_OID: &str = "00000000-0000-0000-0000-0000000000zz";

const BLOB_DATA_READER: &str = "2a2b9908-6ea1-4ae2-8e65-a410df84e7d1";
const SEARCH_SERVICE_CONTRIBUTOR: &str = "7ca78c08-252a-4471-8644-bb5ff32d4ba0";
const FOUNDRY_USER: &str = "53ca6127-db72-4b80-b1b0-d745d6d5456d";

const FOUNDRY: &str = "fndr";
const FOUNDRY_PROJECT: &str = "proj";

fn foundry_account_id() -> String {
    format!(
        "/subscriptions/{SUB}/resourceGroups/{RG}/providers/Microsoft.CognitiveServices/accounts/{FOUNDRY}"
    )
}

fn foundry_project_id() -> String {
    format!("{}/projects/{FOUNDRY_PROJECT}", foundry_account_id())
}

fn storage_id(name: &str) -> String {
    format!(
        "/subscriptions/{SUB}/resourceGroups/{RG}/providers/Microsoft.Storage/storageAccounts/{name}"
    )
}

fn uami_id() -> String {
    format!(
        "/subscriptions/{SUB}/resourceGroups/{RG}/providers/Microsoft.ManagedIdentity/userAssignedIdentities/rigg-mi"
    )
}

// ---------------------------------------------------------------- harness --

/// Unpadded base64url, so a test can mint the JWT `caller_object_id` decodes
/// without pulling in a base64 dependency for one string.
fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        let take = chunk.len() + 1;
        for i in 0..take {
            out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
        }
    }
    out
}

/// A bearer token whose payload names `oid` — what the operator checks read.
fn operator_token(oid: &str) -> String {
    let payload = json!({"oid": oid, "upn": "operator@example.com"}).to_string();
    format!(
        "{}.{}.{}",
        b64url(br#"{"alg":"none"}"#),
        b64url(payload.as_bytes()),
        "sig"
    )
}

fn rigg(dir: &std::path::Path, endpoint: &str) -> Command {
    let mut cmd = Command::cargo_bin("rigg").expect("binary builds");
    cmd.current_dir(dir);
    cmd.env("RIGG_NO_UPDATE_CHECK", "1");
    cmd.env_remove("RIGG_ENV");
    cmd.env("RIGG_ARM_ENDPOINT", endpoint);
    // One mock server serves ARM, Microsoft Graph and the Key Vault secrets
    // data plane: their path spaces (`/subscriptions/…`, `/applications` +
    // `/servicePrincipals`, `/secrets/…`) do not overlap.
    cmd.env("RIGG_GRAPH_ENDPOINT", endpoint);
    cmd.env("RIGG_KEYVAULT_ENDPOINT", endpoint);
    cmd.env("RIGG_ACCESS_TOKEN", operator_token(OPERATOR_OID));
    cmd.env("RIGG_NON_INTERACTIVE", "1");
    cmd.arg("--no-ai");
    cmd
}

/// A two-environment workspace whose `dev` env binds one storage account and
/// points at the mock server for both ARM and the Search data plane.
fn workspace(endpoint: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "name: acme\n\
             environments:\n\
             \x20 dev:\n\
             \x20   default: true\n\
             \x20   tenant: tenant-1\n\
             \x20   search: {{ service: {SEARCH}, endpoint: \"{endpoint}\" }}\n\
             \x20   dependencies:\n\
             \x20     docs: {{ storage: acct }}\n\
             \x20 prod:\n\
             \x20   search: {{ service: {SEARCH}, endpoint: \"{endpoint}\" }}\n"
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

/// Like [`workspace`], but `dev` also names a Foundry account and project —
/// what puts `<account>/projects/<project>` in the identity graph.
fn workspace_with_foundry(endpoint: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "name: acme\n\
             environments:\n\
             \x20 dev:\n\
             \x20   default: true\n\
             \x20   tenant: tenant-1\n\
             \x20   search: {{ service: {SEARCH}, endpoint: \"{endpoint}\" }}\n\
             \x20   foundry: {{ account: {FOUNDRY}, project: {FOUNDRY_PROJECT}, endpoint: \"{endpoint}\" }}\n"
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

/// A single-environment workspace whose only env is **protected** — the
/// shape the protected-gate ordering is asserted against.
fn workspace_protected(endpoint: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "name: acme\n\
             environments:\n\
             \x20 prod:\n\
             \x20   default: true\n\
             \x20   tenant: tenant-1\n\
             \x20   policy: {{ protected: true }}\n\
             \x20   search: {{ service: {SEARCH}, endpoint: \"{endpoint}\" }}\n\
             \x20   dependencies:\n\
             \x20     docs: {{ storage: acct }}\n"
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

fn write_resource(ws: &std::path::Path, dir: &str, name: &str, body: &Value) {
    write_resource_in(ws, "dev", dir, name, body);
}

fn write_resource_in(ws: &std::path::Path, env: &str, dir: &str, name: &str, body: &Value) {
    let d = ws
        .join("projects/demo/envs")
        .join(env)
        .join("search")
        .join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join(format!("{name}.json")),
        serde_json::to_string_pretty(body).unwrap(),
    )
    .unwrap();
}

/// One Foundry agent in `dev` — the resource kind that makes the operator's
/// Azure AI User edge (and therefore the project scope) part of the graph.
fn write_agent(ws: &std::path::Path, name: &str) {
    let d = ws.join("projects/demo/envs/dev/foundry/agents");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join(format!("{name}.json")),
        serde_json::to_string_pretty(&json!({"name": name, "model": "gpt-4o-mini"})).unwrap(),
    )
    .unwrap();
}

fn blob_data_source(identity: Option<&str>) -> Value {
    let mut doc = json!({
        "name": "docs",
        "type": "azureblob",
        "credentials": {"connectionString": format!("ResourceId={};", storage_id("acct"))},
        "container": {"name": "c"}
    });
    if let Some(id) = identity {
        doc["identity"] = json!({
            "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
            "userAssignedIdentity": id
        });
    }
    doc
}

/// The ARM resources every scenario shares.
async fn mount_base(server: &MockServer) {
    mount_arm_fake(
        server,
        &[SUB],
        &[
            ("searchServices", SEARCH, RG, "swedencentral"),
            ("storageAccounts", "acct", RG, "swedencentral"),
            ("userAssignedIdentities", "rigg-mi", RG, "swedencentral"),
        ],
    )
    .await;
}

/// A role-assignment listing at `scope` that answers per principal: the
/// shared fake matches on path alone, which is not enough once one scope is
/// queried for both a service identity and the operator.
async fn mount_assignments_for(
    server: &MockServer,
    scope: &str,
    principal: &str,
    role_ids: &[&str],
    description_env: &str,
) {
    let value: Vec<Value> = role_ids
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "id": format!("{scope}/providers/Microsoft.Authorization/roleAssignments/ra-{principal}-{i}"),
                "name": format!("ra-{principal}-{i}"),
                "properties": {
                    "roleDefinitionId": format!(
                        "/subscriptions/{SUB}/providers/Microsoft.Authorization/roleDefinitions/{r}"
                    ),
                    "principalId": principal,
                    "principalType": "ServicePrincipal",
                    // ARM reports the scope an assignment was made at; an
                    // `atScope()` listing carries inherited ones too, and
                    // `auth roles` keeps to the ones made here.
                    "scope": scope,
                    "description": format!("rigg:acme:{description_env}:test edge {i}")
                }
            })
        })
        .collect();
    let needle = principal.to_string();
    Mock::given(method("GET"))
        .and(path(format!(
            "{scope}/providers/Microsoft.Authorization/roleAssignments"
        )))
        .and(move |req: &Request| {
            req.url
                .query()
                .is_some_and(|q| q.contains(&needle) || !q.contains("assignedTo"))
        })
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": value})))
        .with_priority(1)
        .mount(server)
        .await;
}

/// Recorders for role-assignment writes: `PUT` bodies land in the server's
/// request log for assertions, `DELETE` always succeeds.
async fn mount_assignment_writes(server: &MockServer) {
    Mock::given(method("PUT"))
        .and(path_regex(
            r"^.*/providers/Microsoft\.Authorization/roleAssignments/[^/]+$",
        ))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"name": "created"})))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path_regex(
            r"^.*/providers/Microsoft\.Authorization/roleAssignments/[^/]+$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "deleted"})))
        .with_priority(1)
        .mount(server)
        .await;
}

/// A literal role-assignment listing at `scope`, for the cases that need to
/// control each assignment's own id and `properties.scope`.
async fn mount_assignment_listing(server: &MockServer, scope: &str, value: Vec<Value>) {
    Mock::given(method("GET"))
        .and(path(format!(
            "{scope}/providers/Microsoft.Authorization/roleAssignments"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": value})))
        .with_priority(1)
        .mount(server)
        .await;
}

/// One assignment document. `id_scope` is where its ARM id lives, `at` is
/// `properties.scope` — the scope it was actually made at, which is what
/// separates an at-scope grant from an inherited one.
fn assignment(id_scope: &str, name: &str, role: &str, at: &str, description: &str) -> Value {
    json!({
        "id": format!("{id_scope}/providers/Microsoft.Authorization/roleAssignments/{name}"),
        "name": name,
        "properties": {
            "roleDefinitionId": format!(
                "/subscriptions/{SUB}/providers/Microsoft.Authorization/roleDefinitions/{role}"
            ),
            "principalId": SEARCH_PID,
            "scope": at,
            "description": description
        }
    })
}

/// Every `DELETE` path the server saw, for the removal assertions.
async fn deleted_paths(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == wiremock::http::Method::DELETE)
        .map(|r| r.url.path().to_string())
        .collect()
}

/// An empty listing at every scope not explicitly mounted, so a scope with
/// no assignments answers `[]` instead of 404.
async fn mount_no_assignments(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path_regex(
            r"^.*/providers/Microsoft\.Authorization/roleAssignments$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
        .with_priority(3)
        .mount(server)
        .await;
}

// ------------------------------------------------------------- scenarios --

#[tokio::test(flavor = "multi_thread")]
async fn a_fully_wired_environment_is_green() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Storage Blob Data Reader"))
        .stdout(predicate::str::contains("0 missing"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_role_exits_4_and_prints_the_az_command() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    // The operator can grant at the storage scope, so `--fix` is on offer.
    mount_permissions(&server, &storage_id("acct"), true).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains(
            "az role assignment create --assignee",
        ))
        .stdout(predicate::str::contains("Storage Blob Data Reader"))
        .stdout(predicate::str::contains(storage_id("acct")));
}

#[tokio::test(flavor = "multi_thread")]
async fn fix_yes_creates_the_assignment_with_the_rigg_description() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &storage_id("acct"), true).await;
    mount_assignment_writes(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--fix", "--yes"])
        .assert()
        .success();

    let put = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| {
            r.method == wiremock::http::Method::PUT && r.url.path().contains("roleAssignments")
        })
        .expect("the fix PUTs a role assignment");
    let body: Value = serde_json::from_slice(&put.body).unwrap();
    assert_eq!(body["properties"]["principalId"], SEARCH_PID);
    assert_eq!(body["properties"]["principalType"], "ServicePrincipal");
    assert!(
        body["properties"]["roleDefinitionId"]
            .as_str()
            .unwrap()
            .ends_with(BLOB_DATA_READER)
    );
    let description = body["properties"]["description"].as_str().unwrap();
    assert!(
        description.starts_with("rigg:acme:dev:"),
        "description was {description}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn fix_never_grants_the_operator_their_own_roles() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // The service identity is wired; the only gap is the caller's own
    // Search Service Contributor, which rigg must never grant itself.
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &search_service_id(SUB, RG, SEARCH), true).await;
    mount_assignment_writes(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--fix", "--yes"])
        .assert()
        .code(4)
        // The operator's gap is reported with the command a human runs.
        .stdout(predicate::str::contains(format!(
            "az role assignment create --assignee {OPERATOR_OID}"
        )));

    let puts: Vec<Value> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| {
            r.method == wiremock::http::Method::PUT && r.url.path().contains("roleAssignments")
        })
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert!(
        !puts
            .iter()
            .any(|b| b["properties"]["principalId"] == OPERATOR_OID),
        "rigg must never grant the caller their own rights: {puts:?}"
    );
    assert!(puts.is_empty(), "nothing here was rigg's to fix: {puts:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn fix_without_yes_is_needs_input_non_interactively() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &storage_id("acct"), true).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--fix"])
        .assert()
        .code(6)
        .stdout(predicate::str::contains("auth.fix.all"))
        .stdout(predicate::str::contains("needs-input"));
}

#[tokio::test(flavor = "multi_thread")]
async fn no_search_identity_is_offered_as_a_fix_and_leaves_its_edges_unresolved() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server, SUB, RG, SEARCH, "standard", "None", "", true, "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &search_service_id(SUB, RG, SEARCH), true).await;
    mount_assignment_writes(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    // Reported: the identity check is missing, and the edge that needs it
    // cannot be judged at all.
    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains("no managed identity"));

    // --fix PATCHes the service.
    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--fix", "--yes"])
        .assert()
        .code(4); // the unresolved edge remains until the identity exists

    let patched = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .any(|r| {
            r.method == wiremock::http::Method::PATCH
                && r.url.path() == search_service_id(SUB, RG, SEARCH)
                && String::from_utf8_lossy(&r.body).contains("SystemAssigned")
        });
    assert!(patched, "--fix enables the system-assigned identity");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_firewalled_storage_account_without_the_bypass_is_a_setting_finding() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Deny",
        "Logging, Metrics",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains("storage firewall"))
        .stdout(predicate::str::contains("denies by default"))
        .stdout(predicate::str::contains("--bypass AzureServices"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_user_assigned_identity_on_firewalled_storage_reports_the_constraint() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned, UserAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Deny",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        UAMI_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "data-sources",
        "docs",
        &blob_data_source(Some(&uami_id())),
    );

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains(
            "trusted-services exception needs the system-assigned identity",
        ))
        .stdout(predicate::str::contains("network-rule add"));
}

#[tokio::test(flavor = "multi_thread")]
async fn principal_checks_operator_rights_for_someone_else() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    // The CI identity holds nothing at the search service.
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--principal", OTHER_OID])
        .assert()
        .code(4)
        .stdout(predicate::str::contains(format!("principal:{OTHER_OID}")))
        .stdout(predicate::str::contains("Search Service Contributor"));
}

#[tokio::test(flavor = "multi_thread")]
async fn json_output_carries_edges_checks_operator_and_summary() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &storage_id("acct"), false).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    let out = rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--output", "json"])
        .assert()
        .code(4)
        .get_output()
        .stdout
        .clone();
    let doc: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(doc["env"], "dev");
    let edge = &doc["edges"][0];
    assert_eq!(edge["principal"], "search-system");
    assert_eq!(edge["role"]["name"], "Storage Blob Data Reader");
    assert_eq!(edge["status"], "missing");
    assert_eq!(edge["scope"]["resolved"], storage_id("acct"));
    assert!(!edge["reason"].as_str().unwrap().is_empty());
    assert_eq!(edge["sources"][0]["path"], "credentials.connectionString");
    assert_eq!(edge["fix"]["kind"], "role-assignment");
    assert!(
        doc["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["id"] == "search-rbac-enabled")
    );
    assert!(
        doc["operator"]
            .as_array()
            .unwrap()
            .iter()
            .any(|o| o["id"].as_str().unwrap().starts_with("can-grant:"))
    );
    assert!(doc["summary"]["missing"].as_u64().unwrap() >= 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_narrows_the_graph_to_what_a_push_would_send() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &storage_id("acct"), true).await;
    // The data source already exists remotely, byte-identical to the file:
    // a push would send nothing, so `--plan` derives no edges at all.
    let doc = blob_data_source(None);
    Mock::given(method("GET"))
        .and(path("/datasources/docs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(doc.clone()))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &doc);

    // Without --plan the edge is derived (and missing).
    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains("Storage Blob Data Reader"));

    // With --plan the in-sync resource is out of scope.
    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--plan"])
        .assert()
        .stdout(predicate::str::contains("Storage Blob Data Reader").not());
}

#[tokio::test(flavor = "multi_thread")]
async fn live_attributes_an_auth_shaped_indexer_failure_to_its_storage_edge() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    Mock::given(method("GET"))
        .and(path("/indexers/docs-indexer/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "status": "error",
            "lastResult": {
                "status": "transientFailure",
                "errorMessage":
                    "This request is not authorized to perform this operation. \
                     Storage account 'acct' (403)"
            }
        })))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));
    write_resource(
        ws.path(),
        "indexers",
        "docs-indexer",
        &json!({"name": "docs-indexer", "dataSourceName": "docs", "targetIndexName": "idx"}),
    );

    // Without --live everything is in place; --live turns the proof into a
    // finding on the very edge that would explain it.
    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev"])
        .assert()
        .success();

    rigg(ws.path(), &server.uri())
        .args(["auth", "doctor", "-e", "dev", "--live"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains("live: indexer 'docs-indexer'"));
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_roles_lists_and_removes_only_riggs_own_assignments() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // One rigg-stamped assignment for this env; a hand-made one; a
    // rigg-stamped one *inherited* from the subscription; and one stamped for
    // the neighbouring env `dev2`, whose description starts with this env's
    // prefix but is not this env's. Only the first may be touched.
    let scope = storage_id("acct");
    let sub_scope = format!("/subscriptions/{SUB}");
    mount_assignment_listing(
        &server,
        &scope,
        vec![
            assignment(
                &scope,
                "rigg-one",
                BLOB_DATA_READER,
                &scope,
                "rigg:acme:dev:data source 'docs' reads blobs",
            ),
            assignment(
                &scope,
                "by-hand",
                BLOB_DATA_READER,
                &scope,
                "granted by the platform team",
            ),
            assignment(
                &sub_scope,
                "rigg-inherited",
                BLOB_DATA_READER,
                &sub_scope,
                "rigg:acme:dev:granted subscription-wide",
            ),
            assignment(
                &scope,
                "rigg-dev2",
                BLOB_DATA_READER,
                &scope,
                "rigg:acme:dev2:another environment's grant",
            ),
        ],
    )
    .await;
    mount_no_assignments(&server).await;
    mount_assignment_writes(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "list", "-e", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rigg:acme:dev:"))
        .stdout(predicate::str::contains("granted by the platform team").not())
        // An ancestor's grant is visible at this scope but was not made here.
        .stdout(predicate::str::contains("granted subscription-wide").not())
        // `rigg:acme:dev` must not match `rigg:acme:dev2:…`.
        .stdout(predicate::str::contains("another environment's grant").not());

    // Removal needs an answer; without one it is exit 6, not a silent delete.
    rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "remove", "-e", "dev"])
        .assert()
        .code(6)
        .stdout(predicate::str::contains("auth.roles.remove"));

    rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "remove", "-e", "dev", "--yes"])
        .assert()
        .success();
    let deleted = deleted_paths(&server).await;
    assert!(deleted.iter().any(|p| p.ends_with("rigg-one")));
    assert!(
        !deleted.iter().any(|p| p.ends_with("by-hand")),
        "an assignment rigg did not create is never removed: {deleted:?}"
    );
    assert!(
        !deleted.iter().any(|p| p.ends_with("rigg-inherited")),
        "an assignment inherited from an ancestor scope is never removed: {deleted:?}"
    );
    assert!(
        !deleted.iter().any(|p| p.ends_with("rigg-dev2")),
        "a neighbouring environment's assignment is never removed: {deleted:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_roles_reaches_the_foundry_project_scope() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &[SUB],
        &[
            ("searchServices", SEARCH, RG, "swedencentral"),
            ("accounts", FOUNDRY, RG, "swedencentral"),
        ],
    )
    .await;
    // The assignment `auth doctor` creates for the operator's Azure AI User
    // edge lives at `<account>/projects/<project>`, not at the account.
    let project = foundry_project_id();
    mount_assignment_listing(
        &server,
        &project,
        vec![assignment(
            &project,
            "rigg-project",
            FOUNDRY_USER,
            &project,
            "rigg:acme:dev:agents live on the project",
        )],
    )
    .await;
    mount_no_assignments(&server).await;
    mount_assignment_writes(&server).await;

    let ws = workspace_with_foundry(&server.uri());
    write_agent(ws.path(), "assistant");

    rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "list", "-e", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("agents live on the project"))
        .stdout(predicate::str::contains(&project));

    rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "remove", "-e", "dev", "--yes"])
        .assert()
        .success();
    let deleted = deleted_paths(&server).await;
    assert!(
        deleted.iter().any(|p| p.ends_with("rigg-project")),
        "the project-scope assignment must be removable: {deleted:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn auth_roles_reports_one_assignment_reachable_from_two_scopes_once() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // The very same assignment id answers at two of the scopes the graph
    // knows: one row, one DELETE.
    let storage = storage_id("acct");
    let search = search_service_id(SUB, RG, SEARCH);
    let shared = |at: &str| {
        assignment(
            &storage,
            "rigg-shared",
            BLOB_DATA_READER,
            at,
            "rigg:acme:dev:reachable from two scopes",
        )
    };
    mount_assignment_listing(&server, &storage, vec![shared(&storage)]).await;
    mount_assignment_listing(&server, &search, vec![shared(&search)]).await;
    mount_no_assignments(&server).await;
    mount_assignment_writes(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    let out = rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "list", "-e", "dev"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out.matches("reachable from two scopes").count(),
        1,
        "one assignment, one row: {out}"
    );

    rigg(ws.path(), &server.uri())
        .args(["auth", "roles", "remove", "-e", "dev", "--yes"])
        .assert()
        .success();
    let deleted = deleted_paths(&server).await;
    assert_eq!(
        deleted
            .iter()
            .filter(|p| p.ends_with("rigg-shared"))
            .count(),
        1,
        "one assignment, one DELETE: {deleted:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_auth_adds_one_identity_line_per_environment() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_no_assignments(&server).await;
    mount_permissions(&server, &storage_id("acct"), true).await;
    for p in [
        "datasources",
        "indexes",
        "skillsets",
        "indexers",
        "synonymmaps",
        "aliases",
        "knowledgeSources",
        "knowledgeBases",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
    }

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["status", "-e", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains("identity:").not());

    rigg(ws.path(), &server.uri())
        .args(["status", "-e", "dev", "--auth"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rigg auth doctor -e dev"));
}

// ---------------------------------------------------- push preflight ------

/// Data-plane mocks a one-data-source push needs: the resource does not
/// exist remotely, and the PUT echoes what was sent.
async fn mount_datasource_push(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/datasources/docs"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/datasources/docs"))
        .respond_with(|req: &Request| {
            ResponseTemplate::new(201)
                .set_body_json(serde_json::from_slice::<Value>(&req.body).unwrap())
        })
        .mount(server)
        .await;
}

/// Every data-plane PUT path the server saw, in order.
async fn data_plane_puts(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == wiremock::http::Method::PUT)
        .map(|r| r.url.path().to_string())
        .collect()
}

/// The service identity is fully wired, but the *operator* holds nothing:
/// rigg never grants a caller their own rights, so the preflight refuses
/// with exit 4 before a single resource is written — even under `--yes`.
#[tokio::test(flavor = "multi_thread")]
async fn push_refuses_before_writing_when_the_operator_cannot_do_the_push() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    mount_datasource_push(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["push", "demo", "-e", "dev", "--yes"])
        .assert()
        .code(4)
        .stdout(predicate::str::contains("auth preflight"))
        .stdout(predicate::str::contains("az role assignment create"))
        .stderr(predicate::str::contains("--skip-auth-preflight"));

    assert!(
        data_plane_puts(&server).await.is_empty(),
        "nothing may be written when the preflight refuses"
    );
}

/// `--skip-auth-preflight` is the escape hatch: same environment, the push
/// goes through.
#[tokio::test(flavor = "multi_thread")]
async fn skip_auth_preflight_pushes_anyway() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    mount_no_assignments(&server).await;
    mount_datasource_push(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args([
            "push",
            "demo",
            "-e",
            "dev",
            "--yes",
            "--skip-auth-preflight",
        ])
        .assert()
        .success();

    assert!(
        data_plane_puts(&server)
            .await
            .contains(&"/datasources/docs".to_string()),
        "the push proceeds when the preflight is skipped"
    );
}

/// The whole point of the preflight: the missing role is granted, rigg waits
/// until ARM reports it at the scope, and only then writes the resource.
#[tokio::test(flavor = "multi_thread")]
async fn push_grants_the_missing_role_waits_for_it_then_pushes() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // The operator can do the push and can grant at the storage scope.
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "dev",
    )
    .await;
    mount_permissions(&server, &storage_id("acct"), true).await;
    mount_assignment_writes(&server).await;
    // The storage scope answers "nothing yet" until the grant lands, then
    // reports it — the propagation the preflight waits out.
    Mock::given(method("GET"))
        .and(path(format!(
            "{}/providers/Microsoft.Authorization/roleAssignments",
            storage_id("acct")
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    mount_datasource_push(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .env("RIGG_RBAC_RETRY_SECS", "0")
        .env("RIGG_RBAC_MAX_RETRIES", "3")
        .args(["push", "demo", "-e", "dev", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Storage Blob Data Reader"))
        .stdout(predicate::str::contains("is visible"));

    let puts = data_plane_puts(&server).await;
    let role = puts
        .iter()
        .position(|p| p.contains("roleAssignments"))
        .expect("the preflight grants the missing role");
    let resource = puts
        .iter()
        .position(|p| p == "/datasources/docs")
        .expect("the push proceeds after the grant");
    assert!(role < resource, "grant must precede the write: {puts:?}");
}

/// Every role-assignment PUT the server saw.
async fn role_assignment_puts(server: &MockServer) -> Vec<String> {
    data_plane_puts(server)
        .await
        .into_iter()
        .filter(|p| p.contains("roleAssignments"))
        .collect()
}

/// The preflight *verifies* before the protected-environment gate, but must
/// not *change* anything before it: `--yes` alone never satisfies that gate,
/// so the push stops there (exit 6) — and not one role assignment was
/// created for a push that never happened.
#[tokio::test(flavor = "multi_thread")]
async fn push_to_a_protected_env_grants_nothing_before_the_gate() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // The operator can do the push and can grant at the storage scope, so
    // the missing Storage Blob Data Reader is a fix rigg would apply itself
    // — exactly the case that must NOT be applied before the gate.
    mount_assignments_for(
        &server,
        &search_service_id(SUB, RG, SEARCH),
        OPERATOR_OID,
        &[SEARCH_SERVICE_CONTRIBUTOR],
        "prod",
    )
    .await;
    mount_permissions(&server, &storage_id("acct"), true).await;
    mount_assignment_writes(&server).await;
    mount_no_assignments(&server).await;
    mount_datasource_push(&server).await;

    let ws = workspace_protected(&server.uri());
    write_resource_in(
        ws.path(),
        "prod",
        "data-sources",
        "docs",
        &blob_data_source(None),
    );

    rigg(ws.path(), &server.uri())
        .env("RIGG_RBAC_RETRY_SECS", "0")
        .env("RIGG_RBAC_MAX_RETRIES", "1")
        .args(["push", "demo", "-e", "prod", "--yes"])
        .assert()
        .code(6)
        .stdout(predicate::str::contains("confirm.protected.prod"));

    assert!(
        role_assignment_puts(&server).await.is_empty(),
        "the protected gate must come before every grant: {:?}",
        data_plane_puts(&server).await
    );
    assert!(
        data_plane_puts(&server).await.is_empty(),
        "nothing at all may be written before the gate"
    );
}

/// A preview reports the whole remediation — the `az` lines only a human can
/// run *and* what rigg would fix itself — and does neither: no refusal, no
/// grant, no write.
#[tokio::test(flavor = "multi_thread")]
async fn push_dry_run_reports_auth_problems_without_refusing_or_granting() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // Nobody holds anything: the service identity's missing role is a fix
    // rigg could apply, the operator's own rights are not.
    mount_permissions(&server, &storage_id("acct"), true).await;
    mount_assignment_writes(&server).await;
    mount_no_assignments(&server).await;
    mount_datasource_push(&server).await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    rigg(ws.path(), &server.uri())
        .args(["push", "demo", "-e", "dev", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("auth preflight"))
        .stdout(predicate::str::contains("az role assignment create"))
        .stdout(predicate::str::contains("rigg can fix:"))
        .stdout(predicate::str::contains("dry run — nothing granted"));

    assert!(
        data_plane_puts(&server).await.is_empty(),
        "a preview writes nothing — neither resources nor grants"
    );
}

/// When the PUT-time diagnosis finds requirements rigg may not grant (the
/// operator's own), the retry loop is pointless: rigg says so, prints the
/// `az` line, and stops — instead of re-PUTting for five minutes.
#[tokio::test(flavor = "multi_thread")]
async fn a_requirement_rigg_may_not_grant_stops_the_retry_loop() {
    let server = MockServer::start().await;
    mount_base(&server).await;
    mount_search_service(
        &server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
    mount_storage_account(
        &server,
        &storage_id("acct"),
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    // The service identity is fully wired — so the diagnosis has no fix to
    // offer — while the operator holds nothing.
    mount_assignments_for(
        &server,
        &storage_id("acct"),
        SEARCH_PID,
        &[BLOB_DATA_READER],
        "dev",
    )
    .await;
    mount_no_assignments(&server).await;
    Mock::given(method("GET"))
        .and(path("/datasources/docs"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/datasources/docs"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {
            "code": "InvalidRequestParameter",
            "message": "Cannot access the storage account: the managed identity does not have permission."
        }})))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));

    // The preflight would refuse first, so skip it: this is the PUT-time
    // diagnosis, the safety net behind it.
    rigg(ws.path(), &server.uri())
        .env("RIGG_RBAC_RETRY_SECS", "0")
        .env("RIGG_RBAC_MAX_RETRIES", "3")
        .args([
            "push",
            "demo",
            "-e",
            "dev",
            "--yes",
            "--skip-auth-preflight",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("az role assignment create"))
        .stderr(predicate::str::contains("waiting will not help"));

    let attempts = data_plane_puts(&server)
        .await
        .into_iter()
        .filter(|p| p == "/datasources/docs")
        .count();
    assert_eq!(
        attempts, 1,
        "no retry loop for a problem propagation cannot solve"
    );
}

// --------------------------------------------------------- rigg verify ----

/// The Search + Foundry runtime endpoints `rigg verify` exercises.
async fn mount_runtime(server: &MockServer, indexer_status: Value) {
    Mock::given(method("POST"))
        .and(path("/indexers/docs-indexer/run"))
        .respond_with(ResponseTemplate::new(202))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/indexers/docs-indexer/status"))
        .respond_with(ResponseTemplate::new(200).set_body_json(indexer_status))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/knowledgebases.*/retrieve$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "response": [{"content": [{"type": "text", "text": "[]"}]}],
            "activity": [{"knowledgeSourceName": "docs-ks"}],
            "references": []
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/api/projects/{FOUNDRY_PROJECT}/openai/v1/responses"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"output_text": "OK"})))
        .mount(server)
        .await;
}

fn write_verifiable_tree(ws: &std::path::Path) {
    write_resource(ws, "data-sources", "docs", &blob_data_source(None));
    write_resource(
        ws,
        "indexers",
        "docs-indexer",
        &json!({"name": "docs-indexer", "dataSourceName": "docs", "targetIndexName": "idx"}),
    );
    write_resource(
        ws,
        "knowledge-bases",
        "docs-kb",
        &json!({"name": "docs-kb", "knowledgeSources": [{"name": "docs-ks"}]}),
    );
    write_agent(ws, "regulus");
}

#[tokio::test(flavor = "multi_thread")]
async fn verify_runs_every_indexer_knowledge_base_and_agent() {
    let server = MockServer::start().await;
    mount_runtime(
        &server,
        json!({
            "status": "running",
            "lastResult": {"status": "success", "itemsProcessed": 7, "itemsFailed": 0}
        }),
    )
    .await;

    let ws = workspace_with_foundry(&server.uri());
    write_verifiable_tree(ws.path());

    rigg(ws.path(), &server.uri())
        .env("RIGG_WATCH_INTERVAL_SECS", "0")
        .args(["verify", "demo", "-e", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "indexer 'docs-indexer' — 7 processed",
        ))
        .stdout(predicate::str::contains(
            "knowledge base 'docs-kb' retrieved",
        ))
        .stdout(predicate::str::contains("agent 'regulus' replied"))
        .stdout(predicate::str::contains("3 check(s) passed"));
}

/// A failed run whose message looks like an authorization problem is
/// attributed to the identity edge that would explain it, and fails the run.
#[tokio::test(flavor = "multi_thread")]
async fn verify_attributes_an_auth_shaped_indexer_failure_and_exits_1() {
    let server = MockServer::start().await;
    mount_runtime(
        &server,
        json!({
            "status": "error",
            "lastResult": {
                "status": "error",
                "errorMessage":
                    "This request is not authorized to perform this operation. \
                     Storage account 'acct' (403)"
            }
        }),
    )
    .await;

    let ws = workspace_with_foundry(&server.uri());
    write_resource(ws.path(), "data-sources", "docs", &blob_data_source(None));
    write_resource(
        ws.path(),
        "indexers",
        "docs-indexer",
        &json!({"name": "docs-indexer", "dataSourceName": "docs", "targetIndexName": "idx"}),
    );

    rigg(ws.path(), &server.uri())
        .env("RIGG_WATCH_INTERVAL_SECS", "0")
        .args(["verify", "demo", "-e", "dev"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("✗ indexer 'docs-indexer'"))
        .stdout(predicate::str::contains("→ likely"))
        .stderr(predicate::str::contains("1 of 1 verification(s) failed"));
}

// ------------------------------------------------ easy auth / key sources --

const SEARCH_MI_CLIENT_ID: &str = "00000000-0000-0000-0000-0000000000c1";
const FUNCTION_APP: &str = "fn";

/// A `dev` environment that also declares the dependencies the Easy Auth,
/// key-vault and `--identity` scenarios bind against.
fn workspace_with_deps(endpoint: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "name: acme\n\
             environments:\n\
             \x20 dev:\n\
             \x20   default: true\n\
             \x20   tenant: tenant-1\n\
             \x20   subscription: {SUB}\n\
             \x20   search: {{ service: {SEARCH}, endpoint: \"{endpoint}\" }}\n\
             \x20   dependencies:\n\
             \x20     docs: {{ storage: acct }}\n\
             \x20     enrich-fn: {{ function-app: {FUNCTION_APP} }}\n\
             \x20     secrets: {{ key-vault: kv }}\n\
             \x20     pipeline-mi: {{ identity: rigg-mi }}\n"
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

/// The ARM resources the Easy Auth scenarios need on top of [`mount_base`].
async fn mount_easy_auth_base(server: &MockServer) {
    mount_arm_fake(
        server,
        &[SUB],
        &[
            ("searchServices", SEARCH, RG, "swedencentral"),
            ("storageAccounts", "acct", RG, "swedencentral"),
            ("userAssignedIdentities", "rigg-mi", RG, "swedencentral"),
            ("vaults", "kv", RG, "swedencentral"),
            ("sites", FUNCTION_APP, RG, "swedencentral"),
        ],
    )
    .await;
    mount_search_service(
        server,
        SUB,
        RG,
        SEARCH,
        "standard",
        "SystemAssigned",
        SEARCH_PID,
        true,
        "Enabled",
    )
    .await;
}

fn webapi_skillset(name: &str, extra: Value) -> Value {
    let mut skill = json!({
        "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
        "name": "enrich",
        "uri": format!("https://{FUNCTION_APP}.azurewebsites.net/api/enrich?code=<redacted>"),
        "httpHeaders": {"x-functions-key": "<redacted>"},
        "inputs": [],
        "outputs": []
    });
    if let (Some(s), Some(e)) = (skill.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            s.insert(k.clone(), v.clone());
        }
    }
    json!({"name": name, "skills": [skill]})
}

fn read_resource(ws: &std::path::Path, dir: &str, name: &str) -> Value {
    let path = ws
        .join("projects/demo/envs/dev/search")
        .join(dir)
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

/// The whole §5 wiring in one run: Graph registers the application and its
/// enterprise app, ARM gets a MERGED authsettingsV2, and the local skillset
/// becomes keyless — without pushing anything.
#[tokio::test(flavor = "multi_thread")]
async fn easy_auth_registers_the_app_merges_settings_and_makes_the_skillset_keyless() {
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;
    // The app already has a Google provider and its own login settings; both
    // must survive the merge.
    Mock::given(method("POST"))
        .and(path_regex(format!(
            r"^.*/sites/{FUNCTION_APP}/config/authsettingsV2/list$"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "properties": {
                "platform": {"enabled": false, "runtimeVersion": "~1"},
                "identityProviders": {"google": {"enabled": true}},
                "login": {"tokenStore": {"enabled": true}}
            }
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    mount_easy_auth_write(&server, FUNCTION_APP).await;
    mount_graph(&server).await;
    mount_graph_service_principal(&server, SEARCH_PID, SEARCH_MI_CLIENT_ID).await;

    let ws = workspace_with_deps(&server.uri());
    write_resource(
        ws.path(),
        "skillsets",
        "webss",
        &webapi_skillset("webss", json!({})),
    );

    rigg(ws.path(), &server.uri())
        .args(["auth", "easy-auth", "enrich-fn", "-e", "dev", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains(format!("api://{FAKE_APP_ID}")))
        .stdout(predicate::str::contains("rigg push"));

    let put = last_auth_settings_put(&server)
        .await
        .expect("authsettingsV2 was written");
    let props = &put["properties"];
    assert_eq!(props["platform"]["enabled"], json!(true));
    assert_eq!(props["platform"]["runtimeVersion"], json!("~1"), "kept");
    assert_eq!(
        props["globalValidation"]["requireAuthentication"],
        json!(true)
    );
    assert_eq!(
        props["globalValidation"]["unauthenticatedClientAction"],
        json!("Return401")
    );
    assert_eq!(props["identityProviders"]["google"]["enabled"], json!(true));
    assert_eq!(props["login"]["tokenStore"]["enabled"], json!(true));
    let aad = &props["identityProviders"]["azureActiveDirectory"];
    assert_eq!(aad["enabled"], json!(true));
    assert_eq!(aad["registration"]["clientId"], json!(FAKE_APP_ID));
    assert_eq!(
        aad["registration"]["openIdIssuer"],
        json!("https://login.microsoftonline.com/tenant-1/v2.0")
    );
    assert_eq!(
        aad["validation"]["allowedAudiences"],
        json!([format!("api://{FAKE_APP_ID}")])
    );
    assert_eq!(
        aad["validation"]["defaultAuthorizationPolicy"]["allowedApplications"],
        json!([SEARCH_MI_CLIENT_ID]),
        "the search service's system identity is what calls the function"
    );

    let skill = read_resource(ws.path(), "skillsets", "webss")["skills"][0].clone();
    assert_eq!(
        skill["authResourceId"],
        json!(format!("api://{FAKE_APP_ID}"))
    );
    assert_eq!(
        skill["uri"],
        json!(format!(
            "https://{FUNCTION_APP}.azurewebsites.net/api/enrich"
        )),
        "the redacted code parameter goes with the key"
    );
    assert!(skill.get("httpHeaders").is_none_or(|h| {
        h.as_object()
            .is_none_or(|m| !m.contains_key("x-functions-key"))
    }));
    assert!(skill.get("x-rigg-auth").is_none());
}

/// A skillset that authenticates through a user-assigned identity: THAT
/// identity's client id is what Easy Auth must admit, not the search
/// service's (spec §7).
#[tokio::test(flavor = "multi_thread")]
async fn easy_auth_admits_the_skillsets_user_assigned_identity_when_it_declares_one() {
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;
    mount_easy_auth(&server, FUNCTION_APP, false, "").await;
    mount_easy_auth_write(&server, FUNCTION_APP).await;
    mount_graph(&server).await;
    mount_graph_service_principal(&server, SEARCH_PID, SEARCH_MI_CLIENT_ID).await;

    let ws = workspace_with_deps(&server.uri());
    write_resource(
        ws.path(),
        "skillsets",
        "webss",
        &webapi_skillset(
            "webss",
            json!({"authIdentity": {
                "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
                "userAssignedIdentity": uami_id()
            }}),
        ),
    );

    rigg(ws.path(), &server.uri())
        .args(["auth", "easy-auth", "enrich-fn", "-e", "dev", "--yes"])
        .assert()
        .success();

    let put = last_auth_settings_put(&server).await.unwrap();
    // The ARM fake reports every managed identity's clientId as this value.
    assert_eq!(
        put["properties"]["identityProviders"]["azureActiveDirectory"]["validation"]["defaultAuthorizationPolicy"]
            ["allowedApplications"],
        json!(["00000000-0000-0000-0000-00000000cccc"])
    );
}

/// `--client-id` reuses an existing app registration instead of creating
/// one — no `POST /applications` at all.
#[tokio::test(flavor = "multi_thread")]
async fn easy_auth_reuses_the_registration_named_by_client_id() {
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;
    mount_easy_auth(&server, FUNCTION_APP, false, "").await;
    mount_easy_auth_write(&server, FUNCTION_APP).await;
    mount_graph_application_lookup(&server, true).await;
    mount_graph(&server).await;
    mount_graph_service_principal(&server, SEARCH_PID, SEARCH_MI_CLIENT_ID).await;

    let ws = workspace_with_deps(&server.uri());
    rigg(ws.path(), &server.uri())
        .args([
            "auth",
            "easy-auth",
            "enrich-fn",
            "-e",
            "dev",
            "--client-id",
            FAKE_APP_ID,
            "--yes",
        ])
        .assert()
        .success();

    let created = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method == wiremock::http::Method::POST && r.url.path() == "/applications")
        .count();
    assert_eq!(
        created, 0,
        "an existing registration is reused, not re-created"
    );
    let put = last_auth_settings_put(&server).await.unwrap();
    assert_eq!(
        put["properties"]["identityProviders"]["azureActiveDirectory"]["registration"]["clientId"],
        json!(FAKE_APP_ID)
    );
}

/// Non-interactively and without `--yes`, the confirmation is a question:
/// exit 6, the id named, and not one byte written to Azure or to the file.
#[tokio::test(flavor = "multi_thread")]
async fn easy_auth_asks_before_changing_anything() {
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;
    mount_easy_auth(&server, FUNCTION_APP, false, "").await;
    mount_easy_auth_write(&server, FUNCTION_APP).await;
    mount_graph(&server).await;
    mount_graph_service_principal(&server, SEARCH_PID, SEARCH_MI_CLIENT_ID).await;

    let ws = workspace_with_deps(&server.uri());
    let before = webapi_skillset("webss", json!({}));
    write_resource(ws.path(), "skillsets", "webss", &before);

    rigg(ws.path(), &server.uri())
        .args(["auth", "easy-auth", "enrich-fn", "-e", "dev"])
        .assert()
        .code(6)
        .stdout(predicate::str::contains(format!(
            "auth.easyauth.{FUNCTION_APP}"
        )));

    assert!(
        last_auth_settings_put(&server).await.is_none(),
        "nothing may be written before the confirmation"
    );
    let after = read_resource(ws.path(), "skillsets", "webss");
    assert!(after["skills"][0].get("authResourceId").is_none());
}

/// The positional is a binding name, and it must name a function app.
#[tokio::test(flavor = "multi_thread")]
async fn easy_auth_rejects_a_binding_that_is_not_a_function_app() {
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;

    let ws = workspace_with_deps(&server.uri());
    rigg(ws.path(), &server.uri())
        .args(["auth", "easy-auth", "docs", "-e", "dev", "--yes"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("is a storage binding"));

    rigg(ws.path(), &server.uri())
        .args(["auth", "easy-auth", "nope", "-e", "dev", "--yes"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains(
            "rigg env bind dev nope function-app:",
        ));
}

/// The `key-vault:<secret>@<binding>` key source: the secret is read from
/// the vault at push time, lands ONLY in the outgoing body, and never
/// reaches the file, stdout or stderr.
#[tokio::test(flavor = "multi_thread")]
async fn push_injects_the_key_vault_secret_into_the_body_only() {
    const SECRET: &str = "s3cr3t-function-key-value";
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;
    mount_keyvault_secret(&server, "fn-key", SECRET).await;
    for p in [
        "datasources",
        "indexes",
        "skillsets",
        "indexers",
        "synonymmaps",
        "aliases",
        "knowledgeSources",
        "knowledgeBases",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/skillsets/webss"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;
    // Azure echoes the pushed document with every secret redacted, and push
    // canonicalization writes that echo back to disk — so the fake redacts
    // too, and the on-disk assertion below is a real end-to-end check.
    Mock::given(method("PUT"))
        .and(path("/skillsets/webss"))
        .respond_with(|req: &Request| {
            let mut doc: Value = serde_json::from_slice(&req.body).unwrap();
            doc["skills"][0]["httpHeaders"]["x-functions-key"] = json!("<redacted>");
            ResponseTemplate::new(201).set_body_json(doc)
        })
        .mount(&server)
        .await;

    let ws = workspace_with_deps(&server.uri());
    write_resource(
        ws.path(),
        "skillsets",
        "webss",
        &webapi_skillset("webss", json!({"x-rigg-auth": "key-vault:fn-key@secrets"})),
    );

    let out = rigg(ws.path(), &server.uri())
        .args([
            "push",
            "demo",
            "-e",
            "dev",
            "--yes",
            "--skip-auth-preflight",
        ])
        .assert()
        .success()
        .get_output()
        .clone();
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!printed.contains(SECRET), "the key must never be printed");

    let put = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method == wiremock::http::Method::PUT && r.url.path() == "/skillsets/webss")
        .expect("the skillset was pushed");
    let body: Value = serde_json::from_slice(&put.body).unwrap();
    assert_eq!(
        body["skills"][0]["httpHeaders"]["x-functions-key"],
        json!(SECRET),
        "the key goes in the carrier the skill already uses"
    );
    assert!(
        body["skills"][0].get("x-rigg-auth").is_none(),
        "x-rigg-* keys are stripped before the PUT"
    );

    // The file keeps the annotation and the placeholder — never the value.
    let on_disk = std::fs::read_to_string(
        ws.path()
            .join("projects/demo/envs/dev/search/skillsets/webss.json"),
    )
    .unwrap();
    assert!(!on_disk.contains(SECRET), "no secret on disk");
    assert!(on_disk.contains("key-vault:fn-key@secrets"));
}

/// `validate` accepts the annotation only when the binding really is a key
/// vault — a typo must not become a push-time failure mid-plan.
#[tokio::test(flavor = "multi_thread")]
async fn validate_checks_the_key_vault_binding_behind_the_annotation() {
    let server = MockServer::start().await;
    let ws = workspace_with_deps(&server.uri());

    write_resource(
        ws.path(),
        "skillsets",
        "good",
        &webapi_skillset("good", json!({"x-rigg-auth": "key-vault:fn-key@secrets"})),
    );
    rigg(ws.path(), &server.uri())
        .args(["validate", "demo"])
        .assert()
        .success();

    write_resource(
        ws.path(),
        "skillsets",
        "wrong-kind",
        &webapi_skillset(
            "wrong-kind",
            json!({"x-rigg-auth": "key-vault:fn-key@docs"}),
        ),
    );
    rigg(ws.path(), &server.uri())
        .args(["validate", "demo"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("not a key-vault dependency"));

    std::fs::remove_file(
        ws.path()
            .join("projects/demo/envs/dev/search/skillsets/wrong-kind.json"),
    )
    .unwrap();
    write_resource(
        ws.path(),
        "skillsets",
        "unknown",
        &webapi_skillset("unknown", json!({"x-rigg-auth": "whatever"})),
    );
    rigg(ws.path(), &server.uri())
        .args(["validate", "demo"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("unknown \"x-rigg-auth\" value"));
}

/// `rigg new … --identity <binding>` resolves the binding and writes the
/// DataUserAssignedIdentity object (spec §7).
#[tokio::test(flavor = "multi_thread")]
async fn new_with_identity_writes_the_user_assigned_identity_object() {
    let server = MockServer::start().await;
    mount_easy_auth_base(&server).await;

    let ws = workspace_with_deps(&server.uri());
    rigg(ws.path(), &server.uri())
        .args([
            "new",
            "data-source",
            "docs",
            "-p",
            "demo",
            "-e",
            "dev",
            "--identity",
            "pipeline-mi",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pipeline-mi"));

    let doc = read_resource(ws.path(), "data-sources", "docs");
    assert_eq!(
        doc["identity"]["@odata.type"],
        json!("#Microsoft.Azure.Search.DataUserAssignedIdentity")
    );
    assert_eq!(doc["identity"]["userAssignedIdentity"], json!(uami_id()));

    // A kind with no identity field is a usage error naming the ones that
    // do; a binding of the wrong type is a validation error.
    rigg(ws.path(), &server.uri())
        .args([
            "new",
            "indexer",
            "ix",
            "-p",
            "demo",
            "-e",
            "dev",
            "--identity",
            "pipeline-mi",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("data-source"));
    rigg(ws.path(), &server.uri())
        .args([
            "new",
            "data-source",
            "other",
            "-p",
            "demo",
            "-e",
            "dev",
            "--identity",
            "docs",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("not an identity binding"));
}
