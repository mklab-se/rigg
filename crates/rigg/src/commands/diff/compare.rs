//! Low-level helpers for diffing a single resource against the remote server.

use anyhow::Result;

use rigg_client::AzureSearchClient;
use rigg_core::normalize::normalize;
use rigg_core::resources::ResourceKind;
use rigg_core::resources::managed::{self, ManagedMap};

use crate::commands::explain::format_for_ai;

use super::ResourceDiff;

/// Diff a local file against the remote server.
pub(super) async fn diff_resource(
    client: &AzureSearchClient,
    kind: ResourceKind,
    name: &str,
    path: &std::path::Path,
    strip_fields: &[&str],
    all_diffs: &mut Vec<ResourceDiff>,
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
pub(super) async fn diff_resource_value(
    client: &AzureSearchClient,
    kind: ResourceKind,
    name: &str,
    local: &serde_json::Value,
    strip_fields: &[&str],
    all_diffs: &mut Vec<ResourceDiff>,
    has_changes: &mut bool,
) -> Result<()> {
    let local_normalized = normalize(local, strip_fields);
    let resource_id = format!("{}/{}", kind.directory_name(), name);

    match client.get(kind, name).await {
        Ok(remote) => {
            let remote_normalized = normalize(&remote, strip_fields);
            let diff_result = rigg_diff::diff(&local_normalized, &remote_normalized, "name");

            let has_diff = !diff_result.is_equal;
            if has_diff {
                *has_changes = true;
            }

            all_diffs.push(ResourceDiff {
                local_content: if has_diff {
                    Some(format_for_ai(kind, &local_normalized))
                } else {
                    None
                },
                remote_content: if has_diff {
                    Some(format_for_ai(kind, &remote_normalized))
                } else {
                    None
                },
                kind,
                resource_name: name.to_string(),
                display_id: resource_id,
                result: diff_result,
            });
        }
        Err(rigg_client::ClientError::NotFound { .. }) => {
            *has_changes = true;
            all_diffs.push(ResourceDiff {
                local_content: Some(format_for_ai(kind, &local_normalized)),
                remote_content: None,
                kind,
                resource_name: name.to_string(),
                display_id: resource_id,
                result: rigg_diff::DiffResult {
                    is_equal: false,
                    changes: vec![rigg_diff::Change {
                        path: ".".to_string(),
                        kind: rigg_diff::ChangeKind::Added,
                        old_value: None,
                        new_value: Some(local_normalized),
                        description: None,
                    }],
                },
            });
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Build a managed map from local KS files on disk.
pub(super) fn build_local_managed_map(service_dir: &std::path::Path) -> ManagedMap {
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
