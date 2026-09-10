//! ARM reads and writes that serve `rigg auth doctor` and the push-time
//! auth gates (spec `2026-09-09-identity-and-auth-design.md` §3.3, §4).
//!
//! These are further `impl ArmClient` blocks on the struct defined in
//! [`crate::arm`] — a second file for the same client, so `arm.rs` stays the
//! discovery/binding surface and this one holds the constraint-and-RBAC
//! surface. Every URL goes through `ArmClient::url` + a registry
//! [`Provider`], so no api-version is ever spelled out here.

use serde_json::{Value, json};

use rigg_core::registry::Provider;

use crate::arm::{AiServicesAccount, ArmClient, ArmResource, ResourceIdentity, deterministic_uuid};
use crate::auth::AuthError;
use crate::error::ClientError;

/// The action a caller needs to grant roles.
const ROLE_ASSIGNMENT_WRITE: &str = "Microsoft.Authorization/roleAssignments/write";

/// What doctor needs to know about a search service.
#[derive(Debug, Clone)]
pub struct SearchServiceInfo {
    pub id: String,
    pub name: String,
    pub location: String,
    /// `sku.name` — `free`, `basic`, `standard`, `standard2`, … (Free has no
    /// managed identity; knowledge bases need Basic+).
    pub sku: String,
    pub identity: ResourceIdentity,
    /// Entra ID authentication is accepted: `authOptions.aadOrApiKey` is
    /// present, or local auth is disabled outright (RBAC-only).
    pub rbac_enabled: bool,
    /// `properties.disableLocalAuth` — RBAC-only when true.
    pub disable_local_auth: bool,
    /// `Enabled` / `Disabled`.
    pub public_network_access: String,
    /// `properties.networkRuleSet` verbatim (IP rules, bypass).
    pub network_rule_set: Value,
}

/// What doctor needs to know about a storage account.
#[derive(Debug, Clone)]
pub struct StorageAccountInfo {
    pub id: String,
    pub name: String,
    pub location: String,
    /// `networkAcls.defaultAction` — `Allow` or `Deny` (firewalled).
    pub default_action: String,
    /// `networkAcls.bypass` — a comma-separated set, e.g. `Logging, Metrics,
    /// AzureServices`.
    pub bypass: String,
    /// `networkAcls.resourceAccessRules` — the resource-instance exceptions.
    pub resource_access_rules: Vec<ResourceAccessRule>,
    /// `Enabled` / `Disabled` (Disabled ⇒ only a shared private link works).
    pub public_network_access: String,
    /// `allowSharedKeyAccess` — `None` when the account does not report it.
    pub allow_shared_key_access: Option<bool>,
    /// Hierarchical namespace (ADLS Gen2).
    pub is_hns_enabled: bool,
}

impl StorageAccountInfo {
    /// Whether the firewall is on (`defaultAction: Deny`).
    pub fn is_firewalled(&self) -> bool {
        self.default_action.eq_ignore_ascii_case("Deny")
    }

    /// Whether `bypass` admits the trusted-services exception.
    pub fn bypasses_azure_services(&self) -> bool {
        bypass_contains_azure_services(&self.bypass)
    }

    /// Whether a resource-instance rule already admits `resource_id`.
    pub fn admits_resource(&self, resource_id: &str) -> bool {
        self.resource_access_rules
            .iter()
            .any(|r| r.resource_id.eq_ignore_ascii_case(resource_id))
    }
}

/// One `networkAcls.resourceAccessRules` entry.
#[derive(Debug, Clone)]
pub struct ResourceAccessRule {
    pub tenant_id: String,
    pub resource_id: String,
}

/// Blob service settings that data-source deletion-detection policies depend on.
#[derive(Debug, Clone)]
pub struct BlobServiceInfo {
    pub soft_delete_enabled: bool,
    pub soft_delete_days: Option<u32>,
    /// Blob versioning — must be **off** for
    /// `NativeBlobSoftDeleteDeletionDetectionPolicy`.
    pub versioning_enabled: bool,
}

/// One role assignment, as doctor and `rigg auth roles` read it.
#[derive(Debug, Clone)]
pub struct RoleAssignmentInfo {
    /// The assignment's own ARM id (what `delete_role_assignment` takes).
    pub id: String,
    /// `properties.roleDefinitionId` — the full role definition ARM id.
    pub role_definition_id: String,
    /// `properties.description` — rigg stamps its own assignments here.
    pub description: String,
    /// `properties.scope` — the scope the assignment was **made** at, which
    /// an `atScope()` listing reports for inherited assignments too.
    pub scope: String,
}

