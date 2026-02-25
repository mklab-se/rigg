//! Human-readable change descriptions for diff, pull, and push summaries
//!
//! This module provides a resource-aware description engine that converts
//! raw JSON diff changes into English sentences. It understands Azure AI Search
//! and Microsoft Foundry resource types and produces contextual descriptions.

use colored::Colorize;
use hoist_core::resources::ResourceKind;
use hoist_diff::{Change, ChangeKind, DiffLine, diff_text, format_value_preview, is_long_text};
use serde_json::Value;

/// Maximum number of changes shown per resource before truncating.
const MAX_CHANGES_SHOWN: usize = 25;

/// Maximum length for value previews before truncation (only for fallback cases).
const MAX_LONG_VALUE: usize = 1000;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Annotate changes with human-readable descriptions in-place.
///
/// Call this before serializing changes to JSON so that the `description` field
/// is populated for AI agents and structured output consumers.
pub fn annotate_changes(
    changes: &mut [Change],
    kind: ResourceKind,
    resource_name: &str,
    labels: Option<(&str, &str)>,
) {
    let (old_label, new_label) = labels.unwrap_or(("locally", "on the server"));
    for change in changes.iter_mut() {
        change.description = Some(build_description(
            change,
            kind,
            resource_name,
            old_label,
            new_label,
        ));
    }
}

/// Format a list of changes as indented, colored terminal lines.
///
/// `labels` is `(old_label, new_label)` for directional context.
pub fn describe_changes(
    changes: &[Change],
    kind: ResourceKind,
    resource_name: &str,
    labels: Option<(&str, &str)>,
) -> Vec<String> {
    if changes.is_empty() {
        return vec![];
    }

    let (old_label, new_label) = labels.unwrap_or(("locally", "on the server"));

    // Sort changes: removals first, then modifications, then additions
    let mut sorted: Vec<&Change> = changes.iter().collect();
    sorted.sort_by_key(|c| match c.kind {
        ChangeKind::Removed => 0,
        ChangeKind::Modified => 1,
        ChangeKind::Added => 2,
    });

    let mut lines = Vec::new();
    let shown = sorted.len().min(MAX_CHANGES_SHOWN);

    for change in &sorted[..shown] {
        if is_long_text_change(change, kind) {
            let subject = long_text_subject(&change.path, kind, resource_name);
            lines.extend(format_long_text_colored(
                change, &subject, old_label, new_label,
            ));
        } else {
            let desc = if let Some(d) = &change.description {
                d.clone()
            } else {
                build_description(change, kind, resource_name, old_label, new_label)
            };
            lines.push(format!(
                "      {}",
                colorize_description(&desc, change.kind)
            ));
        }
    }

    if sorted.len() > MAX_CHANGES_SHOWN {
        let remaining = sorted.len() - MAX_CHANGES_SHOWN;
        lines.push(format!(
            "      {} and {} more change(s)",
            "...".dimmed(),
            remaining
        ));
    }

    lines
}

