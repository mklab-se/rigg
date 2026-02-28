//! Resource selection resolution from CLI flags.
//!
//! Resolves which resource kinds (and optional name filters) to operate on
//! based on plural boolean flags (`--indexes`), singular name flags
//! (`--index my-idx`), `--all`, `--search-only`, `--foundry-only`, and
//! the `include_preview` config setting.

use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;

use crate::cli::ResourceTypeFlags;

/// A resolved resource selection: which kinds to operate on and optional name filters.
#[derive(Debug, Clone)]
pub struct ResourceSelection {
    /// Resources to include: (kind, optional_exact_name)
    pub selections: Vec<(ResourceKind, Option<String>)>,
}

impl ResourceSelection {
    /// Get unique resource kinds in this selection.
    pub fn kinds(&self) -> Vec<ResourceKind> {
        let mut seen = Vec::new();
        for (kind, _) in &self.selections {
            if !seen.contains(kind) {
                seen.push(*kind);
            }
        }
        seen
    }

    /// Get the exact name filter for a given kind, if any.
    /// Returns `None` if the kind uses a plural (no-filter) selection or isn't selected.
    pub fn name_filter(&self, kind: ResourceKind) -> Option<&str> {
        for (k, name) in &self.selections {
            if *k == kind {
                return name.as_deref();
            }
        }
        None
    }

    /// Returns true if no resources are selected.
    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }
}

/// Singular resource flags: each is an Option<String> for a specific resource by name.
#[derive(Debug, Default, Clone)]
pub struct SingularFlags {
    pub index: Option<String>,
    pub indexer: Option<String>,
    pub datasource: Option<String>,
    pub skillset: Option<String>,
    pub synonymmap: Option<String>,
    pub alias: Option<String>,
    pub knowledgebase: Option<String>,
    pub knowledgesource: Option<String>,
    pub agent: Option<String>,
}

/// Resolve a ResourceSelection from `ResourceTypeFlags`.
///
/// Singular flags take precedence: `--knowledgebase my-kb` contributes
/// `(KnowledgeBase, Some("my-kb"))` while `--knowledgebases` contributes
/// `(KnowledgeBase, None)`.
///
/// If `--all` is set, singular flags are ignored.
///
/// `--search-only` / `--foundry-only` filter the result by service domain.
/// Default fallback (no flags) returns search resources only for backward compat.
pub fn resolve_resource_selection_from_flags(
    flags: &ResourceTypeFlags,
    include_preview: bool,
    has_default_fallback: bool,
) -> ResourceSelection {
    let singular = flags.singular_flags();
    let sel = resolve_resource_selection(
        flags.all,
        flags.indexes,
        flags.indexers,
        flags.datasources,
        flags.skillsets,
        flags.synonymmaps,
        flags.aliases,
        flags.knowledgebases,
        flags.knowledgesources,
        flags.agents,
        &singular,
        include_preview,
        has_default_fallback,
    );

    // Apply service scope filters
    if flags.search_only {
        ResourceSelection {
            selections: sel
                .selections
                .into_iter()
                .filter(|(k, _)| k.domain() == ServiceDomain::Search)
                .collect(),
        }
    } else if flags.foundry_only {
        ResourceSelection {
            selections: sel
                .selections
                .into_iter()
                .filter(|(k, _)| k.domain() == ServiceDomain::Foundry)
                .collect(),
        }
    } else {
        sel
    }
}

