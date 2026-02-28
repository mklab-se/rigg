//! Human-readable change descriptions for diff, pull, and push summaries
//!
//! This module provides a resource-aware description engine that converts
//! raw JSON diff changes into English sentences. It understands Azure AI Search
//! and Microsoft Foundry resource types and produces contextual descriptions.

mod dispatch;
mod helpers;
mod index_advanced;
mod long_text;
mod resource_fields;
mod resource_fields_extra;

use colored::Colorize;
use hoist_core::resources::ResourceKind;
use hoist_diff::{Change, ChangeKind};

use dispatch::build_description;
use helpers::colorize_description;
use long_text::{format_long_text_colored, is_long_text_change, long_text_subject};

/// Maximum number of changes shown per resource before truncating.
const MAX_CHANGES_SHOWN: usize = 25;

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
/// When `compact` is true, strips the resource type and name prefix from
/// descriptions since the caller already displays them in a header line.
pub fn describe_changes(
    changes: &[Change],
    kind: ResourceKind,
    resource_name: &str,
    labels: Option<(&str, &str)>,
    compact: bool,
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
            let desc = if compact {
                compact_description(&desc, kind, resource_name)
            } else {
                desc
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
// Compact description helper
// ---------------------------------------------------------------------------

/// Strip the resource type and name prefix from a description for compact display.
///
/// Descriptions like `"Index 'my-index' has field 'tags'..."` become
/// `"Has field 'tags'..."` when the resource identity is already shown
/// in a header line above.
fn compact_description(desc: &str, kind: ResourceKind, name: &str) -> String {
    let kind_name = kind.display_name();

    // Try stripping "Kind 'name' " prefix (most common pattern)
    let prefix = format!("{} '{}' ", kind_name, name);
    if let Some(rest) = desc.strip_prefix(&prefix) {
        return capitalize_first(rest);
    }

    // Try stripping "Kind 'name': " prefix (colon variant)
    let prefix_colon = format!("{} '{}':", kind_name, name);
    if let Some(rest) = desc.strip_prefix(&prefix_colon) {
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        return capitalize_first(rest);
    }

    desc.to_string()
}

fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) => format!("{}{}", c.to_uppercase(), chars.as_str()),
        None => String::new(),
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

    // === describe_changes tests ===

    #[test]
    fn test_describe_changes_empty() {
        let lines = describe_changes(&[], ResourceKind::Index, "test", None, false);
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
        let lines = describe_changes(&changes, ResourceKind::Index, "idx", None, false);
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
        let lines = describe_changes(&changes, ResourceKind::Index, "idx", None, false);
        assert_eq!(lines.len(), MAX_CHANGES_SHOWN + 1);
        let last = strip_ansi(lines.last().unwrap());
        assert!(last.contains("and 5 more change(s)"));
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

    // === compact_description tests ===

    #[test]
    fn test_compact_strips_resource_prefix() {
        let desc = "Index 'my-index' has field 'tags' locally that does not exist on the server";
        let result = compact_description(desc, ResourceKind::Index, "my-index");
        assert_eq!(
            result,
            "Has field 'tags' locally that does not exist on the server"
        );
    }

    #[test]
    fn test_compact_strips_agent_prefix() {
        let desc = "Agent 'bot' uses model 'gpt-4o' locally but uses 'gpt-4' on the server";
        let result = compact_description(desc, ResourceKind::Agent, "bot");
        assert_eq!(
            result,
            "Uses model 'gpt-4o' locally but uses 'gpt-4' on the server"
        );
    }

    #[test]
    fn test_compact_preserves_non_matching() {
        // Descriptions that don't start with the resource prefix pass through
        let desc = "Index field 'content' type changed to 'Edm.ComplexType' locally";
        let result = compact_description(desc, ResourceKind::Index, "my-index");
        assert_eq!(result, desc);
    }

    #[test]
    fn test_compact_describe_changes() {
        let changes = vec![change(
            "model",
            ChangeKind::Modified,
            Some(json!("gpt-4")),
            Some(json!("gpt-4o")),
        )];
        let full = describe_changes(
            &changes,
            ResourceKind::Agent,
            "bot",
            Some(("locally", "on the server")),
            false,
        );
        let compact = describe_changes(
            &changes,
            ResourceKind::Agent,
            "bot",
            Some(("locally", "on the server")),
            true,
        );
        // Full version should contain "Agent 'bot'"
        let full_text = strip_ansi(&full[0]);
        assert!(full_text.contains("Agent 'bot'"));
        // Compact version should not
        let compact_text = strip_ansi(&compact[0]);
        assert!(!compact_text.contains("Agent 'bot'"));
        assert!(compact_text.contains("model"));
    }
}
