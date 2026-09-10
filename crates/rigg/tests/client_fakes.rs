//! `ArmClient` / `GraphClient` / Key Vault client behaviour against the
//! wiremock fakes — the identity-and-auth reads, the RBAC helpers, the Easy
//! Auth Graph flow, and secret retrieval.
//!
//! Every client is constructed with an explicit token and base URL, so no
//! test in this binary touches a process env var.

#[path = "arm_fake.rs"]
mod arm_fake;
#[path = "graph_fake.rs"]
mod graph_fake;

use arm_fake::{
    mount_cognitive_account, mount_create_uami, mount_permissions, mount_role_assignments,
    mount_search_service, mount_storage_account, search_service_id,
};
use graph_fake::{
    FAKE_APP_ID, FAKE_APP_OBJECT_ID, FAKE_SP_OBJECT_ID, mount_graph, mount_graph_existing_sp,
    mount_graph_failure, mount_keyvault_secret,
};
use rigg_client::arm::ArmClient;
use rigg_client::graph::GraphClient;
use rigg_client::keyvault::KeyVaultClient;
use rigg_core::registry::Provider;
use serde_json::Value;
use wiremock::MockServer;

const SUB: &str = "sub-a";
const RG: &str = "rg";

fn arm(server: &MockServer) -> ArmClient {
    ArmClient::with_token_and_base("t".into(), server.uri())
}

/// The bodies of every request the fake received for `method` at a path
/// containing `needle`.
async fn bodies(server: &MockServer, method: &str, needle: &str) -> Vec<Value> {
    server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.method.as_str().eq_ignore_ascii_case(method) && r.url.path().contains(needle))
        .map(|r| serde_json::from_slice(&r.body).unwrap_or(Value::Null))
        .collect()
}

// ---------------------------------------------------------------- search ---

#[tokio::test]
async fn get_search_service_reads_sku_identity_and_auth_options() {
    let server = MockServer::start().await;
    mount_search_service(
        &server,
        SUB,
        RG,
        "srch",
        "standard",
        "SystemAssigned",
        "principal-sys",
        true,
        "Enabled",
    )
    .await;
    let info = arm(&server)
        .get_search_service(&search_service_id(SUB, RG, "srch"))
        .await
        .unwrap();
    assert_eq!(info.name, "srch");
    assert_eq!(info.sku, "standard");
    assert_eq!(info.identity.kind, "SystemAssigned");
    assert_eq!(info.identity.principal_id.as_deref(), Some("principal-sys"));
    assert!(info.rbac_enabled);
    assert!(!info.disable_local_auth);
    assert_eq!(info.public_network_access, "Enabled");
}

#[tokio::test]
async fn get_search_service_reports_api_key_only_as_rbac_disabled() {
    let server = MockServer::start().await;
    mount_search_service(
        &server, SUB, RG, "srch", "free", "None", "", false, "Disabled",
    )
    .await;
    let info = arm(&server)
        .get_search_service(&search_service_id(SUB, RG, "srch"))
        .await
        .unwrap();
    assert!(!info.rbac_enabled);
    assert_eq!(info.sku, "free");
    assert_eq!(info.identity.kind, "None");
    assert!(info.identity.principal_id.is_none());
    assert_eq!(info.public_network_access, "Disabled");
}

#[tokio::test]
async fn set_search_auth_options_patches_aad_or_api_key() {
    let server = MockServer::start().await;
    let id = search_service_id(SUB, RG, "srch");
    mount_search_service(
        &server, SUB, RG, "srch", "standard", "None", "", false, "Enabled",
    )
    .await;
    arm(&server).set_search_auth_options(&id).await.unwrap();
    let body = bodies(&server, "PATCH", "/searchServices/srch")
        .await
        .pop()
        .expect("a PATCH was sent");
    assert_eq!(
        body.pointer("/properties/authOptions/aadOrApiKey/aadAuthFailureMode")
            .and_then(Value::as_str),
        Some("http401WithBearerChallenge")
    );
}

