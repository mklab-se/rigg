//! Data Source resource definition

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Data Source definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSource {
    pub name: String,
    #[serde(rename = "type")]
    pub datasource_type: String,
    pub credentials: DataSourceCredentials,
    pub container: DataSourceContainer,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_change_detection_policy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_deletion_detection_policy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption_key: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceCredentials {
    /// Connection string (will be redacted in normalized output)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection_string: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DataSourceContainer {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

impl Resource for DataSource {
    fn kind() -> ResourceKind {
        ResourceKind::DataSource
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn volatile_fields() -> &'static [&'static str] {
        &["@odata.etag", "@odata.context", "credentials"]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datasource_kind() {
        assert_eq!(DataSource::kind(), ResourceKind::DataSource);
    }

    #[test]
    fn test_datasource_volatile_fields_include_credentials() {
        let fields = DataSource::volatile_fields();
        assert!(fields.contains(&"credentials"));
    }

    #[test]
    fn test_datasource_deserialize() {
        let json = r#"{
            "name": "my-ds",
            "type": "azureblob",
            "credentials": { "connectionString": "secret" },
            "container": { "name": "docs" }
        }"#;
        let ds: DataSource = serde_json::from_str(json).unwrap();
        assert_eq!(ds.name, "my-ds");
        assert_eq!(ds.datasource_type, "azureblob");
        assert_eq!(ds.container.name, "docs");
    }
}
