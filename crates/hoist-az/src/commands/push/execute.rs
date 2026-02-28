//! Push execution — sends resources to Azure and syncs local files with server canonical form.

use std::io::{self, Write};
use std::path::Path;

use anyhow::Result;
use colored::Colorize;

use hoist_client::AzureSearchClient;
use hoist_core::normalize::{format_json, normalize};
use hoist_core::resources::ResourceKind;
use hoist_core::resources::agent::{agent_to_yaml, agent_volatile_fields};
use hoist_core::resources::managed::{self, ManagedMap};
use hoist_core::service::ServiceDomain;
use hoist_core::state::{Checksums, LocalState, ResourceState};

use crate::commands::common::get_volatile_fields;

use super::collect::{build_local_managed_map, is_ks_recreation_bug};
use super::credentials::{inject_credentials, needs_credentials, strip_volatile_fields};

/// Push search resources to Azure.
///
/// Handles drop-and-recreate, credential injection, and the known KS recreation bug.
/// Returns `(success_count, error_count, pushed_resources)`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn push_search_resources(
    search_resources: &[&(ResourceKind, String, serde_json::Value, bool)],
    recreate_candidates: &[(ResourceKind, String)],
    env: &hoist_core::config::ResolvedEnvironment,
    cached_connection_string: &mut Option<String>,
) -> Result<(usize, usize, Vec<(ResourceKind, String)>)> {
    let mut success_count = 0;
    let mut error_count = 0;
    let mut pushed_resources = Vec::new();

    let search_svc = env
        .primary_search_service()
        .ok_or_else(|| anyhow::anyhow!("No search service in environment"))?;
    let client = AzureSearchClient::from_service_config(search_svc)?;

    for (kind, name, definition, exists) in search_resources {
        let needs_recreate = recreate_candidates.contains(&(*kind, name.clone()));

        if needs_recreate {
            print!("Dropping {} '{}'... ", kind.display_name(), name);
            io::stdout().flush()?;
            match client.delete(*kind, name).await {
                Ok(_) => println!("done"),
                Err(e) => {
                    println!("FAILED: {}", e);
                    error_count += 1;
                    continue;
                }
            }
        }

        let action = if needs_recreate {
            "Recreating"
        } else if *exists {
            "Updating"
        } else {
            "Creating"
        };
        print!("{} {} '{}'... ", action, kind.display_name(), name);
        io::stdout().flush()?;

        let clean_definition = strip_volatile_fields(*kind, definition);

        // For new data sources/KS, inject credentials if missing
        let final_definition =
            if needs_credentials(*kind, &clean_definition, *exists, needs_recreate) {
                inject_credentials(
                    *kind,
                    &clean_definition,
                    name,
                    env,
                    cached_connection_string,
                )
                .await?
            } else {
                clean_definition
            };

        match client
            .create_or_update(*kind, name, &final_definition)
            .await
        {
            Ok(_) => {
                println!("done");
                success_count += 1;
                pushed_resources.push((*kind, name.clone()));
            }
            Err(e) => {
                println!("FAILED: {}", e);
                // Provide KS-specific guidance for known Azure bug
                if *kind == ResourceKind::KnowledgeSource && is_ks_recreation_bug(&e.to_string()) {
                    println!();
                    println!(
                        "  {} Knowledge source '{}' failed because managed sub-resources",
                        "KNOWN AZURE LIMITATION:".yellow().bold(),
                        name
                    );
                    println!("  (index, indexer, data source, skillset) already exist on Azure.");
                    println!("  Azure manages these sub-resources as part of the knowledge source");
                    println!("  lifecycle — they should not be created separately.");
                    println!();
                    println!("  Workaround — delete from Azure, then retry:");
                    println!(
                        "    1. hoist delete --knowledgesource {} --target remote  (deletes from '{}' on Azure)",
                        name, env.name
                    );
                    println!(
                        "    2. hoist push --knowledgesources                      (recreates in '{}' on Azure)",
                        env.name
                    );
                    println!();
                    println!(
                        "  {} This deletes the search index and all its data.",
                        "WARNING:".yellow().bold()
                    );
                    println!(
                        "  Re-indexing occurs automatically but takes time and may incur costs."
                    );
                }
                error_count += 1;
            }
        }
    }

    Ok((success_count, error_count, pushed_resources))
}

