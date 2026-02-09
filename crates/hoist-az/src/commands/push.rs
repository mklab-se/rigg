//! Push resources to Azure

use std::collections::HashMap;
use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;
use tracing::info;

use hoist_client::auth::AzCliAuth;
use hoist_client::ArmClient;
use hoist_client::AzureSearchClient;
use hoist_core::config::FoundryServiceConfig;
use hoist_core::constraints::check_immutability;
use hoist_core::constraints::ViolationSeverity;
use hoist_core::normalize::{format_json, normalize};
use hoist_core::resources::agent::compose_agent;
use hoist_core::resources::managed::{self, ManagedMap};
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;
use hoist_core::Config;
use hoist_diff::Change;

use crate::cli::ResourceTypeFlags;
use crate::commands::common::{
    get_read_only_fields, get_volatile_fields, order_by_dependencies, read_agent_files,
    resolve_resource_selection_from_flags,
};
use crate::commands::confirm::prompt_yes_no;
use crate::commands::describe::describe_changes;
use crate::commands::load_config;

pub async fn run(
    flags: &ResourceTypeFlags,
    recursive: bool,
    filter: Option<String>,
    dry_run: bool,
    force: bool,
    target: Option<String>,
) -> Result<()> {
    let (project_root, config) = load_config()?;

    // Push has no default fallback — user must specify resource types
    let selection =
        resolve_resource_selection_from_flags(flags, config.sync.include_preview, false);

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

    let default_name = config
        .primary_search_service()
        .map(|s| s.name)
        .unwrap_or_default();
    let server_name = target.as_deref().unwrap_or(&default_name);

    // Collect resources to push
    let mut resources_to_push = Vec::new();
    let mut validation_errors = Vec::new();
    let mut recreate_candidates: Vec<(ResourceKind, String)> = Vec::new();
    let mut total_unchanged = 0;
    let mut change_details: HashMap<(ResourceKind, String), Vec<Change>> = HashMap::new();

    // --- Search resources ---
    if !search_kinds.is_empty() {
        let client = if let Some(ref server) = target {
            AzureSearchClient::new_for_server(&config, server)?
        } else {
            AzureSearchClient::new(&config)?
        };

        info!(
            "Connected to {} using {}",
            server_name,
            client.auth_method()
        );

        let push_search_svc = config.primary_search_service().unwrap();
        let service_dir = config.search_service_dir(&project_root, &push_search_svc.name);

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
                    )
                    .await;

                    // If --knowledge-sources, also collect managed sub-resources from this KS dir
                    if has_ks {
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
                if managed::managing_ks(&managed_map, *kind, name).is_some() {
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
                )
                .await;
            }
        }

        // Handle drop-and-recreate for RequiresRecreate violations
        if !recreate_candidates.is_empty() {
            println!();
            println!(
                "{} resource(s) have immutable field changes that require drop-and-recreate:",
                recreate_candidates.len()
            );
            for (kind, name) in &recreate_candidates {
                println!("  {} {} '{}'", "!".red(), kind.display_name(), name);
            }
            println!();
            println!("WARNING: Drop-and-recreate will DELETE these resources and their data.");
            println!("Re-indexing will be required after recreation.");
            if !force && !prompt_yes_no("Drop and recreate these resources?")? {
                anyhow::bail!("Push blocked: immutable field changes require drop-and-recreate.");
            }

            // Mark these for drop-and-recreate during push execution
            for (kind, name) in &recreate_candidates {
                // Re-read the local file to get the definition
                let resource_dir = service_dir.join(kind.directory_name());
                let path = resource_dir.join(format!("{}.json", name));
                if path.exists() {
                    let content = std::fs::read_to_string(&path)?;
                    let local: serde_json::Value = serde_json::from_str(&content)?;
                    // Push with exists=true so it triggers the drop-and-recreate path
                    resources_to_push.push((*kind, name.clone(), local, true));
                }
            }
        }
    }

    // --- Foundry agents ---
    if has_foundry_kinds && config.has_foundry() {
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

            if !agents_dir.exists() {
                continue;
            }

            // Get existing agents for diffing
            let existing_agents = foundry_client.list_agents().await?;

            // Build name→remote_agent map for diffing
            let remote_agent_map: HashMap<String, &serde_json::Value> = existing_agents
                .iter()
                .filter_map(|a| {
                    let name = a.get("name")?.as_str()?.to_string();
                    Some((name, a))
                })
                .collect();

            let volatile = hoist_core::resources::agent::agent_volatile_fields();

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

                if let Some(exact_name) = selection.name_filter(ResourceKind::Agent) {
                    if name != exact_name {
                        continue;
                    }
                }
                if let Some(ref pattern) = filter {
                    if !name.contains(pattern) {
                        continue;
                    }
                }

                // Read decomposed agent files and compose into API payload
                let agent_files = read_agent_files(&path)?;
                let payload = compose_agent(&agent_files);

                // Compare local vs remote to skip unchanged agents
                match remote_agent_map.get(&name) {
                    Some(remote) => {
                        let normalized_local = hoist_core::normalize::normalize(&payload, volatile);
                        let normalized_remote = hoist_core::normalize::normalize(remote, volatile);

                        let local_json = hoist_core::normalize::format_json(&normalized_local);
                        let remote_json = hoist_core::normalize::format_json(&normalized_remote);

                        if local_json == remote_json {
                            total_unchanged += 1;
                        } else {
                            let diff_result =
                                hoist_diff::diff(&normalized_remote, &normalized_local, "name");
                            change_details
                                .insert((ResourceKind::Agent, name.clone()), diff_result.changes);
                            resources_to_push.push((ResourceKind::Agent, name, payload, true));
                        }
                    }
                    None => {
                        // New agent — will be created
                        resources_to_push.push((ResourceKind::Agent, name, payload, false));
                    }
                }
            }
        }
    }

    // Recursive expansion: include deps and children
    if recursive && !resources_to_push.is_empty() {
        let initial_names: std::collections::HashSet<(ResourceKind, String)> = resources_to_push
            .iter()
            .map(|(k, n, _, _)| (*k, n.clone()))
            .collect();

        // Load all local resources across all kinds for expansion
        let all_kinds = if config.sync.include_preview {
            ResourceKind::all().to_vec()
        } else {
            ResourceKind::stable().to_vec()
        };

        let mut all_local = Vec::new();
        let recurse_search_name = config
            .primary_search_service()
            .map(|s| s.name)
            .unwrap_or_default();
        for k in &all_kinds {
            let dir = if k.domain() == ServiceDomain::Search {
                config
                    .search_service_dir(&project_root, &recurse_search_name)
                    .join(k.directory_name())
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

    // Show what will be pushed
    println!("Resources to push:");

    for (kind, name, _, exists) in &resources_to_push {
        if *exists {
            println!(
                "  {} {} '{}' (update)",
                "~".yellow(),
                kind.display_name(),
                name
            );
            if let Some(changes) = change_details.get(&(*kind, name.clone())) {
                for line in describe_changes(changes, None) {
                    println!("{}", line);
                }
            }
        } else {
            println!(
                "  {} {} '{}' (create)",
                "+".green(),
                kind.display_name(),
                name
            );
        }
    }
    if total_unchanged > 0 {
        println!(
            "  {} resource(s) already match the server",
            total_unchanged.to_string().dimmed()
        );
    }
    println!();

    if dry_run {
        println!("Dry run - no changes made");
        return Ok(());
    }

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

    // Cache for discovered storage connection string (avoids repeated ARM calls)
    let mut cached_connection_string: Option<String> = None;

    // Push search resources
    if !search_resources.is_empty() {
        let client = if let Some(ref server) = target {
            AzureSearchClient::new_for_server(&config, server)?
        } else {
            AzureSearchClient::new(&config)?
        };

        for (kind, name, definition, exists) in &search_resources {
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
                        &config,
                        &mut cached_connection_string,
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
                }
                Err(e) => {
                    println!("FAILED: {}", e);
                    error_count += 1;
                }
            }
        }
    }

    // Push Foundry agents
    if !foundry_resources.is_empty() && config.has_foundry() {
        for foundry_config in config.foundry_services() {
            let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;

            for (kind, name, definition, exists) in &foundry_resources {
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
                    Ok(response) => {
                        println!("done");
                        success_count += 1;

                        // If this was a create, store the server-assigned ID in config.json
                        if !exists {
                            if let Some(id) = response.get("id").and_then(|v| v.as_str()) {
                                update_agent_config_id(
                                    &config,
                                    &project_root,
                                    foundry_config,
                                    name,
                                    id,
                                );
                            }
                        }
                    }
                    Err(e) => {
                        println!("FAILED: {}", e);
                        error_count += 1;
                    }
                }
            }
        }
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

/// Build a managed map from local KS files on disk.
fn build_local_managed_map(service_dir: &std::path::Path) -> ManagedMap {
    let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");
    if !ks_base.exists() {
        return ManagedMap::new();
    }

    let mut ks_pairs = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&ks_base) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let ks_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            let ks_file = path.join(format!("{}.json", ks_name));
            if let Ok(content) = std::fs::read_to_string(&ks_file) {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
                    ks_pairs.push((ks_name, value));
                }
            }
        }
    }

    managed::build_managed_map(&ks_pairs)
}

