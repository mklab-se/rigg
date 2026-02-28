//! Cross-environment diff: compare resources between two remote environments.

use anyhow::Result;

use hoist_client::AzureSearchClient;
use hoist_core::normalize::normalize;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::agent::{agent_volatile_fields, strip_agent_empty_fields};
use hoist_core::service::ServiceDomain;

use crate::cli::{DiffFormat, ResourceTypeFlags};
use crate::commands::common::{
    get_read_only_fields, get_volatile_fields, resolve_resource_selection_from_flags,
};

use super::ResourceDiff;
use super::format::{format_diff_json, format_diff_text};

/// Compare resources across two remote environments.
pub(super) async fn run_cross_env_diff(
    flags: &ResourceTypeFlags,
    format: DiffFormat,
    exit_code: bool,
    left_env: &hoist_core::config::ResolvedEnvironment,
    right_env: &hoist_core::config::ResolvedEnvironment,
) -> Result<()> {
    eprintln!(
        "Comparing environments '{}' and '{}'...",
        left_env.name, right_env.name
    );
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

    let mut all_diffs: Vec<ResourceDiff> = Vec::new();
    let mut has_changes = false;

    // --- Search resources (cross-env) ---
    if let (Some(left_svc), Some(right_svc)) = (
        left_env.primary_search_service(),
        right_env.primary_search_service(),
    ) {
        if !search_kinds.is_empty() {
            diff_search_cross_env(
                &search_kinds,
                left_svc,
                right_svc,
                &selection,
                &mut all_diffs,
                &mut has_changes,
            )
            .await?;
        }
    }

    // --- Foundry agents (cross-env) ---
    if !foundry_kinds.is_empty() {
        diff_foundry_cross_env(
            left_env,
            right_env,
            &selection,
            &mut all_diffs,
            &mut has_changes,
        )
        .await?;
    }

    // Format output (cross-env diffs don't use AI explanations)
    let left_label = format!("on {}", left_env.name);
    let right_label = format!("on {}", right_env.name);
    let no_ai = std::collections::HashMap::new();

    match format {
        DiffFormat::Text => {
            format_diff_text(&all_diffs, Some((&left_label, &right_label)), &no_ai);
        }
        DiffFormat::Json => {
            let json = format_diff_json(&mut all_diffs, (&left_label, &right_label), &no_ai);
            print!("{}", json);
        }
    }

    if exit_code && has_changes {
        std::process::exit(5);
    }

    Ok(())
}

/// Diff search resources between two environments.
async fn diff_search_cross_env(
    search_kinds: &[ResourceKind],
    left_svc: &hoist_core::config::SearchServiceConfig,
    right_svc: &hoist_core::config::SearchServiceConfig,
    selection: &crate::commands::common::ResourceSelection,
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    let left_client = AzureSearchClient::from_service_config(left_svc)?;
    let right_client = AzureSearchClient::from_service_config(right_svc)?;

    for kind in search_kinds {
        let volatile = get_volatile_fields(*kind);
        let read_only = get_read_only_fields(*kind);
        let strip_fields: Vec<&str> = volatile.iter().chain(read_only.iter()).copied().collect();

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
        let right_map: std::collections::HashMap<String, serde_json::Value> = right_resources
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
                    let diff_result = hoist_diff::diff(&left_norm, &right_norm, "name");
                    if !diff_result.is_equal {
                        *has_changes = true;
                    }
                    all_diffs.push(ResourceDiff {
                        kind: *kind,
                        resource_name: name.clone(),
                        display_id: resource_id,
                        result: diff_result,
                        local_content: None,
                        remote_content: None,
                    });
                }
                (Some(left), None) => {
                    *has_changes = true;
                    let left_norm = normalize(left, &strip_fields);
                    all_diffs.push(ResourceDiff {
                        kind: *kind,
                        resource_name: name.clone(),
                        display_id: resource_id,
                        result: hoist_diff::DiffResult {
                            is_equal: false,
                            changes: vec![hoist_diff::Change {
                                path: ".".to_string(),
                                kind: hoist_diff::ChangeKind::Added,
                                old_value: None,
                                new_value: Some(left_norm),
                                description: None,
                            }],
                        },
                        local_content: None,
                        remote_content: None,
                    });
                }
                (None, Some(right)) => {
                    *has_changes = true;
                    let right_norm = normalize(right, &strip_fields);
                    all_diffs.push(ResourceDiff {
                        kind: *kind,
                        resource_name: name.clone(),
                        display_id: resource_id,
                        result: hoist_diff::DiffResult {
                            is_equal: false,
                            changes: vec![hoist_diff::Change {
                                path: ".".to_string(),
                                kind: hoist_diff::ChangeKind::Added,
                                old_value: None,
                                new_value: Some(right_norm),
                                description: None,
                            }],
                        },
                        local_content: None,
                        remote_content: None,
                    });
                }
                (None, None) => {}
            }
        }
    }

    Ok(())
}

/// Diff Foundry agents between two environments.
async fn diff_foundry_cross_env(
    left_env: &hoist_core::config::ResolvedEnvironment,
    right_env: &hoist_core::config::ResolvedEnvironment,
    selection: &crate::commands::common::ResourceSelection,
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
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
                let diff_result = hoist_diff::diff(&left_norm, &right_norm, "name");
                if !diff_result.is_equal {
                    *has_changes = true;
                }
                all_diffs.push(ResourceDiff {
                    kind: ResourceKind::Agent,
                    resource_name: name.clone(),
                    display_id: resource_id,
                    result: diff_result,
                    local_content: None,
                    remote_content: None,
                });
            }
            (Some(left), None) => {
                *has_changes = true;
                let left_norm = normalize(left, volatile);
                all_diffs.push(ResourceDiff {
                    kind: ResourceKind::Agent,
                    resource_name: name.clone(),
                    display_id: resource_id,
                    result: hoist_diff::DiffResult {
                        is_equal: false,
                        changes: vec![hoist_diff::Change {
                            path: ".".to_string(),
                            kind: hoist_diff::ChangeKind::Added,
                            old_value: None,
                            new_value: Some(left_norm),
                            description: None,
                        }],
                    },
                    local_content: None,
                    remote_content: None,
                });
            }
            (None, Some(right)) => {
                *has_changes = true;
                let right_norm = normalize(right, volatile);
                all_diffs.push(ResourceDiff {
                    kind: ResourceKind::Agent,
                    resource_name: name.clone(),
                    display_id: resource_id,
                    result: hoist_diff::DiffResult {
                        is_equal: false,
                        changes: vec![hoist_diff::Change {
                            path: ".".to_string(),
                            kind: hoist_diff::ChangeKind::Added,
                            old_value: None,
                            new_value: Some(right_norm),
                            description: None,
                        }],
                    },
                    local_content: None,
                    remote_content: None,
                });
            }
            (None, None) => {}
        }
    }

    Ok(())
}