/// Push Foundry agents to Azure.
///
/// Returns `(success_count, error_count, pushed_resources)`.
pub(super) async fn push_foundry_agents(
    foundry_resources: &[&(ResourceKind, String, serde_json::Value, bool)],
    env: &hoist_core::config::ResolvedEnvironment,
) -> Result<(usize, usize, Vec<(ResourceKind, String)>)> {
    let mut success_count = 0;
    let mut error_count = 0;
    let mut pushed_resources = Vec::new();

    for foundry_config in &env.foundry {
        let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;

        for (kind, name, definition, exists) in foundry_resources {
            let action = if *exists { "Updating" } else { "Creating" };
            print!("{} {} '{}'... ", action, kind.display_name(), name);
            io::stdout().flush()?;

            // Both create and update use the versions endpoint;
            // update_agent creates a new version of an existing agent.
            let result = if *exists {
                foundry_client.update_agent(name, definition).await
            } else {
                foundry_client.create_agent(definition).await
            };

            match result {
                Ok(_response) => {
                    println!("done");
                    success_count += 1;
                    pushed_resources.push((*kind, name.clone()));
                }
                Err(e) => {
                    println!("FAILED: {}", e);
                    error_count += 1;
                }
            }
        }
    }

    Ok((success_count, error_count, pushed_resources))
}

/// Re-fetch pushed resources to sync local files with server canonical form.
///
/// This eliminates false diffs after push by ensuring local files match exactly what Azure returns.
pub(super) async fn pullback_synced_resources(
    pushed_resources: &[(ResourceKind, String)],
    project_root: &Path,
    files_root: &Path,
    env: &hoist_core::config::ResolvedEnvironment,
) -> Result<()> {
    println!(
        "Syncing {} pushed resource(s) with server canonical form...",
        pushed_resources.len()
    );
    let mut checksums = Checksums::load_env(project_root, &env.name).unwrap_or_default();
    let mut state = LocalState::load_env(project_root, &env.name).unwrap_or_default();
    let managed_map = if let Some(svc) = env.primary_search_service() {
        build_local_managed_map(&env.search_service_dir(files_root, svc))
    } else {
        ManagedMap::new()
    };

    let search_pushed: Vec<_> = pushed_resources
        .iter()
        .filter(|(k, _)| k.domain() == ServiceDomain::Search)
        .collect();
    let agent_pushed: Vec<_> = pushed_resources
        .iter()
        .filter(|(k, _)| k.domain() == ServiceDomain::Foundry)
        .collect();

    // Pull-back search resources
    if !search_pushed.is_empty() {
        if let Some(search_svc) = env.primary_search_service() {
            let client = AzureSearchClient::from_service_config(search_svc)?;
            let service_dir = env.search_service_dir(files_root, search_svc);

            for (kind, name) in &search_pushed {
                if let Ok(remote) = client.get(*kind, name).await {
                    let volatile_fields = get_volatile_fields(*kind);
                    let normalized = normalize(&remote, &volatile_fields);
                    let json_content = format_json(&normalized);
                    let new_checksum = Checksums::calculate(&json_content);

                    // Write the canonical file
                    let resource_dir =
                        service_dir.join(managed::resource_directory(*kind, name, &managed_map));
                    std::fs::create_dir_all(&resource_dir)?;
                    let filename = managed::resource_filename(*kind, name, &managed_map);
                    let file_path = resource_dir.join(&filename);
                    std::fs::write(&file_path, &json_content)?;

                    // Update checksums and state
                    let etag = remote
                        .get("@odata.etag")
                        .and_then(|e| e.as_str())
                        .map(String::from);
                    checksums.set_managed(*kind, name, new_checksum.clone(), &managed_map);
                    state.set_managed(
                        *kind,
                        name,
                        ResourceState {
                            kind: *kind,
                            etag,
                            checksum: new_checksum,
                            synced_at: chrono::Utc::now(),
                        },
                        &managed_map,
                    );
                }
            }
        }
    }

    // Pull-back agents
    if !agent_pushed.is_empty() {
        if let Some(foundry_config) = env.foundry.first() {
            let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;
            let agents_dir = env
                .foundry_service_dir(files_root, foundry_config)
                .join("agents");
            std::fs::create_dir_all(&agents_dir)?;

            for (_kind, name) in &agent_pushed {
                if let Ok(remote) = foundry_client.get_agent(name).await {
                    // Write canonical YAML
                    let yaml_content = agent_to_yaml(&remote);
                    let yaml_path = agents_dir.join(format!("{}.yaml", name));
                    std::fs::write(&yaml_path, &yaml_content)?;

                    // Calculate checksum from normalized JSON (same as pull)
                    let volatile = agent_volatile_fields();
                    let normalized = normalize(&remote, volatile);
                    let json_content = format_json(&normalized);
                    let new_checksum = Checksums::calculate(&json_content);

                    checksums.set(ResourceKind::Agent, name, new_checksum.clone());
                    state.set(
                        ResourceKind::Agent,
                        name,
                        ResourceState {
                            kind: ResourceKind::Agent,
                            etag: None,
                            checksum: new_checksum,
                            synced_at: chrono::Utc::now(),
                        },
                    );
                }
            }
        }
    }

    checksums.save_env(project_root, &env.name)?;
    state.save_env(project_root, &env.name)?;

    Ok(())
}