/// Collect a single resource for push, checking immutability and diffing.
#[allow(clippy::too_many_arguments)]
async fn collect_push_resource(
    client: &AzureSearchClient,
    kind: ResourceKind,
    name: &str,
    local: serde_json::Value,
    resources_to_push: &mut Vec<(ResourceKind, String, serde_json::Value, bool)>,
    validation_errors: &mut Vec<String>,
    recreate_candidates: &mut Vec<(ResourceKind, String)>,
    total_unchanged: &mut usize,
    change_details: &mut HashMap<(ResourceKind, String), Vec<Change>>,
) {
    let remote = client.get(kind, name).await;

    match remote {
        Ok(existing) => {
            let volatile_fields = get_volatile_fields(kind);
            let read_only_fields = get_read_only_fields(kind);
            let push_strip: Vec<&str> = volatile_fields
                .iter()
                .chain(read_only_fields.iter())
                .copied()
                .collect();
            let normalized_existing = normalize(&existing, &push_strip);
            let normalized_local = normalize(&local, &push_strip);

            let violations =
                check_immutability(kind, name, &normalized_existing, &normalized_local);

            if !violations.is_empty() {
                let has_recreate = violations
                    .iter()
                    .any(|v| v.severity == ViolationSeverity::RequiresRecreate);
                let has_hard_block = violations
                    .iter()
                    .any(|v| v.severity == ViolationSeverity::HardBlock);

                if has_hard_block {
                    for v in violations {
                        validation_errors.push(format!("{}", v));
                    }
                } else if has_recreate {
                    recreate_candidates.push((kind, name.to_string()));
                }
            } else {
                let remote_json = format_json(&normalized_existing);
                let local_json = format_json(&normalized_local);

                if remote_json == local_json {
                    *total_unchanged += 1;
                } else {
                    let diff_result =
                        hoist_diff::diff(&normalized_existing, &normalized_local, "name");
                    change_details.insert((kind, name.to_string()), diff_result.changes);
                    resources_to_push.push((kind, name.to_string(), local, true));
                }
            }
        }
        Err(hoist_client::ClientError::NotFound { .. }) => {
            resources_to_push.push((kind, name.to_string(), local, false));
        }
        Err(e) => {
            validation_errors.push(format!(
                "Error checking {} '{}': {}",
                kind.display_name(),
                name,
                e
            ));
        }
    }
}

