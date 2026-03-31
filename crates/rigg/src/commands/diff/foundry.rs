//! Foundry agent diffing: compare local YAML files against remote agents.

use anyhow::Result;

use rigg_core::config::ResolvedEnvironment;
use rigg_core::normalize::normalize;
use rigg_core::resources::ResourceKind;
use rigg_core::resources::agent::{agent_volatile_fields, strip_agent_empty_fields};
use tracing::info;

use crate::commands::common::{ResourceSelection, read_agent_yaml};
use crate::commands::explain::format_for_ai;

use super::ResourceDiff;

/// Diff all local Foundry agents against the remote service.
pub(super) async fn diff_foundry_agents(
    env: &ResolvedEnvironment,
    files_root: &std::path::Path,
    selection: &ResourceSelection,
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    let exact_name = selection.name_filter(ResourceKind::Agent);
    let volatile = agent_volatile_fields();

    for foundry_config in &env.foundry {
        eprintln!(
            "Comparing local and remote agents on {}/{}...",
            foundry_config.name, foundry_config.project
        );
        let foundry_client = rigg_client::FoundryClient::new(foundry_config)?;
        info!(
            "Connected to Foundry {}/{} using {}",
            foundry_config.name,
            foundry_config.project,
            foundry_client.auth_method()
        );

        let agents_dir = env
            .foundry_service_dir(files_root, foundry_config)
            .join("agents");

        // Fetch all remote agents for remote-only detection
        let remote_agents = foundry_client.list_agents().await?;
        let remote_names: std::collections::HashSet<String> = remote_agents
            .iter()
            .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();

        // Diff local agents against remote
        if agents_dir.exists() {
            diff_local_agents(
                &agents_dir,
                exact_name,
                volatile,
                &remote_agents,
                all_diffs,
                has_changes,
            )?;
        }

        // Check for remote-only agents
        detect_remote_only_agents(&agents_dir, &remote_names, all_diffs);
    }

    Ok(())
}

/// Diff local agent YAML files against their remote counterparts.
fn diff_local_agents(
    agents_dir: &std::path::Path,
    exact_name: Option<&str>,
    volatile: &[&str],
    remote_agents: &[serde_json::Value],
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    for entry in std::fs::read_dir(agents_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
            continue;
        }

        let name = match path.file_stem().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if let Some(exact) = exact_name {
            if name != exact {
                continue;
            }
        }

        // Read agent YAML and inject name for comparison
        let mut local_value = read_agent_yaml(&path)?;
        if let Some(obj) = local_value.as_object_mut() {
            obj.insert("name".to_string(), serde_json::Value::String(name.clone()));
        }
        strip_agent_empty_fields(&mut local_value);
        let local_normalized = normalize(&local_value, volatile);

        let resource_id = format!("agents/{}", name);

        // Find matching remote agent
        let remote_agent = remote_agents
            .iter()
            .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(&name));

        match remote_agent {
            Some(remote) => {
                let mut remote_cleaned = remote.clone();
                strip_agent_empty_fields(&mut remote_cleaned);
                let remote_normalized = normalize(&remote_cleaned, volatile);
                let diff_result = rigg_diff::diff(&local_normalized, &remote_normalized, "name");

                let has_diff = !diff_result.is_equal;
                if has_diff {
                    *has_changes = true;
                }

                all_diffs.push(ResourceDiff {
                    kind: ResourceKind::Agent,
                    resource_name: name.clone(),
                    display_id: resource_id,
                    result: diff_result,
                    local_content: if has_diff {
                        Some(format_for_ai(ResourceKind::Agent, &local_normalized))
                    } else {
                        None
                    },
                    remote_content: if has_diff {
                        Some(format_for_ai(ResourceKind::Agent, &remote_normalized))
                    } else {
                        None
                    },
                });
            }
            None => {
                // Local only - will be created
                *has_changes = true;
                let local_ai = format_for_ai(ResourceKind::Agent, &local_normalized);
                all_diffs.push(ResourceDiff {
                    kind: ResourceKind::Agent,
                    resource_name: name.clone(),
                    display_id: resource_id,
                    result: rigg_diff::DiffResult {
                        is_equal: false,
                        changes: vec![rigg_diff::Change {
                            path: ".".to_string(),
                            kind: rigg_diff::ChangeKind::Added,
                            old_value: None,
                            new_value: Some(local_normalized),
                            description: None,
                        }],
                    },
                    local_content: Some(local_ai),
                    remote_content: None,
                });
            }
        }
    }
    Ok(())
}

/// Detect agents that exist on the remote but not locally.
fn detect_remote_only_agents(
    agents_dir: &std::path::Path,
    remote_names: &std::collections::HashSet<String>,
    all_diffs: &mut Vec<ResourceDiff>,
) {
    for remote_name in remote_names {
        let local_yaml = agents_dir.join(format!("{}.yaml", remote_name));
        if !local_yaml.exists() {
            let resource_id = format!("agents/{}", remote_name);
            if all_diffs.iter().any(|d| d.display_id == resource_id) {
                continue;
            }
            all_diffs.push(ResourceDiff {
                kind: ResourceKind::Agent,
                resource_name: remote_name.clone(),
                display_id: resource_id,
                result: rigg_diff::DiffResult {
                    is_equal: true,
                    changes: vec![],
                },
                local_content: None,
                remote_content: None,
            });
        }
    }
}
