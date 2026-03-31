//! Core pull execution: orchestrates discovery, display, confirmation, and writing.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde_json::Value;
use tokio::sync::Semaphore;
use tracing::info;

use rigg_client::AzureSearchClient;
use rigg_core::config::ResolvedEnvironment;
use rigg_core::resources::ResourceKind;
use rigg_core::resources::managed::{self, ManagedMap};
use rigg_core::service::ServiceDomain;
use rigg_core::state::Checksums;

use crate::commands::common::ResourceSelection;
use crate::commands::confirm::prompt_yes_no;

use super::discover;
use super::output;
use super::write;

/// Core pull logic, callable from both `pull` and `init` commands.
///
/// `project_root` is where state files (.rigg/) live.
/// `files_root` is where resource files (search/, foundry/) live.
/// `ai_config` enables AI-generated explanations when `Some`.
#[allow(clippy::too_many_arguments)]
pub async fn execute_pull(
    project_root: &Path,
    files_root: &Path,
    env: &ResolvedEnvironment,
    selection: &ResourceSelection,
    filter: Option<&str>,
    force: bool,
    use_explain: bool,
) -> Result<()> {
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

    // Load existing state
    let checksums = Checksums::load_env(project_root, &env.name)?;

    // === Discovery phase: fetch and classify all resources ===
    let mut new_resources = Vec::new();
    let mut updated_resources = Vec::new();
    let mut deleted_resources: Vec<(ResourceKind, String, std::path::PathBuf)> = Vec::new();
    let mut total_unchanged: usize = 0;
    let mut managed_map = ManagedMap::new();
    let mut checksum_backfill: Vec<(ResourceKind, String, String)> = Vec::new();

    // --- Search resources ---
    if !search_kinds.is_empty() {
        let search_svc = env
            .primary_search_service()
            .ok_or_else(|| anyhow::anyhow!("No search service in environment '{}'", env.name))?;
        eprintln!("Fetching resources from {}...", search_svc.name);
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

        // Always build the managed map when preview resources exist, even if
        // --knowledgesources wasn't explicitly requested. This prevents standalone
        // pulls (e.g. --indexes) from duplicating managed KS sub-resources into
        // the top-level directories.
        let has_ks = fetch_kinds.contains(&ResourceKind::KnowledgeSource);
        let needs_managed_map = has_ks || env.sync.include_preview;
        if needs_managed_map {
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

        discover::discover_search_resources(
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
            &mut checksum_backfill,
        )?;
    }

    // --- Foundry agents ---
    if !foundry_kinds.is_empty() && env.has_foundry() {
        for foundry_config in &env.foundry {
            eprintln!(
                "Fetching agents from {}/{}...",
                foundry_config.name, foundry_config.project
            );
            let agents_dir = write::foundry_agents_dir(env, files_root, foundry_config);
            discover::discover_foundry_agents(
                &agents_dir,
                foundry_config,
                selection,
                filter,
                &checksums,
                &mut new_resources,
                &mut updated_resources,
                &mut deleted_resources,
                &mut total_unchanged,
                &mut checksum_backfill,
            )
            .await?;
        }
    }

    // Backfill checksums for files that match Azure but had no stored checksum.
    write::backfill_checksums(project_root, &env.name, &checksum_backfill, &managed_map)?;

    let total_changes = new_resources.len() + updated_resources.len() + deleted_resources.len();

    // === Display summary ===
    if total_changes == 0 {
        println!(
            "All {} resource(s) are up to date, nothing to pull.",
            total_unchanged
        );
        return Ok(());
    }

    // Try AI narrative mode first when AI is enabled
    let used_ai_narrative = if use_explain {
        match output::generate_pull_narrative(
            &new_resources,
            &updated_resources,
            &deleted_resources,
            total_unchanged,
        )
        .await
        {
            Some(narrative) => {
                println!("{}", narrative);
                true
            }
            None => false,
        }
    } else {
        false
    };

    // Fall back to non-AI output if narrative wasn't used
    if !used_ai_narrative {
        output::print_pull_summary(
            &new_resources,
            &updated_resources,
            &deleted_resources,
            total_unchanged,
        );
    }
    println!();

    // Warn about locally modified files that will be overwritten
    output::print_local_modification_warnings(&updated_resources);

    // === Confirm ===
    if !force && !prompt_yes_no("Proceed with pull?")? {
        println!("Aborted.");
        return Ok(());
    }

    // === Write phase ===
    let (upsert_count, delete_count) = write::write_resources(
        project_root,
        files_root,
        env,
        new_resources,
        updated_resources,
        &deleted_resources,
        &managed_map,
        total_unchanged,
    )?;

    output::print_pull_result(upsert_count, delete_count, total_unchanged);

    Ok(())
}
