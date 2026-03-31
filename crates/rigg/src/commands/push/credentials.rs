//! Credential handling for push operations.
//!
//! Strips volatile/read-only fields before push and injects storage
//! credentials for new data sources and knowledge sources.

use std::io::{self, Write};

use anyhow::Result;
use tracing::info;

use rigg_client::ArmClient;
use rigg_client::auth::AzCliAuth;
use rigg_core::resources::ResourceKind;

use crate::commands::common::{get_read_only_fields, get_volatile_fields};

/// Remove volatile and read-only fields from a resource definition before sending to Azure.
///
/// Recurses through the entire JSON tree, stripping field names at every level.
/// This handles both top-level fields (like `@odata.etag`) and nested read-only
/// fields (like `createdResources` inside `azureBlobParameters`).
///
/// Volatile fields (etag, context, secrets) are always stripped.
/// Read-only fields (knowledgeSources, createdResources, etc.) are kept in local
/// files for documentation but must be stripped before push since Azure rejects them.
pub(super) fn strip_volatile_fields(
    kind: ResourceKind,
    definition: &serde_json::Value,
) -> serde_json::Value {
    let volatile_fields = get_volatile_fields(kind);
    let read_only_fields = get_read_only_fields(kind);
    let all_fields: Vec<&str> = volatile_fields
        .iter()
        .chain(read_only_fields.iter())
        .copied()
        .collect();
    strip_fields_recursive(definition, &all_fields)
}

/// Check if a resource needs credential injection before push.
///
/// Returns true when:
/// - DataSource being created (or recreated) without a `credentials` object
/// - KnowledgeSource being created (or recreated) with `<redacted>` connectionString
pub(super) fn needs_credentials(
    kind: ResourceKind,
    definition: &serde_json::Value,
    exists: bool,
    needs_recreate: bool,
) -> bool {
    let is_new = !exists || needs_recreate;
    if !is_new {
        return false;
    }

    match kind {
        ResourceKind::DataSource => {
            // credentials is a volatile field — stripped during pull, so it's absent on disk.
            // If someone manually added it, respect that.
            definition
                .get("credentials")
                .and_then(|c| c.get("connectionString"))
                .and_then(|s| s.as_str())
                .is_none_or(|s| s.is_empty())
        }
        ResourceKind::KnowledgeSource => {
            // Azure returns "<redacted>" for connectionString in GET responses.
            // Check if it's redacted or missing.
            let conn = definition
                .pointer("/azureBlobParameters/connectionString")
                .and_then(|v| v.as_str());
            matches!(conn, Some("<redacted>") | None)
        }
        _ => false,
    }
}

/// Discover a storage account connection string via ARM.
///
/// Falls back gracefully — returns None on any failure (not logged in,
/// no storage accounts found, etc.).
pub(super) async fn discover_storage_credentials(
    env: &rigg_core::config::ResolvedEnvironment,
    cached: &mut Option<String>,
) -> Option<String> {
    // Return cached value if available
    if let Some(conn) = cached {
        return Some(conn.clone());
    }

    let arm = ArmClient::new().ok()?;

    // Get subscription ID: config first, then az cli
    let subscription_id = env
        .primary_search_service()
        .and_then(|s| s.subscription.clone())
        .or_else(|| {
            AzCliAuth::check_status()
                .ok()
                .and_then(|s| s.subscription_id)
        })?;

    // Get resource group: config first, then ARM discovery
    let search_svc = env.primary_search_service()?;
    let resource_group = if let Some(rg) = search_svc.resource_group.clone() {
        rg
    } else {
        arm.find_resource_group(&subscription_id, &search_svc.name)
            .await
            .ok()?
    };

    let accounts = arm
        .list_storage_accounts(&subscription_id, &resource_group)
        .await
        .ok()?;

    if accounts.is_empty() {
        return None;
    }

    let account_name = if accounts.len() == 1 {
        let name = &accounts[0].name;
        println!();
        info!("Auto-selected storage account: {}", name);
        name.clone()
    } else {
        println!();
        println!(
            "Multiple storage accounts found in resource group '{}':",
            resource_group
        );
        for (i, acct) in accounts.iter().enumerate() {
            println!("  [{}] {}", i + 1, acct);
        }
        print!("Select storage account [1]: ");
        io::stdout().flush().ok()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input).ok()?;
        let input = input.trim();

        let idx = if input.is_empty() {
            0
        } else {
            input.parse::<usize>().ok()?.checked_sub(1)?
        };

        accounts.get(idx)?.name.clone()
    };

    let conn_string = arm
        .get_storage_connection_string(&subscription_id, &resource_group, &account_name)
        .await
        .ok()?;

    *cached = Some(conn_string.clone());
    Some(conn_string)
}