impl RoleAssignmentInfo {
    /// The role definition GUID (the id's last segment).
    pub fn role_guid(&self) -> &str {
        self.role_definition_id
            .rsplit('/')
            .next()
            .unwrap_or(&self.role_definition_id)
    }
}

/// Who the operator's token says they are.
#[derive(Debug, Clone)]
pub struct CallerIdentity {
    /// The `oid` claim — the directory object id to assign roles to.
    pub object_id: String,
    /// `User` or `ServicePrincipal` — what `principalType` a role assignment
    /// for this caller must carry.
    pub principal_type: String,
    /// A human label: UPN, preferred username, name, or the app id.
    pub display: String,
}

impl ArmClient {
    // ------------------------------------------------------------ search --

    /// Read a search service's SKU, identity, auth options and network state.
    pub async fn get_search_service(&self, id: &str) -> Result<SearchServiceInfo, ClientError> {
        let value = self.get_json(&self.url(id, Provider::SearchArm)).await?;
        let auth_options = value.pointer("/properties/authOptions");
        let disable_local_auth = value
            .pointer("/properties/disableLocalAuth")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(SearchServiceInfo {
            id: str_at(&value, "/id").unwrap_or_else(|| id.to_string()),
            name: str_at(&value, "/name").unwrap_or_default(),
            location: str_at(&value, "/location").unwrap_or_default(),
            sku: str_at(&value, "/sku/name").unwrap_or_default(),
            identity: crate::arm::identity_from(&value).unwrap_or(ResourceIdentity {
                kind: "None".to_string(),
                principal_id: None,
                user_assigned: Vec::new(),
            }),
            rbac_enabled: disable_local_auth
                || auth_options.is_some_and(|o| o.get("aadOrApiKey").is_some()),
            disable_local_auth,
            public_network_access: str_at(&value, "/properties/publicNetworkAccess")
                .unwrap_or_default(),
            network_rule_set: value
                .pointer("/properties/networkRuleSet")
                .cloned()
                .unwrap_or(Value::Null),
        })
    }

    /// Accept Entra ID tokens on the data plane: PATCH `authOptions` to
    /// `aadOrApiKey` with the bearer-challenge failure mode (spec §3.3).
    pub async fn set_search_auth_options(&self, id: &str) -> Result<(), ClientError> {
        self.patch_json(
            &self.url(id, Provider::SearchArm),
            &json!({
                "properties": {
                    "authOptions": {
                        "aadOrApiKey": {"aadAuthFailureMode": "http401WithBearerChallenge"}
                    }
                }
            }),
        )
        .await
        .map(|_| ())
    }

    /// Attach a user-assigned identity to a resource, keeping every identity
    /// it already has.
    ///
    /// The current `identity` block is read back and merged: PATCHing
    /// `identity` replaces it, so sending only the new entry would detach
    /// every other user-assigned identity and — where one exists — the
    /// system-assigned identity, which is the only one the storage
    /// trusted-services exception accepts (spec §7). A system identity that
    /// is already on stays on; one that is not is not switched on here.
    pub async fn attach_user_assigned_identity(
        &self,
        id: &str,
        provider: Provider,
        uami_id: &str,
    ) -> Result<(), ClientError> {
        let url = self.url(id, provider);
        let current = self.get_json(&url).await?;
        let mut map = current
            .pointer("/identity/userAssignedIdentities")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        map.insert(uami_id.to_string(), json!({}));
        let has_system = str_at(&current, "/identity/type")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("systemassigned");
        let kind = if has_system {
            "SystemAssigned, UserAssigned"
        } else {
            "UserAssigned"
        };
        self.patch_json(
            &url,
            &json!({
                "identity": {"type": kind, "userAssignedIdentities": Value::Object(map)}
            }),
        )
        .await
        .map(|_| ())
    }

