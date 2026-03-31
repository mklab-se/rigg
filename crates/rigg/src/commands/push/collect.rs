//! Resource collection and comparison for push operations.
//!
//! Reads local resources, compares them against Azure, and builds
//! the list of resources that need to be pushed (created, updated, or recreated).

use std::collections::HashMap;

use rigg_client::AzureSearchClient;
use rigg_core::constraints::ViolationSeverity;
use rigg_core::constraints::check_immutability;
use rigg_core::normalize::{format_json, normalize};
use rigg_core::resources::ResourceKind;
use rigg_core::resources::managed::{self, ManagedMap};
use rigg_core::state::Checksums;
use rigg_diff::Change;

use crate::commands::common::{get_read_only_fields, get_volatile_fields};

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

/// Collect a single resource for push, checking immutability, diffing,
/// and detecting remote conflicts (changes since last pull).
#[allow(clippy::too_many_arguments)]
pub(super) async fn collect_push_resource(
    client: &AzureSearchClient,
    kind: ResourceKind,
    name: &str,
    local: serde_json::Value,
    resources_to_push: &mut Vec<(ResourceKind, String, serde_json::Value, bool)>,
    validation_errors: &mut Vec<String>,
    recreate_candidates: &mut Vec<(ResourceKind, String)>,
    total_unchanged: &mut usize,
    change_details: &mut HashMap<(ResourceKind, String), Vec<Change>>,
    remote_values: &mut HashMap<(ResourceKind, String), serde_json::Value>,
    checksums: &Checksums,
    remote_conflicts: &mut Vec<(ResourceKind, String)>,
) {
    let remote = client.get(kind, name).await;

    match remote {
        Ok(existing) => {
            // Check for remote conflict: has remote changed since last pull?
            let pull_volatile = get_volatile_fields(kind);
            let remote_for_pull = normalize(&existing, &pull_volatile);
            let remote_pull_json = format_json(&remote_for_pull);
            let remote_checksum = Checksums::calculate(&remote_pull_json);
            if let Some(stored) = checksums.get(kind, name) {
                if *stored != remote_checksum {
                    remote_conflicts.push((kind, name.to_string()));
                }
            }

            let read_only_fields = get_read_only_fields(kind);
            let push_strip: Vec<&str> = pull_volatile
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
                        rigg_diff::diff(&normalized_existing, &normalized_local, "name");
                    change_details.insert((kind, name.to_string()), diff_result.changes);
                    remote_values.insert((kind, name.to_string()), normalized_existing);
                    resources_to_push.push((kind, name.to_string(), local, true));
                }
            }
        }
        Err(rigg_client::ClientError::NotFound { .. }) => {
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

/// Check if an error message indicates the known Azure KS recreation bug.
///
/// Azure sometimes tries to recreate managed sub-resources (index, indexer, data source,
/// skillset) when updating or creating a knowledge source. This fails if they already exist.
/// Common patterns:
/// - "Cannot create Knowledge Source '...' because the following Azure Search resources already exist"
/// - 409/Conflict when managed sub-resources exist
pub(super) fn is_ks_recreation_bug(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();
    // Match the explicit Azure error about KS managed sub-resources
    if lower.contains("cannot create knowledge source") && lower.contains("already exist") {
        return true;
    }
    // Match general conflict patterns involving managed sub-resources
    (lower.contains("already exist") || lower.contains("conflict") || lower.contains("409"))
        && (lower.contains("index")
            || lower.contains("indexer")
            || lower.contains("datasource")
            || lower.contains("data source")
            || lower.contains("skillset"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verifies that push comparison strips read-only fields from both sides
    /// so they don't produce false diffs (e.g. createdResources in KS).
    #[test]
    fn test_push_comparison_strips_read_only_from_both_sides() {
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

    // --- Conflict detection tests ---

    /// Verifies that checksum comparison detects remote changes since last pull.
    #[test]
    fn test_conflict_detection_remote_changed() {
        // Simulate a pull that stored a checksum
        let pulled_resource = json!({
            "name": "my-index",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}]
        });
        let volatile = get_volatile_fields(ResourceKind::Index);
        let normalized = normalize(&pulled_resource, &volatile);
        let pull_json = format_json(&normalized);
        let pull_checksum = Checksums::calculate(&pull_json);

        let mut checksums = Checksums::default();
        checksums.set(ResourceKind::Index, "my-index", pull_checksum);

        // Remote has changed (new field added)
        let remote_resource = json!({
            "name": "my-index",
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true},
                {"name": "title", "type": "Edm.String"}
            ]
        });
        let remote_normalized = normalize(&remote_resource, &volatile);
        let remote_json = format_json(&remote_normalized);
        let remote_checksum = Checksums::calculate(&remote_json);

        // Stored checksum != remote checksum → conflict
        let stored = checksums.get(ResourceKind::Index, "my-index").unwrap();
        assert_ne!(
            *stored, remote_checksum,
            "Should detect remote change as conflict"
        );
    }

    /// Verifies that unchanged remote resource matches stored checksum (no conflict).
    #[test]
    fn test_no_conflict_when_remote_unchanged() {
        let resource = json!({
            "name": "my-index",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}]
        });
        let volatile = get_volatile_fields(ResourceKind::Index);
        let normalized = normalize(&resource, &volatile);
        let json = format_json(&normalized);
        let checksum = Checksums::calculate(&json);

        let mut checksums = Checksums::default();
        checksums.set(ResourceKind::Index, "my-index", checksum.clone());

        // Remote returns same resource (with volatile fields)
        let remote_with_volatile = json!({
            "name": "my-index",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}],
            "@odata.etag": "W/\"new-etag\"",
            "@odata.context": "https://svc.search.windows.net/$metadata"
        });
        let remote_normalized = normalize(&remote_with_volatile, &volatile);
        let remote_json = format_json(&remote_normalized);
        let remote_checksum = Checksums::calculate(&remote_json);

        let stored = checksums.get(ResourceKind::Index, "my-index").unwrap();
        assert_eq!(
            *stored, remote_checksum,
            "Volatile fields should not cause false conflict"
        );
    }

    /// Verifies that conflict detection works for resources not yet tracked (no stored checksum).
    #[test]
    fn test_no_conflict_when_no_stored_checksum() {
        let checksums = Checksums::default();
        // No stored checksum means we haven't pulled this resource yet —
        // no conflict to detect (it's a new resource from our perspective)
        assert!(
            checksums
                .get(ResourceKind::Index, "unknown-index")
                .is_none()
        );
    }
}
