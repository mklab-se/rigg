//! Tests for the shared ARM fake / `ArmClient::resolve_binding`.
//!
//! The fake itself lives in `arm_fake.rs` so other test binaries can
//! include it without re-running these tests.

#[path = "arm_fake.rs"]
mod arm_fake;

use arm_fake::mount_arm_fake;
use rigg_client::arm::ArmClient;
use rigg_core::binding::BindingType;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn resolve_binding_by_name_and_by_id() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &["sub-a", "sub-b"],
        &[
            ("storageAccounts", "acct", "rg", "swedencentral"),
            ("storageAccounts", "dup", "rg1", "x"),
            ("storageAccounts", "dup", "rg2", "x"),
        ],
    )
    .await;
    let arm = ArmClient::with_token_and_base("t".into(), server.uri());

    let r = arm
        .resolve_binding(BindingType::Storage, "ACCT", Some("sub-a"))
        .await
        .unwrap();
    assert_eq!(r.resource_group.as_deref(), Some("rg"));
    assert!(
        r.arm_id
            .as_deref()
            .unwrap()
            .starts_with("/subscriptions/sub-a/")
    );

    let err = arm
        .resolve_binding(BindingType::Storage, "dup", Some("sub-a"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ambiguous"));

    assert!(
        arm.resolve_binding(BindingType::Storage, "nope", None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn resolve_binding_by_arm_id_fills_location_and_subscription() {
    let server = MockServer::start().await;
    mount_arm_fake(&server, &["sub-a"], &[]).await;
    let arm = ArmClient::with_token_and_base("t".into(), server.uri());

    let id =
        "/subscriptions/sub-a/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct";
    let r = arm
        .resolve_binding(BindingType::Storage, id, None)
        .await
        .unwrap();
    assert_eq!(r.subscription.as_deref(), Some("sub-a"));
    assert_eq!(r.resource_group.as_deref(), Some("rg"));
    assert_eq!(r.location.as_deref(), Some("swedencentral"));
    assert_eq!(r.arm_id.as_deref(), Some(id));
}

#[tokio::test]
async fn resolve_binding_across_all_subscriptions_when_none_given() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &["sub-a", "sub-b"],
        &[("accounts", "svc", "rg", "swedencentral")],
    )
    .await;
    let arm = ArmClient::with_token_and_base("t".into(), server.uri());

    // "svc" is served under both subscriptions by the fake, so listing every
    // enabled subscription without narrowing to one is itself ambiguous.
    let err = arm
        .resolve_binding(BindingType::AiServices, "svc", None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ambiguous"));

    let r = arm
        .resolve_binding(BindingType::AiServices, "svc", Some("sub-b"))
        .await
        .unwrap();
    assert_eq!(r.subscription.as_deref(), Some("sub-b"));
}

#[tokio::test]
async fn resolve_binding_url_needs_no_arm_call() {
    let server = MockServer::start().await;
    // No mocks mounted at all — resolving a URL binding must not hit the network.
    let arm = ArmClient::with_token_and_base("t".into(), server.uri());
    let r = arm
        .resolve_binding(BindingType::Api, "https://api.example.com/v1", None)
        .await
        .unwrap();
    assert_eq!(r.arm_id, None);
    assert_eq!(r.endpoint.as_deref(), Some("https://api.example.com/v1"));
}

/// A subscription the caller cannot list must not abort the by-name
/// fan-out: the match in the readable subscription still resolves.
#[tokio::test]
async fn resolve_binding_skips_subscriptions_that_cannot_be_listed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/subscriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"subscriptionId": "sub-denied", "displayName": "denied", "state": "Enabled"},
                {"subscriptionId": "sub-ok", "displayName": "ok", "state": "Enabled"},
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/subscriptions/sub-denied/providers/Microsoft.Storage/storageAccounts",
        ))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_string("{\"error\":{\"code\":\"AuthorizationFailed\"}}"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/subscriptions/sub-ok/providers/Microsoft.Storage/storageAccounts",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": [{
            "name": "acct",
            "location": "swedencentral",
            "id": "/subscriptions/sub-ok/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct",
            "properties": {}
        }]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/subscriptions/sub-ok/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "acct", "location": "swedencentral", "properties": {}
        })))
        .mount(&server)
        .await;

    let arm = ArmClient::with_token_and_base("t".into(), server.uri());
    let r = arm
        .resolve_binding(BindingType::Storage, "acct", None)
        .await
        .unwrap();
    assert_eq!(r.subscription.as_deref(), Some("sub-ok"));
}

/// …but when every subscription fails, the error surfaces instead of a
/// misleading "not found".
#[tokio::test]
async fn resolve_binding_reports_the_listing_error_when_every_subscription_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/subscriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"subscriptionId": "sub-denied", "displayName": "denied", "state": "Enabled"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/subscriptions/sub-denied/providers/Microsoft.Storage/storageAccounts",
        ))
        .respond_with(ResponseTemplate::new(403).set_body_string("nope"))
        .mount(&server)
        .await;
    let arm = ArmClient::with_token_and_base("t".into(), server.uri());
    let err = arm
        .resolve_binding(BindingType::Storage, "acct", None)
        .await
        .unwrap_err();
    assert!(
        !err.to_string().contains("not found"),
        "the 403 must not be reported as not-found: {err}"
    );
}
