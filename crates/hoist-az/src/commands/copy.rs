//! Copy resources locally under new names
//!
//! A local-only operation: reads files, rewrites names and cross-references,
//! and writes new files. No network calls — push separately after copying.

use anyhow::{bail, Result};
use colored::Colorize;

use hoist_core::copy::{rewrite_references, NameMap};
use hoist_core::normalize::format_json;
use hoist_core::resources::managed;
use hoist_core::resources::ResourceKind;

use crate::commands::load_config_and_env;

#[allow(clippy::too_many_arguments)]
pub fn run(
    source: &str,
    target: &str,
    knowledgesource: bool,
    knowledgebase: bool,
    index: bool,
    indexer: bool,
    datasource: bool,
    skillset: bool,
    synonymmap: bool,
    alias: bool,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, _config, env) = load_config_and_env(env_override)?;

    let search_svc = env
        .primary_search_service()
        .ok_or_else(|| anyhow::anyhow!("No search service configured"))?;
    let service_dir = env.search_service_dir(&project_root, search_svc);

    if knowledgesource {
        copy_knowledge_source(&service_dir, source, target)
    } else {
        let kind = if knowledgebase {
            ResourceKind::KnowledgeBase
        } else if index {
            ResourceKind::Index
        } else if indexer {
            ResourceKind::Indexer
        } else if datasource {
            ResourceKind::DataSource
        } else if skillset {
            ResourceKind::Skillset
        } else if synonymmap {
            ResourceKind::SynonymMap
        } else if alias {
            ResourceKind::Alias
        } else {
            bail!("Specify a resource type (e.g., --knowledgesource, --index)");
        };
        copy_standalone_resource(&service_dir, kind, source, target)
    }
}

/// Copy a knowledge source and all its managed sub-resources under new names.
fn copy_knowledge_source(service_dir: &std::path::Path, source: &str, target: &str) -> Result<()> {
    let ks_base = service_dir.join("knowledge-sources");
    let source_dir = ks_base.join(source);
    let target_dir = ks_base.join(target);

    if !source_dir.exists() {
        bail!(
            "Knowledge source '{}' not found at {}",
            source,
            source_dir.display()
        );
    }
    if target_dir.exists() {
        bail!(
            "Target '{}' already exists at {}",
            target,
            target_dir.display()
        );
    }

    // Read source KS definition
    let ks_file = source_dir.join(format!("{}.json", source));
    if !ks_file.exists() {
        bail!("KS definition not found at {}", ks_file.display());
    }
    let ks_content = std::fs::read_to_string(&ks_file)?;
    let ks_def: serde_json::Value = serde_json::from_str(&ks_content)?;

    // Extract managed resources to build name map
    let managed = managed::extract_managed_resources(source, &ks_def);

    let mut name_map = NameMap::new();
    name_map.insert(ResourceKind::KnowledgeSource, source, target);

    if let Some(ref old_name) = managed.index {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::Index, old_name, &new_name);
    }
    if let Some(ref old_name) = managed.indexer {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::Indexer, old_name, &new_name);
    }
    if let Some(ref old_name) = managed.datasource {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::DataSource, old_name, &new_name);
    }
    if let Some(ref old_name) = managed.skillset {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::Skillset, old_name, &new_name);
    }

    // Create target directory
    std::fs::create_dir_all(&target_dir)?;

    println!("Copying knowledge source '{}' -> '{}':", source, target);

    // Copy and rewrite KS definition
    let mut new_ks_def = ks_def.clone();
    if let Some(obj) = new_ks_def.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(target.to_string()),
        );
    }
    let warnings = rewrite_references(ResourceKind::KnowledgeSource, &mut new_ks_def, &name_map);
    for w in &warnings {
        println!("  {} {}", "warning:".yellow(), w);
    }
    rewrite_created_resources(&mut new_ks_def, source, target);

    std::fs::write(
        target_dir.join(format!("{}.json", target)),
        format_json(&new_ks_def),
    )?;
    println!("  {} Knowledge Source '{}'", "+".green(), target);

    // Copy and rewrite managed sub-resources
    let suffixes = [
        ("index", ResourceKind::Index),
        ("indexer", ResourceKind::Indexer),
        ("datasource", ResourceKind::DataSource),
        ("skillset", ResourceKind::Skillset),
    ];

    for (suffix, kind) in &suffixes {
        let source_file = source_dir.join(format!("{}-{}.json", source, suffix));
        if !source_file.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&source_file)?;
        let mut def: serde_json::Value = serde_json::from_str(&content)?;

        let old_name = def
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let new_name = name_map
            .get(*kind, &old_name)
            .map(|s| s.to_string())
            .unwrap_or_else(|| old_name.replacen(source, target, 1));

        // Update name field
        if let Some(obj) = def.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(new_name.clone()),
            );
        }

        // Rewrite cross-references (e.g., indexer's dataSourceName, targetIndexName)
        let warnings = rewrite_references(*kind, &mut def, &name_map);
        for w in &warnings {
            println!("  {} {}", "warning:".yellow(), w);
        }

        // For skillsets, also rewrite indexProjections.selectors[].targetIndexName
        if *kind == ResourceKind::Skillset {
            rewrite_index_projections(&mut def, source, target);
        }

        let target_filename = format!("{}-{}.json", target, suffix);
        std::fs::write(target_dir.join(&target_filename), format_json(&def))?;
        println!("  {} {} '{}'", "+".green(), kind.display_name(), new_name);
    }

    println!();
    println!("Copy complete. Run 'hoist push --knowledgesources' to push to Azure.");

    Ok(())
}