#[tokio::test]
async fn attach_user_assigned_identity_patches_dual_identity_map() {
    let server = MockServer::start().await;
    let id = search_service_id(SUB, RG, "srch");
    mount_search_service(
        &server,
        SUB,
        RG,
        "srch",
        "standard",
        "SystemAssigned",
        "p",
        true,
        "Enabled",
    )
    .await;
    let uami = format!(
        "/subscriptions/{SUB}/resourceGroups/{RG}/providers/Microsoft.ManagedIdentity/userAssignedIdentities/uami"
    );
    arm(&server)
        .attach_user_assigned_identity(&id, Provider::SearchArm, &uami)
        .await
        .unwrap();
    let body = bodies(&server, "PATCH", "/searchServices/srch")
        .await
        .pop()
        .expect("a PATCH was sent");
    assert_eq!(
        body.pointer("/identity/type").and_then(Value::as_str),
        Some("SystemAssigned, UserAssigned")
    );
    assert!(
        body.pointer("/identity/userAssignedIdentities")
            .and_then(Value::as_object)
            .is_some_and(|m| m.contains_key(&uami))
    );
}

#[tokio::test]
async fn list_shared_private_links_returns_the_resources() {
    let server = MockServer::start().await;
    mount_search_service(
        &server, SUB, RG, "srch", "standard", "None", "", true, "Disabled",
    )
    .await;
    let links = arm(&server)
        .list_shared_private_links(&search_service_id(SUB, RG, "srch"))
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0]["name"], "spl-blob");
}

// --------------------------------------------------------------- storage ---

fn storage_id(name: &str) -> String {
    format!(
        "/subscriptions/{SUB}/resourceGroups/{RG}/providers/Microsoft.Storage/storageAccounts/{name}"
    )
}

#[tokio::test]
async fn get_storage_account_reads_network_and_key_settings() {
    let server = MockServer::start().await;
    let id = storage_id("acct");
    mount_storage_account(
        &server,
        &id,
        "Deny",
        "Logging, Metrics",
        "Enabled",
        false,
        true,
        Some(7),
        false,
    )
    .await;
    let info = arm(&server).get_storage_account(&id).await.unwrap();
    assert_eq!(info.name, "acct");
    assert_eq!(info.default_action, "Deny");
    assert!(!info.bypasses_azure_services());
    assert_eq!(info.public_network_access, "Enabled");
    assert_eq!(info.allow_shared_key_access, Some(false));
    assert!(info.is_hns_enabled);
    assert!(info.resource_access_rules.is_empty());
}

#[tokio::test]
async fn get_blob_service_properties_reads_soft_delete_and_versioning() {
    let server = MockServer::start().await;
    let id = storage_id("acct");
    mount_storage_account(
        &server,
        &id,
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        Some(7),
        true,
    )
    .await;
    let blob = arm(&server).get_blob_service_properties(&id).await.unwrap();
    assert!(blob.soft_delete_enabled);
    assert_eq!(blob.soft_delete_days, Some(7));
    assert!(blob.versioning_enabled);
}

