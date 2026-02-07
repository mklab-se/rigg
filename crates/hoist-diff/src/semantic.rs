//! Semantic JSON diff algorithm for Azure AI Search resources

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Result of comparing two JSON values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffResult {
    /// Whether the values are identical
    pub is_equal: bool,
    /// List of changes
    pub changes: Vec<Change>,
}

/// A single change between two JSON values
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Change {
    /// JSON path to the changed value
    pub path: String,
    /// Type of change
    pub kind: ChangeKind,
    /// Old value (for modifications and deletions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_value: Option<Value>,
    /// New value (for modifications and additions)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub new_value: Option<Value>,
}

/// Type of change
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeKind {
    Added,
    Removed,
    Modified,
}

/// Compute semantic diff between two JSON values
///
/// This differs from a standard JSON diff by:
/// - Using key-based matching for arrays (by identity_key, typically "name")
/// - Producing human-readable paths
/// - Ignoring order for objects
pub fn diff(old: &Value, new: &Value, identity_key: &str) -> DiffResult {
    let mut changes = Vec::new();
    diff_values(old, new, "", identity_key, &mut changes);

    DiffResult {
        is_equal: changes.is_empty(),
        changes,
    }
}

fn diff_values(
    old: &Value,
    new: &Value,
    path: &str,
    identity_key: &str,
    changes: &mut Vec<Change>,
) {
    match (old, new) {
        (Value::Object(old_obj), Value::Object(new_obj)) => {
            diff_objects(old_obj, new_obj, path, identity_key, changes);
        }
        (Value::Array(old_arr), Value::Array(new_arr)) => {
            diff_arrays(old_arr, new_arr, path, identity_key, changes);
        }
        _ if old != new => {
            changes.push(Change {
                path: if path.is_empty() {
                    ".".to_string()
                } else {
                    path.to_string()
                },
                kind: ChangeKind::Modified,
                old_value: Some(old.clone()),
                new_value: Some(new.clone()),
            });
        }
        _ => {}
    }
}

fn diff_objects(
    old: &serde_json::Map<String, Value>,
    new: &serde_json::Map<String, Value>,
    path: &str,
    identity_key: &str,
    changes: &mut Vec<Change>,
) {
    let old_keys: HashSet<_> = old.keys().collect();
    let new_keys: HashSet<_> = new.keys().collect();

    // Removed keys
    for key in old_keys.difference(&new_keys) {
        let key_path = format_path(path, key);
        changes.push(Change {
            path: key_path,
            kind: ChangeKind::Removed,
            old_value: old.get(*key).cloned(),
            new_value: None,
        });
    }

    // Added keys
    for key in new_keys.difference(&old_keys) {
        let key_path = format_path(path, key);
        changes.push(Change {
            path: key_path,
            kind: ChangeKind::Added,
            old_value: None,
            new_value: new.get(*key).cloned(),
        });
    }

    // Modified keys
    for key in old_keys.intersection(&new_keys) {
        let old_val = old.get(*key).unwrap();
        let new_val = new.get(*key).unwrap();
        let key_path = format_path(path, key);
        diff_values(old_val, new_val, &key_path, identity_key, changes);
    }
}

fn diff_arrays(
    old: &[Value],
    new: &[Value],
    path: &str,
    identity_key: &str,
    changes: &mut Vec<Change>,
) {
    // Try semantic matching by identity key
    let old_has_keys = old.iter().all(|v| v.get(identity_key).is_some());
    let new_has_keys = new.iter().all(|v| v.get(identity_key).is_some());

    if old_has_keys && new_has_keys {
        diff_arrays_by_key(old, new, path, identity_key, changes);
    } else {
        diff_arrays_positional(old, new, path, identity_key, changes);
    }
}

