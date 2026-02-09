//! Azure Search REST API client

use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use serde_json::Value;
use tracing::{debug, instrument, warn};

use hoist_core::resources::ResourceKind;
use hoist_core::Config;

use crate::auth::{get_auth_provider, AuthProvider};
use crate::error::ClientError;

/// Maximum number of retry attempts for retryable errors
const MAX_RETRIES: u32 = 3;

/// Initial backoff delay in seconds
const INITIAL_BACKOFF_SECS: u64 = 1;

/// Calculate the backoff duration for a given retry attempt.
///
/// For `RateLimited` errors with a `retry_after` value, that value is used directly.
/// For other retryable errors, exponential backoff is applied: 1s, 2s, 4s, etc.
fn retry_delay(error: &ClientError, attempt: u32) -> Duration {
    match error {
        ClientError::RateLimited { retry_after } => Duration::from_secs(*retry_after),
        _ => Duration::from_secs(INITIAL_BACKOFF_SECS * 2u64.pow(attempt)),
    }
}

/// Azure Search API client
pub struct AzureSearchClient {
    http: Client,
    auth: Box<dyn AuthProvider>,
    base_url: String,
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
            preview_api_version: config.api_version_for(true).to_string(),
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
            preview_api_version: config.api_version_for(true).to_string(),
        })
    }

    /// Create with a custom auth provider (for testing)
    pub fn with_auth(
        base_url: String,
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
            preview_api_version,
        })
    }

    /// Get the API version to use for a resource kind.
    /// Always uses the preview API version — it is a superset of the stable API
    /// and avoids failures when stable resources contain preview-only features
    /// (e.g. a skillset with ChatCompletionSkill).
    fn api_version_for(&self, _kind: ResourceKind) -> &str {
        &self.preview_api_version
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
                _ => Err(ClientError::from_response_with_url(
                    status.as_u16(),
                    &body,
                    Some(url),
                )),
            }
        }
    }

    /// Execute an HTTP request with retry logic for transient errors.
    ///
    /// Retries up to [`MAX_RETRIES`] times for retryable errors (429 and 503).
    /// Uses exponential backoff (1s, 2s, 4s) for 503 errors and respects the
    /// `retry_after` value for 429 rate-limiting errors.
    async fn request_with_retry(
        &self,
        method: Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>, ClientError> {
        let mut attempt = 0u32;
        loop {
            match self.request(method.clone(), url, body).await {
                Ok(value) => return Ok(value),
                Err(err) if err.is_retryable() && attempt < MAX_RETRIES => {
                    let delay = retry_delay(&err, attempt);
                    warn!(
                        "Request {} {} failed (attempt {}/{}): {}. Retrying in {:?}",
                        method,
                        url,
                        attempt + 1,
                        MAX_RETRIES + 1,
                        err,
                        delay,
                    );
                    tokio::time::sleep(delay).await;
                    attempt += 1;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// List all resources of a given kind
    #[instrument(skip(self))]
    pub async fn list(&self, kind: ResourceKind) -> Result<Vec<Value>, ClientError> {
        let url = self.collection_url(kind);
        let response = self.request_with_retry(Method::GET, &url, None).await?;

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
        let response = self.request_with_retry(Method::GET, &url, None).await?;

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
        self.request_with_retry(Method::PUT, &url, Some(definition))
            .await
    }

    /// Delete a resource
    #[instrument(skip(self))]
    pub async fn delete(&self, kind: ResourceKind, name: &str) -> Result<(), ClientError> {
        let url = self.resource_url(kind, name);
        self.request_with_retry(Method::DELETE, &url, None).await?;
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
            "2025-11-01-preview".to_string(),
            Box::new(FakeAuth),
        )
        .unwrap()
    }

    #[test]
    fn test_collection_url_uses_preview_version() {
        let client = make_client();
        let url = client.collection_url(ResourceKind::Index);
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/indexes?api-version=2025-11-01-preview"
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
    fn test_resource_url_uses_preview_version() {
        let client = make_client();
        let url = client.resource_url(ResourceKind::Index, "my-index");
        assert_eq!(
            url,
            "https://test-svc.search.windows.net/indexes/my-index?api-version=2025-11-01-preview"
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
    fn test_new_for_server_produces_correct_base_url() {
        // We can't easily test new_for_server directly since it calls get_auth_provider,
        // but we can verify the URL format through with_auth
        let client = AzureSearchClient::with_auth(
            "https://other-svc.search.windows.net".to_string(),
            "2025-11-01-preview".to_string(),
            Box::new(FakeAuth),
        )
        .unwrap();
        let url = client.collection_url(ResourceKind::Index);
        assert_eq!(
            url,
            "https://other-svc.search.windows.net/indexes?api-version=2025-11-01-preview"
        );
    }

    #[test]
    fn test_all_kinds_use_preview_version() {
        let client = make_client();
        for kind in ResourceKind::all() {
            let url = client.collection_url(*kind);
            assert!(
                url.contains("2025-11-01-preview"),
                "{:?} should use preview API version, got: {}",
                kind,
                url
            );
        }
    }

    #[test]
    fn test_retry_delay_exponential_backoff_attempt_0() {
        let err = ClientError::ServiceUnavailable("down".to_string());
        let delay = retry_delay(&err, 0);
        assert_eq!(delay, Duration::from_secs(1));
    }

    #[test]
    fn test_retry_delay_exponential_backoff_attempt_1() {
        let err = ClientError::ServiceUnavailable("down".to_string());
        let delay = retry_delay(&err, 1);
        assert_eq!(delay, Duration::from_secs(2));
    }

    #[test]
    fn test_retry_delay_exponential_backoff_attempt_2() {
        let err = ClientError::ServiceUnavailable("down".to_string());
        let delay = retry_delay(&err, 2);
        assert_eq!(delay, Duration::from_secs(4));
    }

    #[test]
    fn test_retry_delay_rate_limited_uses_retry_after() {
        let err = ClientError::RateLimited { retry_after: 30 };
        // retry_after should be used regardless of attempt number
        assert_eq!(retry_delay(&err, 0), Duration::from_secs(30));
        assert_eq!(retry_delay(&err, 1), Duration::from_secs(30));
        assert_eq!(retry_delay(&err, 2), Duration::from_secs(30));
    }

    #[test]
    fn test_retry_delay_rate_limited_default_retry_after() {
        let err = ClientError::RateLimited { retry_after: 60 };
        let delay = retry_delay(&err, 0);
        assert_eq!(delay, Duration::from_secs(60));
    }

    #[test]
    fn test_retry_constants() {
        assert_eq!(MAX_RETRIES, 3);
        assert_eq!(INITIAL_BACKOFF_SECS, 1);
    }

    #[test]
    fn test_retry_delay_backoff_sequence() {
        let err = ClientError::ServiceUnavailable("temporarily unavailable".to_string());
        let delays: Vec<Duration> = (0..MAX_RETRIES).map(|i| retry_delay(&err, i)).collect();
        assert_eq!(
            delays,
            vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
            ]
        );
    }

    #[test]
    fn test_non_retryable_error_still_computes_delay() {
        // retry_delay computes a delay regardless; the caller decides whether to retry.
        // This verifies the function doesn't panic on non-retryable errors.
        let err = ClientError::Api {
            status: 400,
            message: "bad request".to_string(),
        };
        let delay = retry_delay(&err, 0);
        assert_eq!(delay, Duration::from_secs(1));
    }
}