#[tokio::test]
async fn set_blob_soft_delete_puts_the_retention_policy() {
    let server = MockServer::start().await;
    let id = storage_id("acct");
    mount_storage_account(
        &server,
        &id,
        "Allow",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    arm(&server).set_blob_soft_delete(&id, 7).await.unwrap();
    let body = bodies(&server, "PUT", "/blobServices/default")
        .await
        .pop()
        .expect("a PUT was sent");
    assert_eq!(
        body.pointer("/properties/deleteRetentionPolicy/enabled"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        body.pointer("/properties/deleteRetentionPolicy/days")
            .and_then(Value::as_u64),
        Some(7)
    );
}

#[tokio::test]
async fn add_storage_bypass_azure_services_preserves_the_rest_of_network_acls() {
    let server = MockServer::start().await;
    let id = storage_id("acct");
    mount_storage_account(
        &server,
        &id,
        "Deny",
        "Logging, Metrics",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    arm(&server)
        .add_storage_bypass_azure_services(&id)
        .await
        .unwrap();
    let body = bodies(&server, "PATCH", "/storageAccounts/acct")
        .await
        .pop()
        .expect("a PATCH was sent");
    let acls = body.pointer("/properties/networkAcls").unwrap();
    let bypass = acls["bypass"].as_str().unwrap();
    assert!(bypass.contains("AzureServices"), "{bypass}");
    assert!(bypass.contains("Logging"), "{bypass}");
    // defaultAction must survive: PATCHing networkAcls replaces the object.
    assert_eq!(acls["defaultAction"], "Deny");
}

#[tokio::test]
async fn add_storage_bypass_is_idempotent_when_already_present() {
    let server = MockServer::start().await;
    let id = storage_id("acct");
    mount_storage_account(
        &server,
        &id,
        "Deny",
        "AzureServices",
        "Enabled",
        true,
        false,
        None,
        false,
    )
    .await;
    arm(&server)
        .add_storage_bypass_azure_services(&id)
        .await
        .unwrap();
    assert!(
        bodies(&server, "PATCH", "/storageAccounts/acct")
            .await
            .is_empty(),
        "nothing to change → no PATCH"
    );
}

#[tokio::test]
async fn add_storage_resource_instance_rule_appends_the_rule() {
    let server = MockServer::start().await;
    let id = storage_id("acct");
    mount_storage_account(
        &server, &id, "Deny", "Logging", "Enabled", true, false, None, false,
    )
    .await;
    let search = search_service_id(SUB, RG, "srch");
    arm(&server)
        .add_storage_resource_instance_rule(&id, "tenant-1", &search)
        .await
        .unwrap();
    let body = bodies(&server, "PATCH", "/storageAccounts/acct")
        .await
        .pop()
        .expect("a PATCH was sent");
    let rules = body
        .pointer("/properties/networkAcls/resourceAccessRules")
        .and_then(Value::as_array)
        .unwrap();
    assert_eq!(rules.len(), 1);
    assert_eq!(rules[0]["tenantId"], "tenant-1");
    assert_eq!(rules[0]["resourceId"], search.as_str());
}

// ----------------------------------------------------- cognitive services ---

#[tokio::test]
async fn get_cognitive_account_by_id_reads_kind_and_endpoint() {
    let server = MockServer::start().await;
    let id = format!(
        "/subscriptions/{SUB}/resourceGroups/{RG}/providers/Microsoft.CognitiveServices/accounts/ai"
    );
    mount_cognitive_account(&server, &id, "AIServices", "swedencentral").await;
    let acct = arm(&server).get_cognitive_account_by_id(&id).await.unwrap();
    assert_eq!(acct.kind, "AIServices");
    assert_eq!(acct.location, "swedencentral");
    assert_eq!(acct.agents_endpoint(), "https://ai.services.ai.azure.com");
}

// ------------------------------------------------------------------ RBAC ---

#[tokio::test]
async fn can_write_role_assignments_true_for_owner_shaped_permissions() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}");
    mount_permissions(&server, &scope, true).await;
    assert!(
        arm(&server)
            .can_write_role_assignments(&scope)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn can_write_role_assignments_false_when_not_actions_exclude_it() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}");
    mount_permissions(&server, &scope, false).await;
    assert!(
        !arm(&server)
            .can_write_role_assignments(&scope)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn role_assignments_for_filters_at_scope_and_parses_properties() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}/resourceGroups/{RG}");
    mount_role_assignments(&server, &scope, "principal-1", &["role-a", "role-b"]).await;
    let assignments = arm(&server)
        .role_assignments_for(&scope, "principal-1")
        .await
        .unwrap();
    assert_eq!(assignments.len(), 2);
    assert!(assignments[0].role_definition_id.ends_with("role-a"));
    assert_eq!(assignments[0].description, "rigg: env dev edge 0");
    assert!(assignments[0].id.contains("/roleAssignments/ra-0"));

    let query = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path().ends_with("/roleAssignments"))
        .and_then(|r| r.url.query().map(str::to_string))
        .unwrap();
    assert!(query.contains("atScope()"), "{query}");
    assert!(query.contains("assignedTo("), "{query}");
    assert!(query.contains("principal-1"), "{query}");
}

#[tokio::test]
async fn create_role_assignment_described_sends_type_and_description() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}/resourceGroups/{RG}");
    mount_role_assignments(&server, &scope, "p", &[]).await;
    arm(&server)
        .create_role_assignment_described(&scope, "principal-1", "role-guid", "User", "rigg: dev")
        .await
        .unwrap();
    let body = bodies(&server, "PUT", "/roleAssignments/")
        .await
        .pop()
        .expect("a PUT was sent");
    assert_eq!(body["properties"]["principalId"], "principal-1");
    assert_eq!(body["properties"]["principalType"], "User");
    assert_eq!(body["properties"]["description"], "rigg: dev");
    assert!(
        body["properties"]["roleDefinitionId"]
            .as_str()
            .unwrap()
            .ends_with("role-guid")
    );
}

