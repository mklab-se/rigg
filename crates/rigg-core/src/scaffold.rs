//! Resource scaffolding: identity-first starter definitions for every kind.
//!
//! Scaffolds NEVER contain key-based credentials. Data sources use managed
//! identity via ResourceId connection strings; connections use
//! ProjectManagedIdentity; model access relies on RBAC.

use serde_json::{Value, json};

use crate::registry::{self, Channel};
use crate::resources::traits::ResourceKind;

/// Scaffold a starter definition for a resource kind.
///
/// `ds_type` applies to `DataSource` (default `azureblob`).
/// Returns an error message when inputs are invalid (unknown ds type).
pub fn scaffold(kind: ResourceKind, name: &str, ds_type: Option<&str>) -> Result<Value, String> {
    Ok(match kind {
        ResourceKind::DataSource => scaffold_datasource(name, ds_type.unwrap_or("azureblob"))?,
        ResourceKind::Index => scaffold_index(name),
        ResourceKind::Skillset => scaffold_skillset(name),
        ResourceKind::Indexer => scaffold_indexer(name),
        ResourceKind::SynonymMap => scaffold_synonym_map(name),
        ResourceKind::Alias => scaffold_alias(name),
        ResourceKind::KnowledgeSource => scaffold_knowledge_source(name),
        ResourceKind::KnowledgeBase => scaffold_knowledge_base(name),
        ResourceKind::Agent => scaffold_agent(name),
        ResourceKind::Deployment => scaffold_deployment(name),
        ResourceKind::Connection => scaffold_connection(name),
        ResourceKind::Guardrail => scaffold_guardrail(name),
    })
}

/// Validate a data source `type` string. Returns `Ok(warning)` where the
/// warning is set for preview-only types.
pub fn check_datasource_type(ds_type: &str) -> Result<Option<String>, String> {
    let preview = registry::valid_datasource_types(Channel::Preview);
    if !preview.contains(&ds_type) {
        return Err(format!(
            "unknown data source type '{ds_type}' (valid: {})",
            preview.join(", ")
        ));
    }
    if registry::preview_only_datasource_types().contains(&ds_type) {
        return Ok(Some(format!(
            "data source type '{ds_type}' requires a preview api-version; \
             pin `preview-api-version` on the search connection if pushes fail \
             (note: Azure spells Azure Files 'azurefile' in stable and 'azurefiles' in preview)"
        )));
    }
    Ok(None)
}

fn scaffold_datasource(name: &str, ds_type: &str) -> Result<Value, String> {
    check_datasource_type(ds_type)?;
    let (connection_string, container) = match ds_type {
        "azureblob" | "adlsgen2" | "azurefile" | "azurefiles" | "azuretable" => (
            "ResourceId=/subscriptions/<subscription-id>/resourceGroups/<rg>/providers/Microsoft.Storage/storageAccounts/<storage-account>;",
            json!({"name": "<container-name>"}),
        ),
        "cosmosdb" => (
            "ResourceId=/subscriptions/<subscription-id>/resourceGroups/<rg>/providers/Microsoft.DocumentDB/databaseAccounts/<cosmos-account>;Database=<database>;IdentityAuthType=AccessToken",
            json!({"name": "<collection-name>"}),
        ),
        "azuresql" => (
            "ResourceId=/subscriptions/<subscription-id>/resourceGroups/<rg>/providers/Microsoft.Sql/servers/<server>;Database=<database>;Connection Timeout=30;",
            json!({"name": "[dbo].[<table-name>]"}),
        ),
        "onelake" => (
            "ResourceId=<fabric-workspace-guid>;",
            json!({"name": "<lakehouse-guid>"}),
        ),
        _ => (
            "ResourceId=<resource-id-of-the-data-store>;",
            json!({"name": "<container-or-table>"}),
        ),
    };
    Ok(json!({
        "name": name,
        "type": ds_type,
        "credentials": {"connectionString": connection_string},
        "container": container,
        "dataChangeDetectionPolicy": null,
        "dataDeletionDetectionPolicy": null
    }))
}

fn scaffold_index(name: &str) -> Value {
    json!({
        "name": name,
        "fields": [
            {"name": "id", "type": "Edm.String", "key": true, "filterable": true},
            {"name": "content", "type": "Edm.String", "searchable": true, "analyzer": "standard.lucene"},
            {"name": "title", "type": "Edm.String", "searchable": true, "sortable": true},
            {"name": "url", "type": "Edm.String", "retrievable": true}
        ],
        "semantic": {
            "configurations": [{
                "name": "default",
                "prioritizedFields": {
                    "titleField": {"fieldName": "title"},
                    "prioritizedContentFields": [{"fieldName": "content"}]
                }
            }]
        }
    })
}

fn scaffold_skillset(name: &str) -> Value {
    json!({
        "name": name,
        "description": "Enrichment pipeline. Add built-in skills or a WebApiSkill implementing a spec from apis/ (link it with \"x-rigg-api\").",
        "skills": [
            {
                "@odata.type": "#Microsoft.Skills.Text.SplitSkill",
                "name": "split",
                "context": "/document",
                "textSplitMode": "pages",
                "maximumPageLength": 2000,
                "inputs": [{"name": "text", "source": "/document/content"}],
                "outputs": [{"name": "textItems", "targetName": "pages"}]
            }
        ]
    })
}

