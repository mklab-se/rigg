//! Pull resources from Azure

use std::path::Path;

use anyhow::Result;
use serde_json::Value;
use tracing::info;

use colored::Colorize;

use hoist_client::AzureSearchClient;
use hoist_core::normalize::{format_json, normalize};
use hoist_core::resources::ResourceKind;
use hoist_core::state::{Checksums, LocalState, ResourceState};
use hoist_core::Config;
use hoist_diff::Change;

use crate::commands::common::{
    get_volatile_fields, resolve_resource_selection, ResourceSelection, SingularFlags,
};
use crate::commands::confirm::prompt_yes_no;
use crate::commands::describe::describe_changes;
use crate::commands::load_config;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    all: bool,
    indexes: bool,
    indexers: bool,
    datasources: bool,
    skillsets: bool,
    synonymmaps: bool,
    knowledgebases: bool,
    knowledgesources: bool,
    singular: &SingularFlags,
    recursive: bool,
    filter: Option<String>,
    dry_run: bool,
    force: bool,
    source: Option<String>,
) -> Result<()> {
    let (project_root, config) = load_config()?;

    let selection = resolve_resource_selection(
        all,
        indexes,
        indexers,
        datasources,
        skillsets,
        synonymmaps,
        knowledgebases,
        knowledgesources,
        singular,
        config.sync.include_preview,
        true,
    );

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    // Recursive expansion: fetch selected resources from server, expand, then pull all
    let selection = if recursive {
        expand_pull_selection(&config, &selection, source.as_deref()).await?
    } else {
        selection
    };

    execute_pull(
        &project_root,
        &config,
        &selection,
        filter.as_deref(),
        dry_run,
        force,
        source.as_deref(),
    )
    .await
}