/// After creating a new agent, update the local config.json with the server-assigned ID.
fn update_agent_config_id(
    config: &hoist_core::Config,
    project_root: &std::path::Path,
    foundry_config: &FoundryServiceConfig,
    agent_name: &str,
    agent_id: &str,
) {
    let agents_dir = foundry_agents_dir(config, project_root, foundry_config);
    let config_path = agents_dir.join(agent_name).join("config.json");

    if let Ok(content) = std::fs::read_to_string(&config_path) {
        if let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&content) {
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "id".to_string(),
                    serde_json::Value::String(agent_id.to_string()),
                );
                if let Ok(formatted) = serde_json::to_string_pretty(&value) {
                    let _ = std::fs::write(&config_path, formatted);
                    info!(
                        "Updated agent '{}' config with id '{}'",
                        agent_name, agent_id
                    );
                }
            }
        }
    }
}

/// Get the directory path for Foundry agents.
fn foundry_agents_dir(
    config: &hoist_core::Config,
    project_root: &std::path::Path,
    foundry_config: &FoundryServiceConfig,
) -> std::path::PathBuf {
    config
        .foundry_service_dir(project_root, &foundry_config.name, &foundry_config.project)
        .join("agents")
}

/// Remove volatile and read-only fields from a resource definition before sending to Azure.
///
/// Recurses through the entire JSON tree, stripping field names at every level.
/// This handles both top-level fields (like `@odata.etag`) and nested read-only
/// fields (like `createdResources` inside `azureBlobParameters`).
///
/// Volatile fields (etag, context, secrets) are always stripped.
/// Read-only fields (knowledgeSources, createdResources, etc.) are kept in local
/// files for documentation but must be stripped before push since Azure rejects them.
fn strip_volatile_fields(kind: ResourceKind, definition: &serde_json::Value) -> serde_json::Value {
    let volatile_fields = get_volatile_fields(kind);
    let read_only_fields = get_read_only_fields(kind);
    let all_fields: Vec<&str> = volatile_fields
        .iter()
        .chain(read_only_fields.iter())
        .copied()
        .collect();
    strip_fields_recursive(definition, &all_fields)
}

