//! Integration and filesystem tests for the copy module.

use serde_json::json;

use rigg_core::resources::ResourceKind;

use super::knowledge_source::copy_knowledge_source;
use super::standalone::copy_standalone_resource;

#[test]
fn test_copy_knowledge_source_builds_correct_name_map() {
    // Verify the name map logic by testing the replacen behavior
    let source = "test-ks";
    let target = "test-ks-v2";

    let managed_names = vec![
        "test-ks-index",
        "test-ks-indexer",
        "test-ks-datasource",
        "test-ks-skillset",
    ];

    let expected = vec![
        "test-ks-v2-index",
        "test-ks-v2-indexer",
        "test-ks-v2-datasource",
        "test-ks-v2-skillset",
    ];

    for (old, exp) in managed_names.iter().zip(expected.iter()) {
        let new_name = old.replacen(source, target, 1);
        assert_eq!(&new_name, exp, "Failed for {}", old);
    }
}

#[test]
fn test_copy_knowledge_source_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let service_dir = dir.path().to_path_buf();
    let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");

    // Create source KS directory with definition and managed sub-resources
    let source_dir = ks_base.join("my-ks");
    std::fs::create_dir_all(&source_dir).unwrap();

    std::fs::write(
        source_dir.join("my-ks.json"),
        serde_json::to_string_pretty(&json!({
            "name": "my-ks",
            "kind": "azureBlob",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>",
                "createdResources": {
                    "datasource": "my-ks-datasource",
                    "indexer": "my-ks-indexer",
                    "skillset": "my-ks-skillset",
                    "index": "my-ks-index"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(
        source_dir.join("my-ks-index.json"),
        serde_json::to_string_pretty(&json!({
            "name": "my-ks-index",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}]
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(
        source_dir.join("my-ks-indexer.json"),
        serde_json::to_string_pretty(&json!({
            "name": "my-ks-indexer",
            "dataSourceName": "my-ks-datasource",
            "targetIndexName": "my-ks-index",
            "skillsetName": "my-ks-skillset"
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(
        source_dir.join("my-ks-datasource.json"),
        serde_json::to_string_pretty(&json!({
            "name": "my-ks-datasource",
            "type": "azureblob"
        }))
        .unwrap(),
    )
    .unwrap();

    std::fs::write(
        source_dir.join("my-ks-skillset.json"),
        serde_json::to_string_pretty(&json!({
            "name": "my-ks-skillset",
            "skills": [],
            "indexProjections": {
                "selectors": [{
                    "targetIndexName": "my-ks-index",
                    "parentKeyFieldName": "parent_id",
                    "sourceContext": "/document/pages/*",
                    "mappings": []
                }],
                "parameters": {"projectionMode": "skipIndexingParentDocuments"}
            }
        }))
        .unwrap(),
    )
    .unwrap();

    // Run copy
    copy_knowledge_source(&service_dir, "my-ks", "my-ks-v2").unwrap();

    // Verify target directory was created
    let target_dir = ks_base.join("my-ks-v2");
    assert!(target_dir.exists());

    // Verify KS definition
    let ks_content = std::fs::read_to_string(target_dir.join("my-ks-v2.json")).unwrap();
    let ks: serde_json::Value = serde_json::from_str(&ks_content).unwrap();
    assert_eq!(ks["name"], "my-ks-v2");
    let cr = &ks["azureBlobParameters"]["createdResources"];
    assert_eq!(cr["index"], "my-ks-v2-index");
    assert_eq!(cr["indexer"], "my-ks-v2-indexer");
    assert_eq!(cr["datasource"], "my-ks-v2-datasource");
    assert_eq!(cr["skillset"], "my-ks-v2-skillset");

    // Verify managed index
    let idx_content = std::fs::read_to_string(target_dir.join("my-ks-v2-index.json")).unwrap();
    let idx: serde_json::Value = serde_json::from_str(&idx_content).unwrap();
    assert_eq!(idx["name"], "my-ks-v2-index");

    // Verify managed indexer (cross-references rewritten)
    let ixer_content = std::fs::read_to_string(target_dir.join("my-ks-v2-indexer.json")).unwrap();
    let ixer: serde_json::Value = serde_json::from_str(&ixer_content).unwrap();
    assert_eq!(ixer["name"], "my-ks-v2-indexer");
    assert_eq!(ixer["dataSourceName"], "my-ks-v2-datasource");
    assert_eq!(ixer["targetIndexName"], "my-ks-v2-index");
    assert_eq!(ixer["skillsetName"], "my-ks-v2-skillset");

    // Verify managed datasource
    let ds_content = std::fs::read_to_string(target_dir.join("my-ks-v2-datasource.json")).unwrap();
    let ds: serde_json::Value = serde_json::from_str(&ds_content).unwrap();
    assert_eq!(ds["name"], "my-ks-v2-datasource");

    // Verify managed skillset (indexProjections rewritten)
    let sk_content = std::fs::read_to_string(target_dir.join("my-ks-v2-skillset.json")).unwrap();
    let sk: serde_json::Value = serde_json::from_str(&sk_content).unwrap();
    assert_eq!(sk["name"], "my-ks-v2-skillset");
    assert_eq!(
        sk["indexProjections"]["selectors"][0]["targetIndexName"],
        "my-ks-v2-index"
    );

    // Verify source is untouched
    let source_ks = std::fs::read_to_string(source_dir.join("my-ks.json")).unwrap();
    let source_ks: serde_json::Value = serde_json::from_str(&source_ks).unwrap();
    assert_eq!(source_ks["name"], "my-ks");
}

#[test]
fn test_copy_standalone_resource_filesystem() {
    let dir = tempfile::tempdir().unwrap();
    let service_dir = dir.path().to_path_buf();
    let index_dir = service_dir.join("search-management/indexes");
    std::fs::create_dir_all(&index_dir).unwrap();

    std::fs::write(
        index_dir.join("my-index.json"),
        serde_json::to_string_pretty(&json!({
            "name": "my-index",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}]
        }))
        .unwrap(),
    )
    .unwrap();

    copy_standalone_resource(&service_dir, ResourceKind::Index, "my-index", "my-index-v2").unwrap();

    // Verify target file
    let target_content = std::fs::read_to_string(index_dir.join("my-index-v2.json")).unwrap();
    let target: serde_json::Value = serde_json::from_str(&target_content).unwrap();
    assert_eq!(target["name"], "my-index-v2");
    assert!(target["fields"].is_array());

    // Verify source is untouched
    let source_content = std::fs::read_to_string(index_dir.join("my-index.json")).unwrap();
    let source: serde_json::Value = serde_json::from_str(&source_content).unwrap();
    assert_eq!(source["name"], "my-index");
}

#[test]
fn test_copy_standalone_resource_target_exists() {
    let dir = tempfile::tempdir().unwrap();
    let service_dir = dir.path().to_path_buf();
    let index_dir = service_dir.join("search-management/indexes");
    std::fs::create_dir_all(&index_dir).unwrap();

    std::fs::write(index_dir.join("src.json"), r#"{"name":"src","fields":[]}"#).unwrap();
    std::fs::write(index_dir.join("dst.json"), r#"{"name":"dst","fields":[]}"#).unwrap();

    let result = copy_standalone_resource(&service_dir, ResourceKind::Index, "src", "dst");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[test]
fn test_copy_knowledge_source_target_exists() {
    let dir = tempfile::tempdir().unwrap();
    let service_dir = dir.path().to_path_buf();
    let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");

    // Create source
    let source_dir = ks_base.join("src-ks");
    std::fs::create_dir_all(&source_dir).unwrap();
    std::fs::write(
        source_dir.join("src-ks.json"),
        r#"{"name":"src-ks","kind":"azureBlob"}"#,
    )
    .unwrap();

    // Create target (conflict)
    let target_dir = ks_base.join("dst-ks");
    std::fs::create_dir_all(&target_dir).unwrap();

    let result = copy_knowledge_source(&service_dir, "src-ks", "dst-ks");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[test]
fn test_copy_knowledge_source_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let service_dir = dir.path().to_path_buf();
    let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");
    std::fs::create_dir_all(&ks_base).unwrap();

    let result = copy_knowledge_source(&service_dir, "nonexistent", "target");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}
