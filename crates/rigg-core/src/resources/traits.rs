//! Resource kind definitions for Azure AI Search and Microsoft Foundry resources.
//!
//! Per-kind metadata (API paths, directory names, volatile/read-only/secret
//! fields, references, capabilities) lives in [`crate::registry`]; the methods
//! on [`ResourceKind`] delegate there.

use serde::{Deserialize, Serialize};
use std::fmt;

use crate::registry::{self, Domain};
use crate::service::ServiceDomain;

/// Enumeration of all supported resource types across service domains
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceKind {
    // Azure AI Search resources
    DataSource,
    Index,
    Skillset,
    Indexer,
    SynonymMap,
    Alias,
    KnowledgeSource,
    KnowledgeBase,
    // Microsoft Foundry resources
    Agent,
    Deployment,
    Connection,
    Guardrail,
}

impl ResourceKind {
    /// Returns the service domain this resource belongs to
    pub fn domain(&self) -> ServiceDomain {
        match registry::meta(*self).domain {
            Domain::Search => ServiceDomain::Search,
            Domain::FoundryData | Domain::FoundryArm => ServiceDomain::Foundry,
        }
    }

    /// Returns the API collection path segment for this resource type
    pub fn api_path(&self) -> &'static str {
        registry::meta(*self).collection_path
    }

    /// Returns the directory name for local storage, relative to the
    /// project's domain directory (`search/` or `foundry/`).
    pub fn directory_name(&self) -> &'static str {
        registry::meta(*self).dir_name
    }

    /// Parse a directory name back to a kind.
    pub fn from_directory_name(dir: &str) -> Option<ResourceKind> {
        Self::all()
            .iter()
            .find(|k| k.directory_name() == dir)
            .copied()
    }

    /// Returns the CLI name for this resource type (`rigg new <kind>`).
    pub fn cli_name(&self) -> &'static str {
        match self {
            ResourceKind::DataSource => "data-source",
            ResourceKind::Index => "index",
            ResourceKind::Skillset => "skillset",
            ResourceKind::Indexer => "indexer",
            ResourceKind::SynonymMap => "synonym-map",
            ResourceKind::Alias => "alias",
            ResourceKind::KnowledgeSource => "knowledge-source",
            ResourceKind::KnowledgeBase => "knowledge-base",
            ResourceKind::Agent => "agent",
            ResourceKind::Deployment => "deployment",
            ResourceKind::Connection => "connection",
            ResourceKind::Guardrail => "guardrail",
        }
    }

    /// Parse a CLI name (as used by `rigg new`) into a kind.
    pub fn from_cli_name(s: &str) -> Option<ResourceKind> {
        Self::all().iter().find(|k| k.cli_name() == s).copied()
    }

    /// Returns the display name for this resource type
    pub fn display_name(&self) -> &'static str {
        match self {
            ResourceKind::DataSource => "Data Source",
            ResourceKind::Index => "Index",
            ResourceKind::Skillset => "Skillset",
            ResourceKind::Indexer => "Indexer",
            ResourceKind::SynonymMap => "Synonym Map",
            ResourceKind::Alias => "Alias",
            ResourceKind::KnowledgeSource => "Knowledge Source",
            ResourceKind::KnowledgeBase => "Knowledge Base",
            ResourceKind::Agent => "Agent",
            ResourceKind::Deployment => "Model Deployment",
            ResourceKind::Connection => "Connection",
            ResourceKind::Guardrail => "Guardrail",
        }
    }

    /// Returns all resource kinds across all domains
    pub fn all() -> &'static [ResourceKind] {
        registry::all_kinds()
    }

    /// Legacy shim (pre-0.18 semantics): core search-management kinds.
    /// Retired together with the old command implementations.
    pub fn stable() -> &'static [ResourceKind] {
        &[
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
        ]
    }

    /// Legacy shim: old singular flag name. Retired with the old CLI.
    pub fn cli_flag_name(&self) -> &'static str {
        match self {
            ResourceKind::DataSource => "datasource",
            ResourceKind::SynonymMap => "synonymmap",
            ResourceKind::KnowledgeBase => "knowledgebase",
            ResourceKind::KnowledgeSource => "knowledgesource",
            other => other.cli_name(),
        }
    }

    /// Legacy shim: old plural flag name. Retired with the old CLI.
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
            ResourceKind::Deployment => "deployments",
            ResourceKind::Connection => "connections",
            ResourceKind::Guardrail => "guardrails",
        }
    }

    /// Returns all Search resource kinds
    pub fn search_kinds() -> Vec<ResourceKind> {
        Self::all()
            .iter()
            .filter(|k| k.domain() == ServiceDomain::Search)
            .copied()
            .collect()
    }

    /// Returns all Foundry resource kinds
    pub fn foundry_kinds() -> Vec<ResourceKind> {
        Self::all()
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

/// A (kind, name) reference to a resource.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResourceRef {
    pub kind: ResourceKind,
    pub name: String,
}

