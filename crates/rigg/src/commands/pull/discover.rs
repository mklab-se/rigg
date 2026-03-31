//! Discovery phase: fetch resources from Azure and classify as new/updated/deleted.

use std::path::Path;

use anyhow::Result;
use serde_json::Value;
use tracing::info;

use rigg_client::FoundryClient;
use rigg_core::config::FoundryServiceConfig;
use rigg_core::normalize::{format_json, normalize};
use rigg_core::resources::agent::{agent_to_yaml, agent_volatile_fields, strip_agent_empty_fields};
use rigg_core::resources::managed::{self, ManagedMap};
use rigg_core::resources::{ResourceKind, validate_resource_name};
use rigg_core::state::Checksums;
use rigg_diff::Change;

use crate::commands::common::{ResourceSelection, get_volatile_fields, read_agent_yaml};

/// A resource discovered during the fetch phase, pending write.
pub(super) struct DiscoveredResource {
    pub kind: ResourceKind,
    pub name: String,
    pub json_content: String,
    pub new_checksum: String,
    pub raw_resource: Value,
    pub changes: Vec<Change>,
    /// True if the local file has been modified since last pull
    pub locally_modified: bool,
}

/// Discover search resources from fetched API results.
#[allow(clippy::too_many_arguments)]
pub(super) fn discover_search_resources(
    service_dir: &Path,
    fetched_results: &[(ResourceKind, Result<Vec<Value>, rigg_client::ClientError>)],
    selection: &ResourceSelection,
    filter: Option<&str>,
    checksums: &Checksums,
    managed_map: &ManagedMap,
    new_resources: &mut Vec<DiscoveredResource>,
    updated_resources: &mut Vec<DiscoveredResource>,
    deleted_resources: &mut Vec<(ResourceKind, String, std::path::PathBuf)>,
    total_unchanged: &mut usize,
    checksum_backfill: &mut Vec<(ResourceKind, String, String)>,
) -> Result<()> {
    for (kind, result) in fetched_results {
        let resources = result.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;

        // Build set of remote resource names (before filtering, for deletion detection)
        let all_remote_names: std::collections::HashSet<String> = resources
            .iter()
            .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();

        // Filter by singular flag (exact name match) and/or pattern (substring match)
        // For managed sub-resources pulled via --knowledge-sources, skip the selection
        // filter since they were implicitly included.
        let exact_name = selection.name_filter(*kind);
        let resources: Vec<&Value> = resources
            .iter()
            .filter(|r| {
                let name = r.get("name").and_then(|n| n.as_str()).unwrap_or("");
                if let Some(exact) = exact_name {
                    if name != exact {
                        return false;
                    }
                }
                if let Some(pattern) = filter {
                    if !name.contains(pattern) {
                        return false;
                    }
                }
                true
            })
            .collect();

        for resource in &resources {
            let name = resource
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| anyhow::anyhow!("Resource missing name field"))?;

            validate_resource_name(name)?;

            // Route file to correct directory based on managed map
            let resource_dir =
                service_dir.join(managed::resource_directory(*kind, name, managed_map));
            let filename = managed::resource_filename(*kind, name, managed_map);

            // Normalize the JSON
            let volatile_fields = get_volatile_fields(*kind);
            let normalized = normalize(resource, &volatile_fields);
            let json_content = format_json(&normalized);

            // Check if content changed (remote vs stored checksum) and file on disk matches
            let new_checksum = Checksums::calculate(&json_content);
            let file_path = resource_dir.join(&filename);
            let stored_checksum = checksums.get_managed(*kind, name, managed_map);
            let is_existing = stored_checksum.is_some() || file_path.exists();
            let local_matches = file_path.exists()
                && std::fs::read_to_string(&file_path).ok().as_deref()
                    == Some(json_content.as_str());

            if local_matches {
                *total_unchanged += 1;
                if stored_checksum.is_none() {
                    checksum_backfill.push((*kind, name.to_string(), new_checksum));
                }
                continue;
            }

            // Check if local file was modified since last pull
            let locally_modified = if let Some(stored) = stored_checksum {
                if let Ok(disk_content) = std::fs::read_to_string(&file_path) {
                    let disk_checksum = Checksums::calculate(&disk_content);
                    disk_checksum != *stored
                } else {
                    false
                }
            } else {
                false
            };

            // Compute diff for updated resources (compare current local vs incoming server)
            let changes = if file_path.exists() {
                std::fs::read_to_string(&file_path)
                    .ok()
                    .and_then(|content| serde_json::from_str::<Value>(&content).ok())
                    .map(|local_value| rigg_diff::diff(&local_value, &normalized, "name").changes)
                    .unwrap_or_default()
            } else {
                vec![]
            };

            let entry = DiscoveredResource {
                kind: *kind,
                name: name.to_string(),
                json_content,
                new_checksum,
                raw_resource: (*resource).clone(),
                changes,
                locally_modified,
            };

            if is_existing {
                updated_resources.push(entry);
            } else {
                new_resources.push(entry);
            }
        }

        // Detect local files whose resources were deleted on the server
        // For knowledge sources, scan subdirectories
        if *kind == ResourceKind::KnowledgeSource {
            let ks_base_dir = service_dir.join("agentic-retrieval/knowledge-sources");
            if ks_base_dir.exists() {
                for entry in std::fs::read_dir(&ks_base_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if let Some(pattern) = filter {
                        if !name.contains(pattern) {
                            continue;
                        }
                    }
                    if !all_remote_names.contains(&name)
                        && checksums.get_managed(*kind, &name, managed_map).is_some()
                    {
                        // Delete entire KS directory (includes managed sub-resources)
                        deleted_resources.push((*kind, name, path));
                    }
                }
            }
        } else {
            // For other resources, scan the appropriate directories
            // Check standalone directory
            let standalone_dir = service_dir.join(kind.directory_name());
            if standalone_dir.exists() {
                for entry in std::fs::read_dir(&standalone_dir)? {
                    let entry = entry?;
                    let path = entry.path();

                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }

                    let name = match path.file_stem().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };

                    if let Some(pattern) = filter {
                        if !name.contains(pattern) {
                            continue;
                        }
                    }

                    // Skip managed resources when scanning standalone dirs
                    if managed::managing_ks(managed_map, *kind, &name).is_some() {
                        continue;
                    }

                    if !all_remote_names.contains(&name)
                        && checksums.get_managed(*kind, &name, managed_map).is_some()
                    {
                        deleted_resources.push((*kind, name, path));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Discover Foundry agents from the API and prepare them for writing.
#[allow(clippy::too_many_arguments)]
pub(super) async fn discover_foundry_agents(
    agents_dir: &Path,
    foundry_config: &FoundryServiceConfig,
    selection: &ResourceSelection,
    filter: Option<&str>,
    checksums: &Checksums,
    new_resources: &mut Vec<DiscoveredResource>,
    updated_resources: &mut Vec<DiscoveredResource>,
    deleted_resources: &mut Vec<(ResourceKind, String, std::path::PathBuf)>,
    total_unchanged: &mut usize,
    checksum_backfill: &mut Vec<(ResourceKind, String, String)>,
) -> Result<()> {
    let client = FoundryClient::new(foundry_config)?;
    info!(
        "Connected to Foundry {}/{} using {}",
        foundry_config.name,
        foundry_config.project,
        client.auth_method()
    );

    let agents = client.list_agents().await?;
    let kind = ResourceKind::Agent;

    let all_remote_names: std::collections::HashSet<String> = agents
        .iter()
        .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect();

    let exact_name = selection.name_filter(kind);
    let agents: Vec<&Value> = agents
        .iter()
        .filter(|a| {
            let name = a.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if let Some(exact) = exact_name {
                if name != exact {
                    return false;
                }
            }
            if let Some(pattern) = filter {
                if !name.contains(pattern) {
                    return false;
                }
            }
            true
        })
        .collect();

    for agent in &agents {
        let name = agent
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| anyhow::anyhow!("Agent missing name field"))?;

        validate_resource_name(name)?;

        // Strip volatile fields and create a canonical representation for checksumming
        let volatile = agent_volatile_fields();
        let normalized = normalize(agent, volatile);
        let json_content = format_json(&normalized);

        let new_checksum = Checksums::calculate(&json_content);
        let stored_checksum = checksums.get(kind, name);

        // Check if local YAML file matches what Azure would produce
        let yaml_path = agents_dir.join(format!("{}.yaml", name));
        let yaml_for_disk = agent_to_yaml(agent);
        let is_existing = stored_checksum.is_some() || yaml_path.exists();
        let local_matches = yaml_path.exists()
            && std::fs::read_to_string(&yaml_path).ok().as_deref() == Some(yaml_for_disk.as_str());

        if local_matches {
            *total_unchanged += 1;
            if stored_checksum.is_none() {
                checksum_backfill.push((kind, name.to_string(), new_checksum));
            }
            continue;
        }

        // Check if local YAML was modified since last pull
        let locally_modified = if let Some(stored) = stored_checksum {
            if let Ok(disk_content) = std::fs::read_to_string(&yaml_path) {
                let disk_checksum = Checksums::calculate(&disk_content);
                disk_checksum != *stored
            } else {
                false
            }
        } else {
            false
        };

        // Compute diff for updated agents (compare current local vs incoming server)
        let changes = if yaml_path.exists() {
            read_agent_yaml(&yaml_path)
                .ok()
                .map(|mut local_value| {
                    if let Some(obj) = local_value.as_object_mut() {
                        obj.insert(
                            "name".to_string(),
                            serde_json::Value::String(name.to_string()),
                        );
                    }
                    strip_agent_empty_fields(&mut local_value);
                    let local_normalized = normalize(&local_value, volatile);
                    let mut remote_cleaned = (*agent).clone();
                    strip_agent_empty_fields(&mut remote_cleaned);
                    let remote_normalized = normalize(&remote_cleaned, volatile);
                    rigg_diff::diff(&local_normalized, &remote_normalized, "name").changes
                })
                .unwrap_or_default()
        } else {
            vec![]
        };

        let entry = DiscoveredResource {
            kind,
            name: name.to_string(),
            json_content,
            new_checksum,
            raw_resource: (*agent).clone(),
            changes,
            locally_modified,
        };

        if is_existing {
            updated_resources.push(entry);
        } else {
            new_resources.push(entry);
        }
    }

    // Detect deleted agents (scan for .yaml files)
    if agents_dir.exists() {
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
            if let Some(pattern) = filter {
                if !name.contains(pattern) {
                    continue;
                }
            }
            if !all_remote_names.contains(&name) && checksums.get(kind, &name).is_some() {
                deleted_resources.push((kind, name, path));
            }
        }
    }

    Ok(())
}
