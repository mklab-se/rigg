//! hoist-diff - Semantic JSON diff library for Azure AI Search resources
//!
//! This crate provides:
//! - Semantic diffing of JSON structures (key-based array matching)
//! - Diff output in text and JSON formats
//! - Change classification (additions, deletions, modifications)

pub mod output;
pub mod semantic;
pub mod text_diff;

pub use output::{format_json, format_text, format_value_preview};
pub use semantic::{Change, ChangeKind, DiffResult, diff};
pub use text_diff::{
    CONTEXT_TRUNCATE_LEN, DiffLine, TextDiffHunk, TextDiffResult, WordSegment, WordSegmentKind,
    diff_text, diff_words, is_long_text, truncate_context,
};
