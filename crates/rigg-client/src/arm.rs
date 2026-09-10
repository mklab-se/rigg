//! Azure Resource Manager client for discovering Search and Foundry services

use std::collections::BTreeMap;

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;
use tracing::debug;

use rigg_core::binding::{Binding, BindingType, BindingValue, ResolvedBinding, TargetKind};
use rigg_core::registry::{self, ARM_BASE_URL, Provider};

use crate::auth::AzCliAuth;
use crate::error::ClientError;

/// Azure Resource Manager client for subscription/service discovery
pub struct ArmClient {
    http: Client,
    token: String,
    base_url: String,
}

/// The ARM base URL to use: `RIGG_ARM_ENDPOINT` when set (trimmed of a
/// trailing `/`), else [`ARM_BASE_URL`]. Pulled out as a pure function so it
/// can be unit-tested without mutating process env vars.
pub(crate) fn base_url_from(env: Option<&str>) -> String {
    match env {
        Some(v) if !v.is_empty() => v.trim_end_matches('/').to_string(),
        _ => ARM_BASE_URL.to_string(),
    }
}

/// A minimal ARM resource shape shared by list helpers that don't warrant
/// their own typed struct (managed identities, Key Vaults, ...).
#[derive(Debug, Clone, Deserialize)]
pub struct ArmResource {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
}

/// Azure subscription
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub subscription_id: String,
    pub display_name: String,
    pub state: String,
}

impl std::fmt::Display for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.display_name, self.subscription_id)
    }
}

/// Azure AI Search service
#[derive(Debug, Clone, Deserialize)]
pub struct SearchService {
    pub name: String,
    pub location: String,
    pub sku: SearchServiceSku,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchServiceSku {
    pub name: String,
}

impl std::fmt::Display for SearchService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}, {})",
            self.name,
            self.location,
            self.sku.name.to_uppercase()
        )
    }
}

/// Result of the discovery flow
#[derive(Debug, Clone)]
pub struct DiscoveredService {
    pub name: String,
    pub subscription_id: String,
    pub location: String,
}

/// Azure AI Services account (kind=AIServices)
#[derive(Debug, Clone, Deserialize)]
pub struct AiServicesAccount {
    pub name: String,
    pub location: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub properties: AiServicesAccountProperties,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AiServicesAccountProperties {
    /// Primary endpoint (e.g., "https://name.cognitiveservices.azure.com/")
    #[serde(default)]
    pub endpoint: Option<String>,
}

impl AiServicesAccount {
    /// Derive the `.services.ai.azure.com` endpoint for the agents API.
    ///
    /// Extracts the custom subdomain from the ARM `properties.endpoint`
    /// (which may differ from the resource name), then constructs the
    /// AI services endpoint. Falls back to the resource name.
    pub fn agents_endpoint(&self) -> String {
        if let Some(ref endpoint) = self.properties.endpoint
            && let Some(subdomain) = extract_subdomain(endpoint)
        {
            return format!("https://{}.services.ai.azure.com", subdomain);
        }
        format!("https://{}.services.ai.azure.com", self.name)
    }
}

/// Extract the subdomain from an Azure endpoint URL.
///
/// `"https://my-svc.cognitiveservices.azure.com/"` → `"my-svc"`
fn extract_subdomain(endpoint: &str) -> Option<&str> {
    let host = endpoint.strip_prefix("https://")?.split('/').next()?;
    host.split('.').next()
}

impl std::fmt::Display for AiServicesAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.location)
    }
}

/// Microsoft Foundry project (sub-resource of AI Services account)
#[derive(Debug, Clone, Deserialize)]
pub struct FoundryProject {
    /// ARM name — may be "accountName/projectName" for sub-resources
    #[serde(default)]
    name: String,
    pub location: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub properties: FoundryProjectProperties,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FoundryProjectProperties {
    #[serde(default)]
    pub display_name: String,
}

impl FoundryProject {
    /// The project display name (human-friendly, e.g. "proj-default")
    pub fn display_name(&self) -> &str {
        if !self.properties.display_name.is_empty() {
            &self.properties.display_name
        } else {
            // Fallback: parse from "account/project" ARM name
            self.name.rsplit('/').next().unwrap_or(&self.name)
        }
    }
}

impl std::fmt::Display for FoundryProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.display_name(), self.location)
    }
}

/// Azure Storage account
#[derive(Debug, Clone, Deserialize)]
pub struct StorageAccount {
    pub name: String,
    pub location: String,
    #[serde(default)]
    pub id: String,
}

impl std::fmt::Display for StorageAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.location)
    }
}

/// Azure OpenAI model deployment
#[derive(Debug, Clone, Deserialize)]
pub struct ModelDeployment {
    pub name: String,
    #[serde(default)]
    pub properties: ModelDeploymentProperties,
    #[serde(default)]
    pub sku: ModelDeploymentSku,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelDeploymentProperties {
    #[serde(default)]
    pub model: ModelDeploymentModel,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelDeploymentModel {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModelDeploymentSku {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub capacity: u32,
}

impl std::fmt::Display for ModelDeployment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}, {})",
            self.name, self.properties.model.name, self.sku.name
        )
    }
}

/// A resource's managed identity block.
#[derive(Debug, Clone)]
pub struct ResourceIdentity {
    /// `SystemAssigned`, `UserAssigned`, `SystemAssigned, UserAssigned`, or `None`.
    pub kind: String,
    /// System-assigned principal id, when enabled.
    pub principal_id: Option<String>,
    /// (resource id, principal id) of attached user-assigned identities.
    pub user_assigned: Vec<(String, String)>,
}

impl ResourceIdentity {
    /// All principal ids this resource can act as.
    pub fn principal_ids(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.principal_id.iter().map(String::as_str).collect();
        ids.extend(self.user_assigned.iter().map(|(_, p)| p.as_str()));
        ids
    }
}