/// Resolve a ResourceSelection from both plural booleans and singular flags.
///
/// Singular flags take precedence: `--knowledgebase my-kb` contributes
/// `(KnowledgeBase, Some("my-kb"))` while `--knowledgebases` contributes
/// `(KnowledgeBase, None)`.
///
/// If `--all` is set, singular flags are ignored.
#[allow(clippy::too_many_arguments)]
pub fn resolve_resource_selection(
    all: bool,
    indexes: bool,
    indexers: bool,
    datasources: bool,
    skillsets: bool,
    synonymmaps: bool,
    aliases: bool,
    knowledgebases: bool,
    knowledgesources: bool,
    agents: bool,
    singular: &SingularFlags,
    include_preview: bool,
    has_default_fallback: bool,
) -> ResourceSelection {
    if all {
        let mut kinds = if include_preview {
            ResourceKind::all().to_vec()
        } else {
            let mut k = ResourceKind::stable().to_vec();
            // Agent is GA (not preview), always include with --all
            k.push(ResourceKind::Agent);
            k
        };
        // Deduplicate (Agent is already in all())
        kinds.dedup();
        return ResourceSelection {
            selections: kinds.into_iter().map(|k| (k, None)).collect(),
        };
    }

    let mut selections = Vec::new();

    // Singular flags (exact name match)
    let singular_pairs: &[(Option<&String>, ResourceKind, bool)] = &[
        (singular.index.as_ref(), ResourceKind::Index, true),
        (singular.indexer.as_ref(), ResourceKind::Indexer, true),
        (singular.datasource.as_ref(), ResourceKind::DataSource, true),
        (singular.skillset.as_ref(), ResourceKind::Skillset, true),
        (singular.synonymmap.as_ref(), ResourceKind::SynonymMap, true),
        (
            singular.alias.as_ref(),
            ResourceKind::Alias,
            include_preview,
        ),
        (
            singular.knowledgebase.as_ref(),
            ResourceKind::KnowledgeBase,
            include_preview,
        ),
        (
            singular.knowledgesource.as_ref(),
            ResourceKind::KnowledgeSource,
            include_preview,
        ),
        (singular.agent.as_ref(), ResourceKind::Agent, true),
    ];

    for (value, kind, allowed) in singular_pairs {
        if let Some(name) = value {
            if *allowed {
                selections.push((*kind, Some(name.to_string())));
            }
        }
    }

    // Plural boolean flags (no name filter) -- only add if singular not already present for that kind
    let plural_pairs: &[(bool, ResourceKind, bool)] = &[
        (indexes, ResourceKind::Index, true),
        (indexers, ResourceKind::Indexer, true),
        (datasources, ResourceKind::DataSource, true),
        (skillsets, ResourceKind::Skillset, true),
        (synonymmaps, ResourceKind::SynonymMap, true),
        (aliases, ResourceKind::Alias, include_preview),
        (knowledgebases, ResourceKind::KnowledgeBase, include_preview),
        (
            knowledgesources,
            ResourceKind::KnowledgeSource,
            include_preview,
        ),
        (agents, ResourceKind::Agent, true),
    ];

    for (flag, kind, allowed) in plural_pairs {
        if *flag && *allowed {
            // Only add if no singular already covers this kind
            if !selections.iter().any(|(k, _)| *k == *kind) {
                selections.push((*kind, None));
            }
        }
    }

    // Default fallback if nothing specified -- include all configured resources
    if selections.is_empty() && has_default_fallback {
        let mut kinds: Vec<ResourceKind> = if include_preview {
            ResourceKind::all()
                .iter()
                .filter(|k| k.domain() == ServiceDomain::Search)
                .copied()
                .collect()
        } else {
            ResourceKind::stable().to_vec()
        };
        // Always include Foundry resources in default (pull command skips if not configured)
        kinds.push(ResourceKind::Agent);
        return ResourceSelection {
            selections: kinds.into_iter().map(|k| (k, None)).collect(),
        };
    }

    ResourceSelection { selections }
}

/// Resolve which resource kinds to operate on based on CLI flags.
///
/// Superseded by `resolve_resource_selection()` which also handles singular flags.
#[allow(clippy::too_many_arguments)]
#[cfg(test)]
pub fn resolve_resource_kinds(
    all: bool,
    indexes: bool,
    indexers: bool,
    datasources: bool,
    skillsets: bool,
    synonymmaps: bool,
    aliases: bool,
    knowledgebases: bool,
    knowledgesources: bool,
    include_preview: bool,
    has_default_fallback: bool,
) -> Vec<ResourceKind> {
    let sel = resolve_resource_selection(
        all,
        indexes,
        indexers,
        datasources,
        skillsets,
        synonymmaps,
        aliases,
        knowledgebases,
        knowledgesources,
        false, // agents
        &SingularFlags::default(),
        include_preview,
        has_default_fallback,
    );
    sel.kinds()
}

#[cfg(test)]
mod tests {
    use super::*;

    // === resolve_resource_kinds tests ===

    #[test]
    fn test_all_with_preview() {
        let kinds = resolve_resource_kinds(
            true, false, false, false, false, false, false, false, false, true, false,
        );
        assert_eq!(kinds.len(), 9);
        assert!(kinds.contains(&ResourceKind::KnowledgeBase));
        assert!(kinds.contains(&ResourceKind::KnowledgeSource));
        assert!(kinds.contains(&ResourceKind::Agent));
    }

    #[test]
    fn test_all_without_preview() {
        let kinds = resolve_resource_kinds(
            true, false, false, false, false, false, false, false, false, false, false,
        );
        // stable (5) + Agent (1) = 6
        assert_eq!(kinds.len(), 6);
        assert!(!kinds.contains(&ResourceKind::Alias));
        assert!(!kinds.contains(&ResourceKind::KnowledgeBase));
        assert!(!kinds.contains(&ResourceKind::KnowledgeSource));
        assert!(kinds.contains(&ResourceKind::Agent));
    }

    #[test]
    fn test_specific_flags_override_default() {
        let kinds = resolve_resource_kinds(
            false, true, false, false, false, false, false, false, false, true, true,
        );
        assert_eq!(kinds, vec![ResourceKind::Index]);
    }

