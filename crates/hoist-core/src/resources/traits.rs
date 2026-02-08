//! Resource trait definition for Azure AI Search resources

use serde::{Deserialize, Serialize};
use std::fmt;

/// Enumeration of all supported Azure AI Search resource types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    Index,
    Indexer,
    DataSource,
    Skillset,
    SynonymMap,
    Alias,
    KnowledgeBase,
    KnowledgeSource,
}

impl ResourceKind {
    /// Returns the API path segment for this resource type
    pub fn api_path(&self) -> &'static str {
        match self {
            ResourceKind::Index => "indexes",
            ResourceKind::Indexer => "indexers",
            ResourceKind::DataSource => "datasources",
            ResourceKind::Skillset => "skillsets",
            ResourceKind::SynonymMap => "synonymmaps",
            ResourceKind::Alias => "aliases",
            ResourceKind::KnowledgeBase => "knowledgebases",
            ResourceKind::KnowledgeSource => "knowledgesources",
        }
    }

    /// Returns the directory path for local storage (relative to resource root)
    pub fn directory_name(&self) -> &'static str {
        match self {
            ResourceKind::Index => "search-management/indexes",
            ResourceKind::Indexer => "search-management/indexers",
            ResourceKind::DataSource => "search-management/data-sources",
            ResourceKind::Skillset => "search-management/skillsets",
            ResourceKind::SynonymMap => "search-management/synonym-maps",
            ResourceKind::Alias => "search-management/aliases",
            ResourceKind::KnowledgeBase => "agentic-retrieval/knowledge-bases",
            ResourceKind::KnowledgeSource => "agentic-retrieval/knowledge-sources",
        }
    }

    /// Returns true if this resource type uses the preview API
    pub fn is_preview(&self) -> bool {
        matches!(
            self,
            ResourceKind::KnowledgeBase | ResourceKind::KnowledgeSource
        )
    }

    /// Returns the display name for this resource type
    pub fn display_name(&self) -> &'static str {
        match self {
            ResourceKind::Index => "Index",
            ResourceKind::Indexer => "Indexer",
            ResourceKind::DataSource => "Data Source",
            ResourceKind::Skillset => "Skillset",
            ResourceKind::SynonymMap => "Synonym Map",
            ResourceKind::Alias => "Alias",
            ResourceKind::KnowledgeBase => "Knowledge Base",
            ResourceKind::KnowledgeSource => "Knowledge Source",
        }
    }

    /// Returns all resource kinds
    pub fn all() -> &'static [ResourceKind] {
        &[
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
            ResourceKind::Alias,
            ResourceKind::KnowledgeBase,
            ResourceKind::KnowledgeSource,
        ]
    }

    /// Returns non-preview resource kinds
    pub fn stable() -> &'static [ResourceKind] {
        &[
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
            ResourceKind::Alias,
        ]
    }
}

impl fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Trait for Azure AI Search resources
pub trait Resource: Serialize + for<'de> Deserialize<'de> + Clone {
    /// Returns the resource kind
    fn kind() -> ResourceKind;

    /// Returns the resource name (identifier)
    fn name(&self) -> &str;

