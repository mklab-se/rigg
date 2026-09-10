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

/// Mount one search service document at its ARM resource id.
///
/// Mounted at wiremock priority 1 so it wins over [`mount_arm_fake`]'s
/// catch-all `GET {resource id}` responder regardless of mount order.
///
/// `identity_type` is the ARM `identity.type` string (`None`,
/// `SystemAssigned`, `SystemAssigned, UserAssigned`, …); `principal_id` is
/// the system-assigned principal (empty for none). `rbac_enabled` drives
/// `properties.authOptions` (`aadOrApiKey` vs `apiKeyOnly`).
#[allow(clippy::too_many_arguments)]
pub async fn mount_search_service(
    server: &MockServer,
    sub: &str,
    rg: &str,
    name: &str,
    sku: &str,
    identity_type: &str,
    principal_id: &str,
    rbac_enabled: bool,
    public_network: &str,
) {
    let id = search_service_id(sub, rg, name);
    let auth_options = if rbac_enabled {
        json!({"aadOrApiKey": {"aadAuthFailureMode": "http401WithBearerChallenge"}})
    } else {
        json!({"apiKeyOnly": {}})
    };
    let mut identity = json!({"type": identity_type});
    if !principal_id.is_empty() {
        identity["principalId"] = json!(principal_id);
    }
    if identity_type.contains("UserAssigned") {
        identity["userAssignedIdentities"] = json!({
            format!("/subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.ManagedIdentity/userAssignedIdentities/uami"):
                {"principalId": "00000000-0000-0000-0000-0000000000ua", "clientId": "cid-ua"}
        });
    }
    let body = json!({
        "name": name,
        "id": id,
        "location": "swedencentral",
        "sku": {"name": sku},
        "identity": identity,
        "properties": {
            "authOptions": auth_options,
            "disableLocalAuth": false,
            "publicNetworkAccess": public_network,
            "networkRuleSet": {"ipRules": []}
        }
    });
    Mock::given(method("GET"))
        .and(path(id.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(id.clone()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": name})))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("{id}/sharedPrivateLinkResources")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "spl-blob", "properties": {"status": "Approved"}}]
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

/// The ARM resource id of a search service, as the fake serves it.
pub fn search_service_id(sub: &str, rg: &str, name: &str) -> String {
    format!(
        "/subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.Search/searchServices/{name}"
    )
}

/// Mount one storage account at `id`, plus its `blobServices/default`
/// document (GET and PUT). Priority 1, same reason as
/// [`mount_search_service`].
#[allow(clippy::too_many_arguments)]
pub async fn mount_storage_account(
    server: &MockServer,
    id: &str,
    network_default_action: &str,
    bypass: &str,
    public_network: &str,
    shared_key: bool,
    hns: bool,
    soft_delete: Option<u32>,
    versioning: bool,
) {
    let name = id.rsplit('/').next().unwrap_or("acct").to_string();
    let body = json!({
        "name": name,
        "id": id,
        "location": "swedencentral",
        "properties": {
            "networkAcls": {
                "defaultAction": network_default_action,
                "bypass": bypass,
                "ipRules": [],
                "virtualNetworkRules": [],
                "resourceAccessRules": []
            },
            "publicNetworkAccess": public_network,
            "allowSharedKeyAccess": shared_key,
            "isHnsEnabled": hns
        }
    });
    Mock::given(method("GET"))
        .and(path(id.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("PATCH"))
        .and(path(id.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": name})))
        .with_priority(1)
        .mount(server)
        .await;

    let blob = json!({
        "properties": {
            "deleteRetentionPolicy": {
                "enabled": soft_delete.is_some(),
                "days": soft_delete.unwrap_or(0)
            },
            "isVersioningEnabled": versioning
        }
    });
    Mock::given(method("GET"))
        .and(path(format!("{id}/blobServices/default")))
        .respond_with(ResponseTemplate::new(200).set_body_json(blob))
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("PUT"))
        .and(path(format!("{id}/blobServices/default")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "default"})))
        .with_priority(1)
        .mount(server)
        .await;
}

/// Mount `{scope}/providers/Microsoft.Authorization/permissions`.
///
/// When `can_write_role_assignments` the caller gets an Owner-shaped entry
/// (`actions: ["*"]`); otherwise a Reader-shaped one plus an entry that
/// grants the write via a wildcard but takes it back in `notActions` — so
/// the exclusion path is exercised by the negative case too.
pub async fn mount_permissions(server: &MockServer, scope: &str, can_write_role_assignments: bool) {
    let value = if can_write_role_assignments {
        json!([{"actions": ["*"], "notActions": [], "dataActions": [], "notDataActions": []}])
    } else {
        json!([
            {"actions": ["*/read"], "notActions": [], "dataActions": [], "notDataActions": []},
            {
                "actions": ["Microsoft.Authorization/*"],
                "notActions": ["Microsoft.Authorization/roleAssignments/write"],
                "dataActions": [],
                "notDataActions": []
            }
        ])
    };
    Mock::given(method("GET"))
        .and(path(format!(
            "{scope}/providers/Microsoft.Authorization/permissions"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": value})))
        .with_priority(1)
        .mount(server)
        .await;
}

/// Mount `{scope}/providers/Microsoft.Authorization/roleAssignments`:
/// a GET listing (`role_ids` assigned to `principal`, each carrying a
/// `rigg:` description) and a PUT/DELETE recorder for assignment writes.
pub async fn mount_role_assignments(
    server: &MockServer,
    scope: &str,
    principal: &str,
    role_ids: &[&str],
) {
    let sub = scope.split('/').nth(2).unwrap_or("sub");
    let value: Vec<Value> = role_ids
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "id": format!("{scope}/providers/Microsoft.Authorization/roleAssignments/ra-{i}"),
                "name": format!("ra-{i}"),
                "properties": {
                    "roleDefinitionId": format!(
                        "/subscriptions/{sub}/providers/Microsoft.Authorization/roleDefinitions/{r}"
                    ),
                    "principalId": principal,
                    "principalType": "ServicePrincipal",
                    "description": format!("rigg: env dev edge {i}")
                }
            })
        })
        .collect();
    Mock::given(method("GET"))
        .and(path(format!(
            "{scope}/providers/Microsoft.Authorization/roleAssignments"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": value})))
        .with_priority(1)
        .mount(server)
        .await;
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

/// Mount one Microsoft.CognitiveServices account at `id`.
pub async fn mount_cognitive_account(server: &MockServer, id: &str, kind: &str, location: &str) {
    let name = id.rsplit('/').next().unwrap_or("acct").to_string();
    Mock::given(method("GET"))
        .and(path(id.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": name,
            "id": id,
            "kind": kind,
            "location": location,
            "properties": {"endpoint": format!("https://{name}.cognitiveservices.azure.com/")}
        })))
        .with_priority(1)
        .mount(server)
        .await;
}

/// Mount a `PUT userAssignedIdentities/{name}` responder that echoes the
/// created identity with a principal id.
pub async fn mount_create_uami(server: &MockServer, principal_id: &str) {
    let principal_id = principal_id.to_string();
    Mock::given(method("PUT"))
        .and(path_regex(
            r"^.*/providers/Microsoft\.ManagedIdentity/userAssignedIdentities/[^/]+$",
        ))
        .respond_with(move |req: &Request| {
            let id = req.url.path().to_string();
            let name = id.rsplit('/').next().unwrap_or("uami").to_string();
            ResponseTemplate::new(201).set_body_json(json!({
                "name": name,
                "id": id,
                "location": "swedencentral",
                "properties": {"principalId": principal_id, "clientId": "cid-new"}
            }))
        })
        .with_priority(1)
        .mount(server)
        .await;
}
