//! Immutability constraint checking for Azure AI Search resources

use serde_json::Value;
use thiserror::Error;

use crate::resources::ResourceKind;

/// Classification of immutability violations
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViolationSeverity {
    /// Can be resolved by drop-and-recreate (e.g., field removal, type change)
    RequiresRecreate,
    /// Hard block — cannot be resolved automatically
    HardBlock,
}

/// Immutability violation error
#[derive(Debug, Error)]
#[error("{kind} '{name}': field '{field}' cannot be modified after creation. {suggestion}")]
pub struct ImmutabilityViolation {
    pub kind: ResourceKind,
    pub name: String,
    pub field: String,
    pub suggestion: String,
    pub severity: ViolationSeverity,
}

/// Check for immutability violations between existing and new resource definitions
pub fn check_immutability(
    kind: ResourceKind,
    name: &str,
    existing: &Value,
    new: &Value,
) -> Vec<ImmutabilityViolation> {
    let mut violations = Vec::new();

    if kind == ResourceKind::Index {
        check_index_immutability(name, existing, new, &mut violations);
    }

    violations
}

fn check_index_immutability(
    name: &str,
    existing: &Value,
    new: &Value,
    violations: &mut Vec<ImmutabilityViolation>,
) {
    // Check if any existing fields have been modified or removed
    if let (Some(existing_fields), Some(new_fields)) = (
        existing.get("fields").and_then(|v| v.as_array()),
        new.get("fields").and_then(|v| v.as_array()),
    ) {
        let existing_field_map: std::collections::HashMap<&str, &Value> = existing_fields
            .iter()
            .filter_map(|f| f.get("name").and_then(|n| n.as_str()).map(|n| (n, f)))
            .collect();

        let new_field_map: std::collections::HashMap<&str, &Value> = new_fields
            .iter()
            .filter_map(|f| f.get("name").and_then(|n| n.as_str()).map(|n| (n, f)))
            .collect();

        // Check for removed fields
        for field_name in existing_field_map.keys() {
            if !new_field_map.contains_key(field_name) {
                violations.push(ImmutabilityViolation {
                    kind: ResourceKind::Index,
                    name: name.to_string(),
                    field: format!("fields.{}", field_name),
                    suggestion: "Fields cannot be removed from an existing index. Drop and recreate to apply this change.".to_string(),
                    severity: ViolationSeverity::RequiresRecreate,
                });
            }
        }

        // Check for modified fields
        for (field_name, existing_field) in &existing_field_map {
            if let Some(new_field) = new_field_map.get(field_name) {
                let changes = find_field_changes(existing_field, new_field);
                for (attr, _old, _new) in changes {
                    // Allow some attribute changes (like analyzer)
                    if !is_field_attribute_mutable(&attr) {
                        violations.push(ImmutabilityViolation {
                            kind: ResourceKind::Index,
                            name: name.to_string(),
                            field: format!("fields.{}.{}", field_name, attr),
                            suggestion: format!(
                                "The '{}' attribute of field '{}' cannot be changed. Drop and recreate to apply this change.",
                                attr, field_name
                            ),
                            severity: ViolationSeverity::RequiresRecreate,
                        });
                    }
                }
            }
        }
    }
}

fn find_field_changes<'a>(
    existing: &'a Value,
    new: &'a Value,
) -> Vec<(String, &'a Value, &'a Value)> {
    let mut changes = Vec::new();

    if let (Some(existing_obj), Some(new_obj)) = (existing.as_object(), new.as_object()) {
        for (key, existing_val) in existing_obj {
            if let Some(new_val) = new_obj.get(key) {
                if existing_val != new_val {
                    changes.push((key.clone(), existing_val, new_val));
                }
            }
        }
    }

    changes
}

fn is_field_attribute_mutable(attr: &str) -> bool {
    // These attributes can be changed on existing fields
    matches!(
        attr,
        "searchAnalyzer" | "indexAnalyzer" | "synonymMaps" | "stored"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_no_violation_when_adding_field() {
        let existing = json!({
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true}
            ]
        });
        let new = json!({
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true},
                {"name": "title", "type": "Edm.String"}
            ]
        });

        let violations = check_immutability(ResourceKind::Index, "test", &existing, &new);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_violation_when_removing_field() {
        let existing = json!({
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true},
                {"name": "title", "type": "Edm.String"}
            ]
        });
        let new = json!({
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true}
            ]
        });

        let violations = check_immutability(ResourceKind::Index, "test", &existing, &new);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].field.contains("title"));
    }

    #[test]
    fn test_violation_when_changing_field_type() {
        let existing = json!({
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true}
            ]
        });
        let new = json!({
            "fields": [
                {"name": "id", "type": "Edm.Int32", "key": true}
            ]
        });

        let violations = check_immutability(ResourceKind::Index, "test", &existing, &new);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].field.contains("type"));
    }
}
