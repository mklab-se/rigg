//! Wiremock fakes for Microsoft Graph and the Key Vault secrets data plane —
//! reused across test binaries via `#[path = "graph_fake.rs"] mod graph_fake;`.
//!
//! Both clients take an explicit base URL in tests
//! (`GraphClient::with_token_and_base`, `KeyVaultClient::with_token_and_base`)
//! so the `RIGG_GRAPH_ENDPOINT` / `RIGG_KEYVAULT_ENDPOINT` process env vars
//! are never touched — wiremock servers are per-test and tests in a binary
//! run in parallel.

#![allow(dead_code)] // included by several test binaries; each uses part of it

use std::sync::{Arc, Mutex};

use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

/// The object id the application fake hands back for `POST /applications`.
pub const FAKE_APP_OBJECT_ID: &str = "11111111-1111-1111-1111-111111111111";
/// The client (application) id the application fake hands back.
pub const FAKE_APP_ID: &str = "22222222-2222-2222-2222-222222222222";
/// The object id the fake hands back for a newly created service principal.
pub const FAKE_SP_OBJECT_ID: &str = "33333333-3333-3333-3333-333333333333";

/// Mount the Graph endpoints the Easy Auth wiring uses:
/// `POST /applications`, `GET /applications/{id}`, `PATCH /applications/{id}`,
/// `POST /servicePrincipals`,
/// `POST /servicePrincipals/{id}/appRoleAssignedTo`, and a
/// `GET /servicePrincipals?$filter=appId eq '…'` that reports **no** existing
/// service principal (so `ensure_service_principal` takes the create path).
///
/// Use [`mount_graph_existing_sp`] instead when the lookup should find one.
pub async fn mount_graph(server: &MockServer) {
    mount_graph_sp_lookup(server, None, false).await;
    mount_graph_writes(server).await;
}

/// Like [`mount_graph`], but the service-principal lookup finds `sp_id`
/// (with `app_role_assignment_required` as given), so
/// `ensure_service_principal` returns without creating one.
pub async fn mount_graph_existing_sp(
    server: &MockServer,
    sp_id: &str,
    app_role_assignment_required: bool,
) {
    mount_graph_sp_lookup(server, Some(sp_id), app_role_assignment_required).await;
    mount_graph_writes(server).await;
}

