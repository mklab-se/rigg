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
        };
        assert_eq!(format!("{}", account), "my-ai-service (eastus)");
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
