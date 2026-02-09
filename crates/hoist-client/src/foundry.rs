//! Microsoft Foundry REST API client
//!
//! Manages Foundry agents via the project-scoped `/agents` API
//! (new Foundry experience, API version `2025-05-15-preview`).

use std::time::Duration;

use reqwest::{Client, Method, StatusCode};
use serde_json::{Map, Value};
use tracing::{debug, instrument, warn};

use hoist_core::config::FoundryServiceConfig;

use crate::auth::{get_auth_provider_for, AuthProvider};
use crate::error::ClientError;

/// Maximum number of retry attempts for retryable errors
const MAX_RETRIES: u32 = 3;

/// Initial backoff delay in seconds
const INITIAL_BACKOFF_SECS: u64 = 1;

/// Microsoft Foundry API client
pub struct FoundryClient {
    http: Client,
    auth: Box<dyn AuthProvider>,
    base_url: String,
    project: String,
    api_version: String,
}

impl FoundryClient {
    /// Create a new Foundry client from service configuration
    pub fn new(config: &FoundryServiceConfig) -> Result<Self, ClientError> {
        let auth = get_auth_provider_for(hoist_core::ServiceDomain::Foundry)?;
        let http = Client::builder().timeout(Duration::from_secs(30)).build()?;

        Ok(Self {
            http,
            auth,
            base_url: config.service_url(),
            project: config.project.clone(),
            api_version: config.api_version.clone(),
        })
    }

    /// Create with a custom auth provider (for testing)
    pub fn with_auth(
        base_url: String,
        project: String,
        api_version: String,
        auth: Box<dyn AuthProvider>,
    ) -> Result<Self, ClientError> {
        let http = Client::builder().timeout(Duration::from_secs(30)).build()?;

        Ok(Self {
            http,
            auth,
            base_url,
            project,
            api_version,
        })
    }

    /// Build URL for the agents collection
    fn agents_url(&self) -> String {
        format!(
            "{}/api/projects/{}/agents?api-version={}",
            self.base_url, self.project, self.api_version
        )
    }

    /// Build URL for a specific agent
    fn agent_url(&self, id: &str) -> String {
        format!(
            "{}/api/projects/{}/agents/{}?api-version={}",
            self.base_url, self.project, id, self.api_version
        )
    }

