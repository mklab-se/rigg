//! Managed resources map for knowledge source sub-resources
//!
//! Knowledge sources auto-provision sub-resources (index, indexer, data source, skillset)
//! listed in their `createdResources` field. This module tracks the ownership relationship
//! and routes files to the correct directories.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::Value;

use super::traits::ResourceKind;

/// Managed sub-resources provisioned by a knowledge source.
#[derive(Debug, Clone)]
pub struct ManagedResources {
    pub knowledge_source_name: String,
    pub index: Option<String>,
    pub indexer: Option<String>,
    pub datasource: Option<String>,
    pub skillset: Option<String>,
}

/// Type alias for the managed map: (ResourceKind, azure_name) -> ks_name
pub type ManagedMap = HashMap<(ResourceKind, String), String>;

/// Extract managed resources from a knowledge source JSON definition.
///
/// Searches `createdResources` (which may be nested under parameter blocks like
/// `azureBlobParameters`) for auto-provisioned resource names.
pub fn extract_managed_resources(ks_name: &str, ks_def: &Value) -> ManagedResources {
    let mut managed = ManagedResources {
        knowledge_source_name: ks_name.to_string(),
        index: None,
        indexer: None,
        datasource: None,
        skillset: None,
    };

    // createdResources can be at top level or nested under parameter blocks
    let created = ks_def.get("createdResources").or_else(|| {
        // Check common parameter blocks
        for key in &[
            "azureBlobParameters",
            "azureTableParameters",
            "sharePointParameters",
        ] {
            if let Some(params) = ks_def.get(key) {
                if let Some(cr) = params.get("createdResources") {
                    return Some(cr);
                }
            }
        }
        None
    });

    if let Some(created) = created {
        if let Some(obj) = created.as_object() {
            managed.index = obj.get("index").and_then(|v| v.as_str()).map(String::from);
            managed.indexer = obj
                .get("indexer")
                .and_then(|v| v.as_str())
                .map(String::from);
            managed.datasource = obj
                .get("datasource")
                .or_else(|| obj.get("dataSource"))
                .and_then(|v| v.as_str())
                .map(String::from);
            managed.skillset = obj
                .get("skillset")
                .and_then(|v| v.as_str())
                .map(String::from);
        }
    }

    managed
}

/// Build a managed map from all knowledge source definitions.
///
/// Returns a map of (ResourceKind, azure_name) -> ks_name, indicating which
/// resources are managed and by which knowledge source.
pub fn build_managed_map(knowledge_sources: &[(String, Value)]) -> ManagedMap {
    let mut map = ManagedMap::new();

    for (ks_name, ks_def) in knowledge_sources {
        let managed = extract_managed_resources(ks_name, ks_def);

        if let Some(ref name) = managed.index {
            map.insert((ResourceKind::Index, name.clone()), ks_name.clone());
        }
        if let Some(ref name) = managed.indexer {
            map.insert((ResourceKind::Indexer, name.clone()), ks_name.clone());
        }
        if let Some(ref name) = managed.datasource {
            map.insert((ResourceKind::DataSource, name.clone()), ks_name.clone());
        }
        if let Some(ref name) = managed.skillset {
            map.insert((ResourceKind::Skillset, name.clone()), ks_name.clone());
        }
    }

    map
}

/// Check if a resource is managed. Returns the managing KS name if so.
pub fn managing_ks<'a>(map: &'a ManagedMap, kind: ResourceKind, name: &str) -> Option<&'a String> {
    map.get(&(kind, name.to_string()))
}

/// Directory path relative to service root for a resource.
///
/// - Managed resources go under `agentic-retrieval/knowledge-sources/<ks-name>/`
/// - Knowledge sources themselves go under `agentic-retrieval/knowledge-sources/<ks-name>/`
/// - Standalone resources use their default directory (e.g., `search-management/indexes`)
pub fn resource_directory(kind: ResourceKind, name: &str, map: &ManagedMap) -> PathBuf {
    if kind == ResourceKind::KnowledgeSource {
        // KS itself goes in its own directory
        PathBuf::from("agentic-retrieval/knowledge-sources").join(name)
    } else if let Some(ks_name) = managing_ks(map, kind, name) {
        // Managed sub-resource goes in the parent KS directory
        PathBuf::from("agentic-retrieval/knowledge-sources").join(ks_name)
    } else {
        // Standalone resource
        PathBuf::from(kind.directory_name())
    }
}

