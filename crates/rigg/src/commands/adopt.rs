//! `rigg adopt` — bring selected unmanaged remote resources into a project.
//!
//! This module currently holds only the `Selector` parser (Task 1 of the
//! `rigg adopt` workstream); the command itself lands in a later task, so
//! these items are unused from `main.rs`'s perspective until then.
#![allow(dead_code)]

use anyhow::{Result, anyhow};

use rigg_core::resources::{ResourceKind, ResourceRef, validate_resource_name};

/// What the user asked to adopt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// Every unmanaged resource across both services.
    All,
    /// Every unmanaged resource of one kind.
    Kind(ResourceKind),
    /// One specific resource.
    One(ResourceRef),
}

impl Selector {
    pub fn parse(s: &str) -> Result<Selector> {
        if s == "all" {
            return Ok(Selector::All);
        }
        if let Some((dir, name)) = s.split_once('/') {
            let kind = ResourceKind::from_directory_name(dir)
                .ok_or_else(|| anyhow!(unknown_kind_msg(dir)))?;
            validate_resource_name(name)
                .map_err(|e| anyhow!("invalid resource name '{name}': {e}"))?;
            return Ok(Selector::One(ResourceRef::new(kind, name.to_string())));
        }
        let kind =
            ResourceKind::from_directory_name(s).ok_or_else(|| anyhow!(unknown_kind_msg(s)))?;
        Ok(Selector::Kind(kind))
    }

    /// Broad selectors (all / whole-kind) require confirmation before writing.
    pub fn is_broad(&self) -> bool {
        matches!(self, Selector::All | Selector::Kind(_))
    }
}

fn unknown_kind_msg(dir: &str) -> String {
    let kinds = ResourceKind::all()
        .iter()
        .map(|k| k.directory_name())
        .collect::<Vec<_>>()
        .join(", ");
    format!("unknown resource kind '{dir}'. Valid kinds: {kinds}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all() {
        assert_eq!(Selector::parse("all").unwrap(), Selector::All);
    }

    #[test]
    fn parses_bare_kind() {
        assert_eq!(
            Selector::parse("indexes").unwrap(),
            Selector::Kind(ResourceKind::Index)
        );
    }

    #[test]
    fn parses_kind_slash_name() {
        assert_eq!(
            Selector::parse("indexes/hotels").unwrap(),
            Selector::One(ResourceRef::new(ResourceKind::Index, "hotels".to_string()))
        );
    }

    #[test]
    fn unknown_kind_is_error_listing_kinds() {
        let err = Selector::parse("widgets").unwrap_err().to_string();
        assert!(err.contains("unknown resource kind 'widgets'"), "{err}");
        assert!(err.contains("indexes"), "lists valid kinds: {err}");
    }

    #[test]
    fn is_broad_classifies_correctly() {
        assert!(Selector::parse("all").unwrap().is_broad());
        assert!(Selector::parse("indexes").unwrap().is_broad());
        assert!(!Selector::parse("indexes/hotels").unwrap().is_broad());
    }
}
