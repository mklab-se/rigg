//! Unified AI text generation dispatcher
//!
//! Routes AI requests to the configured provider: Azure OpenAI API,
//! local CLI agents (claude, codex, copilot), or Ollama.

use hoist_core::config::{AiConfig, AiProvider};

use crate::error::ClientError;
use crate::local_agent;
use crate::ollama::OllamaClient;
use crate::openai::AzureOpenAIClient;

/// Generate text using the configured AI provider.
///
/// Dispatches to the appropriate backend based on `config.provider`:
/// - `AzureOpenai`: REST API call with AAD token auth
/// - `Claude`/`Codex`/`Copilot`: Local CLI subprocess
/// - `Ollama`: Local HTTP API
pub async fn generate_text(
    config: &AiConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, ClientError> {
    generate_text_with_limit(config, system_prompt, user_prompt, 2000).await
}

/// Generate text with a custom max_tokens limit (only applies to Azure OpenAI).
pub async fn generate_text_with_limit(
    config: &AiConfig,
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> Result<String, ClientError> {
    match config.provider {
        AiProvider::AzureOpenai => {
            let client = AzureOpenAIClient::from_config(config)?;
            client
                .chat_completion_with_limit(system_prompt, user_prompt, 0.3, max_tokens)
                .await
        }

        AiProvider::Claude | AiProvider::Codex | AiProvider::Copilot => {
            let model = config.effective_model();
            local_agent::generate_text(
                &config.provider,
                model.as_deref(),
                system_prompt,
                user_prompt,
            )
            .await
        }

        AiProvider::Ollama => {
            let model = config.effective_model().ok_or_else(|| {
                ClientError::local_agent(
                    "Ollama requires a model to be configured. Run `hoist ai init` to select one.",
                )
            })?;
            let client = OllamaClient::new(Some(&config.ollama_base_url()));
            client
                .chat_completion(&model, system_prompt, user_prompt)
                .await
        }
    }
}
