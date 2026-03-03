//! AI-powered diff explanations using Azure OpenAI

use hoist_client::AzureOpenAIClient;
use hoist_core::resources::ResourceKind;
use hoist_diff::Change;

const NARRATIVE_SYSTEM_PROMPT: &str = "\
You are explaining configuration changes for Azure AI Search and Microsoft Foundry resources \
to a developer. Write a clear, detailed narrative that helps the user understand exactly what \
is different and what will happen.

Guidelines:
- Describe the actual content and behavior of resources, not just property names or JSON paths.
- For agent instructions, explain what the instructions say and how they differ between versions.
- For search indexes, describe what fields are being added, removed, or changed and what that \
  means for search behavior (e.g., filtering, sorting, retrieval).
- For indexers, skillsets, and data sources, explain what the pipeline does and how it changes.
- Reference specific content from the configurations — quote key phrases when they help clarify.
- Group related changes and note cross-resource patterns (e.g., shared models or settings).
- Mention unchanged properties only when they provide useful context.
- Frame the explanation based on the operation context (comparing, pulling, or pushing).
- Write in paragraph form with a professional but approachable tone. Do not use bullet points \
  or technical change lists — the user wants a readable narrative.
- Be thorough — cover all changes — but don't be repetitive.
- Start with a brief overview of the scope, then go into detail for each resource.";

/// Per-resource AI summary (used for JSON/MCP output backward compatibility).
const PER_RESOURCE_SYSTEM_PROMPT: &str = "\
You are analyzing configuration changes for Azure AI Search and Microsoft Foundry resources. \
The user has already seen a detailed per-change breakdown. Your job is to explain the practical \
IMPACT of these changes — what will behave differently, what risks exist, and whether the \
changes look intentional and consistent. Be concise (2-4 sentences). Do not repeat the change \
descriptions the user already sees. Instead, synthesize: Why might someone have made these \
changes? Are there any concerns (e.g., breaking changes, missing dependencies, inconsistencies \
between changes)? If agent instructions changed, summarize the intent shift, not the \
word-level edits.";

/// Status of a resource in the change set.
pub enum ChangeStatus {
    /// Only exists on one side (new locally for push, or new on server for pull)
    New,
    /// Exists on both sides with differences
    Modified,
    /// Was deleted on one side
    Deleted,
}

/// Full context for a resource, used to build AI prompts.
pub struct ResourceContext {
    pub kind: ResourceKind,
    pub name: String,
    pub status: ChangeStatus,
    /// Full content of the local version (YAML for agents, JSON for search resources)
    pub local_content: Option<String>,
    /// Full content of the remote/server version
    pub remote_content: Option<String>,
    /// Pre-computed English descriptions of changes
    pub descriptions: Vec<String>,
}

/// Generate a single narrative covering all changed resources.
///
/// This is the primary AI explanation shown to users in terminal output.
/// It replaces the per-change description list when AI is enabled.
pub async fn explain_all_changes(
    client: &AzureOpenAIClient,
    resources: &[ResourceContext],
    command_context: &str,
    total_unchanged: usize,
) -> Result<String, hoist_client::ClientError> {
    let user_prompt = build_narrative_prompt(resources, command_context, total_unchanged);
    client
        .chat_completion_with_limit(NARRATIVE_SYSTEM_PROMPT, &user_prompt, 0.3, 4000)
        .await
}

/// Generate a per-resource AI summary (for JSON/MCP output).
pub async fn explain_resource_changes(
    client: &AzureOpenAIClient,
    resource_type: &str,
    resource_name: &str,
    changes: &[Change],
    descriptions: &[String],
    command_context: &str,
) -> Result<String, hoist_client::ClientError> {
    let user_prompt = build_per_resource_prompt(
        resource_type,
        resource_name,
        changes,
        descriptions,
        command_context,
    );
    client
        .chat_completion(PER_RESOURCE_SYSTEM_PROMPT, &user_prompt, 0.3)
        .await
}

