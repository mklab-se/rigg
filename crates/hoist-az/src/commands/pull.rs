//! Pull resources from Azure

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::info;

use colored::Colorize;

use hoist_client::AzureSearchClient;
use hoist_core::config::{FoundryServiceConfig, ResolvedEnvironment};
use hoist_core::normalize::{format_json, normalize};
use hoist_core::resources::agent::{agent_to_yaml, agent_volatile_fields};
use hoist_core::resources::managed::{self, ManagedMap};
use hoist_core::resources::{ResourceKind, validate_resource_name};
use hoist_core::service::ServiceDomain;
use hoist_core::state::{Checksums, LocalState, ResourceState};
use hoist_diff::Change;

use crate::cli::ResourceTypeFlags;
use crate::commands::common::{
    ResourceSelection, get_volatile_fields, resolve_resource_selection_from_flags,
};
use crate::commands::confirm::prompt_yes_no;
use crate::commands::describe::describe_changes;
use crate::commands::load_config_and_env;

pub async fn run(
    flags: &ResourceTypeFlags,
    recursive: bool,
    filter: Option<String>,
    dry_run: bool,
    force: bool,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    let selection = resolve_resource_selection_from_flags(flags, env.sync.include_preview, true);

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    // Recursive expansion: fetch selected resources from server, expand, then pull all
    let selection = if recursive {
        expand_pull_selection(&env, &selection).await?
    } else {
        selection
    };

    execute_pull(
        &project_root,
        &files_root,
        &env,
        &selection,
        filter.as_deref(),
        dry_run,
        force,
    )
    .await
}

/// Expand a pull selection by fetching selected resources from the server,
/// discovering their dependencies and children, then building a new selection
/// that includes everything.
async fn expand_pull_selection(
    env: &ResolvedEnvironment,
    selection: &ResourceSelection,
) -> Result<ResourceSelection> {
    let search_svc = env
        .primary_search_service()
        .ok_or_else(|| anyhow::anyhow!("No search service configured"))?;
    let client = AzureSearchClient::from_service_config(search_svc)?;

    // Fetch all resources from all selected kinds concurrently (max 5 in-flight)
    let mut fetched: Vec<(ResourceKind, String, Value)> = Vec::new();
    let mut all_server: Vec<(ResourceKind, String, Value)> = Vec::new();

    let selected_kinds = selection.kinds();
    let semaphore = Arc::new(Semaphore::new(5));
    let selected_results: Vec<(ResourceKind, Result<Vec<Value>, _>)> =
        stream::iter(selected_kinds.iter())
            .map(|kind| {
                let client = &client;
                let sem = Arc::clone(&semaphore);
                async move {
                    let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");
                    let result = client.list(*kind).await;
                    (*kind, result)
                }
            })
            .buffer_unordered(5)
            .collect()
            .await;

    for (kind, result) in &selected_results {
        let resources = result.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        for r in resources {
            let name = r
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            all_server.push((*kind, name.clone(), r.clone()));

            // Check if this resource matches the selection
            if let Some(exact) = selection.name_filter(*kind) {
                if name == exact {
                    fetched.push((*kind, name, r.clone()));
                }
            } else {
                fetched.push((*kind, name, r.clone()));
            }
        }
    }

    if fetched.is_empty() {
        return Ok(selection.clone());
    }

    // Also fetch all resources from kinds not in the selection (needed for expansion)
    let all_kinds = if env.sync.include_preview {
        ResourceKind::all().to_vec()
    } else {
        ResourceKind::stable().to_vec()
    };

    let remaining_kinds: Vec<ResourceKind> = all_kinds
        .into_iter()
        .filter(|k| !selected_kinds.contains(k))
        .collect();

    let remaining_results: Vec<(ResourceKind, Result<Vec<Value>, _>)> =
        stream::iter(remaining_kinds.iter())
            .map(|kind| {
                let client = &client;
                let sem = Arc::clone(&semaphore);
                async move {
                    let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");
                    let result = client.list(*kind).await;
                    (*kind, result)
                }
            })
            .buffer_unordered(5)
            .collect()
            .await;

    for (kind, result) in &remaining_results {
        if let Ok(resources) = result {
            for r in resources {
                let name = r
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                all_server.push((*kind, name, r.clone()));
            }
        }
    }

    let expanded = hoist_core::copy::expand_recursive(&fetched, &all_server);

    // Build new selection from expanded resources
    let mut new_selections = Vec::new();
    for (kind, name, _) in &expanded {
        // Use exact name to avoid pulling unrelated resources of the same kind
        new_selections.push((*kind, Some(name.clone())));
    }

    Ok(ResourceSelection {
        selections: new_selections,
    })
}

