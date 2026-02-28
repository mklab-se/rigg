//! Resource-specific field description functions for index fields and skillset skills.
//!
//! These handle the two most complex structured-field resource types with deeply
//! nested array elements (fields, skills, inputs, outputs).

use hoist_core::resources::ResourceKind;
use hoist_diff::{Change, ChangeKind};

use super::helpers::{
    bool_enabled, build_fallback_short, parse_array_element_path, str_val, val_preview,
    value_comparison,
};

// ---------------------------------------------------------------------------
// Section D: Index fields
// ---------------------------------------------------------------------------

pub(super) fn describe_index_field(
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

pub(super) fn describe_skillset_skill(
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn change(path: &str, kind: ChangeKind, old: Option<Value>, new: Option<Value>) -> Change {
        Change {
            path: path.to_string(),
            kind,
            old_value: old,
            new_value: new,
            description: None,
        }
    }

    #[test]
    fn test_index_field_added() {
        let c = change(
            "fields[newField]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "newField", "type": "Edm.String"})),
        );
        let desc = describe_index_field(
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
        let desc = describe_index_field(
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
        let desc = describe_index_field(&c, ResourceKind::Index, "idx", "locally", "on the server");
        assert!(desc.contains("enabled"));
        assert!(desc.contains("disabled"));
        assert!(desc.contains("searching"));
    }

    #[test]
    fn test_skillset_skill_added() {
        let c = change(
            "skills[split-skill]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "split-skill"})),
        );
        let desc = describe_skillset_skill(&c, "my-ss", "locally", "on the server");
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
        let desc = describe_skillset_skill(&c, "ss", "locally", "on the server");
        assert!(desc.contains("input 'text'"));
        assert!(desc.contains("source"));
    }
}
