//! Ollama local LLM client
//!
//! Communicates with the Ollama HTTP API for chat completions
//! using locally running models.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

use crate::error::ClientError;

/// Default Ollama server URL
const DEFAULT_BASE_URL: &str = "http://localhost:11434";

/// Ollama HTTP API client
pub struct OllamaClient {
    http: reqwest::Client,
    base_url: String,
}

/// A locally installed Ollama model
#[derive(Debug, Clone, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct TagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

impl OllamaClient {
    /// Create a new Ollama client
    pub fn new(base_url: Option<&str>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300)) // local LLMs can be slow
            .build()
            .expect("failed to build HTTP client");

        Self {
            http,
            base_url: base_url
                .unwrap_or(DEFAULT_BASE_URL)
                .trim_end_matches('/')
                .to_string(),
        }
    }

    /// List locally installed models
    pub async fn list_models(&self) -> Result<Vec<OllamaModel>, ClientError> {
        let url = format!("{}/api/tags", self.base_url);
        debug!("Ollama list models: {}", url);

        let response = self.http.get(&url).send().await.map_err(|e| {
            ClientError::local_agent(format!(
                "Cannot connect to Ollama at {}. Is it running? Error: {e}",
                self.base_url
            ))
        })?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ClientError::local_agent(format!(
                "Ollama API error: {body}"
            )));
        }

        let tags: TagsResponse = response.json().await.map_err(|e| {
            ClientError::local_agent(format!("Failed to parse Ollama response: {e}"))
        })?;

        Ok(tags.models)
    }

    /// Send a chat completion request to a local model
    pub async fn chat_completion(
        &self,
        model: &str,
        system_prompt: &str,
        user_prompt: &str,
    ) -> Result<String, ClientError> {
        let url = format!("{}/api/chat", self.base_url);
        debug!("Ollama chat completion: {} model={}", url, model);

        let body = ChatRequest {
            model: model.to_string(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: system_prompt.to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: user_prompt.to_string(),
                },
            ],
            stream: false,
        };

        let response = self.http.post(&url).json(&body).send().await.map_err(|e| {
            ClientError::local_agent(format!(
                "Cannot connect to Ollama at {}. Is it running? Error: {e}",
                self.base_url
            ))
        })?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ClientError::local_agent(format!(
                "Ollama API error ({status}): {body}"
            )));
        }

        let json: Value = response.json().await.map_err(|e| {
            ClientError::local_agent(format!("Failed to parse Ollama response: {e}"))
        })?;

        let content = json
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        if content.is_empty() {
            return Err(ClientError::local_agent(
                "Ollama returned an empty response. The model may not support this task.",
            ));
        }

        Ok(content)
    }
}

/// Format a model size in human-readable form
pub fn format_model_size(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1_000_000_000.0)
    } else if bytes >= 1_000_000 {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_model_size() {
        assert_eq!(format_model_size(3_300_000_000), "3.3 GB");
        assert_eq!(format_model_size(500_000_000), "500 MB");
        assert_eq!(format_model_size(1024), "1024 B");
    }

    #[test]
    fn test_parse_tags_response() {
        let json = r#"{"models": [{"name": "gemma3:4b", "size": 3300000000}, {"name": "llama3:8b", "size": 4700000000}]}"#;
        let tags: TagsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(tags.models.len(), 2);
        assert_eq!(tags.models[0].name, "gemma3:4b");
        assert_eq!(tags.models[1].name, "llama3:8b");
    }

    #[test]
    fn test_parse_chat_response() {
        let json = r#"{"model": "gemma3:4b", "message": {"role": "assistant", "content": "Hello!"}, "done": true}"#;
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let content = value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(content, "Hello!");
    }

    #[test]
    fn test_default_base_url() {
        let client = OllamaClient::new(None);
        assert_eq!(client.base_url, "http://localhost:11434");
    }

    #[test]
    fn test_custom_base_url_trailing_slash() {
        let client = OllamaClient::new(Some("http://my-server:11434/"));
        assert_eq!(client.base_url, "http://my-server:11434");
    }
}
