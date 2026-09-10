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

/// Validate a data source `type` string.
pub fn check_datasource_type(ds_type: &str) -> Result<(), String> {
    let valid = registry::valid_datasource_types(Channel::Stable);
    if valid.contains(&ds_type) {
        Ok(())
    } else {
        Err(format!(
            "unsupported data source type '{ds_type}' — rigg supports Azure Blob Storage only (valid: {})",
            valid.join(", ")
        ))
    }
}

fn scaffold_datasource(name: &str, ds_type: &str) -> Result<Value, String> {
    check_datasource_type(ds_type)?;
    let (connection_string, container) = (
        "ResourceId=/subscriptions/<subscription-id>/resourceGroups/<rg>/providers/Microsoft.Storage/storageAccounts/<storage-account>;",
        json!({"name": "<container-name>"}),
    );
    // Deletion tracking is on by default: without it, deleted source data
    // stays in the index forever — almost never what anyone wants.
    let (change_policy, deletion_policy) = (
        json!(null),
        json!({
            "@odata.type": "#Microsoft.Azure.Search.NativeBlobSoftDeleteDeletionDetectionPolicy"
        }),
    );
    Ok(json!({
        "name": name,
        "type": ds_type,
        "credentials": {"connectionString": connection_string},
        "container": container,
        "dataChangeDetectionPolicy": change_policy,
        "dataDeletionDetectionPolicy": deletion_policy
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

// ---------------------------------------------------------------------------
// Identity choice (spec 2026-09-09-identity-and-auth-design.md §7)
// ---------------------------------------------------------------------------

/// The user-assigned-identity object Azure AI Search expects in an
/// `identity` / `authIdentity` field. `null` (the scaffold default) means the
/// search service's system-assigned identity.
pub fn identity_object(arm_id: &str) -> Value {
    json!({
        "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
        "userAssignedIdentity": arm_id
    })
}

/// The AI services account a `--identity` skillset scaffold bills its
/// built-in skills to — a placeholder, like every other `<…>` in a scaffold,
/// because only the operator knows which Foundry account it is.
pub const AI_SERVICES_SUBDOMAIN_PLACEHOLDER: &str =
    "https://<ai-services-account>.cognitiveservices.azure.com";

/// Kinds whose registry identity path exists but does not fit the *scaffold*
/// `rigg new` writes.
///
/// `KnowledgeSource` scaffolds as `kind: "searchIndex"`, and its only
/// single-field identity path lives under `azureBlobParameters` — a
/// parameter block that contradicts that kind. The blob forms of a knowledge
/// source come from `rigg env learn` / `rigg pull`, not from `rigg new`.
const IDENTITY_NOT_A_SCAFFOLD_TARGET: &[ResourceKind] = &[ResourceKind::KnowledgeSource];

/// The field a scaffold's `--identity <binding>` writes into for `kind`:
/// the first registry [`registry::InfraForm::UserAssignedIdentity`] path
/// that addresses a single field rather than an array element, minus the
/// kinds in [`IDENTITY_NOT_A_SCAFFOLD_TARGET`].
///
/// Array-element identities (`skills[].authIdentity`,
/// `vectorSearch.vectorizers[].authIdentity`,
/// `models[].azureOpenAIParameters.authIdentity`) belong to individual
/// skills/vectorizers/models a scaffold does not yet have, so they are not
/// scaffold targets. `Indexer` has none at all: its only user-assigned
/// identity field is on the preview-only enrichment cache, which the
/// registry deliberately does not model.
pub fn identity_field(kind: ResourceKind) -> Option<&'static str> {
    if IDENTITY_NOT_A_SCAFFOLD_TARGET.contains(&kind) {
        return None;
    }
    registry::infra_refs(kind)
        .iter()
        .find(|r| r.form == registry::InfraForm::UserAssignedIdentity && !r.path.contains("[]"))
        .map(|r| r.path)
}

/// Every kind [`identity_field`] accepts, for the usage error naming them.
pub fn kinds_accepting_identity() -> Vec<ResourceKind> {
    ResourceKind::all()
        .iter()
        .copied()
        .filter(|k| identity_field(*k).is_some())
        .collect()
}

/// Point `kind`'s identity field at the user-assigned identity `arm_id`,
/// creating the intermediate objects the path needs — and the discriminator
/// its container needs to be a legal document. Errors when the kind has no
/// such field.
///
/// A `Skillset`'s identity lives on `cognitiveServices`, which Azure AI
/// Search will only accept with an `@odata.type` saying *which* form of AI
/// services connection it is: writing the identity alone produces a document
/// the service rejects at push. `--identity` therefore also declares the
/// keyless form (`AIServicesByIdentity`) and its required `subdomainUrl`
/// placeholder — the same pair `rigg validate` recommends.
pub fn set_identity(kind: ResourceKind, doc: &mut Value, arm_id: &str) -> Result<(), String> {
    let path = identity_field(kind).ok_or_else(|| {
        format!(
            "{} has no user-assigned identity field — --identity applies to: {}",
            kind.cli_name(),
            kinds_accepting_identity()
                .iter()
                .map(|k| k.cli_name())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let mut cursor: &mut Value = &mut *doc;
    let segments: Vec<&str> = path.split('.').collect();
    let (last, parents) = segments.split_last().expect("registry paths are non-empty");
    for segment in parents {
        if !cursor.get(*segment).is_some_and(Value::is_object) {
            cursor[*segment] = json!({});
        }
        cursor = cursor
            .get_mut(*segment)
            .expect("just ensured it is an object");
    }
    cursor[*last] = identity_object(arm_id);
    if kind == ResourceKind::Skillset {
        let cs = &mut doc["cognitiveServices"];
        cs["@odata.type"] = json!("#Microsoft.Azure.Search.AIServicesByIdentity");
        if !cs
            .get("subdomainUrl")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
        {
            cs["subdomainUrl"] = json!(AI_SERVICES_SUBDOMAIN_PLACEHOLDER);
        }
    }
    Ok(())
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
        assert!(check_datasource_type("azureblob").is_ok());
        assert!(check_datasource_type("adlsgen2").is_ok());
        let err = check_datasource_type("cosmosdb").unwrap_err();
        assert!(err.contains("azureblob, adlsgen2"), "{err}");
        assert!(scaffold(ResourceKind::DataSource, "x", Some("azuresql")).is_err());
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
    fn identity_field_comes_from_the_registry_and_skips_array_paths() {
        assert_eq!(identity_field(ResourceKind::DataSource), Some("identity"));
        assert_eq!(
            identity_field(ResourceKind::Skillset),
            Some("cognitiveServices.identity")
        );
        // The knowledge-source scaffold is `kind: "searchIndex"`; its only
        // identity path belongs to the blob forms, which come from
        // `rigg env learn` / pull rather than from `rigg new`.
        assert_eq!(identity_field(ResourceKind::KnowledgeSource), None);
        // Only `vectorSearch.vectorizers[]` / `models[]` carry one — array
        // elements a scaffold does not have.
        assert_eq!(identity_field(ResourceKind::Index), None);
        assert_eq!(identity_field(ResourceKind::KnowledgeBase), None);
        // The indexer's only UAMI field is the unmodelled preview cache.
        assert_eq!(identity_field(ResourceKind::Indexer), None);
        assert_eq!(identity_field(ResourceKind::Agent), None);
    }

    #[test]
    fn set_identity_writes_the_data_user_assigned_identity_object() {
        let arm = "/subscriptions/s/resourceGroups/rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/mi";
        let mut ds = scaffold(ResourceKind::DataSource, "docs", None).unwrap();
        set_identity(ResourceKind::DataSource, &mut ds, arm).unwrap();
        assert_eq!(
            ds["identity"]["@odata.type"],
            "#Microsoft.Azure.Search.DataUserAssignedIdentity"
        );
        assert_eq!(ds["identity"]["userAssignedIdentity"], arm);
        // Nothing else about the scaffold changed.
        assert_eq!(ds["type"], "azureblob");
    }

    #[test]
    fn set_identity_creates_the_intermediate_objects_and_the_discriminator() {
        let arm = "/subscriptions/s/resourceGroups/rg/providers/Microsoft.ManagedIdentity/userAssignedIdentities/mi";
        let mut ss = scaffold(ResourceKind::Skillset, "ss", None).unwrap();
        set_identity(ResourceKind::Skillset, &mut ss, arm).unwrap();
        let cs = &ss["cognitiveServices"];
        assert_eq!(cs["identity"]["userAssignedIdentity"], arm);
        // Without the discriminator (and the subdomain it implies) Azure AI
        // Search rejects the PUT — see review finding 1.
        assert_eq!(
            cs["@odata.type"],
            "#Microsoft.Azure.Search.AIServicesByIdentity"
        );
        assert_eq!(cs["subdomainUrl"], AI_SERVICES_SUBDOMAIN_PLACEHOLDER);
        // A subdomain the document already carries is kept.
        let mut existing = scaffold(ResourceKind::Skillset, "ss", None).unwrap();
        existing["cognitiveServices"] =
            json!({"subdomainUrl": "https://real.cognitiveservices.azure.com"});
        set_identity(ResourceKind::Skillset, &mut existing, arm).unwrap();
        assert_eq!(
            existing["cognitiveServices"]["subdomainUrl"],
            "https://real.cognitiveservices.azure.com"
        );
    }

    #[test]
    fn set_identity_rejects_kinds_without_an_identity_field_and_names_the_others() {
        let err = set_identity(ResourceKind::Indexer, &mut json!({}), "id").unwrap_err();
        assert!(err.contains("data-source"), "{err}");
        assert!(err.contains("indexer has no"), "{err}");
        assert_eq!(
            kinds_accepting_identity(),
            vec![ResourceKind::DataSource, ResourceKind::Skillset]
        );
    }

    #[test]
    fn without_skillset_pipeline_has_five_parts() {
        let parts = scaffold_pipeline("p", "adlsgen2", false).unwrap();
        assert_eq!(parts.len(), 5);
        let indexer = parts
            .iter()
            .find(|(k, _, _)| *k == ResourceKind::Indexer)
            .map(|(_, _, v)| v)
            .unwrap();
        assert!(indexer["skillsetName"].is_null());
    }
}
