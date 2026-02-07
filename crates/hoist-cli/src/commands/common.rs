//! Shared utilities used by multiple commands

use hoist_core::resources::{Resource, ResourceKind};

/// Resolve which resource kinds to operate on based on CLI flags.
///
/// If `--all` is set, returns all kinds (respecting `include_preview`).
/// If specific flags are set, returns only those kinds.
/// If no flags are set and `has_default_fallback` is true, returns all kinds (respecting `include_preview`).
/// If no flags are set and `has_default_fallback` is false, returns an empty vec.
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
    knowledgebases: bool,
    knowledgesources: bool,
    include_preview: bool,
    has_default_fallback: bool,
) -> Vec<ResourceKind> {
    if all {
        return if include_preview {
            ResourceKind::all().to_vec()
        } else {
            ResourceKind::stable().to_vec()
        };
    }

    let mut kinds = Vec::new();

    if indexes {
        kinds.push(ResourceKind::Index);
    }
    if indexers {
        kinds.push(ResourceKind::Indexer);
    }
    if datasources {
        kinds.push(ResourceKind::DataSource);
    }
    if skillsets {
        kinds.push(ResourceKind::Skillset);
    }
    if synonymmaps {
        kinds.push(ResourceKind::SynonymMap);
    }
    if knowledgebases && include_preview {
        kinds.push(ResourceKind::KnowledgeBase);
    }
    if knowledgesources && include_preview {
        kinds.push(ResourceKind::KnowledgeSource);
    }

    // If nothing specified, default based on include_preview setting
    if kinds.is_empty() && has_default_fallback {
        kinds = if include_preview {
            ResourceKind::all().to_vec()
        } else {
            ResourceKind::stable().to_vec()
        };
    }

    kinds
}

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
    pub knowledgebase: Option<String>,
    pub knowledgesource: Option<String>,
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
    knowledgebases: bool,
    knowledgesources: bool,
    singular: &SingularFlags,
    include_preview: bool,
    has_default_fallback: bool,
) -> ResourceSelection {
    if all {
        let kinds = if include_preview {
            ResourceKind::all().to_vec()
        } else {
            ResourceKind::stable().to_vec()
        };
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
            singular.knowledgebase.as_ref(),
            ResourceKind::KnowledgeBase,
            include_preview,
        ),
        (
            singular.knowledgesource.as_ref(),
            ResourceKind::KnowledgeSource,
            include_preview,
        ),
    ];

    for (value, kind, allowed) in singular_pairs {
        if let Some(name) = value {
            if *allowed {
                selections.push((*kind, Some(name.to_string())));
            }
        }
    }

    // Plural boolean flags (no name filter) — only add if singular not already present for that kind
    let plural_pairs: &[(bool, ResourceKind, bool)] = &[
        (indexes, ResourceKind::Index, true),
        (indexers, ResourceKind::Indexer, true),
        (datasources, ResourceKind::DataSource, true),
        (skillsets, ResourceKind::Skillset, true),
        (synonymmaps, ResourceKind::SynonymMap, true),
        (knowledgebases, ResourceKind::KnowledgeBase, include_preview),
        (
            knowledgesources,
            ResourceKind::KnowledgeSource,
            include_preview,
        ),
    ];

    for (flag, kind, allowed) in plural_pairs {
        if *flag && *allowed {
            // Only add if no singular already covers this kind
            if !selections.iter().any(|(k, _)| *k == *kind) {
                selections.push((*kind, None));
            }
        }
    }

    // Default fallback if nothing specified
    if selections.is_empty() && has_default_fallback {
        let kinds = if include_preview {
            ResourceKind::all().to_vec()
        } else {
            ResourceKind::stable().to_vec()
        };
        return ResourceSelection {
            selections: kinds.into_iter().map(|k| (k, None)).collect(),
        };
    }

    ResourceSelection { selections }
}

