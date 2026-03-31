//! Synonym Map resource definition

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Synonym Map definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SynonymMap {
    pub name: String,
    pub format: String,
    pub synonyms: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption_key: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

impl Resource for SynonymMap {
    fn kind() -> ResourceKind {
        ResourceKind::SynonymMap
    }

    fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_synonym_map_kind() {
        assert_eq!(SynonymMap::kind(), ResourceKind::SynonymMap);
    }

    #[test]
    fn test_synonym_map_default_volatile_fields() {
        let fields = SynonymMap::volatile_fields();
        assert_eq!(fields, &["@odata.etag", "@odata.context"]);
    }

    #[test]
    fn test_synonym_map_identity_key() {
        assert_eq!(SynonymMap::identity_key(), "name");
    }

    #[test]
    fn test_synonym_map_deserialize() {
        let val = json!({
            "name": "my-synonyms",
            "format": "solr",
            "synonyms": "USA, United States, United States of America\nWashington, Wash. => WA"
        });
        let sm: SynonymMap = serde_json::from_value(val).unwrap();
        assert_eq!(sm.name, "my-synonyms");
        assert_eq!(sm.format, "solr");
        assert!(sm.synonyms.contains("USA"));
        assert!(sm.encryption_key.is_none());
    }

    #[test]
    fn test_synonym_map_roundtrip() {
        let val = json!({
            "name": "roundtrip-map",
            "format": "solr",
            "synonyms": "fast, quick, speedy"
        });
        let sm: SynonymMap = serde_json::from_value(val).unwrap();
        let serialized = serde_json::to_string(&sm).unwrap();
        let deserialized: SynonymMap = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.name, "roundtrip-map");
        assert_eq!(deserialized.format, "solr");
        assert_eq!(deserialized.synonyms, "fast, quick, speedy");
    }
}
