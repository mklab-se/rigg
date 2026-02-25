//! Line-level text diffing for long text properties.
//!
//! Produces structured diffs with context lines for human-readable
//! comparison of instructions, descriptions, and other long text fields.
//! Supports word-level highlighting within modified paragraphs.

use similar::{ChangeTag, TextDiff};

/// Minimum string length to trigger line-level diffing.
pub const LONG_TEXT_THRESHOLD: usize = 120;

/// Maximum display length for context lines before truncation.
pub const CONTEXT_TRUNCATE_LEN: usize = 80;

/// A segment of a word-level diff within a single line.
#[derive(Debug, Clone, PartialEq)]
pub struct WordSegment {
    pub text: String,
    pub kind: WordSegmentKind,
}

/// Whether a word segment is unchanged or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WordSegmentKind {
    Equal,
    Changed,
}

/// A single line in a text diff.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine {
    /// Unchanged context line.
    Equal(String),
    /// Deleted line (only in old text).
    Delete(String),
    /// Inserted line (only in new text).
    Insert(String),
    /// A modified line with word-level diff segments.
    /// Contains old line segments and new line segments.
    Modified {
        old_segments: Vec<WordSegment>,
        new_segments: Vec<WordSegment>,
    },
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
/// Returns hunks with 1 line of surrounding context per change group.
/// Consecutive Delete+Insert pairs are converted to Modified entries
/// with word-level diff segments.
pub fn diff_text(old: &str, new: &str) -> TextDiffResult {
    let diff = TextDiff::from_lines(old, new);
    let mut deletions = 0;
    let mut insertions = 0;
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(1) {
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
            lines = pair_modifications(lines);
            hunks.push(TextDiffHunk { lines });
        }
    }

    TextDiffResult {
        hunks,
        deletions,
        insertions,
    }
}

/// Convert consecutive Delete+Insert runs into Modified entries with word-level diffs.
///
/// Pairs them 1:1 — if there are 3 deletes and 2 inserts, the first 2 become
/// Modified pairs and the 3rd remains a standalone Delete.
fn pair_modifications(lines: Vec<DiffLine>) -> Vec<DiffLine> {
    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;

    while i < lines.len() {
        // Look for a run of Delete lines followed by Insert lines
        if matches!(&lines[i], DiffLine::Delete(_)) {
            let delete_start = i;
            while i < lines.len() && matches!(&lines[i], DiffLine::Delete(_)) {
                i += 1;
            }
            let delete_end = i;

            let insert_start = i;
            while i < lines.len() && matches!(&lines[i], DiffLine::Insert(_)) {
                i += 1;
            }
            let insert_end = i;

            let delete_count = delete_end - delete_start;
            let insert_count = insert_end - insert_start;
            let pair_count = delete_count.min(insert_count);

            // Pair deletes with inserts as Modified entries
            for j in 0..pair_count {
                let old_text = match &lines[delete_start + j] {
                    DiffLine::Delete(s) => s.clone(),
                    _ => unreachable!(),
                };
                let new_text = match &lines[insert_start + j] {
                    DiffLine::Insert(s) => s.clone(),
                    _ => unreachable!(),
                };

                let (old_segments, new_segments) = diff_words(&old_text, &new_text);
                result.push(DiffLine::Modified {
                    old_segments,
                    new_segments,
                });
            }

            // Emit remaining unpaired deletes
            for j in pair_count..delete_count {
                result.push(lines[delete_start + j].clone());
            }

            // Emit remaining unpaired inserts
            for j in pair_count..insert_count {
                result.push(lines[insert_start + j].clone());
            }
        } else {
            result.push(lines[i].clone());
            i += 1;
        }
    }

    result
}

/// Compute word-level diff segments between two strings.
///
/// Returns (old_segments, new_segments) where each segment indicates
/// whether it's equal or changed relative to the other side.
pub fn diff_words(old: &str, new: &str) -> (Vec<WordSegment>, Vec<WordSegment>) {
    let diff = TextDiff::from_words(old, new);
    let mut old_segments = Vec::new();
    let mut new_segments = Vec::new();

    for change in diff.iter_all_changes() {
        let text = change.value().to_string();
        match change.tag() {
            ChangeTag::Equal => {
                old_segments.push(WordSegment {
                    text: text.clone(),
                    kind: WordSegmentKind::Equal,
                });
                new_segments.push(WordSegment {
                    text,
                    kind: WordSegmentKind::Equal,
                });
            }
            ChangeTag::Delete => {
                old_segments.push(WordSegment {
                    text,
                    kind: WordSegmentKind::Changed,
                });
            }
            ChangeTag::Insert => {
                new_segments.push(WordSegment {
                    text,
                    kind: WordSegmentKind::Changed,
                });
            }
        }
    }

    (old_segments, new_segments)
}

