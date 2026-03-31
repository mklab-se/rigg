//! Core description dispatch engine.
//!
//! Given a `Change` and its `ResourceKind`, produces a human-readable English
//! description by trying resource-specific patterns first, then falling back
//! to a generic description.

use rigg_core::resources::ResourceKind;
use rigg_diff::{Change, ChangeKind};

use super::helpers::{bool_enabled_text, build_fallback, str_val, val_preview};
use super::index_advanced::describe_index_advanced;
use super::long_text::{describe_long_text_diff, is_long_text_change, long_text_subject};
use super::resource_fields::{describe_index_field, describe_skillset_skill};
use super::resource_fields_extra::{
    describe_agent_tools, describe_alias_index, describe_container, describe_field_mapping,
    describe_indexer_params, describe_kb_ks_ref, describe_ks_blob_params, describe_schedule,
};

pub(super) fn build_description(
    change: &Change,
    kind: ResourceKind,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let path = &change.path;
    let kind_name = kind.display_name();

    // A. Whole-resource presence (path = ".")
    if path == "." {
        return match change.kind {
            ChangeKind::Added => {
                format!("{} '{}' only exists {}", kind_name, name, new_label)
            }
            ChangeKind::Removed => {
                format!("{} '{}' only exists {}", kind_name, name, old_label)
            }
            ChangeKind::Modified => format!(
                "{} '{}' differs between {} and {}",
                kind_name, name, old_label, new_label
            ),
        };
    }

    // Try resource-specific patterns first
    if let Some(desc) = try_specific_pattern(change, kind, name, old_label, new_label) {
        return desc;
    }

    // Fallback: generic description
    build_fallback(change, kind_name, name, old_label, new_label)
}

