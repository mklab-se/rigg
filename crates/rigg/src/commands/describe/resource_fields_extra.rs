//! Additional resource-specific field description functions.
//!
//! Covers indexer (field mappings, schedule, parameters), data source (container),
//! knowledge base/source fields, agent tools, and alias indexes.

use rigg_diff::{Change, ChangeKind};

use super::helpers::{parse_array_element_path, str_val, val_preview, value_comparison};

// ---------------------------------------------------------------------------
// Section F: Indexer field mappings
// ---------------------------------------------------------------------------

pub(super) fn describe_field_mapping(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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

pub(super) fn describe_schedule(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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

pub(super) fn describe_indexer_params(
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

pub(super) fn describe_container(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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

pub(super) fn describe_kb_ks_ref(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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

pub(super) fn describe_ks_blob_params(
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

pub(super) fn describe_agent_tools(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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

pub(super) fn describe_alias_index(
    change: &Change,
    name: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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
