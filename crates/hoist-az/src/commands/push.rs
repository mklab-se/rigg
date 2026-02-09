//! Push resources to Azure

use std::collections::HashMap;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use colored::Colorize;
use tracing::info;

use hoist_client::AzureSearchClient;
use hoist_core::config::FoundryServiceConfig;
use hoist_core::constraints::check_immutability;
use hoist_core::copy::NameMap;
use hoist_core::normalize::{format_json, normalize};
use hoist_core::resources::agent::compose_agent;
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;
use hoist_diff::Change;

use crate::cli::ResourceTypeFlags;
use crate::commands::common::{
    get_read_only_fields, get_volatile_fields, order_by_dependencies, read_agent_files,
    resolve_resource_selection_from_flags,
};
use crate::commands::confirm::prompt_yes_no;
use crate::commands::describe::describe_changes;
use crate::commands::load_config;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    flags: &ResourceTypeFlags,
    recursive: bool,
    filter: Option<String>,
    dry_run: bool,
    force: bool,
    target: Option<String>,
    copy: bool,
    suffix: Option<String>,
    answers: Option<PathBuf>,
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

    let is_copy_mode = copy || suffix.is_some() || answers.is_some();

    // Compute server name for display (used in copy mode output)
    let default_name = config
        .primary_search_service()
        .map(|s| s.name)
        .unwrap_or_default();
    let server_name = target.as_deref().unwrap_or(&default_name);

    // Collect resources to push
    let mut resources_to_push = Vec::new();
    let mut validation_errors = Vec::new();
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
        for kind in &search_kinds {
            let resource_dir = config
                .search_service_dir(&project_root, &push_search_svc.name)
                .join(kind.directory_name());
            if !resource_dir.exists() {
                continue;
            }

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
                if let Some(exact_name) = selection.name_filter(*kind) {
                    if name != exact_name {
                        continue;
                    }
                }

                // Filter if pattern specified (substring match)
                if let Some(ref pattern) = filter {
                    if !name.contains(pattern) {
                        continue;
                    }
                }

                // Read and parse local file
                let content = std::fs::read_to_string(&path)?;
                let local: serde_json::Value = serde_json::from_str(&content)?;

                // In copy mode, skip immutability checks (we're creating new resources)
                if is_copy_mode {
                    resources_to_push.push((*kind, name.to_string(), local, false));
                    continue;
                }

                // Check if resource exists on server
                let remote = client.get(*kind, name).await;

                match remote {
                    Ok(existing) => {
                        // For push comparison, strip both volatile fields (etag, context,
                        // secrets) AND read-only fields (knowledgeSources, createdResources,
                        // startTime, etc.)
                        let volatile_fields = get_volatile_fields(*kind);
                        let read_only_fields = get_read_only_fields(*kind);
                        let push_strip: Vec<&str> = volatile_fields
                            .iter()
                            .chain(read_only_fields.iter())
                            .copied()
                            .collect();
                        let normalized_existing = normalize(&existing, &push_strip, "name");
                        let normalized_local = normalize(&local, &push_strip, "name");

                        // Validate immutability constraints
                        let violations = check_immutability(
                            *kind,
                            name,
                            &normalized_existing,
                            &normalized_local,
                        );

                        if !violations.is_empty() {
                            for v in violations {
                                validation_errors.push(format!("{}", v));
                            }
                        } else {
                            // Compare normalized remote against normalized local
                            let remote_json = format_json(&normalized_existing);
                            let local_json = format_json(&normalized_local);

                            if remote_json == local_json {
                                total_unchanged += 1;
                            } else {
                                // Compute diff: old=server, new=local
                                let diff_result = hoist_diff::diff(
                                    &normalized_existing,
                                    &normalized_local,
                                    "name",
                                );
                                change_details
                                    .insert((*kind, name.to_string()), diff_result.changes);
                                resources_to_push.push((*kind, name.to_string(), local, true));
                            }
                        }
                    }
                    Err(hoist_client::ClientError::NotFound { .. }) => {
                        resources_to_push.push((*kind, name.to_string(), local, false));
                    }
                    Err(e) => return Err(e.into()),
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

            // Get existing agents to map name -> id
            let existing_agents = foundry_client.list_agents().await?;
            let agent_id_map: HashMap<String, String> = existing_agents
                .iter()
                .filter_map(|a| {
                    let name = a.get("name")?.as_str()?.to_string();
                    let id = a.get("id")?.as_str()?.to_string();
                    Some((name, id))
                })
                .collect();

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

                if is_copy_mode {
                    let exists = agent_id_map.contains_key(&name);
                    resources_to_push.push((ResourceKind::Agent, name, payload, exists));
                    continue;
                }

                // Compare local vs remote to skip unchanged agents
                match remote_agent_map.get(&name) {
                    Some(remote) => {
                        let normalized_local =
                            hoist_core::normalize::normalize(&payload, volatile, "name");
                        let normalized_remote =
                            hoist_core::normalize::normalize(remote, volatile, "name");

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
                // In copy mode, mark as new (false); otherwise check server later
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

    // Apply copy mode transformations
    if is_copy_mode {
        let name_map = build_name_map(
            &resources_to_push,
            copy,
            suffix.as_deref(),
            answers.as_deref(),
        )?;

        // Apply name mappings and reference rewrites
        for (kind, name, definition, _exists) in &mut resources_to_push {
            if let Some(new_name) = name_map.get(*kind, name) {
                *name = new_name.to_string();
                // Update the "name" field in the JSON definition
                if let Some(obj) = definition.as_object_mut() {
                    obj.insert("name".to_string(), serde_json::Value::String(name.clone()));
                }
            }

            // Rewrite cross-references
            let warnings = hoist_core::copy::rewrite_references(*kind, definition, &name_map);
            for warning in warnings {
                println!("  Warning: {}", warning);
            }
        }

        // Prompt for credentials needed to create new resources
        prompt_copy_secrets(&mut resources_to_push)?;
    }

    // Show what will be pushed
    if is_copy_mode && target.is_some() {
        println!("Resources to copy to '{}':", server_name);
    } else if is_copy_mode {
        println!("Resources to copy:");
    } else {
        println!("Resources to push:");
    }

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

    // Push search resources
    if !search_resources.is_empty() {
        let client = if let Some(ref server) = target {
            AzureSearchClient::new_for_server(&config, server)?
        } else {
            AzureSearchClient::new(&config)?
        };

        for (kind, name, definition, exists) in &search_resources {
            let action = if *exists { "Updating" } else { "Creating" };
            print!("{} {} '{}'... ", action, kind.display_name(), name);
            io::stdout().flush()?;

            let clean_definition = strip_volatile_fields(*kind, definition);

            match client
                .create_or_update(*kind, name, &clean_definition)
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

/// Build a NameMap from the copy flags (interactive, suffix, or answers file).
fn build_name_map(
    resources: &[(ResourceKind, String, serde_json::Value, bool)],
    interactive: bool,
    suffix: Option<&str>,
    answers_file: Option<&std::path::Path>,
) -> Result<NameMap> {
    if let Some(path) = answers_file {
        return NameMap::from_answers_file(path);
    }

    if let Some(suffix) = suffix {
        let pairs: Vec<_> = resources
            .iter()
            .map(|(k, n, _, _)| (*k, n.clone()))
            .collect();
        return Ok(NameMap::from_suffix(&pairs, suffix));
    }

    if interactive {
        return prompt_name_map(resources);
    }

    Ok(NameMap::new())
}

/// Interactively prompt the user for new names for each resource.
fn prompt_name_map(
    resources: &[(ResourceKind, String, serde_json::Value, bool)],
) -> Result<NameMap> {
    let mut name_map = NameMap::new();
    println!("Enter new names for each resource (press Enter to keep the same name):");

    for (kind, name, _, _) in resources {
        print!("  {} '{}' -> ", kind.display_name(), name);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let new_name = input.trim();

        if !new_name.is_empty() && new_name != name {
            name_map.insert(*kind, name, new_name);
        }
    }
    println!();

    Ok(name_map)
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

/// Find all string fields with `<redacted>` placeholder values in a JSON tree.
/// Returns dot-separated paths (e.g., "azureBlobParameters.connectionString").
fn find_redacted_fields(value: &serde_json::Value, prefix: &str) -> Vec<String> {
    let mut fields = Vec::new();
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let path = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{}.{}", prefix, k)
                };
                fields.extend(find_redacted_fields(v, &path));
            }
        }
        serde_json::Value::String(s) if s == "<redacted>" || s == "<REDACTED>" => {
            fields.push(prefix.to_string());
        }
        _ => {}
    }
    fields
}

/// Set a value at a dot-separated JSON path (e.g., "credentials.connectionString").
/// Creates intermediate objects if they don't exist.
fn set_at_path(value: &mut serde_json::Value, path: &str, new_value: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = value;

    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Set the leaf value
            if let Some(obj) = current.as_object_mut() {
                obj.insert(
                    part.to_string(),
                    serde_json::Value::String(new_value.to_string()),
                );
            }
        } else {
            // Navigate or create intermediate objects
            if current.get(*part).is_none() {
                if let Some(obj) = current.as_object_mut() {
                    obj.insert(part.to_string(), serde_json::json!({}));
                }
            }
            current = current.get_mut(*part).unwrap();
        }
    }
}

/// Collect secrets needed for each resource in copy mode and return them as
/// (index, field_path, prompt_text) tuples.
fn collect_copy_secrets(
    resources: &[(ResourceKind, String, serde_json::Value, bool)],
) -> Vec<(usize, String, String)> {
    let mut secrets = Vec::new();

    for (i, (kind, name, definition, _)) in resources.iter().enumerate() {
        // Check for missing credential fields (stripped as volatile during pull)
        match kind {
            ResourceKind::DataSource => {
                if definition.get("credentials").is_none() {
                    secrets.push((
                        i,
                        "credentials.connectionString".to_string(),
                        format!("Connection string for Data Source '{}'", name),
                    ));
                }
            }
            ResourceKind::KnowledgeBase => {
                if definition.get("storageConnectionStringSecret").is_none() {
                    secrets.push((
                        i,
                        "storageConnectionStringSecret".to_string(),
                        format!("Storage connection string for Knowledge Base '{}' (press Enter to skip)", name),
                    ));
                }
            }
            _ => {}
        }

        // Check for <redacted> placeholder values anywhere in the definition
        let redacted = find_redacted_fields(definition, "");
        for path in redacted {
            secrets.push((
                i,
                path.clone(),
                format!("{} for {} '{}'", path, kind.display_name(), name),
            ));
        }
    }

    secrets
}

/// Prompt the user for credentials needed to create new resources in copy mode.
///
/// Tries to auto-discover storage account connection strings via the ARM API.
/// Falls back to interactive prompts if discovery fails.
fn prompt_copy_secrets(
    resources: &mut [(ResourceKind, String, serde_json::Value, bool)],
) -> Result<()> {
    let secrets = collect_copy_secrets(resources);

    if secrets.is_empty() {
        return Ok(());
    }

    // Try to discover connection strings via ARM (runs a small tokio block)
    let discovered = discover_connection_strings();

    if !discovered.is_empty() {
        println!(
            "New resources need credentials. Found {} storage account(s):",
            discovered.len()
        );

        // Let the user pick which connection string to use
        let conn_str = if discovered.len() == 1 {
            let (name, conn) = &discovered[0];
            println!("  Using storage account '{}'", name);
            Some(conn.clone())
        } else {
            println!();
            for (i, (name, _)) in discovered.iter().enumerate() {
                println!("  {}. {}", i + 1, name);
            }
            print!("\nWhich storage account to use? [1]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let choice = input.trim();

            let idx = if choice.is_empty() {
                0
            } else {
                choice.parse::<usize>().unwrap_or(1).saturating_sub(1)
            };

            discovered.get(idx).map(|(_, conn)| conn.clone())
        };

        if let Some(conn_str) = conn_str {
            // Apply the connection string to all resources that need it
            for (idx, path, _) in &secrets {
                set_at_path(&mut resources[*idx].2, path, &conn_str);
            }
            println!();
            return Ok(());
        }
    }

    // Fallback: prompt individually
    println!("New resources need credentials that aren't stored in local files.");
    println!("Enter values below (secrets are not masked):\n");

    for (idx, path, prompt) in &secrets {
        print!("  {}: ", prompt);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        let value = input.trim();

        if value.is_empty() {
            continue; // Skip — user pressed Enter
        }

        // Inject the value into the resource definition
        set_at_path(&mut resources[*idx].2, path, value);
    }
    println!();

    Ok(())
}

/// Try to discover storage account connection strings via the ARM API.
///
/// Returns a list of (account_name, connection_string) pairs.
/// Returns empty vec on any failure (falls back to manual prompts).
fn discover_connection_strings() -> Vec<(String, String)> {
    // Load config to get subscription ID and service name
    let Ok((_, config)) = crate::commands::load_config() else {
        return Vec::new();
    };

    let primary = match config.primary_search_service() {
        Some(s) => s,
        None => return Vec::new(),
    };

    let subscription_id = match &primary.subscription {
        Some(sub) => sub.clone(),
        None => {
            // Try to get from az account show
            match hoist_client::auth::AzCliAuth::check_status() {
                Ok(status) => match status.subscription_id {
                    Some(id) => id,
                    None => return Vec::new(),
                },
                Err(_) => return Vec::new(),
            }
        }
    };

    // Use a blocking runtime since we're inside sync code called from async context
    let rt = match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // We're inside an async runtime — use block_in_place
            return tokio::task::block_in_place(|| {
                handle.block_on(discover_connection_strings_async(
                    &subscription_id,
                    &primary.name,
                    primary.resource_group.as_deref(),
                ))
            });
        }
        Err(_) => {
            // No runtime — create one
            match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(_) => return Vec::new(),
            }
        }
    };

    rt.block_on(discover_connection_strings_async(
        &subscription_id,
        &primary.name,
        primary.resource_group.as_deref(),
    ))
}