/// Get the volatile fields to strip for a given resource kind during normalization.
/// These are stripped from local files during pull AND before push.
pub fn get_volatile_fields(kind: ResourceKind) -> Vec<&'static str> {
    match kind {
        ResourceKind::Index => hoist_core::resources::Index::volatile_fields().to_vec(),
        ResourceKind::Indexer => hoist_core::resources::Indexer::volatile_fields().to_vec(),
        ResourceKind::DataSource => hoist_core::resources::DataSource::volatile_fields().to_vec(),
        ResourceKind::Skillset => hoist_core::resources::Skillset::volatile_fields().to_vec(),
        ResourceKind::SynonymMap => hoist_core::resources::SynonymMap::volatile_fields().to_vec(),
        ResourceKind::KnowledgeBase => {
            hoist_core::resources::KnowledgeBase::volatile_fields().to_vec()
        }
        ResourceKind::KnowledgeSource => {
            hoist_core::resources::KnowledgeSource::volatile_fields().to_vec()
        }
    }
}

/// Get the read-only fields for a given resource kind.
/// These are kept in local files (informational) but stripped before pushing to Azure.
pub fn get_read_only_fields(kind: ResourceKind) -> Vec<&'static str> {
    match kind {
        ResourceKind::Index => hoist_core::resources::Index::read_only_fields().to_vec(),
        ResourceKind::Indexer => hoist_core::resources::Indexer::read_only_fields().to_vec(),
        ResourceKind::DataSource => hoist_core::resources::DataSource::read_only_fields().to_vec(),
        ResourceKind::Skillset => hoist_core::resources::Skillset::read_only_fields().to_vec(),
        ResourceKind::SynonymMap => hoist_core::resources::SynonymMap::read_only_fields().to_vec(),
        ResourceKind::KnowledgeBase => {
            hoist_core::resources::KnowledgeBase::read_only_fields().to_vec()
        }
        ResourceKind::KnowledgeSource => {
            hoist_core::resources::KnowledgeSource::read_only_fields().to_vec()
        }
    }
}

