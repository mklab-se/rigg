//! `rigg az agent` — single-shot prompts against live Foundry agents.

use anyhow::Result;
use serde_json::Value;

use crate::cli::AzAgentCommands;
use crate::commands::GlobalContext;

pub async fn run(ctx: &GlobalContext, command: AzAgentCommands) -> Result<()> {
    match command {
        AzAgentCommands::Ask { name, prompt } => ask(ctx, &name, &prompt).await,
    }
}

async fn ask(ctx: &GlobalContext, name: &str, prompt: &str) -> Result<()> {
    let (_ws, _env, remote) = super::connect(ctx)?;
    let result = remote.agent_ask(name, prompt).await?;
    if ctx.json() {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    let text = extract_reply(&result);
    match text {
        Some(text) => println!("{text}"),
        None => {
            println!("(could not find reply text in the response — raw payload follows)");
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
    }
    Ok(())
}

/// Pull the reply text out of an OpenAI-shaped responses object: prefer the
/// convenience `output_text`, else walk `output[] → content[]` collecting
/// `output_text` entries.
fn extract_reply(result: &Value) -> Option<String> {
    if let Some(text) = result.get("output_text").and_then(Value::as_str) {
        return Some(text.to_string());
    }
    let mut parts: Vec<String> = Vec::new();
    for item in result.get("output").and_then(Value::as_array)? {
        let Some(contents) = item.get("content").and_then(Value::as_array) else {
            continue;
        };
        for content in contents {
            let is_text = content
                .get("type")
                .and_then(Value::as_str)
                .is_none_or(|t| t == "output_text" || t == "text");
            if is_text {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    parts.push(text.to_string());
                }
            }
        }
    }
    (!parts.is_empty()).then(|| parts.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_output_text_items() {
        let response = json!({
            "output": [
                {"type": "message", "content": [
                    {"type": "output_text", "text": "Hello"},
                    {"type": "output_text", "text": "world"}
                ]}
            ]
        });
        assert_eq!(extract_reply(&response), Some("Hello\nworld".to_string()));
    }

    #[test]
    fn prefers_top_level_output_text() {
        let response = json!({"output_text": "short"});
        assert_eq!(extract_reply(&response), Some("short".to_string()));
    }

    #[test]
    fn none_when_no_text() {
        assert_eq!(extract_reply(&json!({"output": []})), None);
    }
}