    /// Build URL for creating/updating agent versions
    fn agent_versions_url(&self, name: &str) -> String {
        format!(
            "{}/api/projects/{}/agents/{}/versions?api-version={}",
            self.base_url, self.project, name, self.api_version
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
                    kind: "agent".to_string(),
                    name: url.to_string(),
                }),
                StatusCode::TOO_MANY_REQUESTS => {
                    let retry_after = 60;
                    Err(ClientError::RateLimited { retry_after })
                }
                StatusCode::SERVICE_UNAVAILABLE => Err(ClientError::ServiceUnavailable(body)),
                _ => Err(ClientError::from_response(status.as_u16(), &body)),
            }
        }
    }

    /// Execute an HTTP request with retry logic
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
                    let delay = match &err {
                        ClientError::RateLimited { retry_after } => {
                            Duration::from_secs(*retry_after)
                        }
                        _ => Duration::from_secs(INITIAL_BACKOFF_SECS * 2u64.pow(attempt)),
                    };
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

    /// List all agents in the project
    #[instrument(skip(self))]
    pub async fn list_agents(&self) -> Result<Vec<Value>, ClientError> {
        let url = self.agents_url();
        let response = self.request_with_retry(Method::GET, &url, None).await?;

        match response {
            Some(value) => {
                let items = value
                    .get("data")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                // Flatten versioned response into flat agent objects
                Ok(items.iter().map(flatten_agent_response).collect())
            }
            None => Ok(Vec::new()),
        }
    }

    /// Get a specific agent by ID
    #[instrument(skip(self))]
    pub async fn get_agent(&self, id: &str) -> Result<Value, ClientError> {
        let url = self.agent_url(id);
        let response = self.request_with_retry(Method::GET, &url, None).await?;

        let raw = response.ok_or_else(|| ClientError::NotFound {
            kind: "Agent".to_string(),
            name: id.to_string(),
        })?;
        Ok(flatten_agent_response(&raw))
    }

    /// Create a new agent (creates first version)
    ///
    /// Takes a flat agent definition and wraps it in the API format
    /// before posting to `/agents/{name}/versions`.
    #[instrument(skip(self, definition))]
    pub async fn create_agent(&self, definition: &Value) -> Result<Value, ClientError> {
        let name = definition
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| ClientError::Api {
                status: 400,
                message: "Agent definition missing 'name' field".to_string(),
            })?;
        let payload = wrap_agent_payload(definition);
        let url = self.agent_versions_url(name);
        let response = self
            .request_with_retry(Method::POST, &url, Some(&payload))
            .await?;

        let raw = response.ok_or_else(|| ClientError::Api {
            status: 500,
            message: "No response body from agent creation".to_string(),
        })?;
        Ok(flatten_agent_response(&raw))
    }

    /// Update an existing agent (creates new version)
    ///
    /// Takes a flat agent definition and wraps it in the API format
    /// before posting to `/agents/{name}/versions`.
    #[instrument(skip(self, definition))]
    pub async fn update_agent(&self, id: &str, definition: &Value) -> Result<Value, ClientError> {
        let payload = wrap_agent_payload(definition);
        let url = self.agent_versions_url(id);
        let response = self
            .request_with_retry(Method::POST, &url, Some(&payload))
            .await?;

        let raw = response.ok_or_else(|| ClientError::Api {
            status: 500,
            message: "No response body from agent update".to_string(),
        })?;
        Ok(flatten_agent_response(&raw))
    }

    /// Delete an agent
    #[instrument(skip(self))]
    pub async fn delete_agent(&self, id: &str) -> Result<(), ClientError> {
        let url = self.agent_url(id);
        self.request_with_retry(Method::DELETE, &url, None).await?;
        Ok(())
    }

    /// Get the authentication method being used
    pub fn auth_method(&self) -> &'static str {
        self.auth.method_name()
    }
}

/// Wrap a flat agent definition into the API request format.
///
/// Converts from flat: `{ "name", "model", "instructions", "tools", ... }`
/// To API format:
/// ```json
/// {
///   "metadata": {...},
///   "description": "...",
///   "definition": {
///     "kind": "prompt",
///     "model": "...",
///     "instructions": "...",
///     "tools": [...]
///   }
/// }
/// ```
fn wrap_agent_payload(flat: &Value) -> Value {
    let obj = match flat.as_object() {
        Some(o) => o,
        None => return flat.clone(),
    };

    // Fields that go at the version level (outside definition)
    const VERSION_LEVEL_FIELDS: &[&str] = &["metadata", "description"];

    // Fields that are response-only and should not be sent
    const EXCLUDED_FIELDS: &[&str] = &["id", "name", "version", "created_at", "object"];

    let mut wrapper = Map::new();
    let mut definition = Map::new();

    for (key, value) in obj {
        if EXCLUDED_FIELDS.contains(&key.as_str()) {
            continue;
        } else if VERSION_LEVEL_FIELDS.contains(&key.as_str()) {
            wrapper.insert(key.clone(), value.clone());
        } else {
            definition.insert(key.clone(), value.clone());
        }
    }

    // Ensure kind is set (default to "prompt")
    if !definition.contains_key("kind") {
        definition.insert("kind".to_string(), Value::String("prompt".to_string()));
    }

    wrapper.insert("definition".to_string(), Value::Object(definition));
    Value::Object(wrapper)
}

