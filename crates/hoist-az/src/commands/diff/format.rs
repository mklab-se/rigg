//! Output formatting for diff results (text and JSON).

use colored::Colorize;

use crate::commands::describe::{annotate_changes, describe_changes, describe_changes_plain};

use super::ResourceDiff;

/// Print diff results as colored terminal text.
pub(super) fn format_diff_text(
    diffs: &[ResourceDiff],
    labels: Option<(&str, &str)>,
    ai_summaries: &std::collections::HashMap<String, String>,
) {
    let (changed, unchanged): (Vec<_>, Vec<_>) = diffs.iter().partition(|d| !d.result.is_equal);

    if changed.is_empty() {
        println!(
            "No drift detected, all {} resource(s) match.",
            unchanged.len()
        );
        return;
    }

    println!(
        "{} resource(s) with drift:\n",
        changed.len().to_string().yellow()
    );
    for (idx, d) in changed.iter().enumerate() {
        if idx > 0 {
            println!();
        }
        println!(
            "  {} {} ({} change{})",
            "~".yellow(),
            d.display_id,
            d.result.changes.len(),
            if d.result.changes.len() == 1 { "" } else { "s" }
        );
        for line in describe_changes(&d.result.changes, d.kind, &d.resource_name, labels, true) {
            println!("{}", line);
        }
        // AI summary
        if let Some(summary) = ai_summaries.get(&d.display_id) {
            println!();
            for line in summary.lines() {
                println!("      {} {}", "AI:".cyan(), line);
            }
        }
    }
    println!();
    if !unchanged.is_empty() {
        println!(
            "  {} resource(s) match",
            unchanged.len().to_string().dimmed()
        );
    }
}

/// Produce enhanced JSON diff output with annotated descriptions.
pub(super) fn format_diff_json(
    diffs: &mut [ResourceDiff],
    labels: (&str, &str),
    ai_summaries: &std::collections::HashMap<String, String>,
) -> String {
    let report: Vec<_> = diffs
        .iter_mut()
        .map(|d| {
            // Determine status
            let status = if d.result.is_equal {
                "unchanged"
            } else if d.result.changes.len() == 1 && d.result.changes[0].path == "." {
                match d.result.changes[0].kind {
                    hoist_diff::ChangeKind::Added => "local_only",
                    hoist_diff::ChangeKind::Removed => "remote_only",
                    _ => "modified",
                }
            } else {
                "modified"
            };

            // Annotate changes with English descriptions
            annotate_changes(
                &mut d.result.changes,
                d.kind,
                &d.resource_name,
                Some(labels),
            );

            // Build summary line
            let summary = if d.result.is_equal {
                format!(
                    "{} '{}' is unchanged",
                    d.kind.display_name(),
                    d.resource_name
                )
            } else {
                let descs = describe_changes_plain(
                    &d.result.changes,
                    d.kind,
                    &d.resource_name,
                    Some(labels),
                );
                if descs.len() == 1 {
                    descs[0].clone()
                } else {
                    format!(
                        "{} '{}' has {} difference(s)",
                        d.kind.display_name(),
                        d.resource_name,
                        d.result.changes.len()
                    )
                }
            };

            let mut entry = serde_json::json!({
                "resource_type": d.kind.display_name(),
                "resource_name": d.resource_name,
                "resource_id": d.display_id,
                "status": status,
                "summary": summary,
                "changes": d.result.changes,
            });
            if let Some(ai) = ai_summaries.get(&d.display_id) {
                entry["ai_summary"] = serde_json::json!(ai);
            }
            entry
        })
        .collect();

    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "[]".to_string())
}