/// Expand a pull selection by fetching selected resources from the server,
/// discovering their dependencies and children, then building a new selection
/// that includes everything.
async fn expand_pull_selection(
    config: &Config,
    selection: &ResourceSelection,
    source: Option<&str>,
) -> Result<ResourceSelection> {
    let client = if let Some(server) = source {
        AzureSearchClient::new_for_server(config, server)?
    } else {
        AzureSearchClient::new(config)?
    };

    // Fetch all resources from all selected kinds
    let mut fetched: Vec<(ResourceKind, String, Value)> = Vec::new();
    let mut all_server: Vec<(ResourceKind, String, Value)> = Vec::new();

    for kind in selection.kinds() {
        let resources = client.list(kind).await?;
        for r in &resources {
            let name = r
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            all_server.push((kind, name.clone(), r.clone()));

            // Check if this resource matches the selection
            if let Some(exact) = selection.name_filter(kind) {
                if name == exact {
                    fetched.push((kind, name, r.clone()));
                }
            } else {
                fetched.push((kind, name, r.clone()));
            }
        }
    }

    if fetched.is_empty() {
        return Ok(selection.clone());
    }

    // Also fetch all resources from kinds not in the selection (needed for expansion)
    let all_kinds = if config.sync.include_preview {
        ResourceKind::all().to_vec()
    } else {
        ResourceKind::stable().to_vec()
    };

    for kind in &all_kinds {
        if selection.kinds().contains(kind) {
            continue; // Already fetched
        }
        if let Ok(resources) = client.list(*kind).await {
            for r in &resources {
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
#[allow(clippy::too_many_arguments)]
pub async fn execute_pull(
    project_root: &Path,
    config: &Config,
    selection: &ResourceSelection,
    filter: Option<&str>,
    dry_run: bool,
    force: bool,
    source: Option<&str>,
) -> Result<()> {
    let kinds = selection.kinds();
    if dry_run {
        println!("Dry run - no changes will be made");
        println!();
    }

    // Create client (possibly for a different server)
    let client = if let Some(server) = source {
        AzureSearchClient::new_for_server(config, server)?
    } else {
        AzureSearchClient::new(config)?
    };

    let server_name = source.unwrap_or(&config.service.name);
    info!(
        "Connected to {} using {}",
        server_name,
        client.auth_method()
    );

    // Load existing state
    let checksums = Checksums::load(project_root)?;

    // === Discovery phase: fetch and classify all resources ===
    let mut new_resources = Vec::new();
    let mut updated_resources = Vec::new();
    let mut deleted_resources: Vec<(ResourceKind, String, std::path::PathBuf)> = Vec::new();
    let mut total_unchanged: usize = 0;

    for kind in &kinds {
        // Fetch resources from Azure
        let resources = client.list(*kind).await?;

        // Build set of remote resource names (before filtering, for deletion detection)
        let all_remote_names: std::collections::HashSet<String> = resources
            .iter()
            .filter_map(|r| r.get("name").and_then(|n| n.as_str()).map(String::from))
            .collect();

        // Filter by singular flag (exact name match) and/or pattern (substring match)
        let exact_name = selection.name_filter(*kind);
        let resources: Vec<_> = resources
            .into_iter()
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

        let resource_dir = config
            .resource_dir(project_root)
            .join(kind.directory_name());

        for resource in &resources {
            let name = resource
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| anyhow::anyhow!("Resource missing name field"))?;

            // Normalize the JSON
            let volatile_fields = get_volatile_fields(*kind);
            let normalized = normalize(resource, &volatile_fields, "name");
            let json_content = format_json(&normalized);

            // Check if content changed (remote vs stored checksum) and file on disk matches
            let new_checksum = Checksums::calculate(&json_content);
            let file_path = resource_dir.join(format!("{}.json", name));
            let is_existing = checksums.get(*kind, name).is_some();
            let remote_unchanged = checksums.get(*kind, name) == Some(&new_checksum);
            let local_matches = file_path.exists()
                && std::fs::read_to_string(&file_path).ok().as_deref()
                    == Some(json_content.as_str());

            if remote_unchanged && local_matches {
                total_unchanged += 1;
                continue;
            }

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
                raw_resource: resource.clone(),
                changes,
            };

            if is_existing {
                updated_resources.push(entry);
            } else {
                new_resources.push(entry);
            }
        }

        // Detect local files whose resources were deleted on the server
        if resource_dir.exists() {
            for entry in std::fs::read_dir(&resource_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }

                let name = match path.file_stem().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                // If filter is set, only consider files matching the filter
                if let Some(pattern) = filter {
                    if !name.contains(pattern) {
                        continue;
                    }
                }

                // If this name doesn't exist on the server AND was previously tracked,
                // it's been deleted. We check checksums to avoid flagging new local-only
                // files that the user created for pushing.
                if !all_remote_names.contains(&name) && checksums.get(*kind, &name).is_some() {
                    deleted_resources.push((*kind, name, path));
                }
            }
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
    let mut state = LocalState::load(project_root)?;
    let mut checksums = Checksums::load(project_root)?;

    let all_upserts: Vec<_> = new_resources.into_iter().chain(updated_resources).collect();

    for entry in &all_upserts {
        let resource_dir = config
            .resource_dir(project_root)
            .join(entry.kind.directory_name());
        std::fs::create_dir_all(&resource_dir)?;

        let file_path = resource_dir.join(format!("{}.json", entry.name));
        std::fs::write(&file_path, &entry.json_content)?;
        info!("Wrote {}", file_path.display());

        // Update state
        let etag = entry
            .raw_resource
            .get("@odata.etag")
            .and_then(|e| e.as_str())
            .map(String::from);

        state.set(
            entry.kind,
            &entry.name,
            ResourceState {
                kind: entry.kind,
                etag,
                checksum: entry.new_checksum.clone(),
                synced_at: chrono::Utc::now(),
            },
        );
        checksums.set(entry.kind, &entry.name, entry.new_checksum.clone());
    }

    // Delete local files for resources removed on server
    for (kind, name, path) in &deleted_resources {
        std::fs::remove_file(path)?;
        info!("Deleted {}", path.display());
        state.remove(*kind, name);
        checksums.remove(*kind, name);
    }

    // Save state
    state.last_sync = Some(chrono::Utc::now());
    state.save(project_root)?;
    checksums.save(project_root)?;

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

/// A resource discovered during the fetch phase, pending write.
struct DiscoveredResource {
    kind: ResourceKind,
    name: String,
    json_content: String,
    new_checksum: String,
    raw_resource: Value,
    changes: Vec<Change>,
}
