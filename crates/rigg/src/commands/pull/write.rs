//! Write phase: persist discovered resources to disk and update state.

use std::path::Path;

use anyhow::Result;
use tracing::info;

use rigg_core::config::{FoundryServiceConfig, ResolvedEnvironment};
use rigg_core::resources::ResourceKind;
use rigg_core::resources::agent::agent_to_yaml;
use rigg_core::resources::managed::{self, ManagedMap};
use rigg_core::state::{Checksums, LocalState, ResourceState};

use super::discover::DiscoveredResource;

/// Write all new/updated resources to disk and update state files.
///
/// Returns the count of upserted resources.
#[allow(clippy::too_many_arguments)]
pub(super) fn write_resources(
    project_root: &Path,
    files_root: &Path,
    env: &ResolvedEnvironment,
    new_resources: Vec<DiscoveredResource>,
    updated_resources: Vec<DiscoveredResource>,
    deleted_resources: &[(ResourceKind, String, std::path::PathBuf)],
    managed_map: &ManagedMap,
    total_unchanged: usize,
) -> Result<(usize, usize)> {
    let mut state = LocalState::load_env(project_root, &env.name)?;
    let mut checksums = Checksums::load_env(project_root, &env.name)?;

    let all_upserts: Vec<_> = new_resources.into_iter().chain(updated_resources).collect();

    // Determine search service dir for writing files
    let write_service_dir = env
        .primary_search_service()
        .map(|svc| env.search_service_dir(files_root, svc));

    for entry in &all_upserts {
        if entry.kind == ResourceKind::Agent {
            // Agent: write YAML file
            write_agent_yaml(env, files_root, entry)?;
        } else {
            // Search resource: write to managed-aware path
            let service_dir = write_service_dir
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("No search service configured"))?;
            let resource_dir = service_dir.join(managed::resource_directory(
                entry.kind,
                &entry.name,
                managed_map,
            ));
            std::fs::create_dir_all(&resource_dir)?;

            let filename = managed::resource_filename(entry.kind, &entry.name, managed_map);
            let file_path = resource_dir.join(&filename);
            std::fs::write(&file_path, &entry.json_content)?;
            info!("Wrote {}", file_path.display());
        }

        // Update state
        let etag = entry
            .raw_resource
            .get("@odata.etag")
            .and_then(|e| e.as_str())
            .map(String::from);

        state.set_managed(
            entry.kind,
            &entry.name,
            ResourceState {
                kind: entry.kind,
                etag,
                checksum: entry.new_checksum.clone(),
                synced_at: chrono::Utc::now(),
            },
            managed_map,
        );
        checksums.set_managed(
            entry.kind,
            &entry.name,
            entry.new_checksum.clone(),
            managed_map,
        );
    }

    // Delete local files for resources removed on server
    for (kind, name, path) in deleted_resources {
        if *kind == ResourceKind::Agent {
            // Agent: remove YAML file
            if path.exists() {
                std::fs::remove_file(path)?;
                info!("Deleted agent file {}", path.display());
            }
        } else if *kind == ResourceKind::KnowledgeSource && path.is_dir() {
            // KS: remove entire directory (includes managed sub-resources)
            std::fs::remove_dir_all(path)?;
            info!("Deleted knowledge source directory {}", path.display());
        } else {
            std::fs::remove_file(path)?;
            info!("Deleted {}", path.display());
        }
        state.remove_managed(*kind, name, managed_map);
        checksums.remove_managed(*kind, name, managed_map);
    }

    // Save state
    state.last_sync = Some(chrono::Utc::now());
    state.save_env(project_root, &env.name)?;
    checksums.save_env(project_root, &env.name)?;

    let upsert_count = all_upserts.len();
    let delete_count = deleted_resources.len();
    let _ = total_unchanged; // Used by caller for output

    Ok((upsert_count, delete_count))
}

/// Write an agent YAML file to disk.
fn write_agent_yaml(
    env: &ResolvedEnvironment,
    files_root: &Path,
    entry: &DiscoveredResource,
) -> Result<()> {
    // Use the first configured Foundry service for directory path
    let foundry_config = env
        .foundry
        .first()
        .ok_or_else(|| anyhow::anyhow!("No Foundry service configured"))?;

    let agents_dir = foundry_agents_dir(env, files_root, foundry_config);
    std::fs::create_dir_all(&agents_dir)?;

    let yaml_content = agent_to_yaml(&entry.raw_resource);
    let yaml_path = agents_dir.join(format!("{}.yaml", entry.name));
    std::fs::write(&yaml_path, &yaml_content)?;

    // Clean up old-format directory if it exists (one-time migration)
    let old_dir = agents_dir.join(&entry.name);
    if old_dir.is_dir() {
        std::fs::remove_dir_all(&old_dir)?;
        info!("Removed old agent directory {}", old_dir.display());
    }

    info!("Wrote agent {}", yaml_path.display());
    Ok(())
}

/// Get the directory path for Foundry agents.
pub(super) fn foundry_agents_dir(
    env: &ResolvedEnvironment,
    files_root: &Path,
    foundry_config: &FoundryServiceConfig,
) -> std::path::PathBuf {
    env.foundry_service_dir(files_root, foundry_config)
        .join("agents")
}

/// Backfill checksums for files that match Azure but had no stored checksum.
///
/// This happens when resource files come from git but `.rigg/` state is gitignored.
pub(super) fn backfill_checksums(
    project_root: &Path,
    env_name: &str,
    checksum_backfill: &[(ResourceKind, String, String)],
    managed_map: &ManagedMap,
) -> Result<()> {
    if checksum_backfill.is_empty() {
        return Ok(());
    }

    let mut bf_state = LocalState::load_env(project_root, env_name)?;
    let mut bf_checksums = Checksums::load_env(project_root, env_name)?;
    for (kind, name, checksum) in checksum_backfill {
        if kind.domain() == rigg_core::service::ServiceDomain::Foundry {
            bf_state.set(
                *kind,
                name,
                ResourceState {
                    kind: *kind,
                    etag: None,
                    checksum: checksum.clone(),
                    synced_at: chrono::Utc::now(),
                },
            );
            bf_checksums.set(*kind, name, checksum.clone());
        } else {
            bf_state.set_managed(
                *kind,
                name,
                ResourceState {
                    kind: *kind,
                    etag: None,
                    checksum: checksum.clone(),
                    synced_at: chrono::Utc::now(),
                },
                managed_map,
            );
            bf_checksums.set_managed(*kind, name, checksum.clone(), managed_map);
        }
    }
    bf_state.last_sync = Some(chrono::Utc::now());
    bf_state.save_env(project_root, env_name)?;
    bf_checksums.save_env(project_root, env_name)?;

    Ok(())
}
