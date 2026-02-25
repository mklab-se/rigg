//! AI-powered diff explanations using Azure OpenAI

use hoist_client::AzureOpenAIClient;
use hoist_diff::Change;

const SYSTEM_PROMPT: &str = "\
You are analyzing configuration differences for Azure AI Search and Microsoft Foundry resources. \
Provide a brief, clear explanation of what the changes mean for the resource's behavior. \
Focus on practical impact. Use 2-3 sentences. \
Do not say things were \"added\" or \"removed\" — describe what differs between the two versions. \
If instructions or prompts changed, summarize the intent shift.";

/// Generate an AI explanation of resource changes.
pub async fn explain_resource_changes(
    client: &AzureOpenAIClient,
    resource_type: &str,
    resource_name: &str,
    changes: &[Change],
) -> Result<String, hoist_client::ClientError> {
    let user_prompt = build_user_prompt(resource_type, resource_name, changes);
    client
        .chat_completion(SYSTEM_PROMPT, &user_prompt, 0.3)
        .await
}

fn build_user_prompt(resource_type: &str, resource_name: &str, changes: &[Change]) -> String {
    let mut prompt = format!(
        "Resource: {} '{}'\nChanges:\n",
        resource_type, resource_name
    );

    for (i, change) in changes.iter().enumerate() {
        let desc = match change.kind {
            hoist_diff::ChangeKind::Modified => {
                let old = change
                    .old_value
                    .as_ref()
                    .map(summarize_value)
                    .unwrap_or_default();
                let new = change
                    .new_value
                    .as_ref()
                    .map(summarize_value)
                    .unwrap_or_default();
                format!("{}: {} → {}", change.path, old, new)
            }
            hoist_diff::ChangeKind::Added => {
                let new = change
                    .new_value
                    .as_ref()
                    .map(summarize_value)
                    .unwrap_or_default();
                format!("{}: (new) {}", change.path, new)
            }
            hoist_diff::ChangeKind::Removed => {
                let old = change
                    .old_value
                    .as_ref()
                    .map(summarize_value)
                    .unwrap_or_default();
                format!("{}: (removed) {}", change.path, old)
            }
        };
        prompt.push_str(&format!("{}. {}\n", i + 1, desc));
    }

    prompt
}

/// Summarize a JSON value for the prompt, truncating long strings.
fn summarize_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > 200 {
                format!("\"{}...\" ({} chars)", &s[..200], s.len())
            } else {
                format!("\"{}\"", s)
            }
        }
        serde_json::Value::Array(arr) => format!("[{} items]", arr.len()),
        serde_json::Value::Object(obj) => format!("{{{} fields}}", obj.len()),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_value_string() {
        let v = serde_json::Value::String("hello".to_string());
        assert_eq!(summarize_value(&v), "\"hello\"");
    }

    #[test]
    fn test_summarize_value_long_string() {
        let s = "a".repeat(300);
        let v = serde_json::Value::String(s);
        let result = summarize_value(&v);
        assert!(result.contains("300 chars"));
        assert!(result.ends_with(')'));
    }

    #[test]
    fn test_summarize_value_array() {
        let v = serde_json::json!([1, 2, 3]);
        assert_eq!(summarize_value(&v), "[3 items]");
    }

    #[test]
    fn test_summarize_value_object() {
        let v = serde_json::json!({"a": 1, "b": 2});
        assert_eq!(summarize_value(&v), "{2 fields}");
    }

    #[test]
    fn test_build_user_prompt() {
        let changes = vec![Change {
            path: ".model".to_string(),
            kind: hoist_diff::ChangeKind::Modified,
            old_value: Some(serde_json::json!("gpt-4")),
            new_value: Some(serde_json::json!("gpt-4o")),
            description: None,
        }];
        let prompt = build_user_prompt("Agent", "my-agent", &changes);
        assert!(prompt.contains("Agent 'my-agent'"));
        assert!(prompt.contains(".model"));
        assert!(prompt.contains("gpt-4"));
        assert!(prompt.contains("gpt-4o"));
    }
}