fn scaffold_indexer(name: &str) -> Value {
    json!({
        "name": name,
        "dataSourceName": "<data-source-name>",
        "targetIndexName": "<index-name>",
        "skillsetName": null,
        "schedule": null,
        "parameters": {
            "configuration": {}
        }
    })
}

fn scaffold_synonym_map(name: &str) -> Value {
    json!({
        "name": name,
        "format": "solr",
        "synonyms": "car, automobile\nlaptop, notebook => computer"
    })
}

fn scaffold_alias(name: &str) -> Value {
    json!({
        "name": name,
        "indexes": ["<index-name>"]
    })
}

fn scaffold_knowledge_source(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "searchIndex",
        "description": "Explicit knowledge source over an existing index.",
        "searchIndexParameters": {
            "searchIndexName": "<index-name>"
        }
    })
}

fn scaffold_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "description": "Agentic retrieval over the listed knowledge sources.",
        "knowledgeSources": [
            {"name": "<knowledge-source-name>"}
        ]
    })
}

fn scaffold_agent(name: &str) -> Value {
    json!({
        "name": name,
        "kind": "prompt",
        "model": "<deployment-name>",
        "instructions": format!("You are {name}. Describe the agent's task, tone and constraints here."),
        "tools": []
    })
}

fn scaffold_deployment(name: &str) -> Value {
    json!({
        "name": name,
        "sku": {"name": "GlobalStandard", "capacity": 1},
        "properties": {
            "model": {"format": "OpenAI", "name": name, "version": "<model-version>"},
            "versionUpgradeOption": "OnceNewDefaultVersionAvailable",
            "raiPolicyName": "Microsoft.DefaultV2"
        }
    })
}

fn scaffold_connection(name: &str) -> Value {
    json!({
        "name": name,
        "properties": {
            "category": "RemoteTool",
            "target": "<endpoint-url>",
            "authType": "ProjectManagedIdentity",
            "metadata": {}
        }
    })
}

fn scaffold_guardrail(name: &str) -> Value {
    json!({
        "name": name,
        "properties": {
            "mode": "Blocking",
            "basePolicyName": "Microsoft.DefaultV2",
            "contentFilters": [
                {"name": "Violence", "blocking": true, "enabled": true, "severityThreshold": "Medium", "source": "Prompt"},
                {"name": "Violence", "blocking": true, "enabled": true, "severityThreshold": "Medium", "source": "Completion"}
            ]
        }
    })
}

/// The explicit pipeline scaffold: data source → index → skillset → indexer →
/// knowledge source → knowledge base, cross-referenced by name.
pub fn scaffold_pipeline(
    name: &str,
    ds_type: &str,
    with_skillset: bool,
) -> Result<Vec<(ResourceKind, String, Value)>, String> {
    let ds_name = format!("{name}-ds");
    let index_name = format!("{name}-index");
    let skillset_name = format!("{name}-skills");
    let indexer_name = format!("{name}-indexer");
    let ks_name = format!("{name}-ks");
    let kb_name = format!("{name}-kb");

    let mut out = Vec::new();
    out.push((
        ResourceKind::DataSource,
        ds_name.clone(),
        scaffold_datasource(&ds_name, ds_type)?,
    ));
    out.push((
        ResourceKind::Index,
        index_name.clone(),
        scaffold_index(&index_name),
    ));
    if with_skillset {
        out.push((
            ResourceKind::Skillset,
            skillset_name.clone(),
            scaffold_skillset(&skillset_name),
        ));
    }
    let mut indexer = scaffold_indexer(&indexer_name);
    indexer["dataSourceName"] = json!(ds_name);
    indexer["targetIndexName"] = json!(index_name);
    if with_skillset {
        indexer["skillsetName"] = json!(skillset_name);
    }
    out.push((ResourceKind::Indexer, indexer_name, indexer));

    let mut ks = scaffold_knowledge_source(&ks_name);
    ks["searchIndexParameters"]["searchIndexName"] = json!(index_name);
    out.push((ResourceKind::KnowledgeSource, ks_name.clone(), ks));

    let mut kb = scaffold_knowledge_base(&kb_name);
    kb["knowledgeSources"] = json!([{"name": ks_name}]);
    out.push((ResourceKind::KnowledgeBase, kb_name, kb));
    Ok(out)
}

