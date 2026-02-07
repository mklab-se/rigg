//! Copy/rename support for push operations

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde_json::Value;

use crate::resources::ResourceKind;

/// Maps old resource names to new resource names, keyed by (ResourceKind, old_name).
#[derive(Default)]
pub struct NameMap {
    map: HashMap<(ResourceKind, String), String>,
}

impl NameMap {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    pub fn insert(&mut self, kind: ResourceKind, old: &str, new: &str) {
        self.map.insert((kind, old.to_string()), new.to_string());
    }

    pub fn get(&self, kind: ResourceKind, old: &str) -> Option<&str> {
        self.map.get(&(kind, old.to_string())).map(|s| s.as_str())
    }

    /// Create a NameMap by appending a suffix to all resource names.
    pub fn from_suffix(resources: &[(ResourceKind, String)], suffix: &str) -> Self {
        let mut map = Self::new();
        for (kind, name) in resources {
            map.insert(*kind, name, &format!("{}{}", name, suffix));
        }
        map
    }

    /// Load a NameMap from a JSON answers file.
    ///
    /// Expected format:
    /// ```json
    /// {
    ///   "indexes": { "old-name": "new-name" },
    ///   "indexers": { "old-indexer": "new-indexer" }
    /// }
    /// ```
    ///
    /// Keys are the `api_path()` values for each resource kind.
    pub fn from_answers_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let root: Value = serde_json::from_str(&content)?;

        let obj = root
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Answers file must be a JSON object"))?;

        let mut map = Self::new();

        for kind in ResourceKind::all() {
            if let Some(mappings) = obj.get(kind.api_path()) {
                let mappings = mappings
                    .as_object()
                    .ok_or_else(|| anyhow::anyhow!("'{}' must be an object", kind.api_path()))?;

                for (old_name, new_name) in mappings {
                    let new_name = new_name.as_str().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Value for '{}' in '{}' must be a string",
                            old_name,
                            kind.api_path()
                        )
                    })?;
                    map.insert(*kind, old_name, new_name);
                }
            }
        }

        Ok(map)
    }

    /// Look up a new name by searching all resource kinds.
    /// Used for reference rewriting where we know the referenced name but not necessarily
    /// which kind it belongs to from the reference field alone.
    fn find_by_name(&self, name: &str) -> Option<&str> {
        for ((_, old_name), new_name) in &self.map {
            if old_name == name {
                return Some(new_name.as_str());
            }
        }
        None
    }
}

/// Reference field definitions: which fields in which resource kinds contain references
/// to other resource names.
const REFERENCE_FIELDS: &[(ResourceKind, &[&str])] = &[
    (
        ResourceKind::Indexer,
        &["dataSourceName", "targetIndexName", "skillsetName"],
    ),
    (
        ResourceKind::KnowledgeSource,
        &["indexName", "knowledgeBaseName"],
    ),
];

/// Array reference fields: fields containing arrays of objects with a "name" key
/// that references other resources. Used for KB → KS relationships.
const ARRAY_REFERENCE_FIELDS: &[(ResourceKind, &str, ResourceKind)] = &[(
    ResourceKind::KnowledgeBase,
    "knowledgeSources",
    ResourceKind::KnowledgeSource,
)];