/// Order resources by dependencies (data sources before indexers, etc.)
pub fn order_by_dependencies(
    resources: &[(ResourceKind, String, serde_json::Value, bool)],
) -> Vec<(ResourceKind, String, serde_json::Value, bool)> {
    let order = [
        ResourceKind::SynonymMap,      // No dependencies
        ResourceKind::DataSource,      // No dependencies
        ResourceKind::Index,           // May depend on synonym maps
        ResourceKind::Skillset,        // No dependencies
        ResourceKind::KnowledgeBase,   // No dependencies
        ResourceKind::Indexer,         // Depends on data source, index, skillset
        ResourceKind::KnowledgeSource, // Depends on index, knowledge base
    ];

    let mut ordered = resources.to_vec();
    ordered
        .sort_by_key(|(kind, _, _, _)| order.iter().position(|k| k == kind).unwrap_or(usize::MAX));

    ordered
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // === resolve_resource_kinds tests ===

    #[test]
    fn test_all_with_preview() {
        let kinds = resolve_resource_kinds(
            true, false, false, false, false, false, false, false, true, false,
        );
        assert_eq!(kinds.len(), 7);
        assert!(kinds.contains(&ResourceKind::KnowledgeBase));
        assert!(kinds.contains(&ResourceKind::KnowledgeSource));
    }

    #[test]
    fn test_all_without_preview() {
        let kinds = resolve_resource_kinds(
            true, false, false, false, false, false, false, false, false, false,
        );
        assert_eq!(kinds.len(), 5);
        assert!(!kinds.contains(&ResourceKind::KnowledgeBase));
        assert!(!kinds.contains(&ResourceKind::KnowledgeSource));
    }

    #[test]
    fn test_specific_flags_override_default() {
        let kinds = resolve_resource_kinds(
            false, true, false, false, false, false, false, false, true, true,
        );
        assert_eq!(kinds, vec![ResourceKind::Index]);
    }

    #[test]
    fn test_no_flags_with_fallback_and_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, false, true, true,
        );
        assert_eq!(kinds.len(), 7);
    }

    #[test]
    fn test_no_flags_with_fallback_without_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, false, false, true,
        );
        assert_eq!(kinds, ResourceKind::stable().to_vec());
    }

    #[test]
    fn test_no_flags_without_fallback_returns_empty() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, false, false, true, false,
        );
        assert!(kinds.is_empty());
    }

    #[test]
    fn test_knowledge_flags_require_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, true, true, false, false,
        );
        // include_preview is false, so KB/KS flags are ignored
        assert!(kinds.is_empty());
    }

    #[test]
    fn test_knowledge_flags_with_preview() {
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, true, true, true, false,
        );
        assert_eq!(kinds.len(), 2);
        assert!(kinds.contains(&ResourceKind::KnowledgeBase));
        assert!(kinds.contains(&ResourceKind::KnowledgeSource));
    }

    #[test]
    fn test_knowledge_flags_ignored_falls_back_to_default() {
        // KB/KS flags set but include_preview=false, no other flags → falls back
        let kinds = resolve_resource_kinds(
            false, false, false, false, false, false, true, true, false, true,
        );
        assert_eq!(kinds, ResourceKind::stable().to_vec());
    }

    // === get_volatile_fields tests ===

    #[test]
    fn test_volatile_fields_always_include_etag() {
        for kind in ResourceKind::all() {
            let fields = get_volatile_fields(*kind);
            assert!(
                fields.contains(&"@odata.etag"),
                "{:?} missing @odata.etag",
                kind
            );
        }
    }

    #[test]
    fn test_volatile_fields_knowledge_base_strips_secrets() {
        let fields = get_volatile_fields(ResourceKind::KnowledgeBase);
        assert!(fields.contains(&"storageConnectionStringSecret"));
    }

    #[test]
    fn test_volatile_fields_datasource_strips_credentials() {
        let fields = get_volatile_fields(ResourceKind::DataSource);
        assert!(fields.contains(&"credentials"));
    }

    // === get_read_only_fields tests ===

    #[test]
    fn test_read_only_fields_kb_is_empty() {
        // knowledgeSources is a normal pushable field, not read-only
        let fields = get_read_only_fields(ResourceKind::KnowledgeBase);
        assert!(fields.is_empty());
    }

    #[test]
    fn test_read_only_fields_ks_has_created_resources() {
        let fields = get_read_only_fields(ResourceKind::KnowledgeSource);
        assert!(fields.contains(&"createdResources"));
        assert!(fields.contains(&"ingestionPermissionOptions"));
    }

    #[test]
    fn test_read_only_fields_indexer_has_start_time() {
        let fields = get_read_only_fields(ResourceKind::Indexer);
        assert!(fields.contains(&"startTime"));
    }

    #[test]
    fn test_read_only_fields_empty_for_most_types() {
        assert!(get_read_only_fields(ResourceKind::Index).is_empty());
        assert!(get_read_only_fields(ResourceKind::DataSource).is_empty());
        assert!(get_read_only_fields(ResourceKind::Skillset).is_empty());
        assert!(get_read_only_fields(ResourceKind::SynonymMap).is_empty());
    }

    // === order_by_dependencies tests ===

    #[test]
    fn test_order_datasource_before_indexer() {
        let resources = vec![
            (ResourceKind::Indexer, "ixer".to_string(), json!({}), false),
            (ResourceKind::DataSource, "ds".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        assert_eq!(ordered[0].0, ResourceKind::DataSource);
        assert_eq!(ordered[1].0, ResourceKind::Indexer);
    }

    #[test]
    fn test_order_index_before_indexer() {
        let resources = vec![
            (ResourceKind::Indexer, "ixer".to_string(), json!({}), false),
            (ResourceKind::Index, "idx".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        assert_eq!(ordered[0].0, ResourceKind::Index);
        assert_eq!(ordered[1].0, ResourceKind::Indexer);
    }

    #[test]
    fn test_order_knowledge_base_before_knowledge_source() {
        let resources = vec![
            (
                ResourceKind::KnowledgeSource,
                "ks".to_string(),
                json!({}),
                false,
            ),
            (
                ResourceKind::KnowledgeBase,
                "kb".to_string(),
                json!({}),
                false,
            ),
        ];
        let ordered = order_by_dependencies(&resources);
        assert_eq!(ordered[0].0, ResourceKind::KnowledgeBase);
        assert_eq!(ordered[1].0, ResourceKind::KnowledgeSource);
    }

    #[test]
    fn test_order_full_dependency_chain() {
        let resources = vec![
            (
                ResourceKind::KnowledgeSource,
                "ks".to_string(),
                json!({}),
                false,
            ),
            (ResourceKind::Indexer, "ixer".to_string(), json!({}), false),
            (ResourceKind::Index, "idx".to_string(), json!({}), false),
            (ResourceKind::DataSource, "ds".to_string(), json!({}), false),
            (
                ResourceKind::KnowledgeBase,
                "kb".to_string(),
                json!({}),
                false,
            ),
            (ResourceKind::Skillset, "sk".to_string(), json!({}), false),
            (ResourceKind::SynonymMap, "sm".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        let kinds: Vec<_> = ordered.iter().map(|(k, _, _, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ResourceKind::SynonymMap,
                ResourceKind::DataSource,
                ResourceKind::Index,
                ResourceKind::Skillset,
                ResourceKind::KnowledgeBase,
                ResourceKind::Indexer,
                ResourceKind::KnowledgeSource,
            ]
        );
    }

    #[test]
    fn test_order_empty() {
        let resources: Vec<(ResourceKind, String, serde_json::Value, bool)> = vec![];
        let ordered = order_by_dependencies(&resources);
        assert!(ordered.is_empty());
    }

    #[test]
    fn test_order_preserves_within_same_kind() {
        let resources = vec![
            (ResourceKind::Index, "b-index".to_string(), json!({}), false),
            (ResourceKind::Index, "a-index".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        // sort_by_key is stable, so same-kind order is preserved
        assert_eq!(ordered[0].1, "b-index");
        assert_eq!(ordered[1].1, "a-index");
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
            false, false, false, false, false, false, false, false, &singular, true, false,
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
            false, true, false, false, false, false, false, false, &singular, true, false,
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
            true, false, false, false, false, false, false, false, &singular, true, false,
        );

        assert_eq!(sel.kinds().len(), 7);
        // --all clears all name filters
        assert_eq!(sel.name_filter(ResourceKind::KnowledgeBase), None);
    }

    #[test]
    fn test_selection_name_filter() {
        let mut singular = no_singular();
        singular.index = Some("my-idx".to_string());
        singular.indexer = Some("my-ixer".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, &singular, true, false,
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
            false, true, false, false, false, false, false, false, &singular, true, false,
        );

        // Singular takes precedence — only one entry for Index
        assert_eq!(sel.kinds(), vec![ResourceKind::Index]);
        assert_eq!(sel.name_filter(ResourceKind::Index), Some("specific-idx"));
    }

    #[test]
    fn test_selection_preview_singular_requires_include_preview() {
        let mut singular = no_singular();
        singular.knowledgebase = Some("my-kb".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, &singular, false, false,
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
            false, false, false, false, false, false, false, false, &singular, false, false,
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
            &no_singular(),
            true,
            true,
        );
        // Falls back to all kinds (with preview)
        assert_eq!(sel.kinds().len(), 7);
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
            false, false, false, false, false, false, false, false, &singular, true, false,
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
        // A singular flag is set → should NOT fall back to all kinds
        let mut singular = no_singular();
        singular.index = Some("my-idx".to_string());

        let sel = resolve_resource_selection(
            false, false, false, false, false, false, false, false, &singular, true,
            true, // has_default_fallback=true
        );

        // Only Index, not all 7
        assert_eq!(sel.kinds(), vec![ResourceKind::Index]);
    }
}