/// Core pull logic, callable from both `pull` and `init` commands.
///
/// `project_root` is where state files (.hoist/) live.
/// `files_root` is where resource files (search/, foundry/) live.
#[allow(clippy::too_many_arguments)]
pub async fn execute_pull(
    project_root: &Path,
    files_root: &Path,
    env: &ResolvedEnvironment,
    selection: &ResourceSelection,
    filter: Option<&str>,
    dry_run: bool,
    force: bool,
) -> Result<()> {
    let kinds = selection.kinds();
    if dry_run {
        println!("Dry run - no changes will be made");
        println!();
    }

    // Split kinds by service domain
    let search_kinds: Vec<ResourceKind> = kinds
        .iter()
        .filter(|k| k.domain() == ServiceDomain::Search)
        .copied()
        .collect();
    let foundry_kinds: Vec<ResourceKind> = kinds
        .iter()
        .filter(|k| k.domain() == ServiceDomain::Foundry)
        .copied()
        .collect();

    // Load existing state
    let checksums = Checksums::load_env(project_root, &env.name)?;

    // === Discovery phase: fetch and classify all resources ===
    let mut new_resources = Vec::new();
    let mut updated_resources = Vec::new();
    let mut deleted_resources: Vec<(ResourceKind, String, std::path::PathBuf)> = Vec::new();
    let mut total_unchanged: usize = 0;
    let mut managed_map = ManagedMap::new();

    // --- Search resources ---
    if !search_kinds.is_empty() {
        let search_svc = env
            .primary_search_service()
            .ok_or_else(|| anyhow::anyhow!("No search service in environment '{}'", env.name))?;
        let client = AzureSearchClient::from_service_config(search_svc)?;

        info!(
            "Connected to {} using {}",
            search_svc.name,
            client.auth_method()
        );

        // Determine which kinds to actually fetch. If --knowledge-sources is
        // requested, also fetch managed sub-resource kinds.
        let mut fetch_kinds = search_kinds.clone();
        if fetch_kinds.contains(&ResourceKind::KnowledgeSource) {
            for managed_kind in managed::MANAGED_SUB_RESOURCE_KINDS {
                if !fetch_kinds.contains(managed_kind) {
                    fetch_kinds.push(*managed_kind);
                }
            }
        }

        // Fetch knowledge sources first if included, to build managed map
        let has_ks = fetch_kinds.contains(&ResourceKind::KnowledgeSource);
        if has_ks {
            let ks_results = client.list(ResourceKind::KnowledgeSource).await;
            if let Ok(ks_list) = &ks_results {
                let ks_pairs: Vec<(String, Value)> = ks_list
                    .iter()
                    .filter_map(|r| {
                        r.get("name")
                            .and_then(|n| n.as_str())
                            .map(|n| (n.to_string(), r.clone()))
                    })
                    .collect();
                managed_map = managed::build_managed_map(&ks_pairs);
            }
        }

        // Fetch all resource kinds concurrently (max 5 in-flight requests)
        // Skip KnowledgeSource if we already fetched it above
        let remaining_kinds: Vec<ResourceKind> = if has_ks {
            fetch_kinds
                .iter()
                .filter(|k| **k != ResourceKind::KnowledgeSource)
                .copied()
                .collect()
        } else {
            fetch_kinds.clone()
        };

        let semaphore = Arc::new(Semaphore::new(5));
        let mut fetched_results: Vec<(ResourceKind, Result<Vec<Value>, _>)> =
            stream::iter(remaining_kinds.iter())
                .map(|kind| {
                    let client = &client;
                    let sem = Arc::clone(&semaphore);
                    async move {
                        let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");
                        let result = client.list(*kind).await;
                        (*kind, result)
                    }
                })
                .buffer_unordered(5)
                .collect()
                .await;

        // Add KS results back if we fetched them separately
        if has_ks {
            let ks_result = client.list(ResourceKind::KnowledgeSource).await;
            fetched_results.push((ResourceKind::KnowledgeSource, ks_result));
        }

        let service_dir = env.search_service_dir(files_root, search_svc);

        discover_search_resources(
            &service_dir,
            &fetched_results,
            selection,
            filter,
            &checksums,
            &managed_map,
            &mut new_resources,
            &mut updated_resources,
            &mut deleted_resources,
            &mut total_unchanged,
        )?;
    }

    // --- Foundry agents ---
    if !foundry_kinds.is_empty() && env.has_foundry() {
        for foundry_config in &env.foundry {
            let agents_dir = foundry_agents_dir(env, files_root, foundry_config);
            discover_foundry_agents(
                &agents_dir,
                foundry_config,
                selection,
                filter,
                &checksums,
                &mut new_resources,
                &mut updated_resources,
                &mut deleted_resources,
                &mut total_unchanged,
            )
            .await?;
        }
    }

    let total_changes = new_resources.len() + updated_resources.len() + deleted_resources.len();

    // === Display summary ===
    if total_changes == 0 {
        println!(
            "All {} resource(s) are up to date, nothing to pull.",
            total_unchanged
        );
        return Ok(());
    }

    println!("Pull summary:");
    for r in &new_resources {
        println!(
            "  {} {} '{}' (new)",
            "+".green(),
            r.kind.display_name(),
            r.name
        );
    }
    for r in &updated_resources {
        println!(
            "  {} {} '{}' (modified)",
            "~".yellow(),
            r.kind.display_name(),
            r.name
        );
        for line in describe_changes(&r.changes, None) {
            println!("{}", line);
        }
    }
    for (kind, name, _) in &deleted_resources {
        println!(
            "  {} {} '{}' (deleted on server)",
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
    println!();

    // Warn about locally modified files that will be overwritten
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

    if dry_run {
        println!("Dry run - no changes made");
        return Ok(());
    }

    // === Confirm ===
    if !force && !prompt_yes_no("Proceed with pull?")? {
        println!("Aborted.");
        return Ok(());
    }

    // === Write phase ===
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
                &managed_map,
            ));
            std::fs::create_dir_all(&resource_dir)?;

            let filename = managed::resource_filename(entry.kind, &entry.name, &managed_map);
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
            &managed_map,
        );
        checksums.set_managed(
            entry.kind,
            &entry.name,
            entry.new_checksum.clone(),
            &managed_map,
        );
    }

    // Delete local files for resources removed on server
    for (kind, name, path) in &deleted_resources {
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
        state.remove_managed(*kind, name, &managed_map);
        checksums.remove_managed(*kind, name, &managed_map);
    }

    // Save state
    state.last_sync = Some(chrono::Utc::now());
    state.save_env(project_root, &env.name)?;
    checksums.save_env(project_root, &env.name)?;

    println!();
    let upsert_count = all_upserts.len();
    let delete_count = deleted_resources.len();
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

    Ok(())
}

