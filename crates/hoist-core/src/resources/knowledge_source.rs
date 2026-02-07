//! Knowledge Source resource definition (Agentic Search Preview)

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Knowledge Source definition (Preview API)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSource {
    pub name: String,
    pub index_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge_base_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic_configuration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub select_fields: Option<Vec<String>>,
    /// Catch-all for additional fields from preview API
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

impl Resource for KnowledgeSource {
    fn kind() -> ResourceKind {
        ResourceKind::KnowledgeSource
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn volatile_fields() -> &'static [&'static str] {
        &["@odata.etag", "@odata.context"]
    }

    fn read_only_fields() -> &'static [&'static str] {
        // ingestionPermissionOptions — Azure returns it in GET but rejects it in PUT.
        // createdResources — Azure tracks which resources were auto-created (nested in azureBlobParameters).
        // Both kept in local files to show resource relationships, stripped before push.
        &["ingestionPermissionOptions", "createdResources"]
    }

    fn dependencies(&self) -> Vec<(ResourceKind, String)> {
        let mut deps = vec![(ResourceKind::Index, self.index_name.clone())];
        if let Some(ref kb) = self.knowledge_base_name {
            deps.push((ResourceKind::KnowledgeBase, kb.clone()));
        }
        deps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knowledge_source_kind() {
        assert_eq!(KnowledgeSource::kind(), ResourceKind::KnowledgeSource);
    }

    #[test]
    fn test_knowledge_source_dependencies_without_kb() {
        let ks = KnowledgeSource {
            name: "ks1".to_string(),
            index_name: "idx".to_string(),
            description: None,
            knowledge_base_name: None,
            query_type: None,
            semantic_configuration: None,
            top: None,
            filter: None,
            select_fields: None,
            extra: Default::default(),
        };
        let deps = ks.dependencies();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0], (ResourceKind::Index, "idx".to_string()));
    }

    #[test]
    fn test_knowledge_source_volatile_fields() {
        let fields = KnowledgeSource::volatile_fields();
        assert!(fields.contains(&"@odata.etag"));
        assert!(fields.contains(&"@odata.context"));
        // These should NOT be volatile — they're informational
        assert!(!fields.contains(&"ingestionPermissionOptions"));
        assert!(!fields.contains(&"createdResources"));
    }

    #[test]
    fn test_knowledge_source_read_only_fields() {
        let fields = KnowledgeSource::read_only_fields();
        assert!(
            fields.contains(&"ingestionPermissionOptions"),
            "ingestionPermissionOptions is read-only: kept in local, stripped before push"
        );
        assert!(
            fields.contains(&"createdResources"),
            "createdResources is read-only: kept in local, stripped before push"
        );
    }

    #[test]
    fn test_knowledge_source_dependencies_with_kb() {
        let ks = KnowledgeSource {
            name: "ks1".to_string(),
            index_name: "idx".to_string(),
            description: None,
            knowledge_base_name: Some("kb1".to_string()),
            query_type: None,
            semantic_configuration: None,
            top: None,
            filter: None,
            select_fields: None,
            extra: Default::default(),
        };
        let deps = ks.dependencies();
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&(ResourceKind::KnowledgeBase, "kb1".to_string())));
    }
}