// ---------------------------------------------------------------------------
// Narrative prompt (all resources, full content)
// ---------------------------------------------------------------------------

fn build_narrative_prompt(
    resources: &[ResourceContext],
    command_context: &str,
    total_unchanged: usize,
) -> String {
    let direction = match command_context {
        "push" => {
            "Pushing local configuration to Azure. These changes will be applied to the server."
        }
        "pull" => {
            "Pulling configuration from Azure. These changes will be applied to your local files."
        }
        _ => "Comparing local configuration files against what is currently on Azure.",
    };

    let mut prompt = format!("Operation: {}\n{}\n\n", command_context, direction);

    let changed_count = resources.len();
    prompt.push_str(&format!("{} resource(s) with changes", changed_count));
    if total_unchanged > 0 {
        prompt.push_str(&format!(", {} unchanged", total_unchanged));
    }
    prompt.push_str(".\n\n");

    for resource in resources {
        let status_label = match (&resource.status, command_context) {
            (ChangeStatus::New, "push") => "new locally — will be created on the server",
            (ChangeStatus::New, "pull") => "new on the server — will be created locally",
            (ChangeStatus::New, _) => "exists on one side only",
            (ChangeStatus::Modified, _) => "modified — differs between local and server",
            (ChangeStatus::Deleted, "push") => "deleted locally",
            (ChangeStatus::Deleted, "pull") => "deleted on the server — will be removed locally",
            (ChangeStatus::Deleted, _) => "exists on one side only",
        };

        prompt.push_str(&format!(
            "=== {} '{}' ({}) ===\n",
            resource.kind.display_name(),
            resource.name,
            status_label
        ));

        if let Some(local) = &resource.local_content {
            prompt.push_str("--- Local version ---\n");
            push_content_with_limit(&mut prompt, local, 15000);
            prompt.push('\n');
        }

        if let Some(remote) = &resource.remote_content {
            prompt.push_str("--- Server version ---\n");
            push_content_with_limit(&mut prompt, remote, 15000);
            prompt.push('\n');
        }

        if !resource.descriptions.is_empty() {
            prompt.push_str("Change summary:\n");
            for desc in &resource.descriptions {
                prompt.push_str(&format!("- {}\n", desc));
            }
        }

        prompt.push('\n');
    }

    prompt
}

/// Append content to the prompt, truncating if it exceeds `max_chars`.
fn push_content_with_limit(prompt: &mut String, content: &str, max_chars: usize) {
    if content.len() <= max_chars {
        prompt.push_str(content);
    } else {
        prompt.push_str(&content[..max_chars]);
        prompt.push_str(&format!(
            "\n... (truncated, {} chars total)\n",
            content.len()
        ));
    }
}

/// Format a resource value for the AI prompt.
/// Agents get YAML (more readable), search resources get formatted JSON.
pub fn format_for_ai(kind: ResourceKind, value: &serde_json::Value) -> String {
    if kind == ResourceKind::Agent {
        hoist_core::resources::agent::agent_to_yaml(value)
    } else {
        hoist_core::normalize::format_json(value)
    }
}

// ---------------------------------------------------------------------------
// Per-resource prompt (for JSON/MCP backward compat)
// ---------------------------------------------------------------------------

fn build_per_resource_prompt(
    resource_type: &str,
    resource_name: &str,
    changes: &[Change],
    descriptions: &[String],
    command_context: &str,
) -> String {
    let context_explanation = match command_context {
        "push" => "These changes will be applied from local files to Azure.",
        "pull" => "These changes will be pulled from Azure to overwrite local files.",
        _ => "Showing differences between local files and Azure.",
    };

    let mut prompt = format!(
        "Resource: {} '{}'\nOperation: {} — {}\n",
        resource_type, resource_name, command_context, context_explanation
    );

    if !descriptions.is_empty() {
        prompt.push_str("\nChange descriptions:\n");
        for (i, desc) in descriptions.iter().enumerate() {
            prompt.push_str(&format!("{}. {}\n", i + 1, desc));
        }
    }

    let mut raw_details = Vec::new();
    for change in changes {
        if let Some(d) = build_value_detail(change) {
            raw_details.push(d);
        }
    }

    if !raw_details.is_empty() {
        prompt.push_str("\nValue details:\n");
        for detail in &raw_details {
            prompt.push_str(&format!("- {}\n", detail));
        }
    }

    prompt
}