/// Inject credentials into a resource definition for new data sources or knowledge sources.
///
/// Tries ARM-based auto-discovery first, then falls back to prompting the user.
pub(super) async fn inject_credentials(
    kind: ResourceKind,
    definition: &serde_json::Value,
    name: &str,
    env: &rigg_core::config::ResolvedEnvironment,
    cached: &mut Option<String>,
) -> Result<serde_json::Value> {
    // Try auto-discovery first
    let conn_string = match discover_storage_credentials(env, cached).await {
        Some(c) => c,
        None => {
            // Fall back to manual prompt
            println!();
            print!(
                "Enter connection string for {} '{}' (or press Enter to skip): ",
                kind.display_name(),
                name
            );
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_string();
            if input.is_empty() {
                return Ok(definition.clone());
            }
            input
        }
    };

    let mut def = definition.clone();

    match kind {
        ResourceKind::DataSource => {
            // Inject {"credentials": {"connectionString": "..."}}
            if let Some(obj) = def.as_object_mut() {
                obj.insert(
                    "credentials".to_string(),
                    serde_json::json!({"connectionString": conn_string}),
                );
            }
        }
        ResourceKind::KnowledgeSource => {
            // Replace azureBlobParameters.connectionString
            if let Some(blob_params) = def.get_mut("azureBlobParameters") {
                if let Some(obj) = blob_params.as_object_mut() {
                    obj.insert(
                        "connectionString".to_string(),
                        serde_json::Value::String(conn_string),
                    );
                }
            }
        }
        _ => {}
    }

    Ok(def)
}

