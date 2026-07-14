//! Knowledge-source migration transforms: convert an indexed knowledge
//! source (azureBlob, azureSql, oneLake, ...) with an Azure-generated
//! pipeline into the explicit `searchIndex` shape plus first-class
//! data-source/index/skillset/indexer definitions.
//!
//! Everything here is a pure document transform — the `rigg migrate` command
//! writes files, and `rigg push` performs the actual replace.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::registry;
use crate::resources::ResourceKind;

/// The generated sub-resources named by a knowledge source's
/// `createdResources` object (found at any nesting depth — the live shape
/// nests it under `<kind>Parameters`). Unknown member names are ignored.
pub fn created_resources(ks_doc: &Value) -> BTreeMap<ResourceKind, String> {
    let mut out = BTreeMap::new();
    walk_created(ks_doc, &mut out);
    out
}

fn walk_created(v: &Value, out: &mut BTreeMap<ResourceKind, String>) {
    match v {
        Value::Object(map) => {
            if let Some(Value::Object(created)) = map.get("createdResources") {
                for (member, name) in created {
                    let kind = match member.as_str() {
                        "datasource" => Some(ResourceKind::DataSource),
                        "indexer" => Some(ResourceKind::Indexer),
                        "skillset" => Some(ResourceKind::Skillset),
                        "index" => Some(ResourceKind::Index),
                        _ => None, // future member names: ignore
                    };
                    if let (Some(kind), Some(name)) = (kind, name.as_str()) {
                        out.insert(kind, name.to_string());
                    }
                }
            }
            for val in map.values() {
                walk_created(val, out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                walk_created(item, out);
            }
        }
        _ => {}
    }
}

/// Whether this knowledge source is an indexed kind with a generated
/// pipeline that can be migrated (i.e. carries a non-empty
/// `createdResources`). Remote kinds (web, MCP, ...) and `searchIndex`
/// sources have nothing to migrate.
pub fn is_indexed_with_created(ks_doc: &Value) -> bool {
    ks_doc.get("kind").and_then(Value::as_str) != Some("searchIndex")
        && !created_resources(ks_doc).is_empty()
}

/// Build the explicit `searchIndex` knowledge-source document replacing an
/// indexed one: preserves `name` and `description`, points at `index_name`,
/// and drops every `<kind>Parameters` payload.
pub fn to_search_index_ks(ks_doc: &Value, index_name: &str) -> Value {
    let mut out = json!({
        "name": ks_doc.get("name").cloned().unwrap_or(Value::Null),
        "kind": "searchIndex",
        "searchIndexParameters": { "searchIndexName": index_name }
    });
    if let Some(desc) = ks_doc.get("description").filter(|d| !d.is_null()) {
        out["description"] = desc.clone();
    }
    out
}

/// Derive side-by-side names for the new pipeline: when a generated name
/// starts with the old knowledge-source name, that prefix is swapped for the
/// new name (`regulatory-index` → `reg2-index`); otherwise a conventional
/// `<new>-<kind>` name is used.
pub fn derive_names(
    old_ks: &str,
    new_ks: &str,
    created: &BTreeMap<ResourceKind, String>,
) -> BTreeMap<ResourceKind, String> {
    created
        .iter()
        .map(|(kind, name)| {
            let derived = match name.strip_prefix(old_ks) {
                Some(rest) => format!("{new_ks}{rest}"),
                None => {
                    let suffix = match kind {
                        ResourceKind::DataSource => "datasource",
                        ResourceKind::Index => "index",
                        ResourceKind::Skillset => "skillset",
                        ResourceKind::Indexer => "indexer",
                        other => registry::meta(*other).dir_name,
                    };
                    format!("{new_ks}-{suffix}")
                }
            };
            (*kind, derived)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob_ks() -> Value {
        json!({
            "name": "regulatory",
            "kind": "azureBlob",
            "description": "Legal docs.",
            "azureBlobParameters": {
                "containerName": "regulatory",
                "createdResources": {
                    "datasource": "regulatory-datasource",
                    "indexer": "regulatory-indexer",
                    "skillset": "regulatory-skillset",
                    "index": "regulatory-index",
                    "somethingFuture": "ignored-name"
                }
            }
        })
    }

    #[test]
    fn created_resources_reads_nested_map() {
        let map = created_resources(&blob_ks());
        assert_eq!(map.len(), 4);
        assert_eq!(map[&ResourceKind::Index], "regulatory-index");
        assert_eq!(map[&ResourceKind::DataSource], "regulatory-datasource");
        assert_eq!(map[&ResourceKind::Skillset], "regulatory-skillset");
        assert_eq!(map[&ResourceKind::Indexer], "regulatory-indexer");
    }

    #[test]
    fn is_indexed_with_created_true_for_blob_false_otherwise() {
        assert!(is_indexed_with_created(&blob_ks()));
        let search_index = json!({"name": "ks", "kind": "searchIndex",
            "searchIndexParameters": {"searchIndexName": "docs"}});
        assert!(!is_indexed_with_created(&search_index));
        let web = json!({"name": "ks", "kind": "web", "webParameters": {}});
        assert!(!is_indexed_with_created(&web));
    }

    #[test]
    fn to_search_index_ks_preserves_name_and_description() {
        let ks = to_search_index_ks(&blob_ks(), "regulatory-index");
        assert_eq!(
            ks,
            json!({
                "name": "regulatory",
                "kind": "searchIndex",
                "description": "Legal docs.",
                "searchIndexParameters": {"searchIndexName": "regulatory-index"}
            })
        );
    }

    #[test]
    fn to_search_index_ks_omits_missing_description() {
        let ks = json!({"name": "n", "kind": "azureBlob", "description": null});
        let out = to_search_index_ks(&ks, "i");
        assert!(out.get("description").is_none());
    }

    #[test]
    fn derive_names_swaps_prefix() {
        let created = created_resources(&blob_ks());
        let names = derive_names("regulatory", "reg2", &created);
        assert_eq!(names[&ResourceKind::Index], "reg2-index");
        assert_eq!(names[&ResourceKind::DataSource], "reg2-datasource");
        assert_eq!(names[&ResourceKind::Skillset], "reg2-skillset");
        assert_eq!(names[&ResourceKind::Indexer], "reg2-indexer");
    }

    #[test]
    fn derive_names_falls_back_to_conventional_suffix() {
        let mut created = BTreeMap::new();
        created.insert(ResourceKind::Index, "weird-name".to_string());
        let names = derive_names("regulatory", "reg2", &created);
        assert_eq!(names[&ResourceKind::Index], "reg2-index");
    }
}
