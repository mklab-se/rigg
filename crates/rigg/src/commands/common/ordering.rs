//! Resource dependency ordering and agent YAML reading.

use anyhow::Result;

use rigg_core::resources::ResourceKind;

/// Order resources by dependencies (data sources before indexers, etc.)
pub fn order_by_dependencies(
    resources: &[(ResourceKind, String, serde_json::Value, bool)],
) -> Vec<(ResourceKind, String, serde_json::Value, bool)> {
    let order = [
        ResourceKind::SynonymMap,      // No dependencies
        ResourceKind::DataSource,      // No dependencies
        ResourceKind::Index,           // May depend on synonym maps
        ResourceKind::Alias,           // Points to indexes
        ResourceKind::Skillset,        // No dependencies
        ResourceKind::KnowledgeBase,   // No dependencies
        ResourceKind::Indexer,         // Depends on data source, index, skillset
        ResourceKind::KnowledgeSource, // Depends on index, knowledge base
        ResourceKind::Agent,           // Foundry: no cross-service dependencies
    ];

    let mut ordered = resources.to_vec();
    ordered
        .sort_by_key(|(kind, _, _, _)| order.iter().position(|k| k == kind).unwrap_or(usize::MAX));

    ordered
}

/// Read a single agent YAML file and return the parsed JSON Value.
///
/// The agent name is derived from the filename stem (e.g. `regulus.yaml` -> `"regulus"`).
/// The name is NOT injected here -- callers add it before wrapping for API use.
pub fn read_agent_yaml(path: &std::path::Path) -> Result<serde_json::Value> {
    let content = std::fs::read_to_string(path)?;
    rigg_core::resources::agent::yaml_to_agent(&content)
        .map_err(|e| anyhow::anyhow!("Invalid YAML in {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // === order_by_dependencies tests ===

    #[test]
    fn test_order_datasource_before_indexer() {
        let resources = vec![
            (ResourceKind::Indexer, "ixer".to_string(), json!({}), false),
            (ResourceKind::DataSource, "ds".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        assert_eq!(ordered[0].0, ResourceKind::DataSource);
        assert_eq!(ordered[1].0, ResourceKind::Indexer);
    }

    #[test]
    fn test_order_index_before_indexer() {
        let resources = vec![
            (ResourceKind::Indexer, "ixer".to_string(), json!({}), false),
            (ResourceKind::Index, "idx".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        assert_eq!(ordered[0].0, ResourceKind::Index);
        assert_eq!(ordered[1].0, ResourceKind::Indexer);
    }

    #[test]
    fn test_order_knowledge_base_before_knowledge_source() {
        let resources = vec![
            (
                ResourceKind::KnowledgeSource,
                "ks".to_string(),
                json!({}),
                false,
            ),
            (
                ResourceKind::KnowledgeBase,
                "kb".to_string(),
                json!({}),
                false,
            ),
        ];
        let ordered = order_by_dependencies(&resources);
        assert_eq!(ordered[0].0, ResourceKind::KnowledgeBase);
        assert_eq!(ordered[1].0, ResourceKind::KnowledgeSource);
    }

    #[test]
    fn test_order_full_dependency_chain() {
        let resources = vec![
            (
                ResourceKind::KnowledgeSource,
                "ks".to_string(),
                json!({}),
                false,
            ),
            (ResourceKind::Indexer, "ixer".to_string(), json!({}), false),
            (ResourceKind::Index, "idx".to_string(), json!({}), false),
            (ResourceKind::Alias, "al".to_string(), json!({}), false),
            (ResourceKind::DataSource, "ds".to_string(), json!({}), false),
            (
                ResourceKind::KnowledgeBase,
                "kb".to_string(),
                json!({}),
                false,
            ),
            (ResourceKind::Skillset, "sk".to_string(), json!({}), false),
            (ResourceKind::SynonymMap, "sm".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        let kinds: Vec<_> = ordered.iter().map(|(k, _, _, _)| *k).collect();
        assert_eq!(
            kinds,
            vec![
                ResourceKind::SynonymMap,
                ResourceKind::DataSource,
                ResourceKind::Index,
                ResourceKind::Alias,
                ResourceKind::Skillset,
                ResourceKind::KnowledgeBase,
                ResourceKind::Indexer,
                ResourceKind::KnowledgeSource,
            ]
        );
    }

    #[test]
    fn test_order_empty() {
        let resources: Vec<(ResourceKind, String, serde_json::Value, bool)> = vec![];
        let ordered = order_by_dependencies(&resources);
        assert!(ordered.is_empty());
    }

    #[test]
    fn test_order_preserves_within_same_kind() {
        let resources = vec![
            (ResourceKind::Index, "b-index".to_string(), json!({}), false),
            (ResourceKind::Index, "a-index".to_string(), json!({}), false),
        ];
        let ordered = order_by_dependencies(&resources);
        // sort_by_key is stable, so same-kind order is preserved
        assert_eq!(ordered[0].1, "b-index");
        assert_eq!(ordered[1].1, "a-index");
    }

    // === read_agent_yaml tests ===

    #[test]
    fn test_read_agent_yaml_full() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("my-agent.yaml");

        std::fs::write(
            &yaml_path,
            "kind: prompt\nmodel: gpt-4o\ninstructions: You are helpful.\ntools:\n  - type: code_interpreter\ntool_resources:\n  file_search:\n    vector_store_ids:\n      - vs_1\n",
        )
        .unwrap();

        let value = read_agent_yaml(&yaml_path).unwrap();
        assert_eq!(value["model"], "gpt-4o");
        assert_eq!(value["kind"], "prompt");
        assert!(value["instructions"].as_str().unwrap().contains("helpful"));
        assert_eq!(value["tools"].as_array().unwrap().len(), 1);
        assert!(
            value["tool_resources"]["file_search"]["vector_store_ids"]
                .as_array()
                .unwrap()
                .len()
                == 1
        );
    }

    #[test]
    fn test_read_agent_yaml_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("missing.yaml");

        let result = read_agent_yaml(&yaml_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_agent_yaml_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("bad.yaml");
        std::fs::write(&yaml_path, "{{invalid yaml").unwrap();

        let result = read_agent_yaml(&yaml_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_read_agent_yaml_roundtrip() {
        use rigg_core::resources::agent::agent_to_yaml;

        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("roundtrip.yaml");

        let original = json!({
            "name": "roundtrip",
            "kind": "prompt",
            "model": "gpt-4o",
            "instructions": "Be concise.",
            "tools": [{"type": "file_search"}]
        });

        // Write YAML from agent JSON
        let yaml = agent_to_yaml(&original);
        std::fs::write(&yaml_path, &yaml).unwrap();

        // Read back
        let parsed = read_agent_yaml(&yaml_path).unwrap();
        assert_eq!(parsed["kind"], "prompt");
        assert_eq!(parsed["model"], "gpt-4o");
        assert_eq!(parsed["instructions"], "Be concise.");
        assert_eq!(parsed["tools"].as_array().unwrap().len(), 1);
    }
}