/// Check if a resource needs credential injection before push.
///
/// Returns true when:
/// - DataSource being created (or recreated) without a `credentials` object
/// - KnowledgeSource being created (or recreated) with `<redacted>` connectionString
fn needs_credentials(
    kind: ResourceKind,
    definition: &serde_json::Value,
    exists: bool,
    needs_recreate: bool,
) -> bool {
    let is_new = !exists || needs_recreate;
    if !is_new {
        return false;
    }

    match kind {
        ResourceKind::DataSource => {
            // credentials is a volatile field — stripped during pull, so it's absent on disk.
            // If someone manually added it, respect that.
            definition
                .get("credentials")
                .and_then(|c| c.get("connectionString"))
                .and_then(|s| s.as_str())
                .is_none_or(|s| s.is_empty())
        }
        ResourceKind::KnowledgeSource => {
            // Azure returns "<redacted>" for connectionString in GET responses.
            // Check if it's redacted or missing.
            let conn = definition
                .pointer("/azureBlobParameters/connectionString")
                .and_then(|v| v.as_str());
            matches!(conn, Some("<redacted>") | None)
        }
        _ => false,
    }
}

/// Discover a storage account connection string via ARM.
///
/// Falls back gracefully — returns None on any failure (not logged in,
/// no storage accounts found, etc.).
async fn discover_storage_credentials(
    config: &Config,
    cached: &mut Option<String>,
) -> Option<String> {
    // Return cached value if available
    if let Some(ref conn) = cached {
        return Some(conn.clone());
    }

    let arm = ArmClient::new().ok()?;

    // Get subscription ID: config first, then az cli
    let subscription_id = config
        .primary_search_service()
        .and_then(|s| s.subscription.clone())
        .or_else(|| {
            AzCliAuth::check_status()
                .ok()
                .and_then(|s| s.subscription_id)
        })?;

    // Get resource group: config first, then ARM discovery
    let search_svc = config.primary_search_service()?;
    let resource_group = if let Some(rg) = search_svc.resource_group.clone() {
        rg
    } else {
        arm.find_resource_group(&subscription_id, &search_svc.name)
            .await
            .ok()?
    };

    let accounts = arm
        .list_storage_accounts(&subscription_id, &resource_group)
        .await
        .ok()?;

    if accounts.is_empty() {
        return None;
    }

    let account_name = if accounts.len() == 1 {
        let name = &accounts[0].name;
        println!();
        info!("Auto-selected storage account: {}", name);
        name.clone()
    } else {
        println!();
        println!(
            "Multiple storage accounts found in resource group '{}':",
            resource_group
        );
        for (i, acct) in accounts.iter().enumerate() {
            println!("  [{}] {}", i + 1, acct);
        }
        print!("Select storage account [1]: ");
        io::stdout().flush().ok()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input).ok()?;
        let input = input.trim();

        let idx = if input.is_empty() {
            0
        } else {
            input.parse::<usize>().ok()?.checked_sub(1)?
        };

        accounts.get(idx)?.name.clone()
    };

    let conn_string = arm
        .get_storage_connection_string(&subscription_id, &resource_group, &account_name)
        .await
        .ok()?;

    *cached = Some(conn_string.clone());
    Some(conn_string)
}