/// Filename for a resource within its directory.
///
/// - KS definition: `<ks-name>.json`
/// - Managed sub-resource: `<ks-name>-<suffix>.json` where suffix is index/indexer/datasource/skillset
/// - Standalone: `<name>.json`
pub fn resource_filename(kind: ResourceKind, name: &str, map: &ManagedMap) -> String {
    if kind == ResourceKind::KnowledgeSource {
        // KS definition file
        format!("{}.json", name)
    } else if let Some(ks_name) = managing_ks(map, kind, name) {
        // Managed sub-resource: use the KS name with a type suffix
        let suffix = match kind {
            ResourceKind::Index => "index",
            ResourceKind::Indexer => "indexer",
            ResourceKind::DataSource => "datasource",
            ResourceKind::Skillset => "skillset",
            _ => "resource",
        };
        format!("{}-{}.json", ks_name, suffix)
    } else {
        // Standalone resource
        format!("{}.json", name)
    }
}

/// Find knowledge bases that reference a given knowledge source.
pub fn find_kb_references(ks_name: &str, kbs: &[(String, Value)]) -> Vec<String> {
    let mut refs = Vec::new();

    for (kb_name, kb_def) in kbs {
        if let Some(sources) = kb_def.get("knowledgeSources").and_then(|v| v.as_array()) {
            for source in sources {
                if let Some(source_name) = source
                    .as_object()
                    .and_then(|o| o.get("name"))
                    .and_then(|n| n.as_str())
                {
                    if source_name == ks_name {
                        refs.push(kb_name.clone());
                        break;
                    }
                }
            }
        }
    }

    refs
}

/// The resource kinds that are managed sub-resources of knowledge sources.
pub const MANAGED_SUB_RESOURCE_KINDS: &[ResourceKind] = &[
    ResourceKind::Index,
    ResourceKind::Indexer,
    ResourceKind::DataSource,
    ResourceKind::Skillset,
];

