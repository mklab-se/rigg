//! Long text diffing and word-level highlighting.
//!
//! Handles line-level diff display for long text properties like agent instructions,
//! synonym maps, knowledge base instructions, and long descriptions.

use colored::Colorize;
use rigg_core::resources::ResourceKind;
use rigg_diff::{
    CONTEXT_TRUNCATE_LEN, Change, ChangeKind, DiffLine, WordSegment, WordSegmentKind, diff_text,
    is_long_text, normalize_for_diff, truncate_context,
};

use super::helpers::full_str_val;

// ---------------------------------------------------------------------------
// Long text property detection
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
pub(super) fn is_long_text_change(change: &Change, kind: ResourceKind) -> bool {
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

// ---------------------------------------------------------------------------
// Subject and verb helpers
// ---------------------------------------------------------------------------

/// Build a human-readable subject phrase for a long text property.
pub(super) fn long_text_subject(path: &str, kind: ResourceKind, name: &str) -> String {
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

// ---------------------------------------------------------------------------
// Plain text long text diff
// ---------------------------------------------------------------------------

/// Produce a line-level diff description for modified long text, or full text
/// for added/removed. Used for plain-text output (MCP/JSON, annotations).
pub(super) fn describe_long_text_diff(
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
            let old_v = normalize_for_diff(&full_str_val(&change.old_value));
            let new_v = normalize_for_diff(&full_str_val(&change.new_value));
            let result = diff_text(&old_v, &new_v);
            let total_changed = result.deletions + result.insertions;
            let line_word = if total_changed == 1 { "line" } else { "lines" };
            let mut parts = vec![format!(
                "The {} {} {} vs {} ({} {} changed):",
                subject, verb_differ, old_label, new_label, total_changed, line_word,
            )];
            for (i, hunk) in result.hunks.iter().enumerate() {
                if i > 0 {
                    parts.push("  ---".to_string());
                }
                let mut prev_was_change = false;
                for line in &hunk.lines {
                    match line {
                        DiffLine::Equal(s) => {
                            prev_was_change = false;
                            let trimmed = s.trim_end_matches(['\n', '\r']);
                            let display = truncate_context(trimmed, CONTEXT_TRUNCATE_LEN);
                            parts.push(format!("    {}", display));
                        }
                        DiffLine::Delete(s) => {
                            prev_was_change = true;
                            parts.push(format!("  - {}", s.trim_end_matches(['\n', '\r'])));
                        }
                        DiffLine::Insert(s) => {
                            prev_was_change = true;
                            parts.push(format!("  + {}", s.trim_end_matches(['\n', '\r'])));
                        }
                        DiffLine::Modified {
                            old_segments,
                            new_segments,
                        } => {
                            if prev_was_change {
                                parts.push(String::new());
                            }
                            prev_was_change = true;
                            parts.push(format!("  - {}", render_word_segments_plain(old_segments)));
                            parts.push(format!("  + {}", render_word_segments_plain(new_segments)));
                        }
                    }
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

// ---------------------------------------------------------------------------
// Colored terminal long text diff
// ---------------------------------------------------------------------------

/// Format a long text change with per-line terminal colors.
///
/// For Modified: line-level diff with red/green/dimmed context.
/// For Added/Removed: indented text body in green/red.
pub(super) fn format_long_text_colored(
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
            let old_v = normalize_for_diff(&full_str_val(&change.old_value));
            let new_v = normalize_for_diff(&full_str_val(&change.new_value));
            let result = diff_text(&old_v, &new_v);
            let total_changed = result.deletions + result.insertions;
            let line_word = if total_changed == 1 { "line" } else { "lines" };

            let mut lines = vec![format!(
                "      {}",
                format!(
                    "The {} {} {} vs {} ({} {} changed):",
                    subject, verb_differ, old_label, new_label, total_changed, line_word,
                )
                .yellow()
            )];
            lines.push(format!(
                "        {}",
                format!("- = {}  + = {}", old_label, new_label).dimmed()
            ));

            for (i, hunk) in result.hunks.iter().enumerate() {
                if i > 0 {
                    lines.push(format!("        {}", "---".dimmed()));
                }
                let mut prev_was_change = false;
                for diff_line in &hunk.lines {
                    match diff_line {
                        DiffLine::Equal(s) => {
                            prev_was_change = false;
                            let trimmed = s.trim_end_matches(['\n', '\r']);
                            let display = truncate_context(trimmed, CONTEXT_TRUNCATE_LEN);
                            lines.push(format!("        {}", format!("  {}", display).dimmed()));
                        }
                        DiffLine::Delete(s) => {
                            // No blank line before Delete — Deletes and following Inserts
                            // are part of the same logical change group.
                            prev_was_change = true;
                            lines.push(format!(
                                "        {}",
                                format!("- {}", s.trim_end_matches(['\n', '\r'])).red()
                            ));
                        }
                        DiffLine::Insert(s) => {
                            prev_was_change = true;
                            lines.push(format!(
                                "        {}",
                                format!("+ {}", s.trim_end_matches(['\n', '\r'])).green()
                            ));
                        }
                        DiffLine::Modified {
                            old_segments,
                            new_segments,
                        } => {
                            // Blank line before each Modified pair when following another
                            // change — visually separates side-by-side comparisons.
                            if prev_was_change {
                                lines.push(String::new());
                            }
                            prev_was_change = true;
                            lines.push(format!(
                                "        {}{}",
                                "- ".red(),
                                render_word_segments_colored(old_segments, true)
                            ));
                            lines.push(format!(
                                "        {}{}",
                                "+ ".green(),
                                render_word_segments_colored(new_segments, false)
                            ));
                        }
                    }
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
// Word segment rendering helpers
// ---------------------------------------------------------------------------

/// Render word segments as a colored string for terminal output.
///
/// - `is_old = true`: unchanged words are dimmed, changed words are red+bold
/// - `is_old = false`: unchanged words are dimmed, changed words are green+bold
fn render_word_segments_colored(segments: &[WordSegment], is_old: bool) -> String {
    use std::fmt::Write;
    let mut buf = String::new();
    for seg in segments {
        let text = seg.text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        match seg.kind {
            WordSegmentKind::Equal => {
                let _ = write!(buf, "{}", text.dimmed());
            }
            WordSegmentKind::Changed => {
                if is_old {
                    let _ = write!(buf, "{}", text.red().bold());
                } else {
                    let _ = write!(buf, "{}", text.green().bold());
                }
            }
        }
    }
    buf
}

/// Render word segments as plain text with [brackets] around changed words.
fn render_word_segments_plain(segments: &[WordSegment]) -> String {
    let mut buf = String::new();
    for seg in segments {
        let text = seg.text.trim_end_matches(['\n', '\r']);
        if text.is_empty() {
            continue;
        }
        match seg.kind {
            WordSegmentKind::Equal => buf.push_str(text),
            WordSegmentKind::Changed => {
                buf.push('[');
                buf.push_str(text);
                buf.push(']');
            }
        }
    }
    buf
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::build_description;
    use rigg_core::resources::ResourceKind;
    use rigg_diff::{Change, ChangeKind};
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

    /// Strip ANSI color codes from a string (for testing).
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
        assert!(desc.contains("differ locally vs on the server (2 lines changed)"));
        // Word-level diff: old side shows all-equal (no brackets), new side brackets changed words
        assert!(desc.contains("- Always be polite."));
        assert!(desc.contains("+ Always be"));
        assert!(desc.contains("[extremely]"));
        assert!(desc.contains("polite."));
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
        assert!(!desc.contains("lines changed"));
        assert!(!desc.contains("line changed"));
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
        // Word-level diff: changed number bracketed, rest equal
        assert!(desc.contains("- Maximum [5] sources per response."));
        assert!(desc.contains("+ Maximum [10] sources per response."));
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
        assert!(desc.contains("differs locally vs on the server (1 line changed)"));
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
        assert!(!desc.contains("lines changed"));
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
        let lines = super::super::describe_changes(
            &changes,
            ResourceKind::Agent,
            "bot",
            Some(("locally", "on the server")),
            false,
        );
        // Should produce multiple lines (header + diff lines)
        assert!(lines.len() > 1);
        let joined = lines.iter().map(|l| strip_ansi(l)).collect::<String>();
        assert!(joined.contains("differ"));
        assert!(joined.contains("- Line two"));
        assert!(joined.contains("+ Line TWO"));
    }
}
