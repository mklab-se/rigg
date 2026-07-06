//! Resource field metadata helpers.
//!
//! Provides volatile and read-only field lists for each `ResourceKind`,
//! used during JSON normalization on pull and field stripping before push.

use rigg_core::resources::ResourceKind;

/// Get the volatile fields to strip for a given resource kind during normalization.
/// These are stripped from local files during pull AND before push.
pub fn get_volatile_fields(kind: ResourceKind) -> Vec<&'static str> {
    rigg_core::registry::meta(kind).volatile_fields.to_vec()
}

/// Get the read-only fields for a given resource kind.
/// These are never written to local files and are stripped before pushing to Azure.
pub fn get_read_only_fields(kind: ResourceKind) -> Vec<&'static str> {
    rigg_core::registry::meta(kind).read_only_fields.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    // === get_volatile_fields tests ===

    #[test]
    fn test_volatile_fields_search_resources_include_etag() {
        for kind in ResourceKind::search_kinds() {
            let fields = get_volatile_fields(kind);
            assert!(
                fields.contains(&"@odata.etag"),
                "{:?} missing @odata.etag",
                kind
            );
        }
    }

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
    fn test_read_only_fields_empty_for_most_types() {
        assert!(get_read_only_fields(ResourceKind::Index).is_empty());
        assert!(get_read_only_fields(ResourceKind::DataSource).is_empty());
        assert!(get_read_only_fields(ResourceKind::Skillset).is_empty());
        assert!(get_read_only_fields(ResourceKind::SynonymMap).is_empty());
        assert!(get_read_only_fields(ResourceKind::Alias).is_empty());
    }
}
