//! AI-powered push narrative generation.

use std::collections::HashMap;

use hoist_core::resources::ResourceKind;
use hoist_diff::Change;

use crate::commands::describe::describe_changes_plain;
use crate::commands::explain::{self, ChangeStatus, ResourceContext, format_for_ai};

/// Generate a single AI narrative covering all push changes.
///
/// Returns `Some(narrative)` on success, `None` on failure (caller falls back to non-AI output).
pub(super) async fn generate_push_narrative(
    config: &hoist_core::Config,
    resources_to_push: &[(ResourceKind, String, serde_json::Value, bool)],
    change_details: &HashMap<(ResourceKind, String), Vec<Change>>,
    remote_values: &HashMap<(ResourceKind, String), serde_json::Value>,
    total_unchanged: usize,
) -> Option<String> {
    let ai_config = config.ai.as_ref()?;

    eprintln!("Generating AI explanation...");

    let push_labels = Some(("on the server", "locally"));
    let mut contexts = Vec::new();

    for (kind, name, local_def, exists) in resources_to_push {
        let key = (*kind, name.clone());

        if *exists {
            // Updated resource — has changes
            if let Some(changes) = change_details.get(&key) {
                let local_content = Some(format_for_ai(*kind, local_def));
                let remote_content = remote_values.get(&key).map(|v| format_for_ai(*kind, v));
                let descriptions = describe_changes_plain(changes, *kind, name, push_labels);
                contexts.push(ResourceContext {
                    kind: *kind,
                    name: name.clone(),
                    status: ChangeStatus::Modified,
                    local_content,
                    remote_content,
                    descriptions,
                });
            }
        } else {
            // New resource — will be created
            let local_content = Some(format_for_ai(*kind, local_def));
            contexts.push(ResourceContext {
                kind: *kind,
                name: name.clone(),
                status: ChangeStatus::New,
                local_content,
                remote_content: None,
                descriptions: vec![],
            });
        }
    }

    if contexts.is_empty() {
        return None;
    }

    match explain::explain_all_changes(ai_config, &contexts, "push", total_unchanged).await {
        Ok(narrative) => Some(narrative),
        Err(e) => {
            eprintln!("Warning: AI explanation failed: {}", e);
            None
        }
    }
}
