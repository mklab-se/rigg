//! AI-powered explanation helpers for diff output.

use hoist_core::Config;

use crate::commands::describe::describe_changes_plain;
use crate::commands::explain::{self, ChangeStatus, ResourceContext};

use super::ResourceDiff;

/// Generate a single AI narrative covering all changed resources.
///
/// Returns `Some(narrative)` on success, `None` on failure (caller falls back to non-AI output).
pub(super) async fn generate_ai_narrative(
    config: &Config,
    diffs: &[ResourceDiff],
    command_context: &str,
    total_unchanged: usize,
) -> Option<String> {
    let ai_config = config.ai.as_ref()?;

    let changed: Vec<&ResourceDiff> = diffs.iter().filter(|d| !d.result.is_equal).collect();
    if changed.is_empty() {
        return None;
    }

    eprintln!("Generating AI explanation...");

    let labels = Some(("locally", "on the server"));
    let contexts: Vec<ResourceContext> = changed
        .iter()
        .map(|d| {
            let status = if d.result.changes.len() == 1 && d.result.changes[0].path == "." {
                ChangeStatus::New
            } else {
                ChangeStatus::Modified
            };
            let descriptions =
                describe_changes_plain(&d.result.changes, d.kind, &d.resource_name, labels);
            ResourceContext {
                kind: d.kind,
                name: d.resource_name.clone(),
                status,
                local_content: d.local_content.clone(),
                remote_content: d.remote_content.clone(),
                descriptions,
            }
        })
        .collect();

    match explain::explain_all_changes(ai_config, &contexts, command_context, total_unchanged).await
    {
        Ok(narrative) => Some(narrative),
        Err(e) => {
            eprintln!("Warning: AI explanation failed: {}", e);
            None
        }
    }
}

/// Generate AI summaries for all changed resources.
pub(super) async fn generate_ai_summaries(
    config: &Config,
    diffs: &[ResourceDiff],
) -> std::collections::HashMap<String, String> {
    let mut summaries = std::collections::HashMap::new();

    let ai_config = match &config.ai {
        Some(c) => c,
        None => return summaries,
    };

    let changed: Vec<&ResourceDiff> = diffs.iter().filter(|d| !d.result.is_equal).collect();
    if changed.is_empty() {
        return summaries;
    }

    eprintln!("Generating AI explanations...");

    // Run LLM calls concurrently
    let labels = Some(("locally", "on the server"));
    let futures: Vec<_> = changed
        .iter()
        .map(|d| {
            let display_id = d.display_id.clone();
            let resource_type = d.kind.display_name().to_string();
            let resource_name = d.resource_name.clone();
            let changes = d.result.changes.clone();
            let descriptions =
                describe_changes_plain(&d.result.changes, d.kind, &d.resource_name, labels);
            async move {
                let result = crate::commands::explain::explain_resource_changes(
                    ai_config,
                    &resource_type,
                    &resource_name,
                    &changes,
                    &descriptions,
                    "diff",
                )
                .await;
                (display_id, result)
            }
        })
        .collect();

    let results = futures::future::join_all(futures).await;

    for (display_id, result) in results {
        match result {
            Ok(summary) => {
                summaries.insert(display_id, summary);
            }
            Err(e) => {
                eprintln!("Warning: AI explanation failed for {}: {}", display_id, e);
            }
        }
    }

    summaries
}
