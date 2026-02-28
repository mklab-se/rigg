//! Lint checks for Azure Search resources.

use std::collections::HashMap;

use hoist_core::resources::ResourceKind;

/// Field count threshold for the "large index" lint warning.
const LARGE_FIELD_COUNT_THRESHOLD: usize = 50;

pub(super) fn lint_resources(
    resources: &HashMap<ResourceKind, Vec<(String, serde_json::Value)>>,
    warnings: &mut Vec<String>,
) {
    // Lint indexes
    if let Some(indexes) = resources.get(&ResourceKind::Index) {
        for (name, value) in indexes {
            lint_index(name, value, warnings);
        }
    }

    // Lint indexers
    if let Some(indexers) = resources.get(&ResourceKind::Indexer) {
        for (name, value) in indexers {
            lint_indexer(name, value, warnings);
        }
    }

    // Lint data sources
    if let Some(datasources) = resources.get(&ResourceKind::DataSource) {
        for (name, value) in datasources {
            lint_datasource(name, value, warnings);
        }
    }
}

fn lint_index(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    if let Some(fields) = value.get("fields").and_then(|f| f.as_array()) {
        // Check for missing key field
        let has_key = fields
            .iter()
            .any(|f| f.get("key").and_then(|k| k.as_bool()).unwrap_or(false));
        if !has_key {
            warnings.push(format!(
                "indexes/{}.json: no field has \"key\": true — index has no key field",
                name
            ));
        }

        // Check for large field count
        let field_count = fields.len();
        if field_count > LARGE_FIELD_COUNT_THRESHOLD {
            warnings.push(format!(
                "indexes/{}.json: index has {} fields (threshold: {}), which may impact performance",
                name, field_count, LARGE_FIELD_COUNT_THRESHOLD
            ));
        }
    }
}

fn lint_indexer(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    // Check for missing or null schedule
    let has_schedule = value
        .get("schedule")
        .is_some_and(|s| !s.is_null() && s.get("interval").is_some());
    if !has_schedule {
        warnings.push(format!(
            "indexers/{}.json: no schedule defined — indexer will only run when triggered manually",
            name
        ));
    }
}

fn lint_datasource(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    // Check for empty or missing container name
    let container_name = value
        .get("container")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");

    if container_name.is_empty() {
        warnings.push(format!(
            "data-sources/{}.json: container name is empty or missing",
            name
        ));
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
    fn test_lint_index_no_key_field() {
        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "my-index",
                json!({
                    "name": "my-index",
                    "fields": [
                        {"name": "title", "type": "Edm.String", "key": false},
                        {"name": "content", "type": "Edm.String"}
                    ]
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no key field"));
        assert!(warnings[0].contains("my-index"));
    }

    #[test]
    fn test_lint_index_with_key_field_no_warning() {
        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "my-index",
                json!({
                    "name": "my-index",
                    "fields": [
                        {"name": "id", "type": "Edm.String", "key": true},
                        {"name": "title", "type": "Edm.String"}
                    ]
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_indexer_no_schedule() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx"
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no schedule defined"));
        assert!(warnings[0].contains("my-indexer"));
        assert!(warnings[0].contains("manually"));
    }

    #[test]
    fn test_lint_indexer_null_schedule() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx",
                    "schedule": null
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no schedule defined"));
    }

    #[test]
    fn test_lint_indexer_with_schedule_no_warning() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx",
                    "schedule": {"interval": "PT5M"}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_index_large_field_count() {
        let mut fields = Vec::new();
        for i in 0..55 {
            fields.push(json!({"name": format!("field_{}", i), "type": "Edm.String"}));
        }

        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "big-index",
                json!({
                    "name": "big-index",
                    "fields": fields
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        // Should have 2 warnings: no key field + large field count
        assert_eq!(warnings.len(), 2);
        let large_warning = warnings.iter().find(|w| w.contains("55 fields"));
        assert!(
            large_warning.is_some(),
            "Expected large field count warning"
        );
        assert!(large_warning.unwrap().contains("big-index"));
    }

    #[test]
    fn test_lint_index_at_threshold_no_warning() {
        let mut fields = Vec::new();
        for i in 0..49 {
            fields.push(json!({"name": format!("field_{}", i), "type": "Edm.String"}));
        }
        fields.push(json!({"name": "id", "type": "Edm.String", "key": true}));
        // 50 fields total — at threshold, not above

        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "normal-index",
                json!({
                    "name": "normal-index",
                    "fields": fields
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings at threshold, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_datasource_empty_container_name() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {},
                    "container": {"name": ""}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("container name is empty or missing"));
        assert!(warnings[0].contains("my-ds"));
    }

    #[test]
    fn test_lint_datasource_missing_container() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("container name is empty or missing"));
    }

    #[test]
    fn test_lint_datasource_missing_container_name_field() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {},
                    "container": {}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("container name is empty or missing"));
    }

    #[test]
    fn test_lint_datasource_valid_container_no_warning() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {},
                    "container": {"name": "my-container"}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_no_resources_no_warnings() {
        let resources: HashMap<ResourceKind, Vec<(String, serde_json::Value)>> = HashMap::new();
        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(warnings.is_empty());
    }
}