/// Flatten a new Foundry agents API response into a flat structure
/// compatible with the agent decomposition pipeline.
///
/// The new Foundry API returns a versioned structure:
/// ```json
/// {
///   "object": "agent",
///   "id": "MyAgent",
///   "name": "MyAgent",
///   "versions": {
///     "latest": {
///       "metadata": {...},
///       "version": "5",
///       "definition": {
///         "kind": "prompt",
///         "model": "gpt-5.2-chat",
///         "instructions": "...",
///         "tools": [...]
///       }
///     }
///   }
/// }
/// ```
///
/// This flattens to: `{ "id", "name", "model", "instructions", "tools", ... }`
fn flatten_agent_response(agent: &Value) -> Value {
    let obj = match agent.as_object() {
        Some(o) => o,
        None => return agent.clone(),
    };

    let mut flat = Map::new();

    // Top-level fields
    if let Some(id) = obj.get("id") {
        flat.insert("id".to_string(), id.clone());
    }
    if let Some(name) = obj.get("name") {
        flat.insert("name".to_string(), name.clone());
    }

    // Extract from versions.latest
    if let Some(latest) = obj
        .get("versions")
        .and_then(|v| v.get("latest"))
        .and_then(|l| l.as_object())
    {
        // Version-level fields
        if let Some(metadata) = latest.get("metadata") {
            flat.insert("metadata".to_string(), metadata.clone());
        }
        if let Some(description) = latest.get("description") {
            flat.insert("description".to_string(), description.clone());
        }
        if let Some(version) = latest.get("version") {
            flat.insert("version".to_string(), version.clone());
        }
        if let Some(created_at) = latest.get("created_at") {
            flat.insert("created_at".to_string(), created_at.clone());
        }

        // Definition-level fields (model, instructions, tools, kind, etc.)
        if let Some(definition) = latest.get("definition").and_then(|d| d.as_object()) {
            for (key, value) in definition {
                flat.insert(key.clone(), value.clone());
            }
        }
    }

    Value::Object(flat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{AuthError, AuthProvider};
    use serde_json::json;

    struct FakeAuth;
    impl AuthProvider for FakeAuth {
        fn get_token(&self) -> Result<String, AuthError> {
            Ok("fake-token".to_string())
        }
        fn method_name(&self) -> &'static str {
            "Fake"
        }
    }

    fn make_client() -> FoundryClient {
        FoundryClient::with_auth(
            "https://my-ai-svc.services.ai.azure.com".to_string(),
            "my-project".to_string(),
            "2025-05-15-preview".to_string(),
            Box::new(FakeAuth),
        )
        .unwrap()
    }

    #[test]
    fn test_agents_url() {
        let client = make_client();
        let url = client.agents_url();
        assert_eq!(
            url,
            "https://my-ai-svc.services.ai.azure.com/api/projects/my-project/agents?api-version=2025-05-15-preview"
        );
    }

    #[test]
    fn test_agent_url() {
        let client = make_client();
        let url = client.agent_url("Regulus");
        assert_eq!(
            url,
            "https://my-ai-svc.services.ai.azure.com/api/projects/my-project/agents/Regulus?api-version=2025-05-15-preview"
        );
    }

    #[test]
    fn test_agent_versions_url() {
        let client = make_client();
        let url = client.agent_versions_url("KITT");
        assert_eq!(
            url,
            "https://my-ai-svc.services.ai.azure.com/api/projects/my-project/agents/KITT/versions?api-version=2025-05-15-preview"
        );
    }

    #[test]
    fn test_auth_method() {
        let client = make_client();
        assert_eq!(client.auth_method(), "Fake");
    }

    #[test]
    fn test_wrap_agent_payload() {
        let flat = json!({
            "id": "KITT",
            "name": "KITT",
            "model": "gpt-5.2-chat",
            "kind": "prompt",
            "instructions": "You are KITT.",
            "tools": [{"type": "code_interpreter"}],
            "metadata": {"logo": "kitt.svg"},
            "description": "A smart car",
            "version": "3",
            "created_at": 1234567890
        });

        let wrapped = wrap_agent_payload(&flat);
        let obj = wrapped.as_object().unwrap();

        // Top level: metadata, description, definition
        assert!(obj.contains_key("definition"));
        assert!(obj.contains_key("metadata"));
        assert!(obj.contains_key("description"));

        // Excluded from payload
        assert!(!obj.contains_key("id"));
        assert!(!obj.contains_key("name"));
        assert!(!obj.contains_key("version"));
        assert!(!obj.contains_key("created_at"));

        // Definition should contain model, instructions, tools, kind
        let def = obj.get("definition").unwrap().as_object().unwrap();
        assert_eq!(def.get("model").unwrap(), "gpt-5.2-chat");
        assert_eq!(def.get("kind").unwrap(), "prompt");
        assert_eq!(def.get("instructions").unwrap(), "You are KITT.");
        assert!(def.get("tools").unwrap().as_array().unwrap().len() == 1);

        // Definition should NOT contain excluded or version-level fields
        assert!(!def.contains_key("id"));
        assert!(!def.contains_key("name"));
        assert!(!def.contains_key("metadata"));
    }

    #[test]
    fn test_wrap_agent_payload_adds_default_kind() {
        let flat = json!({
            "name": "simple",
            "model": "gpt-4o",
            "instructions": "Be helpful."
        });

        let wrapped = wrap_agent_payload(&flat);
        let def = wrapped.get("definition").unwrap().as_object().unwrap();
        assert_eq!(def.get("kind").unwrap(), "prompt");
    }

    #[test]
    fn test_flatten_then_wrap_roundtrip() {
        let api_response = json!({
            "object": "agent",
            "id": "KITT",
            "name": "KITT",
            "versions": {
                "latest": {
                    "metadata": {"logo": "kitt.svg"},
                    "version": "3",
                    "description": "Smart car",
                    "created_at": 1234567890,
                    "definition": {
                        "kind": "prompt",
                        "model": "gpt-5.2-chat",
                        "instructions": "You are KITT.",
                        "tools": [{"type": "code_interpreter"}]
                    }
                }
            }
        });

        let flat = flatten_agent_response(&api_response);
        let wrapped = wrap_agent_payload(&flat);

        // The wrapped payload should have a definition with the same content
        let def = wrapped.get("definition").unwrap().as_object().unwrap();
        assert_eq!(def.get("model").unwrap(), "gpt-5.2-chat");
        assert_eq!(def.get("instructions").unwrap(), "You are KITT.");
        assert_eq!(def.get("kind").unwrap(), "prompt");
    }

    #[test]
    fn test_flatten_agent_response_full() {
        let api_response = json!({
            "object": "agent",
            "id": "Regulus",
            "name": "Regulus",
            "versions": {
                "latest": {
                    "metadata": {
                        "logo": "Avatar_Default.svg",
                        "description": "",
                        "modified_at": "1769974547"
                    },
                    "object": "agent.version",
                    "id": "Regulus:5",
                    "name": "Regulus",
                    "version": "5",
                    "description": "",
                    "created_at": 1769974549,
                    "definition": {
                        "kind": "prompt",
                        "model": "gpt-5.2-chat",
                        "instructions": "You are Regulus.",
                        "tools": [
                            {"type": "mcp", "server_label": "kb_test"}
                        ]
                    }
                }
            }
        });

        let flat = flatten_agent_response(&api_response);
        let obj = flat.as_object().unwrap();

        assert_eq!(obj.get("id").unwrap(), "Regulus");
        assert_eq!(obj.get("name").unwrap(), "Regulus");
        assert_eq!(obj.get("model").unwrap(), "gpt-5.2-chat");
        assert_eq!(obj.get("kind").unwrap(), "prompt");
        assert_eq!(obj.get("instructions").unwrap(), "You are Regulus.");
        assert_eq!(obj.get("version").unwrap(), "5");
        assert_eq!(obj.get("description").unwrap(), "");
        assert!(obj.get("metadata").is_some());
        assert!(obj.get("tools").unwrap().as_array().unwrap().len() == 1);

        // Should NOT have the nested versions structure
        assert!(!obj.contains_key("versions"));
        assert!(!obj.contains_key("object"));
    }

    #[test]
    fn test_flatten_agent_response_minimal() {
        let api_response = json!({
            "object": "agent",
            "id": "simple",
            "name": "simple"
        });

        let flat = flatten_agent_response(&api_response);
        let obj = flat.as_object().unwrap();

        assert_eq!(obj.get("id").unwrap(), "simple");
        assert_eq!(obj.get("name").unwrap(), "simple");
        assert!(!obj.contains_key("model"));
    }

    #[test]
    fn test_flatten_agent_response_non_object() {
        let flat = flatten_agent_response(&json!("not an object"));
        assert_eq!(flat, json!("not an object"));
    }
}