/// Format a list of changes as plain text lines (no ANSI colors).
///
/// Used for MCP/JSON output where terminal colors aren't appropriate.
pub fn describe_changes_plain(
    changes: &[Change],
    kind: ResourceKind,
    resource_name: &str,
    labels: Option<(&str, &str)>,
) -> Vec<String> {
    if changes.is_empty() {
        return vec![];
    }

    let (old_label, new_label) = labels.unwrap_or(("locally", "on the server"));

    changes
        .iter()
        .map(|change| {
            if let Some(d) = &change.description {
                d.clone()
            } else {
                build_description(change, kind, resource_name, old_label, new_label)
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Core description engine
// ---------------------------------------------------------------------------

fn build_description(
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
    if is_long_text_property(path, kind) {
        let value_is_long = change
            .old_value
            .as_ref()
            .and_then(|v| v.as_str())
            .is_some_and(is_long_text)
            || change
                .new_value
                .as_ref()
                .and_then(|v| v.as_str())
                .is_some_and(is_long_text);
        if value_is_long {
            let subject = long_text_subject(path, kind, name);
            return Some(describe_long_text_diff(
                change, &subject, old_label, new_label,
            ));
        }
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
// Section D: Index fields
// ---------------------------------------------------------------------------

fn describe_index_field(
    change: &Change,
    _kind: ResourceKind,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let path = &change.path;

    // Extract field name from path like "fields[myField]" or "fields[myField].prop"
    let (field_name, sub_path) = parse_array_element_path(path, "fields");

    match (sub_path.as_deref(), change.kind) {
        // Whole field added/removed
        (None, ChangeKind::Added) => format!(
            "Index '{}' has field '{}' {} that does not exist {}",
            name, field_name, new_label, old_label
        ),
        (None, ChangeKind::Removed) => format!(
            "Index '{}' has field '{}' {} that does not exist {}",
            name, field_name, old_label, new_label
        ),

        // Field type (immutable)
        (Some("type"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Index field '{}' has type '{}' {} but type '{}' {} (immutable — requires drop and recreate)",
                field_name, new_v, new_label, old_v, old_label
            )
        }

        // Boolean field attributes
        (Some("searchable"), ChangeKind::Modified) => format!(
            "Index field '{}' is {} {} for searching (was {} {})",
            field_name,
            bool_enabled(&change.new_value),
            new_label,
            bool_enabled(&change.old_value),
            old_label
        ),
        (Some("filterable"), ChangeKind::Modified) => format!(
            "Index field '{}' is {} {} for filtering (was {} {})",
            field_name,
            bool_enabled(&change.new_value),
            new_label,
            bool_enabled(&change.old_value),
            old_label
        ),
        (Some("sortable"), ChangeKind::Modified) => format!(
            "Index field '{}' is {} {} for sorting (was {} {})",
            field_name,
            bool_enabled(&change.new_value),
            new_label,
            bool_enabled(&change.old_value),
            old_label
        ),
        (Some("facetable"), ChangeKind::Modified) => format!(
            "Index field '{}' is {} {} for faceting (was {} {})",
            field_name,
            bool_enabled(&change.new_value),
            new_label,
            bool_enabled(&change.old_value),
            old_label
        ),
        (Some("retrievable"), ChangeKind::Modified) => format!(
            "Index field '{}' is {} {} for retrieval (was {} {})",
            field_name,
            bool_enabled(&change.new_value),
            new_label,
            bool_enabled(&change.old_value),
            old_label
        ),

        // Analyzer properties
        (Some("analyzer"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Index field '{}' uses analyzer '{}' {} (was '{}' {})",
                field_name, new_v, new_label, old_v, old_label
            )
        }
        (Some("searchAnalyzer"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Index field '{}' uses search analyzer '{}' {} (was '{}' {})",
                field_name, new_v, new_label, old_v, old_label
            )
        }
        (Some("indexAnalyzer"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Index field '{}' uses index analyzer '{}' {} (was '{}' {})",
                field_name, new_v, new_label, old_v, old_label
            )
        }

        // Vector properties
        (Some("dimensions"), ChangeKind::Modified) => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!(
                "Index field '{}' vector dimensions changed to {} {} (was {} {})",
                field_name, new_v, new_label, old_v, old_label
            )
        }
        (Some("vectorSearchProfile"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Index field '{}' uses vector search profile '{}' {} (was '{}' {})",
                field_name, new_v, new_label, old_v, old_label
            )
        }

        // Synonym maps
        (Some("synonymMaps"), ChangeKind::Modified) => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!(
                "Index field '{}' synonym maps changed {}: {} (was {} {})",
                field_name, new_label, new_v, old_v, old_label
            )
        }

        // Nested fields (recursive)
        (Some(sub), _) if sub.starts_with("fields[") => {
            format!(
                "In nested field '{}': {}",
                field_name,
                build_fallback_short(change, sub, old_label, new_label)
            )
        }

        // Other field properties
        (Some(prop), _) => format!(
            "Index field '{}' property '{}' {}",
            field_name,
            prop,
            value_comparison(change, old_label, new_label)
        ),

        // Whole field modified
        (None, ChangeKind::Modified) => format!(
            "Index '{}' field '{}' differs between {} and {}",
            name, field_name, old_label, new_label
        ),
    }
}

// ---------------------------------------------------------------------------
// Section E: Skillset skills
// ---------------------------------------------------------------------------

fn describe_skillset_skill(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let (skill_name, sub_path) = parse_array_element_path(&change.path, "skills");

    match (sub_path.as_deref(), change.kind) {
        (None, ChangeKind::Added) => format!(
            "Skillset '{}' has skill '{}' {} that does not exist {}",
            name, skill_name, new_label, old_label
        ),
        (None, ChangeKind::Removed) => format!(
            "Skillset '{}' has skill '{}' {} that does not exist {}",
            name, skill_name, old_label, new_label
        ),
        (Some("@odata.type"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Skill '{}' in skillset '{}' has type '{}' {} (was '{}' {})",
                skill_name, name, new_v, new_label, old_v, old_label
            )
        }
        (Some("context"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Skill '{}' context changed to '{}' {} (was '{}' {})",
                skill_name, new_v, new_label, old_v, old_label
            )
        }
        (Some(sub), _) if sub.starts_with("inputs[") => {
            let (input_name, input_sub) = parse_array_element_path(sub, "inputs");
            match (input_sub.as_deref(), change.kind) {
                (None, ChangeKind::Added) => format!(
                    "Skill '{}' has a new input '{}' {}",
                    skill_name, input_name, new_label
                ),
                (None, ChangeKind::Removed) => format!(
                    "Skill '{}' input '{}' exists {} but not {}",
                    skill_name, input_name, old_label, new_label
                ),
                (Some("source"), ChangeKind::Modified) => {
                    let old_v = str_val(&change.old_value);
                    let new_v = str_val(&change.new_value);
                    format!(
                        "Skill '{}' input '{}' source changed to '{}' {} (was '{}' {})",
                        skill_name, input_name, new_v, new_label, old_v, old_label
                    )
                }
                _ => format!(
                    "Skill '{}' input '{}' {}",
                    skill_name,
                    input_name,
                    value_comparison(change, old_label, new_label)
                ),
            }
        }
        (Some(sub), _) if sub.starts_with("outputs[") => {
            let (output_name, _) = parse_array_element_path(sub, "outputs");
            match change.kind {
                ChangeKind::Added => format!(
                    "Skill '{}' has a new output '{}' {}",
                    skill_name, output_name, new_label
                ),
                ChangeKind::Removed => format!(
                    "Skill '{}' output '{}' exists {} but not {}",
                    skill_name, output_name, old_label, new_label
                ),
                _ => format!(
                    "Skill '{}' output '{}' {}",
                    skill_name,
                    output_name,
                    value_comparison(change, old_label, new_label)
                ),
            }
        }
        (Some(prop), _) => format!(
            "Skill '{}' property '{}' {}",
            skill_name,
            prop,
            value_comparison(change, old_label, new_label)
        ),
        (None, ChangeKind::Modified) => format!(
            "Skill '{}' in skillset '{}' differs between {} and {}",
            skill_name, name, old_label, new_label
        ),
    }
}

// ---------------------------------------------------------------------------
// Section F: Indexer field mappings
// ---------------------------------------------------------------------------

fn describe_field_mapping(change: &Change, name: &str, old_label: &str, new_label: &str) -> String {
    let path = &change.path;
    let is_output = path.starts_with("outputFieldMappings");
    let prefix = if is_output { "output " } else { "" };
    let array_name = if is_output {
        "outputFieldMappings"
    } else {
        "fieldMappings"
    };

    let (mapping_name, sub_path) = parse_array_element_path(path, array_name);

    match (sub_path.as_deref(), change.kind) {
        (None, ChangeKind::Added) => format!(
            "Indexer '{}' has a new {}field mapping from '{}' {}",
            name, prefix, mapping_name, new_label
        ),
        (None, ChangeKind::Removed) => format!(
            "Indexer '{}' {}field mapping from '{}' exists {} but not {}",
            name, prefix, mapping_name, old_label, new_label
        ),
        (Some("targetFieldName"), ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Indexer '{}' {}field mapping '{}' targets '{}' {} (was '{}' {})",
                name, prefix, mapping_name, new_v, new_label, old_v, old_label
            )
        }
        (Some("mappingFunction"), _) => format!(
            "Indexer '{}' {}field mapping '{}' mapping function changed {}",
            name, prefix, mapping_name, new_label
        ),
        _ => format!(
            "Indexer '{}' {}field mapping '{}' {}",
            name,
            prefix,
            mapping_name,
            value_comparison(change, old_label, new_label)
        ),
    }
}

// ---------------------------------------------------------------------------
// Section G: Indexer schedule
// ---------------------------------------------------------------------------

fn describe_schedule(change: &Change, name: &str, old_label: &str, new_label: &str) -> String {
    let path = &change.path;
    match (path.as_str(), change.kind) {
        ("schedule", ChangeKind::Added) => format!(
            "Indexer '{}' has a schedule {} but none {}",
            name, new_label, old_label
        ),
        ("schedule", ChangeKind::Removed) => format!(
            "Indexer '{}' has a schedule {} but none {}",
            name, old_label, new_label
        ),
        ("schedule.interval", ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Indexer '{}' runs every '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            )
        }
        _ => format!(
            "Indexer '{}' schedule {}",
            name,
            value_comparison(change, old_label, new_label)
        ),
    }
}

// ---------------------------------------------------------------------------
// Section H: Indexer parameters
// ---------------------------------------------------------------------------

fn describe_indexer_params(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let path = &change.path;
    if let Some(config_key) = path.strip_prefix("parameters.configuration.") {
        let old_v = val_preview(&change.old_value);
        let new_v = val_preview(&change.new_value);
        return format!(
            "Indexer '{}' configuration '{}' is {} {} (was {} {})",
            name, config_key, new_v, new_label, old_v, old_label
        );
    }
    match path.as_str() {
        "parameters.batchSize" => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!(
                "Indexer '{}' batch size is {} {} (was {} {})",
                name, new_v, new_label, old_v, old_label
            )
        }
        "parameters.maxFailedItems" => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!(
                "Indexer '{}' max failed items is {} {} (was {} {})",
                name, new_v, new_label, old_v, old_label
            )
        }
        "parameters.maxFailedItemsPerBatch" => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!(
                "Indexer '{}' max failed items per batch is {} {} (was {} {})",
                name, new_v, new_label, old_v, old_label
            )
        }
        _ => format!(
            "Indexer '{}' parameter '{}' {}",
            name,
            path.strip_prefix("parameters.").unwrap_or(path),
            value_comparison(change, old_label, new_label)
        ),
    }
}