/// OpenAPI 3.1 spec scaffold shaped to the Azure custom WebApiSkill contract.
pub fn scaffold_api_spec(name: &str) -> Value {
    let operation = json!({
        "post": {
            "operationId": name,
            "summary": "Enrich a batch of documents",
            "requestBody": {
                "required": true,
                "content": {"application/json": {"schema": {"$ref": "#/components/schemas/EnrichmentRequest"}}}
            },
            "responses": {
                "200": {
                    "description": "Enriched values",
                    "content": {"application/json": {"schema": {"$ref": "#/components/schemas/EnrichmentResponse"}}}
                }
            }
        }
    });
    let description = format!(
        "Custom Web API skill contract. Implement this API (e.g. as an Azure Function) \
         and point a skillset WebApiSkill at it with \"x-rigg-api\": \"{name}\"."
    );
    json!({
        "openapi": "3.1.0",
        "info": {
            "title": name,
            "version": "1.0.0",
            "description": description
        },
        "paths": {
            "/api/enrich": operation
        },
        "components": {
            "schemas": {
                "EnrichmentRequest": {
                    "type": "object",
                    "required": ["values"],
                    "properties": {
                        "values": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["recordId", "data"],
                                "properties": {
                                    "recordId": {"type": "string"},
                                    "data": {
                                        "type": "object",
                                        "description": "Skill inputs — replace with your input fields",
                                        "additionalProperties": true
                                    }
                                }
                            }
                        }
                    }
                },
                "EnrichmentResponse": {
                    "type": "object",
                    "required": ["values"],
                    "properties": {
                        "values": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["recordId", "data"],
                                "properties": {
                                    "recordId": {"type": "string"},
                                    "data": {
                                        "type": "object",
                                        "description": "Skill outputs — replace with your output fields",
                                        "additionalProperties": true
                                    },
                                    "errors": {"type": "array", "items": {"type": "object"}},
                                    "warnings": {"type": "array", "items": {"type": "object"}}
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_exists_for_every_kind() {
        for kind in ResourceKind::all() {
            let v = scaffold(*kind, "test-name", None).unwrap();
            assert_eq!(v["name"], "test-name", "{kind:?}");
        }
    }

    #[test]
    fn scaffolds_are_identity_first_no_secrets() {
        for kind in ResourceKind::all() {
            let v = scaffold(*kind, "x", None).unwrap();
            let text = serde_json::to_string(&v).unwrap();
            assert!(
                !text.contains("AccountKey="),
                "{kind:?} leaks a key pattern"
            );
            assert!(!text.contains("apiKey"), "{kind:?} contains apiKey");
            assert!(!text.to_lowercase().contains("password"), "{kind:?}");
        }
        let ds = scaffold(ResourceKind::DataSource, "d", Some("azureblob")).unwrap();
        assert!(
            ds["credentials"]["connectionString"]
                .as_str()
                .unwrap()
                .starts_with("ResourceId=")
        );
        let conn = scaffold(ResourceKind::Connection, "c", None).unwrap();
        assert_eq!(conn["properties"]["authType"], "ProjectManagedIdentity");
    }

    #[test]
    fn datasource_type_validation() {
        assert!(check_datasource_type("cosmosdb").unwrap().is_none());
        assert!(
            check_datasource_type("sharepoint").unwrap().is_some(),
            "preview warns"
        );
        assert!(check_datasource_type("azurefiles").unwrap().is_some());
        assert!(check_datasource_type("bogus").is_err());
    }

    #[test]
    fn pipeline_cross_references_by_name() {
        let parts = scaffold_pipeline("demo", "azureblob", true).unwrap();
        assert_eq!(parts.len(), 6);
        let get = |kind: ResourceKind| {
            parts
                .iter()
                .find(|(k, _, _)| *k == kind)
                .map(|(_, _, v)| v)
                .unwrap()
        };
        let indexer = get(ResourceKind::Indexer);
        assert_eq!(indexer["dataSourceName"], "demo-ds");
        assert_eq!(indexer["targetIndexName"], "demo-index");
        assert_eq!(indexer["skillsetName"], "demo-skills");
        let ks = get(ResourceKind::KnowledgeSource);
        assert_eq!(ks["searchIndexParameters"]["searchIndexName"], "demo-index");
        let kb = get(ResourceKind::KnowledgeBase);
        assert_eq!(kb["knowledgeSources"][0]["name"], "demo-ks");

        // pipeline ordering works via the graph
        let items: Vec<_> = parts
            .iter()
            .map(|(k, n, v)| {
                (
                    crate::resources::traits::ResourceRef::new(*k, n.clone()),
                    v.clone(),
                )
            })
            .collect();
        let order = crate::graph::push_order(&items).unwrap();
        assert_eq!(order.len(), 6);
    }

    #[test]
    fn api_spec_matches_webapi_contract() {
        let spec = scaffold_api_spec("doc-enrichment");
        assert_eq!(spec["openapi"], "3.1.0");
        let req = &spec["components"]["schemas"]["EnrichmentRequest"];
        assert_eq!(
            req["properties"]["values"]["items"]["required"][0],
            "recordId"
        );
    }

    #[test]
    fn without_skillset_pipeline_has_five_parts() {
        let parts = scaffold_pipeline("p", "cosmosdb", false).unwrap();
        assert_eq!(parts.len(), 5);
        let indexer = parts
            .iter()
            .find(|(k, _, _)| *k == ResourceKind::Indexer)
            .map(|(_, _, v)| v)
            .unwrap();
        assert!(indexer["skillsetName"].is_null());
    }
}
