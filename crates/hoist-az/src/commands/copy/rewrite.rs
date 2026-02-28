//! JSON rewriting helpers for copy operations.
//!
//! Rewrites `createdResources` blocks and `indexProjections` selectors
//! when copying knowledge sources under new names.

/// Rewrite all values in a KS's `createdResources` block by replacing the
/// source KS name prefix with the target KS name.
pub(super) fn rewrite_created_resources(
    ks_def: &mut serde_json::Value,
    source: &str,
    target: &str,
) {
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
pub(super) fn rewrite_index_projections(
    skillset_def: &mut serde_json::Value,
    source: &str,
    target: &str,
) {
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
}