/// Mount `GET /applications?$filter=appId eq '…'` — the `--client-id` reuse
/// path. `found` is whether the tenant already has that application.
pub async fn mount_graph_application_lookup(server: &MockServer, found: bool) {
    let value = if found {
        json!([{"id": FAKE_APP_OBJECT_ID, "appId": FAKE_APP_ID, "displayName": "existing"}])
    } else {
        json!([])
    };
    Mock::given(method("GET"))
        .and(path("/applications"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": value})))
        .mount(server)
        .await;
}

/// Mount `GET /servicePrincipals/{objectId}` — how a managed identity's
/// principal (object) id becomes the client id Easy Auth admits.
pub async fn mount_graph_service_principal(server: &MockServer, object_id: &str, app_id: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/servicePrincipals/{object_id}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": object_id,
            "appId": app_id,
            "appRoleAssignmentRequired": false
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn mount_graph_sp_lookup(
    server: &MockServer,
    sp_id: Option<&str>,
    app_role_assignment_required: bool,
) {
    let value = match sp_id {
        Some(id) => json!([{
            "id": id,
            "appId": FAKE_APP_ID,
            "appRoleAssignmentRequired": app_role_assignment_required
        }]),
        None => json!([]),
    };
    Mock::given(method("GET"))
        .and(path("/servicePrincipals"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": value})))
        .mount(server)
        .await;
}

/// Mount `GET /applications/{id}` with the `appRoles` the application
/// already publishes. Priority 1, so it wins over [`mount_graph`]'s default
/// (an application with no roles) regardless of mount order.
pub async fn mount_graph_application_roles(server: &MockServer, app_roles: serde_json::Value) {
    mount_graph_application(server, app_roles, json!([])).await;
}

/// Like [`mount_graph_application_roles`], but the application also already
/// publishes `identifier_uris` — the audiences its existing callers ask
/// tokens for, which a PATCH must not drop.
pub async fn mount_graph_application(
    server: &MockServer,
    app_roles: serde_json::Value,
    identifier_uris: serde_json::Value,
) {
    Mock::given(method("GET"))
        .and(path_regex(r"^/applications/[^/]+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": FAKE_APP_OBJECT_ID,
            "appId": FAKE_APP_ID,
            "displayName": "rigg-app",
            "identifierUris": identifier_uris,
            "appRoles": app_roles
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

async fn mount_graph_writes(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path_regex(r"^/applications/[^/]+$"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": FAKE_APP_OBJECT_ID,
            "appId": FAKE_APP_ID,
            "displayName": "rigg-app",
            "appRoles": []
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/applications"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": FAKE_APP_OBJECT_ID,
            "appId": FAKE_APP_ID,
            "displayName": "rigg-app"
        })))
        .mount(server)
        .await;
    Mock::given(method("PATCH"))
        .and(path_regex(r"^/applications/[^/]+$"))
        .respond_with(ResponseTemplate::new(204))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/servicePrincipals"))
        .respond_with(|req: &Request| {
            let body: serde_json::Value = req.body_json().unwrap_or(json!({}));
            ResponseTemplate::new(201).set_body_json(json!({
                "id": FAKE_SP_OBJECT_ID,
                "appId": body.get("appId").cloned().unwrap_or(json!(FAKE_APP_ID)),
                "appRoleAssignmentRequired": false
            }))
        })
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/servicePrincipals/[^/]+/appRoleAssignedTo$"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "assignment-1"})))
        .mount(server)
        .await;
}

/// Mount a STATEFUL `appRoleAssignedTo` pair (priority 1, so it wins over
/// [`mount_graph`]'s fire-and-forget POST): the POST records the assignment,
/// the GET lists what has been recorded. That is how Graph behaves, and it
/// is what lets a test prove the wiring is idempotent — a second run sees
/// its own first grant instead of a fresh, empty directory.
pub async fn mount_graph_app_role_assignments(server: &MockServer) {
    let assignments: Arc<Mutex<Vec<serde_json::Value>>> = Arc::new(Mutex::new(Vec::new()));
    let listing = Arc::clone(&assignments);
    Mock::given(method("GET"))
        .and(path_regex(r"^/servicePrincipals/[^/]+/appRoleAssignedTo$"))
        .respond_with(move |_: &Request| {
            let value = listing.lock().expect("assignments lock").clone();
            ResponseTemplate::new(200).set_body_json(json!({"value": value}))
        })
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/servicePrincipals/[^/]+/appRoleAssignedTo$"))
        .respond_with(move |req: &Request| {
            let mut body: serde_json::Value = req.body_json().unwrap_or(json!({}));
            body["id"] = json!("assignment-1");
            assignments
                .lock()
                .expect("assignments lock")
                .push(body.clone());
            ResponseTemplate::new(201).set_body_json(body)
        })
        .with_priority(1)
        .mount(server)
        .await;
}

/// Mount a Graph endpoint that fails with Graph's error envelope, so the
/// client's message extraction (`error.message`) can be asserted.
pub async fn mount_graph_failure(server: &MockServer, status: u16, message: &str) {
    Mock::given(method("POST"))
        .and(path("/applications"))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({
            "error": {"code": "Authorization_RequestDenied", "message": message}
        })))
        .mount(server)
        .await;
}

/// Mount one Key Vault secret: `GET /secrets/{name}` → `{ "value": … }`.
pub async fn mount_keyvault_secret(server: &MockServer, name: &str, value: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/secrets/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": format!("https://vault.vault.azure.net/secrets/{name}/version"),
            "value": value,
            "attributes": {"enabled": true}
        })))
        .mount(server)
        .await;
}
