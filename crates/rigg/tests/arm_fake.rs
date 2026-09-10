//! Wiremock ARM fake for `ArmClient` — reused by later workstreams via
//! `#[path = "arm_fake.rs"] mod arm_fake;`.
//!
//! Uses `ArmClient::with_token_and_base(token, server.uri())` exclusively —
//! never the `RIGG_ARM_ENDPOINT` process env var, since wiremock servers are
//! per-test and tests in this binary run in parallel.

#![allow(dead_code)] // included by several test binaries; each uses part of it

use serde_json::{Value, json};
use wiremock::matchers::{method, path, path_regex};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

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

    // Catch-all `GET {resource id}`: answer with the resource actually asked
    // for (its name is the id's last segment), so a resolution's name and
    // physical name match what the caller looked up.
    Mock::given(method("GET"))
        .and(path_regex(
            r"^/subscriptions/[^/]+/resourceGroups/[^/]+/providers/.+$",
        ))
        .respond_with(|req: &Request| {
            let name = req
                .url
                .path()
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or("by-id")
                .to_string();
            ResponseTemplate::new(200).set_body_json(json!({
                "name": name,
                "location": "swedencentral",
                "properties": {
                    "endpoint": format!("https://{name}.cognitiveservices.azure.com/"),
                    "vaultUri": format!("https://{name}.vault.azure.net/"),
                    "principalId": "00000000-0000-0000-0000-00000000aaaa",
                    "primaryEndpoints": {
                        "blob": format!("https://{name}.blob.core.windows.net/")
                    }
                }
            }))
        })
        .mount(server)
        .await;
}

/// Mount one site's Easy Auth (`authsettingsV2`) document. ARM serves it as
/// a POST action on the site's resource id, so this never collides with the
/// catch-all `GET {resource id}` responder above.
///
/// `enabled` drives both the platform switch and the Microsoft identity
/// provider; `client_id` becomes the app registration's client id and the
/// single allowed audience (`api://{client_id}`).
pub async fn mount_easy_auth(server: &MockServer, site: &str, enabled: bool, client_id: &str) {
    let audiences: Vec<String> = if client_id.is_empty() {
        Vec::new()
    } else {
        vec![format!("api://{client_id}")]
    };
    let body = json!({
        "properties": {
            "platform": {"enabled": enabled},
            "identityProviders": {
                "azureActiveDirectory": {
                    "enabled": enabled,
                    "registration": {"clientId": client_id},
                    "validation": {"allowedAudiences": audiences}
                }
            }
        }
    });
    Mock::given(method("POST"))
        .and(path_regex(format!(
            r"^.*/sites/{site}/config/authsettingsV2/list$"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Mount one site's `authsettingsV2` action as a failure (403, 500, …) —
/// what an unauthorized caller gets. Promote must report this as a skipped
/// decision, never as a verdict.
pub async fn mount_easy_auth_failure(server: &MockServer, site: &str, status: u16) {
    Mock::given(method("POST"))
        .and(path_regex(format!(
            r"^.*/sites/{site}/config/authsettingsV2/list$"
        )))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({
            "error": {"code": "AuthorizationFailed", "message": "no permission to read auth settings"}
        })))
        .mount(server)
        .await;
}

/// Mount `Microsoft.CognitiveServices/locations/{location}/models` and
/// `.../usages` for one subscription — what the deployment availability and
/// quota checks read.
pub async fn mount_models(
    server: &MockServer,
    sub: &str,
    location: &str,
    models: Vec<Value>,
    usages: Vec<Value>,
) {
    mount_models_paged(server, sub, location, vec![models], usages).await;
}

/// Like [`mount_models`], but serves the models listing across several pages
/// linked by `nextLink` — page `i > 0` lives at `models-page{i}`, and the
/// last page has no link.
pub async fn mount_models_paged(
    server: &MockServer,
    sub: &str,
    location: &str,
    pages: Vec<Vec<Value>>,
    usages: Vec<Value>,
) {
    let base =
        format!("/subscriptions/{sub}/providers/Microsoft.CognitiveServices/locations/{location}");
    let last = pages.len().saturating_sub(1);
    for (i, page) in pages.into_iter().enumerate() {
        let mut body = json!({"value": page});
        if i < last {
            // ARM's own next link is absolute and carries its own query; the
            // client must follow it verbatim rather than rebuild it. The
            // marker below stands in for the real api-version, which would
            // trip the registry's no-version-literals guard.
            body["nextLink"] = json!(format!(
                "{}{base}/models-page{}?api-version=from-the-next-link",
                server.uri(),
                i + 1
            ));
        }
        let at = if i == 0 {
            format!("{base}/models")
        } else {
            format!("{base}/models-page{i}")
        };
        Mock::given(method("GET"))
            .and(path(at))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path(format!("{base}/usages")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": usages})))
        .mount(server)
        .await;
}