/// Discover search resources from fetched API results.
#[allow(clippy::too_many_arguments)]
fn discover_search_resources(
    service_dir: &Path,
    fetched_results: &[(ResourceKind, Result<Vec<Value>, hoist_client::ClientError>)],
    selection: &ResourceSelection,
    filter: Option<&str>,
    checksums: &Checksums,
    managed_map: &ManagedMap,
    new_resources: &mut Vec<DiscoveredResource>,
    updated_resources: &mut Vec<DiscoveredResource>,
    deleted_resources: &mut Vec<(ResourceKind, String, std::path::PathBuf)>,
    total_unchanged: &mut usize,
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
            let is_existing = stored_checksum.is_some();
            let remote_unchanged = stored_checksum == Some(&new_checksum);
            let local_matches = file_path.exists()
                && std::fs::read_to_string(&file_path).ok().as_deref()
                    == Some(json_content.as_str());

            if remote_unchanged && local_matches {
                *total_unchanged += 1;
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
                    .map(|local_value| hoist_diff::diff(&local_value, &normalized, "name").changes)
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
async fn discover_foundry_agents(
    agents_dir: &Path,
    foundry_config: &FoundryServiceConfig,
    selection: &ResourceSelection,
    filter: Option<&str>,
    checksums: &Checksums,
    new_resources: &mut Vec<DiscoveredResource>,
    updated_resources: &mut Vec<DiscoveredResource>,
    deleted_resources: &mut Vec<(ResourceKind, String, std::path::PathBuf)>,
    total_unchanged: &mut usize,
) -> Result<()> {
    let client = hoist_client::FoundryClient::new(foundry_config)?;
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
        let is_existing = stored_checksum.is_some();
        let remote_unchanged = stored_checksum == Some(&new_checksum);

        // Check if local YAML file matches
        let yaml_path = agents_dir.join(format!("{}.yaml", name));
        let local_matches = yaml_path.exists() && remote_unchanged;

        if remote_unchanged && local_matches {
            *total_unchanged += 1;
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

        let entry = DiscoveredResource {
            kind,
            name: name.to_string(),
            json_content,
            new_checksum,
            raw_resource: (*agent).clone(),
            changes: vec![],
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
fn foundry_agents_dir(
    env: &ResolvedEnvironment,
    files_root: &Path,
    foundry_config: &FoundryServiceConfig,
) -> std::path::PathBuf {
    env.foundry_service_dir(files_root, foundry_config)
        .join("agents")
}

/// A resource discovered during the fetch phase, pending write.
struct DiscoveredResource {
    kind: ResourceKind,
    name: String,
    json_content: String,
    new_checksum: String,
    raw_resource: Value,
    changes: Vec<Change>,
    /// True if the local file has been modified since last pull
    locally_modified: bool,
}
