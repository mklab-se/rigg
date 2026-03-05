//! Pull resources from Azure
//!
//! Submodules:
//! - `discover` — Fetch resources from Azure and classify as new/updated/deleted
//! - `execute` — Core pull orchestration (discovery, display, confirmation, writing)
//! - `output` — Display summaries and AI narrative generation
//! - `write` — Persist resources to disk and update state

mod discover;
mod execute;
mod output;
mod write;

use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde_json::Value;
use tokio::sync::Semaphore;

use hoist_client::AzureSearchClient;
use hoist_core::config::ResolvedEnvironment;
use hoist_core::resources::ResourceKind;

use crate::cli::ResourceTypeFlags;
use crate::commands::common::{ResourceSelection, resolve_resource_selection_from_flags};
use crate::commands::load_config_and_env;

// Re-export the public API
pub use execute::execute_pull;

pub async fn run(
    flags: &ResourceTypeFlags,
    recursive: bool,
    filter: Option<String>,
    force: bool,
    no_explain: bool,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    // AI explanations: on by default when ailloy is configured, unless --no-explain
    let use_explain = !no_explain && crate::commands::ai::is_ai_active();

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
        force,
        use_explain,
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
