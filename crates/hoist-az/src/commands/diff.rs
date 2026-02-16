//! Show differences between local and remote

use anyhow::Result;
use colored::Colorize;
use tracing::info;

use hoist_client::AzureSearchClient;
use hoist_core::normalize::normalize;
use hoist_core::resources::agent::{agent_volatile_fields, strip_agent_empty_fields};
use hoist_core::resources::managed::{self, ManagedMap};
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;
use hoist_diff::{
    diff,
    output::{format_report, OutputFormat},
};

use crate::cli::{DiffFormat, ResourceTypeFlags};
use crate::commands::common::{
    get_read_only_fields, get_volatile_fields, read_agent_yaml,
    resolve_resource_selection_from_flags,
};
use crate::commands::describe::describe_changes;
use crate::commands::load_config_and_env;

pub async fn run(
    flags: &ResourceTypeFlags,
    format: DiffFormat,
    exit_code: bool,
    env_override: Option<&str>,
    compare_env: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    // Cross-environment diff: compare two remotes directly
    if let Some(right_env_name) = compare_env {
        let right_env = config
            .resolve_env(Some(right_env_name))
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        return run_cross_env_diff(flags, format, exit_code, &env, &right_env).await;
    }

    // Determine which resource types to diff
    let selection = resolve_resource_selection_from_flags(flags, env.sync.include_preview, true);

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

    let primary_search_svc = env.primary_search_service();

    let mut all_diffs = Vec::new();
    let mut has_changes = false;

    // --- Search resources ---
    if let (false, Some(search_svc)) = (search_kinds.is_empty(), primary_search_svc) {
        let client = AzureSearchClient::from_service_config(search_svc)?;

        let service_dir = env.search_service_dir(&files_root, search_svc);

        // Build managed map from local KS files
        let managed_map = build_local_managed_map(&service_dir);

        let has_ks = search_kinds.contains(&ResourceKind::KnowledgeSource);

        for kind in &search_kinds {
            // Strip both volatile and read-only fields — matches push behavior.
            // Read-only fields (knowledgeSources, createdResources, etc.) can't be
            // pushed, so showing them as diffs would be misleading.
            let volatile = get_volatile_fields(*kind);
            let read_only = get_read_only_fields(*kind);
            let strip_fields: Vec<&str> =
                volatile.iter().chain(read_only.iter()).copied().collect();

            let exact_name = selection.name_filter(*kind);

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
                    if let Some(exact) = exact_name {
                        if ks_name != exact {
                            continue;
                        }
                    }
                    let ks_file = path.join(format!("{}.json", ks_name));
                    if !ks_file.exists() {
                        continue;
                    }

                    diff_resource(
                        &client,
                        *kind,
                        &ks_name,
                        &ks_file,
                        &strip_fields,
                        &mut all_diffs,
                        &mut has_changes,
                    )
                    .await?;

                    // If --knowledge-sources, also diff managed sub-resources
                    if has_ks {
                        let managed_subs = managed::read_managed_sub_resources(&path, &ks_name);
                        for (sub_kind, sub_name, sub_def) in managed_subs {
                            let sub_volatile = get_volatile_fields(sub_kind);
                            let sub_read_only = get_read_only_fields(sub_kind);
                            let sub_strip: Vec<&str> = sub_volatile
                                .iter()
                                .chain(sub_read_only.iter())
                                .copied()
                                .collect();
                            diff_resource_value(
                                &client,
                                sub_kind,
                                &sub_name,
                                &sub_def,
                                &sub_strip,
                                &mut all_diffs,
                                &mut has_changes,
                            )
                            .await?;
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

                // Skip managed resources — they're diffed via KS cascade
                if managed::managing_ks(&managed_map, *kind, name).is_some() {
                    continue;
                }

                // Filter by singular flag (exact name match)
                if let Some(exact) = exact_name {
                    if name != exact {
                        continue;
                    }
                }

                diff_resource(
                    &client,
                    *kind,
                    name,
                    &path,
                    &strip_fields,
                    &mut all_diffs,
                    &mut has_changes,
                )
                .await?;
            }

            // Check for remote-only resources (will be kept, not deleted)
            let remote_resources = client.list(*kind).await?;
            let resource_dir = service_dir.join(kind.directory_name());
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
    if !foundry_kinds.is_empty() && env.has_foundry() {
        let exact_name = selection.name_filter(ResourceKind::Agent);
        let volatile = agent_volatile_fields();

        for foundry_config in &env.foundry {
            let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;
            info!(
                "Connected to Foundry {}/{} using {}",
                foundry_config.name,
                foundry_config.project,
                foundry_client.auth_method()
            );

            let agents_dir = env
                .foundry_service_dir(&files_root, foundry_config)
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
                    if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                        continue;
                    }

                    let name = match path.file_stem().and_then(|n| n.to_str()) {
                        Some(n) => n.to_string(),
                        None => continue,
                    };

                    if let Some(exact) = exact_name {
                        if name != exact {
                            continue;
                        }
                    }

                    // Read agent YAML and inject name for comparison
                    let mut local_value = read_agent_yaml(&path)?;
                    if let Some(obj) = local_value.as_object_mut() {
                        obj.insert("name".to_string(), serde_json::Value::String(name.clone()));
                    }
                    strip_agent_empty_fields(&mut local_value);
                    let local_normalized = normalize(&local_value, volatile);

                    let resource_id = format!("agents/{}", name);

                    // Find matching remote agent
                    let remote_agent = remote_agents
                        .iter()
                        .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(&name));

                    match remote_agent {
                        Some(remote) => {
                            let mut remote_cleaned = remote.clone();
                            strip_agent_empty_fields(&mut remote_cleaned);
                            let remote_normalized = normalize(&remote_cleaned, volatile);
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
                let local_yaml = agents_dir.join(format!("{}.yaml", remote_name));
                if !local_yaml.exists() {
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

/// Diff a local file against the remote server.
async fn diff_resource(
    client: &AzureSearchClient,
    kind: ResourceKind,
    name: &str,
    path: &std::path::Path,
    strip_fields: &[&str],
    all_diffs: &mut Vec<(String, hoist_diff::DiffResult)>,
    has_changes: &mut bool,
) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let local: serde_json::Value = serde_json::from_str(&content)?;
    diff_resource_value(
        client,
        kind,
        name,
        &local,
        strip_fields,
        all_diffs,
        has_changes,
    )
    .await
}

/// Diff a local JSON value against the remote server.
async fn diff_resource_value(
    client: &AzureSearchClient,
    kind: ResourceKind,
    name: &str,
    local: &serde_json::Value,
    strip_fields: &[&str],
    all_diffs: &mut Vec<(String, hoist_diff::DiffResult)>,
    has_changes: &mut bool,
) -> Result<()> {
    let local_normalized = normalize(local, strip_fields);
    let resource_id = format!("{}/{}", kind.directory_name(), name);

    match client.get(kind, name).await {
        Ok(remote) => {
            let remote_normalized = normalize(&remote, strip_fields);
            let diff_result = diff(&local_normalized, &remote_normalized, "name");

            if !diff_result.is_equal {
                *has_changes = true;
            }

            all_diffs.push((resource_id, diff_result));
        }
        Err(hoist_client::ClientError::NotFound { .. }) => {
            *has_changes = true;
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
    Ok(())
}

/// Cross-environment diff: fetch resources from two remote environments and diff them.
async fn run_cross_env_diff(
    flags: &ResourceTypeFlags,
    format: DiffFormat,
    exit_code: bool,
    left_env: &hoist_core::config::ResolvedEnvironment,
    right_env: &hoist_core::config::ResolvedEnvironment,
) -> Result<()> {
    let include_preview = left_env.sync.include_preview || right_env.sync.include_preview;
    let selection = resolve_resource_selection_from_flags(flags, include_preview, true);

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    let kinds = selection.kinds();
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

    let mut all_diffs = Vec::new();
    let mut has_changes = false;

    // --- Search resources (cross-env) ---
    if let (Some(left_svc), Some(right_svc)) = (
        left_env.primary_search_service(),
        right_env.primary_search_service(),
    ) {
        if !search_kinds.is_empty() {
            let left_client = AzureSearchClient::from_service_config(left_svc)?;
            let right_client = AzureSearchClient::from_service_config(right_svc)?;

            for kind in &search_kinds {
                let volatile = get_volatile_fields(*kind);
                let read_only = get_read_only_fields(*kind);
                let strip_fields: Vec<&str> =
                    volatile.iter().chain(read_only.iter()).copied().collect();

                let exact_name = selection.name_filter(*kind);

                let left_resources = left_client.list(*kind).await?;
                let right_resources = right_client.list(*kind).await?;

                // Build name maps
                let left_map: std::collections::HashMap<String, serde_json::Value> = left_resources
                    .into_iter()
                    .filter_map(|r| {
                        let name = r.get("name")?.as_str()?.to_string();
                        Some((name, r))
                    })
                    .collect();
                let right_map: std::collections::HashMap<String, serde_json::Value> =
                    right_resources
                        .into_iter()
                        .filter_map(|r| {
                            let name = r.get("name")?.as_str()?.to_string();
                            Some((name, r))
                        })
                        .collect();

                // All unique names
                let mut all_names: Vec<String> = left_map.keys().cloned().collect();
                for name in right_map.keys() {
                    if !all_names.contains(name) {
                        all_names.push(name.clone());
                    }
                }
                all_names.sort();

                for name in &all_names {
                    if let Some(exact) = exact_name {
                        if name != exact {
                            continue;
                        }
                    }

                    let resource_id = format!("{}/{}", kind.directory_name(), name);

                    match (left_map.get(name), right_map.get(name)) {
                        (Some(left), Some(right)) => {
                            let left_norm = normalize(left, &strip_fields);
                            let right_norm = normalize(right, &strip_fields);
                            let diff_result = diff(&left_norm, &right_norm, "name");
                            if !diff_result.is_equal {
                                has_changes = true;
                            }
                            all_diffs.push((resource_id, diff_result));
                        }
                        (Some(left), None) => {
                            has_changes = true;
                            let left_norm = normalize(left, &strip_fields);
                            all_diffs.push((
                                format!("{} ({} only)", resource_id, left_env.name),
                                hoist_diff::DiffResult {
                                    is_equal: false,
                                    changes: vec![hoist_diff::Change {
                                        path: ".".to_string(),
                                        kind: hoist_diff::ChangeKind::Added,
                                        old_value: None,
                                        new_value: Some(left_norm),
                                    }],
                                },
                            ));
                        }
                        (None, Some(right)) => {
                            has_changes = true;
                            let right_norm = normalize(right, &strip_fields);
                            all_diffs.push((
                                format!("{} ({} only)", resource_id, right_env.name),
                                hoist_diff::DiffResult {
                                    is_equal: false,
                                    changes: vec![hoist_diff::Change {
                                        path: ".".to_string(),
                                        kind: hoist_diff::ChangeKind::Added,
                                        old_value: None,
                                        new_value: Some(right_norm),
                                    }],
                                },
                            ));
                        }
                        (None, None) => {}
                    }
                }
            }
        }
    }

    // --- Foundry agents (cross-env) ---
    if !foundry_kinds.is_empty() {
        let volatile = agent_volatile_fields();
        let exact_name = selection.name_filter(ResourceKind::Agent);

        let left_agents = if left_env.has_foundry() {
            let mut agents = Vec::new();
            for fc in &left_env.foundry {
                let client = hoist_client::FoundryClient::new(fc)?;
                agents.extend(client.list_agents().await?);
            }
            agents
        } else {
            vec![]
        };

        let right_agents = if right_env.has_foundry() {
            let mut agents = Vec::new();
            for fc in &right_env.foundry {
                let client = hoist_client::FoundryClient::new(fc)?;
                agents.extend(client.list_agents().await?);
            }
            agents
        } else {
            vec![]
        };

        let left_map: std::collections::HashMap<String, serde_json::Value> = left_agents
            .into_iter()
            .filter_map(|mut a| {
                strip_agent_empty_fields(&mut a);
                let name = a.get("name")?.as_str()?.to_string();
                Some((name, a))
            })
            .collect();
        let right_map: std::collections::HashMap<String, serde_json::Value> = right_agents
            .into_iter()
            .filter_map(|mut a| {
                strip_agent_empty_fields(&mut a);
                let name = a.get("name")?.as_str()?.to_string();
                Some((name, a))
            })
            .collect();

        let mut all_names: Vec<String> = left_map.keys().cloned().collect();
        for name in right_map.keys() {
            if !all_names.contains(name) {
                all_names.push(name.clone());
            }
        }
        all_names.sort();

        for name in &all_names {
            if let Some(exact) = exact_name {
                if name != exact {
                    continue;
                }
            }

            let resource_id = format!("agents/{}", name);

            match (left_map.get(name), right_map.get(name)) {
                (Some(left), Some(right)) => {
                    let left_norm = normalize(left, volatile);
                    let right_norm = normalize(right, volatile);
                    let diff_result = diff(&left_norm, &right_norm, "name");
                    if !diff_result.is_equal {
                        has_changes = true;
                    }
                    all_diffs.push((resource_id, diff_result));
                }
                (Some(left), None) => {
                    has_changes = true;
                    let left_norm = normalize(left, volatile);
                    all_diffs.push((
                        format!("{} ({} only)", resource_id, left_env.name),
                        hoist_diff::DiffResult {
                            is_equal: false,
                            changes: vec![hoist_diff::Change {
                                path: ".".to_string(),
                                kind: hoist_diff::ChangeKind::Added,
                                old_value: None,
                                new_value: Some(left_norm),
                            }],
                        },
                    ));
                }
                (None, Some(right)) => {
                    has_changes = true;
                    let right_norm = normalize(right, volatile);
                    all_diffs.push((
                        format!("{} ({} only)", resource_id, right_env.name),
                        hoist_diff::DiffResult {
                            is_equal: false,
                            changes: vec![hoist_diff::Change {
                                path: ".".to_string(),
                                kind: hoist_diff::ChangeKind::Added,
                                old_value: None,
                                new_value: Some(right_norm),
                            }],
                        },
                    ));
                }
                (None, None) => {}
            }
        }
    }

    // Format output
    let left_label = &left_env.name;
    let right_label = &right_env.name;

    match format {
        DiffFormat::Text => {
            let (changed, unchanged): (Vec<_>, Vec<_>) =
                all_diffs.iter().partition(|(_, r)| !r.is_equal);

            if changed.is_empty() {
                println!(
                    "No differences between '{}' and '{}', all {} resource(s) match.",
                    left_label,
                    right_label,
                    unchanged.len()
                );
            } else {
                println!(
                    "{} resource(s) differ between '{}' and '{}':\n",
                    changed.len().to_string().yellow(),
                    left_label,
                    right_label,
                );
                for (name, result) in &changed {
                    println!(
                        "  {} {} ({} change{})",
                        "~".yellow(),
                        name,
                        result.changes.len(),
                        if result.changes.len() == 1 { "" } else { "s" }
                    );
                    for line in describe_changes(&result.changes, Some((left_label, right_label))) {
                        println!("{}", line);
                    }
                }
                println!();
                if !unchanged.is_empty() {
                    println!(
                        "  {} resource(s) match between environments",
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

    if exit_code && has_changes {
        std::process::exit(5);
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