fn build_value_detail(change: &Change) -> Option<String> {
    match change.kind {
        hoist_diff::ChangeKind::Modified => {
            let old = change.old_value.as_ref()?;
            let new = change.new_value.as_ref()?;
            if is_simple_scalar(old) && is_simple_scalar(new) {
                return None;
            }
            let old_summary = summarize_value_rich(old);
            let new_summary = summarize_value_rich(new);
            Some(format!(
                "{}: {} → {}",
                change.path, old_summary, new_summary
            ))
        }
        hoist_diff::ChangeKind::Added => {
            let new = change.new_value.as_ref()?;
            if is_simple_scalar(new) {
                return None;
            }
            Some(format!(
                "{}: (added) {}",
                change.path,
                summarize_value_rich(new)
            ))
        }
        hoist_diff::ChangeKind::Removed => {
            let old = change.old_value.as_ref()?;
            if is_simple_scalar(old) {
                return None;
            }
            Some(format!(
                "{}: (removed) {}",
                change.path,
                summarize_value_rich(old)
            ))
        }
    }
}

fn is_simple_scalar(value: &serde_json::Value) -> bool {
    matches!(
        value,
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) | serde_json::Value::Null
    ) || matches!(value, serde_json::Value::String(s) if s.len() <= 200)
}

