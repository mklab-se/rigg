//! Push reporting — display diffs, conflict warnings, and recreate prompts.

use std::collections::HashMap;

use colored::Colorize;

use hoist_core::resources::ResourceKind;
use hoist_core::state::Checksums;
use hoist_diff::Change;

use crate::commands::confirm::prompt_yes_no;
use crate::commands::describe::describe_changes;

/// Print the non-AI push plan listing all resources to be created/updated.
pub(super) fn print_push_plan(
    resources_to_push: &[(ResourceKind, String, serde_json::Value, bool)],
    change_details: &HashMap<(ResourceKind, String), Vec<Change>>,
    total_unchanged: usize,
) {
    let push_labels = Some(("on the server", "locally"));
    println!("Resources to push:");

    let mut prev_had_details = false;
    for (kind, name, _, exists) in resources_to_push {
        if *exists {
            if let Some(changes) = change_details.get(&(*kind, name.clone())) {
                if prev_had_details {
                    println!();
                }
                println!(
                    "  {} {} '{}' will be updated on the server with {} change{}:",
                    "~".yellow(),
                    kind.display_name(),
                    name,
                    changes.len(),
                    if changes.len() == 1 { "" } else { "s" }
                );
                for line in describe_changes(changes, *kind, name, push_labels, true) {
                    println!("{}", line);
                }
                prev_had_details = true;
            } else {
                println!(
                    "  {} {} '{}' will be updated on the server",
                    "~".yellow(),
                    kind.display_name(),
                    name
                );
                prev_had_details = false;
            }
        } else {
            println!(
                "  {} {} '{}' will be created on the server",
                "+".green(),
                kind.display_name(),
                name
            );
            prev_had_details = false;
        }
    }
    if total_unchanged > 0 {
        println!(
            "  {} resource(s) already match the server",
            total_unchanged.to_string().dimmed()
        );
    }
}

/// Print warnings about resources that changed on the server since the last pull.
pub(super) fn print_conflict_warnings(
    remote_conflicts: &[(ResourceKind, String)],
    resources_to_push: &[(ResourceKind, String, serde_json::Value, bool)],
) {
    if remote_conflicts.is_empty() {
        return;
    }

    // Filter to only conflicts that are actually being pushed
    let push_set: std::collections::HashSet<(ResourceKind, String)> = resources_to_push
        .iter()
        .map(|(k, n, _, _)| (*k, n.clone()))
        .collect();
    let active_conflicts: Vec<_> = remote_conflicts
        .iter()
        .filter(|(k, n)| push_set.contains(&(*k, n.clone())))
        .collect();
    if !active_conflicts.is_empty() {
        println!(
            "{} {} resource(s) changed on the server since your last pull:",
            "WARNING:".yellow().bold(),
            active_conflicts.len()
        );
        for (kind, name) in &active_conflicts {
            println!("  {} {} '{}'", "!".yellow(), kind.display_name(), name);
        }
        println!(
            "  Pushing will overwrite those remote changes. Run {} first to review.",
            "hoist pull".bold()
        );
        println!();
    }
}

/// Prompt for drop-and-recreate confirmation and re-read local definitions.
///
/// Returns `Ok(())` if the user confirms (or `force` is set), adding the
/// recreate resources to `resources_to_push`. Returns an error if the user
/// declines.
pub(super) fn handle_recreate_candidates(
    recreate_candidates: &[(ResourceKind, String)],
    resources_to_push: &mut Vec<(ResourceKind, String, serde_json::Value, bool)>,
    service_dir: &std::path::Path,
    force: bool,
) -> anyhow::Result<()> {
    if recreate_candidates.is_empty() {
        return Ok(());
    }

    println!();
    println!(
        "{} resource(s) have immutable field changes that require drop-and-recreate:",
        recreate_candidates.len()
    );
    for (kind, name) in recreate_candidates {
        println!("  {} {} '{}'", "!".red(), kind.display_name(), name);
    }
    println!();
    println!("WARNING: Drop-and-recreate will DELETE these resources and their data.");
    println!("Re-indexing will be required after recreation.");
    if !force && !prompt_yes_no("Drop and recreate these resources?")? {
        anyhow::bail!("Push blocked: immutable field changes require drop-and-recreate.");
    }

    // Mark these for drop-and-recreate during push execution
    for (kind, name) in recreate_candidates {
        // Re-read the local file to get the definition
        let resource_dir = service_dir.join(kind.directory_name());
        let path = resource_dir.join(format!("{}.json", name));
        if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            let local: serde_json::Value = serde_json::from_str(&content)?;
            // Push with exists=true so it triggers the drop-and-recreate path
            resources_to_push.push((*kind, name.clone(), local, true));
        }
    }

    Ok(())
}

/// Check for remote conflicts on an agent by comparing checksums.
///
/// If the remote agent's checksum differs from the stored baseline,
/// it means the remote changed since the last pull.
pub(super) fn check_agent_remote_conflict(
    normalized_remote: &serde_json::Value,
    name: &str,
    checksums: &Checksums,
    remote_conflicts: &mut Vec<(ResourceKind, String)>,
) {
    let remote_agent_json = hoist_core::normalize::format_json(normalized_remote);
    let remote_agent_checksum = Checksums::calculate(&remote_agent_json);
    if let Some(stored) = checksums.get(ResourceKind::Agent, name) {
        if *stored != remote_agent_checksum {
            remote_conflicts.push((ResourceKind::Agent, name.to_string()));
        }
    }
}
