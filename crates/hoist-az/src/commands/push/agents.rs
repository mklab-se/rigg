//! Foundry agent collection — reads local agent YAML files and compares against Azure.

use std::collections::HashMap;

use hoist_core::normalize::normalize;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::agent::{agent_volatile_fields, strip_agent_empty_fields};
use hoist_core::state::Checksums;
use hoist_diff::Change;

use crate::commands::common::{ResourceSelection, read_agent_yaml};

use super::report::check_agent_remote_conflict;

/// Collect Foundry agents that need to be pushed, comparing local YAML against remote.
///
/// Reads agent YAML files from the agents directory, compares them against remote agents,
/// and populates the push/change tracking collections.
#[allow(clippy::too_many_arguments)]
pub(super) async fn collect_foundry_agents(
    env: &hoist_core::config::ResolvedEnvironment,
    files_root: &std::path::Path,
    selection: &ResourceSelection,
    filter: &Option<String>,
    resources_to_push: &mut Vec<(ResourceKind, String, serde_json::Value, bool)>,
    total_unchanged: &mut usize,
    change_details: &mut HashMap<(ResourceKind, String), Vec<Change>>,
    remote_values: &mut HashMap<(ResourceKind, String), serde_json::Value>,
    checksums: &Checksums,
    remote_conflicts: &mut Vec<(ResourceKind, String)>,
) -> anyhow::Result<()> {
    for foundry_config in &env.foundry {
        eprintln!(
            "Comparing local agents against {}/{}...",
            foundry_config.name, foundry_config.project
        );
        let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;
        tracing::info!(
            "Connected to Foundry {}/{} using {}",
            foundry_config.name,
            foundry_config.project,
            foundry_client.auth_method()
        );

        let agents_dir = env
            .foundry_service_dir(files_root, foundry_config)
            .join("agents");

        if !agents_dir.exists() {
            continue;
        }

        // Get existing agents for diffing
        let existing_agents = foundry_client.list_agents().await?;

        // Build name->remote_agent map for diffing
        let remote_agent_map: HashMap<String, &serde_json::Value> = existing_agents
            .iter()
            .filter_map(|a| {
                let name = a.get("name")?.as_str()?.to_string();
                Some((name, a))
            })
            .collect();

        let volatile = agent_volatile_fields();

        for entry in std::fs::read_dir(&agents_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }

            let name = match path.file_stem().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };

            if let Some(exact_name) = selection.name_filter(ResourceKind::Agent) {
                if name != exact_name {
                    continue;
                }
            }
            if let Some(pattern) = filter {
                if !name.contains(pattern) {
                    continue;
                }
            }

            // Read agent YAML and inject name for API use
            let mut payload = read_agent_yaml(&path)?;
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("name".to_string(), serde_json::Value::String(name.clone()));
            }

            // Compare local vs remote to skip unchanged agents
            strip_agent_empty_fields(&mut payload);
            match remote_agent_map.get(&name) {
                Some(remote) => {
                    let mut remote_cleaned = (*remote).clone();
                    strip_agent_empty_fields(&mut remote_cleaned);
                    let normalized_local = normalize(&payload, volatile);
                    let normalized_remote = normalize(&remote_cleaned, volatile);

                    // Check for remote conflict on agents
                    check_agent_remote_conflict(
                        &normalized_remote,
                        &name,
                        checksums,
                        remote_conflicts,
                    );

                    if normalized_local == normalized_remote {
                        *total_unchanged += 1;
                    } else {
                        let diff_result =
                            hoist_diff::diff(&normalized_remote, &normalized_local, "name");
                        change_details
                            .insert((ResourceKind::Agent, name.clone()), diff_result.changes);
                        remote_values
                            .insert((ResourceKind::Agent, name.clone()), normalized_remote);
                        resources_to_push.push((ResourceKind::Agent, name, payload, true));
                    }
                }
                None => {
                    // New agent -- will be created
                    resources_to_push.push((ResourceKind::Agent, name, payload, false));
                }
            }
        }
    }

    Ok(())
}