    /// The service's `sharedPrivateLinkResources` — what a storage account
    /// with `publicNetworkAccess: Disabled` needs (spec §3.3).
    pub async fn list_shared_private_links(
        &self,
        search_id: &str,
    ) -> Result<Vec<Value>, ClientError> {
        let url = self.url(
            &format!("{search_id}/sharedPrivateLinkResources"),
            Provider::SearchArm,
        );
        let value = self.get_json(&url).await?;
        Ok(value
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    // ----------------------------------------------------------- storage --

    /// Read a storage account's network rules and key/namespace settings.
    pub async fn get_storage_account(&self, id: &str) -> Result<StorageAccountInfo, ClientError> {
        let value = self.get_json(&self.url(id, Provider::StorageArm)).await?;
        let acls = value
            .pointer("/properties/networkAcls")
            .cloned()
            .unwrap_or(Value::Null);
        Ok(StorageAccountInfo {
            id: str_at(&value, "/id").unwrap_or_else(|| id.to_string()),
            name: str_at(&value, "/name").unwrap_or_default(),
            location: str_at(&value, "/location").unwrap_or_default(),
            default_action: str_at(&acls, "/defaultAction").unwrap_or_else(|| "Allow".to_string()),
            // ARM always reports `bypass` (a new account defaults to
            // `AzureServices`), so an absent value is not a default to
            // reconstruct — it is a value rigg has not seen. `None` is the
            // conservative reading: it makes the trusted-services check say
            // "not bypassed", which at worst proposes a fix that is already
            // in place, where assuming `AzureServices` would silently pass a
            // check that was never verified.
            bypass: str_at(&acls, "/bypass").unwrap_or_else(|| "None".to_string()),
            resource_access_rules: acls
                .get("resourceAccessRules")
                .and_then(Value::as_array)
                .map(|rules| {
                    rules
                        .iter()
                        .map(|r| ResourceAccessRule {
                            tenant_id: str_at(r, "/tenantId").unwrap_or_default(),
                            resource_id: str_at(r, "/resourceId").unwrap_or_default(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            public_network_access: str_at(&value, "/properties/publicNetworkAccess")
                .unwrap_or_default(),
            allow_shared_key_access: value
                .pointer("/properties/allowSharedKeyAccess")
                .and_then(Value::as_bool),
            is_hns_enabled: value
                .pointer("/properties/isHnsEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Read `blobServices/default`: soft delete and versioning.
    pub async fn get_blob_service_properties(
        &self,
        account_id: &str,
    ) -> Result<BlobServiceInfo, ClientError> {
        let url = self.url(
            &format!("{account_id}/blobServices/default"),
            Provider::StorageArm,
        );
        let value = self.get_json(&url).await?;
        let policy = value.pointer("/properties/deleteRetentionPolicy");
        let enabled = policy
            .and_then(|p| p.get("enabled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok(BlobServiceInfo {
            soft_delete_enabled: enabled,
            soft_delete_days: enabled
                .then(|| {
                    policy
                        .and_then(|p| p.get("days"))
                        .and_then(Value::as_u64)
                        .map(|d| d as u32)
                })
                .flatten(),
            versioning_enabled: value
                .pointer("/properties/isVersioningEnabled")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    /// Turn on blob soft delete with a retention of `days`.
    pub async fn set_blob_soft_delete(
        &self,
        account_id: &str,
        days: u32,
    ) -> Result<(), ClientError> {
        let url = self.url(
            &format!("{account_id}/blobServices/default"),
            Provider::StorageArm,
        );
        self.put_json(
            &url,
            &json!({
                "properties": {
                    "deleteRetentionPolicy": {"enabled": true, "days": days}
                }
            }),
        )
        .await
        .map(|_| ())
    }

    /// Add `AzureServices` to a storage account's firewall bypass — the
    /// trusted-services exception the search service's *system* identity
    /// needs (spec §3.3).
    ///
    /// A no-op (no request) when the bypass already admits it. The whole
    /// `networkAcls` object is read back and re-sent, because PATCHing it
    /// replaces rather than merges: dropping `defaultAction`, the IP rules
    /// or the VNet rules here would silently open or close the firewall.
    pub async fn add_storage_bypass_azure_services(&self, id: &str) -> Result<(), ClientError> {
        let url = self.url(id, Provider::StorageArm);
        let value = self.get_json(&url).await?;
        let mut acls = value
            .pointer("/properties/networkAcls")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let current = acls.get("bypass").and_then(Value::as_str).unwrap_or("");
        if bypass_contains_azure_services(current) {
            return Ok(());
        }
        acls["bypass"] = json!(with_azure_services(current));
        self.patch_json(&url, &json!({"properties": {"networkAcls": acls}}))
            .await
            .map(|_| ())
    }

    /// Add a resource-instance rule admitting `resource_id` (a search
    /// service) through a storage account's firewall — the alternative to
    /// the trusted-services bypass, and the only one that works for a
    /// user-assigned identity (spec §3.3).
    ///
    /// A no-op when the rule is already present; merges into the existing
    /// `networkAcls` for the same reason as
    /// [`Self::add_storage_bypass_azure_services`].
    pub async fn add_storage_resource_instance_rule(
        &self,
        id: &str,
        tenant: &str,
        resource_id: &str,
    ) -> Result<(), ClientError> {
        let url = self.url(id, Provider::StorageArm);
        let value = self.get_json(&url).await?;
        let mut acls = value
            .pointer("/properties/networkAcls")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let mut rules = acls
            .get("resourceAccessRules")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if rules.iter().any(|r| {
            r.get("resourceId")
                .and_then(Value::as_str)
                .is_some_and(|existing| existing.eq_ignore_ascii_case(resource_id))
        }) {
            return Ok(());
        }
        rules.push(json!({"tenantId": tenant, "resourceId": resource_id}));
        acls["resourceAccessRules"] = Value::Array(rules);
        self.patch_json(&url, &json!({"properties": {"networkAcls": acls}}))
            .await
            .map(|_| ())
    }

    // ------------------------------------------------ cognitive services --

    /// Read a Microsoft.CognitiveServices account by ARM id — the `kind`
    /// check for `AIServicesByIdentity` (spec §3.3) needs the account the
    /// file's `subdomainUrl` resolves to, not a name lookup.
    pub async fn get_cognitive_account_by_id(
        &self,
        id: &str,
    ) -> Result<AiServicesAccount, ClientError> {
        let value = self
            .get_json(&self.url(id, Provider::CognitiveServicesArm))
            .await?;
        Ok(serde_json::from_value(value)?)
    }

    // -------------------------------------------------------------- RBAC --

    /// Whether the caller may create role assignments at `scope`.
    ///
    /// `Microsoft.Authorization/permissions` returns the caller's effective
    /// permission sets at the scope; the answer is yes when any set's
    /// `actions` matches `roleAssignments/write` and its `notActions` does
    /// not take it back.
    pub async fn can_write_role_assignments(&self, scope: &str) -> Result<bool, ClientError> {
        let url = self.url(
            &format!("{scope}/providers/Microsoft.Authorization/permissions"),
            Provider::AuthorizationArm,
        );
        let value = self.get_json(&url).await?;
        Ok(value
            .get("value")
            .and_then(Value::as_array)
            .is_some_and(|sets| {
                sets.iter()
                    .any(|s| permission_grants(s, ROLE_ASSIGNMENT_WRITE))
            }))
    }

    /// Every role assignment that applies to one principal **at** `scope` —
    /// the ones made here and the ones inherited from an ancestor scope.
    ///
    /// `atScope() and assignedTo('{id}')` is the filter doctor needs:
    /// `assignedTo` also matches assignments the principal holds through
    /// group membership, and `atScope()` includes what a parent scope grants,
    /// which is just as effective as a grant made here. Each entry reports
    /// the scope it was made at, so a caller that *writes* — `rigg auth roles
    /// remove` — can keep to the ones at this scope; see
    /// [`Self::list_rigg_role_assignments`].
    pub async fn role_assignments_for(
        &self,
        scope: &str,
        principal_id: &str,
    ) -> Result<Vec<RoleAssignmentInfo>, ClientError> {
        self.list_role_assignments_filtered(
            scope,
            &format!("atScope() and assignedTo('{principal_id}')"),
        )
        .await
    }

    /// `{scope}/…/roleAssignments?$filter=…`, following `nextLink` to the
    /// end: a scope with many assignments pages, and a truncated first page
    /// would read as "the role is missing".
    async fn list_role_assignments_filtered(
        &self,
        scope: &str,
        filter: &str,
    ) -> Result<Vec<RoleAssignmentInfo>, ClientError> {
        // Same cycle and page-cap guard as `ArmClient::list_location`: a
        // server that keeps handing back a next link cannot hang the caller.
        const MAX_LIST_PAGES: usize = 1000;

        let mut url = format!(
            "{}&$filter={}",
            self.url(
                &format!("{scope}/providers/Microsoft.Authorization/roleAssignments"),
                Provider::AuthorizationArm
            ),
            urlencoding::encode(filter)
        );
        let mut assignments = Vec::new();
        let mut pages = 0usize;
        loop {
            let value = self.get_json(&url).await?;
            assignments.extend(role_assignments_from(&value));
            pages += 1;
            match value.get("nextLink").and_then(Value::as_str) {
                Some(next) if !next.is_empty() => {
                    if next == url || pages >= MAX_LIST_PAGES {
                        return Err(ClientError::Api {
                            status: 502,
                            message: format!(
                                "listing role assignments at {scope} did not terminate: Azure kept \
                                 returning a next page link after {pages} pages"
                            ),
                        });
                    }
                    url = next.to_string();
                }
                _ => return Ok(assignments),
            }
        }
    }

    /// Create a role assignment with an explicit principal type and
    /// description. The name is deterministic in
    /// `(scope, principal, role)`, so re-running is idempotent and a 409 is
    /// success.
    pub async fn create_role_assignment_described(
        &self,
        scope: &str,
        principal_id: &str,
        role_definition_guid: &str,
        principal_type: &str,
        description: &str,
    ) -> Result<(), ClientError> {
        let assignment_name =
            deterministic_uuid(&format!("{scope}|{principal_id}|{role_definition_guid}"));
        let url = self.url(
            &format!("{scope}/providers/Microsoft.Authorization/roleAssignments/{assignment_name}"),
            Provider::AuthorizationArm,
        );
        let sub = scope.split('/').nth(2).unwrap_or_default();
        let mut body = json!({
            "properties": {
                "roleDefinitionId": format!(
                    "/subscriptions/{sub}/providers/Microsoft.Authorization/roleDefinitions/{role_definition_guid}"
                ),
                "principalId": principal_id,
                "principalType": principal_type
            }
        });
        // `description` is optional; omitted rather than sent empty so the
        // plain `create_role_assignment` keeps its exact 1.x wire format.
        if !description.is_empty() {
            body["properties"]["description"] = json!(description);
        }
        match self.put_json(&url, &body).await {
            Ok(_) => Ok(()),
            // Already there → the desired state is reached.
            Err(ClientError::Api { status: 409, .. }) => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Remove one role assignment by its ARM id. A missing assignment is
    /// success — the desired state is "gone".
    pub async fn delete_role_assignment(&self, id: &str) -> Result<(), ClientError> {
        self.delete_ok(&self.url(id, Provider::AuthorizationArm))
            .await
    }

    /// Role assignments made **at** `scope` whose description starts with
    /// `description_prefix` — the ones rigg stamped here, for
    /// `rigg auth roles list|remove`.
    ///
    /// `atScope()` also returns what ancestor scopes grant, so the result is
    /// narrowed to assignments whose own `properties.scope` *is* `scope`:
    /// removing a subscription-wide grant because it happened to be visible
    /// at a storage account would take away far more than rigg gave.
    pub async fn list_rigg_role_assignments(
        &self,
        scope: &str,
        description_prefix: &str,
    ) -> Result<Vec<RoleAssignmentInfo>, ClientError> {
        Ok(self
            .list_role_assignments_filtered(scope, "atScope()")
            .await?
            .into_iter()
            .filter(|a| {
                a.description.starts_with(description_prefix) && a.scope.eq_ignore_ascii_case(scope)
            })
            .collect())
    }

    // ---------------------------------------------------------- identity --

    /// Create (or update) a user-assigned managed identity.
    pub async fn create_user_assigned_identity(
        &self,
        subscription: &str,
        rg: &str,
        name: &str,
        location: &str,
    ) -> Result<ArmResource, ClientError> {
        let url = self.url(
            &format!(
                "/subscriptions/{subscription}/resourceGroups/{rg}/providers/Microsoft.ManagedIdentity/userAssignedIdentities/{name}"
            ),
            Provider::ManagedIdentityArm,
        );
        let value = self.put_json(&url, &json!({"location": location})).await?;
        Ok(crate::arm::arm_resource_from_value(&value, None))
    }

    /// Who the bearer token belongs to, decoded from the token itself.
    ///
    /// The JWT payload is read without verifying the signature — the token
    /// is one Azure just issued to *this* process, and ARM verifies it on
    /// every call anyway; this only avoids a Graph round-trip for the
    /// caller's own `oid`. A token that is not a JWT (a test token, or a
    /// pre-minted opaque one) is an error the callers treat as "unknown".
    pub async fn caller_object_id(&self) -> Result<CallerIdentity, ClientError> {
        caller_identity_from_token(self.token())
    }
}

/// The string at a JSON pointer, when present and non-empty.
fn str_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Parse a role-assignment listing response.
fn role_assignments_from(value: &Value) -> Vec<RoleAssignmentInfo> {
    value
        .get("value")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|a| RoleAssignmentInfo {
                    id: str_at(a, "/id").unwrap_or_default(),
                    role_definition_id: str_at(a, "/properties/roleDefinitionId")
                        .unwrap_or_default(),
                    description: str_at(a, "/properties/description").unwrap_or_default(),
                    scope: str_at(a, "/properties/scope").unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether one `Microsoft.Authorization/permissions` entry grants `action`.
fn permission_grants(set: &Value, action: &str) -> bool {
    let matches_any = |field: &str| {
        set.get(field)
            .and_then(Value::as_array)
            .is_some_and(|patterns| {
                patterns
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|p| action_matches(p, action))
            })
    };
    matches_any("actions") && !matches_any("notActions")
}

/// ARM action-pattern matching, case-insensitive: `*` matches any run of
/// characters, so `*` matches everything, a trailing `*` is a prefix match,
/// and a pattern without one is an exact match.
///
/// The general form matters for `notActions`: Contributor's
/// `Microsoft.Authorization/*/Write` — a wildcard in the *middle* — is
/// exactly what takes the role-assignment grant back from a caller whose
/// `actions` say `*`.
fn action_matches(pattern: &str, action: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let action = action.to_ascii_lowercase();
    let mut segments = pattern.split('*');
    let Some(first) = segments.next() else {
        return false;
    };
    let Some(mut rest) = action.strip_prefix(first) else {
        return false;
    };
    let segments: Vec<&str> = segments.collect();
    let Some((last, middle)) = segments.split_last() else {
        // No wildcard at all: the prefix had to consume everything.
        return rest.is_empty();
    };
    for segment in middle {
        match rest.find(segment) {
            Some(at) => rest = &rest[at + segment.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Whether a `networkAcls.bypass` set admits Azure trusted services.
fn bypass_contains_azure_services(bypass: &str) -> bool {
    bypass
        .split(',')
        .any(|part| part.trim().eq_ignore_ascii_case("AzureServices"))
}

/// `bypass` with `AzureServices` added, preserving the existing entries.
///
/// `None` is ARM's sentinel for "nothing is bypassed", not an entry: keeping
/// it would make the set `None, AzureServices`, which ARM rejects.
fn with_azure_services(bypass: &str) -> String {
    let mut parts: Vec<&str> = bypass
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty() && !p.eq_ignore_ascii_case("None"))
        .collect();
    parts.push("AzureServices");
    parts.join(", ")
}

/// Decode an access token's claims into a [`CallerIdentity`].
pub(crate) fn caller_identity_from_token(token: &str) -> Result<CallerIdentity, ClientError> {
    let claims = jwt_claims(token).ok_or_else(|| {
        ClientError::Auth(AuthError::AuthFailed(
            "token is not a JWT; the caller's own object id is unknown".to_string(),
        ))
    })?;
    let claim = |name: &str| str_at(&claims, &format!("/{name}"));
    let object_id = claim("oid").or_else(|| claim("sub")).ok_or_else(|| {
        ClientError::Auth(AuthError::AuthFailed(
            "token carries no `oid` claim; the caller's own object id is unknown".to_string(),
        ))
    })?;
    let user_name = claim("upn").or_else(|| claim("preferred_username"));
    let principal_type = if user_name.is_some() {
        "User"
    } else {
        "ServicePrincipal"
    };
    let display = user_name
        .or_else(|| claim("name"))
        .or_else(|| claim("appid"))
        .or_else(|| claim("azp"))
        .unwrap_or_else(|| object_id.clone());
    Ok(CallerIdentity {
        object_id,
        principal_type: principal_type.to_string(),
        display,
    })
}

/// The claims of a JWT's payload segment. `None` when `token` is not a
/// three-segment JWT with a base64url JSON payload.
fn jwt_claims(token: &str) -> Option<Value> {
    let mut parts = token.split('.');
    let (_header, payload, signature) = (parts.next()?, parts.next()?, parts.next()?);
    if signature.is_empty() || parts.next().is_some() {
        return None;
    }
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

/// Decode unpadded base64url. Written out rather than pulling in a
/// dependency: this is the only base64 in the crate.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let sextet = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        })
    };
    let input = input.trim_end_matches('=');
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    for chunk in input.as_bytes().chunks(4) {
        if chunk.len() == 1 {
            return None;
        }
        let mut acc = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            acc |= sextet(*c)? << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_matches_star_prefix_and_exact() {
        // `*` — Owner.
        assert!(action_matches("*", ROLE_ASSIGNMENT_WRITE));
        // trailing-`*` prefix — User Access Administrator's grant.
        assert!(action_matches(
            "Microsoft.Authorization/*",
            ROLE_ASSIGNMENT_WRITE
        ));
        assert!(action_matches(
            "Microsoft.Authorization/roleAssignments/*",
            ROLE_ASSIGNMENT_WRITE
        ));
        // exact, case-insensitively.
        assert!(action_matches(ROLE_ASSIGNMENT_WRITE, ROLE_ASSIGNMENT_WRITE));
        assert!(action_matches(
            "microsoft.authorization/roleassignments/write",
            ROLE_ASSIGNMENT_WRITE
        ));
        // a wildcard in the middle — Contributor's notAction.
        assert!(action_matches(
            "Microsoft.Authorization/*/Write",
            ROLE_ASSIGNMENT_WRITE
        ));
        // non-matches.
        assert!(!action_matches(
            "Microsoft.Storage/*",
            ROLE_ASSIGNMENT_WRITE
        ));
        assert!(!action_matches("*/read", ROLE_ASSIGNMENT_WRITE));
        assert!(!action_matches(
            "Microsoft.Authorization/roleAssignments/read",
            ROLE_ASSIGNMENT_WRITE
        ));
        assert!(!action_matches(
            "Microsoft.Authorization/*/Delete",
            ROLE_ASSIGNMENT_WRITE
        ));
        assert!(!action_matches("", ROLE_ASSIGNMENT_WRITE));
    }

    #[test]
    fn permission_set_not_actions_take_the_grant_back() {
        let owner = json!({"actions": ["*"], "notActions": []});
        assert!(permission_grants(&owner, ROLE_ASSIGNMENT_WRITE));

        let contributor = json!({
            "actions": ["*"],
            "notActions": ["Microsoft.Authorization/*/Write", "Microsoft.Authorization/*/Delete"]
        });
        assert!(!permission_grants(&contributor, ROLE_ASSIGNMENT_WRITE));

        let reader = json!({"actions": ["*/read"], "notActions": []});
        assert!(!permission_grants(&reader, ROLE_ASSIGNMENT_WRITE));

        let uaa = json!({
            "actions": ["Microsoft.Authorization/roleAssignments/*"],
            "notActions": []
        });
        assert!(permission_grants(&uaa, ROLE_ASSIGNMENT_WRITE));
    }

    #[test]
    fn bypass_parsing_and_merging() {
        assert!(bypass_contains_azure_services(
            "Logging, Metrics, AzureServices"
        ));
        assert!(bypass_contains_azure_services("azureservices"));
        assert!(!bypass_contains_azure_services("Logging, Metrics"));
        assert!(!bypass_contains_azure_services("None"));
        assert_eq!(
            with_azure_services("Logging, Metrics"),
            "Logging, Metrics, AzureServices"
        );
        assert_eq!(with_azure_services(""), "AzureServices");
        // `None` is the "nothing bypassed" sentinel, not an entry to keep.
        assert_eq!(with_azure_services("None"), "AzureServices");
        assert_eq!(with_azure_services("none"), "AzureServices");
    }

    #[test]
    fn base64url_decodes_unpadded_input() {
        assert_eq!(base64url_decode("").unwrap(), Vec::<u8>::new());
        assert_eq!(base64url_decode("YQ").unwrap(), b"a");
        assert_eq!(base64url_decode("YWI").unwrap(), b"ab");
        assert_eq!(base64url_decode("YWJj").unwrap(), b"abc");
        assert_eq!(base64url_decode("YWJjZA==").unwrap(), b"abcd");
        // `+` and `/` are the *standard* alphabet, not base64url.
        assert!(base64url_decode("a+b/").is_none());
    }

    /// Build a token whose payload carries `claims`.
    fn token_with(claims: Value) -> String {
        fn enc(bytes: &[u8]) -> String {
            const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in bytes.chunks(3) {
                let b = [
                    chunk[0],
                    *chunk.get(1).unwrap_or(&0),
                    *chunk.get(2).unwrap_or(&0),
                ];
                let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                for i in 0..=chunk.len() {
                    out.push(A[((n >> (18 - 6 * i)) & 63) as usize] as char);
                }
            }
            out
        }
        format!(
            "{}.{}.sig",
            enc(br#"{"alg":"RS256"}"#),
            enc(claims.to_string().as_bytes())
        )
    }

    #[test]
    fn caller_identity_reads_user_claims() {
        let caller = caller_identity_from_token(&token_with(json!({
            "oid": "oid-1",
            "upn": "k@example.com",
            "appid": "cli"
        })))
        .unwrap();
        assert_eq!(caller.object_id, "oid-1");
        assert_eq!(caller.principal_type, "User");
        assert_eq!(caller.display, "k@example.com");
    }

    #[test]
    fn caller_identity_prefers_preferred_username_when_upn_is_absent() {
        let caller = caller_identity_from_token(&token_with(json!({
            "oid": "oid-2",
            "preferred_username": "guest@other.com",
            "name": "Guest"
        })))
        .unwrap();
        assert_eq!(caller.principal_type, "User");
        assert_eq!(caller.display, "guest@other.com");
    }

    #[test]
    fn caller_identity_reads_service_principal_claims() {
        let caller =
            caller_identity_from_token(&token_with(json!({"oid": "oid-3", "azp": "app-2"})))
                .unwrap();
        assert_eq!(caller.object_id, "oid-3");
        assert_eq!(caller.principal_type, "ServicePrincipal");
        assert_eq!(caller.display, "app-2");
    }

    #[test]
    fn caller_identity_rejects_a_non_jwt() {
        // The token every fake-backed test uses.
        let err = caller_identity_from_token("test-token").unwrap_err();
        assert!(err.to_string().contains("not a JWT"), "{err}");
        // Three segments, but the payload is not base64url JSON.
        let err = caller_identity_from_token("a.!!.c").unwrap_err();
        assert!(err.to_string().contains("not a JWT"), "{err}");
    }

    #[test]
    fn caller_identity_without_an_oid_claim_is_an_error() {
        let err = caller_identity_from_token(&token_with(json!({"appid": "app"}))).unwrap_err();
        assert!(err.to_string().contains("oid"), "{err}");
    }

    #[test]
    fn role_assignment_role_guid_is_the_last_segment() {
        let a = RoleAssignmentInfo {
            id: "/scope/providers/Microsoft.Authorization/roleAssignments/ra".to_string(),
            role_definition_id:
                "/subscriptions/s/providers/Microsoft.Authorization/roleDefinitions/guid-1"
                    .to_string(),
            description: String::new(),
            scope: "/scope".to_string(),
        };
        assert_eq!(a.role_guid(), "guid-1");
    }

    #[test]
    fn storage_info_helpers() {
        let info = StorageAccountInfo {
            id: "/id".to_string(),
            name: "acct".to_string(),
            location: "swedencentral".to_string(),
            default_action: "Deny".to_string(),
            bypass: "Logging, Metrics".to_string(),
            resource_access_rules: vec![ResourceAccessRule {
                tenant_id: "t".to_string(),
                resource_id: "/subscriptions/s/.../searchServices/srch".to_string(),
            }],
            public_network_access: "Enabled".to_string(),
            allow_shared_key_access: Some(false),
            is_hns_enabled: false,
        };
        assert!(info.is_firewalled());
        assert!(!info.bypasses_azure_services());
        assert!(info.admits_resource("/SUBSCRIPTIONS/S/.../SEARCHSERVICES/SRCH"));
        assert!(!info.admits_resource("/other"));
    }
}
