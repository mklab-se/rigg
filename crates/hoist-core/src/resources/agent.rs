//! Foundry Agent resource — YAML on-disk format
//!
//! Foundry agents are stored as a single YAML file per agent:
//! `agents/<agent-name>.yaml`
//!
//! The YAML format matches the Foundry portal's YAML view. Agent name is
//! derived from the filename (not stored in the YAML).

use serde_json::Value;

/// Fields to strip from agent API responses (transient/server-managed)
const VOLATILE_FIELDS: &[&str] = &["id", "created_at", "object", "version"];

/// Fields excluded from YAML (volatile + identity fields managed externally)
const YAML_EXCLUDE_FIELDS: &[&str] = &["name", "id", "created_at", "object", "version"];

/// Field order for YAML output (matches Foundry portal).
const AGENT_YAML_FIELD_ORDER: &[&str] = &[
    "kind",
    "model",
    "description",
    "temperature",
    "top_p",
    "response_format",
    "metadata",
    "instructions",
    "tools",
    "tool_resources",
];

/// Convert a flattened agent JSON value to ordered YAML string.
///
/// Strips identity/volatile fields (`name`, `id`, `created_at`, `object`, `version`),
/// omits empty optional fields, and orders remaining fields per `AGENT_YAML_FIELD_ORDER`.
pub fn agent_to_yaml(agent: &Value) -> String {
    let obj = agent.as_object().cloned().unwrap_or_default();

    // Build ordered map
    let mut ordered = serde_json::Map::new();

    // First: known fields in display order
    for &field in AGENT_YAML_FIELD_ORDER {
        if let Some(value) = obj.get(field) {
            // Skip empty/null optional fields
            if should_omit(field, value) {
                continue;
            }
            ordered.insert(field.to_string(), value.clone());
        }
    }

    // Then: any remaining fields not in the known list and not excluded
    for (key, value) in &obj {
        if YAML_EXCLUDE_FIELDS.contains(&key.as_str()) {
            continue;
        }
        if ordered.contains_key(key) {
            continue;
        }
        if should_omit(key, value) {
            continue;
        }
        ordered.insert(key.clone(), value.clone());
    }

    let ordered_value = Value::Object(ordered);
    serde_yaml::to_string(&ordered_value).unwrap_or_default()
}

/// Parse agent YAML back to a serde_json::Value for API operations.
pub fn yaml_to_agent(yaml: &str) -> Result<Value, serde_yaml::Error> {
    serde_yaml::from_str(yaml)
}

/// Returns volatile fields to strip from Foundry agent responses
pub fn agent_volatile_fields() -> &'static [&'static str] {
    VOLATILE_FIELDS
}

/// Strip empty optional fields from an agent value for consistent comparison.
///
/// The YAML format intentionally omits empty `description`, `metadata`, `tool_resources`,
/// etc. The server always returns these fields (as `""` or `{}`). Call this on remote
/// agent values before comparing to local YAML-derived values so that omitted-vs-empty
/// differences don't appear as drift.
pub fn strip_agent_empty_fields(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.retain(|key, val| !should_omit(key, val));
    }
}

/// Whether an optional field should be omitted from YAML output.
fn should_omit(field: &str, value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) if s.is_empty() => true,
        Value::Array(arr) if arr.is_empty() && field == "tools" => false, // keep empty tools
        Value::Array(arr) if arr.is_empty() => true,
        Value::Object(map) if map.is_empty() && field == "tool_resources" => true,
        Value::Object(map) if map.is_empty() && field == "metadata" => true,
        _ => false,
    }
}

