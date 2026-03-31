//! Diff output formatting

use crate::semantic::{Change, ChangeKind, DiffResult};
use serde_json::Value;

/// Format diff result as human-readable text
pub fn format_text(result: &DiffResult, resource_name: &str) -> String {
    if result.is_equal {
        return format!("{}: no changes\n", resource_name);
    }

    let mut output = String::new();
    output.push_str(&format!(
        "{}: {} change(s)\n",
        resource_name,
        result.changes.len()
    ));

    for change in &result.changes {
        output.push_str(&format_change_text(change));
    }

    output
}

fn format_change_text(change: &Change) -> String {
    // If a higher layer set a description, use it directly
    if let Some(desc) = &change.description {
        return format!("  {}\n", desc);
    }

    match change.kind {
        ChangeKind::Added => {
            let value_str = format_value_preview(change.new_value.as_ref());
            format!("  + {}: {}\n", change.path, value_str)
        }
        ChangeKind::Removed => {
            let value_str = format_value_preview(change.old_value.as_ref());
            format!("  - {}: {}\n", change.path, value_str)
        }
        ChangeKind::Modified => {
            let old_str = format_value_preview(change.old_value.as_ref());
            let new_str = format_value_preview(change.new_value.as_ref());
            format!("  ~ {}: was {}, now {}\n", change.path, old_str, new_str)
        }
    }
}

/// Create a human-readable preview of a JSON value.
///
/// Used by the base diff formatter. Higher-level formatters (describe.rs)
/// have their own value formatting with resource-aware context.
pub fn format_value_preview(value: Option<&Value>) -> String {
    match value {
        None => "(none)".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => {
            if s.len() > 500 {
                format!("\"{}...\" ({} chars)", &s[..497], s.len())
            } else {
                format!("\"{}\"", s)
            }
        }
        Some(Value::Array(arr)) => {
            if arr.is_empty() {
                "[]".to_string()
            } else if arr.len() <= 3 && arr.iter().all(is_simple_value) {
                // Show actual values for small arrays of simple items
                let items: Vec<String> =
                    arr.iter().map(|v| format_value_preview(Some(v))).collect();
                format!("[{}]", items.join(", "))
            } else {
                format!("[{} items]", arr.len())
            }
        }
        Some(Value::Object(obj)) => format!("{{...}} ({} keys)", obj.len()),
    }
}

/// Check if a value is a simple scalar (not array or object).
fn is_simple_value(value: &Value) -> bool {
    matches!(
        value,
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
    )
}

/// Format diff result as JSON
pub fn format_json(result: &DiffResult) -> String {
    serde_json::to_string_pretty(result).unwrap_or_else(|_| "{}".to_string())
}

/// Format a full diff report for multiple resources
pub fn format_report(diffs: &[(String, DiffResult)], format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => format_report_text(diffs),
        OutputFormat::Json => format_report_json(diffs),
    }
}

/// Output format options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

fn format_report_text(diffs: &[(String, DiffResult)]) -> String {
    let mut output = String::new();

    let (changed, unchanged): (Vec<_>, Vec<_>) = diffs.iter().partition(|(_, r)| !r.is_equal);

    if changed.is_empty() {
        output.push_str("No changes detected.\n");
        return output;
    }

    output.push_str(&format!(
        "Found {} resource(s) with changes:\n\n",
        changed.len()
    ));

    for (name, result) in &changed {
        output.push_str(&format_text(result, name));
        output.push('\n');
    }

    if !unchanged.is_empty() {
        output.push_str(&format!("{} resource(s) unchanged.\n", unchanged.len()));
    }

    output
}

fn format_report_json(diffs: &[(String, DiffResult)]) -> String {
    let report: Vec<_> = diffs
        .iter()
        .map(|(name, result)| {
            serde_json::json!({
                "resource": name,
                "changed": !result.is_equal,
                "changes": result.changes
            })
        })
        .collect();

    serde_json::to_string_pretty(&report).unwrap_or_else(|_| "[]".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::diff;
    use serde_json::json;

    #[test]
    fn test_format_text_no_changes() {
        let result = DiffResult {
            is_equal: true,
            changes: vec![],
        };

        let output = format_text(&result, "test-index");
        assert!(output.contains("no changes"));
    }

    #[test]
    fn test_format_text_with_changes() {
        let old = json!({"name": "test", "value": 1});
        let new = json!({"name": "test", "value": 2});

        let result = diff(&old, &new, "name");
        let output = format_text(&result, "test-index");

        assert!(output.contains("1 change"));
        assert!(output.contains("value"));
        assert!(output.contains("~")); // modified indicator
        assert!(output.contains("was"));
        assert!(output.contains("now"));
    }

    #[test]
    fn test_format_text_uses_description_when_set() {
        let result = DiffResult {
            is_equal: false,
            changes: vec![Change {
                path: "description".to_string(),
                kind: ChangeKind::Modified,
                old_value: Some(json!("old")),
                new_value: Some(json!("new")),
                description: Some(
                    "The description differs: locally has \"old\" while on the server has \"new\""
                        .to_string(),
                ),
            }],
        };

        let output = format_text(&result, "test-index");
        assert!(output.contains("The description differs"));
        assert!(!output.contains("~")); // Should not use generic format
    }

    #[test]
    fn test_format_json() {
        let result = DiffResult {
            is_equal: false,
            changes: vec![Change {
                path: "name".to_string(),
                kind: ChangeKind::Modified,
                old_value: Some(json!("old")),
                new_value: Some(json!("new")),
                description: None,
            }],
        };

        let output = format_json(&result);
        assert!(output.contains("modified"));
        assert!(output.contains("name"));
    }

    #[test]
    fn test_format_value_preview_long_string() {
        let long = "a".repeat(600);
        let preview = format_value_preview(Some(&json!(long)));
        assert!(preview.contains("..."));
        assert!(preview.contains("600 chars"));
    }

    #[test]
    fn test_format_value_preview_medium_string_not_truncated() {
        let medium = "a".repeat(400);
        let preview = format_value_preview(Some(&json!(medium)));
        assert!(!preview.contains("..."));
        assert_eq!(preview, format!("\"{}\"", medium));
    }

    #[test]
    fn test_format_value_preview_small_array() {
        let preview = format_value_preview(Some(&json!([1, 2, 3])));
        assert_eq!(preview, "[1, 2, 3]");
    }

    #[test]
    fn test_format_value_preview_small_string_array() {
        let preview = format_value_preview(Some(&json!(["a", "b"])));
        assert_eq!(preview, "[\"a\", \"b\"]");
    }

    #[test]
    fn test_format_value_preview_large_array() {
        let preview = format_value_preview(Some(&json!([1, 2, 3, 4])));
        assert_eq!(preview, "[4 items]");
    }

    #[test]
    fn test_format_value_preview_empty_array() {
        let preview = format_value_preview(Some(&json!([])));
        assert_eq!(preview, "[]");
    }

    #[test]
    fn test_format_value_preview_complex_array_items() {
        let preview = format_value_preview(Some(&json!([{"a": 1}])));
        assert_eq!(preview, "[1 items]");
    }

    #[test]
    fn test_modified_uses_english_phrasing() {
        let change = Change {
            path: "description".to_string(),
            kind: ChangeKind::Modified,
            old_value: Some(json!("old text")),
            new_value: Some(json!("new text")),
            description: None,
        };
        let output = format_change_text(&change);
        assert!(output.contains("was"));
        assert!(output.contains("now"));
        assert!(!output.contains("->"));
    }
}
