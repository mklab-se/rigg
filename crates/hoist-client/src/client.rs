//! Azure Search REST API client

use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use tracing::{debug, instrument};

use hoist_core::resources::ResourceKind;
use hoist_core::Config;

use crate::auth::{get_auth_provider, AuthProvider};
use crate::error::ClientError;

/// Azure Search API client
pub struct AzureSearchClient {
    http: Client,
    auth: Box<dyn AuthProvider>,
    base_url: String,
    api_version: String,
    preview_api_version: String,
}

impl AzureSearchClient {
    /// Create a new client from configuration
    pub fn new(config: &Config) -> Result<Self, ClientError> {
        let auth = get_auth_provider()?;
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            http,
            auth,
            base_url: config.service_url(),
            api_version: config.service.api_version.clone(),
            preview_api_version: config.service.preview_api_version.clone(),
        })
    }

    /// Create a client pointing to a different server, using the same auth and API versions
    pub fn new_for_server(config: &Config, server_name: &str) -> Result<Self, ClientError> {
        let auth = get_auth_provider()?;
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            http,
            auth,
            base_url: format!("https://{}.search.windows.net", server_name),
            api_version: config.service.api_version.clone(),
            preview_api_version: config.service.preview_api_version.clone(),
        })
    }

    /// Create with a custom auth provider (for testing)
    pub fn with_auth(
        base_url: String,
        api_version: String,
        preview_api_version: String,
        auth: Box<dyn AuthProvider>,
    ) -> Result<Self, ClientError> {
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;

        Ok(Self {
            http,
            auth,
            base_url,
            api_version,
            preview_api_version,
        })
    }

    /// Get the API version to use for a resource kind
    fn api_version_for(&self, kind: ResourceKind) -> &str {
        if kind.is_preview() {
            &self.preview_api_version
        } else {
            &self.api_version
        }
    }

    /// Build URL for a resource collection
    fn collection_url(&self, kind: ResourceKind) -> String {
        format!(
            "{}/{}?api-version={}",
            self.base_url,
            kind.api_path(),
            self.api_version_for(kind)
        )
    }

    /// Build URL for a specific resource
    fn resource_url(&self, kind: ResourceKind, name: &str) -> String {
        format!(
            "{}/{}/{}?api-version={}",
            self.base_url,
            kind.api_path(),
            name,
            self.api_version_for(kind)
        )
    }

    /// Execute an HTTP request
    async fn request(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>, ClientError> {
        let token = self.auth.get_token()?;

        let mut request = self
            .http
            .request(method.clone(), url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json");

        if let Some(json) = body {
            request = request.json(json);
        }

        debug!("Request: {} {}", method, url);
        let response = request.send().await?;
        let status = response.status();

        if status == StatusCode::NO_CONTENT {
            return Ok(None);
        }

        let body = response.text().await?;

        if status.is_success() {
            if body.is_empty() {
                Ok(None)
            } else {
                let value: Value = serde_json::from_str(&body)?;
                Ok(Some(value))
            }
        } else {
            match status {
                StatusCode::NOT_FOUND => Err(ClientError::NotFound {
                    kind: "resource".to_string(),
                    name: url.to_string(),
                }),
                StatusCode::CONFLICT => Err(ClientError::AlreadyExists {
                    kind: "resource".to_string(),
                    name: url.to_string(),
                }),
                StatusCode::TOO_MANY_REQUESTS => {
                    let retry_after = 60; // Default retry time
                    Err(ClientError::RateLimited { retry_after })
                }
                StatusCode::SERVICE_UNAVAILABLE => Err(ClientError::ServiceUnavailable(body)),
                _ => Err(ClientError::from_response(status.as_u16(), &body)),
            }
        }
    }

    /// List all resources of a given kind
    #[instrument(skip(self))]
    pub async fn list(&self, kind: ResourceKind) -> Result<Vec<Value>, ClientError> {
        let url = self.collection_url(kind);
        let response = self.request(Method::GET, &url, None).await?;

        match response {
            Some(value) => {
                // Azure returns { "value": [...] }
                let items = value
                    .get("value")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                Ok(items)
            }
            None => Ok(Vec::new()),
        }
    }

    /// Get a specific resource
    #[instrument(skip(self))]
    pub async fn get(&self, kind: ResourceKind, name: &str) -> Result<Value, ClientError> {
        let url = self.resource_url(kind, name);
        let response = self.request(Method::GET, &url, None).await?;

        response.ok_or_else(|| ClientError::NotFound {
            kind: kind.display_name().to_string(),
            name: name.to_string(),
        })
    }

    /// Create or update a resource
    ///
    /// Returns the response body if the API returns one. Some APIs (especially
    /// preview endpoints like Knowledge Sources) return 204 No Content on
    /// successful update, which yields `Ok(None)`.
    #[instrument(skip(self, definition))]
    pub async fn create_or_update(
        &self,
        kind: ResourceKind,
        name: &str,
        definition: &Value,
    ) -> Result<Option<Value>, ClientError> {
        let url = self.resource_url(kind, name);
        self.request(Method::PUT, &url, Some(definition)).await
    }

    /// Delete a resource
    #[instrument(skip(self))]
    pub async fn delete(&self, kind: ResourceKind, name: &str) -> Result<(), ClientError> {
        let url = self.resource_url(kind, name);
        self.request(Method::DELETE, &url, None).await?;
        Ok(())
    }

    /// Check if a resource exists
    pub async fn exists(&self, kind: ResourceKind, name: &str) -> Result<bool, ClientError> {
        match self.get(kind, name).await {
            Ok(_) => Ok(true),
            Err(ClientError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Get the authentication method being used
    pub fn auth_method(&self) -> &'static str {
        self.auth.method_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, AuthProvider};

    struct FakeAuth;
    impl AuthProvider for FakeAuth {
        fn get_token(&self) -> Result<String, AuthError> {
            Ok("fake-token".to_string())
        }
        fn method_name(&self) -> &'static str {
            "Fake"
        }
    }

    fn make_client() -> AzureSearchClient {
        AzureSearchClient::with_auth(
            "https://test-svc.search.windows.net".to_string(),
            "2024-07-01".to_string(),
            "2025-11-01-preview".to_string(),
            Box::new(FakeAuth),
        )
        .unwrap()
    }

    #[test]
    fn test_collection_url_stable_resource() {
        let client = make_client();
        let url = client.collection_url(ResourceKind::Index);
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/indexes?api-version=2024-07-01"
        );
    }

    #[test]
    fn test_collection_url_preview_resource_uses_preview_version() {
        let client = make_client();
        let url = client.collection_url(ResourceKind::KnowledgeBase);
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/knowledgebases?api-version=2025-11-01-preview"
        );
    }

    #[test]
    fn test_collection_url_knowledge_source_uses_preview_version() {
        let client = make_client();
        let url = client.collection_url(ResourceKind::KnowledgeSource);
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/knowledgesources?api-version=2025-11-01-preview"
        );
    }

    #[test]
    fn test_resource_url_stable() {
        let client = make_client();
        let url = client.resource_url(ResourceKind::Index, "my-index");
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/indexes/my-index?api-version=2024-07-01"
        );
    }

    #[test]
    fn test_resource_url_preview() {
        let client = make_client();
        let url = client.resource_url(ResourceKind::KnowledgeBase, "my-kb");
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/knowledgebases/my-kb?api-version=2025-11-01-preview"
        );
    }

    #[test]
    fn test_all_stable_kinds_use_stable_version() {
        let client = make_client();
        for kind in ResourceKind::stable() {
            let url = client.collection_url(*kind);
            assert!(
                url.contains("2024-07-01"),
                "{:?} should use stable API version, got: {}",
                kind,
                url
            );
        }
    }

    #[test]
    fn test_new_for_server_produces_correct_base_url() {
        // We can't easily test new_for_server directly since it calls get_auth_provider,
        // but we can verify the URL format through with_auth
        let client = AzureSearchClient::with_auth(
            "https://other-svc.search.windows.net".to_string(),
            "2024-07-01".to_string(),
            "2025-11-01-preview".to_string(),
            Box::new(FakeAuth),
        )
        .unwrap();
        let url = client.collection_url(ResourceKind::Index);
        assert_eq!(
            url,
            "https://other-svc.search.windows.net/indexes?api-version=2024-07-01"
        );
    }

    #[test]
    fn test_all_preview_kinds_use_preview_version() {
        let client = make_client();
        for kind in ResourceKind::all() {
            if kind.is_preview() {
                let url = client.collection_url(*kind);
                assert!(
                    url.contains("2025-11-01-preview"),
                    "{:?} should use preview API version, got: {}",
                    kind,
                    url
                );
            }
        }
    }
}
