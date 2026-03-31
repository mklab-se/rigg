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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_skillset_kind() {
        assert_eq!(Skillset::kind(), ResourceKind::Skillset);
    }

    #[test]
    fn test_skillset_volatile_fields() {
        let fields = Skillset::volatile_fields();
        assert!(fields.contains(&"cognitiveServices"));
        assert!(fields.contains(&"@odata.etag"));
        assert!(fields.contains(&"@odata.context"));
    }

    #[test]
    fn test_skillset_identity_key() {
        assert_eq!(Skillset::identity_key(), "name");
    }

    #[test]
    fn test_skillset_no_dependencies() {
        let json = r#"{
            "name": "my-skillset",
            "skills": []
        }"#;
        let skillset: Skillset = serde_json::from_str(json).unwrap();
        assert!(skillset.dependencies().is_empty());
    }

    #[test]
    fn test_skillset_deserialize() {
        let val = json!({
            "name": "my-skillset",
            "description": "A test skillset",
            "skills": [
                {
                    "@odata.type": "#Microsoft.Skills.Text.EntityRecognitionSkill",
                    "name": "entity-recognition",
                    "description": "Recognize entities",
                    "context": "/document",
                    "inputs": [
                        { "name": "text", "source": "/document/content" }
                    ],
                    "outputs": [
                        { "name": "persons", "targetName": "people" }
                    ]
                }
            ]
        });
        let skillset: Skillset = serde_json::from_value(val).unwrap();
        assert_eq!(skillset.name, "my-skillset");
        assert_eq!(skillset.description.as_deref(), Some("A test skillset"));
        assert_eq!(skillset.skills.len(), 1);
        assert_eq!(skillset.skills[0].name, "entity-recognition");
        assert_eq!(
            skillset.skills[0].odata_type,
            "#Microsoft.Skills.Text.EntityRecognitionSkill"
        );
        assert_eq!(skillset.skills[0].inputs.len(), 1);
        assert_eq!(skillset.skills[0].inputs[0].name, "text");
        assert_eq!(skillset.skills[0].outputs.len(), 1);
        assert_eq!(
            skillset.skills[0].outputs[0].target_name.as_deref(),
            Some("people")
        );
    }

    #[test]
    fn test_skillset_roundtrip() {
        let val = json!({
            "name": "roundtrip-skillset",
            "skills": [
                {
                    "@odata.type": "#Microsoft.Skills.Text.KeyPhraseExtractionSkill",
                    "name": "keyphrases",
                    "context": "/document",
                    "inputs": [
                        { "name": "text", "source": "/document/content" }
                    ],
                    "outputs": [
                        { "name": "keyPhrases", "targetName": "keyphrases" }
                    ]
                }
            ]
        });
        let skillset: Skillset = serde_json::from_value(val).unwrap();
        let serialized = serde_json::to_string(&skillset).unwrap();
        let deserialized: Skillset = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized.name, "roundtrip-skillset");
        assert_eq!(deserialized.skills.len(), 1);
        assert_eq!(deserialized.skills[0].name, "keyphrases");
    }
}