// ---------------------------------------------------------------------------
// Section I: Data source container
// ---------------------------------------------------------------------------

fn describe_container(change: &Change, name: &str, old_label: &str, new_label: &str) -> String {
    let path = &change.path;
    match (path.as_str(), change.kind) {
        ("container.name", ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Data source '{}' container is '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            )
        }
        ("container.query", ChangeKind::Modified) => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Data source '{}' container query changed: {} has \"{}\" (was \"{}\" {})",
                name, new_label, new_v, old_v, old_label
            )
        }
        ("container.query", ChangeKind::Added) => format!(
            "Data source '{}' has a container query {} but not {}",
            name, new_label, old_label
        ),
        ("container.query", ChangeKind::Removed) => format!(
            "Data source '{}' has a container query {} but not {}",
            name, old_label, new_label
        ),
        _ => format!(
            "Data source '{}' container {}",
            name,
            value_comparison(change, old_label, new_label)
        ),
    }
}

// ---------------------------------------------------------------------------
// Section K: KB knowledge source references
// ---------------------------------------------------------------------------

fn describe_kb_ks_ref(change: &Change, name: &str, old_label: &str, new_label: &str) -> String {
    let (ks_name, _) = parse_array_element_path(&change.path, "knowledgeSources");
    match change.kind {
        ChangeKind::Added => format!(
            "Knowledge base '{}' {} references knowledge source '{}' that is not referenced {}",
            name, new_label, ks_name, old_label
        ),
        ChangeKind::Removed => format!(
            "Knowledge base '{}' {} references knowledge source '{}' that is not referenced {}",
            name, old_label, ks_name, new_label
        ),
        _ => format!(
            "Knowledge base '{}' knowledge source '{}' {}",
            name,
            ks_name,
            value_comparison(change, old_label, new_label)
        ),
    }
}

// ---------------------------------------------------------------------------
// Section L: KS blob parameters
// ---------------------------------------------------------------------------