/// Inject credentials into a resource definition for new data sources or knowledge sources.
///
/// Tries ARM-based auto-discovery first, then falls back to prompting the user.
async fn inject_credentials(
    kind: ResourceKind,
    definition: &serde_json::Value,
    name: &str,
    config: &Config,
    cached: &mut Option<String>,
) -> Result<serde_json::Value> {
    // Try auto-discovery first
    let conn_string = match discover_storage_credentials(config, cached).await {
        Some(c) => c,
        None => {
            // Fall back to manual prompt
            println!();
            print!(
                "Enter connection string for {} '{}' (or press Enter to skip): ",
                kind.display_name(),
                name
            );
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_string();
            if input.is_empty() {
                return Ok(definition.clone());
            }
            input
        }
    };

    let mut def = definition.clone();

    match kind {
        ResourceKind::DataSource => {
            // Inject {"credentials": {"connectionString": "..."}}
            if let Some(obj) = def.as_object_mut() {
                obj.insert(
                    "credentials".to_string(),
                    serde_json::json!({"connectionString": conn_string}),
                );
            }
        }
        ResourceKind::KnowledgeSource => {
            // Replace azureBlobParameters.connectionString
            if let Some(blob_params) = def.get_mut("azureBlobParameters") {
                if let Some(obj) = blob_params.as_object_mut() {
                    obj.insert(
                        "connectionString".to_string(),
                        serde_json::Value::String(conn_string),
                    );
                }
            }
        }
        _ => {}
    }

    Ok(def)
}

