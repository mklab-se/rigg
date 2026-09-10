//! `rigg auth doctor` / `rigg auth roles` / `rigg status --auth` end to end
//! against the wiremock ARM fake.
//!
//! One mock server serves both planes: ARM lives under `/subscriptions/…`
//! and the Search data plane under `/datasources`, `/indexers/…`, so a single
//! `RIGG_ARM_ENDPOINT` + `endpoint:` pair covers `--plan` and `--live` too.

#[path = "arm_fake.rs"]
mod arm_fake;

use arm_fake::{
    mount_arm_fake, mount_permissions, mount_search_service, mount_storage_account,
    search_service_id,
};
use assert_cmd::Command;
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

fn write_resource(ws: &std::path::Path, dir: &str, name: &str, body: &Value) {
    let d = ws.join("projects/demo/envs/dev/search").join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join(format!("{name}.json")),
        serde_json::to_string_pretty(body).unwrap(),
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
    // One rigg-stamped assignment, a hand-made one, and a rigg-stamped one
    // *inherited* from the subscription — the last two must both survive.
    let scope = storage_id("acct");
    Mock::given(method("GET"))
        .and(path(format!(
            "{scope}/providers/Microsoft.Authorization/roleAssignments"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [
            {
                "id": format!("{scope}/providers/Microsoft.Authorization/roleAssignments/rigg-one"),
                "name": "rigg-one",
                "properties": {
                    "roleDefinitionId": format!("/subscriptions/{SUB}/providers/Microsoft.Authorization/roleDefinitions/{BLOB_DATA_READER}"),
                    "principalId": SEARCH_PID,
                    "scope": scope,
                    "description": "rigg:acme:dev:data source 'docs' reads blobs"
                }
            },
            {
                "id": format!("{scope}/providers/Microsoft.Authorization/roleAssignments/by-hand"),
                "name": "by-hand",
                "properties": {
                    "roleDefinitionId": format!("/subscriptions/{SUB}/providers/Microsoft.Authorization/roleDefinitions/{BLOB_DATA_READER}"),
                    "principalId": SEARCH_PID,
                    "scope": scope,
                    "description": "granted by the platform team"
                }
            },
            {
                "id": format!("/subscriptions/{SUB}/providers/Microsoft.Authorization/roleAssignments/rigg-inherited"),
                "name": "rigg-inherited",
                "properties": {
                    "roleDefinitionId": format!("/subscriptions/{SUB}/providers/Microsoft.Authorization/roleDefinitions/{BLOB_DATA_READER}"),
                    "principalId": SEARCH_PID,
                    "scope": format!("/subscriptions/{SUB}"),
                    "description": "rigg:acme:dev:granted subscription-wide"
                }
            }
        ]})))
        .with_priority(1)
        .mount(&server)
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
        .stdout(predicate::str::contains("granted subscription-wide").not());

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
    let deleted: Vec<String> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method == wiremock::http::Method::DELETE)
        .map(|r| r.url.path().to_string())
        .collect();
    assert!(deleted.iter().any(|p| p.ends_with("rigg-one")));
    assert!(
        !deleted.iter().any(|p| p.ends_with("by-hand")),
        "an assignment rigg did not create is never removed: {deleted:?}"
    );
    assert!(
        !deleted.iter().any(|p| p.ends_with("rigg-inherited")),
        "an assignment inherited from an ancestor scope is never removed: {deleted:?}"
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