/// Rewrite resource references using the name map.
/// Returns a list of warning messages for references not found in the name map.
pub fn rewrite_references(
    kind: ResourceKind,
    definition: &mut Value,
    name_map: &NameMap,
) -> Vec<String> {
    let mut warnings = Vec::new();

    let obj = match definition.as_object_mut() {
        Some(obj) => obj,
        None => return warnings,
    };

    // Rewrite string reference fields (e.g., Indexer's dataSourceName)
    let string_fields = REFERENCE_FIELDS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, f)| *f)
        .unwrap_or(&[]);

    for field in string_fields {
        if let Some(value) = obj.get(*field) {
            if let Some(old_ref) = value.as_str() {
                if let Some(new_ref) = name_map.find_by_name(old_ref) {
                    obj.insert(field.to_string(), Value::String(new_ref.to_string()));
                } else if !old_ref.is_empty() {
                    warnings.push(format!(
                        "{} '{}' references '{}' via '{}' which is not in the copy set",
                        kind.display_name(),
                        obj.get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown"),
                        old_ref,
                        field,
                    ));
                }
            }
        }
    }

    // Rewrite array reference fields (e.g., KB's knowledgeSources)
    let resource_name = obj
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("unknown")
        .to_string();

    for (ref_kind, field_name, target_kind) in ARRAY_REFERENCE_FIELDS {
        if *ref_kind != kind {
            continue;
        }
        if let Some(arr) = obj.get_mut(*field_name) {
            if let Some(items) = arr.as_array_mut() {
                for item in items {
                    if let Some(item_obj) = item.as_object_mut() {
                        if let Some(name_val) = item_obj.get("name") {
                            if let Some(old_name) = name_val.as_str() {
                                if let Some(new_name) = name_map.get(*target_kind, old_name) {
                                    item_obj.insert(
                                        "name".to_string(),
                                        Value::String(new_name.to_string()),
                                    );
                                } else if !old_name.is_empty() {
                                    warnings.push(format!(
                                        "{} '{}' references '{}' via '{}' which is not in the copy set",
                                        kind.display_name(),
                                        resource_name,
                                        old_name,
                                        field_name,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    warnings
}

/// Compute the dependency closure: given a set of selected resources,
/// add any resources they depend on.
pub fn compute_dependency_closure(
    selected: &[(ResourceKind, String, Value)],
    all_resources: &[(ResourceKind, String, Value)],
) -> Vec<(ResourceKind, String, Value)> {
    let mut result: Vec<(ResourceKind, String, Value)> = selected.to_vec();
    let mut seen: HashMap<(ResourceKind, String), bool> = HashMap::new();

    for (kind, name, _) in &result {
        seen.insert((*kind, name.clone()), true);
    }

    let mut changed = true;
    while changed {
        changed = false;
        let current: Vec<_> = result.clone();

        for (kind, _, definition) in &current {
            let ref_fields = REFERENCE_FIELDS
                .iter()
                .find(|(k, _)| k == kind)
                .map(|(_, f)| *f)
                .unwrap_or(&[]);

            if let Some(obj) = definition.as_object() {
                // String reference fields
                for field in ref_fields {
                    if let Some(ref_name) = obj.get(*field).and_then(|v| v.as_str()) {
                        for (ak, an, av) in all_resources {
                            if an == ref_name && !seen.contains_key(&(*ak, an.clone())) {
                                result.push((*ak, an.clone(), av.clone()));
                                seen.insert((*ak, an.clone()), true);
                                changed = true;
                            }
                        }
                    }
                }

                // Array reference fields (e.g., KB's knowledgeSources)
                for (ref_kind, field_name, target_kind) in ARRAY_REFERENCE_FIELDS {
                    if ref_kind != kind {
                        continue;
                    }
                    if let Some(arr) = obj.get(*field_name).and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(ref_name) = item
                                .as_object()
                                .and_then(|o| o.get("name"))
                                .and_then(|n| n.as_str())
                            {
                                for (ak, an, av) in all_resources {
                                    if *ak == *target_kind
                                        && an == ref_name
                                        && !seen.contains_key(&(*ak, an.clone()))
                                    {
                                        result.push((*ak, an.clone(), av.clone()));
                                        seen.insert((*ak, an.clone()), true);
                                        changed = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

/// Expand a resource selection recursively: include dependencies (upward)
/// and children (downward via containment arrays).
///
/// `selected` — initial resources with their JSON definitions
/// `all_local` — all locally available resources to draw from
///
/// Returns the expanded set (including originals), deduplicated.
pub fn expand_recursive(
    selected: &[(ResourceKind, String, Value)],
    all_local: &[(ResourceKind, String, Value)],
) -> Vec<(ResourceKind, String, Value)> {
    let mut result: Vec<(ResourceKind, String, Value)> = selected.to_vec();
    let mut seen: std::collections::HashSet<(ResourceKind, String)> =
        std::collections::HashSet::new();

    for (kind, name, _) in &result {
        seen.insert((*kind, name.clone()));
    }

    let mut changed = true;
    while changed {
        changed = false;
        let current: Vec<_> = result.clone();

        for (kind, _, definition) in &current {
            if let Some(obj) = definition.as_object() {
                // Upward: string reference fields (Indexer→DS, KS→Index, etc.)
                let ref_fields = REFERENCE_FIELDS
                    .iter()
                    .find(|(k, _)| k == kind)
                    .map(|(_, f)| *f)
                    .unwrap_or(&[]);

                for field in ref_fields {
                    if let Some(ref_name) = obj.get(*field).and_then(|v| v.as_str()) {
                        if ref_name.is_empty() {
                            continue;
                        }
                        for (ak, an, av) in all_local {
                            if an == ref_name && !seen.contains(&(*ak, an.clone())) {
                                result.push((*ak, an.clone(), av.clone()));
                                seen.insert((*ak, an.clone()));
                                changed = true;
                            }
                        }
                    }
                }

                // Both directions: array reference fields (KB→KS, KS→KB)
                for (ref_kind, field_name, target_kind) in ARRAY_REFERENCE_FIELDS {
                    if ref_kind == kind {
                        // Downward: parent contains children (KB→KS)
                        if let Some(arr) = obj.get(*field_name).and_then(|v| v.as_array()) {
                            for item in arr {
                                if let Some(ref_name) = item
                                    .as_object()
                                    .and_then(|o| o.get("name"))
                                    .and_then(|n| n.as_str())
                                {
                                    for (ak, an, av) in all_local {
                                        if *ak == *target_kind
                                            && an == ref_name
                                            && !seen.contains(&(*ak, an.clone()))
                                        {
                                            result.push((*ak, an.clone(), av.clone()));
                                            seen.insert((*ak, an.clone()));
                                            changed = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    if target_kind == kind {
                        // Upward: child references parent (KS→KB)
                        // Find parents that contain this resource in their array
                        for (ak, an, av) in all_local {
                            if *ak == *ref_kind && !seen.contains(&(*ak, an.clone())) {
                                if let Some(arr) = av
                                    .as_object()
                                    .and_then(|o| o.get(*field_name))
                                    .and_then(|v| v.as_array())
                                {
                                    let resource_name =
                                        obj.get("name").and_then(|n| n.as_str()).unwrap_or("");
                                    let contains = arr.iter().any(|item| {
                                        item.as_object()
                                            .and_then(|o| o.get("name"))
                                            .and_then(|n| n.as_str())
                                            == Some(resource_name)
                                    });
                                    if contains {
                                        result.push((*ak, an.clone(), av.clone()));
                                        seen.insert((*ak, an.clone()));
                                        changed = true;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_name_map_insert_and_get() {
        let mut map = NameMap::new();
        map.insert(ResourceKind::Index, "old-idx", "new-idx");
        assert_eq!(map.get(ResourceKind::Index, "old-idx"), Some("new-idx"));
        assert_eq!(map.get(ResourceKind::Index, "missing"), None);
        assert_eq!(map.get(ResourceKind::Indexer, "old-idx"), None);
    }

    #[test]
    fn test_name_map_from_suffix() {
        let resources = vec![
            (ResourceKind::Index, "my-index".to_string()),
            (ResourceKind::Indexer, "my-indexer".to_string()),
            (ResourceKind::DataSource, "my-ds".to_string()),
        ];
        let map = NameMap::from_suffix(&resources, "-v2");

        assert_eq!(
            map.get(ResourceKind::Index, "my-index"),
            Some("my-index-v2")
        );
        assert_eq!(
            map.get(ResourceKind::Indexer, "my-indexer"),
            Some("my-indexer-v2")
        );
        assert_eq!(map.get(ResourceKind::DataSource, "my-ds"), Some("my-ds-v2"));
    }

    #[test]
    fn test_name_map_from_answers_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("answers.json");
        std::fs::write(
            &path,
            r#"{
                "indexes": { "old-idx": "new-idx" },
                "indexers": { "old-ixer": "new-ixer" }
            }"#,
        )
        .unwrap();

        let map = NameMap::from_answers_file(&path).unwrap();
        assert_eq!(map.get(ResourceKind::Index, "old-idx"), Some("new-idx"));
        assert_eq!(map.get(ResourceKind::Indexer, "old-ixer"), Some("new-ixer"));
        assert_eq!(map.get(ResourceKind::DataSource, "anything"), None);
    }

    #[test]
    fn test_name_map_from_answers_file_missing_sections() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("answers.json");
        std::fs::write(&path, r#"{ "indexes": { "a": "b" } }"#).unwrap();

        let map = NameMap::from_answers_file(&path).unwrap();
        assert_eq!(map.get(ResourceKind::Index, "a"), Some("b"));
        assert_eq!(map.get(ResourceKind::Indexer, "anything"), None);
    }

    #[test]
    fn test_rewrite_references_indexer() {
        let mut name_map = NameMap::new();
        name_map.insert(ResourceKind::DataSource, "old-ds", "new-ds");
        name_map.insert(ResourceKind::Index, "old-idx", "new-idx");
        name_map.insert(ResourceKind::Skillset, "old-sk", "new-sk");

        let mut definition = json!({
            "name": "my-indexer",
            "dataSourceName": "old-ds",
            "targetIndexName": "old-idx",
            "skillsetName": "old-sk"
        });

        let warnings = rewrite_references(ResourceKind::Indexer, &mut definition, &name_map);

        assert!(warnings.is_empty());
        assert_eq!(definition["dataSourceName"], "new-ds");
        assert_eq!(definition["targetIndexName"], "new-idx");
        assert_eq!(definition["skillsetName"], "new-sk");
    }

    #[test]
    fn test_rewrite_references_knowledge_source() {
        let mut name_map = NameMap::new();
        name_map.insert(ResourceKind::Index, "old-idx", "new-idx");
        name_map.insert(ResourceKind::KnowledgeBase, "old-kb", "new-kb");

        let mut definition = json!({
            "name": "my-ks",
            "indexName": "old-idx",
            "knowledgeBaseName": "old-kb"
        });

        let warnings =
            rewrite_references(ResourceKind::KnowledgeSource, &mut definition, &name_map);

        assert!(warnings.is_empty());
        assert_eq!(definition["indexName"], "new-idx");
        assert_eq!(definition["knowledgeBaseName"], "new-kb");
    }

    #[test]
    fn test_rewrite_references_warns_on_unmapped() {
        let name_map = NameMap::new();

        let mut definition = json!({
            "name": "my-indexer",
            "dataSourceName": "some-ds",
            "targetIndexName": "some-idx",
            "skillsetName": ""
        });

        let warnings = rewrite_references(ResourceKind::Indexer, &mut definition, &name_map);

        // Two warnings: one for dataSourceName, one for targetIndexName
        // skillsetName is empty, no warning
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("some-ds"));
        assert!(warnings[1].contains("some-idx"));
    }

    #[test]
    fn test_rewrite_references_non_referencing_kind() {
        let name_map = NameMap::new();
        let mut definition = json!({ "name": "my-index", "fields": [] });

        let warnings = rewrite_references(ResourceKind::Index, &mut definition, &name_map);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_compute_dependency_closure_includes_deps() {
        let selected = vec![(
            ResourceKind::Indexer,
            "my-indexer".to_string(),
            json!({
                "name": "my-indexer",
                "dataSourceName": "my-ds",
                "targetIndexName": "my-idx",
                "skillsetName": ""
            }),
        )];

        let all = vec![
            (
                ResourceKind::DataSource,
                "my-ds".to_string(),
                json!({ "name": "my-ds", "type": "azureblob" }),
            ),
            (
                ResourceKind::Index,
                "my-idx".to_string(),
                json!({ "name": "my-idx", "fields": [] }),
            ),
            (
                ResourceKind::Indexer,
                "my-indexer".to_string(),
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "my-ds",
                    "targetIndexName": "my-idx",
                    "skillsetName": ""
                }),
            ),
            (
                ResourceKind::Index,
                "unrelated-idx".to_string(),
                json!({ "name": "unrelated-idx" }),
            ),
        ];

        let result = compute_dependency_closure(&selected, &all);
        assert_eq!(result.len(), 3); // indexer + ds + idx
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        assert!(names.contains(&"my-indexer"));
        assert!(names.contains(&"my-ds"));
        assert!(names.contains(&"my-idx"));
        assert!(!names.contains(&"unrelated-idx"));
    }

    #[test]
    fn test_compute_dependency_closure_deduplicates() {
        let selected = vec![
            (
                ResourceKind::Indexer,
                "ixer-1".to_string(),
                json!({ "name": "ixer-1", "dataSourceName": "shared-ds", "targetIndexName": "idx-1" }),
            ),
            (
                ResourceKind::Indexer,
                "ixer-2".to_string(),
                json!({ "name": "ixer-2", "dataSourceName": "shared-ds", "targetIndexName": "idx-2" }),
            ),
        ];

        let all = vec![
            (
                ResourceKind::DataSource,
                "shared-ds".to_string(),
                json!({ "name": "shared-ds" }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1" }),
            ),
            (
                ResourceKind::Index,
                "idx-2".to_string(),
                json!({ "name": "idx-2" }),
            ),
            (
                ResourceKind::Indexer,
                "ixer-1".to_string(),
                json!({ "name": "ixer-1", "dataSourceName": "shared-ds", "targetIndexName": "idx-1" }),
            ),
            (
                ResourceKind::Indexer,
                "ixer-2".to_string(),
                json!({ "name": "ixer-2", "dataSourceName": "shared-ds", "targetIndexName": "idx-2" }),
            ),
        ];

        let result = compute_dependency_closure(&selected, &all);
        // 2 indexers + 1 shared ds + 2 indexes = 5
        assert_eq!(result.len(), 5);

        // shared-ds should appear exactly once
        let ds_count = result.iter().filter(|(_, n, _)| n == "shared-ds").count();
        assert_eq!(ds_count, 1);
    }

    #[test]
    fn test_from_suffix_empty() {
        let resources: Vec<(ResourceKind, String)> = vec![];
        let map = NameMap::from_suffix(&resources, "-test");
        assert_eq!(map.get(ResourceKind::Index, "anything"), None);
    }

    #[test]
    fn test_rewrite_references_kb_knowledge_sources_array() {
        let mut name_map = NameMap::new();
        name_map.insert(ResourceKind::KnowledgeSource, "ks-1", "ks-1-v2");
        name_map.insert(ResourceKind::KnowledgeSource, "ks-2", "ks-2-v2");

        let mut definition = json!({
            "name": "my-kb",
            "knowledgeSources": [
                {"name": "ks-1"},
                {"name": "ks-2"}
            ]
        });

        let warnings = rewrite_references(ResourceKind::KnowledgeBase, &mut definition, &name_map);

        assert!(warnings.is_empty());
        let sources = definition["knowledgeSources"].as_array().unwrap();
        assert_eq!(sources[0]["name"], "ks-1-v2");
        assert_eq!(sources[1]["name"], "ks-2-v2");
    }

    #[test]
    fn test_rewrite_references_kb_warns_on_unmapped_ks() {
        let name_map = NameMap::new();

        let mut definition = json!({
            "name": "my-kb",
            "knowledgeSources": [
                {"name": "ks-unmapped"}
            ]
        });

        let warnings = rewrite_references(ResourceKind::KnowledgeBase, &mut definition, &name_map);

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ks-unmapped"));
        assert!(warnings[0].contains("knowledgeSources"));
    }

    #[test]
    fn test_rewrite_references_kb_no_knowledge_sources_field() {
        let name_map = NameMap::new();
        let mut definition = json!({
            "name": "my-kb",
            "description": "No KS array"
        });

        let warnings = rewrite_references(ResourceKind::KnowledgeBase, &mut definition, &name_map);

        assert!(warnings.is_empty());
    }

    #[test]
    fn test_compute_dependency_closure_includes_kb_knowledge_sources() {
        let selected = vec![(
            ResourceKind::KnowledgeBase,
            "my-kb".to_string(),
            json!({
                "name": "my-kb",
                "knowledgeSources": [
                    {"name": "ks-1"},
                    {"name": "ks-2"}
                ]
            }),
        )];

        let all = vec![
            (
                ResourceKind::KnowledgeBase,
                "my-kb".to_string(),
                json!({
                    "name": "my-kb",
                    "knowledgeSources": [
                        {"name": "ks-1"},
                        {"name": "ks-2"}
                    ]
                }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-1".to_string(),
                json!({ "name": "ks-1", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-2".to_string(),
                json!({ "name": "ks-2", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1", "fields": [] }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-other".to_string(),
                json!({ "name": "ks-other", "indexName": "idx-2", "knowledgeBaseName": "other-kb" }),
            ),
        ];

        let result = compute_dependency_closure(&selected, &all);
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        // KB + ks-1 + ks-2 (from array) + idx-1 (from ks-1/ks-2 deps)
        assert!(names.contains(&"my-kb"));
        assert!(names.contains(&"ks-1"));
        assert!(names.contains(&"ks-2"));
        assert!(names.contains(&"idx-1"));
        assert!(!names.contains(&"ks-other"));
    }

    // === expand_recursive tests ===

    #[test]
    fn test_expand_recursive_includes_kb_ks_children() {
        let selected = vec![(
            ResourceKind::KnowledgeBase,
            "my-kb".to_string(),
            json!({
                "name": "my-kb",
                "knowledgeSources": [
                    {"name": "ks-1"},
                    {"name": "ks-2"}
                ]
            }),
        )];

        let all = vec![
            selected[0].clone(),
            (
                ResourceKind::KnowledgeSource,
                "ks-1".to_string(),
                json!({ "name": "ks-1", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-2".to_string(),
                json!({ "name": "ks-2", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1", "fields": [] }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        assert!(names.contains(&"my-kb"));
        assert!(names.contains(&"ks-1"));
        assert!(names.contains(&"ks-2"));
        assert!(names.contains(&"idx-1"));
    }

    #[test]
    fn test_expand_recursive_includes_indexer_dependencies() {
        let selected = vec![(
            ResourceKind::Indexer,
            "my-ixer".to_string(),
            json!({
                "name": "my-ixer",
                "dataSourceName": "my-ds",
                "targetIndexName": "my-idx",
                "skillsetName": "my-sk"
            }),
        )];

        let all = vec![
            selected[0].clone(),
            (
                ResourceKind::DataSource,
                "my-ds".to_string(),
                json!({ "name": "my-ds" }),
            ),
            (
                ResourceKind::Index,
                "my-idx".to_string(),
                json!({ "name": "my-idx" }),
            ),
            (
                ResourceKind::Skillset,
                "my-sk".to_string(),
                json!({ "name": "my-sk" }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        assert!(names.contains(&"my-ixer"));
        assert!(names.contains(&"my-ds"));
        assert!(names.contains(&"my-idx"));
        assert!(names.contains(&"my-sk"));
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_expand_recursive_both_directions() {
        // Starting from KS, should expand up to KB (parent) and Index (dep)
        let selected = vec![(
            ResourceKind::KnowledgeSource,
            "ks-1".to_string(),
            json!({ "name": "ks-1", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
        )];

        let all = vec![
            selected[0].clone(),
            (
                ResourceKind::KnowledgeBase,
                "my-kb".to_string(),
                json!({
                    "name": "my-kb",
                    "knowledgeSources": [{"name": "ks-1"}, {"name": "ks-2"}]
                }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-2".to_string(),
                json!({ "name": "ks-2", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1" }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        // ks-1 → idx-1 (string ref), ks-1 → my-kb (string ref: knowledgeBaseName)
        // my-kb → ks-2 (array ref), ks-2 → idx-1 (already seen)
        assert!(names.contains(&"ks-1"));
        assert!(names.contains(&"idx-1"));
        assert!(names.contains(&"my-kb"));
        assert!(names.contains(&"ks-2"));
    }

    #[test]
    fn test_expand_recursive_deduplicates() {
        let selected = vec![
            (
                ResourceKind::KnowledgeSource,
                "ks-1".to_string(),
                json!({ "name": "ks-1", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-2".to_string(),
                json!({ "name": "ks-2", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
        ];

        let all = vec![
            selected[0].clone(),
            selected[1].clone(),
            (
                ResourceKind::KnowledgeBase,
                "my-kb".to_string(),
                json!({
                    "name": "my-kb",
                    "knowledgeSources": [{"name": "ks-1"}, {"name": "ks-2"}]
                }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1" }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        // idx-1 and my-kb should appear exactly once each
        let idx_count = result.iter().filter(|(_, n, _)| n == "idx-1").count();
        let kb_count = result.iter().filter(|(_, n, _)| n == "my-kb").count();
        assert_eq!(idx_count, 1);
        assert_eq!(kb_count, 1);
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_expand_recursive_no_deps_no_children() {
        let selected = vec![(
            ResourceKind::Index,
            "my-idx".to_string(),
            json!({ "name": "my-idx", "fields": [] }),
        )];

        let all = vec![
            selected[0].clone(),
            (
                ResourceKind::Index,
                "other-idx".to_string(),
                json!({ "name": "other-idx" }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1, "my-idx");
    }

    #[test]
    fn test_expand_recursive_transitive_chain() {
        // KB → KS → Index (via transitive closure)
        let selected = vec![(
            ResourceKind::KnowledgeBase,
            "my-kb".to_string(),
            json!({
                "name": "my-kb",
                "knowledgeSources": [{"name": "ks-1"}]
            }),
        )];

        let all = vec![
            selected[0].clone(),
            (
                ResourceKind::KnowledgeSource,
                "ks-1".to_string(),
                json!({ "name": "ks-1", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1" }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        assert!(names.contains(&"my-kb"));
        assert!(names.contains(&"ks-1"));
        assert!(names.contains(&"idx-1"));
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn test_expand_recursive_empty_selected() {
        let all = vec![(
            ResourceKind::Index,
            "idx-1".to_string(),
            json!({ "name": "idx-1" }),
        )];
        let result = expand_recursive(&[], &all);
        assert!(result.is_empty());
    }

    #[test]
    fn test_expand_recursive_missing_dep_in_all_local() {
        // Indexer references a DS that doesn't exist in all_local — should silently skip
        let selected = vec![(
            ResourceKind::Indexer,
            "my-ixer".to_string(),
            json!({
                "name": "my-ixer",
                "dataSourceName": "missing-ds",
                "targetIndexName": "also-missing",
                "skillsetName": ""
            }),
        )];

        let result = expand_recursive(&selected, &selected);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1, "my-ixer");
    }

    #[test]
    fn test_rewrite_references_indexer_partial_mapping() {
        // Only DS is mapped; Index and Skillset are not → two warnings
        let mut name_map = NameMap::new();
        name_map.insert(ResourceKind::DataSource, "old-ds", "new-ds");

        let mut definition = json!({
            "name": "my-indexer",
            "dataSourceName": "old-ds",
            "targetIndexName": "unmapped-idx",
            "skillsetName": "unmapped-sk"
        });

        let warnings = rewrite_references(ResourceKind::Indexer, &mut definition, &name_map);

        assert_eq!(definition["dataSourceName"], "new-ds");
        assert_eq!(definition["targetIndexName"], "unmapped-idx"); // unchanged
        assert_eq!(definition["skillsetName"], "unmapped-sk"); // unchanged
        assert_eq!(warnings.len(), 2);
        assert!(warnings[0].contains("unmapped-idx"));
        assert!(warnings[1].contains("unmapped-sk"));
    }

    #[test]
    fn test_rewrite_references_null_definition() {
        let name_map = NameMap::new();
        let mut definition = serde_json::Value::Null;

        let warnings = rewrite_references(ResourceKind::Indexer, &mut definition, &name_map);
        assert!(warnings.is_empty());
    }

    #[test]
    fn test_rewrite_references_kb_mixed_mapped_and_unmapped() {
        let mut name_map = NameMap::new();
        name_map.insert(ResourceKind::KnowledgeSource, "ks-1", "ks-1-v2");
        // ks-2 is NOT in the map

        let mut definition = json!({
            "name": "my-kb",
            "knowledgeSources": [
                {"name": "ks-1"},
                {"name": "ks-2"}
            ]
        });

        let warnings = rewrite_references(ResourceKind::KnowledgeBase, &mut definition, &name_map);

        let sources = definition["knowledgeSources"].as_array().unwrap();
        assert_eq!(sources[0]["name"], "ks-1-v2"); // rewritten
        assert_eq!(sources[1]["name"], "ks-2"); // unchanged
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("ks-2"));
    }

    #[test]
    fn test_compute_dependency_closure_empty_selected() {
        let all = vec![(
            ResourceKind::Index,
            "idx-1".to_string(),
            json!({ "name": "idx-1" }),
        )];
        let result = compute_dependency_closure(&[], &all);
        assert!(result.is_empty());
    }

    #[test]
    fn test_compute_dependency_closure_empty_all() {
        let selected = vec![(
            ResourceKind::Indexer,
            "my-ixer".to_string(),
            json!({
                "name": "my-ixer",
                "dataSourceName": "my-ds",
                "targetIndexName": "my-idx"
            }),
        )];
        let result = compute_dependency_closure(&selected, &[]);
        // Only the originally-selected resource
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].1, "my-ixer");
    }

    #[test]
    fn test_rewrite_references_kb_empty_knowledge_sources_array() {
        let name_map = NameMap::new();
        let mut definition = json!({
            "name": "my-kb",
            "knowledgeSources": []
        });

        let warnings = rewrite_references(ResourceKind::KnowledgeBase, &mut definition, &name_map);

        assert!(warnings.is_empty());
        assert_eq!(definition["knowledgeSources"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_expand_recursive_ks_finds_parent_kb() {
        // Starting from a KS, expand should find its parent KB via knowledgeBaseName (string ref)
        // AND the KB should pull in sibling KS via knowledgeSources (array ref)
        let selected = vec![(
            ResourceKind::KnowledgeSource,
            "ks-1".to_string(),
            json!({ "name": "ks-1", "indexName": "idx-1", "knowledgeBaseName": "my-kb" }),
        )];

        let all = vec![
            selected[0].clone(),
            (
                ResourceKind::KnowledgeBase,
                "my-kb".to_string(),
                json!({
                    "name": "my-kb",
                    "knowledgeSources": [{"name": "ks-1"}, {"name": "ks-sibling"}]
                }),
            ),
            (
                ResourceKind::KnowledgeSource,
                "ks-sibling".to_string(),
                json!({ "name": "ks-sibling", "indexName": "idx-2", "knowledgeBaseName": "my-kb" }),
            ),
            (
                ResourceKind::Index,
                "idx-1".to_string(),
                json!({ "name": "idx-1" }),
            ),
            (
                ResourceKind::Index,
                "idx-2".to_string(),
                json!({ "name": "idx-2" }),
            ),
        ];

        let result = expand_recursive(&selected, &all);
        let names: Vec<_> = result.iter().map(|(_, n, _)| n.as_str()).collect();
        assert!(names.contains(&"ks-1"));
        assert!(names.contains(&"my-kb")); // parent KB
        assert!(names.contains(&"ks-sibling")); // sibling via KB's array
        assert!(names.contains(&"idx-1")); // ks-1's dep
        assert!(names.contains(&"idx-2")); // ks-sibling's dep
        assert_eq!(result.len(), 5);
    }

    #[test]
    fn test_rewrite_references_kb_knowledge_sources_not_an_array() {
        // knowledgeSources is an object instead of array — should not panic
        let name_map = NameMap::new();
        let mut definition = json!({
            "name": "my-kb",
            "knowledgeSources": {"name": "ks-1"}
        });

        let warnings = rewrite_references(ResourceKind::KnowledgeBase, &mut definition, &name_map);
        // Not an array, silently skipped
        assert!(warnings.is_empty());
    }
}
