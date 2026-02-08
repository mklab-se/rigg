//! Alias resource definition

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Alias definition
///
/// Aliases provide stable endpoint names that point to one or more indexes,
/// enabling zero-downtime reindexing by swapping which index an alias points to.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Alias {
    pub name: String,
    pub indexes: Vec<String>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

impl Resource for Alias {
    fn kind() -> ResourceKind {
        ResourceKind::Alias
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn dependencies(&self) -> Vec<(ResourceKind, String)> {
        self.indexes
            .iter()
            .map(|idx| (ResourceKind::Index, idx.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_alias_kind() {
        assert_eq!(Alias::kind(), ResourceKind::Alias);
    }

    #[test]
    fn test_alias_default_volatile_fields() {
        let fields = Alias::volatile_fields();
        assert_eq!(fields, &["@odata.etag", "@odata.context"]);
    }

    #[test]
    fn test_alias_identity_key() {
        assert_eq!(Alias::identity_key(), "name");
    }

    #[test]
    fn test_alias_deserialize() {
        let val = json!({
            "name": "my-alias",
            "indexes": ["hotels-v1"]
        });
        let alias: Alias = serde_json::from_value(val).unwrap();
        assert_eq!(alias.name, "my-alias");
        assert_eq!(alias.indexes, vec!["hotels-v1"]);
    }

    #[test]
    fn test_alias_dependencies() {
        let alias = Alias {
            name: "my-alias".to_string(),
            indexes: vec!["index-a".to_string(), "index-b".to_string()],
            extra: Default::default(),
        };
        let deps = alias.dependencies();
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0], (ResourceKind::Index, "index-a".to_string()));
        assert_eq!(deps[1], (ResourceKind::Index, "index-b".to_string()));
    }

    #[test]
    fn test_alias_roundtrip() {
        let val = json!({
            "name": "roundtrip-alias",
            "indexes": ["idx-1", "idx-2"]
        });
        let alias: Alias = serde_json::from_value(val).unwrap();
        let serialized = serde_json::to_string(&alias).unwrap();
        let deserialized: Alias = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.name, "roundtrip-alias");
        assert_eq!(deserialized.indexes, vec!["idx-1", "idx-2"]);
    }
}