async fn discover_connection_strings_async(
    subscription_id: &str,
    service_name: &str,
    resource_group: Option<&str>,
) -> Vec<(String, String)> {
    let arm = match hoist_client::ArmClient::new() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // Find resource group (from config or by looking up the search service)
    let rg = match resource_group {
        Some(rg) => rg.to_string(),
        None => match arm.find_resource_group(subscription_id, service_name).await {
            Ok(rg) => rg,
            Err(_) => return Vec::new(),
        },
    };

    // List storage accounts in the same resource group
    let accounts = match arm.list_storage_accounts(subscription_id, &rg).await {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };

    // Get connection strings for each
    let mut results = Vec::new();
    for account in &accounts {
        if let Ok(conn) = arm
            .get_storage_connection_string(subscription_id, &rg, &account.name)
            .await
        {
            results.push((account.name.clone(), conn));
        }
    }

    results
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

        let normalized_remote = normalize(&remote, &push_strip, "name");
        let normalized_local = normalize(&local, &push_strip, "name");

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
        let normalized_modified = normalize(&local_modified, &push_strip, "name");
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
        let normalized = normalize(&remote, &volatile, "name");
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

    // === find_redacted_fields tests ===

    #[test]
    fn test_find_redacted_fields_top_level() {
        let value = json!({
            "name": "test",
            "connectionString": "<redacted>"
        });
        let fields = find_redacted_fields(&value, "");
        assert_eq!(fields, vec!["connectionString"]);
    }

    #[test]
    fn test_find_redacted_fields_nested() {
        let value = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        let fields = find_redacted_fields(&value, "");
        assert_eq!(fields, vec!["azureBlobParameters.connectionString"]);
    }

    #[test]
    fn test_find_redacted_fields_none() {
        let value = json!({
            "name": "test",
            "description": "Normal value"
        });
        let fields = find_redacted_fields(&value, "");
        assert!(fields.is_empty());
    }

    #[test]
    fn test_find_redacted_fields_uppercase() {
        let value = json!({"secret": "<REDACTED>"});
        let fields = find_redacted_fields(&value, "");
        assert_eq!(fields, vec!["secret"]);
    }

    // === set_at_path tests ===

    #[test]
    fn test_set_at_path_simple() {
        let mut value = json!({"name": "test"});
        set_at_path(&mut value, "description", "hello");
        assert_eq!(value["description"], "hello");
    }

    #[test]
    fn test_set_at_path_nested_existing() {
        let mut value = json!({
            "azureBlobParameters": {
                "connectionString": "<redacted>",
                "containerName": "docs"
            }
        });
        set_at_path(
            &mut value,
            "azureBlobParameters.connectionString",
            "real-secret",
        );
        assert_eq!(
            value["azureBlobParameters"]["connectionString"],
            "real-secret"
        );
        assert_eq!(value["azureBlobParameters"]["containerName"], "docs");
    }

    #[test]
    fn test_set_at_path_creates_intermediate() {
        let mut value = json!({"name": "ds-1"});
        set_at_path(&mut value, "credentials.connectionString", "secret");
        assert_eq!(value["credentials"]["connectionString"], "secret");
    }

    // === collect_copy_secrets tests ===

    #[test]
    fn test_collect_copy_secrets_datasource_missing_credentials() {
        let resources = vec![(
            ResourceKind::DataSource,
            "ds-1".to_string(),
            json!({"name": "ds-1", "type": "azureblob"}),
            false,
        )];
        let secrets = collect_copy_secrets(&resources);
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].1, "credentials.connectionString");
    }

    #[test]
    fn test_collect_copy_secrets_ks_redacted() {
        let resources = vec![(
            ResourceKind::KnowledgeSource,
            "ks-1".to_string(),
            json!({
                "name": "ks-1",
                "azureBlobParameters": {
                    "connectionString": "<redacted>"
                }
            }),
            false,
        )];
        let secrets = collect_copy_secrets(&resources);
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].1, "azureBlobParameters.connectionString");
    }

    #[test]
    fn test_collect_copy_secrets_index_needs_nothing() {
        let resources = vec![(
            ResourceKind::Index,
            "idx-1".to_string(),
            json!({"name": "idx-1", "fields": []}),
            false,
        )];
        let secrets = collect_copy_secrets(&resources);
        assert!(secrets.is_empty());
    }

    #[test]
    fn test_collect_copy_secrets_kb_missing_storage_secret() {
        let resources = vec![(
            ResourceKind::KnowledgeBase,
            "kb-1".to_string(),
            json!({"name": "kb-1", "description": "Test"}),
            false,
        )];
        let secrets = collect_copy_secrets(&resources);
        assert_eq!(secrets.len(), 1);
        assert_eq!(secrets[0].1, "storageConnectionStringSecret");
    }

    // read_agent_files tests are in common.rs (the function lives there now)

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

        let normalized_remote = normalize(&remote, &push_strip, "name");
        let normalized_local = normalize(&local, &push_strip, "name");

        // knowledgeSources is pushable — change must be detected
        assert_ne!(
            format_json(&normalized_remote),
            format_json(&normalized_local),
            "knowledgeSources change must be detected by push"
        );
    }
}