// --- Legacy support: keep wrap/flatten for API layer (in foundry.rs) ---
// decompose_agent / compose_agent / AgentFiles are removed.

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_agent_to_yaml_full() {
        let agent = json!({
            "id": "asst_abc123",
            "name": "regulus",
            "kind": "prompt",
            "model": "gpt-5.2-chat",
            "description": "A regulatory compliance assistant",
            "temperature": 0.7,
            "top_p": 1.0,
            "instructions": "You are Regulus.\n\n## Personality\nFriendly and calm.",
            "tools": [
                {"type": "mcp", "server_label": "kb_test"}
            ],
            "tool_resources": {
                "file_search": {
                    "vector_store_ids": ["vs_abc123"]
                }
            },
            "created_at": 1700000000,
            "object": "assistant",
            "version": "1.0"
        });

        let yaml = agent_to_yaml(&agent);

        // Verify field order
        let lines: Vec<&str> = yaml.lines().collect();
        let first_field = lines.iter().find(|l| !l.starts_with("---")).unwrap();
        assert!(
            first_field.starts_with("kind:"),
            "First field should be 'kind', got: {}",
            first_field
        );

        // Verify volatile/identity fields are excluded
        assert!(!yaml.contains("name:"), "name should be excluded");
        assert!(
            !yaml.contains("id:"),
            "id should be excluded (but 'kind' may contain 'id' substring)"
        );
        assert!(
            !yaml.contains("created_at:"),
            "created_at should be excluded"
        );
        assert!(!yaml.contains("object:"), "object should be excluded");
        assert!(!yaml.contains("version:"), "version should be excluded");

        // Verify content is present
        assert!(yaml.contains("model: gpt-5.2-chat"));
        assert!(yaml.contains("description: A regulatory compliance assistant"));
        assert!(yaml.contains("temperature: 0.7"));
        assert!(yaml.contains("top_p: 1.0"));
        assert!(yaml.contains("You are Regulus."));
        assert!(yaml.contains("mcp"));
        assert!(yaml.contains("vs_abc123"));
    }

    #[test]
    fn test_agent_to_yaml_minimal() {
        let agent = json!({
            "name": "minimal",
            "model": "gpt-4o",
            "kind": "prompt"
        });

        let yaml = agent_to_yaml(&agent);

        assert!(yaml.contains("kind: prompt"));
        assert!(yaml.contains("model: gpt-4o"));
        // Optional fields should be absent
        assert!(!yaml.contains("description"));
        assert!(!yaml.contains("temperature"));
        assert!(!yaml.contains("top_p"));
        assert!(!yaml.contains("instructions"));
        assert!(!yaml.contains("tool_resources"));
    }

    #[test]
    fn test_agent_to_yaml_strips_volatile() {
        let agent = json!({
            "name": "test",
            "id": "asst_123",
            "model": "gpt-4o",
            "created_at": 1700000000,
            "object": "assistant",
            "version": "1.0"
        });

        let yaml = agent_to_yaml(&agent);

        assert!(!yaml.contains("created_at"));
        assert!(!yaml.contains("object"));
        assert!(!yaml.contains("version"));
        assert!(!yaml.contains("asst_123"));
        assert!(yaml.contains("model: gpt-4o"));
    }

    #[test]
    fn test_agent_to_yaml_multiline_instructions() {
        let agent = json!({
            "name": "test",
            "model": "gpt-4o",
            "instructions": "Line one.\n\nLine three.\nLine four."
        });

        let yaml = agent_to_yaml(&agent);

        // serde_yaml should use block scalar for multiline
        assert!(yaml.contains("instructions:"));
        assert!(yaml.contains("Line one."));
        assert!(yaml.contains("Line three."));
    }

    #[test]
    fn test_yaml_to_agent_roundtrip() {
        let agent = json!({
            "name": "roundtrip",
            "id": "asst_rt",
            "kind": "prompt",
            "model": "gpt-4o",
            "description": "Test agent",
            "temperature": 0.7,
            "instructions": "Be helpful.\n\nBe concise.",
            "tools": [
                {"type": "code_interpreter"}
            ],
            "tool_resources": {
                "file_search": {
                    "vector_store_ids": ["vs_1"]
                }
            }
        });

        let yaml = agent_to_yaml(&agent);
        let parsed = yaml_to_agent(&yaml).unwrap();

        // All non-excluded fields should round-trip
        assert_eq!(parsed["kind"], "prompt");
        assert_eq!(parsed["model"], "gpt-4o");
        assert_eq!(parsed["description"], "Test agent");
        assert_eq!(parsed["temperature"], 0.7);
        assert!(parsed["instructions"]
            .as_str()
            .unwrap()
            .contains("Be helpful."));
        assert_eq!(parsed["tools"].as_array().unwrap().len(), 1);
        assert!(
            parsed["tool_resources"]["file_search"]["vector_store_ids"]
                .as_array()
                .unwrap()
                .len()
                == 1
        );

        // Excluded fields should NOT be present
        assert!(parsed.get("name").is_none());
        assert!(parsed.get("id").is_none());
    }

    #[test]
    fn test_yaml_to_agent_from_portal() {
        let portal_yaml = r#"
kind: prompt
model: gpt-5.2-chat
description: A regulatory compliance assistant
temperature: 0.7
top_p: 1.0
instructions: |
  You are Regulus, a knowledgeable and reliable AI assistant.

  ## Personality and Tone
  You are friendly, calm, and helpful.
tools:
  - type: mcp
    server_label: kb_regulatory_kb_9kdyn
    server_url: https://mklabsrch.search.windows.net/knowledgebases/regulatory-kb/mcp
    require_approval: never
    project_connection_id: kb-regulatory-kb-9kdyn
tool_resources:
  file_search:
    vector_store_ids:
      - vs_abc123
"#;
        let parsed = yaml_to_agent(portal_yaml).unwrap();

        assert_eq!(parsed["kind"], "prompt");
        assert_eq!(parsed["model"], "gpt-5.2-chat");
        assert_eq!(parsed["temperature"], 0.7);
        assert_eq!(parsed["top_p"], 1.0);
        assert!(parsed["instructions"].as_str().unwrap().contains("Regulus"));
        assert_eq!(parsed["tools"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["tools"][0]["type"], "mcp");
        assert_eq!(
            parsed["tool_resources"]["file_search"]["vector_store_ids"][0],
            "vs_abc123"
        );
    }

    #[test]
    fn test_agent_volatile_fields() {
        let fields = agent_volatile_fields();
        assert!(fields.contains(&"id"));
        assert!(fields.contains(&"created_at"));
        assert!(fields.contains(&"object"));
        assert!(fields.contains(&"version"));
        assert!(!fields.contains(&"name"));
    }

    #[test]
    fn test_agent_to_yaml_non_object_returns_empty() {
        let yaml = agent_to_yaml(&json!("not an object"));
        // Should produce valid YAML (empty object)
        assert!(yaml.trim() == "{}" || yaml.trim().is_empty());
    }

    #[test]
    fn test_agent_to_yaml_preserves_unknown_fields() {
        let agent = json!({
            "name": "test",
            "model": "gpt-4o",
            "kind": "prompt",
            "custom_field": "custom_value"
        });

        let yaml = agent_to_yaml(&agent);
        assert!(yaml.contains("custom_field: custom_value"));
    }

    #[test]
    fn test_agent_to_yaml_empty_tools_preserved() {
        let agent = json!({
            "name": "test",
            "model": "gpt-4o",
            "tools": []
        });

        let yaml = agent_to_yaml(&agent);
        assert!(yaml.contains("tools:"));
    }

    #[test]
    fn test_strip_agent_empty_fields() {
        let mut value = json!({
            "name": "test",
            "model": "gpt-4o",
            "description": "",
            "metadata": {},
            "tool_resources": {},
            "instructions": "Be helpful."
        });
        strip_agent_empty_fields(&mut value);

        assert_eq!(value["name"], "test");
        assert_eq!(value["model"], "gpt-4o");
        assert_eq!(value["instructions"], "Be helpful.");
        assert!(value.get("description").is_none());
        assert!(value.get("metadata").is_none());
        assert!(value.get("tool_resources").is_none());
    }

    #[test]
    fn test_agent_to_yaml_empty_metadata_omitted() {
        let agent = json!({
            "name": "test",
            "model": "gpt-4o",
            "metadata": {}
        });

        let yaml = agent_to_yaml(&agent);
        assert!(!yaml.contains("metadata"));
    }

    #[test]
    fn test_agent_yaml_roundtrip_preserves_require_approval() {
        // Test string form: "never"
        let agent_never = json!({
            "name": "test",
            "kind": "prompt",
            "model": "gpt-4o",
            "tools": [{
                "type": "mcp",
                "server_label": "kb_test",
                "require_approval": "never"
            }]
        });

        let yaml = agent_to_yaml(&agent_never);
        let parsed = yaml_to_agent(&yaml).unwrap();
        assert_eq!(parsed["tools"][0]["require_approval"], "never");

        // Test string form: "always"
        let agent_always = json!({
            "name": "test",
            "kind": "prompt",
            "model": "gpt-4o",
            "tools": [{
                "type": "mcp",
                "server_label": "kb_test",
                "require_approval": "always"
            }]
        });

        let yaml = agent_to_yaml(&agent_always);
        let parsed = yaml_to_agent(&yaml).unwrap();
        assert_eq!(parsed["tools"][0]["require_approval"], "always");

        // Test object form with granular per-tool control
        let agent_object = json!({
            "name": "test",
            "kind": "prompt",
            "model": "gpt-4o",
            "tools": [{
                "type": "mcp",
                "server_label": "kb_test",
                "require_approval": {
                    "never": {"tool_names": ["safe_tool"]},
                    "always": {"tool_names": ["dangerous_tool"]}
                }
            }]
        });

        let yaml = agent_to_yaml(&agent_object);
        let parsed = yaml_to_agent(&yaml).unwrap();
        let ra = &parsed["tools"][0]["require_approval"];
        assert_eq!(ra["never"]["tool_names"][0], "safe_tool");
        assert_eq!(ra["always"]["tool_names"][0], "dangerous_tool");
    }

    #[test]
    fn test_agent_yaml_roundtrip_preserves_allowed_tools() {
        let agent = json!({
            "name": "test",
            "kind": "prompt",
            "model": "gpt-4o",
            "tools": [{
                "type": "mcp",
                "server_label": "kb_test",
                "allowed_tools": ["tool_a", "tool_b"]
            }]
        });

        let yaml = agent_to_yaml(&agent);
        let parsed = yaml_to_agent(&yaml).unwrap();
        let allowed = parsed["tools"][0]["allowed_tools"].as_array().unwrap();
        assert_eq!(allowed.len(), 2);
        assert_eq!(allowed[0], "tool_a");
        assert_eq!(allowed[1], "tool_b");
    }

    #[test]
    fn test_strip_agent_empty_fields_preserves_tool_permissions() {
        let mut value = json!({
            "name": "test",
            "model": "gpt-4o",
            "description": "",
            "metadata": {},
            "tools": [{
                "type": "mcp",
                "server_label": "kb_test",
                "require_approval": "never",
                "allowed_tools": ["tool_a", "tool_b"]
            }]
        });

        strip_agent_empty_fields(&mut value);

        // Empty top-level fields should be stripped
        assert!(value.get("description").is_none());
        assert!(value.get("metadata").is_none());

        // Tool permission fields inside tool objects must be untouched
        let tool = &value["tools"][0];
        assert_eq!(tool["require_approval"], "never");
        let allowed = tool["allowed_tools"].as_array().unwrap();
        assert_eq!(allowed.len(), 2);
        assert_eq!(allowed[0], "tool_a");
        assert_eq!(allowed[1], "tool_b");
    }
}