fn strip_fields_recursive(value: &serde_json::Value, fields: &[&str]) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let filtered: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .filter(|(k, _)| !fields.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), strip_fields_recursive(v, fields)))
                .collect();
            serde_json::Value::Object(filtered)
        }
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|v| strip_fields_recursive(v, fields))
                .collect(),
        ),
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_strip_volatile_fields_removes_etag_and_context() {
        let definition = json!({
            "name": "test-index",
            "fields": [],
            "@odata.etag": "W/\"abc\"",
            "@odata.context": "https://svc.search.windows.net/$metadata#indexes/$entity"
        });
        let clean = strip_volatile_fields(ResourceKind::Index, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("fields"));
        assert!(!obj.contains_key("@odata.etag"));
        assert!(!obj.contains_key("@odata.context"));
    }

    #[test]
    fn test_strip_volatile_fields_removes_knowledge_source_top_level() {
        let definition = json!({
            "name": "ks-1",
            "indexName": "my-index",
            "description": "Test",
            "ingestionPermissionOptions": { "someConfig": true }
        });
        let clean = strip_volatile_fields(ResourceKind::KnowledgeSource, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("indexName"));
        assert!(obj.contains_key("description"));
        assert!(!obj.contains_key("ingestionPermissionOptions"));
    }

    #[test]
    fn test_strip_volatile_fields_removes_nested_created_resources() {
        let definition = json!({
            "name": "ks-1",
            "kind": "azureBlob",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>",
                "createdResources": {
                    "datasource": "ds-1",
                    "indexer": "ixer-1",
                    "skillset": "sk-1",
                    "index": "idx-1"
                }
            }
        });
        let clean = strip_volatile_fields(ResourceKind::KnowledgeSource, &definition);
        let blob_params = clean
            .get("azureBlobParameters")
            .unwrap()
            .as_object()
            .unwrap();
        assert!(blob_params.contains_key("containerName"));
        assert!(blob_params.contains_key("connectionString"));
        assert!(
            !blob_params.contains_key("createdResources"),
            "createdResources should be stripped from nested object"
        );
    }

    #[test]
    fn test_strip_volatile_fields_preserves_knowledge_base_knowledge_sources() {
        // knowledgeSources is a normal pushable field — NOT stripped
        let definition = json!({
            "name": "my-kb",
            "description": "Test KB",
            "knowledgeSources": [
                {"name": "ks-1"},
                {"name": "ks-2"}
            ]
        });
        let clean = strip_volatile_fields(ResourceKind::KnowledgeBase, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(obj.contains_key("description"));
        assert!(
            obj.contains_key("knowledgeSources"),
            "knowledgeSources is pushable and should be preserved"
        );
    }

    #[test]
    fn test_strip_volatile_fields_removes_datasource_credentials() {
        let definition = json!({
            "name": "ds-1",
            "type": "azureblob",
            "credentials": { "connectionString": "secret" }
        });
        let clean = strip_volatile_fields(ResourceKind::DataSource, &definition);
        let obj = clean.as_object().unwrap();
        assert!(obj.contains_key("name"));
        assert!(!obj.contains_key("credentials"));
    }

    #[test]
    fn test_strip_volatile_fields_preserves_non_volatile() {
        let definition = json!({
            "name": "sk-1",
            "skills": [{"name": "skill1"}],
            "description": "My skillset"
        });
        let clean = strip_volatile_fields(ResourceKind::Skillset, &definition);
        assert_eq!(clean, definition);
    }

    #[test]
    fn test_strip_volatile_fields_handles_no_volatile_present() {
        let definition = json!({
            "name": "test",
            "fields": []
        });
        let clean = strip_volatile_fields(ResourceKind::Index, &definition);
        assert_eq!(clean, definition);
    }

    #[test]
    fn test_strip_volatile_fields_removes_indexer_start_time() {
        let definition = json!({
            "name": "my-indexer",
            "dataSourceName": "ds-1",
            "targetIndexName": "idx-1",
            "schedule": {
                "interval": "P1D",
                "startTime": "2026-02-06T22:03:10.254Z"
            }
        });
        let clean = strip_volatile_fields(ResourceKind::Indexer, &definition);
        let schedule = clean.get("schedule").unwrap().as_object().unwrap();
        assert!(schedule.contains_key("interval"));
        assert!(
            !schedule.contains_key("startTime"),
            "startTime should be stripped from indexer schedule"
        );
    }

    // --- Credential injection tests ---

    #[test]
    fn test_needs_credentials_datasource_new_no_creds() {
        let def = json!({"name": "ds-1", "type": "azureblob", "container": {"name": "docs"}});
        assert!(needs_credentials(
            ResourceKind::DataSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_datasource_update() {
        let def = json!({"name": "ds-1", "type": "azureblob", "container": {"name": "docs"}});
        // Existing resource — Azure preserves credentials on update
        assert!(!needs_credentials(
            ResourceKind::DataSource,
            &def,
            true,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_datasource_recreate() {
        let def = json!({"name": "ds-1", "type": "azureblob", "container": {"name": "docs"}});
        // Drop-and-recreate needs credentials like a new resource
        assert!(needs_credentials(
            ResourceKind::DataSource,
            &def,
            true,
            true
        ));
    }

    #[test]
    fn test_needs_credentials_datasource_with_creds() {
        let def = json!({
            "name": "ds-1",
            "type": "azureblob",
            "credentials": {"connectionString": "DefaultEndpointsProtocol=https;AccountName=..."},
            "container": {"name": "docs"}
        });
        // Credentials already present — no injection needed
        assert!(!needs_credentials(
            ResourceKind::DataSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_redacted() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        assert!(needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_missing_connection_string() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {"containerName": "docs"}
        });
        assert!(needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_real_value() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "DefaultEndpointsProtocol=https;AccountName=test;AccountKey=abc"
            }
        });
        assert!(!needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            false,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_ks_update() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        // Existing KS update — Azure preserves credentials
        assert!(!needs_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            true,
            false
        ));
    }

    #[test]
    fn test_needs_credentials_index() {
        let def = json!({"name": "idx-1", "fields": []});
        // Indexes don't need credentials
        assert!(!needs_credentials(ResourceKind::Index, &def, false, false));
    }

    fn test_env() -> rigg_core::config::ResolvedEnvironment {
        rigg_core::config::ResolvedEnvironment {
            name: "test".to_string(),
            search: vec![rigg_core::SearchServiceConfig {
                name: "test-search".to_string(),
                label: None,
                subscription: None,
                resource_group: None,
                api_version: "2024-07-01".to_string(),
                preview_api_version: "2025-11-01-preview".to_string(),
            }],
            foundry: vec![],
            sync: rigg_core::config::SyncConfig::default(),
        }
    }

    #[tokio::test]
    async fn test_inject_credentials_datasource() {
        let def = json!({
            "name": "ds-1",
            "type": "azureblob",
            "container": {"name": "docs"}
        });
        let conn = "DefaultEndpointsProtocol=https;AccountName=test;AccountKey=abc";
        let mut cached = Some(conn.to_string());
        let env = test_env();

        let result = inject_credentials(ResourceKind::DataSource, &def, "ds-1", &env, &mut cached)
            .await
            .unwrap();

        assert_eq!(
            result
                .get("credentials")
                .unwrap()
                .get("connectionString")
                .unwrap()
                .as_str()
                .unwrap(),
            conn
        );
        // Original fields preserved
        assert_eq!(result.get("name").unwrap().as_str().unwrap(), "ds-1");
        assert_eq!(result.get("type").unwrap().as_str().unwrap(), "azureblob");
    }

    #[tokio::test]
    async fn test_inject_credentials_ks() {
        let def = json!({
            "name": "ks-1",
            "azureBlobParameters": {
                "containerName": "docs",
                "connectionString": "<redacted>"
            }
        });
        let conn = "DefaultEndpointsProtocol=https;AccountName=test;AccountKey=abc";
        let mut cached = Some(conn.to_string());
        let env = test_env();

        let result = inject_credentials(
            ResourceKind::KnowledgeSource,
            &def,
            "ks-1",
            &env,
            &mut cached,
        )
        .await
        .unwrap();

        assert_eq!(
            result
                .pointer("/azureBlobParameters/connectionString")
                .unwrap()
                .as_str()
                .unwrap(),
            conn
        );
        // Original fields preserved
        assert_eq!(
            result
                .pointer("/azureBlobParameters/containerName")
                .unwrap()
                .as_str()
                .unwrap(),
            "docs"
        );
    }
}