fn diff_arrays_by_key(
    old: &[Value],
    new: &[Value],
    path: &str,
    identity_key: &str,
    changes: &mut Vec<Change>,
) {
    let old_map: BTreeMap<&str, &Value> = old
        .iter()
        .filter_map(|v| v.get(identity_key).and_then(|k| k.as_str()).map(|k| (k, v)))
        .collect();

    let new_map: BTreeMap<&str, &Value> = new
        .iter()
        .filter_map(|v| v.get(identity_key).and_then(|k| k.as_str()).map(|k| (k, v)))
        .collect();

    let old_keys: HashSet<_> = old_map.keys().cloned().collect();
    let new_keys: HashSet<_> = new_map.keys().cloned().collect();

    // Removed items
    for key in old_keys.difference(&new_keys) {
        let item_path = format!("{}[{}]", path, key);
        changes.push(Change {
            path: item_path,
            kind: ChangeKind::Removed,
            old_value: old_map.get(key).cloned().cloned(),
            new_value: None,
        });
    }

    // Added items
    for key in new_keys.difference(&old_keys) {
        let item_path = format!("{}[{}]", path, key);
        changes.push(Change {
            path: item_path,
            kind: ChangeKind::Added,
            old_value: None,
            new_value: new_map.get(key).cloned().cloned(),
        });
    }

    // Modified items
    for key in old_keys.intersection(&new_keys) {
        let old_val = old_map.get(key).unwrap();
        let new_val = new_map.get(key).unwrap();
        let item_path = format!("{}[{}]", path, key);
        diff_values(old_val, new_val, &item_path, identity_key, changes);
    }
}

fn diff_arrays_positional(
    old: &[Value],
    new: &[Value],
    path: &str,
    identity_key: &str,
    changes: &mut Vec<Change>,
) {
    let max_len = old.len().max(new.len());

    for i in 0..max_len {
        let item_path = format!("{}[{}]", path, i);

        match (old.get(i), new.get(i)) {
            (Some(old_val), Some(new_val)) => {
                diff_values(old_val, new_val, &item_path, identity_key, changes);
            }
            (Some(old_val), None) => {
                changes.push(Change {
                    path: item_path,
                    kind: ChangeKind::Removed,
                    old_value: Some(old_val.clone()),
                    new_value: None,
                });
            }
            (None, Some(new_val)) => {
                changes.push(Change {
                    path: item_path,
                    kind: ChangeKind::Added,
                    old_value: None,
                    new_value: Some(new_val.clone()),
                });
            }
            (None, None) => unreachable!(),
        }
    }
}

fn format_path(base: &str, key: &str) -> String {
    if base.is_empty() {
        key.to_string()
    } else {
        format!("{}.{}", base, key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_equal_values() {
        let old = json!({"name": "test", "value": 42});
        let new = json!({"name": "test", "value": 42});

        let result = diff(&old, &new, "name");
        assert!(result.is_equal);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_added_field() {
        let old = json!({"name": "test"});
        let new = json!({"name": "test", "value": 42});

        let result = diff(&old, &new, "name");
        assert!(!result.is_equal);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Added);
        assert_eq!(result.changes[0].path, "value");
    }

    #[test]
    fn test_removed_field() {
        let old = json!({"name": "test", "value": 42});
        let new = json!({"name": "test"});

        let result = diff(&old, &new, "name");
        assert!(!result.is_equal);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Removed);
        assert_eq!(result.changes[0].path, "value");
    }

    #[test]
    fn test_modified_field() {
        let old = json!({"name": "test", "value": 42});
        let new = json!({"name": "test", "value": 100});

        let result = diff(&old, &new, "name");
        assert!(!result.is_equal);
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].kind, ChangeKind::Modified);
        assert_eq!(result.changes[0].path, "value");
    }

    #[test]
    fn test_array_by_key() {
        let old = json!({
            "items": [
                {"name": "a", "value": 1},
                {"name": "b", "value": 2}
            ]
        });
        let new = json!({
            "items": [
                {"name": "b", "value": 2},
                {"name": "c", "value": 3}
            ]
        });

        let result = diff(&old, &new, "name");
        assert!(!result.is_equal);

        // Should detect: removed "a", added "c"
        let removed: Vec<_> = result
            .changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Removed)
            .collect();
        let added: Vec<_> = result
            .changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Added)
            .collect();

        assert_eq!(removed.len(), 1);
        assert!(removed[0].path.contains("[a]"));

        assert_eq!(added.len(), 1);
        assert!(added[0].path.contains("[c]"));
    }
}