fn describe_ks_blob_params(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let path = &change.path;
    match path.as_str() {
        "azureBlobParameters" => format!(
            "Knowledge source '{}' blob parameters differ between {} and {}",
            name, old_label, new_label
        ),
        "azureBlobParameters.containerName" => {
            let old_v = str_val(&change.old_value);
            let new_v = str_val(&change.new_value);
            format!(
                "Knowledge source '{}' blob container is '{}' {} (was '{}' {})",
                name, new_v, new_label, old_v, old_label
            )
        }
        _ => {
            let prop = path.strip_prefix("azureBlobParameters.").unwrap_or(path);
            format!(
                "Knowledge source '{}' blob parameter '{}' {}",
                name,
                prop,
                value_comparison(change, old_label, new_label)
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Section M: Agent tools
// ---------------------------------------------------------------------------

fn describe_agent_tools(change: &Change, name: &str, old_label: &str, new_label: &str) -> String {
    let path = &change.path;

    if path == "tools" {
        return format!(
            "Agent '{}' tool configuration differs between {} and {}",
            name, old_label, new_label
        );
    }

    // Parse tools[N] or tools[N].prop
    if let Some(rest) = path.strip_prefix("tools[") {
        if let Some(bracket_end) = rest.find(']') {
            let _index = &rest[..bracket_end];
            let sub_path = rest
                .get(bracket_end + 1..)
                .and_then(|s| s.strip_prefix('.'));

            let type_preview = change
                .new_value
                .as_ref()
                .or(change.old_value.as_ref())
                .and_then(|v| v.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            return match (sub_path, change.kind) {
                (None, ChangeKind::Added) => format!(
                    "Agent '{}' has an additional tool {}: {}",
                    name, new_label, type_preview
                ),
                (None, ChangeKind::Removed) => format!(
                    "Agent '{}' has a tool {} that is not present {}: {}",
                    name, old_label, new_label, type_preview
                ),
                (Some("type"), ChangeKind::Modified) => {
                    let old_v = str_val(&change.old_value);
                    let new_v = str_val(&change.new_value);
                    format!(
                        "Agent '{}' tool type changed to '{}' {} (was '{}' {})",
                        name, new_v, new_label, old_v, old_label
                    )
                }
                (Some("server_label"), ChangeKind::Modified) => {
                    let old_v = str_val(&change.old_value);
                    let new_v = str_val(&change.new_value);
                    format!(
                        "Agent '{}' MCP tool server changed to '{}' {} (was '{}' {})",
                        name, new_v, new_label, old_v, old_label
                    )
                }
                _ => format!(
                    "Agent '{}' tool {}",
                    name,
                    value_comparison(change, old_label, new_label)
                ),
            };
        }
    }

    format!(
        "Agent '{}' tools {}",
        name,
        value_comparison(change, old_label, new_label)
    )
}

// ---------------------------------------------------------------------------
// Section N: Alias indexes
// ---------------------------------------------------------------------------

fn describe_alias_index(change: &Change, name: &str, old_label: &str, new_label: &str) -> String {
    match change.kind {
        ChangeKind::Added => {
            let new_v = str_val(&change.new_value);
            format!(
                "Alias '{}' points to index '{}' {} which is not referenced {}",
                name, new_v, new_label, old_label
            )
        }
        ChangeKind::Removed => {
            let old_v = str_val(&change.old_value);
            format!(
                "Alias '{}' points to index '{}' {} which is not referenced {}",
                name, old_v, old_label, new_label
            )
        }
        _ => format!(
            "Alias '{}' index reference {}",
            name,
            value_comparison(change, old_label, new_label)
        ),
    }
}

// ---------------------------------------------------------------------------
// Section O: Index advanced config
// ---------------------------------------------------------------------------

fn describe_index_advanced(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> Option<String> {
    let path = &change.path;

    // Scoring profiles
    if path.starts_with("scoringProfiles[") {
        let (profile_name, sub_path) = parse_array_element_path(path, "scoringProfiles");
        return Some(match (sub_path.as_deref(), change.kind) {
            (None, ChangeKind::Added) => format!(
                "Index '{}' has scoring profile '{}' {} that does not exist {}",
                name, profile_name, new_label, old_label
            ),
            (None, ChangeKind::Removed) => format!(
                "Index '{}' has scoring profile '{}' {} that does not exist {}",
                name, profile_name, old_label, new_label
            ),
            _ => format!(
                "Scoring profile '{}' in index '{}': {}",
                profile_name,
                name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // Custom analyzers, tokenizers, token filters, char filters
    for (prefix, label) in [
        ("analyzers", "custom analyzer"),
        ("tokenizers", "custom tokenizer"),
        ("tokenFilters", "custom token filter"),
        ("charFilters", "custom char filter"),
    ] {
        if path.starts_with(&format!("{}[", prefix)) {
            let (item_name, _) = parse_array_element_path(path, prefix);
            return Some(match change.kind {
                ChangeKind::Added => format!(
                    "Index '{}' has {} '{}' {} that does not exist {}",
                    name, label, item_name, new_label, old_label
                ),
                ChangeKind::Removed => format!(
                    "Index '{}' has {} '{}' {} that does not exist {}",
                    name, label, item_name, old_label, new_label
                ),
                _ => format!(
                    "Index '{}' {} '{}' {}",
                    name,
                    label,
                    item_name,
                    value_comparison(change, old_label, new_label)
                ),
            });
        }
    }

    // Vector search
    if path == "vectorSearch" {
        return Some(format!(
            "Index '{}' vector search configuration differs between {} and {}",
            name, old_label, new_label
        ));
    }
    if path.starts_with("vectorSearch.profiles[") {
        let (profile_name, _) = parse_array_element_path(
            path.strip_prefix("vectorSearch.").unwrap_or(path),
            "profiles",
        );
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has vector search profile '{}' {} that does not exist {}",
                name, profile_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has vector search profile '{}' {} that does not exist {}",
                name, profile_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' vector search profile '{}' {}",
                name,
                profile_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }
    if path.starts_with("vectorSearch.algorithms[") {
        let (alg_name, _) = parse_array_element_path(
            path.strip_prefix("vectorSearch.").unwrap_or(path),
            "algorithms",
        );
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has vector search algorithm '{}' {} that does not exist {}",
                name, alg_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has vector search algorithm '{}' {} that does not exist {}",
                name, alg_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' vector search algorithm '{}' {}",
                name,
                alg_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // Semantic search
    if path == "semantic" {
        return Some(format!(
            "Index '{}' semantic search configuration differs between {} and {}",
            name, old_label, new_label
        ));
    }
    if path.starts_with("semantic.configurations[") {
        let (config_name, _) = parse_array_element_path(
            path.strip_prefix("semantic.").unwrap_or(path),
            "configurations",
        );
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has semantic configuration '{}' {} that does not exist {}",
                name, config_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has semantic configuration '{}' {} that does not exist {}",
                name, config_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' semantic configuration '{}' {}",
                name,
                config_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // Suggesters
    if path.starts_with("suggesters[") {
        let (sug_name, _) = parse_array_element_path(path, "suggesters");
        return Some(match change.kind {
            ChangeKind::Added => format!(
                "Index '{}' has suggester '{}' {} that does not exist {}",
                name, sug_name, new_label, old_label
            ),
            ChangeKind::Removed => format!(
                "Index '{}' has suggester '{}' {} that does not exist {}",
                name, sug_name, old_label, new_label
            ),
            _ => format!(
                "Index '{}' suggester '{}' {}",
                name,
                sug_name,
                value_comparison(change, old_label, new_label)
            ),
        });
    }

    // CORS, similarity
    if path == "corsOptions" {
        return Some(format!(
            "Index '{}' CORS configuration differs between {} and {}",
            name, old_label, new_label
        ));
    }
    if path == "similarity" {
        return Some(format!(
            "Index '{}' similarity algorithm changed between {} and {}",
            name, old_label, new_label
        ));
    }

    None
}

// ---------------------------------------------------------------------------
// Long text descriptions
// ---------------------------------------------------------------------------

/// Properties that benefit from line-level diffing when their values are long.
fn is_long_text_property(path: &str, kind: ResourceKind) -> bool {
    matches!(
        (path, kind),
        ("instructions", ResourceKind::Agent)
            | ("synonyms", ResourceKind::SynonymMap)
            | ("retrievalInstructions", ResourceKind::KnowledgeBase)
            | ("answerInstructions", ResourceKind::KnowledgeBase)
            | ("description", _)
    )
}

/// Check if a change involves a long text property with a long actual value.
fn is_long_text_change(change: &Change, kind: ResourceKind) -> bool {
    if !is_long_text_property(&change.path, kind) {
        return false;
    }
    change
        .old_value
        .as_ref()
        .and_then(|v| v.as_str())
        .is_some_and(is_long_text)
        || change
            .new_value
            .as_ref()
            .and_then(|v| v.as_str())
            .is_some_and(is_long_text)
}

/// Build a human-readable subject phrase for a long text property.
fn long_text_subject(path: &str, kind: ResourceKind, name: &str) -> String {
    let kind_name = kind.display_name();
    match (path, kind) {
        ("instructions", ResourceKind::Agent) => format!("instructions for agent '{}'", name),
        ("synonyms", ResourceKind::SynonymMap) => format!("synonym rules for '{}'", name),
        ("retrievalInstructions", ResourceKind::KnowledgeBase) => {
            format!("retrieval instructions for knowledge base '{}'", name)
        }
        ("answerInstructions", ResourceKind::KnowledgeBase) => {
            format!("answer instructions for knowledge base '{}'", name)
        }
        ("description", _) => format!("description of {} '{}'", kind_name, name),
        _ => format!("'{}' of {} '{}'", path, kind_name, name),
    }
}

/// Whether the subject noun is singular (affects verb conjugation).
fn is_singular_subject(path: &str) -> bool {
    path == "description"
}

/// Indent each line of a multi-line string.
fn indent_lines(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|l| format!("{}{}", prefix, l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produce a line-level diff description for modified long text, or full text
/// for added/removed. Used for plain-text output (MCP/JSON, annotations).
fn describe_long_text_diff(
    change: &Change,
    subject: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let verb_differ = if is_singular_subject(&change.path) {
        "differs"
    } else {
        "differ"
    };
    let verb_exist = if is_singular_subject(&change.path) {
        "exists"
    } else {
        "exist"
    };

    match change.kind {
        ChangeKind::Modified => {
            let old_v = full_str_val(&change.old_value);
            let new_v = full_str_val(&change.new_value);
            let result = diff_text(&old_v, &new_v);
            let mut parts = vec![format!(
                "The {} {} ({} removed, {} added):",
                subject, verb_differ, result.deletions, result.insertions
            )];
            for (i, hunk) in result.hunks.iter().enumerate() {
                if i > 0 {
                    parts.push("  ...".to_string());
                }
                for line in &hunk.lines {
                    let text = match line {
                        DiffLine::Equal(s) => {
                            format!("    {}", s.trim_end_matches(['\n', '\r']))
                        }
                        DiffLine::Delete(s) => {
                            format!("  - {}", s.trim_end_matches(['\n', '\r']))
                        }
                        DiffLine::Insert(s) => {
                            format!("  + {}", s.trim_end_matches(['\n', '\r']))
                        }
                    };
                    parts.push(text);
                }
            }
            parts.join("\n")
        }
        ChangeKind::Added => {
            let new_v = full_str_val(&change.new_value);
            format!(
                "The {} {} {} but not {}:\n{}",
                subject,
                verb_exist,
                new_label,
                old_label,
                indent_lines(&new_v, "    ")
            )
        }
        ChangeKind::Removed => {
            let old_v = full_str_val(&change.old_value);
            format!(
                "The {} {} {} but not {}:\n{}",
                subject,
                verb_exist,
                old_label,
                new_label,
                indent_lines(&old_v, "    ")
            )
        }
    }
}

/// Format a long text change with per-line terminal colors.
///
/// For Modified: line-level diff with red/green/dimmed context.
/// For Added/Removed: indented text body in green/red.
fn format_long_text_colored(
    change: &Change,
    subject: &str,
    old_label: &str,
    new_label: &str,
) -> Vec<String> {
    let verb_differ = if is_singular_subject(&change.path) {
        "differs"
    } else {
        "differ"
    };
    let verb_exist = if is_singular_subject(&change.path) {
        "exists"
    } else {
        "exist"
    };

    match change.kind {
        ChangeKind::Modified => {
            let old_v = full_str_val(&change.old_value);
            let new_v = full_str_val(&change.new_value);
            let result = diff_text(&old_v, &new_v);

            let mut lines = vec![format!(
                "      {}",
                format!(
                    "The {} {} ({} removed, {} added):",
                    subject, verb_differ, result.deletions, result.insertions
                )
                .yellow()
            )];

            for (i, hunk) in result.hunks.iter().enumerate() {
                if i > 0 {
                    lines.push(format!("        {}", "...".dimmed()));
                }
                for diff_line in &hunk.lines {
                    let text = match diff_line {
                        DiffLine::Equal(s) => {
                            format!(
                                "        {}",
                                format!("  {}", s.trim_end_matches(['\n', '\r'])).dimmed()
                            )
                        }
                        DiffLine::Delete(s) => {
                            format!(
                                "        {}",
                                format!("- {}", s.trim_end_matches(['\n', '\r'])).red()
                            )
                        }
                        DiffLine::Insert(s) => {
                            format!(
                                "        {}",
                                format!("+ {}", s.trim_end_matches(['\n', '\r'])).green()
                            )
                        }
                    };
                    lines.push(text);
                }
            }

            lines
        }
        ChangeKind::Added => {
            let new_v = full_str_val(&change.new_value);
            let mut lines = vec![format!(
                "      {}",
                format!(
                    "The {} {} {} but not {}:",
                    subject, verb_exist, new_label, old_label
                )
                .green()
            )];
            for text_line in new_v.lines() {
                lines.push(format!("        {}", text_line.green()));
            }
            lines
        }
        ChangeKind::Removed => {
            let old_v = full_str_val(&change.old_value);
            let mut lines = vec![format!(
                "      {}",
                format!(
                    "The {} {} {} but not {}:",
                    subject, verb_exist, old_label, new_label
                )
                .red()
            )];
            for text_line in old_v.lines() {
                lines.push(format!("        {}", text_line.red()));
            }
            lines
        }
    }
}

// ---------------------------------------------------------------------------
// Fallback descriptions
// ---------------------------------------------------------------------------

fn build_fallback(
    change: &Change,
    kind_name: &str,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
    let path = &change.path;
    match change.kind {
        ChangeKind::Added => {
            let new_v = val_preview(&change.new_value);
            format!(
                "{} '{}' has '{}' {} ({}) but not {}",
                kind_name, name, path, new_label, new_v, old_label
            )
        }
        ChangeKind::Removed => {
            let old_v = val_preview(&change.old_value);
            format!(
                "{} '{}' has '{}' {} ({}) but not {}",
                kind_name, name, path, old_label, old_v, new_label
            )
        }
        ChangeKind::Modified => {
            if is_complex(&change.old_value) || is_complex(&change.new_value) {
                format!(
                    "The '{}' property of {} '{}' differs between {} and {}",
                    path, kind_name, name, old_label, new_label
                )
            } else {
                let old_v = val_preview(&change.old_value);
                let new_v = val_preview(&change.new_value);
                format!(
                    "The '{}' property of {} '{}' is {} {} (was {} {})",
                    path, kind_name, name, new_v, new_label, old_v, old_label
                )
            }
        }
    }
}

fn build_fallback_short(change: &Change, path: &str, old_label: &str, new_label: &str) -> String {
    match change.kind {
        ChangeKind::Added => format!("'{}' added {}", path, new_label),
        ChangeKind::Removed => format!("'{}' removed {}", path, new_label),
        ChangeKind::Modified => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!(
                "'{}' is {} {} (was {} {})",
                path, new_v, new_label, old_v, old_label
            )
        }
    }
}

// ---------------------------------------------------------------------------
// Value comparison helpers
// ---------------------------------------------------------------------------

fn value_comparison(change: &Change, old_label: &str, new_label: &str) -> String {
    match change.kind {
        ChangeKind::Added => {
            let new_v = val_preview(&change.new_value);
            format!("set to {} {} but not {}", new_v, new_label, old_label)
        }
        ChangeKind::Removed => {
            let old_v = val_preview(&change.old_value);
            format!("set to {} {} but not {}", old_v, old_label, new_label)
        }
        ChangeKind::Modified => {
            let old_v = val_preview(&change.old_value);
            let new_v = val_preview(&change.new_value);
            format!("is {} {} (was {} {})", new_v, new_label, old_v, old_label)
        }
    }
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

fn colorize_description(desc: &str, change_kind: ChangeKind) -> String {
    match change_kind {
        ChangeKind::Added => format!("{}", desc.green()),
        ChangeKind::Removed => format!("{}", desc.red()),
        ChangeKind::Modified => format!("{}", desc.yellow()),
    }
}

// ---------------------------------------------------------------------------
// Value extraction helpers
// ---------------------------------------------------------------------------

/// Extract string value, with reasonable truncation for long values.
fn str_val(value: &Option<Value>) -> String {
    match value {
        Some(Value::String(s)) => {
            if s.len() > MAX_LONG_VALUE {
                format!("{}... ({} chars)", &s[..MAX_LONG_VALUE], s.len())
            } else {
                s.clone()
            }
        }
        Some(v) => format_value_preview(Some(v)),
        None => "(none)".to_string(),
    }
}

/// Extract full string value without any truncation (for instructions, synonyms).
fn full_str_val(value: &Option<Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(v) => format_value_preview(Some(v)),
        None => "(none)".to_string(),
    }
}

/// Get a preview string for any value.
fn val_preview(value: &Option<Value>) -> String {
    format_value_preview(value.as_ref())
}

/// Check if a value is an object or array (complex).
fn is_complex(value: &Option<Value>) -> bool {
    matches!(value, Some(Value::Object(_)) | Some(Value::Array(_)))
}

/// Convert a boolean value to "enabled"/"disabled".
fn bool_enabled(value: &Option<Value>) -> &'static str {
    match value {
        Some(Value::Bool(true)) => "enabled",
        Some(Value::Bool(false)) => "disabled",
        _ => "unknown",
    }
}

/// Convert a boolean-like value to enabled/disabled text for Indexer disabled field.
fn bool_enabled_text(value: &Option<Value>) -> &'static str {
    match value {
        Some(Value::Bool(true)) => "disabled",
        Some(Value::Bool(false)) => "enabled",
        _ => "unknown",
    }
}

// ---------------------------------------------------------------------------
// Path parsing helpers
// ---------------------------------------------------------------------------

/// Parse a path like "fields[myField].subProp" into ("myField", Some("subProp")).
fn parse_array_element_path(path: &str, array_prefix: &str) -> (String, Option<String>) {
    let prefix = format!("{}[", array_prefix);
    let rest = path.strip_prefix(&prefix).unwrap_or(path);

    if let Some(bracket_end) = rest.find(']') {
        let element_name = rest[..bracket_end].to_string();
        let after_bracket = &rest[bracket_end + 1..];
        let sub_path = if after_bracket.is_empty() {
            None
        } else {
            Some(
                after_bracket
                    .strip_prefix('.')
                    .unwrap_or(after_bracket)
                    .to_string(),
            )
        };
        (element_name, sub_path)
    } else {
        (rest.to_string(), None)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Strip ANSI color codes from a string (for testing).
#[cfg(test)]
fn strip_ansi(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;
    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
        } else if in_escape {
            if c.is_ascii_alphabetic() {
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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

    // === annotate_changes tests ===

    #[test]
    fn test_annotate_changes_fills_descriptions() {
        let mut changes = vec![change(
            "description",
            ChangeKind::Modified,
            Some(json!("old")),
            Some(json!("new")),
        )];
        annotate_changes(
            &mut changes,
            ResourceKind::Index,
            "my-index",
            Some(("locally", "on the server")),
        );
        assert!(changes[0].description.is_some());
        let desc = changes[0].description.as_ref().unwrap();
        assert!(desc.contains("description"));
        assert!(desc.contains("my-index"));
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

    // === Index field tests ===

    #[test]
    fn test_index_field_added() {
        let c = change(
            "fields[newField]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "newField", "type": "Edm.String"})),
        );
        let desc = build_description(
            &c,
            ResourceKind::Index,
            "my-index",
            "locally",
            "on the server",
        );
        assert!(desc.contains("field 'newField'"));
        assert!(desc.contains("on the server"));
        assert!(desc.contains("does not exist"));
    }

    #[test]
    fn test_index_field_type_changed() {
        let c = change(
            "fields[content].type",
            ChangeKind::Modified,
            Some(json!("Edm.String")),
            Some(json!("Collection(Edm.String)")),
        );
        let desc = build_description(
            &c,
            ResourceKind::Index,
            "my-index",
            "locally",
            "on the server",
        );
        assert!(desc.contains("immutable"));
        assert!(desc.contains("drop and recreate"));
    }

    #[test]
    fn test_index_field_searchable_changed() {
        let c = change(
            "fields[title].searchable",
            ChangeKind::Modified,
            Some(json!(true)),
            Some(json!(false)),
        );
        let desc = build_description(&c, ResourceKind::Index, "idx", "locally", "on the server");
        assert!(desc.contains("enabled"));
        assert!(desc.contains("disabled"));
        assert!(desc.contains("searching"));
    }

    // === Skillset skill tests ===

    #[test]
    fn test_skillset_skill_added() {
        let c = change(
            "skills[split-skill]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "split-skill"})),
        );
        let desc = build_description(
            &c,
            ResourceKind::Skillset,
            "my-ss",
            "locally",
            "on the server",
        );
        assert!(desc.contains("skill 'split-skill'"));
        assert!(desc.contains("on the server"));
    }

    #[test]
    fn test_skill_input_source_changed() {
        let c = change(
            "skills[embed].inputs[text].source",
            ChangeKind::Modified,
            Some(json!("/document/content")),
            Some(json!("/document/merged_content")),
        );
        let desc = build_description(&c, ResourceKind::Skillset, "ss", "locally", "on the server");
        assert!(desc.contains("input 'text'"));
        assert!(desc.contains("source"));
    }

    // === describe_changes tests ===

    #[test]
    fn test_describe_changes_empty() {
        let lines = describe_changes(&[], ResourceKind::Index, "test", None);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_describe_changes_sorted_by_kind() {
        let changes = vec![
            change("added_field", ChangeKind::Added, None, Some(json!(1))),
            change("removed_field", ChangeKind::Removed, Some(json!(1)), None),
            change(
                "changed_field",
                ChangeKind::Modified,
                Some(json!(1)),
                Some(json!(2)),
            ),
        ];
        let lines = describe_changes(&changes, ResourceKind::Index, "idx", None);
        assert_eq!(lines.len(), 3);
        let texts: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        // Removals first
        assert!(texts[0].contains("removed_field"));
        // Then modifications
        assert!(texts[1].contains("changed_field"));
        // Then additions
        assert!(texts[2].contains("added_field"));
    }

    #[test]
    fn test_describe_changes_truncated_when_many() {
        let changes: Vec<Change> = (0..30)
            .map(|i| {
                change(
                    &format!("field{}", i),
                    ChangeKind::Modified,
                    Some(json!(i)),
                    Some(json!(i + 100)),
                )
            })
            .collect();
        let lines = describe_changes(&changes, ResourceKind::Index, "idx", None);
        assert_eq!(lines.len(), MAX_CHANGES_SHOWN + 1);
        let last = strip_ansi(lines.last().unwrap());
        assert!(last.contains("and 5 more change(s)"));
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

    // === Plain text output ===

    #[test]
    fn test_describe_changes_plain() {
        let changes = vec![change(
            "description",
            ChangeKind::Modified,
            Some(json!("old")),
            Some(json!("new")),
        )];
        let lines = describe_changes_plain(
            &changes,
            ResourceKind::Index,
            "my-index",
            Some(("locally", "on the server")),
        );
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("description"));
        assert!(!lines[0].contains("\x1b")); // no ANSI codes
    }

    // === Agent instructions (line-level diff) ===

    #[test]
    fn test_agent_instructions_line_diff() {
        let old = "You are a helpful assistant.\nAlways be polite.\nUse formal language.\n";
        let new =
            "You are a helpful assistant.\nAlways be extremely polite.\nUse formal language.\n";
        let c = change(
            "instructions",
            ChangeKind::Modified,
            Some(json!(old)),
            Some(json!(new)),
        );
        let desc = build_description(&c, ResourceKind::Agent, "bot", "locally", "on the server");
        assert!(desc.contains("differ (1 removed, 1 added)"));
        assert!(desc.contains("- Always be polite."));
        assert!(desc.contains("+ Always be extremely polite."));
    }

    #[test]
    fn test_agent_instructions_added() {
        let instructions = "You are a helpful assistant.\nAlways be polite.\n";
        let c = change(
            "instructions",
            ChangeKind::Added,
            None,
            Some(json!(instructions)),
        );
        let desc = build_description(&c, ResourceKind::Agent, "bot", "locally", "on the server");
        assert!(desc.contains("exist on the server but not locally"));
        assert!(desc.contains("You are a helpful assistant."));
    }

    #[test]
    fn test_short_instructions_fall_through() {
        let c = change(
            "instructions",
            ChangeKind::Modified,
            Some(json!("short old")),
            Some(json!("short new")),
        );
        let desc = build_description(&c, ResourceKind::Agent, "bot", "locally", "on the server");
        // Short values should use fallback, not line-level diff
        assert!(!desc.contains("removed"));
        assert!(!desc.contains("added)"));
    }

    #[test]
    fn test_kb_retrieval_instructions_diff() {
        let old = "Prioritize EU directives.\nMaximum 5 sources per response.\nInclude cross-references.\n";
        let new = "Prioritize EU directives.\nMaximum 10 sources per response.\nInclude cross-references.\n";
        let c = change(
            "retrievalInstructions",
            ChangeKind::Modified,
            Some(json!(old)),
            Some(json!(new)),
        );
        let desc = build_description(
            &c,
            ResourceKind::KnowledgeBase,
            "my-kb",
            "locally",
            "on the server",
        );
        assert!(desc.contains("retrieval instructions for knowledge base 'my-kb'"));
        assert!(desc.contains("- Maximum 5 sources per response."));
        assert!(desc.contains("+ Maximum 10 sources per response."));
    }

    #[test]
    fn test_long_description_uses_diff() {
        let old = "a\n".repeat(70);
        let mut new_text = old.clone();
        new_text.push_str("extra line\n");
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!(old)),
            Some(json!(new_text)),
        );
        let desc = build_description(
            &c,
            ResourceKind::Index,
            "my-index",
            "locally",
            "on the server",
        );
        assert!(desc.contains("description of Index 'my-index'"));
        assert!(desc.contains("0 removed, 1 added"));
    }

    #[test]
    fn test_short_description_uses_inline() {
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!("old desc")),
            Some(json!("new desc")),
        );
        let desc = build_description(
            &c,
            ResourceKind::Index,
            "my-index",
            "locally",
            "on the server",
        );
        assert!(desc.contains("old desc"));
        assert!(desc.contains("new desc"));
        assert!(!desc.contains("removed"));
    }

    #[test]
    fn test_describe_changes_long_text_multiline() {
        let old = "Line one\nLine two\nLine three\n";
        let new = "Line one\nLine TWO\nLine three\n";
        let changes = vec![change(
            "instructions",
            ChangeKind::Modified,
            Some(json!(old)),
            Some(json!(new)),
        )];
        let lines = describe_changes(
            &changes,
            ResourceKind::Agent,
            "bot",
            Some(("locally", "on the server")),
        );
        // Should produce multiple lines (header + diff lines)
        assert!(lines.len() > 1);
        let joined = lines.iter().map(|l| strip_ansi(l)).collect::<String>();
        assert!(joined.contains("differ"));
        assert!(joined.contains("- Line two"));
        assert!(joined.contains("+ Line TWO"));
    }

    // === Fallback for unknown properties ===

    #[test]
    fn test_fallback_modified_simple() {
        let c = change(
            "unknownProp",
            ChangeKind::Modified,
            Some(json!(42)),
            Some(json!(99)),
        );
        let desc = build_description(&c, ResourceKind::Index, "idx", "locally", "on the server");
        assert!(desc.contains("'unknownProp'"));
        assert!(desc.contains("42"));
        assert!(desc.contains("99"));
    }

    #[test]
    fn test_fallback_modified_complex() {
        let c = change(
            "unknownProp",
            ChangeKind::Modified,
            Some(json!({"a": 1})),
            Some(json!({"b": 2})),
        );
        let desc = build_description(&c, ResourceKind::Index, "idx", "locally", "on the server");
        assert!(desc.contains("'unknownProp'"));
        assert!(desc.contains("differs between"));
    }

    // === parse_array_element_path tests ===

    #[test]
    fn test_parse_simple() {
        let (name, sub) = parse_array_element_path("fields[myField]", "fields");
        assert_eq!(name, "myField");
        assert_eq!(sub, None);
    }

    #[test]
    fn test_parse_with_sub_path() {
        let (name, sub) = parse_array_element_path("fields[myField].type", "fields");
        assert_eq!(name, "myField");
        assert_eq!(sub, Some("type".to_string()));
    }

    #[test]
    fn test_parse_nested() {
        let (name, sub) = parse_array_element_path("skills[embed].inputs[text].source", "skills");
        assert_eq!(name, "embed");
        assert_eq!(sub, Some("inputs[text].source".to_string()));
    }
}
