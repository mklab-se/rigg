//! Cross-resource reference validation.

use std::collections::{HashMap, HashSet};

use rigg_core::resources::ResourceKind;

pub(super) fn validate_references(
    resources: &HashMap<ResourceKind, Vec<(String, serde_json::Value)>>,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    // Build lookup sets
    let indexes: HashSet<_> = resources
        .get(&ResourceKind::Index)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    let datasources: HashSet<_> = resources
        .get(&ResourceKind::DataSource)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    let skillsets: HashSet<_> = resources
        .get(&ResourceKind::Skillset)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    let synonym_maps: HashSet<_> = resources
        .get(&ResourceKind::SynonymMap)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    // Validate indexer references
    if let Some(indexers) = resources.get(&ResourceKind::Indexer) {
        for (name, value) in indexers {
            // Check data source reference
            if let Some(ds_name) = value.get("dataSourceName").and_then(|n| n.as_str()) {
                if !datasources.contains(ds_name) {
                    errors.push(format!(
                        "indexers/{}.json: references missing data source '{}'",
                        name, ds_name
                    ));
                }
            }

            // Check target index reference
            if let Some(idx_name) = value.get("targetIndexName").and_then(|n| n.as_str()) {
                if !indexes.contains(idx_name) {
                    errors.push(format!(
                        "indexers/{}.json: references missing index '{}'",
                        name, idx_name
                    ));
                }
            }

            // Check skillset reference (optional)
            if let Some(ss_name) = value.get("skillsetName").and_then(|n| n.as_str()) {
                if !skillsets.contains(ss_name) {
                    errors.push(format!(
                        "indexers/{}.json: references missing skillset '{}'",
                        name, ss_name
                    ));
                }
            }
        }
    }

    // Validate knowledge source references
    if let Some(knowledge_sources) = resources.get(&ResourceKind::KnowledgeSource) {
        let knowledge_bases: HashSet<_> = resources
            .get(&ResourceKind::KnowledgeBase)
            .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
            .unwrap_or_default();

        for (name, value) in knowledge_sources {
            // Check index reference
            if let Some(idx_name) = value.get("indexName").and_then(|n| n.as_str()) {
                if !indexes.contains(idx_name) {
                    errors.push(format!(
                        "knowledge-sources/{}.json: references missing index '{}'",
                        name, idx_name
                    ));
                }
            }

            // Check knowledge base reference (optional)
            if let Some(kb_name) = value.get("knowledgeBaseName").and_then(|n| n.as_str()) {
                if !knowledge_bases.contains(kb_name) {
                    errors.push(format!(
                        "knowledge-sources/{}.json: references missing knowledge base '{}'",
                        name, kb_name
                    ));
                }
            }
        }
    }

    // Validate index synonym map references
    if let Some(indexes_list) = resources.get(&ResourceKind::Index) {
        for (name, value) in indexes_list {
            if let Some(fields) = value.get("fields").and_then(|f| f.as_array()) {
                for field in fields {
                    if let Some(syn_maps) = field.get("synonymMaps").and_then(|s| s.as_array()) {
                        for syn_map in syn_maps {
                            if let Some(syn_name) = syn_map.as_str() {
                                if !synonym_maps.contains(syn_name) {
                                    let field_name = field
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("unknown");
                                    errors.push(format!(
                                        "indexes/{}.json: field '{}' references missing synonym map '{}'",
                                        name, field_name, syn_name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_resources(
        entries: Vec<(ResourceKind, Vec<(&str, serde_json::Value)>)>,
    ) -> HashMap<ResourceKind, Vec<(String, serde_json::Value)>> {
        entries
            .into_iter()
            .map(|(kind, items)| {
                (
                    kind,
                    items
                        .into_iter()
                        .map(|(name, val)| (name.to_string(), val))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn test_valid_references_pass() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("my-index", json!({"name": "my-index", "fields": []}))],
            ),
            (
                ResourceKind::DataSource,
                vec![(
                    "my-ds",
                    json!({"name": "my-ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "my-ds",
                        "targetIndexName": "my-index"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_missing_datasource_reference() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "missing-ds",
                        "targetIndexName": "idx"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing data source 'missing-ds'"));
    }

    #[test]
    fn test_missing_index_reference() {
        let resources = make_resources(vec![
            (
                ResourceKind::DataSource,
                vec![(
                    "ds",
                    json!({"name": "ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "ds",
                        "targetIndexName": "missing-index"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing index 'missing-index'"));
    }

    #[test]
    fn test_missing_skillset_reference() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::DataSource,
                vec![(
                    "ds",
                    json!({"name": "ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "ds",
                        "targetIndexName": "idx",
                        "skillsetName": "missing-skillset"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing skillset 'missing-skillset'"));
    }

    #[test]
    fn test_missing_synonym_map_reference() {
        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "my-index",
                json!({
                    "name": "my-index",
                    "fields": [
                        {"name": "title", "type": "Edm.String", "synonymMaps": ["missing-syn"]}
                    ]
                }),
            )],
        )]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing synonym map 'missing-syn'"));
        assert!(errors[0].contains("field 'title'"));
    }

    #[test]
    fn test_indexer_without_skillset_passes() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::DataSource,
                vec![(
                    "ds",
                    json!({"name": "ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "ds",
                        "targetIndexName": "idx"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_empty_resources_passes() {
        let resources: HashMap<ResourceKind, Vec<(String, serde_json::Value)>> = HashMap::new();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_knowledge_source_missing_index() {
        let resources = make_resources(vec![(
            ResourceKind::KnowledgeSource,
            vec![(
                "ks1",
                json!({
                    "name": "ks1",
                    "indexName": "missing-index"
                }),
            )],
        )]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing index 'missing-index'"));
    }

    #[test]
    fn test_knowledge_source_missing_knowledge_base() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::KnowledgeSource,
                vec![(
                    "ks1",
                    json!({
                        "name": "ks1",
                        "indexName": "idx",
                        "knowledgeBaseName": "missing-kb"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing knowledge base 'missing-kb'"));
    }

    #[test]
    fn test_knowledge_source_valid_references() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::KnowledgeBase,
                vec![("kb1", json!({"name": "kb1"}))],
            ),
            (
                ResourceKind::KnowledgeSource,
                vec![(
                    "ks1",
                    json!({
                        "name": "ks1",
                        "indexName": "idx",
                        "knowledgeBaseName": "kb1"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_multiple_errors_accumulated() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "missing-ds",
                    "targetIndexName": "missing-idx",
                    "skillsetName": "missing-ss"
                }),
            )],
        )]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 3);
    }
}