fn strip_fields_recursive(value: &serde_json::Value, fields: &[&str]) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let filtered: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter(|(k, _)| !fields.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), strip_fields_recursive(v, fields)))
                .collect();
            serde_json::Value::Object(filtered)
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|v| strip_fields_recursive(v, fields))
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strip_volatile_fields_removes_etag_and_context() {
        let definition = json!({
            "name": "test-index",
            "fields": [],
            "@odata.etag": "W/\"abc\"",
            "@odata.context": "https://svc.search.windows.net/$metadata#indexes/$entity"
        });
        let clean = strip_volatile_fields(ResourceKind::Index, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("fields"));
        assert!(!obj.contains_key("@odata.etag"));
        assert!(!obj.contains_key("@odata.context"));
    }

    #[test]
    fn test_strip_volatile_fields_removes_knowledge_source_top_level() {
        let definition = json!({
            "name": "ks-1",
            "indexName": "my-index",
            "description": "Test",
            "ingestionPermissionOptions": { "someConfig": true }
        });
        let clean = strip_volatile_fields(ResourceKind::KnowledgeSource, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("indexName"));
        assert!(obj.contains_key("description"));
        assert!(!obj.contains_key("ingestionPermissionOptions"));
    }

    #[test]
    fn test_strip_volatile_fields_removes_nested_created_resources() {
        let definition = json!({
            "name": "ks-1",
            "kind": "azureBlob",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>",
                "createdResources": {
                    "datasource": "ds-1",
                    "indexer": "ixer-1",
                    "skillset": "sk-1",
                    "index": "idx-1"
                }
            }
        });
        let clean = strip_volatile_fields(ResourceKind::KnowledgeSource, &definition);
        let blob_params = clean
            .get("azureBlobParameters")
            .unwrap()
            .as_object()
            .unwrap();
        assert!(blob_params.contains_key("containerName"));
        assert!(blob_params.contains_key("connectionString"));
        assert!(
            !blob_params.contains_key("createdResources"),
            "createdResources should be stripped from nested object"
        );
    }

    #[test]
    fn test_strip_volatile_fields_preserves_knowledge_base_knowledge_sources() {
        // knowledgeSources is a normal pushable field — NOT stripped
        let definition = json!({
            "name": "my-kb",
            "description": "Test KB",
            "knowledgeSources": [
                {"name": "ks-1"},
                {"name": "ks-2"}
            ]
        });
        let clean = strip_volatile_fields(ResourceKind::KnowledgeBase, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("description"));
        assert!(
            obj.contains_key("knowledgeSources"),
            "knowledgeSources is pushable and should be preserved"
        );
    }

    #[test]
    fn test_strip_volatile_fields_removes_datasource_credentials() {
        let definition = json!({
            "name": "ds-1",
            "type": "azureblob",
            "credentials": { "connectionString": "secret" }
        });
        let clean = strip_volatile_fields(ResourceKind::DataSource, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(!obj.contains_key("credentials"));
    }

    #[test]
    fn test_strip_volatile_fields_preserves_non_volatile() {
        let definition = json!({
            "name": "sk-1",
            "skills": [{"name": "skill1"}],
            "description": "My skillset"
        });
        let clean = strip_volatile_fields(ResourceKind::Skillset, &definition);
        assert_eq!(clean, definition);
    }

    #[test]
    fn test_strip_volatile_fields_handles_no_volatile_present() {
        let definition = json!({
            "name": "test",
            "fields": []
        });
        let clean = strip_volatile_fields(ResourceKind::Index, &definition);
        assert_eq!(clean, definition);
    }

    #[test]
    fn test_strip_volatile_fields_removes_indexer_start_time() {
        let definition = json!({
            "name": "my-indexer",
            "dataSourceName": "ds-1",
            "targetIndexName": "idx-1",
            "schedule": {
                "interval": "P1D",
                "startTime": "2026-02-06T22:03:10.254Z"
            }
        });
        let clean = strip_volatile_fields(ResourceKind::Indexer, &definition);
        let schedule = clean.get("schedule").unwrap().as_object().unwrap();
        assert!(schedule.contains_key("interval"));
        assert!(
            !schedule.contains_key("startTime"),
            "startTime should be stripped from indexer schedule"
        );
    }

    /// Verifies that push comparison strips read-only fields from both sides
    /// so they don't produce false diffs (e.g. createdResources in KS).
    #[test]
    fn test_push_comparison_strips_read_only_from_both_sides() {
        use crate::commands::common::{get_read_only_fields, get_volatile_fields};
        use hoist_core::normalize::{format_json, normalize};

        // Local KS file has createdResources (preserved by pull for documentation)
        let local = json!({
            "name": "ks-1",
            "indexName": "my-index",
            "azureBlobParameters": {
                "containerName": "docs",
                "createdResources": {"datasource": "ds-1"}
            }
        });

        // Remote KS also has createdResources
        let remote = json!({
            "name": "ks-1",
            "indexName": "my-index",
            "azureBlobParameters": {
                "containerName": "docs",
                "createdResources": {"datasource": "ds-1"}
            }
        });

        // Push combines volatile + read_only for comparison
        let volatile = get_volatile_fields(ResourceKind::KnowledgeSource);
        let read_only = get_read_only_fields(ResourceKind::KnowledgeSource);
        let push_strip: Vec<&str> = volatile.iter().chain(read_only.iter()).copied().collect();

        let normalized_remote = normalize(&remote, &push_strip);
        let normalized_local = normalize(&local, &push_strip);

        // createdResources stripped from both → no false diff
        assert_eq!(
            format_json(&normalized_remote),
            format_json(&normalized_local)
        );

        // But a real change is still detected
        let local_modified = json!({
            "name": "ks-1",
            "indexName": "other-index",
            "azureBlobParameters": {
                "containerName": "docs",
                "createdResources": {"datasource": "ds-1"}
            }
        });
        let normalized_modified = normalize(&local_modified, &push_strip);
        assert_ne!(
            format_json(&normalized_remote),
            format_json(&normalized_modified)
        );
    }

    /// Verifies that pull normalization keeps read-only fields in local files
    /// and that pushable fields like knowledgeSources are also preserved.
    #[test]
    fn test_pull_normalization_preserves_non_volatile_fields() {
        use crate::commands::common::get_volatile_fields;
        use hoist_core::normalize::normalize;

        let remote = json!({
            "name": "my-kb",
            "description": "Test",
            "@odata.etag": "W/\"abc\"",
            "storageConnectionStringSecret": "secret",
            "knowledgeSources": [{"name": "ks-1"}]
        });

        // Pull uses only volatile_fields
        let volatile = get_volatile_fields(ResourceKind::KnowledgeBase);
        let normalized = normalize(&remote, &volatile);
        let obj = normalized.as_object().unwrap();

        // Volatile fields stripped
        assert!(!obj.contains_key("@odata.etag"));
        assert!(!obj.contains_key("storageConnectionStringSecret"));

        // Pushable fields preserved
        assert!(
            obj.contains_key("knowledgeSources"),
            "knowledgeSources is pushable and must be preserved"
        );
    }

    /// Verifies that knowledgeSources changes on a KB are detected by push.
    #[test]
    fn test_push_detects_knowledge_sources_change() {
        use crate::commands::common::{get_read_only_fields, get_volatile_fields};
        use hoist_core::normalize::{format_json, normalize};

        let local = json!({
            "name": "my-kb",
            "knowledgeSources": [{"name": "ks-1"}]
        });

        let remote = json!({
            "name": "my-kb",
            "knowledgeSources": [{"name": "ks-1"}, {"name": "ks-2"}]
        });

        let volatile = get_volatile_fields(ResourceKind::KnowledgeBase);
        let read_only = get_read_only_fields(ResourceKind::KnowledgeBase);
        let push_strip: Vec<&str> = volatile.iter().chain(read_only.iter()).copied().collect();

        let normalized_remote = normalize(&remote, &push_strip);
        let normalized_local = normalize(&local, &push_strip);

        // knowledgeSources is pushable — change must be detected
        assert_ne!(
            format_json(&normalized_remote),
            format_json(&normalized_local),
            "knowledgeSources change must be detected by push"
        );
    }

    // --- Credential injection tests ---

    #[test]
    fn test_needs_credentials_datasource_new_no_creds() {
        let def = json!({"name": "ds-1", "type": "azureblob", "container": {"name": "docs"}});
        assert!(needs_credentials(
            ResourceKind::DataSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_datasource_update() {
        let def = json!({"name": "ds-1", "type": "azureblob", "container": {"name": "docs"}});
        // Existing resource — Azure preserves credentials on update
        assert!(!needs_credentials(
            ResourceKind::DataSource,
            &def,
            true,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_datasource_recreate() {
        let def = json!({"name": "ds-1", "type": "azureblob", "container": {"name": "docs"}});
        // Drop-and-recreate needs credentials like a new resource
        assert!(needs_credentials(
            ResourceKind::DataSource,
            &def,
            true,
            true
        ));
    }

    #[test]
    fn test_needs_credentials_datasource_with_creds() {
        let def = json!({
            "name": "ds-1",
            "type": "azureblob",
            "credentials": {"connectionString": "DefaultEndpointsProtocol=https;AccountName=..."},
            "container": {"name": "docs"}
        });
        // Credentials already present — no injection needed
        assert!(!needs_credentials(
            ResourceKind::DataSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_redacted() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        assert!(needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_missing_connection_string() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {"containerName": "docs"}
        });
        assert!(needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_real_value() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "DefaultEndpointsProtocol=https;AccountName=test;AccountKey=abc"
            }
        });
        assert!(!needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_update() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        // Existing KS update — Azure preserves credentials
        assert!(!needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            true,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_index() {
        let def = json!({"name": "idx-1", "fields": []});
        // Indexes don't need credentials
        assert!(!needs_credentials(ResourceKind::Index, &def, false, false));
    }

    fn test_config() -> hoist_core::Config {
        let toml_str = r#"
            [[services.search]]
            name = "test-search"
        "#;
        toml::from_str(toml_str).unwrap()
    }

    #[tokio::test]
    async fn test_inject_credentials_datasource() {
        let def = json!({
            "name": "ds-1",
            "type": "azureblob",
            "container": {"name": "docs"}
        });
        let conn = "DefaultEndpointsProtocol=https;AccountName=test;AccountKey=abc";
        let mut cached = Some(conn.to_string());
        let config = test_config();

        let result =
            inject_credentials(ResourceKind::DataSource, &def, "ds-1", &config, &mut cached)
                .await
                .unwrap();

        assert_eq!(
            result
                .get("credentials")
                .unwrap()
                .get("connectionString")
                .unwrap()
                .as_str()
                .unwrap(),
            conn
        );
        // Original fields preserved
        assert_eq!(result.get("name").unwrap().as_str().unwrap(), "ds-1");
        assert_eq!(result.get("type").unwrap().as_str().unwrap(), "azureblob");
    }

    #[tokio::test]
    async fn test_inject_credentials_ks() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        let conn = "DefaultEndpointsProtocol=https;AccountName=test;AccountKey=abc";
        let mut cached = Some(conn.to_string());
        let config = test_config();

        let result = inject_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            "ks-1",
            &config,
            &mut cached,
        )
        .await
        .unwrap();

        assert_eq!(
            result
                .pointer("/azureBlobParameters/connectionString")
                .unwrap()
                .as_str()
                .unwrap(),
            conn
        );
        // Original fields preserved
        assert_eq!(
            result
                .pointer("/azureBlobParameters/containerName")
                .unwrap()
                .as_str()
                .unwrap(),
            "docs"
        );
    }
}
