//! Human-readable change descriptions for pull and push summaries

use colored::Colorize;
use hoist_diff::{Change, ChangeKind};
use serde_json::Value;

/// Maximum number of changes shown per resource before truncating.
const MAX_CHANGES_SHOWN: usize = 8;

/// Maximum length for value previews in change descriptions.
const MAX_VALUE_LEN: usize = 60;

/// Format a list of changes as indented, colored terminal lines.
///
/// When `labels` is provided as `(old_label, new_label)`, change descriptions
/// include direction info (e.g. "added X (on server)" for diff). When `None`,
/// descriptions are direction-neutral (for pull/push where context is clear).
pub fn describe_changes(changes: &[Change], labels: Option<(&str, &str)>) -> Vec<String> {
    if changes.is_empty() {
        return vec![];
    }

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
        lines.push(format!("      {}", describe_change(change, labels)));
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

/// Format a single change as a human-readable colored string.
///
/// When `labels` is `Some((old_label, new_label))`, adds direction annotations.
fn describe_change(change: &Change, labels: Option<(&str, &str)>) -> String {
    match change.kind {
        ChangeKind::Added => format_added(change, labels),
        ChangeKind::Removed => format_removed(change, labels),
        ChangeKind::Modified => format_modified(change, labels),
    }
}

fn format_added(change: &Change, labels: Option<(&str, &str)>) -> String {
    let path = humanize_path(&change.path);
    let verb = "added".green();
    let direction = labels.map(|(_, new)| format!(" (on {})", new));
    match &change.new_value {
        Some(v) if is_simple_value(v) => {
            format!(
                "{} {}: {}{}",
                verb,
                path.bold(),
                preview_value(v).dimmed(),
                direction.unwrap_or_default().dimmed()
            )
        }
        _ => format!(
            "{} {}{}",
            verb,
            path.bold(),
            direction.unwrap_or_default().dimmed()
        ),
    }
}

fn format_removed(change: &Change, labels: Option<(&str, &str)>) -> String {
    let path = humanize_path(&change.path);
    let verb = "removed".red();
    let direction = labels.map(|(old, _)| format!(" (only in {})", old));
    format!(
        "{} {}{}",
        verb,
        path.bold(),
        direction.unwrap_or_default().dimmed()
    )
}

fn format_modified(change: &Change, labels: Option<(&str, &str)>) -> String {
    let path = humanize_path(&change.path);
    let verb = "changed".yellow();
    let old_preview = change
        .old_value
        .as_ref()
        .map(preview_value)
        .unwrap_or_default();
    let new_preview = change
        .new_value
        .as_ref()
        .map(preview_value)
        .unwrap_or_default();

    match labels {
        Some((old_label, new_label)) => format!(
            "{} {}: {} ({}) {} {} ({})",
            verb,
            path.bold(),
            old_preview.dimmed(),
            old_label,
            "\u{2192}".dimmed(), // → arrow
            new_preview,
            new_label
        ),
        None => format!(
            "{} {}: {} {} {}",
            verb,
            path.bold(),
            old_preview.dimmed(),
            "\u{2192}".dimmed(), // → arrow
            new_preview
        ),
    }
}

/// Make a JSON path more readable for humans.
///
/// Keeps the path structure but makes it cleaner:
/// - `fields[myField].type` stays as-is (already readable)
/// - Leading dots are stripped
fn humanize_path(path: &str) -> String {
    let p = path.strip_prefix('.').unwrap_or(path);
    if p.is_empty() {
        "(root)".to_string()
    } else {
        p.to_string()
    }
}

/// Check if a value is simple enough to preview inline.
fn is_simple_value(value: &Value) -> bool {
    matches!(
        value,
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null
    )
}

/// Create a truncated human-readable preview of a JSON value.
fn preview_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => {
            if s.len() > MAX_VALUE_LEN {
                format!("\"{}...\"", &s[..MAX_VALUE_LEN])
            } else {
                format!("\"{}\"", s)
            }
        }
        Value::Array(arr) => {
            if arr.is_empty() {
                "[]".to_string()
            } else {
                format!("[{} item(s)]", arr.len())
            }
        }
        Value::Object(obj) => {
            if obj.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{} field(s)}}", obj.len())
            }
        }
    }
}

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
        }
    }

    // === preview_value tests ===

    #[test]
    fn test_preview_null() {
        assert_eq!(preview_value(&Value::Null), "null");
    }

    #[test]
    fn test_preview_bool() {
        assert_eq!(preview_value(&json!(true)), "true");
        assert_eq!(preview_value(&json!(false)), "false");
    }

    #[test]
    fn test_preview_number() {
        assert_eq!(preview_value(&json!(42)), "42");
        assert_eq!(preview_value(&json!(3.14)), "3.14");
    }

    #[test]
    fn test_preview_short_string() {
        assert_eq!(preview_value(&json!("hello")), "\"hello\"");
    }

    #[test]
    fn test_preview_long_string_truncated() {
        let long = "a".repeat(100);
        let preview = preview_value(&json!(long));
        assert!(preview.len() < 100);
        assert!(preview.ends_with("...\""));
    }

    #[test]
    fn test_preview_empty_array() {
        assert_eq!(preview_value(&json!([])), "[]");
    }

    #[test]
    fn test_preview_array_with_items() {
        assert_eq!(preview_value(&json!([1, 2, 3])), "[3 item(s)]");
    }

    #[test]
    fn test_preview_empty_object() {
        assert_eq!(preview_value(&json!({})), "{}");
    }

    #[test]
    fn test_preview_object_with_fields() {
        assert_eq!(preview_value(&json!({"a": 1, "b": 2})), "{2 field(s)}");
    }

    // === humanize_path tests ===

    #[test]
    fn test_humanize_path_simple() {
        assert_eq!(humanize_path("description"), "description");
    }

    #[test]
    fn test_humanize_path_nested() {
        assert_eq!(humanize_path("fields[title].type"), "fields[title].type");
    }

    #[test]
    fn test_humanize_path_empty() {
        assert_eq!(humanize_path(""), "(root)");
    }

    #[test]
    fn test_humanize_path_dot_only() {
        assert_eq!(humanize_path("."), "(root)");
    }

    // === describe_change tests ===

    #[test]
    fn test_describe_added_simple_value() {
        let c = change("description", ChangeKind::Added, None, Some(json!("hello")));
        let result = strip_ansi(&describe_change(&c, None));
        assert!(result.contains("added"));
        assert!(result.contains("description"));
        assert!(result.contains("\"hello\""));
    }

    #[test]
    fn test_describe_added_complex_value() {
        let c = change(
            "fields[newField]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "newField", "type": "Edm.String"})),
        );
        let result = strip_ansi(&describe_change(&c, None));
        assert!(result.contains("added"));
        assert!(result.contains("fields[newField]"));
        // Complex values should NOT show inline preview
        assert!(!result.contains("Edm.String"));
    }

    #[test]
    fn test_describe_removed() {
        let c = change("oldField", ChangeKind::Removed, Some(json!("value")), None);
        let result = strip_ansi(&describe_change(&c, None));
        assert!(result.contains("removed"));
        assert!(result.contains("oldField"));
    }

    #[test]
    fn test_describe_modified_string() {
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!("old text")),
            Some(json!("new text")),
        );
        let result = strip_ansi(&describe_change(&c, None));
        assert!(result.contains("changed"));
        assert!(result.contains("description"));
        assert!(result.contains("\"old text\""));
        assert!(result.contains("\"new text\""));
        assert!(result.contains("\u{2192}")); // → arrow
    }

    #[test]
    fn test_describe_modified_number() {
        let c = change(
            "maxTokens",
            ChangeKind::Modified,
            Some(json!(100)),
            Some(json!(200)),
        );
        let result = strip_ansi(&describe_change(&c, None));
        assert!(result.contains("changed"));
        assert!(result.contains("100"));
        assert!(result.contains("200"));
    }

    #[test]
    fn test_describe_modified_bool() {
        let c = change(
            "fields[title].searchable",
            ChangeKind::Modified,
            Some(json!(true)),
            Some(json!(false)),
        );
        let result = strip_ansi(&describe_change(&c, None));
        assert!(result.contains("changed"));
        assert!(result.contains("fields[title].searchable"));
        assert!(result.contains("true"));
        assert!(result.contains("false"));
    }

    // === describe_changes tests ===

    #[test]
    fn test_describe_changes_empty() {
        let lines = describe_changes(&[], None);
        assert!(lines.is_empty());
    }

    #[test]
    fn test_describe_changes_single() {
        let changes = vec![change(
            "description",
            ChangeKind::Modified,
            Some(json!("old")),
            Some(json!("new")),
        )];
        let lines = describe_changes(&changes, None);
        assert_eq!(lines.len(), 1);
        let text = strip_ansi(&lines[0]);
        assert!(text.contains("changed"));
        assert!(text.contains("description"));
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
        let lines = describe_changes(&changes, None);
        assert_eq!(lines.len(), 3);
        let texts: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        // Removals first, then modifications, then additions
        assert!(texts[0].contains("removed"));
        assert!(texts[1].contains("changed"));
        assert!(texts[2].contains("added"));
    }

    #[test]
    fn test_describe_changes_truncated_when_many() {
        let changes: Vec<Change> = (0..12)
            .map(|i| {
                change(
                    &format!("field{}", i),
                    ChangeKind::Modified,
                    Some(json!(i)),
                    Some(json!(i + 100)),
                )
            })
            .collect();
        let lines = describe_changes(&changes, None);
        // Should show MAX_CHANGES_SHOWN + 1 (for the "... and N more" line)
        assert_eq!(lines.len(), MAX_CHANGES_SHOWN + 1);
        let last = strip_ansi(lines.last().unwrap());
        assert!(last.contains("and 4 more change(s)"));
    }

    #[test]
    fn test_describe_changes_exactly_at_limit() {
        let changes: Vec<Change> = (0..MAX_CHANGES_SHOWN)
            .map(|i| {
                change(
                    &format!("field{}", i),
                    ChangeKind::Modified,
                    Some(json!(i)),
                    Some(json!(i + 100)),
                )
            })
            .collect();
        let lines = describe_changes(&changes, None);
        // No truncation message when exactly at limit
        assert_eq!(lines.len(), MAX_CHANGES_SHOWN);
    }

    // === is_simple_value tests ===

    #[test]
    fn test_is_simple_value() {
        assert!(is_simple_value(&json!("text")));
        assert!(is_simple_value(&json!(42)));
        assert!(is_simple_value(&json!(true)));
        assert!(is_simple_value(&json!(null)));
        assert!(!is_simple_value(&json!([1, 2])));
        assert!(!is_simple_value(&json!({"a": 1})));
    }

    // === Integration-style tests with realistic Azure Search changes ===

    #[test]
    fn test_index_field_added() {
        let changes = vec![change(
            "fields[newVectorField]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "newVectorField", "type": "Collection(Edm.Single)"})),
        )];
        let lines = describe_changes(&changes, None);
        assert_eq!(lines.len(), 1);
        let text = strip_ansi(&lines[0]);
        assert!(text.contains("added"));
        assert!(text.contains("fields[newVectorField]"));
    }

    #[test]
    fn test_skillset_skill_removed() {
        let changes = vec![change(
            "skills[split-skill]",
            ChangeKind::Removed,
            Some(
                json!({"name": "split-skill", "@odata.type": "#Microsoft.Skills.Text.SplitSkill"}),
            ),
            None,
        )];
        let lines = describe_changes(&changes, None);
        assert_eq!(lines.len(), 1);
        let text = strip_ansi(&lines[0]);
        assert!(text.contains("removed"));
        assert!(text.contains("skills[split-skill]"));
    }

    #[test]
    fn test_description_changed() {
        let changes = vec![change(
            "description",
            ChangeKind::Modified,
            Some(json!("Process safety regulations for oil and gas")),
            Some(json!("Updated process safety regulations")),
        )];
        let lines = describe_changes(&changes, None);
        assert_eq!(lines.len(), 1);
        let text = strip_ansi(&lines[0]);
        assert!(text.contains("changed"));
        assert!(text.contains("description"));
        assert!(text.contains("Process safety"));
        assert!(text.contains("Updated process"));
    }

    #[test]
    fn test_field_attribute_changed() {
        let changes = vec![change(
            "fields[title].searchable",
            ChangeKind::Modified,
            Some(json!(true)),
            Some(json!(false)),
        )];
        let lines = describe_changes(&changes, None);
        let text = strip_ansi(&lines[0]);
        assert!(text.contains("changed"));
        assert!(text.contains("fields[title].searchable"));
        assert!(text.contains("true"));
        assert!(text.contains("false"));
    }

    // === Label tests (diff direction) ===

    #[test]
    fn test_labels_added_shows_direction() {
        let c = change(
            "knowledgeSources[ks-2]",
            ChangeKind::Added,
            None,
            Some(json!({"name": "ks-2"})),
        );
        let result = strip_ansi(&describe_change(&c, Some(("local", "server"))));
        assert!(result.contains("added"));
        assert!(result.contains("(on server)"));
    }

    #[test]
    fn test_labels_removed_shows_direction() {
        let c = change(
            "knowledgeSources[ks-old]",
            ChangeKind::Removed,
            Some(json!({"name": "ks-old"})),
            None,
        );
        let result = strip_ansi(&describe_change(&c, Some(("local", "server"))));
        assert!(result.contains("removed"));
        assert!(result.contains("(only in local)"));
    }

    #[test]
    fn test_labels_modified_shows_both_sides() {
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!("old text")),
            Some(json!("new text")),
        );
        let result = strip_ansi(&describe_change(&c, Some(("local", "server"))));
        assert!(result.contains("(local)"));
        assert!(result.contains("(server)"));
    }

    #[test]
    fn test_no_labels_omits_direction() {
        let c = change(
            "description",
            ChangeKind::Modified,
            Some(json!("old")),
            Some(json!("new")),
        );
        let result = strip_ansi(&describe_change(&c, None));
        assert!(!result.contains("(local)"));
        assert!(!result.contains("(server)"));
    }

    // === Integration-style tests ===

    #[test]
    fn test_multiple_mixed_changes() {
        let changes = vec![
            change(
                "description",
                ChangeKind::Modified,
                Some(json!("old desc")),
                Some(json!("new desc")),
            ),
            change(
                "fields[removed-field]",
                ChangeKind::Removed,
                Some(json!({"name": "removed-field", "type": "Edm.String"})),
                None,
            ),
            change(
                "fields[new-field]",
                ChangeKind::Added,
                None,
                Some(json!({"name": "new-field", "type": "Edm.Int32"})),
            ),
            change(
                "fields[existing].filterable",
                ChangeKind::Modified,
                Some(json!(false)),
                Some(json!(true)),
            ),
        ];
        let lines = describe_changes(&changes, None);
        assert_eq!(lines.len(), 4);
        let texts: Vec<String> = lines.iter().map(|l| strip_ansi(l)).collect();
        // Removals first
        assert!(texts[0].contains("removed"));
        // Then modifications
        assert!(texts[1].contains("changed"));
        assert!(texts[2].contains("changed"));
        // Then additions
        assert!(texts[3].contains("added"));
    }
}