fn summarize_value_rich(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => {
            if s.len() > 2000 {
                format!("\"{}...\" ({} chars total)", &s[..2000], s.len())
            } else {
                format!("\"{}\"", s)
            }
        }
        serde_json::Value::Array(arr) => {
            let names: Vec<&str> = arr
                .iter()
                .filter_map(|item| item.get("name").and_then(|n| n.as_str()))
                .take(10)
                .collect();
            if names.is_empty() {
                format!("[{} items]", arr.len())
            } else if names.len() < arr.len() {
                format!("[{} items: {}, ...]", arr.len(), names.join(", "))
            } else {
                format!("[{} items: {}]", arr.len(), names.join(", "))
            }
        }
        serde_json::Value::Object(obj) => {
            let keys: Vec<&str> = obj.keys().take(10).map(|k| k.as_str()).collect();
            if keys.len() < obj.len() {
                format!("{{keys: {}, ...}}", keys.join(", "))
            } else {
                format!("{{keys: {}}}", keys.join(", "))
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_value_rich_string() {
        let v = serde_json::Value::String("hello".to_string());
        assert_eq!(summarize_value_rich(&v), "\"hello\"");
    }

    #[test]
    fn test_summarize_value_rich_long_string() {
        let s = "a".repeat(3000);
        let v = serde_json::Value::String(s);
        let result = summarize_value_rich(&v);
        assert!(result.contains("3000 chars total"));
        assert!(result.contains(&"a".repeat(2000)));
    }

    #[test]
    fn test_summarize_value_rich_array_with_names() {
        let v = serde_json::json!([
            {"name": "field1", "type": "string"},
            {"name": "field2", "type": "int"}
        ]);
        assert_eq!(summarize_value_rich(&v), "[2 items: field1, field2]");
    }

    #[test]
    fn test_summarize_value_rich_array_without_names() {
        let v = serde_json::json!([1, 2, 3]);
        assert_eq!(summarize_value_rich(&v), "[3 items]");
    }

    #[test]
    fn test_summarize_value_rich_object() {
        let v = serde_json::json!({"model": "gpt-4", "temperature": 0.7});
        let result = summarize_value_rich(&v);
        assert!(result.starts_with("{keys: "));
        assert!(result.contains("model"));
        assert!(result.contains("temperature"));
    }

    #[test]
    fn test_narrative_prompt_structure() {
        let resources = vec![ResourceContext {
            kind: ResourceKind::Agent,
            name: "bot".to_string(),
            status: ChangeStatus::Modified,
            local_content: Some("instructions: Be friendly\nmodel: gpt-4o\n".to_string()),
            remote_content: Some("instructions: Be formal\nmodel: gpt-4o\n".to_string()),
            descriptions: vec!["Instructions differ between local and server".to_string()],
        }];
        let prompt = build_narrative_prompt(&resources, "diff", 3);
        assert!(prompt.contains("Operation: diff"));
        assert!(prompt.contains("Comparing local"));
        assert!(prompt.contains("Agent 'bot'"));
        assert!(prompt.contains("--- Local version ---"));
        assert!(prompt.contains("--- Server version ---"));
        assert!(prompt.contains("Be friendly"));
        assert!(prompt.contains("Be formal"));
        assert!(prompt.contains("3 unchanged"));
    }

    #[test]
    fn test_narrative_prompt_push_framing() {
        let resources = vec![ResourceContext {
            kind: ResourceKind::Index,
            name: "products".to_string(),
            status: ChangeStatus::New,
            local_content: Some("{\"name\": \"products\"}".to_string()),
            remote_content: None,
            descriptions: vec![],
        }];
        let prompt = build_narrative_prompt(&resources, "push", 0);
        assert!(prompt.contains("Pushing local configuration to Azure"));
        assert!(prompt.contains("will be created on the server"));
    }

    #[test]
    fn test_narrative_prompt_pull_framing() {
        let resources = vec![ResourceContext {
            kind: ResourceKind::Agent,
            name: "helper".to_string(),
            status: ChangeStatus::Deleted,
            local_content: Some("instructions: old\n".to_string()),
            remote_content: None,
            descriptions: vec![],
        }];
        let prompt = build_narrative_prompt(&resources, "pull", 0);
        assert!(prompt.contains("Pulling configuration from Azure"));
        assert!(prompt.contains("will be removed locally"));
    }

    #[test]
    fn test_content_truncation() {
        let mut prompt = String::new();
        let long_content = "x".repeat(20000);
        push_content_with_limit(&mut prompt, &long_content, 15000);
        assert!(prompt.contains("truncated"));
        assert!(prompt.contains("20000 chars total"));
    }

    #[test]
    fn test_per_resource_prompt() {
        let changes = vec![Change {
            path: "model".to_string(),
            kind: hoist_diff::ChangeKind::Modified,
            old_value: Some(serde_json::json!("gpt-4")),
            new_value: Some(serde_json::json!("gpt-4o")),
            description: None,
        }];
        let descriptions =
            vec!["Uses model 'gpt-4o' locally but 'gpt-4' on the server".to_string()];
        let prompt =
            build_per_resource_prompt("Agent", "my-agent", &changes, &descriptions, "push");
        assert!(prompt.contains("Agent 'my-agent'"));
        assert!(prompt.contains("push"));
        assert!(prompt.contains("Uses model"));
    }

    #[test]
    fn test_build_value_detail_skips_simple_scalars() {
        let change = Change {
            path: "model".to_string(),
            kind: hoist_diff::ChangeKind::Modified,
            old_value: Some(serde_json::json!("gpt-4")),
            new_value: Some(serde_json::json!("gpt-4o")),
            description: None,
        };
        assert!(build_value_detail(&change).is_none());
    }

    #[test]
    fn test_build_value_detail_includes_long_strings() {
        let change = Change {
            path: "instructions".to_string(),
            kind: hoist_diff::ChangeKind::Modified,
            old_value: Some(serde_json::Value::String("a".repeat(500))),
            new_value: Some(serde_json::Value::String("b".repeat(500))),
            description: None,
        };
        let detail = build_value_detail(&change);
        assert!(detail.is_some());
        assert!(detail.unwrap().contains("instructions:"));
    }
}
