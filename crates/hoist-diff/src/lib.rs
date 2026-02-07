//! hoist-diff - Semantic JSON diff library for Azure AI Search resources
//!
//! This crate provides:
//! - Semantic diffing of JSON structures (key-based array matching)
//! - Diff output in text and JSON formats
//! - Change classification (additions, deletions, modifications)

pub mod output;
pub mod semantic;

pub use output::{format_json, format_text};
pub use semantic::{diff, Change, ChangeKind, DiffResult};