#[tokio::test]
async fn create_role_assignment_keeps_service_principal_default() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}/resourceGroups/{RG}");
    mount_role_assignments(&server, &scope, "p", &[]).await;
    arm(&server)
        .create_role_assignment(&scope, "principal-1", "role-guid")
        .await
        .unwrap();
    let body = bodies(&server, "PUT", "/roleAssignments/")
        .await
        .pop()
        .expect("a PUT was sent");
    assert_eq!(body["properties"]["principalType"], "ServicePrincipal");
}

#[tokio::test]
async fn list_rigg_role_assignments_filters_by_description_prefix() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}/resourceGroups/{RG}");
    mount_role_assignments(&server, &scope, "p", &["role-a", "role-b"]).await;
    let ours = arm(&server)
        .list_rigg_role_assignments(&scope, "rigg:")
        .await
        .unwrap();
    assert_eq!(ours.len(), 2);
    let none = arm(&server)
        .list_rigg_role_assignments(&scope, "someone-else:")
        .await
        .unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn delete_role_assignment_issues_a_delete_on_the_assignment_id() {
    let server = MockServer::start().await;
    let scope = format!("/subscriptions/{SUB}/resourceGroups/{RG}");
    mount_role_assignments(&server, &scope, "p", &["role-a"]).await;
    let id = format!("{scope}/providers/Microsoft.Authorization/roleAssignments/ra-0");
    arm(&server).delete_role_assignment(&id).await.unwrap();
    let deletes: Vec<_> = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .collect();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].url.path(), id);
}

// -------------------------------------------------------------- identity ---

#[tokio::test]
async fn create_user_assigned_identity_puts_the_location_and_returns_principal() {
    let server = MockServer::start().await;
    mount_create_uami(&server, "principal-new").await;
    let created = arm(&server)
        .create_user_assigned_identity(SUB, RG, "rigg-uami", "swedencentral")
        .await
        .unwrap();
    assert_eq!(created.name, "rigg-uami");
    assert_eq!(created.principal_id.as_deref(), Some("principal-new"));
    assert_eq!(created.client_id.as_deref(), Some("cid-new"));
    let body = bodies(&server, "PUT", "/userAssignedIdentities/rigg-uami")
        .await
        .pop()
        .expect("a PUT was sent");
    assert_eq!(body["location"], "swedencentral");
}

/// base64url (no padding) — building a token payload for the decoder test.
fn b64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        for (i, ix) in idx.iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[*ix as usize] as char);
            }
        }
    }
    out
}

