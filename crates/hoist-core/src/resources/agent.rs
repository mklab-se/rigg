//! Foundry Agent resource decomposition/composition
//!
//! Foundry agents are stored as a single JSON object in the API, but decomposed
//! into multiple human-friendly files on disk:
//! - `config.json` — id, name, model, temperature, top_p, metadata, response_format
//! - `instructions.md` — agent instructions as Markdown
//! - `tools.json` — tools array from agent definition
//! - `knowledge.json` — tool_resources object

use serde_json::{Map, Value};

/// Decomposed agent files for on-disk storage
#[derive(Debug, Clone)]
pub struct AgentFiles {
    /// Agent configuration (id, name, model, temperature, etc.)
    pub config: Value,
    /// Agent instructions as Markdown
    pub instructions: String,
    /// Tools array from agent definition
    pub tools: Value,
    /// Tool resources (knowledge/file references)
    pub knowledge: Value,
}

/// Fields to extract into separate files (not stored in config.json)
const DECOMPOSED_FIELDS: &[&str] = &["instructions", "tools", "tool_resources"];

/// Fields to strip from agent API responses (transient/server-managed)
const VOLATILE_FIELDS: &[&str] = &["created_at", "object", "version"];

/// Split an API response into decomposed on-disk files
pub fn decompose_agent(api_response: &Value) -> AgentFiles {
    let obj = api_response.as_object().cloned().unwrap_or_default();

    // Extract instructions
    let instructions = obj
        .get("instructions")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    // Extract tools
    let tools = obj
        .get("tools")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    // Extract tool_resources (knowledge)
    let knowledge = obj
        .get("tool_resources")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    // Build config: everything except decomposed and volatile fields
    let config_map: Map<String, Value> = obj
        .into_iter()
        .filter(|(k, _)| {
            !DECOMPOSED_FIELDS.contains(&k.as_str()) && !VOLATILE_FIELDS.contains(&k.as_str())
        })
        .collect();

    AgentFiles {
        config: Value::Object(config_map),
        instructions,
        tools,
        knowledge,
    }
}

/// Merge decomposed on-disk files back into an API payload for push
pub fn compose_agent(files: &AgentFiles) -> Value {
    let mut obj = files.config.as_object().cloned().unwrap_or_else(Map::new);

    // Add instructions
    if !files.instructions.is_empty() {
        obj.insert(
            "instructions".to_string(),
            Value::String(files.instructions.clone()),
        );
    }

    // Add tools (always include to match API response shape)
    obj.insert("tools".to_string(), files.tools.clone());

    // Add tool_resources (always include to match API response shape)
    obj.insert("tool_resources".to_string(), files.knowledge.clone());

    Value::Object(obj)
}