impl ResourceRef {
    pub fn new(kind: ResourceKind, name: impl Into<String>) -> Self {
        ResourceRef {
            kind,
            name: name.into(),
        }
    }

    /// Stable string key, e.g. `indexes/my-index` — used in state files.
    pub fn key(&self) -> String {
        format!("{}/{}", self.kind.directory_name(), self.name)
    }
}

impl fmt::Display for ResourceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.kind.directory_name(), self.name)
    }
}

/// Legacy typed-resource trait. The registry (`crate::registry`) is the new
/// source of truth for per-kind metadata; this trait remains only while the
/// old typed resource modules and their consumers are being retired during
/// the 0.18 rewrite.
pub trait Resource: Serialize + for<'de> Deserialize<'de> + Clone {
    fn kind() -> ResourceKind;
    fn name(&self) -> &str;
    fn volatile_fields() -> &'static [&'static str] {
        &["@odata.etag", "@odata.context"]
    }
    fn read_only_fields() -> &'static [&'static str] {
        &[]
    }
    fn identity_key() -> &'static str {
        "name"
    }
    fn dependencies(&self) -> Vec<(ResourceKind, String)> {
        Vec::new()
    }
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
    fn all_returns_twelve_kinds() {
        assert_eq!(ResourceKind::all().len(), 12);
    }

    #[test]
    fn api_paths() {
        assert_eq!(ResourceKind::Index.api_path(), "indexes");
        assert_eq!(ResourceKind::DataSource.api_path(), "datasources");
        assert_eq!(ResourceKind::KnowledgeBase.api_path(), "knowledgeBases");
        assert_eq!(ResourceKind::KnowledgeSource.api_path(), "knowledgeSources");
        assert_eq!(ResourceKind::Agent.api_path(), "agents");
        assert_eq!(ResourceKind::Deployment.api_path(), "deployments");
    }

    #[test]
    fn directory_names_are_flat_and_roundtrip() {
        for kind in ResourceKind::all() {
            let dir = kind.directory_name();
            assert!(!dir.contains('/'), "{kind:?} dir must be flat: {dir}");
            assert_eq!(ResourceKind::from_directory_name(dir), Some(*kind));
        }
        assert_eq!(
            ResourceKind::KnowledgeSource.directory_name(),
            "knowledge-sources"
        );
        assert_eq!(ResourceKind::Guardrail.directory_name(), "guardrails");
    }

    #[test]
    fn cli_names_roundtrip() {
        for kind in ResourceKind::all() {
            assert_eq!(ResourceKind::from_cli_name(kind.cli_name()), Some(*kind));
        }
    }

    #[test]
    fn domains() {
        assert_eq!(ResourceKind::Index.domain(), ServiceDomain::Search);
        assert_eq!(ResourceKind::KnowledgeBase.domain(), ServiceDomain::Search);
        assert_eq!(ResourceKind::Agent.domain(), ServiceDomain::Foundry);
        assert_eq!(ResourceKind::Deployment.domain(), ServiceDomain::Foundry);
        assert_eq!(ResourceKind::Connection.domain(), ServiceDomain::Foundry);
        assert_eq!(ResourceKind::Guardrail.domain(), ServiceDomain::Foundry);
        assert_eq!(ResourceKind::search_kinds().len(), 8);
        assert_eq!(ResourceKind::foundry_kinds().len(), 4);
    }

    #[test]
    fn resource_ref_key() {
        let r = ResourceRef::new(ResourceKind::Index, "docs");
        assert_eq!(r.key(), "indexes/docs");
        assert_eq!(r.to_string(), "indexes/docs");
    }

    #[test]
    fn serde_roundtrip() {
        for kind in ResourceKind::all() {
            let json = serde_json::to_string(kind).unwrap();
            let back: ResourceKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, *kind);
        }
        assert_eq!(
            serde_json::to_string(&ResourceKind::DataSource).unwrap(),
            "\"data-source\""
        );
    }

    #[test]
    fn validate_resource_name_rules() {
        assert!(validate_resource_name("my-index").is_ok());
        assert!(validate_resource_name(&"a".repeat(260)).is_ok());
        assert!(validate_resource_name("").is_err());
        assert!(validate_resource_name(&"a".repeat(261)).is_err());
        assert!(validate_resource_name("foo/bar").is_err());
        assert!(validate_resource_name("foo\\bar").is_err());
        assert!(validate_resource_name("foo\0bar").is_err());
        assert!(validate_resource_name(".").is_err());
        assert!(validate_resource_name("..").is_err());
    }
}
