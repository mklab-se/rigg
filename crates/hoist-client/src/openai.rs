//! Azure OpenAI client for AI-enhanced features

use reqwest::Client;
use tracing::debug;

use crate::auth::{AuthProvider, get_cognitive_services_auth};
use crate::error::ClientError;
use hoist_core::config::AiConfig;

/// Azure OpenAI client for chat completions
pub struct AzureOpenAIClient {
    http: Client,
    auth: Box<dyn AuthProvider>,
    endpoint: String,
    deployment: String,
    api_version: String,
}

impl AzureOpenAIClient {
    /// Create from an AiConfig section
    pub fn from_config(config: &AiConfig) -> Result<Self, ClientError> {
        let auth = get_cognitive_services_auth()?;
        let http = Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;

        Ok(Self {
            http,
            auth,
            endpoint: config.openai_endpoint(),
            deployment: config.deployment.clone(),
            api_version: config.api_version.clone(),
        })
    }

    /// Send a chat completion request and return the response text.
    pub async fn chat_completion(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        temperature: f32,
    ) -> Result<String, ClientError> {
        let token = self.auth.get_token()?;

        let url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            self.endpoint, self.deployment, self.api_version
        );
        debug!("Azure OpenAI chat completion: {}", url);

        let body = serde_json::json!({
            "messages": [
                { "role": "system", "content": system_prompt },
                { "role": "user", "content": user_prompt },
            ],
            "temperature": temperature,
            "max_tokens": 500,
        });

        let response = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await?;
            return Err(ClientError::from_response(status.as_u16(), &body));
        }

        let json: serde_json::Value = response.json().await?;
        let content = json
            .get("choices")
            .and_then(|c| c.get(0))
            .and_then(|c| c.get("message"))
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        Ok(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_openai_endpoint_from_config() {
        let config = AiConfig {
            account: "my-ai-services".to_string(),
            deployment: "gpt-4o-mini".to_string(),
            endpoint: None,
            subscription: None,
            resource_group: None,
            api_version: "2024-12-01-preview".to_string(),
        };
        assert_eq!(
            config.openai_endpoint(),
            "https://my-ai-services.openai.azure.com"
        );
    }

    #[test]
    fn test_openai_endpoint_with_override() {
        let config = AiConfig {
            account: "my-ai-services".to_string(),
            deployment: "gpt-4o-mini".to_string(),
            endpoint: Some("https://custom.openai.azure.com/".to_string()),
            subscription: None,
            resource_group: None,
            api_version: "2024-12-01-preview".to_string(),
        };
        assert_eq!(config.openai_endpoint(), "https://custom.openai.azure.com");
    }
}