/// Returns volatile fields to strip from Foundry agent responses
pub fn agent_volatile_fields() -> &'static [&'static str] {
    VOLATILE_FIELDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_decompose_full_agent() {
        let api_response = json!({
            "id": "asst_abc123",
            "name": "my-agent",
            "model": "gpt-4o",
            "temperature": 0.7,
            "instructions": "You are a helpful assistant.\n\nBe concise.",
            "tools": [
                {"type": "code_interpreter"},
                {"type": "file_search"}
            ],
            "tool_resources": {
                "file_search": {
                    "vector_store_ids": ["vs_123"]
                }
            },
            "created_at": 1700000000,
            "object": "assistant"
        });

        let files = decompose_agent(&api_response);

        // Config should have id, name, model, temperature — not instructions/tools/volatile
        let config = files.config.as_object().unwrap();
        assert_eq!(config.get("id").unwrap(), "asst_abc123");
        assert_eq!(config.get("name").unwrap(), "my-agent");
        assert_eq!(config.get("model").unwrap(), "gpt-4o");
        assert!(!config.contains_key("instructions"));
        assert!(!config.contains_key("tools"));
        assert!(!config.contains_key("tool_resources"));
        assert!(!config.contains_key("created_at"));
        assert!(!config.contains_key("object"));

        // Instructions
        assert_eq!(
            files.instructions,
            "You are a helpful assistant.\n\nBe concise."
        );

        // Tools
        assert_eq!(files.tools.as_array().unwrap().len(), 2);

        // Knowledge
        assert!(files.knowledge.get("file_search").is_some());
    }

    #[test]
    fn test_decompose_minimal_agent() {
        let api_response = json!({
            "id": "asst_min",
            "name": "minimal",
            "model": "gpt-4o"
        });

        let files = decompose_agent(&api_response);

        assert_eq!(files.instructions, "");
        assert_eq!(files.tools, json!([]));
        assert_eq!(files.knowledge, json!({}));
    }

    #[test]
    fn test_compose_roundtrip() {
        let original = json!({
            "id": "asst_abc123",
            "name": "my-agent",
            "model": "gpt-4o",
            "temperature": 0.7,
            "instructions": "You are helpful.",
            "tools": [
                {"type": "code_interpreter"}
            ],
            "tool_resources": {
                "code_interpreter": {
                    "file_ids": ["file_123"]
                }
            },
            "created_at": 1700000000,
            "object": "assistant"
        });

        let files = decompose_agent(&original);
        let composed = compose_agent(&files);

        // Composed should have the non-volatile fields
        let obj = composed.as_object().unwrap();
        assert_eq!(obj.get("id").unwrap(), "asst_abc123");
        assert_eq!(obj.get("name").unwrap(), "my-agent");
        assert_eq!(obj.get("instructions").unwrap(), "You are helpful.");
        assert!(obj.get("tools").unwrap().as_array().unwrap().len() == 1);

        // Should NOT have volatile fields
        assert!(!obj.contains_key("created_at"));
        assert!(!obj.contains_key("object"));
    }

    #[test]
    fn test_compose_empty_fields_preserved() {
        let files = AgentFiles {
            config: json!({"id": "asst_1", "name": "test", "model": "gpt-4o"}),
            instructions: String::new(),
            tools: json!([]),
            knowledge: json!({}),
        };

        let composed = compose_agent(&files);
        let obj = composed.as_object().unwrap();

        // Empty instructions omitted, but tools and tool_resources kept to match API shape
        assert!(!obj.contains_key("instructions"));
        assert_eq!(obj.get("tools").unwrap(), &json!([]));
        assert_eq!(obj.get("tool_resources").unwrap(), &json!({}));
    }

    #[test]
    fn test_compose_with_tools_only() {
        let files = AgentFiles {
            config: json!({"id": "asst_1", "name": "test", "model": "gpt-4o"}),
            instructions: String::new(),
            tools: json!([{"type": "code_interpreter"}]),
            knowledge: json!({}),
        };

        let composed = compose_agent(&files);
        let obj = composed.as_object().unwrap();

        assert!(obj.contains_key("tools"));
        assert!(!obj.contains_key("instructions"));
        assert_eq!(obj.get("tool_resources").unwrap(), &json!({}));
    }

    #[test]
    fn test_decompose_preserves_extra_config_fields() {
        let api_response = json!({
            "id": "asst_1",
            "name": "agent",
            "model": "gpt-4o",
            "top_p": 0.9,
            "metadata": {"team": "search"},
            "response_format": "auto",
            "instructions": "Be helpful."
        });

        let files = decompose_agent(&api_response);
        let config = files.config.as_object().unwrap();

        assert_eq!(config.get("top_p").unwrap(), 0.9);
        assert!(config.get("metadata").is_some());
        assert_eq!(config.get("response_format").unwrap(), "auto");
    }

    #[test]
    fn test_agent_volatile_fields() {
        let fields = agent_volatile_fields();
        assert!(fields.contains(&"created_at"));
        assert!(fields.contains(&"object"));
        assert!(fields.contains(&"version"));
        assert!(!fields.contains(&"name"));
    }

    #[test]
    fn test_decompose_non_object_returns_defaults() {
        let files = decompose_agent(&json!("not an object"));
        assert_eq!(files.instructions, "");
        assert_eq!(files.tools, json!([]));
        assert_eq!(files.knowledge, json!({}));
        assert_eq!(files.config, json!({}));
    }
}
