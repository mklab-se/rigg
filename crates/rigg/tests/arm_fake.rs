//! Wiremock ARM fake for `ArmClient` — reused by later workstreams via
//! `#[path = "arm_fake.rs"] mod arm_fake;`.
//!
//! Uses `ArmClient::with_token_and_base(token, server.uri())` exclusively —
//! never the `RIGG_ARM_ENDPOINT` process env var, since wiremock servers are
//! per-test and tests in this binary run in parallel.

#![allow(dead_code)] // included by several test binaries; each uses part of it

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