/// Copy a standalone (non-managed) resource under a new name.
fn copy_standalone_resource(
    service_dir: &std::path::Path,
    kind: ResourceKind,
    source: &str,
    target: &str,
) -> Result<()> {
    let resource_dir = service_dir.join(kind.directory_name());
    let source_file = resource_dir.join(format!("{}.json", source));
    let target_file = resource_dir.join(format!("{}.json", target));

    if !source_file.exists() {
        bail!(
            "{} '{}' not found at {}",
            kind.display_name(),
            source,
            source_file.display()
        );
    }
    if target_file.exists() {
        bail!(
            "Target '{}' already exists at {}",
            target,
            target_file.display()
        );
    }

    let content = std::fs::read_to_string(&source_file)?;
    let mut def: serde_json::Value = serde_json::from_str(&content)?;

    // Update name field
    if let Some(obj) = def.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(target.to_string()),
        );
    }

    std::fs::write(&target_file, format_json(&def))?;
    println!(
        "  {} {} '{}' (copied from '{}')",
        "+".green(),
        kind.display_name(),
        target,
        source
    );

    Ok(())
}

/// Rewrite all values in a KS's `createdResources` block by replacing the
/// source KS name prefix with the target KS name.
fn rewrite_created_resources(ks_def: &mut serde_json::Value, source: &str, target: &str) {
    let param_keys = [
        "azureBlobParameters",
        "azureTableParameters",
        "sharePointParameters",
        "indexedSharePointParameters",
        "indexedOneLakeParameters",
    ];

    // Try top-level createdResources first
    if ks_def.get("createdResources").is_some() {
        if let Some(cr) = ks_def.get_mut("createdResources") {
            rewrite_cr_entries(cr, source, target);
        }
        return;
    }

    // Try nested under parameter blocks
    for key in &param_keys {
        let has_cr = ks_def
            .get(*key)
            .and_then(|p| p.get("createdResources"))
            .is_some();
        if has_cr {
            if let Some(params) = ks_def.get_mut(*key) {
                if let Some(cr) = params.get_mut("createdResources") {
                    rewrite_cr_entries(cr, source, target);
                }
            }
            return;
        }
    }
}

fn rewrite_cr_entries(cr: &mut serde_json::Value, source: &str, target: &str) {
    if let Some(obj) = cr.as_object_mut() {
        for (_, val) in obj.iter_mut() {
            if let Some(s) = val.as_str() {
                let new_s = s.replacen(source, target, 1);
                *val = serde_json::Value::String(new_s);
            }
        }
    }
}

