//! Knowledge Base resource definition (Agentic Search Preview)

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Knowledge Base definition (Preview API)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBase {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_connection_string_secret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_container: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Value>,
    /// Catch-all for additional fields from preview API
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

impl Resource for KnowledgeBase {
    fn kind() -> ResourceKind {
        ResourceKind::KnowledgeBase
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn volatile_fields() -> &'static [&'static str] {
        &[
            "@odata.etag",
            "@odata.context",
            "storageConnectionStringSecret",
        ]
    }

    // knowledgeSources is a normal pushable field — you can add/remove knowledge sources
    // from a KB via PUT. It is NOT read-only despite being managed in the portal UI.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_knowledge_base_kind() {
        assert_eq!(KnowledgeBase::kind(), ResourceKind::KnowledgeBase);
    }

    #[test]
    fn test_knowledge_base_volatile_fields() {
        let fields = KnowledgeBase::volatile_fields();
        assert!(fields.contains(&"storageConnectionStringSecret"));
        // knowledgeSources is a normal pushable field, NOT volatile
        assert!(!fields.contains(&"knowledgeSources"));
    }

    #[test]
    fn test_knowledge_base_knowledge_sources_is_pushable() {
        // knowledgeSources is editable via PUT — you can add/remove KS from a KB
        let volatile = KnowledgeBase::volatile_fields();
        let read_only = KnowledgeBase::read_only_fields();
        assert!(!volatile.contains(&"knowledgeSources"));
        assert!(!read_only.contains(&"knowledgeSources"));
    }

    #[test]
    fn test_knowledge_base_deserialize() {
        let json = r#"{
            "name": "my-kb",
            "description": "test kb"
        }"#;
        let kb: KnowledgeBase = serde_json::from_str(json).unwrap();
        assert_eq!(kb.name, "my-kb");
        assert_eq!(kb.description.as_deref(), Some("test kb"));
    }
}
