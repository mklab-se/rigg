//! Resource trait definition for Azure AI Search and Microsoft Foundry resources

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::service::ServiceDomain;

/// Enumeration of all supported resource types across service domains
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    // Azure AI Search resources
    Index,
    Indexer,
    DataSource,
    Skillset,
    SynonymMap,
    Alias,
    KnowledgeBase,
    KnowledgeSource,
    // Microsoft Foundry resources
    Agent,
}

impl ResourceKind {
    /// Returns the service domain this resource belongs to
    pub fn domain(&self) -> ServiceDomain {
        match self {
            ResourceKind::Agent => ServiceDomain::Foundry,
            _ => ServiceDomain::Search,
        }
    }

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
            ResourceKind::Agent => "assistants",
        }
    }

    /// Returns the directory path for local storage (relative to service root).
    ///
    /// Search resources are organized under category prefixes:
    /// - `search-management/` for core search resources
    /// - `agentic-retrieval/` for knowledge bases and knowledge sources
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
            ResourceKind::Agent => "agents",
        }
    }

    /// Returns true if this resource type uses the preview API
    pub fn is_preview(&self) -> bool {
        matches!(
            self,
            ResourceKind::Alias | ResourceKind::KnowledgeBase | ResourceKind::KnowledgeSource
        )
    }

    /// Returns the singular CLI flag name for this resource type.
    ///
    /// Used in hint messages like `hoist delete --index my-index --target local`.
    pub fn cli_flag_name(&self) -> &'static str {
        match self {
            ResourceKind::Index => "index",
            ResourceKind::Indexer => "indexer",
            ResourceKind::DataSource => "datasource",
            ResourceKind::Skillset => "skillset",
            ResourceKind::SynonymMap => "synonymmap",
            ResourceKind::Alias => "alias",
            ResourceKind::KnowledgeBase => "knowledgebase",
            ResourceKind::KnowledgeSource => "knowledgesource",
            ResourceKind::Agent => "agent",
        }
    }

    /// Returns the plural CLI flag name for this resource type.
    ///
    /// Used in hint messages like `hoist pull --indexes`.
    pub fn cli_flag_name_plural(&self) -> &'static str {
        match self {
            ResourceKind::Index => "indexes",
            ResourceKind::Indexer => "indexers",
            ResourceKind::DataSource => "datasources",
            ResourceKind::Skillset => "skillsets",
            ResourceKind::SynonymMap => "synonymmaps",
            ResourceKind::Alias => "aliases",
            ResourceKind::KnowledgeBase => "knowledgebases",
            ResourceKind::KnowledgeSource => "knowledgesources",
            ResourceKind::Agent => "agents",
        }
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
            ResourceKind::Agent => "Agent",
        }
    }

    /// Returns all resource kinds across all domains
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
            ResourceKind::Agent,
        ]
    }

    /// Returns non-preview Search resource kinds (stable search resources only)
    pub fn stable() -> &'static [ResourceKind] {
        &[
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
        ]
    }

    /// Returns all Search resource kinds
    pub fn search_kinds() -> Vec<ResourceKind> {
        ResourceKind::all()
            .iter()
            .filter(|k| k.domain() == ServiceDomain::Search)
            .copied()
            .collect()
    }

    /// Returns all Foundry resource kinds
    pub fn foundry_kinds() -> Vec<ResourceKind> {
        ResourceKind::all()
            .iter()
            .filter(|k| k.domain() == ServiceDomain::Foundry)
            .copied()
            .collect()
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

/// Validate a resource name returned from Azure API responses.
///
/// Rejects:
/// - Empty names
/// - Names longer than 260 characters
/// - Names containing `/`, `\`, or null bytes
/// - Names that are exactly `.` or `..`
pub fn validate_resource_name(name: &str) -> Result<(), anyhow::Error> {
    if name.is_empty() {
        anyhow::bail!("Resource name must not be empty");
    }
    if name.len() > 260 {
        anyhow::bail!(
            "Resource name must not exceed 260 characters (got {})",
            name.len()
        );
    }
    if name.contains('/') || name.contains('\\') || name.contains('\0') {
        anyhow::bail!(
            "Resource name must not contain '/', '\\', or null bytes: '{}'",
            name
        );
    }
    if name == "." || name == ".." {
        anyhow::bail!("Resource name must not be '.' or '..'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_returns_nine_kinds() {
        assert_eq!(ResourceKind::all().len(), 9);
    }

    #[test]
    fn test_stable_excludes_preview() {
        let stable = ResourceKind::stable();
        assert_eq!(stable.len(), 5);
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
        assert!(ResourceKind::Alias.is_preview());
        assert!(!ResourceKind::Agent.is_preview());
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
        assert_eq!(ResourceKind::Agent.api_path(), "assistants");
    }

    #[test]
    fn test_directory_names() {
        assert_eq!(
            ResourceKind::Index.directory_name(),
            "search-management/indexes"
        );
        assert_eq!(
            ResourceKind::Indexer.directory_name(),
            "search-management/indexers"
        );
        assert_eq!(
            ResourceKind::DataSource.directory_name(),
            "search-management/data-sources"
        );
        assert_eq!(
            ResourceKind::Skillset.directory_name(),
            "search-management/skillsets"
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
        assert_eq!(ResourceKind::Agent.directory_name(), "agents");
    }

    #[test]
    fn test_directory_names_categorized() {
        // Search resources are organized under category prefixes
        let agentic_kinds = [ResourceKind::KnowledgeBase, ResourceKind::KnowledgeSource];
        for kind in ResourceKind::search_kinds() {
            if agentic_kinds.contains(&kind) {
                assert!(
                    kind.directory_name().starts_with("agentic-retrieval/"),
                    "{:?} should be under agentic-retrieval/, got: {}",
                    kind,
                    kind.directory_name()
                );
            } else {
                assert!(
                    kind.directory_name().starts_with("search-management/"),
                    "{:?} should be under search-management/, got: {}",
                    kind,
                    kind.directory_name()
                );
            }
        }
        // Foundry agents are flat
        assert_eq!(ResourceKind::Agent.directory_name(), "agents");
    }

    #[test]
    fn test_display_names() {
        assert_eq!(ResourceKind::Index.display_name(), "Index");
        assert_eq!(ResourceKind::DataSource.display_name(), "Data Source");
        assert_eq!(ResourceKind::KnowledgeBase.display_name(), "Knowledge Base");
        assert_eq!(ResourceKind::Alias.display_name(), "Alias");
        assert_eq!(ResourceKind::Agent.display_name(), "Agent");
    }

    #[test]
    fn test_display_trait() {
        assert_eq!(format!("{}", ResourceKind::Index), "Index");
        assert_eq!(format!("{}", ResourceKind::Skillset), "Skillset");
        assert_eq!(format!("{}", ResourceKind::Alias), "Alias");
        assert_eq!(format!("{}", ResourceKind::Agent), "Agent");
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
    fn test_serde_roundtrip_agent() {
        let kind = ResourceKind::Agent;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, "\"agent\"");
        let back: ResourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(back, kind);
    }

    #[test]
    fn test_cli_flag_names() {
        assert_eq!(ResourceKind::Index.cli_flag_name(), "index");
        assert_eq!(ResourceKind::Indexer.cli_flag_name(), "indexer");
        assert_eq!(ResourceKind::DataSource.cli_flag_name(), "datasource");
        assert_eq!(ResourceKind::Skillset.cli_flag_name(), "skillset");
        assert_eq!(ResourceKind::SynonymMap.cli_flag_name(), "synonymmap");
        assert_eq!(ResourceKind::Alias.cli_flag_name(), "alias");
        assert_eq!(ResourceKind::KnowledgeBase.cli_flag_name(), "knowledgebase");
        assert_eq!(
            ResourceKind::KnowledgeSource.cli_flag_name(),
            "knowledgesource"
        );
        assert_eq!(ResourceKind::Agent.cli_flag_name(), "agent");
    }

    #[test]
    fn test_cli_flag_names_plural() {
        assert_eq!(ResourceKind::Index.cli_flag_name_plural(), "indexes");
        assert_eq!(ResourceKind::Alias.cli_flag_name_plural(), "aliases");
        assert_eq!(ResourceKind::Agent.cli_flag_name_plural(), "agents");
    }

    #[test]
    fn test_all_kinds_in_stable_or_preview() {
        for kind in ResourceKind::all() {
            if kind.domain() == ServiceDomain::Search {
                if kind.is_preview() {
                    assert!(!ResourceKind::stable().contains(kind));
                } else {
                    assert!(ResourceKind::stable().contains(kind));
                }
            }
        }
    }

    #[test]
    fn test_domain_search_kinds() {
        let search = ResourceKind::search_kinds();
        assert_eq!(search.len(), 8);
        for kind in &search {
            assert_eq!(kind.domain(), ServiceDomain::Search);
        }
    }

    #[test]
    fn test_domain_foundry_kinds() {
        let foundry = ResourceKind::foundry_kinds();
        assert_eq!(foundry.len(), 1);
        assert_eq!(foundry[0], ResourceKind::Agent);
        for kind in &foundry {
            assert_eq!(kind.domain(), ServiceDomain::Foundry);
        }
    }

    #[test]
    fn test_agent_domain_is_foundry() {
        assert_eq!(ResourceKind::Agent.domain(), ServiceDomain::Foundry);
    }

    #[test]
    fn test_search_resources_domain_is_search() {
        for kind in ResourceKind::stable() {
            assert_eq!(kind.domain(), ServiceDomain::Search);
        }
        assert_eq!(ResourceKind::Alias.domain(), ServiceDomain::Search);
        assert_eq!(ResourceKind::KnowledgeBase.domain(), ServiceDomain::Search);
        assert_eq!(
            ResourceKind::KnowledgeSource.domain(),
            ServiceDomain::Search
        );
    }

    #[test]
    fn test_search_plus_foundry_equals_all() {
        let mut combined = ResourceKind::search_kinds();
        combined.extend(ResourceKind::foundry_kinds());
        assert_eq!(combined.len(), ResourceKind::all().len());
        for kind in ResourceKind::all() {
            assert!(combined.contains(kind));
        }
    }

    #[test]
    fn test_validate_resource_name_valid() {
        assert!(validate_resource_name("my-index").is_ok());
        assert!(validate_resource_name("my_index_123").is_ok());
        assert!(validate_resource_name("a").is_ok());
        assert!(validate_resource_name("index.v2").is_ok());
        assert!(validate_resource_name("...").is_ok());
    }

    #[test]
    fn test_validate_resource_name_empty() {
        let err = validate_resource_name("").unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn test_validate_resource_name_too_long() {
        let long_name = "a".repeat(261);
        let err = validate_resource_name(&long_name).unwrap_err();
        assert!(err.to_string().contains("260"));
    }

    #[test]
    fn test_validate_resource_name_exactly_260_ok() {
        let name = "a".repeat(260);
        assert!(validate_resource_name(&name).is_ok());
    }

    #[test]
    fn test_validate_resource_name_forward_slash() {
        let err = validate_resource_name("foo/bar").unwrap_err();
        assert!(err.to_string().contains("/"));
    }

    #[test]
    fn test_validate_resource_name_backslash() {
        let err = validate_resource_name("foo\\bar").unwrap_err();
        assert!(err.to_string().contains("\\"));
    }

    #[test]
    fn test_validate_resource_name_null_byte() {
        let err = validate_resource_name("foo\0bar").unwrap_err();
        assert!(err.to_string().contains("null"));
    }

    #[test]
    fn test_validate_resource_name_dot() {
        let err = validate_resource_name(".").unwrap_err();
        assert!(err.to_string().contains("'.'"));
    }

    #[test]
    fn test_validate_resource_name_dotdot() {
        let err = validate_resource_name("..").unwrap_err();
        assert!(err.to_string().contains("'..'"));
    }
}
