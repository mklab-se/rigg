//! Azure Resource Manager client for discovering Search and Foundry services

use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use crate::auth::AzCliAuth;
use crate::error::ClientError;

const ARM_BASE_URL: &str = "https://management.azure.com";

/// Azure Resource Manager client for subscription/service discovery
pub struct ArmClient {
    http: Client,
    token: String,
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
        if let Some(ref endpoint) = self.properties.endpoint {
            if let Some(subdomain) = extract_subdomain(endpoint) {
                return format!("https://{}.services.ai.azure.com", subdomain);
            }
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

/// Storage account key
#[derive(Debug, Clone, Deserialize)]
struct StorageKey {
    value: String,
}

/// Storage account key list response
#[derive(Debug, Deserialize)]
struct StorageKeyList {
    keys: Vec<StorageKey>,
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
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// ARM list response envelope
#[derive(Debug, Deserialize)]
struct ArmListResponse<T> {
    value: Vec<T>,
}

impl ArmClient {
    /// Create a new ARM client using Azure CLI credentials
    pub fn new() -> Result<Self, ClientError> {
        let token = AzCliAuth::get_arm_token()?;
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self { http, token })
    }

    /// Read a resource's managed identity block: `GET {id}?api-version=...`.
    pub async fn get_resource_identity(
        &self,
        resource_id: &str,
        api_version: &str,
    ) -> Result<Option<ResourceIdentity>, ClientError> {
        let url = format!("{ARM_BASE_URL}{resource_id}?api-version={api_version}");
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
            "{ARM_BASE_URL}{scope}/providers/Microsoft.Authorization/roleAssignments?api-version=2022-04-01&$filter=principalId%20eq%20'{principal_id}'"
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
        let assignment_name = deterministic_uuid(&format!(
            "{scope}|{principal_id}|{role_definition_guid}"
        ));
        let url = format!(
            "{ARM_BASE_URL}{scope}/providers/Microsoft.Authorization/roleAssignments/{assignment_name}?api-version=2022-04-01"
        );
        let sub = scope
            .split('/')
            .nth(2)
            .unwrap_or_default();
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
        let url = format!("{}/subscriptions?api-version=2022-12-01", ARM_BASE_URL);
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
        let url = format!(
            "{}/subscriptions/{}/providers/Microsoft.Search/searchServices?api-version=2023-11-01",
            ARM_BASE_URL, subscription_id
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
        let url = format!(
            "{}/subscriptions/{}/providers/Microsoft.CognitiveServices/accounts?api-version=2024-10-01",
            ARM_BASE_URL, subscription_id
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

        let url = format!(
            "{}/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}/projects?api-version=2025-06-01",
            ARM_BASE_URL, subscription_id, resource_group, account.name
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
        let url = format!(
            "{}/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Storage/storageAccounts?api-version=2023-05-01",
            ARM_BASE_URL, subscription_id, resource_group
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

    /// Get the primary access key for a storage account.
    pub async fn get_storage_account_key(
        &self,
        subscription_id: &str,
        resource_group: &str,
        account_name: &str,
    ) -> Result<String, ClientError> {
        let url = format!(
            "{}/subscriptions/{}/resourceGroups/{}/providers/Microsoft.Storage/storageAccounts/{}/listKeys?api-version=2023-05-01",
            ARM_BASE_URL, subscription_id, resource_group, account_name
        );
        debug!("Getting storage account keys: {}", url);

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

        let key_list: StorageKeyList = response.json().await?;
        key_list
            .keys
            .into_iter()
            .next()
            .map(|k| k.value)
            .ok_or_else(|| ClientError::Api {
                status: 0,
                message: "No keys found for storage account".to_string(),
            })
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

        let url = format!(
            "{}/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}/deployments?api-version=2024-10-01",
            ARM_BASE_URL, subscription_id, resource_group, account.name
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

        let url = format!(
            "{}/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}/deployments/{}?api-version=2024-10-01",
            ARM_BASE_URL, subscription_id, resource_group, account.name, deployment_name
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

    /// Build a full connection string for a storage account.
    pub async fn get_storage_connection_string(
        &self,
        subscription_id: &str,
        resource_group: &str,
        account_name: &str,
    ) -> Result<String, ClientError> {
        let key = self
            .get_storage_account_key(subscription_id, resource_group, account_name)
            .await?;

        Ok(format!(
            "DefaultEndpointsProtocol=https;AccountName={};AccountKey={};EndpointSuffix=core.windows.net",
            account_name, key
        ))
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