/// Read managed sub-resources from a KS directory on disk.
///
/// Returns a list of (ResourceKind, azure_name, Value) for each managed
/// sub-resource file found in the directory.
pub fn read_managed_sub_resources(
    ks_dir: &std::path::Path,
    ks_name: &str,
) -> Vec<(ResourceKind, String, Value)> {
    let mut results = Vec::new();

    let suffixes = [
        ("index", ResourceKind::Index),
        ("indexer", ResourceKind::Indexer),
        ("datasource", ResourceKind::DataSource),
        ("skillset", ResourceKind::Skillset),
    ];

    for (suffix, kind) in &suffixes {
        let filename = format!("{}-{}.json", ks_name, suffix);
        let path = ks_dir.join(&filename);
        if path.exists() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(value) = serde_json::from_str::<Value>(&content) {
                    let azure_name = value
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .to_string();
                    results.push((*kind, azure_name, value));
                }
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_managed_resources_top_level() {
        let ks_def = json!({
            "name": "test-ks",
            "createdResources": {
                "index": "test-ks-index",
                "indexer": "test-ks-indexer",
                "dataSource": "test-ks-datasource",
                "skillset": "test-ks-skillset"
            }
        });

        let managed = extract_managed_resources("test-ks", &ks_def);
        assert_eq!(managed.knowledge_source_name, "test-ks");
        assert_eq!(managed.index.as_deref(), Some("test-ks-index"));
        assert_eq!(managed.indexer.as_deref(), Some("test-ks-indexer"));
        assert_eq!(managed.datasource.as_deref(), Some("test-ks-datasource"));
        assert_eq!(managed.skillset.as_deref(), Some("test-ks-skillset"));
    }

    #[test]
    fn test_extract_managed_resources_nested() {
        let ks_def = json!({
            "name": "test-ks",
            "azureBlobParameters": {
                "containerName": "docs",
                "createdResources": {
                    "index": "test-ks-index",
                    "indexer": "test-ks-indexer",
                    "dataSource": "test-ks-datasource",
                    "skillset": "test-ks-skillset"
                }
            }
        });

        let managed = extract_managed_resources("test-ks", &ks_def);
        assert_eq!(managed.index.as_deref(), Some("test-ks-index"));
        assert_eq!(managed.indexer.as_deref(), Some("test-ks-indexer"));
    }

    #[test]
    fn test_extract_managed_resources_no_created() {
        let ks_def = json!({
            "name": "test-ks",
            "indexName": "my-idx"
        });

        let managed = extract_managed_resources("test-ks", &ks_def);
        assert!(managed.index.is_none());
        assert!(managed.indexer.is_none());
        assert!(managed.datasource.is_none());
        assert!(managed.skillset.is_none());
    }

    #[test]
    fn test_extract_managed_resources_partial() {
        let ks_def = json!({
            "name": "test-ks",
            "createdResources": {
                "index": "test-ks-index"
            }
        });

        let managed = extract_managed_resources("test-ks", &ks_def);
        assert_eq!(managed.index.as_deref(), Some("test-ks-index"));
        assert!(managed.indexer.is_none());
    }

    #[test]
    fn test_build_managed_map() {
        let knowledge_sources = vec![
            (
                "ks-1".to_string(),
                json!({
                    "name": "ks-1",
                    "createdResources": {
                        "index": "ks-1-index",
                        "indexer": "ks-1-indexer",
                        "dataSource": "ks-1-datasource",
                        "skillset": "ks-1-skillset"
                    }
                }),
            ),
            (
                "ks-2".to_string(),
                json!({
                    "name": "ks-2",
                    "createdResources": {
                        "index": "ks-2-index"
                    }
                }),
            ),
        ];

        let map = build_managed_map(&knowledge_sources);

        assert_eq!(
            managing_ks(&map, ResourceKind::Index, "ks-1-index"),
            Some(&"ks-1".to_string())
        );
        assert_eq!(
            managing_ks(&map, ResourceKind::Indexer, "ks-1-indexer"),
            Some(&"ks-1".to_string())
        );
        assert_eq!(
            managing_ks(&map, ResourceKind::Index, "ks-2-index"),
            Some(&"ks-2".to_string())
        );
        // Standalone resource
        assert_eq!(
            managing_ks(&map, ResourceKind::Index, "standalone-idx"),
            None
        );
    }

    #[test]
    fn test_resource_directory_managed() {
        let mut map = ManagedMap::new();
        map.insert(
            (ResourceKind::Index, "ks-1-index".to_string()),
            "ks-1".to_string(),
        );

        assert_eq!(
            resource_directory(ResourceKind::Index, "ks-1-index", &map),
            PathBuf::from("agentic-retrieval/knowledge-sources/ks-1")
        );
    }

    #[test]
    fn test_resource_directory_standalone() {
        let map = ManagedMap::new();
        assert_eq!(
            resource_directory(ResourceKind::Index, "my-index", &map),
            PathBuf::from("search-management/indexes")
        );
    }

    #[test]
    fn test_resource_directory_ks() {
        let map = ManagedMap::new();
        assert_eq!(
            resource_directory(ResourceKind::KnowledgeSource, "test-ks", &map),
            PathBuf::from("agentic-retrieval/knowledge-sources/test-ks")
        );
    }

    #[test]
    fn test_resource_filename_managed() {
        let mut map = ManagedMap::new();
        map.insert(
            (ResourceKind::Index, "ks-1-index".to_string()),
            "ks-1".to_string(),
        );
        map.insert(
            (ResourceKind::Indexer, "ks-1-indexer".to_string()),
            "ks-1".to_string(),
        );
        map.insert(
            (ResourceKind::DataSource, "ks-1-datasource".to_string()),
            "ks-1".to_string(),
        );
        map.insert(
            (ResourceKind::Skillset, "ks-1-skillset".to_string()),
            "ks-1".to_string(),
        );

        assert_eq!(
            resource_filename(ResourceKind::Index, "ks-1-index", &map),
            "ks-1-index.json"
        );
        assert_eq!(
            resource_filename(ResourceKind::Indexer, "ks-1-indexer", &map),
            "ks-1-indexer.json"
        );
        assert_eq!(
            resource_filename(ResourceKind::DataSource, "ks-1-datasource", &map),
            "ks-1-datasource.json"
        );
        assert_eq!(
            resource_filename(ResourceKind::Skillset, "ks-1-skillset", &map),
            "ks-1-skillset.json"
        );
    }

    #[test]
    fn test_resource_filename_standalone() {
        let map = ManagedMap::new();
        assert_eq!(
            resource_filename(ResourceKind::Index, "my-index", &map),
            "my-index.json"
        );
    }

    #[test]
    fn test_resource_filename_ks() {
        let map = ManagedMap::new();
        assert_eq!(
            resource_filename(ResourceKind::KnowledgeSource, "test-ks", &map),
            "test-ks.json"
        );
    }

    #[test]
    fn test_find_kb_references() {
        let kbs = vec![
            (
                "kb-1".to_string(),
                json!({
                    "name": "kb-1",
                    "knowledgeSources": [
                        {"name": "ks-1"},
                        {"name": "ks-2"}
                    ]
                }),
            ),
            (
                "kb-2".to_string(),
                json!({
                    "name": "kb-2",
                    "knowledgeSources": [
                        {"name": "ks-3"}
                    ]
                }),
            ),
        ];

        let refs = find_kb_references("ks-1", &kbs);
        assert_eq!(refs, vec!["kb-1"]);

        let refs = find_kb_references("ks-3", &kbs);
        assert_eq!(refs, vec!["kb-2"]);

        let refs = find_kb_references("ks-missing", &kbs);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_find_kb_references_no_sources_array() {
        let kbs = vec![(
            "kb-1".to_string(),
            json!({
                "name": "kb-1"
            }),
        )];

        let refs = find_kb_references("ks-1", &kbs);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_build_managed_map_empty() {
        let map = build_managed_map(&[]);
        assert!(map.is_empty());
    }

    #[test]
    fn test_read_managed_sub_resources() {
        let dir = tempfile::tempdir().unwrap();
        let ks_dir = dir.path().join("test-ks");
        std::fs::create_dir_all(&ks_dir).unwrap();

        // Write a managed index file
        std::fs::write(
            ks_dir.join("test-ks-index.json"),
            r#"{"name": "test-ks-index", "fields": []}"#,
        )
        .unwrap();

        // Write a managed skillset file
        std::fs::write(
            ks_dir.join("test-ks-skillset.json"),
            r#"{"name": "test-ks-skillset", "skills": []}"#,
        )
        .unwrap();

        let results = read_managed_sub_resources(&ks_dir, "test-ks");
        assert_eq!(results.len(), 2);

        let index = results.iter().find(|(k, _, _)| *k == ResourceKind::Index);
        assert!(index.is_some());
        assert_eq!(index.unwrap().1, "test-ks-index");

        let skillset = results
            .iter()
            .find(|(k, _, _)| *k == ResourceKind::Skillset);
        assert!(skillset.is_some());
        assert_eq!(skillset.unwrap().1, "test-ks-skillset");
    }

    #[test]
    fn test_read_managed_sub_resources_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let ks_dir = dir.path().join("test-ks");
        std::fs::create_dir_all(&ks_dir).unwrap();

        let results = read_managed_sub_resources(&ks_dir, "test-ks");
        assert!(results.is_empty());
    }

    #[test]
    fn test_multiple_ks_managed_map() {
        let knowledge_sources = vec![
            (
                "ks-a".to_string(),
                json!({
                    "name": "ks-a",
                    "createdResources": {
                        "index": "ks-a-index",
                        "indexer": "ks-a-indexer"
                    }
                }),
            ),
            (
                "ks-b".to_string(),
                json!({
                    "name": "ks-b",
                    "createdResources": {
                        "index": "ks-b-index",
                        "indexer": "ks-b-indexer"
                    }
                }),
            ),
        ];

        let map = build_managed_map(&knowledge_sources);

        assert_eq!(
            managing_ks(&map, ResourceKind::Index, "ks-a-index"),
            Some(&"ks-a".to_string())
        );
        assert_eq!(
            managing_ks(&map, ResourceKind::Index, "ks-b-index"),
            Some(&"ks-b".to_string())
        );
        // No cross-contamination
        assert_ne!(
            managing_ks(&map, ResourceKind::Index, "ks-a-index"),
            Some(&"ks-b".to_string())
        );
    }
}
