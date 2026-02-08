//! Show differences between local and remote

use anyhow::Result;
use colored::Colorize;

use hoist_client::AzureSearchClient;
use hoist_core::normalize::normalize;
use hoist_diff::{
    diff,
    output::{format_report, OutputFormat},
};

use crate::cli::DiffFormat;
use crate::commands::common::{
    get_read_only_fields, get_volatile_fields, resolve_resource_selection, SingularFlags,
};
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
    aliases: bool,
    knowledgebases: bool,
    knowledgesources: bool,
    singular: &SingularFlags,
    format: DiffFormat,
    exit_code: bool,
) -> Result<()> {
    let (project_root, config) = load_config()?;

    // Determine which resource types to diff
    let selection = resolve_resource_selection(
        all,
        indexes,
        indexers,
        datasources,
        skillsets,
        synonymmaps,
        aliases,
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

    let kinds = selection.kinds();

    // Create client
    let client = AzureSearchClient::new(&config)?;

    let mut all_diffs = Vec::new();
    let mut has_changes = false;

    for kind in &kinds {
        let resource_dir = config
            .resource_dir(&project_root)
            .join(kind.directory_name());
        if !resource_dir.exists() {
            continue;
        }

        // Strip both volatile and read-only fields — matches push behavior.
        // Read-only fields (knowledgeSources, createdResources, etc.) can't be
        // pushed, so showing them as diffs would be misleading.
        let volatile = get_volatile_fields(*kind);
        let read_only = get_read_only_fields(*kind);
        let strip_fields: Vec<&str> = volatile.iter().chain(read_only.iter()).copied().collect();

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
            let local_normalized = normalize(&local, &strip_fields, "name");

            // Fetch remote
            let resource_id = format!("{}/{}", kind.directory_name(), name);

            match client.get(*kind, name).await {
                Ok(remote) => {
                    let remote_normalized = normalize(&remote, &strip_fields, "name");
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
