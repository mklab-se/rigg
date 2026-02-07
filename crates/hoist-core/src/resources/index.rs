//! Index resource definition

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Index definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Index {
    pub name: String,
    pub fields: Vec<Field>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoring_profiles: Option<Vec<ScoringProfile>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_scoring_profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cors_options: Option<CorsOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggesters: Option<Vec<Suggester>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzers: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokenizers: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_filters: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub char_filters: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic: Option<SemanticConfiguration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_search: Option<VectorSearch>,
    /// Catch-all for additional fields from Azure API
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub searchable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filterable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sortable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facetable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrievable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stored: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_analyzer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_analyzer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub synonym_maps: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<Field>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vector_search_profile: Option<String>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoringProfile {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub functions: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function_aggregation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorsOptions {
    pub allowed_origins: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_in_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggester {
    pub name: String,
    pub search_mode: String,
    pub source_fields: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticConfiguration {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_configuration: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configurations: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VectorSearch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub algorithms: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vectorizers: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compressions: Option<Vec<Value>>,
}

impl Resource for Index {
    fn kind() -> ResourceKind {
        ResourceKind::Index
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn immutable_fields() -> &'static [&'static str] {
        // Index fields cannot be modified after creation (only added)
        &["fields"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_kind() {
        assert_eq!(Index::kind(), ResourceKind::Index);
    }

    #[test]
    fn test_index_immutable_fields() {
        assert_eq!(Index::immutable_fields(), &["fields"]);
    }

    #[test]
    fn test_index_deserialize_minimal() {
        let json = r#"{
            "name": "my-index",
            "fields": [
                { "name": "id", "type": "Edm.String", "key": true }
            ]
        }"#;
        let index: Index = serde_json::from_str(json).unwrap();
        assert_eq!(index.name, "my-index");
        assert_eq!(index.fields.len(), 1);
        assert_eq!(index.fields[0].name, "id");
        assert_eq!(index.fields[0].key, Some(true));
    }

    #[test]
    fn test_index_deserialize_with_vector_search() {
        let json = r#"{
            "name": "vec-index",
            "fields": [],
            "vectorSearch": {
                "algorithms": [{"name": "hnsw"}],
                "profiles": [{"name": "default"}]
            }
        }"#;
        let index: Index = serde_json::from_str(json).unwrap();
        assert!(index.vector_search.is_some());
    }

    #[test]
    fn test_index_extra_fields_preserved() {
        let json = r#"{
            "name": "idx",
            "fields": [],
            "customField": "hello"
        }"#;
        let index: Index = serde_json::from_str(json).unwrap();
        assert_eq!(
            index.extra.get("customField").and_then(|v| v.as_str()),
            Some("hello")
        );
    }
}