    #[test]
    fn test_no_flags_with_fallback_and_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, false, false, true, true,
        );
        // Default fallback returns search resources (8) + Agent (1) = 9
        assert_eq!(kinds.len(), 9);
        assert!(kinds.contains(&ResourceKind::Agent));
    }

    #[test]
    fn test_no_flags_with_fallback_without_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, false, false, false, true,
        );
        // stable (5) + Agent (1) = 6
        assert_eq!(kinds.len(), 6);
        assert!(kinds.contains(&ResourceKind::Agent));
        for k in ResourceKind::stable() {
            assert!(kinds.contains(k));
        }
    }

    #[test]
    fn test_no_flags_without_fallback_returns_empty() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, false, false, true, false,
        );
        assert!(kinds.is_empty());
    }

    #[test]
    fn test_knowledge_flags_require_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, true, true, false, false,
        );
        // include_preview is false, so KB/KS flags are ignored
        assert!(kinds.is_empty());
    }

    #[test]
    fn test_knowledge_flags_with_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, true, true, true, false,
        );
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&ResourceKind::KnowledgeBase));
        assert!(kinds.contains(&ResourceKind::KnowledgeSource));
    }

    #[test]
    fn test_knowledge_flags_ignored_falls_back_to_default() {
        // KB/KS flags set but include_preview=false, no other flags -> falls back
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, true, true, false, true,
        );
        // stable (5) + Agent (1) = 6
        assert_eq!(kinds.len(), 6);
        assert!(kinds.contains(&ResourceKind::Agent));
    }

    // === resolve_resource_selection tests ===

    fn no_singular() -> SingularFlags {
        SingularFlags::default()
    }

    #[test]
    fn test_selection_singular_flag() {
        let mut singular = no_singular();
        singular.knowledgebase = Some("my-kb".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        assert_eq!(sel.kinds(), vec![ResourceKind::KnowledgeBase]);
        assert_eq!(sel.name_filter(ResourceKind::KnowledgeBase), Some("my-kb"));
    }

    #[test]
    fn test_selection_plural_flag() {
        let sel = resolve_resource_selection(
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            &no_singular(),
            true,
            false,
        );

        assert_eq!(sel.kinds(), vec![ResourceKind::Index]);
        assert_eq!(sel.name_filter(ResourceKind::Index), None);
    }

    #[test]
    fn test_selection_mix_singular_and_plural() {
        let mut singular = no_singular();
        singular.knowledgebase = Some("my-kb".to_string());

        let sel = resolve_resource_selection(
            false, true, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        assert_eq!(sel.kinds().len(), 2);
        assert_eq!(sel.name_filter(ResourceKind::Index), None);
        assert_eq!(sel.name_filter(ResourceKind::KnowledgeBase), Some("my-kb"));
    }

    #[test]
    fn test_selection_all_ignores_singulars() {
        let mut singular = no_singular();
        singular.knowledgebase = Some("my-kb".to_string());

        let sel = resolve_resource_selection(
            true, false, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        assert_eq!(sel.kinds().len(), 9);
        // --all clears all name filters
        assert_eq!(sel.name_filter(ResourceKind::KnowledgeBase), None);
    }

    #[test]
    fn test_selection_name_filter() {
        let mut singular = no_singular();
        singular.index = Some("my-idx".to_string());
        singular.indexer = Some("my-ixer".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        assert_eq!(sel.name_filter(ResourceKind::Index), Some("my-idx"));
        assert_eq!(sel.name_filter(ResourceKind::Indexer), Some("my-ixer"));
        assert_eq!(sel.name_filter(ResourceKind::DataSource), None);
    }

    #[test]
    fn test_selection_singular_plural_same_kind_singular_wins() {
        let mut singular = no_singular();
        singular.index = Some("specific-idx".to_string());

        // Both --index specific-idx and --indexes are set
        let sel = resolve_resource_selection(
            false, true, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        // Singular takes precedence -- only one entry for Index
        assert_eq!(sel.kinds(), vec![ResourceKind::Index]);
        assert_eq!(sel.name_filter(ResourceKind::Index), Some("specific-idx"));
    }

    #[test]
    fn test_selection_preview_singular_requires_include_preview() {
        let mut singular = no_singular();
        singular.knowledgebase = Some("my-kb".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, false,
            false,
        );

        assert!(sel.is_empty());
    }

    #[test]
    fn test_selection_mixed_preview_and_stable_singular() {
        // --index my-idx --knowledgebase my-kb with include_preview=false
        // Should include Index but NOT KnowledgeBase
        let mut singular = no_singular();
        singular.index = Some("my-idx".to_string());
        singular.knowledgebase = Some("my-kb".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, false,
            false,
        );

        assert_eq!(sel.kinds(), vec![ResourceKind::Index]);
        assert_eq!(sel.name_filter(ResourceKind::Index), Some("my-idx"));
        assert_eq!(sel.name_filter(ResourceKind::KnowledgeBase), None);
    }

    #[test]
    fn test_selection_no_flags_no_singular_no_fallback() {
        let sel = resolve_resource_selection(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            &no_singular(),
            true,
            false,
        );
        assert!(sel.is_empty());
    }

    #[test]
    fn test_selection_no_flags_no_singular_with_fallback() {
        let sel = resolve_resource_selection(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            &no_singular(),
            true,
            true,
        );
        // Falls back to search kinds (8) + Agent (1) = 9
        assert_eq!(sel.kinds().len(), 9);
        // All entries have no name filter
        for kind in sel.kinds() {
            assert_eq!(sel.name_filter(kind), None);
        }
    }

    #[test]
    fn test_selection_is_empty_with_items() {
        let sel = resolve_resource_selection(
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            &no_singular(),
            true,
            false,
        );
        assert!(!sel.is_empty());
    }

    #[test]
    fn test_selection_multiple_singular_flags() {
        let mut singular = no_singular();
        singular.index = Some("my-idx".to_string());
        singular.datasource = Some("my-ds".to_string());
        singular.indexer = Some("my-ixer".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        assert_eq!(sel.kinds().len(), 3);
        assert_eq!(sel.name_filter(ResourceKind::Index), Some("my-idx"));
        assert_eq!(sel.name_filter(ResourceKind::DataSource), Some("my-ds"));
        assert_eq!(sel.name_filter(ResourceKind::Indexer), Some("my-ixer"));
        // Unselected kinds return None
        assert_eq!(sel.name_filter(ResourceKind::Skillset), None);
    }

    #[test]
    fn test_selection_singular_prevents_fallback() {
        // A singular flag is set -> should NOT fall back to all kinds
        let mut singular = no_singular();
        singular.index = Some("my-idx".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, true,
            true, // has_default_fallback=true
        );

        // Only Index, not all 8
        assert_eq!(sel.kinds(), vec![ResourceKind::Index]);
    }

    // === Foundry agent selection tests ===

    #[test]
    fn test_selection_agents_plural_flag() {
        let sel = resolve_resource_selection(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            &no_singular(),
            true,
            false,
        );
        assert_eq!(sel.kinds(), vec![ResourceKind::Agent]);
    }

    #[test]
    fn test_selection_agent_singular_flag() {
        let mut singular = no_singular();
        singular.agent = Some("my-agent".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, false, false, &singular, true,
            false,
        );

        assert_eq!(sel.kinds(), vec![ResourceKind::Agent]);
        assert_eq!(sel.name_filter(ResourceKind::Agent), Some("my-agent"));
    }

    #[test]
    fn test_selection_agents_with_search() {
        let sel = resolve_resource_selection(
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            true,
            &no_singular(),
            true,
            false,
        );
        assert_eq!(sel.kinds().len(), 2);
        assert!(sel.kinds().contains(&ResourceKind::Index));
        assert!(sel.kinds().contains(&ResourceKind::Agent));
    }

    #[test]
    fn test_selection_default_fallback_includes_foundry() {
        // Default fallback now includes Agent
        let sel = resolve_resource_selection(
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            &no_singular(),
            false,
            true,
        );
        assert!(sel.kinds().contains(&ResourceKind::Agent));
        // stable (5) + Agent (1) = 6
        assert_eq!(sel.kinds().len(), 6);
    }

    // === resolve_resource_selection_from_flags tests ===

    #[test]
    fn test_flags_search_only_filters() {
        let flags = ResourceTypeFlags {
            all: true,
            search_only: true,
            ..Default::default()
        };
        let sel = resolve_resource_selection_from_flags(&flags, true, false);
        for (kind, _) in &sel.selections {
            assert_eq!(kind.domain(), ServiceDomain::Search);
        }
        assert!(!sel.kinds().contains(&ResourceKind::Agent));
    }

    #[test]
    fn test_flags_foundry_only_filters() {
        let flags = ResourceTypeFlags {
            all: true,
            foundry_only: true,
            ..Default::default()
        };
        let sel = resolve_resource_selection_from_flags(&flags, true, false);
        for (kind, _) in &sel.selections {
            assert_eq!(kind.domain(), ServiceDomain::Foundry);
        }
        assert_eq!(sel.kinds(), vec![ResourceKind::Agent]);
    }

    #[test]
    fn test_flags_singular_extraction() {
        let flags = ResourceTypeFlags {
            index: Some("my-idx".to_string()),
            agent: Some("my-agent".to_string()),
            ..Default::default()
        };
        let singular = flags.singular_flags();
        assert_eq!(singular.index, Some("my-idx".to_string()));
        assert_eq!(singular.agent, Some("my-agent".to_string()));
        assert_eq!(singular.indexer, None);
    }
}
