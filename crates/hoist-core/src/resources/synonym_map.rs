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
