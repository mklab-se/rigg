//! Push resources to Azure

mod agents;
mod collect;
mod credentials;
mod execute;
mod explain;
mod report;

use std::collections::HashMap;

use anyhow::Result;
use colored::Colorize;
use tracing::info;

use hoist_client::AzureSearchClient;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::managed;
use hoist_core::service::ServiceDomain;
use hoist_core::state::Checksums;
use hoist_diff::Change;

use crate::cli::ResourceTypeFlags;
use crate::commands::common::{order_by_dependencies, resolve_resource_selection_from_flags};
use crate::commands::confirm::prompt_yes_no;
use crate::commands::load_config_and_env;

use collect::{build_local_managed_map, collect_push_resource};

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

    // AI explanations: on by default when ai: is configured, unless --no-explain
    let use_explain = !no_explain && crate::commands::ai::is_ai_active();

    // Push has no default fallback — user must specify resource types
    let selection = resolve_resource_selection_from_flags(flags, env.sync.include_preview, false);

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
    let has_foundry_kinds = kinds.iter().any(|k| k.domain() == ServiceDomain::Foundry);

    let primary_search_svc = env.primary_search_service();
    let server_name = primary_search_svc
        .map(|s| s.name.as_str())
        .unwrap_or("(none)");

    // Load checksums from last pull for conflict detection
    let checksums = Checksums::load_env(&project_root, &env.name).unwrap_or_default();

    // Collect resources to push
    let mut resources_to_push = Vec::new();
    let mut validation_errors = Vec::new();
    let mut recreate_candidates: Vec<(ResourceKind, String)> = Vec::new();
    let mut total_unchanged = 0;
    let mut change_details: HashMap<(ResourceKind, String), Vec<Change>> = HashMap::new();
    let mut remote_values: HashMap<(ResourceKind, String), serde_json::Value> = HashMap::new();
    let mut remote_conflicts: Vec<(ResourceKind, String)> = Vec::new();

    // --- Search resources ---
    if !search_kinds.is_empty() {
        let search_svc = primary_search_svc
            .ok_or_else(|| anyhow::anyhow!("No search service in environment '{}'", env.name))?;
        eprintln!("Comparing local resources against {}...", server_name);
        let client = AzureSearchClient::from_service_config(search_svc)?;

        info!(
            "Connected to {} using {}",
            server_name,
            client.auth_method()
        );

        let service_dir = env.search_service_dir(&files_root, search_svc);

        // Build managed map from local KS files
        let managed_map = build_local_managed_map(&service_dir);

        // Determine which kinds to push, handling --knowledge-sources expansion
        let has_ks = search_kinds.contains(&ResourceKind::KnowledgeSource);

        for kind in &search_kinds {
            // For standalone resource types, read from their standard directories
            // but skip managed resources (they'll be pushed via KS cascade)
            if *kind == ResourceKind::KnowledgeSource {
                // Read KS definitions from their subdirectories
                let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");
                if !ks_base.exists() {
                    continue;
                }
                for entry in std::fs::read_dir(&ks_base)? {
                    let entry = entry?;
                    let path = entry.path();
                    if !path.is_dir() {
                        continue;
                    }
                    let ks_name = match path.file_name().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };
                    if let Some(exact_name) = selection.name_filter(*kind) {
                        if ks_name != exact_name {
                            continue;
                        }
                    }
                    if let Some(ref pattern) = filter {
                        if !ks_name.contains(pattern) {
                            continue;
                        }
                    }
                    let ks_file = path.join(format!("{}.json", ks_name));
                    if !ks_file.exists() {
                        continue;
                    }
                    let content = std::fs::read_to_string(&ks_file)?;
                    let local: serde_json::Value = serde_json::from_str(&content)?;

                    let push_count_before = resources_to_push.len();
                    collect_push_resource(
                        &client,
                        *kind,
                        &ks_name,
                        local,
                        &mut resources_to_push,
                        &mut validation_errors,
                        &mut recreate_candidates,
                        &mut total_unchanged,
                        &mut change_details,
                        &mut remote_values,
                        &checksums,
                        &mut remote_conflicts,
                    )
                    .await;

                    // Check if this KS is being created (new) vs updated
                    let ks_is_new = resources_to_push.get(push_count_before).is_some_and(
                        |(k, n, _, exists)| {
                            *k == ResourceKind::KnowledgeSource && n == &ks_name && !exists
                        },
                    );

                    // Only collect managed sub-resources for EXISTING knowledge sources.
                    // When creating a new KS, Azure auto-provisions sub-resources — pushing
                    // them first would cause KS creation to fail ("resources already exist").
                    if has_ks && !ks_is_new {
                        let managed_subs = managed::read_managed_sub_resources(&path, &ks_name);
                        for (sub_kind, sub_name, sub_def) in managed_subs {
                            collect_push_resource(
                                &client,
                                sub_kind,
                                &sub_name,
                                sub_def,
                                &mut resources_to_push,
                                &mut validation_errors,
                                &mut recreate_candidates,
                                &mut total_unchanged,
                                &mut change_details,
                                &mut remote_values,
                                &checksums,
                                &mut remote_conflicts,
                            )
                            .await;
                        }
                    }
                }
                continue;
            }

            // For other resource types, read from standalone directories
            let resource_dir = service_dir.join(kind.directory_name());
            if !resource_dir.exists() {
                continue;
            }

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

                // Skip managed resources in standalone dirs — they're pushed via KS cascade
                if let Some(ks_name) = managed::managing_ks(&managed_map, *kind, name) {
                    info!(
                        "Skipping managed {} '{}' (owned by knowledge source '{}') — use --knowledgesources to push",
                        kind.display_name(),
                        name,
                        ks_name
                    );
                    continue;
                }

                if let Some(exact_name) = selection.name_filter(*kind) {
                    if name != exact_name {
                        continue;
                    }
                }
                if let Some(ref pattern) = filter {
                    if !name.contains(pattern) {
                        continue;
                    }
                }

                let content = std::fs::read_to_string(&path)?;
                let local: serde_json::Value = serde_json::from_str(&content)?;

                collect_push_resource(
                    &client,
                    *kind,
                    name,
                    local,
                    &mut resources_to_push,
                    &mut validation_errors,
                    &mut recreate_candidates,
                    &mut total_unchanged,
                    &mut change_details,
                    &mut remote_values,
                    &checksums,
                    &mut remote_conflicts,
                )
                .await;
            }
        }

        // Handle drop-and-recreate for RequiresRecreate violations
        report::handle_recreate_candidates(
            &recreate_candidates,
            &mut resources_to_push,
            &service_dir,
            force,
        )?;
    }

    // --- Foundry agents ---
    if has_foundry_kinds && env.has_foundry() {
        agents::collect_foundry_agents(
            &env,
            &files_root,
            &selection,
            &filter,
            &mut resources_to_push,
            &mut total_unchanged,
            &mut change_details,
            &mut remote_values,
            &checksums,
            &mut remote_conflicts,
        )
        .await?;
    }

    // Recursive expansion: include deps and children
    if recursive && !resources_to_push.is_empty() {
        let initial_names: std::collections::HashSet<(ResourceKind, String)> = resources_to_push
            .iter()
            .map(|(k, n, _, _)| (*k, n.clone()))
            .collect();

        // Load all local resources across all kinds for expansion
        let all_kinds = if env.sync.include_preview {
            ResourceKind::all().to_vec()
        } else {
            ResourceKind::stable().to_vec()
        };

        let mut all_local = Vec::new();
        let recurse_search_svc = env.primary_search_service();
        for k in &all_kinds {
            let dir = if k.domain() == ServiceDomain::Search {
                if let Some(svc) = recurse_search_svc {
                    env.search_service_dir(&files_root, svc)
                        .join(k.directory_name())
                } else {
                    continue;
                }
            } else {
                // For Foundry kinds, skip here — agents are loaded separately
                continue;
            };
            if !dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let n = path
                    .file_stem()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                if let Ok(content) = std::fs::read_to_string(&path) {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                        all_local.push((*k, n, val));
                    }
                }
            }
        }

        let selected: Vec<_> = resources_to_push
            .iter()
            .map(|(k, n, v, _)| (*k, n.clone(), v.clone()))
            .collect();

        let expanded = hoist_core::copy::expand_recursive(&selected, &all_local);

        // Add newly discovered resources
        for (k, n, v) in expanded {
            if !initial_names.contains(&(k, n.clone())) {
                println!(
                    "  {} {} '{}' (included by --recursive)",
                    "+".green(),
                    k.display_name(),
                    n
                );
                resources_to_push.push((k, n, v, false));
            }
        }
    }

    // Report validation errors
    if !validation_errors.is_empty() {
        println!("Validation errors:");
        for error in &validation_errors {
            println!("  {}", error);
        }
        println!();
        anyhow::bail!(
            "Push blocked: {} validation error(s). Fix the issues above and try again.",
            validation_errors.len()
        );
    }

    if resources_to_push.is_empty() {
        if total_unchanged > 0 {
            println!(
                "All {} resource(s) match the server, nothing to push.",
                total_unchanged
            );
        } else {
            println!("No local resources found to push.");
        }
        return Ok(());
    }

    // Try AI narrative mode first when AI is enabled
    let used_ai_narrative = if use_explain {
        explain::generate_push_narrative(
            &resources_to_push,
            &change_details,
            &remote_values,
            total_unchanged,
        )
        .await
        .map(|narrative| {
            println!("{}", narrative);
        })
        .is_some()
    } else {
        false
    };

    // Fall back to non-AI output if narrative wasn't used
    if !used_ai_narrative {
        report::print_push_plan(&resources_to_push, &change_details, total_unchanged);
    }
    println!();

    // Show remote conflict warnings
    report::print_conflict_warnings(&remote_conflicts, &resources_to_push);

    // Confirm unless --force
    if !force && !prompt_yes_no("Proceed with push?")? {
        println!("Aborted.");
        return Ok(());
    }

    // Push resources in dependency order
    let ordered = order_by_dependencies(&resources_to_push);

    // Split ordered resources by domain for execution
    let search_resources: Vec<_> = ordered
        .iter()
        .filter(|(k, _, _, _)| k.domain() == ServiceDomain::Search)
        .collect();
    let foundry_resources: Vec<_> = ordered
        .iter()
        .filter(|(k, _, _, _)| k.domain() == ServiceDomain::Foundry)
        .collect();

    let mut success_count = 0;
    let mut error_count = 0;
    let mut pushed_resources: Vec<(ResourceKind, String)> = Vec::new();

    // Cache for discovered storage connection string (avoids repeated ARM calls)
    let mut cached_connection_string: Option<String> = None;

    // Push search resources
    if !search_resources.is_empty() {
        let (s, e, pushed) = execute::push_search_resources(
            &search_resources,
            &recreate_candidates,
            &env,
            &mut cached_connection_string,
        )
        .await?;
        success_count += s;
        error_count += e;
        pushed_resources.extend(pushed);
    }

    // Push Foundry agents
    if !foundry_resources.is_empty() && env.has_foundry() {
        let (s, e, pushed) = execute::push_foundry_agents(&foundry_resources, &env).await?;
        success_count += s;
        error_count += e;
        pushed_resources.extend(pushed);
    }

    // === Pull-back phase: re-fetch pushed resources to sync local files with server canonical form ===
    if !pushed_resources.is_empty() {
        execute::pullback_synced_resources(&pushed_resources, &project_root, &files_root, &env)
            .await?;
    }

    println!();
    println!(
        "Push complete: {} succeeded, {} failed",
        success_count, error_count
    );

    if error_count > 0 {
        std::process::exit(1);
    }

    Ok(())
}