/// Try resource-type-specific and path-pattern-specific descriptions.
fn try_specific_pattern(
    change: &Change,
    kind: ResourceKind,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> Option<String> {
    let path = &change.path;
    let kind_name = kind.display_name();

    // C. Long text properties — line-level diff for modified, full text for added/removed
    if is_long_text_change(change, kind) {
        let subject = long_text_subject(path, kind, name);
        return Some(describe_long_text_diff(
            change, &subject, old_label, new_label,
        ));
    }

    // B. Top-level simple properties
    match (path.as_str(), kind, change.kind) {
        // Description field (all resource types)
        ("description", _, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "The description of {} '{}' differs: {} has \"{}\" while {} has \"{}\"",
                kind_name, name, old_label, old_v, new_label, new_v
            ));
        }
        ("description", _, ChangeKind::Added) => {
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "{} '{}' has a description {} (\"{}\") but not {}",
                kind_name, name, new_label, new_v, old_label
            ));
        }
        ("description", _, ChangeKind::Removed) => {
            let old_v = str_val(&change.old_value);
            return Some(format!(
                "{} '{}' has a description {} (\"{}\") but not {}",
                kind_name, name, old_label, old_v, new_label
            ));
        }

        // Agent-specific
        ("model", ResourceKind::Agent, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Agent '{}' uses model '{}' {} but uses '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("kind", ResourceKind::Agent, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Agent '{}' has kind '{}' {} but kind '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("temperature", ResourceKind::Agent, ChangeKind::Modified) => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            return Some(format!(
                "Agent '{}' has temperature {} {} (was {} {})",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("top_p", ResourceKind::Agent, ChangeKind::Modified) => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            return Some(format!(
                "Agent '{}' has top_p {} {} (was {} {})",
                name, new_v, new_label, old_v, old_label
            ));
        }

        // Indexer references
        ("dataSourceName", ResourceKind::Indexer, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Indexer '{}' references data source '{}' {} but '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("targetIndexName", ResourceKind::Indexer, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Indexer '{}' targets index '{}' {} but '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("skillsetName", ResourceKind::Indexer, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Indexer '{}' references skillset '{}' {} but '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("skillsetName", ResourceKind::Indexer, ChangeKind::Added) => {
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Indexer '{}' references skillset '{}' {} but has no skillset {}",
                name, new_v, new_label, old_label
            ));
        }
        ("skillsetName", ResourceKind::Indexer, ChangeKind::Removed) => {
            let old_v = str_val(&change.old_value);
            return Some(format!(
                "Indexer '{}' references skillset '{}' {} but has no skillset {}",
                name, old_v, old_label, new_label
            ));
        }
        ("disabled", ResourceKind::Indexer, ChangeKind::Modified) => {
            let old_text = bool_enabled_text(&change.old_value);
            let new_text = bool_enabled_text(&change.new_value);
            return Some(format!(
                "Indexer '{}' is {} {} (was {} {})",
                name, new_text, new_label, old_text, old_label
            ));
        }

        // Index scoring
        ("defaultScoringProfile", ResourceKind::Index, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Index '{}' uses default scoring profile '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            ));
        }

        // SynonymMap format
        ("format", ResourceKind::SynonymMap, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Synonym map '{}' has format '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            ));
        }

        // DataSource type
        ("type", ResourceKind::DataSource, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Data source '{}' has type '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            ));
        }

        // Knowledge source properties
        ("indexName", ResourceKind::KnowledgeSource, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Knowledge source '{}' references index '{}' {} but '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("knowledgeBaseName", ResourceKind::KnowledgeSource, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Knowledge source '{}' belongs to knowledge base '{}' {} but '{}' {}",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("queryType", ResourceKind::KnowledgeSource, ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            return Some(format!(
                "Knowledge source '{}' uses query type '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            ));
        }
        ("top", ResourceKind::KnowledgeSource, ChangeKind::Modified) => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            return Some(format!(
                "Knowledge source '{}' returns top {} results {} (was {} {})",
                name, new_v, new_label, old_v, old_label
            ));
        }

        _ => {}
    }

    // D. Index fields
    if path.starts_with("fields[") {
        return Some(describe_index_field(
            change, kind, name, old_label, new_label,
        ));
    }

    // E. Skillset skills
    if path.starts_with("skills[") && kind == ResourceKind::Skillset {
        return Some(describe_skillset_skill(change, name, old_label, new_label));
    }

    // F. Indexer field mappings
    if (path.starts_with("fieldMappings[") || path.starts_with("outputFieldMappings["))
        && kind == ResourceKind::Indexer
    {
        return Some(describe_field_mapping(change, name, old_label, new_label));
    }

    // G. Indexer schedule
    if path.starts_with("schedule") && kind == ResourceKind::Indexer {
        return Some(describe_schedule(change, name, old_label, new_label));
    }

    // H. Indexer parameters
    if path.starts_with("parameters") && kind == ResourceKind::Indexer {
        return Some(describe_indexer_params(change, name, old_label, new_label));
    }

    // I. Data source container
    if path.starts_with("container") && kind == ResourceKind::DataSource {
        return Some(describe_container(change, name, old_label, new_label));
    }

    // J. Detection policies
    if (path == "dataChangeDetectionPolicy" || path == "dataDeletionDetectionPolicy")
        && kind == ResourceKind::DataSource
    {
        let policy_type = if path.contains("Change") {
            "change detection"
        } else {
            "deletion detection"
        };
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Data source '{}' has a {} policy {} but not {}",
                name, policy_type, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Data source '{}' has a {} policy {} but not {}",
                name, policy_type, old_label, new_label
            ),
            ChangeKind::Modified => format!(
                "Data source '{}' {} policy differs between {} and {}",
                name, policy_type, old_label, new_label
            ),
        });
    }

    // K. Knowledge base references
    if path.starts_with("knowledgeSources[") && kind == ResourceKind::KnowledgeBase {
        return Some(describe_kb_ks_ref(change, name, old_label, new_label));
    }
    if path == "storageContainer" && kind == ResourceKind::KnowledgeBase {
        let old_v = str_val(&change.old_value);
        let new_v = str_val(&change.new_value);
        return Some(format!(
            "Knowledge base '{}' storage container is '{}' {} (was '{}' {})",
            name, new_v, new_label, old_v, old_label
        ));
    }

    // L. Knowledge source parameters
    if path.starts_with("azureBlobParameters") && kind == ResourceKind::KnowledgeSource {
        return Some(describe_ks_blob_params(change, name, old_label, new_label));
    }
    if path == "selectFields" && kind == ResourceKind::KnowledgeSource {
        let old_v = val_preview(&change.old_value);
        let new_v = val_preview(&change.new_value);
        return Some(format!(
            "Knowledge source '{}' select fields changed: {} {} (was {} {})",
            name, new_v, new_label, old_v, old_label
        ));
    }
    if path == "semanticConfiguration" && kind == ResourceKind::KnowledgeSource {
        let old_v = str_val(&change.old_value);
        let new_v = str_val(&change.new_value);
        return Some(format!(
            "Knowledge source '{}' semantic configuration is '{}' {} (was '{}' {})",
            name, new_v, new_label, old_v, old_label
        ));
    }

    // M. Agent tools
    if path.starts_with("tools") && kind == ResourceKind::Agent {
        return Some(describe_agent_tools(change, name, old_label, new_label));
    }
    if path == "tool_resources" && kind == ResourceKind::Agent {
        return Some(format!(
            "Agent '{}' tool resources differ between {} and {}",
            name, old_label, new_label
        ));
    }
    if path == "metadata" && kind == ResourceKind::Agent {
        return Some(format!(
            "Agent '{}' metadata differs between {} and {}",
            name, old_label, new_label
        ));
    }

    // N. Alias indexes
    if path.starts_with("indexes[") && kind == ResourceKind::Alias {
        return Some(describe_alias_index(change, name, old_label, new_label));
    }

    // O. Index advanced config
    if kind == ResourceKind::Index {
        if let Some(desc) = describe_index_advanced(change, name, old_label, new_label) {
            return Some(desc);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    // Helper to create a Change
    fn change(path: &str, kind: ChangeKind, old: Option<Value>, new: Option<Value>) -> Change {
        Change {
            path: path.to_string(),
            kind,
            old_value: old,
            new_value: new,
            description: None,
        }
    }

    // === Whole-resource presence tests ===

    #[test]
    fn test_whole_resource_added() {
        let c = change(".", ChangeKind::Added, None, Some(json!({})));
        let desc = build_description(
            &c,
            ResourceKind::Index,
            "products",
            "locally",
            "on the server",
        );
        assert_eq!(desc, "Index 'products' only exists on the server");
    }

    #[test]
    fn test_whole_resource_removed() {
        let c = change(".", ChangeKind::Removed, Some(json!({})), None);
        let desc = build_description(
            &c,
            ResourceKind::Agent,
            "helper",
            "locally",
            "on the server",
        );
        assert_eq!(desc, "Agent 'helper' only exists locally");
    }

    // === Top-level property tests ===

    #[test]
    fn test_description_modified() {
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!("old text")),
            Some(json!("new text")),
        );
        let desc = build_description(
            &c,
            ResourceKind::Index,
            "my-index",
            "locally",
            "on the server",
        );
        assert!(desc.contains("description"));
        assert!(desc.contains("old text"));
        assert!(desc.contains("new text"));
        assert!(!desc.contains("\u{2192}")); // no arrow
    }

    #[test]
    fn test_agent_model_changed() {
        let c = change(
            "model",
            ChangeKind::Modified,
            Some(json!("gpt-4")),
            Some(json!("gpt-4o")),
        );
        let desc = build_description(
            &c,
            ResourceKind::Agent,
            "helper",
            "locally",
            "on the server",
        );
        assert!(desc.contains("gpt-4o"));
        assert!(desc.contains("gpt-4"));
    }

    #[test]
    fn test_indexer_disabled_changed() {
        let c = change(
            "disabled",
            ChangeKind::Modified,
            Some(json!(false)),
            Some(json!(true)),
        );
        let desc = build_description(
            &c,
            ResourceKind::Indexer,
            "my-ixer",
            "locally",
            "on the server",
        );
        assert!(desc.contains("disabled"));
        assert!(desc.contains("enabled"));
    }

    // === Cross-environment labels ===

    #[test]
    fn test_cross_env_labels() {
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!("old")),
            Some(json!("new")),
        );
        let desc = build_description(&c, ResourceKind::Index, "idx", "on prod", "on staging");
        assert!(desc.contains("on prod"));
        assert!(desc.contains("on staging"));
    }
}
