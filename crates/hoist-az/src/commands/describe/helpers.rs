//! Fallback builders, value extraction, path parsing, and color helpers.

use colored::Colorize;
use hoist_diff::{Change, ChangeKind, format_value_preview};
use serde_json::Value;

/// Maximum length for value previews before truncation (only for fallback cases).
pub(super) const MAX_LONG_VALUE: usize = 1000;

// ---------------------------------------------------------------------------
// Fallback descriptions
// ---------------------------------------------------------------------------

pub(super) fn build_fallback(
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

pub(super) fn build_fallback_short(
    change: &Change,
    path: &str,
    old_label: &str,
    new_label: &str,
) -> String {
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

pub(super) fn value_comparison(change: &Change, old_label: &str, new_label: &str) -> String {
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

pub(super) fn colorize_description(desc: &str, change_kind: ChangeKind) -> String {
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
pub(super) fn str_val(value: &Option<Value>) -> String {
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
pub(super) fn full_str_val(value: &Option<Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(v) => format_value_preview(Some(v)),
        None => "(none)".to_string(),
    }
}

/// Get a preview string for any value.
pub(super) fn val_preview(value: &Option<Value>) -> String {
    format_value_preview(value.as_ref())
}

/// Check if a value is an object or array (complex).
pub(super) fn is_complex(value: &Option<Value>) -> bool {
    matches!(value, Some(Value::Object(_)) | Some(Value::Array(_)))
}

/// Convert a boolean value to "enabled"/"disabled".
pub(super) fn bool_enabled(value: &Option<Value>) -> &'static str {
    match value {
        Some(Value::Bool(true)) => "enabled",
        Some(Value::Bool(false)) => "disabled",
        _ => "unknown",
    }
}

/// Convert a boolean-like value to enabled/disabled text for Indexer disabled field.
pub(super) fn bool_enabled_text(value: &Option<Value>) -> &'static str {
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
pub(super) fn parse_array_element_path(path: &str, array_prefix: &str) -> (String, Option<String>) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn test_fallback_modified_simple() {
        let c = change(
            "unknownProp",
            ChangeKind::Modified,
            Some(json!(42)),
            Some(json!(99)),
        );
        let desc = build_fallback(&c, "Index", "idx", "locally", "on the server");
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
        let desc = build_fallback(&c, "Index", "idx", "locally", "on the server");
        assert!(desc.contains("'unknownProp'"));
        assert!(desc.contains("differs between"));
    }

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
