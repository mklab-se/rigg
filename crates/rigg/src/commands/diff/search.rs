//! Search resource diffing: compare local files against Azure Search.

use anyhow::Result;

use rigg_client::AzureSearchClient;
use rigg_core::config::SearchServiceConfig;
use rigg_core::resources::ResourceKind;
use rigg_core::resources::managed;

use crate::commands::common::ResourceSelection;
use crate::commands::common::{get_read_only_fields, get_volatile_fields};

use super::ResourceDiff;
use super::compare::{build_local_managed_map, diff_resource, diff_resource_value};

/// Diff all selected search resource types against the remote service.
pub(super) async fn diff_search_resources(
    search_kinds: &[ResourceKind],
    search_svc: &SearchServiceConfig,
    service_dir: &std::path::Path,
    selection: &ResourceSelection,
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    eprintln!(
        "Comparing local and remote resources on {}...",
        search_svc.name
    );
    let client = AzureSearchClient::from_service_config(search_svc)?;

    // Build managed map from local KS files
    let managed_map = build_local_managed_map(service_dir);

    let has_ks = search_kinds.contains(&ResourceKind::KnowledgeSource);

    for kind in search_kinds {
        // Strip both volatile and read-only fields — matches push behavior.
        // Read-only fields (knowledgeSources, createdResources, etc.) can't be
        // pushed, so showing them as diffs would be misleading.
        let volatile = get_volatile_fields(*kind);
        let read_only = get_read_only_fields(*kind);
        let strip_fields: Vec<&str> = volatile.iter().chain(read_only.iter()).copied().collect();

        let exact_name = selection.name_filter(*kind);

        if *kind == ResourceKind::KnowledgeSource {
            diff_knowledge_sources(
                &client,
                service_dir,
                &strip_fields,
                exact_name,
                has_ks,
                all_diffs,
                has_changes,
            )
            .await?;
            continue;
        }

        // For other resource types, read from standalone directories
        diff_standalone_resources(
            &client,
            *kind,
            service_dir,
            &strip_fields,
            exact_name,
            &managed_map,
            all_diffs,
            has_changes,
        )
        .await?;
    }

    Ok(())
}

/// Diff knowledge source resources and their managed sub-resources.
async fn diff_knowledge_sources(
    client: &AzureSearchClient,
    service_dir: &std::path::Path,
    strip_fields: &[&str],
    exact_name: Option<&str>,
    has_ks: bool,
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");
    if !ks_base.exists() {
        return Ok(());
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
            client,
            ResourceKind::KnowledgeSource,
            &ks_name,
            &ks_file,
            strip_fields,
            all_diffs,
            has_changes,
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
                    client,
                    sub_kind,
                    &sub_name,
                    &sub_def,
                    &sub_strip,
                    all_diffs,
                    has_changes,
                )
                .await?;
            }
        }
    }
    Ok(())
}

/// Diff standalone (non-KS) resources against the remote service.
#[allow(clippy::too_many_arguments)]
async fn diff_standalone_resources(
    client: &AzureSearchClient,
    kind: ResourceKind,
    service_dir: &std::path::Path,
    strip_fields: &[&str],
    exact_name: Option<&str>,
    managed_map: &rigg_core::resources::managed::ManagedMap,
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    let resource_dir = service_dir.join(kind.directory_name());
    if !resource_dir.exists() {
        // Still check for remote-only even if local dir doesn't exist
        return diff_remote_only(client, kind, service_dir, all_diffs).await;
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
        if managed::managing_ks(managed_map, kind, name).is_some() {
            continue;
        }

        // Filter by singular flag (exact name match)
        if let Some(exact) = exact_name {
            if name != exact {
                continue;
            }
        }

        diff_resource(
            client,
            kind,
            name,
            &path,
            strip_fields,
            all_diffs,
            has_changes,
        )
        .await?;
    }

    // Check for remote-only resources (will be kept, not deleted)
    diff_remote_only(client, kind, service_dir, all_diffs).await
}

/// Detect remote-only resources (present on server but not locally).
async fn diff_remote_only(
    client: &AzureSearchClient,
    kind: ResourceKind,
    service_dir: &std::path::Path,
    all_diffs: &mut Vec<ResourceDiff>,
) -> Result<()> {
    let remote_resources = client.list(kind).await?;
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
            if all_diffs.iter().any(|d| d.display_id == resource_id) {
                continue;
            }

            // Remote only - note it but don't mark as change (we don't auto-delete)
            all_diffs.push(ResourceDiff {
                kind,
                resource_name: name.to_string(),
                display_id: resource_id,
                result: rigg_diff::DiffResult {
                    is_equal: true, // Don't count as change for exit code
                    changes: vec![],
                },
                local_content: None,
                remote_content: None,
            });
        }
    }
    Ok(())
}