/// Rewrite `indexProjections.selectors[].targetIndexName` in a skillset definition
/// by replacing the source KS name prefix with the target KS name.
fn rewrite_index_projections(skillset_def: &mut serde_json::Value, source: &str, target: &str) {
    let selectors = skillset_def
        .get_mut("indexProjections")
        .and_then(|ip| ip.get_mut("selectors"))
        .and_then(|s| s.as_array_mut());

    if let Some(selectors) = selectors {
        for selector in selectors {
            if let Some(obj) = selector.as_object_mut() {
                if let Some(idx_name) = obj
                    .get("targetIndexName")
                    .and_then(|v| v.as_str())
                    .map(String::from)
                {
                    let new_name = idx_name.replacen(source, target, 1);
                    obj.insert(
                        "targetIndexName".to_string(),
                        serde_json::Value::String(new_name),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_rewrite_created_resources_nested() {
        let mut ks_def = json!({
            "name": "test-ks",
            "azureBlobParameters": {
                "containerName": "docs",
                "createdResources": {
                    "datasource": "test-ks-datasource",
                    "indexer": "test-ks-indexer",
                    "skillset": "test-ks-skillset",
                    "index": "test-ks-index"
                }
            }
        });

        rewrite_created_resources(&mut ks_def, "test-ks", "test-ks-v2");

        let cr = ks_def["azureBlobParameters"]["createdResources"]
            .as_object()
            .unwrap();
        assert_eq!(cr["datasource"], "test-ks-v2-datasource");
        assert_eq!(cr["indexer"], "test-ks-v2-indexer");
        assert_eq!(cr["skillset"], "test-ks-v2-skillset");
        assert_eq!(cr["index"], "test-ks-v2-index");
    }

    #[test]
    fn test_rewrite_created_resources_top_level() {
        let mut ks_def = json!({
            "name": "test-ks",
            "createdResources": {
                "index": "test-ks-index",
                "indexer": "test-ks-indexer"
            }
        });

        rewrite_created_resources(&mut ks_def, "test-ks", "my-new-ks");

        let cr = ks_def["createdResources"].as_object().unwrap();
        assert_eq!(cr["index"], "my-new-ks-index");
        assert_eq!(cr["indexer"], "my-new-ks-indexer");
    }

    #[test]
    fn test_rewrite_created_resources_no_created() {
        let mut ks_def = json!({
            "name": "test-ks",
            "kind": "azureBlob"
        });

        // Should not panic
        rewrite_created_resources(&mut ks_def, "test-ks", "test-ks-v2");
        assert_eq!(ks_def["name"], "test-ks");
    }

    #[test]
    fn test_rewrite_index_projections() {
        let mut skillset = json!({
            "name": "test-ks-skillset",
            "indexProjections": {
                "selectors": [
                    {
                        "targetIndexName": "test-ks-index",
                        "parentKeyFieldName": "parent_id",
                        "sourceContext": "/document/pages/*"
                    }
                ],
                "parameters": {
                    "projectionMode": "skipIndexingParentDocuments"
                }
            }
        });

        rewrite_index_projections(&mut skillset, "test-ks", "test-ks-v2");

        let target_idx = skillset["indexProjections"]["selectors"][0]["targetIndexName"]
            .as_str()
            .unwrap();
        assert_eq!(target_idx, "test-ks-v2-index");
    }

    #[test]
    fn test_rewrite_index_projections_multiple_selectors() {
        let mut skillset = json!({
            "name": "my-skillset",
            "indexProjections": {
                "selectors": [
                    { "targetIndexName": "ks-a-index" },
                    { "targetIndexName": "ks-a-secondary" }
                ]
            }
        });

        rewrite_index_projections(&mut skillset, "ks-a", "ks-b");

        let selectors = skillset["indexProjections"]["selectors"]
            .as_array()
            .unwrap();
        assert_eq!(selectors[0]["targetIndexName"], "ks-b-index");
        assert_eq!(selectors[1]["targetIndexName"], "ks-b-secondary");
    }

    #[test]
    fn test_rewrite_index_projections_no_projections() {
        let mut skillset = json!({
            "name": "simple-skillset",
            "skills": []
        });

        // Should not panic
        rewrite_index_projections(&mut skillset, "old", "new");
        assert_eq!(skillset["name"], "simple-skillset");
    }

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
        let ks_base = service_dir.join("knowledge-sources");

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
        let ixer_content =
            std::fs::read_to_string(target_dir.join("my-ks-v2-indexer.json")).unwrap();
        let ixer: serde_json::Value = serde_json::from_str(&ixer_content).unwrap();
        assert_eq!(ixer["name"], "my-ks-v2-indexer");
        assert_eq!(ixer["dataSourceName"], "my-ks-v2-datasource");
        assert_eq!(ixer["targetIndexName"], "my-ks-v2-index");
        assert_eq!(ixer["skillsetName"], "my-ks-v2-skillset");

        // Verify managed datasource
        let ds_content =
            std::fs::read_to_string(target_dir.join("my-ks-v2-datasource.json")).unwrap();
        let ds: serde_json::Value = serde_json::from_str(&ds_content).unwrap();
        assert_eq!(ds["name"], "my-ks-v2-datasource");

        // Verify managed skillset (indexProjections rewritten)
        let sk_content =
            std::fs::read_to_string(target_dir.join("my-ks-v2-skillset.json")).unwrap();
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
        let index_dir = service_dir.join("indexes");
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

        copy_standalone_resource(&service_dir, ResourceKind::Index, "my-index", "my-index-v2")
            .unwrap();

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
        let index_dir = service_dir.join("indexes");
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
        let ks_base = service_dir.join("knowledge-sources");

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
        let ks_base = service_dir.join("knowledge-sources");
        std::fs::create_dir_all(&ks_base).unwrap();

        let result = copy_knowledge_source(&service_dir, "nonexistent", "target");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }
}
