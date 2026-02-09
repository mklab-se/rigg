//! Show differences between local and remote

use anyhow::Result;
use colored::Colorize;
use tracing::info;

use hoist_client::AzureSearchClient;
use hoist_core::normalize::normalize;
use hoist_core::resources::agent::{agent_volatile_fields, compose_agent};
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;
use hoist_diff::{
    diff,
    output::{format_report, OutputFormat},
};

use crate::cli::{DiffFormat, ResourceTypeFlags};
use crate::commands::common::{
    get_read_only_fields, get_volatile_fields, read_agent_files,
    resolve_resource_selection_from_flags,
};
use crate::commands::describe::describe_changes;
use crate::commands::load_config;

pub async fn run(flags: &ResourceTypeFlags, format: DiffFormat, exit_code: bool) -> Result<()> {
    let (project_root, config) = load_config()?;

    // Determine which resource types to diff
    let selection = resolve_resource_selection_from_flags(flags, config.sync.include_preview, true);

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    let kinds = selection.kinds();

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

    let primary_search_name = config
        .primary_search_service()
        .map(|s| s.name)
        .unwrap_or_default();

    let mut all_diffs = Vec::new();
    let mut has_changes = false;

    // --- Search resources ---
    if !search_kinds.is_empty() && !primary_search_name.is_empty() {
        let client = AzureSearchClient::new(&config)?;

        for kind in &search_kinds {
            let resource_dir = config
                .search_service_dir(&project_root, &primary_search_name)
                .join(kind.directory_name());
            if !resource_dir.exists() {
                continue;
            }

            // Strip both volatile and read-only fields — matches push behavior.
            // Read-only fields (knowledgeSources, createdResources, etc.) can't be
            // pushed, so showing them as diffs would be misleading.
            let volatile = get_volatile_fields(*kind);
            let read_only = get_read_only_fields(*kind);
            let strip_fields: Vec<&str> =
                volatile.iter().chain(read_only.iter()).copied().collect();

            let exact_name = selection.name_filter(*kind);

            // Read all JSON files in directory
            for entry in std::fs::read_dir(&resource_dir)? {
                let entry = entry?;
                let path = entry.path();

                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }

                let name = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| anyhow::anyhow!("Invalid file name"))?;

                // Filter by singular flag (exact name match)
                if let Some(exact) = exact_name {
                    if name != exact {
                        continue;
                    }
                }

                // Read local file
                let content = std::fs::read_to_string(&path)?;
                let local: serde_json::Value = serde_json::from_str(&content)?;
                let local_normalized = normalize(&local, &strip_fields);

                // Fetch remote
                let resource_id = format!("{}/{}", kind.directory_name(), name);

                match client.get(*kind, name).await {
                    Ok(remote) => {
                        let remote_normalized = normalize(&remote, &strip_fields);
                        let diff_result = diff(&local_normalized, &remote_normalized, "name");

                        if !diff_result.is_equal {
                            has_changes = true;
                        }

                        all_diffs.push((resource_id, diff_result));
                    }
                    Err(hoist_client::ClientError::NotFound { .. }) => {
                        // Local only - will be created
                        has_changes = true;
                        all_diffs.push((
                            resource_id,
                            hoist_diff::DiffResult {
                                is_equal: false,
                                changes: vec![hoist_diff::Change {
                                    path: ".".to_string(),
                                    kind: hoist_diff::ChangeKind::Added,
                                    old_value: None,
                                    new_value: Some(local_normalized),
                                }],
                            },
                        ));
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            // Check for remote-only resources (will be kept, not deleted)
            let remote_resources = client.list(*kind).await?;
            for remote in remote_resources {
                let name = remote
                    .get("name")
                    .and_then(|n| n.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Resource missing name field"))?;

                let local_path = resource_dir.join(format!("{}.json", name));
                if !local_path.exists() {
                    let resource_id = format!("{}/{}", kind.directory_name(), name);

                    // Check if already reported
                    if all_diffs.iter().any(|(id, _)| id == &resource_id) {
                        continue;
                    }

                    // Remote only - note it but don't mark as change (we don't auto-delete)
                    all_diffs.push((
                        format!("{} (remote only)", resource_id),
                        hoist_diff::DiffResult {
                            is_equal: true, // Don't count as change for exit code
                            changes: vec![],
                        },
                    ));
                }
            }
        }
    }

    // --- Foundry agents ---
    if !foundry_kinds.is_empty() && config.has_foundry() {
        let exact_name = selection.name_filter(ResourceKind::Agent);
        let volatile = agent_volatile_fields();

        for foundry_config in config.foundry_services() {
            let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;
            info!(
                "Connected to Foundry {}/{} using {}",
                foundry_config.name,
                foundry_config.project,
                foundry_client.auth_method()
            );

            let agents_dir = config
                .foundry_service_dir(&project_root, &foundry_config.name, &foundry_config.project)
                .join("agents");

            // Fetch all remote agents for remote-only detection
            let remote_agents = foundry_client.list_agents().await?;
            let remote_names: std::collections::HashSet<String> = remote_agents
                .iter()
                .filter_map(|a| a.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect();

            // Diff local agents against remote
            if agents_dir.exists() {
                for entry in std::fs::read_dir(&agents_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }

                    let name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };

                    if let Some(exact) = exact_name {
                        if name != exact {
                            continue;
                        }
                    }

                    // Read and compose local agent
                    let agent_files = read_agent_files(&path)?;
                    let local_composed = compose_agent(&agent_files);
                    let local_normalized = normalize(&local_composed, volatile);

                    let resource_id = format!("agents/{}", name);

                    // Find matching remote agent
                    let remote_agent = remote_agents
                        .iter()
                        .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(&name));

                    match remote_agent {
                        Some(remote) => {
                            let remote_normalized = normalize(remote, volatile);
                            let diff_result = diff(&local_normalized, &remote_normalized, "name");

                            if !diff_result.is_equal {
                                has_changes = true;
                            }

                            all_diffs.push((resource_id, diff_result));
                        }
                        None => {
                            // Local only - will be created
                            has_changes = true;
                            all_diffs.push((
                                resource_id,
                                hoist_diff::DiffResult {
                                    is_equal: false,
                                    changes: vec![hoist_diff::Change {
                                        path: ".".to_string(),
                                        kind: hoist_diff::ChangeKind::Added,
                                        old_value: None,
                                        new_value: Some(local_normalized),
                                    }],
                                },
                            ));
                        }
                    }
                }
            }

            // Check for remote-only agents
            for remote_name in &remote_names {
                let local_dir = agents_dir.join(remote_name);
                if !local_dir.exists() {
                    let resource_id = format!("agents/{}", remote_name);
                    if all_diffs.iter().any(|(id, _)| id == &resource_id) {
                        continue;
                    }
                    all_diffs.push((
                        format!("{} (remote only)", resource_id),
                        hoist_diff::DiffResult {
                            is_equal: true,
                            changes: vec![],
                        },
                    ));
                }
            }
        }
    }

    // Format output
    match format {
        DiffFormat::Text => {
            let (changed, unchanged): (Vec<_>, Vec<_>) =
                all_diffs.iter().partition(|(_, r)| !r.is_equal);

            if changed.is_empty() {
                println!(
                    "No drift detected, all {} resource(s) match the server.",
                    unchanged.len()
                );
            } else {
                println!(
                    "{} resource(s) with drift:\n",
                    changed.len().to_string().yellow()
                );
                for (name, result) in &changed {
                    println!(
                        "  {} {} ({} change{})",
                        "~".yellow(),
                        name,
                        result.changes.len(),
                        if result.changes.len() == 1 { "" } else { "s" }
                    );
                    for line in describe_changes(&result.changes, Some(("local", "server"))) {
                        println!("{}", line);
                    }
                }
                println!();
                if !unchanged.is_empty() {
                    println!(
                        "  {} resource(s) match the server",
                        unchanged.len().to_string().dimmed()
                    );
                }
            }
        }
        DiffFormat::Json => {
            let report = format_report(&all_diffs, OutputFormat::Json);
            print!("{}", report);
        }
    }

    // Exit code handling
    if exit_code && has_changes {
        std::process::exit(5); // 5 = drift detected
    }

    Ok(())
}
