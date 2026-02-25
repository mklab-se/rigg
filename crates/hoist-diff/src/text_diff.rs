//! Line-level text diffing for long text properties.
//!
//! Produces structured diffs with context lines for human-readable
//! comparison of instructions, descriptions, and other long text fields.

use similar::{ChangeTag, TextDiff};

/// Minimum string length to trigger line-level diffing.
pub const LONG_TEXT_THRESHOLD: usize = 120;

/// A single line in a text diff.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    /// Unchanged context line.
    Equal(String),
    /// Deleted line (only in old text).
    Delete(String),
    /// Inserted line (only in new text).
    Insert(String),
}

/// A group of contiguous diff lines with surrounding context.
#[derive(Debug, Clone)]
pub struct TextDiffHunk {
    pub lines: Vec<DiffLine>,
}

/// Result of a line-level text diff.
#[derive(Debug, Clone)]
pub struct TextDiffResult {
    pub hunks: Vec<TextDiffHunk>,
    pub deletions: usize,
    pub insertions: usize,
}

/// Returns `true` if the string is long enough or multi-line to warrant
/// line-level diffing instead of inline comparison.
pub fn is_long_text(s: &str) -> bool {
    s.len() >= LONG_TEXT_THRESHOLD || s.contains('\n')
}

/// Compute a line-level diff between two strings.
///
/// Returns hunks with 2 lines of surrounding context per change group.
pub fn diff_text(old: &str, new: &str) -> TextDiffResult {
    let diff = TextDiff::from_lines(old, new);
    let mut deletions = 0;
    let mut insertions = 0;
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(2) {
        let mut lines = Vec::new();
        for op in &group {
            for change in diff.iter_changes(op) {
                let text = change.value().to_string();
                match change.tag() {
                    ChangeTag::Equal => lines.push(DiffLine::Equal(text)),
                    ChangeTag::Delete => {
                        deletions += 1;
                        lines.push(DiffLine::Delete(text));
                    }
                    ChangeTag::Insert => {
                        insertions += 1;
                        lines.push(DiffLine::Insert(text));
                    }
                }
            }
        }
        if !lines.is_empty() {
            hunks.push(TextDiffHunk { lines });
        }
    }

    TextDiffResult {
        hunks,
        deletions,
        insertions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_long_text_short() {
        assert!(!is_long_text("hello"));
    }

    #[test]
    fn test_is_long_text_by_length() {
        let s = "a".repeat(LONG_TEXT_THRESHOLD);
        assert!(is_long_text(&s));
    }

    #[test]
    fn test_is_long_text_multiline() {
        assert!(is_long_text("line one\nline two"));
    }

    #[test]
    fn test_diff_identical() {
        let result = diff_text("hello\n", "hello\n");
        assert_eq!(result.deletions, 0);
        assert_eq!(result.insertions, 0);
        assert!(result.hunks.is_empty());
    }

    #[test]
    fn test_diff_single_line_change() {
        let old = "line one\nline two\nline three\n";
        let new = "line one\nline TWO\nline three\n";
        let result = diff_text(old, new);
        assert_eq!(result.deletions, 1);
        assert_eq!(result.insertions, 1);
        assert_eq!(result.hunks.len(), 1);

        let lines = &result.hunks[0].lines;
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Delete(s) if s.contains("line two")))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Insert(s) if s.contains("line TWO")))
        );
    }

    #[test]
    fn test_diff_addition() {
        let old = "line one\nline three\n";
        let new = "line one\nline two\nline three\n";
        let result = diff_text(old, new);
        assert_eq!(result.deletions, 0);
        assert_eq!(result.insertions, 1);
    }

    #[test]
    fn test_diff_deletion() {
        let old = "line one\nline two\nline three\n";
        let new = "line one\nline three\n";
        let result = diff_text(old, new);
        assert_eq!(result.deletions, 1);
        assert_eq!(result.insertions, 0);
    }

    #[test]
    fn test_diff_multiple_hunks() {
        let old = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
        let new = "a\nB\nc\nd\ne\nf\ng\nH\ni\nj\n";
        let result = diff_text(old, new);
        assert_eq!(result.deletions, 2);
        assert_eq!(result.insertions, 2);
        // With 2 context lines, changes at positions 2 and 8 (0-indexed)
        // should form separate hunks since they're >4 lines apart
        assert_eq!(result.hunks.len(), 2);
    }

    #[test]
    fn test_diff_context_lines() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nC\nd\ne\n";
        let result = diff_text(old, new);
        assert_eq!(result.hunks.len(), 1);

        let lines = &result.hunks[0].lines;
        // Should have context lines around the change
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Equal(s) if s.starts_with('a')))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Equal(s) if s.starts_with('b')))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Delete(s) if s.starts_with('c')))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Insert(s) if s.starts_with('C')))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Equal(s) if s.starts_with('d')))
        );
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Equal(s) if s.starts_with('e')))
        );
    }
}