/// Deterministic UUID-shaped name from a string (stable role-assignment names).
fn deterministic_uuid(input: &str) -> String {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x9e3779b97f4a7c15;
    for b in input.as_bytes() {
        h1 ^= u64::from(*b);
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 = h2.rotate_left(7) ^ u64::from(*b);
        h2 = h2.wrapping_mul(0x2545f4914f6cdd1d);
    }
    let bytes = [h1.to_be_bytes(), h2.to_be_bytes()].concat();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

/// ARM list response envelope
#[derive(Debug, Deserialize)]
struct ArmListResponse<T> {
    value: Vec<T>,
}

impl ArmClient {
    /// Create a new ARM client using the default tenant: `RIGG_ACCESS_TOKEN`
    /// when set, else Azure CLI credentials.
    pub fn new() -> Result<Self, ClientError> {
        Self::for_tenant(None)
    }

    /// Create a new ARM client scoped to a specific tenant. `RIGG_ACCESS_TOKEN`
    /// wins over everything (same static-token path the data-plane clients
    /// use); otherwise falls back to the Azure CLI, via
    /// [`AzCliAuth::get_arm_token_for_tenant`].
    pub fn for_tenant(tenant: Option<&str>) -> Result<Self, ClientError> {
        let token = match std::env::var("RIGG_ACCESS_TOKEN") {
            Ok(t) if !t.is_empty() => t,
            _ => AzCliAuth::get_arm_token_for_tenant(tenant)?,
        };
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            http,
            token,
            base_url: base_url_from(std::env::var("RIGG_ARM_ENDPOINT").ok().as_deref()),
        })
    }

    /// Create a new ARM client from an already-obtained bearer token
    /// (tests, and callers that already hold a token). Honours
    /// `RIGG_ARM_ENDPOINT` for the base URL.
    pub fn with_token(token: String) -> Self {
        Self::with_token_and_base(
            token,
            base_url_from(std::env::var("RIGG_ARM_ENDPOINT").ok().as_deref()),
        )
    }

    /// Create a new ARM client from an already-obtained bearer token and an
    /// explicit base URL — for tests, so ARM-fake tests never touch the
    /// process env var (they run in parallel and would stomp each other).
    pub fn with_token_and_base(token: String, base_url: String) -> Self {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("reqwest client builds");
        Self {
            http,
            token,
            base_url,
        }
    }

    /// ARM URL for `path` (leading `/`) on `provider`'s pinned api-version.
    pub fn url(&self, path: &str, provider: Provider) -> String {
        format!(
            "{}{}?api-version={}",
            self.base_url,
            path,
            registry::provider(provider).stable
        )
    }

    /// This client's ARM base URL (honours `RIGG_ARM_ENDPOINT`) — for
    /// callers building their own URLs (e.g. `ArmResourceClient`).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `resourceType → apiVersions` as ARM registers them for `namespace` in
    /// `subscription_id` (what `az provider show` prints). The ground truth
    /// for which api-version a call may use — the specs repository can be
    /// ahead of it.
    pub async fn provider_api_versions(
        &self,
        subscription_id: &str,
        namespace: &str,
    ) -> Result<BTreeMap<String, Vec<String>>, ClientError> {
        let url = self.url(
            &format!("/subscriptions/{subscription_id}/providers/{namespace}"),
            Provider::ResourcesArm,
        );
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let value: Value = response.json().await?;
        let mut out = BTreeMap::new();
        for rt in value["resourceTypes"].as_array().into_iter().flatten() {
            let name = rt["resourceType"].as_str().unwrap_or_default().to_string();
            let versions = rt["apiVersions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            out.insert(name, versions);
        }
        Ok(out)
    }

    /// Read a resource's managed identity block: `GET {id}?api-version=...`.
    pub async fn get_resource_identity(
        &self,
        resource_id: &str,
        provider: Provider,
    ) -> Result<Option<ResourceIdentity>, ClientError> {
        let url = self.url(resource_id, provider);
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let value: serde_json::Value = response.json().await?;
        let Some(identity) = value.get("identity") else {
            return Ok(None);
        };
        let kind = identity
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("None")
            .to_string();
        let principal_id = identity
            .get("principalId")
            .and_then(|p| p.as_str())
            .map(str::to_string);
        let user_assigned = identity
            .get("userAssignedIdentities")
            .and_then(|u| u.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(id, v)| {
                        v.get("principalId")
                            .and_then(|p| p.as_str())
                            .map(|p| (id.clone(), p.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Some(ResourceIdentity {
            kind,
            principal_id,
            user_assigned,
        }))
    }

    /// Role definition IDs assigned to `principal_id` at (or inherited by) `scope`.
    pub async fn list_role_assignments(
        &self,
        scope: &str,
        principal_id: &str,
    ) -> Result<Vec<String>, ClientError> {
        let url = format!(
            "{}&$filter=principalId%20eq%20'{principal_id}'",
            self.url(
                &format!("{scope}/providers/Microsoft.Authorization/roleAssignments"),
                Provider::AuthorizationArm
            )
        );
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let value: serde_json::Value = response.json().await?;
        Ok(value
            .get("value")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| {
                        a.get("properties")
                            .and_then(|p| p.get("roleDefinitionId"))
                            .and_then(|r| r.as_str())
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Create a role assignment for a principal at a scope.
    pub async fn create_role_assignment(
        &self,
        scope: &str,
        principal_id: &str,
        role_definition_guid: &str,
    ) -> Result<(), ClientError> {
        let assignment_name =
            deterministic_uuid(&format!("{scope}|{principal_id}|{role_definition_guid}"));
        let url = self.url(
            &format!("{scope}/providers/Microsoft.Authorization/roleAssignments/{assignment_name}"),
            Provider::AuthorizationArm,
        );
        let sub = scope.split('/').nth(2).unwrap_or_default();
        let body = serde_json::json!({
            "properties": {
                "roleDefinitionId": format!(
                    "/subscriptions/{sub}/providers/Microsoft.Authorization/roleDefinitions/{role_definition_guid}"
                ),
                "principalId": principal_id,
                "principalType": "ServicePrincipal"
            }
        });
        let response = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        // 409 = already exists → fine
        if status.is_success() || status.as_u16() == 409 {
            return Ok(());
        }
        let text = response.text().await?;
        Err(ClientError::from_response(status.as_u16(), &text))
    }

    /// Enable a system-assigned managed identity on a resource (PATCH).
    pub async fn enable_system_identity(
        &self,
        resource_id: &str,
        provider: Provider,
    ) -> Result<(), ClientError> {
        let url = self.url(resource_id, provider);
        let response = self
            .http
            .patch(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&serde_json::json!({"identity": {"type": "SystemAssigned"}}))
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let text = response.text().await?;
        Err(ClientError::from_response(status.as_u16(), &text))
    }

    /// Find the full ARM resource id of a search service by name.
    pub async fn find_search_service_id(&self, name: &str) -> Result<String, ClientError> {
        for sub in self.list_subscriptions().await? {
            for svc in self.list_search_services(&sub.subscription_id).await? {
                if svc.name.eq_ignore_ascii_case(name) && !svc.id.is_empty() {
                    return Ok(svc.id);
                }
            }
        }
        Err(ClientError::NotFound {
            kind: "search service".to_string(),
            name: name.to_string(),
        })
    }

    /// The ARM bearer token this client authenticated with.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// List subscriptions the user has access to
    pub async fn list_subscriptions(&self) -> Result<Vec<Subscription>, ClientError> {
        let url = self.url("/subscriptions", Provider::ResourcesArm);
        debug!("Listing subscriptions: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<Subscription> = response.json().await?;
        // Only return enabled subscriptions
        Ok(result
            .value
            .into_iter()
            .filter(|s| s.state == "Enabled")
            .collect())
    }

    /// List Azure AI Search services in a subscription
    pub async fn list_search_services(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<SearchService>, ClientError> {
        let url = self.url(
            &format!("/subscriptions/{subscription_id}/providers/Microsoft.Search/searchServices"),
            Provider::SearchArm,
        );
        debug!("Listing search services: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<SearchService> = response.json().await?;
        Ok(result.value)
    }

    /// Find the resource group of a search service by scanning the subscription.
    ///
    /// Returns the resource group name extracted from the service's ARM resource ID.
    pub async fn find_resource_group(
        &self,
        subscription_id: &str,
        service_name: &str,
    ) -> Result<String, ClientError> {
        let services = self.list_search_services(subscription_id).await?;

        for svc in &services {
            if svc.name.eq_ignore_ascii_case(service_name) {
                // Parse resource group from ARM ID:
                // /subscriptions/{sub}/resourceGroups/{rg}/providers/...
                return parse_resource_group(&svc.id).ok_or_else(|| ClientError::Api {
                    status: 0,
                    message: format!("Could not parse resource group from ARM ID: {}", svc.id),
                });
            }
        }

        Err(ClientError::NotFound {
            kind: "Search service".to_string(),
            name: service_name.to_string(),
        })
    }

    /// List Azure AI Services accounts in a subscription (filtered to kind=AIServices)
    pub async fn list_ai_services_accounts(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<AiServicesAccount>, ClientError> {
        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/providers/Microsoft.CognitiveServices/accounts"
            ),
            Provider::CognitiveServicesArm,
        );
        debug!("Listing AI Services accounts: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<AiServicesAccount> = response.json().await?;
        Ok(result
            .value
            .into_iter()
            .filter(|a| a.kind.eq_ignore_ascii_case("AIServices"))
            .collect())
    }

    /// Find the ARM resource id of ANY Microsoft.CognitiveServices account
    /// by name — regardless of kind (AIServices, CognitiveServices, OpenAI,
    /// ...). Unlike [`Self::list_ai_services_accounts`] (which serves
    /// Foundry discovery and filters to kind AIServices), this covers e.g.
    /// the plain CognitiveServices accounts skillsets use for enrichment.
    pub async fn find_cognitive_account_id(&self, name: &str) -> Result<String, ClientError> {
        self.find_cognitive_account(name).await.map(|a| a.id)
    }

    /// Find ANY Microsoft.CognitiveServices account by name (full document,
    /// including its `kind` — needed to tell Foundry/AIServices resources
    /// apart from legacy CognitiveServices accounts).
    pub async fn find_cognitive_account(
        &self,
        name: &str,
    ) -> Result<AiServicesAccount, ClientError> {
        for sub in self.list_subscriptions().await? {
            for acct in self.list_cognitive_accounts(&sub.subscription_id).await? {
                if acct.name.eq_ignore_ascii_case(name) && !acct.id.is_empty() {
                    return Ok(acct);
                }
            }
        }
        Err(ClientError::NotFound {
            kind: "Cognitive Services account".to_string(),
            name: name.to_string(),
        })
    }

    /// List every Microsoft.CognitiveServices account in a subscription
    /// (any kind). Subscriptions the caller cannot read yield an empty list.
    pub async fn list_cognitive_accounts(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<AiServicesAccount>, ClientError> {
        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/providers/Microsoft.CognitiveServices/accounts"
            ),
            Provider::CognitiveServicesArm,
        );
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        if !response.status().is_success() {
            return Ok(Vec::new());
        }
        let result: ArmListResponse<AiServicesAccount> = response.json().await?;
        Ok(result.value)
    }

    /// Every kind=AIServices (Foundry) account visible to the caller,
    /// across all subscriptions.
    pub async fn all_foundry_accounts(&self) -> Result<Vec<AiServicesAccount>, ClientError> {
        let mut out = Vec::new();
        for sub in self.list_subscriptions().await? {
            out.extend(
                self.list_cognitive_accounts(&sub.subscription_id)
                    .await?
                    .into_iter()
                    .filter(|a| a.kind.eq_ignore_ascii_case("AIServices")),
            );
        }
        Ok(out)
    }

    /// All Microsoft.Web sites (function apps / web apps) visible to this
    /// login, by name, across every subscription. Sorted, de-duplicated.
    pub async fn list_web_sites(&self) -> Result<Vec<String>, ClientError> {
        let mut out: Vec<String> = Vec::new();
        for sub in self.list_subscriptions().await? {
            let url = self.url(
                &format!(
                    "/subscriptions/{}/providers/Microsoft.Web/sites",
                    sub.subscription_id
                ),
                Provider::WebArm,
            );
            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.token))
                .send()
                .await?;
            if !response.status().is_success() {
                continue;
            }
            #[derive(Deserialize)]
            struct Site {
                name: String,
            }
            let result: ArmListResponse<Site> = response.json().await?;
            out.extend(result.value.into_iter().map(|s| s.name));
        }
        out.sort();
        out.dedup();
        Ok(out)
    }

    /// Microsoft.Web sites (function apps / web apps) within one
    /// subscription only — the per-subscription counterpart to
    /// [`Self::list_web_sites`], used by the binding fan-out so
    /// `--subscription` narrows the search instead of being ignored.
    pub async fn list_web_sites_subscription(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<ArmResource>, ClientError> {
        Ok(self
            .list_provider_resources(subscription_id, "Microsoft.Web/sites", Provider::WebArm)
            .await?
            .iter()
            .map(|v| arm_resource_from_value(v, None))
            .collect())
    }

    /// Find a Microsoft.Web site (function app / web app) by name across
    /// all visible subscriptions; returns its ARM resource id.
    pub async fn find_web_site_id(&self, name: &str) -> Result<String, ClientError> {
        for sub in self.list_subscriptions().await? {
            let url = self.url(
                &format!(
                    "/subscriptions/{}/providers/Microsoft.Web/sites",
                    sub.subscription_id
                ),
                Provider::WebArm,
            );
            let response = self
                .http
                .get(&url)
                .header("Authorization", format!("Bearer {}", self.token))
                .send()
                .await?;
            if !response.status().is_success() {
                continue;
            }
            #[derive(Deserialize)]
            struct Site {
                name: String,
                #[serde(default)]
                id: String,
            }
            let result: ArmListResponse<Site> = response.json().await?;
            for site in result.value {
                if site.name.eq_ignore_ascii_case(name) && !site.id.is_empty() {
                    return Ok(site.id);
                }
            }
        }
        Err(ClientError::NotFound {
            kind: "function app".to_string(),
            name: name.to_string(),
        })
    }

    /// Fetch a usable key for one function of a function app: the
    /// function-scoped `default` key when present, else any function key,
    /// else the host's `default` function key.
    pub async fn function_key(
        &self,
        site_id: &str,
        function_name: &str,
    ) -> Result<String, ClientError> {
        let url = self.url(
            &format!("{site_id}/functions/{function_name}/listkeys"),
            Provider::WebArm,
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Length", "0")
            .send()
            .await?;
        if response.status().is_success() {
            let keys: serde_json::Value = response.json().await?;
            if let Some(k) = keys.get("default").and_then(Value::as_str).or_else(|| {
                keys.as_object()
                    .and_then(|o| o.values().find_map(Value::as_str))
            }) {
                return Ok(k.to_string());
            }
        }
        // Fallback: host-level function keys.
        let url = self.url(
            &format!("{site_id}/host/default/listkeys"),
            Provider::WebArm,
        );
        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .header("Content-Length", "0")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let keys: serde_json::Value = response.json().await?;
        keys.pointer("/functionKeys/default")
            .and_then(Value::as_str)
            .or_else(|| keys.get("masterKey").and_then(Value::as_str))
            .map(str::to_string)
            .ok_or_else(|| ClientError::NotFound {
                kind: "function key".to_string(),
                name: function_name.to_string(),
            })
    }

    /// The site's Easy Auth (authSettingsV2) configuration.
    pub async fn site_auth_settings(&self, site_id: &str) -> Result<Value, ClientError> {
        let url = self.url(
            &format!("{site_id}/config/authsettingsV2/list"),
            Provider::WebArm,
        );
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        Ok(response.json().await?)
    }

    /// Site configuration (`ipSecurityRestrictions`, `publicNetworkAccess`, …):
    /// not returned by `GET sites/{name}`; lives under `config/web`.
    // Used by the identity-and-auth workstream (spec 2026-09-09-identity-and-auth-design.md).
    pub async fn site_config(&self, site_id: &str) -> Result<Value, ClientError> {
        let url = self.url(&format!("{site_id}/config/web"), Provider::WebArm);
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        Ok(response.json().await?)
    }

    /// List Microsoft Foundry projects under a specific AI Services account.
    ///
    /// Projects are sub-resources at:
    /// `Microsoft.CognitiveServices/accounts/{accountName}/projects`
    ///
    /// The `account_id` should be the full ARM resource ID of the account,
    /// from which we extract the resource group.
    pub async fn list_foundry_projects(
        &self,
        account: &AiServicesAccount,
        subscription_id: &str,
    ) -> Result<Vec<FoundryProject>, ClientError> {
        let resource_group = parse_resource_group(&account.id).ok_or_else(|| ClientError::Api {
            status: 0,
            message: format!("Could not parse resource group from ARM ID: {}", account.id),
        })?;

        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/resourceGroups/{resource_group}/providers/Microsoft.CognitiveServices/accounts/{}/projects",
                account.name
            ),
            Provider::CognitiveServicesArm,
        );
        debug!("Listing Foundry projects: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<FoundryProject> = response.json().await?;
        Ok(result.value)
    }

    /// List storage accounts in a resource group.
    pub async fn list_storage_accounts(
        &self,
        subscription_id: &str,
        resource_group: &str,
    ) -> Result<Vec<StorageAccount>, ClientError> {
        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/resourceGroups/{resource_group}/providers/Microsoft.Storage/storageAccounts"
            ),
            Provider::StorageArm,
        );
        debug!("Listing storage accounts: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<StorageAccount> = response.json().await?;
        Ok(result.value)
    }

    /// List storage accounts across a whole subscription.
    pub async fn list_storage_accounts_subscription(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<StorageAccount>, ClientError> {
        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/providers/Microsoft.Storage/storageAccounts"
            ),
            Provider::StorageArm,
        );
        debug!("Listing storage accounts (subscription-wide): {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<StorageAccount> = response.json().await?;
        Ok(result.value)
    }

    /// Whether a storage account contains a blob container with this name
    /// (checked via ARM with the caller's CLI token — no data-plane access
    /// or account keys involved).
    pub async fn storage_account_has_container(
        &self,
        account_id: &str,
        container: &str,
    ) -> Result<bool, ClientError> {
        let url = self.url(
            &format!("{account_id}/blobServices/default/containers/{container}"),
            Provider::StorageArm,
        );
        debug!("Checking container: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        match response.status().as_u16() {
            200 => Ok(true),
            404 => Ok(false),
            status => {
                let body = response.text().await?;
                Err(ClientError::from_response(status, &body))
            }
        }
    }

    /// Find every storage account (across all visible subscriptions) that
    /// holds a blob container named `container`. Used to auto-construct
    /// identity-based `ResourceId=` data-source connections — the user is
    /// already logged in via Azure CLI, so rigg discovers instead of asking.
    /// Accounts that fail the container check (e.g. insufficient RBAC on an
    /// unrelated subscription) are skipped, not fatal.
    pub async fn find_storage_accounts_with_container(
        &self,
        container: &str,
    ) -> Result<Vec<StorageAccount>, ClientError> {
        let mut matches = Vec::new();
        for sub in self.list_subscriptions().await? {
            let accounts = match self
                .list_storage_accounts_subscription(&sub.subscription_id)
                .await
            {
                Ok(a) => a,
                Err(e) => {
                    debug!(
                        "skipping subscription {} ({}): {e}",
                        sub.display_name, sub.subscription_id
                    );
                    continue;
                }
            };
            for account in accounts {
                if account.id.is_empty() {
                    continue;
                }
                match self
                    .storage_account_has_container(&account.id, container)
                    .await
                {
                    Ok(true) => matches.push(account),
                    Ok(false) => {}
                    Err(e) => debug!("skipping account {} ({e})", account.name),
                }
            }
        }
        Ok(matches)
    }

    /// List model deployments for an AI Services account.
    pub async fn list_model_deployments(
        &self,
        account: &AiServicesAccount,
        subscription_id: &str,
    ) -> Result<Vec<ModelDeployment>, ClientError> {
        let resource_group = parse_resource_group(&account.id).ok_or_else(|| ClientError::Api {
            status: 0,
            message: format!("Could not parse resource group from ARM ID: {}", account.id),
        })?;

        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/resourceGroups/{resource_group}/providers/Microsoft.CognitiveServices/accounts/{}/deployments",
                account.name
            ),
            Provider::CognitiveServicesArm,
        );
        debug!("Listing model deployments: {}", url);

        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let result: ArmListResponse<ModelDeployment> = response.json().await?;
        Ok(result.value)
    }

    /// Create a model deployment on an AI Services account.
    pub async fn create_model_deployment(
        &self,
        account: &AiServicesAccount,
        subscription_id: &str,
        deployment_name: &str,
        model_name: &str,
        model_version: &str,
    ) -> Result<(), ClientError> {
        let resource_group = parse_resource_group(&account.id).ok_or_else(|| ClientError::Api {
            status: 0,
            message: format!("Could not parse resource group from ARM ID: {}", account.id),
        })?;

        let url = self.url(
            &format!(
                "/subscriptions/{subscription_id}/resourceGroups/{resource_group}/providers/Microsoft.CognitiveServices/accounts/{}/deployments/{deployment_name}",
                account.name
            ),
            Provider::CognitiveServicesArm,
        );
        debug!("Creating model deployment: {}", url);

        let body = serde_json::json!({
            "sku": {
                "name": "GlobalStandard",
                "capacity": 1
            },
            "properties": {
                "model": {
                    "format": "OpenAI",
                    "name": model_name,
                    "version": model_version
                }
            }
        });

        let response = self
            .http
            .put(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        Ok(())
    }

    /// List `Microsoft.ManagedIdentity/userAssignedIdentities` in a subscription.
    pub async fn list_user_assigned_identities(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<ArmResource>, ClientError> {
        let items = self
            .list_provider_resources(
                subscription_id,
                "Microsoft.ManagedIdentity/userAssignedIdentities",
                Provider::ManagedIdentityArm,
            )
            .await?;
        Ok(items
            .iter()
            .map(|v| arm_resource_from_value(v, None))
            .collect())
    }

    /// List `Microsoft.KeyVault/vaults` in a subscription.
    pub async fn list_key_vaults(
        &self,
        subscription_id: &str,
    ) -> Result<Vec<ArmResource>, ClientError> {
        let items = self
            .list_provider_resources(
                subscription_id,
                "Microsoft.KeyVault/vaults",
                Provider::KeyVaultArm,
            )
            .await?;
        Ok(items
            .iter()
            .map(|v| arm_resource_from_value(v, Some("vaultUri")))
            .collect())
    }

    /// `GET /subscriptions/{sub}/providers/{resource_type_path}` — the raw
    /// `value` array, shared by the typed list helpers above.
    async fn list_provider_resources(
        &self,
        subscription_id: &str,
        resource_type_path: &str,
        provider: Provider,
    ) -> Result<Vec<Value>, ClientError> {
        let url = self.url(
            &format!("/subscriptions/{subscription_id}/providers/{resource_type_path}"),
            provider,
        );
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let value: Value = response.json().await?;
        Ok(value
            .get("value")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default())
    }

    /// The [`ArmResource`] list for one [`BindingType`] in one subscription —
    /// used by [`Self::resolve_binding`]'s by-name lookup.
    async fn list_resources_for_kind(
        &self,
        kind: TargetKind,
        subscription_id: &str,
    ) -> Result<Vec<ArmResource>, ClientError> {
        match kind {
            TargetKind::Search => Ok(self
                .list_provider_resources(
                    subscription_id,
                    "Microsoft.Search/searchServices",
                    Provider::SearchArm,
                )
                .await?
                .iter()
                .map(|v| arm_resource_from_value(v, None))
                .collect()),
            TargetKind::Foundry | TargetKind::Binding(BindingType::AiServices) => Ok(self
                .list_cognitive_accounts(subscription_id)
                .await?
                .into_iter()
                .map(|a| ArmResource {
                    name: a.name,
                    id: a.id,
                    location: a.location,
                    kind: Some(a.kind),
                    endpoint: a.properties.endpoint,
                })
                .collect()),
            TargetKind::Binding(BindingType::Storage) => Ok(self
                .list_storage_accounts_subscription(subscription_id)
                .await?
                .into_iter()
                .map(|a| ArmResource {
                    name: a.name,
                    id: a.id,
                    location: a.location,
                    kind: None,
                    endpoint: None,
                })
                .collect()),
            TargetKind::Binding(BindingType::FunctionApp) => {
                self.list_web_sites_subscription(subscription_id).await
            }
            TargetKind::Binding(BindingType::Identity) => {
                self.list_user_assigned_identities(subscription_id).await
            }
            TargetKind::Binding(BindingType::KeyVault) => {
                self.list_key_vaults(subscription_id).await
            }
            TargetKind::Binding(BindingType::Api) => Ok(Vec::new()),
        }
    }

    /// The [`Provider`] channel a [`TargetKind`]'s ARM id is read back
    /// through — `None` for `api` (URL bindings have no ARM resource).
    fn provider_for_target(kind: TargetKind) -> Option<Provider> {
        match kind {
            TargetKind::Search => Some(Provider::SearchArm),
            TargetKind::Foundry => Some(Provider::CognitiveServicesArm),
            TargetKind::Binding(BindingType::Storage) => Some(Provider::StorageArm),
            TargetKind::Binding(BindingType::AiServices) => Some(Provider::CognitiveServicesArm),
            TargetKind::Binding(BindingType::FunctionApp) => Some(Provider::WebArm),
            TargetKind::Binding(BindingType::Identity) => Some(Provider::ManagedIdentityArm),
            TargetKind::Binding(BindingType::KeyVault) => Some(Provider::KeyVaultArm),
            TargetKind::Binding(BindingType::Api) => None,
        }
    }

    /// `GET {id}` on `kind`'s provider version, filling location/endpoint/
    /// principal_id from the response and subscription/resource_group from
    /// the id itself.
    async fn resolve_arm_id(
        &self,
        kind: TargetKind,
        id: &str,
    ) -> Result<ResolvedBinding, ClientError> {
        let provider = Self::provider_for_target(kind).ok_or_else(|| ClientError::Api {
            status: 400,
            message: format!("{kind} bindings have no ARM resource to resolve"),
        })?;
        let url = self.url(id, provider);
        let response = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.token))
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }
        let body: Value = response.json().await?;

        let name = body
            .get("name")
            .and_then(Value::as_str)
            .map(String::from)
            .or_else(|| rigg_core::binding::arm_resource_name(id).map(String::from))
            .unwrap_or_default();
        let location = body
            .get("location")
            .and_then(Value::as_str)
            .map(String::from);
        let endpoint = match kind {
            TargetKind::Search => Some(format!("https://{name}.search.windows.net")),
            TargetKind::Foundry | TargetKind::Binding(BindingType::AiServices) => body
                .pointer("/properties/endpoint")
                .and_then(Value::as_str)
                .map(String::from),
            TargetKind::Binding(BindingType::Storage) => body
                .pointer("/properties/primaryEndpoints/blob")
                .and_then(Value::as_str)
                .map(String::from),
            TargetKind::Binding(BindingType::FunctionApp) => {
                Some(format!("https://{name}.azurewebsites.net"))
            }
            TargetKind::Binding(BindingType::KeyVault) => body
                .pointer("/properties/vaultUri")
                .and_then(Value::as_str)
                .map(String::from),
            TargetKind::Binding(BindingType::Identity | BindingType::Api) => None,
        };
        let principal_id = (kind == TargetKind::Binding(BindingType::Identity))
            .then(|| {
                body.pointer("/properties/principalId")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .flatten();

        Ok(ResolvedBinding {
            physical_name: name.to_lowercase(),
            name,
            kind,
            arm_id: Some(id.to_string()),
            subscription: rigg_core::binding::arm_subscription(id).map(String::from),
            resource_group: rigg_core::binding::arm_resource_group(id).map(String::from),
            location,
            endpoint,
            principal_id,
            resolved_at: chrono::Utc::now().to_rfc3339(),
        })
    }

    /// Resolve a declared `dependencies` binding — [`Self::resolve_target`]
    /// for [`TargetKind::Binding`].
    pub async fn resolve_binding(
        &self,
        kind: BindingType,
        value: &str,
        subscription: Option<&str>,
    ) -> Result<ResolvedBinding, ClientError> {
        self.resolve_target(TargetKind::Binding(kind), value, subscription)
            .await
    }

    /// Resolve a binding value (bare name, ARM id, or URL) to a
    /// [`ResolvedBinding`] — for a declared dependency type or for one of an
    /// environment's implicit targets ([`TargetKind::Search`],
    /// [`TargetKind::Foundry`]).
    ///
    /// - An ARM id is `GET`'d directly on the target's provider version.
    /// - A bare name is matched (case-insensitively) against every resource
    ///   of `kind` in `subscription` (or every enabled subscription, when
    ///   `None`): zero matches is a [`ClientError::NotFound`], more than one
    ///   is a `409` [`ClientError::Api`] naming the ambiguous ids.
    /// - A URL (only meaningful for [`BindingType::Api`]) resolves locally —
    ///   no ARM call.
    ///
    /// The returned [`ResolvedBinding::name`] is the ARM resource's name;
    /// callers that know the binding name overwrite it before caching.
    pub async fn resolve_target(
        &self,
        kind: TargetKind,
        value: &str,
        subscription: Option<&str>,
    ) -> Result<ResolvedBinding, ClientError> {
        let probe = Binding {
            kind: kind.binding_type().unwrap_or(BindingType::AiServices),
            value: value.to_string(),
        };
        match probe.value() {
            BindingValue::ArmId(id) => self.resolve_arm_id(kind, &id).await,
            BindingValue::Url(url) => Ok(ResolvedBinding {
                name: value.to_string(),
                kind,
                physical_name: probe.physical_name(),
                arm_id: None,
                subscription: None,
                resource_group: None,
                location: None,
                endpoint: Some(url),
                principal_id: None,
                resolved_at: chrono::Utc::now().to_rfc3339(),
            }),
            BindingValue::Name(name) => {
                let subs: Vec<String> = match subscription {
                    Some(s) => vec![s.to_string()],
                    None => self
                        .list_subscriptions()
                        .await?
                        .into_iter()
                        .map(|s| s.subscription_id)
                        .collect(),
                };
                let mut matches: Vec<ArmResource> = Vec::new();
                // A subscription we cannot list (no RBAC on it, a disabled
                // provider) must not sink the whole lookup — the resource
                // usually lives in one of the others. Remember the first
                // failure so an all-denied fan-out reports *that* instead of
                // a misleading "not found"; but once at least one
                // subscription listed successfully, a zero-match result is a
                // genuine "not found", not that earlier failure.
                let mut first_error: Option<ClientError> = None;
                let mut any_listed_ok = false;
                for sub in &subs {
                    let items = match self.list_resources_for_kind(kind, sub).await {
                        Ok(items) => items,
                        Err(e) => {
                            debug!("skipping subscription {sub} while resolving {kind}: {e}");
                            first_error.get_or_insert(e);
                            continue;
                        }
                    };
                    any_listed_ok = true;
                    matches.extend(
                        items
                            .into_iter()
                            .filter(|r| r.name.eq_ignore_ascii_case(&name)),
                    );
                }
                match matches.len() {
                    0 => Err(if any_listed_ok {
                        ClientError::NotFound {
                            kind: kind.to_string(),
                            name,
                        }
                    } else {
                        first_error.unwrap_or(ClientError::NotFound {
                            kind: kind.to_string(),
                            name,
                        })
                    }),
                    1 => self.resolve_arm_id(kind, &matches[0].id).await,
                    _ => Err(ClientError::Api {
                        status: 409,
                        message: format!(
                            "ambiguous: {} — use the full ARM id",
                            matches
                                .iter()
                                .map(|m| m.id.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    }),
                }
            }
        }
    }
}

/// Build an [`ArmResource`] from a raw ARM list-item `Value`, reading the
/// endpoint from `properties.<endpoint_field>` when given.
fn arm_resource_from_value(v: &Value, endpoint_field: Option<&str>) -> ArmResource {
    ArmResource {
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        id: v
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        location: v
            .get("location")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        kind: v.get("kind").and_then(Value::as_str).map(String::from),
        endpoint: endpoint_field.and_then(|field| {
            v.pointer(&format!("/properties/{field}"))
                .and_then(Value::as_str)
                .map(String::from)
        }),
    }
}

/// Parse resource group from an ARM resource ID.
///
/// ARM IDs look like: `/subscriptions/{sub}/resourceGroups/{rg}/providers/...`
fn parse_resource_group(arm_id: &str) -> Option<String> {
    let parts: Vec<&str> = arm_id.split('/').collect();
    for (i, part) in parts.iter().enumerate() {
        if part.eq_ignore_ascii_case("resourceGroups")
            || part.eq_ignore_ascii_case("resourcegroups")
        {
            return parts.get(i + 1).map(|s| s.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_resource_group() {
        let id = "/subscriptions/abc-123/resourceGroups/my-rg/providers/Microsoft.Search/searchServices/my-svc";
        assert_eq!(parse_resource_group(id), Some("my-rg".to_string()));
    }

    #[test]
    fn test_parse_resource_group_case_insensitive() {
        let id = "/subscriptions/abc/resourcegroups/MyRG/providers/Something";
        assert_eq!(parse_resource_group(id), Some("MyRG".to_string()));
    }

    #[test]
    fn test_parse_resource_group_missing() {
        let id = "/subscriptions/abc/providers/Something";
        assert_eq!(parse_resource_group(id), None);
    }

    #[test]
    fn test_ai_services_account_display() {
        let account = AiServicesAccount {
            name: "my-ai-service".to_string(),
            location: "eastus".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties::default(),
        };
        assert_eq!(format!("{}", account), "my-ai-service (eastus)");
    }

    #[test]
    fn test_agents_endpoint_from_arm_endpoint() {
        let account = AiServicesAccount {
            name: "irma-prod-foundry".to_string(),
            location: "swedencentral".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties {
                endpoint: Some("https://custom-subdomain.cognitiveservices.azure.com/".to_string()),
            },
        };
        assert_eq!(
            account.agents_endpoint(),
            "https://custom-subdomain.services.ai.azure.com"
        );
    }

    #[test]
    fn test_agents_endpoint_fallback_to_name() {
        let account = AiServicesAccount {
            name: "irma-prod-foundry".to_string(),
            location: "swedencentral".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties::default(),
        };
        assert_eq!(
            account.agents_endpoint(),
            "https://irma-prod-foundry.services.ai.azure.com"
        );
    }

    #[test]
    fn test_extract_subdomain() {
        assert_eq!(
            extract_subdomain("https://my-svc.cognitiveservices.azure.com/"),
            Some("my-svc")
        );
        assert_eq!(
            extract_subdomain("https://custom.services.ai.azure.com"),
            Some("custom")
        );
        assert_eq!(extract_subdomain("not-a-url"), None);
    }

    #[test]
    fn test_foundry_project_display_with_display_name() {
        let project = FoundryProject {
            name: "my-account/my-project".to_string(),
            location: "westus2".to_string(),
            id: String::new(),
            properties: FoundryProjectProperties {
                display_name: "my-project".to_string(),
            },
        };
        assert_eq!(format!("{}", project), "my-project (westus2)");
        assert_eq!(project.display_name(), "my-project");
    }

    #[test]
    fn test_model_deployment_display() {
        let deployment = ModelDeployment {
            name: "gpt-4o-mini".to_string(),
            properties: ModelDeploymentProperties {
                model: ModelDeploymentModel {
                    name: "gpt-4o-mini".to_string(),
                    version: "2024-07-18".to_string(),
                },
            },
            sku: ModelDeploymentSku {
                name: "GlobalStandard".to_string(),
                capacity: 1,
            },
        };
        assert_eq!(
            format!("{}", deployment),
            "gpt-4o-mini (gpt-4o-mini, GlobalStandard)"
        );
    }

    #[test]
    fn test_foundry_project_display_name_fallback() {
        let project = FoundryProject {
            name: "my-account/proj-default".to_string(),
            location: "swedencentral".to_string(),
            id: String::new(),
            properties: FoundryProjectProperties::default(),
        };
        assert_eq!(project.display_name(), "proj-default");
        assert_eq!(format!("{}", project), "proj-default (swedencentral)");
    }
}

#[cfg(test)]
mod provider_table_tests {
    use super::*;

    #[test]
    fn arm_base_url_can_be_overridden_for_tests() {
        assert_eq!(
            base_url_from(Some("http://127.0.0.1:1")),
            "http://127.0.0.1:1"
        );
        // trailing slash is trimmed
        assert_eq!(
            base_url_from(Some("http://127.0.0.1:1/")),
            "http://127.0.0.1:1"
        );
        // no override, or an empty one, falls back to the real ARM base URL
        assert_eq!(base_url_from(None), ARM_BASE_URL);
        assert_eq!(base_url_from(Some("")), ARM_BASE_URL);

        let c = ArmClient::with_token_and_base("t".into(), "http://127.0.0.1:1".into());
        assert!(
            c.url("/subscriptions", Provider::ResourcesArm)
                .starts_with("http://127.0.0.1:1/subscriptions?api-version=")
        );
    }

    #[test]
    fn arm_urls_come_from_the_provider_table() {
        use rigg_core::registry::{Provider, provider};
        let c = ArmClient::with_token("t".to_string());
        assert_eq!(
            c.url(
                "/subscriptions/s/providers/Microsoft.Search/searchServices",
                Provider::SearchArm
            ),
            format!(
                "https://management.azure.com/subscriptions/s/providers/Microsoft.Search/searchServices?api-version={}",
                provider(Provider::SearchArm).stable
            )
        );
        assert!(
            c.url("/subscriptions", Provider::ResourcesArm)
                .ends_with(&format!(
                    "?api-version={}",
                    rigg_core::registry::ARM_RESOURCES_API_VERSION
                ))
        );
    }

    #[test]
    fn provider_api_versions_url_uses_resources_arm_version() {
        let c = ArmClient::with_token("t".into());
        assert_eq!(
            c.url(
                "/subscriptions/s/providers/Microsoft.CognitiveServices",
                rigg_core::registry::Provider::ResourcesArm
            ),
            format!(
                "https://management.azure.com/subscriptions/s/providers/Microsoft.CognitiveServices?api-version={}",
                rigg_core::registry::ARM_RESOURCES_API_VERSION
            )
        );
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;

    #[test]
    fn deterministic_uuid_stable_and_shaped() {
        let a = deterministic_uuid("scope|principal|role");
        let b = deterministic_uuid("scope|principal|role");
        let c = deterministic_uuid("scope|principal|other-role");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 36);
        assert_eq!(a.chars().filter(|ch| *ch == '-').count(), 4);
    }

    #[test]
    fn resource_identity_principal_ids() {
        let id = ResourceIdentity {
            kind: "SystemAssigned, UserAssigned".into(),
            principal_id: Some("sys".into()),
            user_assigned: vec![("id1".into(), "ua1".into())],
        };
        assert_eq!(id.principal_ids(), vec!["sys", "ua1"]);
    }
}
