//! Skillset resource definition

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::traits::{Resource, ResourceKind};

/// Azure AI Search Skillset definition
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skillset {
    pub name: String,
    pub skills: Vec<Skill>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cognitive_services: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub knowledge_store: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_projections: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encryption_key: Option<Value>,
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    #[serde(rename = "@odata.type")]
    pub odata_type: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    pub inputs: Vec<SkillInput>,
    pub outputs: Vec<SkillOutput>,
    /// Skill-specific configuration (varies by skill type)
    #[serde(flatten)]
    pub extra: std::collections::HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillInput {
    pub name: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_context: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inputs: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillOutput {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
}

impl Resource for Skillset {
    fn kind() -> ResourceKind {
        ResourceKind::Skillset
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn volatile_fields() -> &'static [&'static str] {
        // Cognitive services may contain API keys
        &["@odata.etag", "@odata.context", "cognitiveServices"]
    }
}