fn jwt_with(payload: serde_json::Value) -> String {
    format!(
        "{}.{}.{}",
        b64url(br#"{"alg":"none"}"#),
        b64url(payload.to_string().as_bytes()),
        "sig"
    )
}

#[tokio::test]
async fn caller_object_id_decodes_a_user_token() {
    let server = MockServer::start().await;
    let token = jwt_with(serde_json::json!({
        "oid": "user-oid",
        "upn": "kristofer@example.com",
        "appid": "cli-app"
    }));
    let arm = ArmClient::with_token_and_base(token, server.uri());
    let caller = arm.caller_object_id().await.unwrap();
    assert_eq!(caller.object_id, "user-oid");
    assert_eq!(caller.principal_type, "User");
    assert_eq!(caller.display, "kristofer@example.com");
}

#[tokio::test]
async fn caller_object_id_decodes_a_service_principal_token() {
    let server = MockServer::start().await;
    let token = jwt_with(serde_json::json!({"oid": "sp-oid", "appid": "app-1"}));
    let arm = ArmClient::with_token_and_base(token, server.uri());
    let caller = arm.caller_object_id().await.unwrap();
    assert_eq!(caller.object_id, "sp-oid");
    assert_eq!(caller.principal_type, "ServicePrincipal");
    assert_eq!(caller.display, "app-1");
}

#[tokio::test]
async fn caller_object_id_errors_on_a_non_jwt_token() {
    let server = MockServer::start().await;
    let arm = ArmClient::with_token_and_base("test-token".into(), server.uri());
    let err = arm.caller_object_id().await.unwrap_err();
    assert!(err.to_string().contains("not a JWT"), "{err}");
}

// ----------------------------------------------------------------- Graph ---

#[tokio::test]
async fn graph_easy_auth_flow_creates_app_role_and_service_principal() {
    let server = MockServer::start().await;
    mount_graph(&server).await;
    let graph = GraphClient::with_token_and_base("t".into(), server.uri());

    let app = graph.create_application("rigg-site").await.unwrap();
    assert_eq!(app.id, FAKE_APP_OBJECT_ID);
    assert_eq!(app.app_id, FAKE_APP_ID);

    let uri = format!("api://{}", app.app_id);
    let role_id = graph
        .set_identifier_uri_and_role(&app.id, &uri)
        .await
        .unwrap();
    let patch = bodies(&server, "PATCH", "/applications/")
        .await
        .pop()
        .expect("a PATCH was sent");
    assert_eq!(patch["identifierUris"][0], uri.as_str());
    assert_eq!(
        patch
            .pointer("/api/requestedAccessTokenVersion")
            .and_then(Value::as_u64),
        Some(2)
    );
    let role = &patch["appRoles"][0];
    assert_eq!(role["id"], role_id.as_str());
    assert_eq!(role["allowedMemberTypes"][0], "Application");
    assert_eq!(role["value"], "Caller");
    assert_eq!(role["isEnabled"], Value::Bool(true));
    // Deterministic in the identifier URI: re-deriving gives the same id.
    assert_eq!(
        graph
            .set_identifier_uri_and_role(&app.id, &uri)
            .await
            .unwrap(),
        role_id
    );

    let sp = graph.ensure_service_principal(&app.app_id).await.unwrap();
    assert_eq!(sp.id, FAKE_SP_OBJECT_ID);
    assert!(!sp.app_role_assignment_required);
    let created = bodies(&server, "POST", "/servicePrincipals").await;
    assert_eq!(created.len(), 1, "the SP was created, not reused");
    assert_eq!(created[0]["appId"], app.app_id.as_str());

    graph
        .assign_app_role(&sp.id, "search-mi-object-id", &role_id)
        .await
        .unwrap();
    let assignment = bodies(&server, "POST", "/appRoleAssignedTo")
        .await
        .pop()
        .expect("an assignment was posted");
    assert_eq!(assignment["principalId"], "search-mi-object-id");
    assert_eq!(assignment["resourceId"], sp.id.as_str());
    assert_eq!(assignment["appRoleId"], role_id.as_str());
}

#[tokio::test]
async fn ensure_service_principal_reuses_an_existing_one() {
    let server = MockServer::start().await;
    mount_graph_existing_sp(&server, "existing-sp", true).await;
    let graph = GraphClient::with_token_and_base("t".into(), server.uri());
    let sp = graph.ensure_service_principal(FAKE_APP_ID).await.unwrap();
    assert_eq!(sp.id, "existing-sp");
    assert!(sp.app_role_assignment_required);
    assert!(
        bodies(&server, "POST", "/servicePrincipals")
            .await
            .is_empty(),
        "an existing SP must not be re-created"
    );
    let query = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/servicePrincipals")
        .and_then(|r| r.url.query().map(str::to_string))
        .unwrap();
    assert!(query.contains("appId"), "{query}");
    assert!(query.contains(FAKE_APP_ID), "{query}");
}

#[tokio::test]
async fn graph_errors_carry_the_graph_message() {
    let server = MockServer::start().await;
    mount_graph_failure(
        &server,
        403,
        "Insufficient privileges to complete the operation.",
    )
    .await;
    let graph = GraphClient::with_token_and_base("t".into(), server.uri());
    let err = graph.create_application("rigg-site").await.unwrap_err();
    assert!(err.to_string().contains("Insufficient privileges"), "{err}");
}

// ------------------------------------------------------------- Key Vault ---

#[tokio::test]
async fn key_vault_get_secret_returns_the_value() {
    let server = MockServer::start().await;
    mount_keyvault_secret(&server, "func-key", "s3cret").await;
    let kv = KeyVaultClient::with_token_and_base("t".into(), server.uri());
    assert_eq!(kv.get_secret("func-key").await.unwrap(), "s3cret");
    let req = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/secrets/func-key")
        .unwrap();
    assert!(
        req.url
            .query()
            .is_some_and(|q| q.starts_with("api-version=")),
        "{:?}",
        req.url.query()
    );
    assert_eq!(
        req.headers.get("authorization").unwrap().to_str().unwrap(),
        "Bearer t"
    );
}

#[tokio::test]
async fn key_vault_missing_secret_is_an_error_not_an_empty_value() {
    let server = MockServer::start().await;
    mount_keyvault_secret(&server, "other", "v").await;
    let kv = KeyVaultClient::with_token_and_base("t".into(), server.uri());
    assert!(kv.get_secret("func-key").await.is_err());
}
