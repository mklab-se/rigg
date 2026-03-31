//! Unified AI text generation via ailloy
//!
//! Uses the globally configured ailloy provider for AI requests.

use ailloy::{ChatOptions, Client, Message};

/// Generate text using the globally configured ailloy provider.
///
/// Uses the default chat node from `~/.config/ailloy/config.yaml`.
/// Run `ailloy config` to set up a provider.
pub async fn generate_text(system_prompt: &str, user_prompt: &str) -> anyhow::Result<String> {
    generate_text_with_limit(system_prompt, user_prompt, 2000).await
}

/// Generate text with a custom max_tokens limit.
pub async fn generate_text_with_limit(
    system_prompt: &str,
    user_prompt: &str,
    max_tokens: u32,
) -> anyhow::Result<String> {
    let client = Client::from_config()?;
    let opts = ChatOptions::builder()
        .temperature(0.3)
        .max_tokens(max_tokens)
        .build();
    let response = client
        .chat_with(
            &[Message::system(system_prompt), Message::user(user_prompt)],
            &opts,
        )
        .await?;
    Ok(response.content)
}

/// Check if ailloy is configured with a default chat node.
pub fn is_configured() -> bool {
    ailloy::config::Config::load()
        .map(|c| c.default_chat_node().is_ok())
        .unwrap_or(false)
}
