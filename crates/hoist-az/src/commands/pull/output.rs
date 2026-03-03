//! Display and AI narrative generation for pull results.

use std::path::PathBuf;

use colored::Colorize;

use hoist_core::resources::ResourceKind;

use crate::commands::describe::{describe_changes, describe_changes_plain};
use crate::commands::explain::{self, ChangeStatus, ResourceContext};

use super::discover::DiscoveredResource;

/// Print the non-AI pull summary to stdout.
pub(super) fn print_pull_summary(
    new_resources: &[DiscoveredResource],
    updated_resources: &[DiscoveredResource],
    deleted_resources: &[(ResourceKind, String, PathBuf)],
    total_unchanged: usize,
) {
    let pull_labels = Some(("locally", "on the server"));
    println!("Pull summary:");
    for r in new_resources {
        println!(
            "  {} {} '{}' is new on the server and will be created locally",
            "+".green(),
            r.kind.display_name(),
            r.name
        );
    }
    let mut prev_had_details = false;
    for r in updated_resources {
        if r.changes.is_empty() {
            println!(
                "  {} {} '{}' has changed on the server — pulling will update your local file",
                "~".yellow(),
                r.kind.display_name(),
                r.name
            );
            prev_had_details = false;
        } else {
            if prev_had_details {
                println!();
            }
            println!(
                "  {} {} '{}' has {} difference{} — pulling will update your local file:",
                "~".yellow(),
                r.kind.display_name(),
                r.name,
                r.changes.len(),
                if r.changes.len() == 1 { "" } else { "s" }
            );
            for line in describe_changes(&r.changes, r.kind, &r.name, pull_labels, true) {
                println!("{}", line);
            }
            prev_had_details = true;
        }
    }
    for (kind, name, _) in deleted_resources {
        println!(
            "  {} {} '{}' was deleted on the server — pulling will remove your local file",
            "-".red(),
            kind.display_name(),
            name
        );
    }
    if total_unchanged > 0 {
        println!(
            "  {} resource(s) already up to date",
            total_unchanged.to_string().dimmed()
        );
    }
}

/// Print warnings about locally modified files that will be overwritten.
pub(super) fn print_local_modification_warnings(updated_resources: &[DiscoveredResource]) {
    let locally_modified: Vec<_> = updated_resources
        .iter()
        .filter(|r| r.locally_modified)
        .collect();
    if !locally_modified.is_empty() {
        println!(
            "{} {} resource(s) have been modified locally since last pull:",
            "WARNING:".yellow().bold(),
            locally_modified.len()
        );
        for r in &locally_modified {
            println!("  {} {} '{}'", "!".yellow(), r.kind.display_name(), r.name);
        }
        println!(
            "  Pulling will overwrite your local changes. Commit or stash them first if needed."
        );
        println!();
    }
}

/// Generate a single AI narrative covering all pull changes.
///
/// Returns `Some(narrative)` on success, `None` on failure (caller falls back to non-AI output).
pub(super) async fn generate_pull_narrative(
    ai_config: &hoist_core::config::AiConfig,
    new_resources: &[DiscoveredResource],
    updated_resources: &[DiscoveredResource],
    deleted_resources: &[(ResourceKind, String, PathBuf)],
    total_unchanged: usize,
) -> Option<String> {
    eprintln!("Generating AI explanation...");

    let pull_labels = Some(("locally", "on the server"));
    let mut contexts = Vec::new();

    // New resources (exist on server, not locally)
    for r in new_resources {
        let remote_content = format_for_ai_resource(r);
        contexts.push(ResourceContext {
            kind: r.kind,
            name: r.name.clone(),
            status: ChangeStatus::New,
            local_content: None,
            remote_content: Some(remote_content),
            descriptions: vec![],
        });
    }

    // Updated resources
    for r in updated_resources {
        if r.changes.is_empty() {
            continue;
        }
        let remote_content = format_for_ai_resource(r);
        let descriptions = describe_changes_plain(&r.changes, r.kind, &r.name, pull_labels);
        contexts.push(ResourceContext {
            kind: r.kind,
            name: r.name.clone(),
            status: ChangeStatus::Modified,
            local_content: None,
            remote_content: Some(remote_content),
            descriptions,
        });
    }

    // Deleted resources
    for (kind, name, _) in deleted_resources {
        contexts.push(ResourceContext {
            kind: *kind,
            name: name.clone(),
            status: ChangeStatus::Deleted,
            local_content: None,
            remote_content: None,
            descriptions: vec![],
        });
    }

    if contexts.is_empty() {
        return None;
    }

    match explain::explain_all_changes(ai_config, &contexts, "pull", total_unchanged).await {
        Ok(narrative) => Some(narrative),
        Err(e) => {
            eprintln!("Warning: AI explanation failed: {}", e);
            None
        }
    }
}

/// Format a discovered resource's content for the AI prompt.
fn format_for_ai_resource(r: &DiscoveredResource) -> String {
    if r.kind == ResourceKind::Agent {
        // Agents: use YAML representation (more readable for AI)
        hoist_core::resources::agent::agent_to_yaml(&r.raw_resource)
    } else {
        // Search resources: use the normalized JSON content
        r.json_content.clone()
    }
}

/// Print the final result line after pull completes.
pub(super) fn print_pull_result(upsert_count: usize, delete_count: usize, total_unchanged: usize) {
    println!();
    if upsert_count > 0 && delete_count > 0 {
        println!(
            "Pulled {} resource(s), deleted {}. {} already up to date.",
            upsert_count, delete_count, total_unchanged
        );
    } else if delete_count > 0 {
        println!(
            "Deleted {} resource(s). {} already up to date.",
            delete_count, total_unchanged
        );
    } else {
        println!(
            "Pulled {} resource(s). {} already up to date.",
            upsert_count, total_unchanged
        );
    }
}
