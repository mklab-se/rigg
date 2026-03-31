//! Indexer resource definition

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Indexer definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Indexer {
    pub name: String,
    pub data_source_name: String,
    pub target_index_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skillset_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<IndexerSchedule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<IndexerParameters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field_mappings: Option<Vec<FieldMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_field_mappings: Option<Vec<FieldMapping>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption_key: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerSchedule {
    pub interval: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexerParameters {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub batch_size: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_failed_items: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_failed_items_per_batch: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldMapping {
    pub source_field_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_field_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_function: Option<MappingFunction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<Value>,
}

impl Resource for Indexer {
    fn kind() -> ResourceKind {
        ResourceKind::Indexer
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn read_only_fields() -> &'static [&'static str] {
        // startTime is server-managed — Azure updates it every time the indexer runs.
        // Kept in local files to show scheduling info, stripped before push.
        &["startTime"]
    }

    fn dependencies(&self) -> Vec<(ResourceKind, String)> {
        let mut deps = vec![
            (ResourceKind::DataSource, self.data_source_name.clone()),
            (ResourceKind::Index, self.target_index_name.clone()),
        ];
        if let Some(ref skillset) = self.skillset_name {
            deps.push((ResourceKind::Skillset, skillset.clone()));
        }
        deps
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_indexer(skillset: Option<&str>) -> Indexer {
        Indexer {
            name: "my-indexer".to_string(),
            data_source_name: "my-ds".to_string(),
            target_index_name: "my-index".to_string(),
            skillset_name: skillset.map(String::from),
            description: None,
            schedule: None,
            parameters: None,
            field_mappings: None,
            output_field_mappings: None,
            disabled: None,
            cache: None,
            encryption_key: None,
            extra: Default::default(),
        }
    }

    #[test]
    fn test_indexer_kind() {
        assert_eq!(Indexer::kind(), ResourceKind::Indexer);
    }

    #[test]
    fn test_indexer_dependencies_without_skillset() {
        let indexer = make_indexer(None);
        let deps = indexer.dependencies();
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&(ResourceKind::DataSource, "my-ds".to_string())));
        assert!(deps.contains(&(ResourceKind::Index, "my-index".to_string())));
    }

    #[test]
    fn test_indexer_dependencies_with_skillset() {
        let indexer = make_indexer(Some("my-skillset"));
        let deps = indexer.dependencies();
        assert_eq!(deps.len(), 3);
        assert!(deps.contains(&(ResourceKind::Skillset, "my-skillset".to_string())));
    }

    #[test]
    fn test_indexer_deserialize() {
        let json = r#"{
            "name": "test-indexer",
            "dataSourceName": "ds",
            "targetIndexName": "idx"
        }"#;
        let indexer: Indexer = serde_json::from_str(json).unwrap();
        assert_eq!(indexer.name, "test-indexer");
        assert_eq!(indexer.data_source_name, "ds");
        assert_eq!(indexer.target_index_name, "idx");
    }

    #[test]
    fn test_indexer_read_only_fields_includes_start_time() {
        let fields = Indexer::read_only_fields();
        assert!(
            fields.contains(&"startTime"),
            "startTime is server-managed and must be stripped before push"
        );
    }
}
