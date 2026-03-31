//! Resource field metadata helpers.
//!
//! Provides volatile and read-only field lists for each `ResourceKind`,
//! used during JSON normalization on pull and field stripping before push.

use rigg_core::resources::{Resource, ResourceKind};

/// Get the volatile fields to strip for a given resource kind during normalization.
/// These are stripped from local files during pull AND before push.
pub fn get_volatile_fields(kind: ResourceKind) -> Vec<&'static str> {
    match kind {
        ResourceKind::Index => rigg_core::resources::Index::volatile_fields().to_vec(),
        ResourceKind::Indexer => rigg_core::resources::Indexer::volatile_fields().to_vec(),
        ResourceKind::DataSource => rigg_core::resources::DataSource::volatile_fields().to_vec(),
        ResourceKind::Skillset => rigg_core::resources::Skillset::volatile_fields().to_vec(),
        ResourceKind::SynonymMap => rigg_core::resources::SynonymMap::volatile_fields().to_vec(),
        ResourceKind::Alias => rigg_core::resources::Alias::volatile_fields().to_vec(),
        ResourceKind::KnowledgeBase => {
            rigg_core::resources::KnowledgeBase::volatile_fields().to_vec()
        }
        ResourceKind::KnowledgeSource => {
            rigg_core::resources::KnowledgeSource::volatile_fields().to_vec()
        }
        ResourceKind::Agent => vec!["created_at", "object"],
    }
}

/// Get the read-only fields for a given resource kind.
/// These are kept in local files (informational) but stripped before pushing to Azure.
pub fn get_read_only_fields(kind: ResourceKind) -> Vec<&'static str> {
    match kind {
        ResourceKind::Index => rigg_core::resources::Index::read_only_fields().to_vec(),
        ResourceKind::Indexer => rigg_core::resources::Indexer::read_only_fields().to_vec(),
        ResourceKind::DataSource => rigg_core::resources::DataSource::read_only_fields().to_vec(),
        ResourceKind::Skillset => rigg_core::resources::Skillset::read_only_fields().to_vec(),
        ResourceKind::SynonymMap => rigg_core::resources::SynonymMap::read_only_fields().to_vec(),
        ResourceKind::Alias => rigg_core::resources::Alias::read_only_fields().to_vec(),
        ResourceKind::KnowledgeBase => {
            rigg_core::resources::KnowledgeBase::read_only_fields().to_vec()
        }
        ResourceKind::KnowledgeSource => {
            rigg_core::resources::KnowledgeSource::read_only_fields().to_vec()
        }
        ResourceKind::Agent => vec![],
    }
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
    fn test_volatile_fields_agent_has_created_at() {
        let fields = get_volatile_fields(ResourceKind::Agent);
        assert!(fields.contains(&"created_at"));
        assert!(fields.contains(&"object"));
        assert!(!fields.contains(&"@odata.etag"));
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
        assert!(get_read_only_fields(ResourceKind::Alias).is_empty());
    }
}
