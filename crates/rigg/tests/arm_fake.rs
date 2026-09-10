//! Wiremock ARM fake for `ArmClient` — reused by later workstreams via
//! `#[path = "arm_fake.rs"] mod arm_fake;`.
//!
//! Uses `ArmClient::with_token_and_base(token, server.uri())` exclusively —
//! never the `RIGG_ARM_ENDPOINT` process env var, since wiremock servers are
//! per-test and tests in this binary run in parallel.

use rigg_client::arm::ArmClient;
use rigg_core::binding::BindingType;
use serde_json::json;
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Mount an ARM fake on `server`: `/subscriptions`, and per-subscription
/// provider listings for storage accounts, cognitive services accounts,
/// web sites, managed identities, key vaults, and search services — plus a
/// catch-all `GET {resource id}` responder.
///
/// `resources` is `(type, name, resource_group, location)`, where `type` is
/// one of `storageAccounts`, `accounts`, `sites`, `userAssignedIdentities`,
/// `vaults`, `searchServices`. Every resource is served under every
/// subscription in `subs` (with an id scoped to that subscription) — tests
/// that care about a specific subscription should pass `subscription:
/// Some(...)` to `resolve_binding` rather than relying on placement here.
pub async fn mount_arm_fake(
    server: &MockServer,
    subs: &[&str],
    resources: &[(&str, &str, &str, &str)],
) {
    Mock::given(method("GET"))
        .and(path("/subscriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": subs.iter().map(|s| json!({
                "subscriptionId": s,
                "displayName": s,
                "state": "Enabled"
            })).collect::<Vec<_>>()
        })))
        .mount(server)
        .await;

    for (rt, ns) in [
        ("storageAccounts", "Microsoft.Storage"),
        ("accounts", "Microsoft.CognitiveServices"),
        ("sites", "Microsoft.Web"),
        ("userAssignedIdentities", "Microsoft.ManagedIdentity"),
        ("vaults", "Microsoft.KeyVault"),
        ("searchServices", "Microsoft.Search"),
    ] {
        for sub in subs {
            let items: Vec<_> = resources
                .iter()
                .filter(|(t, ..)| *t == rt)
                .map(|(t, name, rg, loc)| {
                    json!({
                        "name": name,
                        "location": loc,
                        "kind": if *t == "accounts" { "AIServices" } else { "" },
                        "id": format!("/subscriptions/{sub}/resourceGroups/{rg}/providers/{ns}/{t}/{name}"),
                        "properties": {
                            "endpoint": format!("https://{name}.cognitiveservices.azure.com/"),
                            "vaultUri": format!("https://{name}.vault.azure.net/"),
                            "principalId": "00000000-0000-0000-0000-00000000aaaa",
                            "primaryEndpoints": {
                                "blob": format!("https://{name}.blob.core.windows.net/")
                            }
                        }
                    })
                })
                .collect();
            Mock::given(method("GET"))
                .and(path(format!("/subscriptions/{sub}/providers/{ns}/{rt}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": items})))
                .mount(server)
                .await;
        }
    }

    Mock::given(method("GET"))
        .and(path_regex(
            r"^/subscriptions/[^/]+/resourceGroups/[^/]+/providers/.+$",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "by-id",
            "location": "swedencentral",
            "properties": {}
        })))
        .mount(server)
        .await;
}

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