/// Truncate a string to approximately `max_len` characters with `" ... "` in the middle.
///
/// If the string is shorter than or equal to `max_len`, returns it unchanged.
/// Uses `char_indices()` for Unicode safety.
pub fn truncate_context(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        return s.to_string();
    }

    let ellipsis = " ... ";
    let ellipsis_len = ellipsis.len();

    if max_len <= ellipsis_len + 4 {
        // Too short for meaningful truncation
        return s.chars().take(max_len).collect();
    }

    let half = (max_len - ellipsis_len) / 2;

    // Find the byte index for the prefix (first `half` chars)
    let prefix_end = s
        .char_indices()
        .nth(half)
        .map(|(i, _)| i)
        .unwrap_or(s.len());

    // Find the byte index for the suffix (last `half` chars)
    let total_chars = s.chars().count();
    let suffix_start_char = total_chars.saturating_sub(half);
    let suffix_start = s
        .char_indices()
        .nth(suffix_start_char)
        .map(|(i, _)| i)
        .unwrap_or(s.len());

    format!("{}{}{}", &s[..prefix_end], ellipsis, &s[suffix_start..])
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

        // Should produce a Modified entry (paired delete+insert)
        let lines = &result.hunks[0].lines;
        assert!(lines.iter().any(|l| matches!(l, DiffLine::Modified { .. })));
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
        // With 1 context line, changes at positions 2 and 8 (0-indexed)
        // should form separate hunks since they're >2 lines apart
        assert_eq!(result.hunks.len(), 2);
    }

    #[test]
    fn test_diff_context_lines() {
        let old = "a\nb\nc\nd\ne\n";
        let new = "a\nb\nC\nd\ne\n";
        let result = diff_text(old, new);
        assert_eq!(result.hunks.len(), 1);

        let lines = &result.hunks[0].lines;
        // With 1 context line, we should have: b (context), Modified(c->C), d (context)
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Equal(s) if s.starts_with('b')))
        );
        assert!(lines.iter().any(|l| matches!(l, DiffLine::Modified { .. })));
        assert!(
            lines
                .iter()
                .any(|l| matches!(l, DiffLine::Equal(s) if s.starts_with('d')))
        );
    }

    // === Word-level diff tests ===

    #[test]
    fn test_diff_words_single_word_change() {
        let (old_segs, new_segs) = diff_words("hello world", "hello earth");
        // "hello " is equal, "world"/"earth" is changed
        assert!(
            old_segs
                .iter()
                .any(|s| s.kind == WordSegmentKind::Changed && s.text == "world")
        );
        assert!(
            new_segs
                .iter()
                .any(|s| s.kind == WordSegmentKind::Changed && s.text == "earth")
        );
        assert!(
            old_segs
                .iter()
                .any(|s| s.kind == WordSegmentKind::Equal && s.text.contains("hello"))
        );
    }

    #[test]
    fn test_diff_words_no_change() {
        let (old_segs, new_segs) = diff_words("same text", "same text");
        assert!(old_segs.iter().all(|s| s.kind == WordSegmentKind::Equal));
        assert!(new_segs.iter().all(|s| s.kind == WordSegmentKind::Equal));
    }

    #[test]
    fn test_diff_words_completely_different() {
        let (old_segs, new_segs) = diff_words("foo bar", "baz qux");
        assert!(old_segs.iter().any(|s| s.kind == WordSegmentKind::Changed));
        assert!(new_segs.iter().any(|s| s.kind == WordSegmentKind::Changed));
    }

    // === pair_modifications tests ===

    #[test]
    fn test_pair_modifications_basic() {
        let lines = vec![
            DiffLine::Equal("context\n".to_string()),
            DiffLine::Delete("old line\n".to_string()),
            DiffLine::Insert("new line\n".to_string()),
            DiffLine::Equal("context\n".to_string()),
        ];
        let result = pair_modifications(lines);
        assert_eq!(result.len(), 3); // Equal, Modified, Equal
        assert!(matches!(&result[1], DiffLine::Modified { .. }));
    }

    #[test]
    fn test_pair_modifications_uneven_more_deletes() {
        let lines = vec![
            DiffLine::Delete("old 1\n".to_string()),
            DiffLine::Delete("old 2\n".to_string()),
            DiffLine::Delete("old 3\n".to_string()),
            DiffLine::Insert("new 1\n".to_string()),
            DiffLine::Insert("new 2\n".to_string()),
        ];
        let result = pair_modifications(lines);
        // 2 Modified + 1 leftover Delete
        assert_eq!(result.len(), 3);
        assert!(matches!(&result[0], DiffLine::Modified { .. }));
        assert!(matches!(&result[1], DiffLine::Modified { .. }));
        assert!(matches!(&result[2], DiffLine::Delete(_)));
    }

    #[test]
    fn test_pair_modifications_uneven_more_inserts() {
        let lines = vec![
            DiffLine::Delete("old 1\n".to_string()),
            DiffLine::Insert("new 1\n".to_string()),
            DiffLine::Insert("new 2\n".to_string()),
        ];
        let result = pair_modifications(lines);
        // 1 Modified + 1 leftover Insert
        assert_eq!(result.len(), 2);
        assert!(matches!(&result[0], DiffLine::Modified { .. }));
        assert!(matches!(&result[1], DiffLine::Insert(_)));
    }

    #[test]
    fn test_pair_modifications_no_pairs() {
        let lines = vec![
            DiffLine::Equal("context\n".to_string()),
            DiffLine::Insert("new\n".to_string()),
            DiffLine::Equal("context\n".to_string()),
        ];
        let result = pair_modifications(lines);
        assert_eq!(result.len(), 3);
        assert!(matches!(&result[1], DiffLine::Insert(_)));
    }

    // === truncate_context tests ===

    #[test]
    fn test_truncate_context_short_string() {
        let s = "short string";
        assert_eq!(truncate_context(s, 80), s);
    }

    #[test]
    fn test_truncate_context_exact_length() {
        let s = "a".repeat(80);
        assert_eq!(truncate_context(&s, 80), s);
    }

    #[test]
    fn test_truncate_context_long_string() {
        let s = "a".repeat(200);
        let truncated = truncate_context(&s, 80);
        assert!(truncated.len() <= 85); // allow slight overshoot from char boundaries
        assert!(truncated.contains(" ... "));
    }

    #[test]
    fn test_truncate_context_preserves_ends() {
        let s = "START middle padding that is quite long and should be truncated away END";
        let truncated = truncate_context(&s, 40);
        assert!(truncated.starts_with("START"));
        assert!(truncated.ends_with("END"));
        assert!(truncated.contains(" ... "));
    }

    #[test]
    fn test_truncate_context_unicode() {
        let s = "aaa\u{00e9}\u{00e9}\u{00e9}".repeat(20);
        let truncated = truncate_context(&s, 40);
        // Should not panic on unicode boundaries
        assert!(truncated.len() < s.len());
    }

    // === Modified variant in diff output ===

    #[test]
    fn test_diff_produces_modified_for_paired_changes() {
        let old = "line one\nold paragraph content here\nline three\n";
        let new = "line one\nnew paragraph content here\nline three\n";
        let result = diff_text(old, new);

        let has_modified = result.hunks.iter().any(|h| {
            h.lines
                .iter()
                .any(|l| matches!(l, DiffLine::Modified { .. }))
        });
        assert!(has_modified, "Paired delete+insert should become Modified");
    }

    #[test]
    fn test_modified_has_word_segments() {
        let old = "The IRMA framework requires validation.\n";
        let new = "The revised IRMA framework v2 requires validation.\n";
        let result = diff_text(old, new);

        for hunk in &result.hunks {
            for line in &hunk.lines {
                if let DiffLine::Modified {
                    old_segments,
                    new_segments,
                } = line
                {
                    // Old side should have equal segments and no "revised"/"v2"
                    assert!(
                        old_segments
                            .iter()
                            .any(|s| s.kind == WordSegmentKind::Equal)
                    );
                    // New side should have "revised" and "v2" as changed
                    let new_changed: String = new_segments
                        .iter()
                        .filter(|s| s.kind == WordSegmentKind::Changed)
                        .map(|s| s.text.as_str())
                        .collect();
                    assert!(
                        new_changed.contains("revised") || new_changed.contains("v2"),
                        "Expected changed words in new_segments, got: {}",
                        new_changed
                    );
                    return; // Found and verified
                }
            }
        }
        panic!("Expected a Modified DiffLine");
    }
}