    /// Returns fields that should be stripped during normalization (pull and push).
    /// These are truly transient or sensitive: OData metadata, secrets, credentials.
    fn volatile_fields() -> &'static [&'static str] {
        &["@odata.etag", "@odata.context"]
    }

    /// Returns fields that are read-only — Azure returns them in GET but rejects
    /// them in PUT. These are kept in local files for documentation (e.g. showing
    /// which resources are connected) but stripped before pushing to Azure.
    fn read_only_fields() -> &'static [&'static str] {
        &[]
    }

    /// Returns the identity key for array sorting within this resource type
    fn identity_key() -> &'static str {
        "name"
    }

    /// Returns dependencies on other resources (resource kind, name)
    fn dependencies(&self) -> Vec<(ResourceKind, String)> {
        Vec::new()
    }

    /// Returns fields that are immutable after creation
    fn immutable_fields() -> &'static [&'static str] {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_returns_eight_kinds() {
        assert_eq!(ResourceKind::all().len(), 8);
    }

    #[test]
    fn test_stable_excludes_preview() {
        let stable = ResourceKind::stable();
        assert_eq!(stable.len(), 6);
        for kind in stable {
            assert!(!kind.is_preview());
        }
    }

    #[test]
    fn test_preview_kinds() {
        assert!(ResourceKind::KnowledgeBase.is_preview());
        assert!(ResourceKind::KnowledgeSource.is_preview());
        assert!(!ResourceKind::Index.is_preview());
        assert!(!ResourceKind::Indexer.is_preview());
        assert!(!ResourceKind::DataSource.is_preview());
        assert!(!ResourceKind::Skillset.is_preview());
        assert!(!ResourceKind::SynonymMap.is_preview());
        assert!(!ResourceKind::Alias.is_preview());
    }

    #[test]
    fn test_api_paths() {
        assert_eq!(ResourceKind::Index.api_path(), "indexes");
        assert_eq!(ResourceKind::Indexer.api_path(), "indexers");
        assert_eq!(ResourceKind::DataSource.api_path(), "datasources");
        assert_eq!(ResourceKind::Skillset.api_path(), "skillsets");
        assert_eq!(ResourceKind::SynonymMap.api_path(), "synonymmaps");
        assert_eq!(ResourceKind::Alias.api_path(), "aliases");
        assert_eq!(ResourceKind::KnowledgeBase.api_path(), "knowledgebases");
        assert_eq!(ResourceKind::KnowledgeSource.api_path(), "knowledgesources");
    }

    #[test]
    fn test_directory_names() {
        assert_eq!(
            ResourceKind::Index.directory_name(),
            "search-management/indexes"
        );
        assert_eq!(
            ResourceKind::DataSource.directory_name(),
            "search-management/data-sources"
        );
        assert_eq!(
            ResourceKind::SynonymMap.directory_name(),
            "search-management/synonym-maps"
        );
        assert_eq!(
            ResourceKind::Alias.directory_name(),
            "search-management/aliases"
        );
        assert_eq!(
            ResourceKind::KnowledgeBase.directory_name(),
            "agentic-retrieval/knowledge-bases"
        );
        assert_eq!(
            ResourceKind::KnowledgeSource.directory_name(),
            "agentic-retrieval/knowledge-sources"
        );
    }

    #[test]
    fn test_stable_kinds_under_search_management() {
        for kind in ResourceKind::stable() {
            assert!(
                kind.directory_name().starts_with("search-management/"),
                "{:?} should be under search-management/",
                kind
            );
        }
    }

    #[test]
    fn test_preview_kinds_under_agentic_retrieval() {
        for kind in ResourceKind::all() {
            if kind.is_preview() {
                assert!(
                    kind.directory_name().starts_with("agentic-retrieval/"),
                    "{:?} should be under agentic-retrieval/",
                    kind
                );
            }
        }
    }

    #[test]
    fn test_display_names() {
        assert_eq!(ResourceKind::Index.display_name(), "Index");
        assert_eq!(ResourceKind::DataSource.display_name(), "Data Source");
        assert_eq!(ResourceKind::KnowledgeBase.display_name(), "Knowledge Base");
        assert_eq!(ResourceKind::Alias.display_name(), "Alias");
    }

    #[test]
    fn test_display_trait() {
        assert_eq!(format!("{}", ResourceKind::Index), "Index");
        assert_eq!(format!("{}", ResourceKind::Skillset), "Skillset");
        assert_eq!(format!("{}", ResourceKind::Alias), "Alias");
    }

    #[test]
    fn test_serde_roundtrip() {
        let kind = ResourceKind::DataSource;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"data-source\"");
        let back: ResourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn test_serde_roundtrip_alias() {
        let kind = ResourceKind::Alias;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"alias\"");
        let back: ResourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn test_all_kinds_in_stable_or_preview() {
        for kind in ResourceKind::all() {
            if kind.is_preview() {
                assert!(!ResourceKind::stable().contains(kind));
            } else {
                assert!(ResourceKind::stable().contains(kind));
            }
        }
    }
}
