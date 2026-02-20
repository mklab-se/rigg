//! Resource scaffolding — generate clean template files for new resources
//!
//! Each function returns a `serde_json::Value` representing a valid Azure resource
//! definition with sensible defaults. No Azure connection required.

use serde_json::{Value, json};

/// Scaffold an Azure AI Search index definition.
///
/// Basic: `id` (key) + `content` field.
/// With `vector`: adds `contentVector` field + `vectorSearch` HNSW config.
/// With `semantic`: adds `semantic` configuration referencing `content`.
pub fn scaffold_index(name: &str, vector: bool, semantic: bool) -> Value {
    let mut fields = vec![
        json!({
            "name": "id",
            "type": "Edm.String",
            "key": true,
            "filterable": true
        }),
        json!({
            "name": "content",
            "type": "Edm.String",
            "searchable": true
        }),
    ];

    if vector {
        fields.push(json!({
            "name": "contentVector",
            "type": "Collection(Edm.Single)",
            "searchable": true,
            "dimensions": 1536,
            "vectorSearchProfile": "default-vector-profile"
        }));
    }

    let mut index = json!({
        "name": name,
        "fields": fields
    });

    if vector {
        index["vectorSearch"] = json!({
            "algorithms": [{
                "name": "default-hnsw",
                "kind": "hnsw",
                "hnswParameters": {
                    "metric": "cosine",
                    "m": 4,
                    "efConstruction": 400,
                    "efSearch": 500
                }
            }],
            "profiles": [{
                "name": "default-vector-profile",
                "algorithm": "default-hnsw"
            }]
        });
    }

    if semantic {
        index["semantic"] = json!({
            "configurations": [{
                "name": "default-semantic-config",
                "prioritizedFields": {
                    "contentFields": [{
                        "fieldName": "content"
                    }]
                }
            }]
        });
    }

    index
}

/// Scaffold an Azure AI Search data source definition.
pub fn scaffold_datasource(name: &str, ds_type: &str, container: &str) -> Value {
    json!({
        "name": name,
        "type": ds_type,
        "credentials": {
            "connectionString": ""
        },
        "container": {
            "name": container
        }
    })
}

/// Scaffold an Azure AI Search indexer definition.
pub fn scaffold_indexer(
    name: &str,
    datasource: &str,
    index: &str,
    skillset: Option<&str>,
    schedule: &str,
) -> Value {
    let mut indexer = json!({
        "name": name,
        "dataSourceName": datasource,
        "targetIndexName": index,
        "schedule": {
            "interval": schedule
        },
        "parameters": {
            "batchSize": 1000
        }
    });

    if let Some(ss) = skillset {
        indexer["skillsetName"] = json!(ss);
    }

    indexer
}

/// Scaffold an Azure AI Search skillset definition.
pub fn scaffold_skillset(name: &str) -> Value {
    json!({
        "name": name,
        "skills": []
    })
}

/// Scaffold an Azure AI Search synonym map definition.
pub fn scaffold_synonym_map(name: &str) -> Value {
    json!({
        "name": name,
        "format": "solr",
        "synonyms": ""
    })
}

/// Scaffold an Azure AI Search alias definition.
pub fn scaffold_alias(name: &str, index: &str) -> Value {
    json!({
        "name": name,
        "indexes": [index]
    })
}

/// Scaffold an Azure AI Search knowledge base definition.
pub fn scaffold_knowledge_base(name: &str) -> Value {
    json!({
        "name": name,
        "description": ""
    })
}

/// Scaffold an Azure AI Search knowledge source definition.
pub fn scaffold_knowledge_source(name: &str, index: &str, knowledge_base: Option<&str>) -> Value {
    let mut ks = json!({
        "name": name,
        "indexName": index
    });

    if let Some(kb) = knowledge_base {
        ks["knowledgeBaseName"] = json!(kb);
    }

    ks
}

/// Scaffold a Foundry agent definition as a JSON value.
///
/// The returned value can be passed to `agent_to_yaml()` to produce the
/// on-disk YAML format.
pub fn scaffold_agent(name: &str, model: &str) -> Value {
    json!({
        "name": name,
        "kind": "prompt",
        "model": model,
        "instructions": "You are a helpful AI assistant.",
        "tools": []
    })
}

/// Result of scaffolding a complete Agentic RAG system.
///
/// Contains all interconnected resource definitions ready to be written to disk.
pub struct AgenticRagScaffold {
    /// Knowledge base definition
    pub knowledge_base: Value,
    pub knowledge_base_name: String,
    /// Knowledge source definition
    pub knowledge_source: Value,
    pub knowledge_source_name: String,
    /// Agent definition (pass to `agent_to_yaml()` for on-disk format)
    pub agent: Value,
    pub agent_name: String,
}

/// Scaffold a complete Agentic RAG system: agent + knowledge base + knowledge source.
///
/// The agent is pre-wired with an MCP tool pointing to the knowledge base.
/// The knowledge source references the knowledge base.
/// All naming follows the convention `<base>`, `<base>-kb`, `<base>-ks`.
pub fn scaffold_agentic_rag(
    base_name: &str,
    model: &str,
    search_service: &str,
    datasource_type: &str,
    container: &str,
) -> AgenticRagScaffold {
    let kb_name = format!("{}-kb", base_name);
    let ks_name = format!("{}-ks", base_name);
    let index_name = format!("{}-ks-index", base_name);

    let knowledge_base = json!({
        "name": kb_name,
        "description": "",
        "retrievalInstructions": "",
        "outputMode": "extractiveData"
    });

    let knowledge_source = json!({
        "name": ks_name,
        "indexName": index_name,
        "knowledgeBaseName": kb_name,
        "kind": datasource_type,
        "description": "",
        format!("{}Parameters", datasource_type): {
            "containerName": container
        }
    });

    let mcp_url = format!(
        "https://{}.search.windows.net/knowledgebases/{}/mcp",
        search_service, kb_name
    );

    let agent = json!({
        "name": base_name,
        "kind": "prompt",
        "model": model,
        "instructions": "You are a helpful AI assistant.",
        "tools": [
            {
                "type": "mcp",
                "server_label": kb_name,
                "server_url": mcp_url
            }
        ]
    });

    AgenticRagScaffold {
        knowledge_base,
        knowledge_base_name: kb_name,
        knowledge_source,
        knowledge_source_name: ks_name,
        agent,
        agent_name: base_name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scaffold_index_basic() {
        let idx = scaffold_index("my-index", false, false);
        assert_eq!(idx["name"], "my-index");
        let fields = idx["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0]["name"], "id");
        assert!(fields[0]["key"].as_bool().unwrap());
        assert_eq!(fields[1]["name"], "content");
        assert!(idx.get("vectorSearch").is_none());
        assert!(idx.get("semantic").is_none());
    }

    #[test]
    fn test_scaffold_index_vector() {
        let idx = scaffold_index("vec-index", true, false);
        let fields = idx["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 3);
        assert_eq!(fields[2]["name"], "contentVector");
        assert_eq!(fields[2]["dimensions"], 1536);
        assert!(idx.get("vectorSearch").is_some());
        assert_eq!(idx["vectorSearch"]["algorithms"][0]["kind"], "hnsw");
        assert!(idx.get("semantic").is_none());
    }

    #[test]
    fn test_scaffold_index_semantic() {
        let idx = scaffold_index("sem-index", false, true);
        let fields = idx["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 2);
        assert!(idx.get("semantic").is_some());
        assert_eq!(
            idx["semantic"]["configurations"][0]["prioritizedFields"]["contentFields"][0]["fieldName"],
            "content"
        );
        assert!(idx.get("vectorSearch").is_none());
    }

    #[test]
    fn test_scaffold_index_vector_and_semantic() {
        let idx = scaffold_index("full-index", true, true);
        let fields = idx["fields"].as_array().unwrap();
        assert_eq!(fields.len(), 3);
        assert!(idx.get("vectorSearch").is_some());
        assert!(idx.get("semantic").is_some());
    }

    #[test]
    fn test_scaffold_datasource() {
        let ds = scaffold_datasource("my-ds", "azureblob", "documents");
        assert_eq!(ds["name"], "my-ds");
        assert_eq!(ds["type"], "azureblob");
        assert_eq!(ds["container"]["name"], "documents");
        assert_eq!(ds["credentials"]["connectionString"], "");
    }

    #[test]
    fn test_scaffold_indexer_basic() {
        let ixer = scaffold_indexer("my-indexer", "my-ds", "my-index", None, "PT5M");
        assert_eq!(ixer["name"], "my-indexer");
        assert_eq!(ixer["dataSourceName"], "my-ds");
        assert_eq!(ixer["targetIndexName"], "my-index");
        assert_eq!(ixer["schedule"]["interval"], "PT5M");
        assert_eq!(ixer["parameters"]["batchSize"], 1000);
        assert!(ixer.get("skillsetName").is_none());
    }

    #[test]
    fn test_scaffold_indexer_with_skillset() {
        let ixer = scaffold_indexer(
            "my-indexer",
            "my-ds",
            "my-index",
            Some("my-skillset"),
            "PT1H",
        );
        assert_eq!(ixer["skillsetName"], "my-skillset");
        assert_eq!(ixer["schedule"]["interval"], "PT1H");
    }

    #[test]
    fn test_scaffold_skillset() {
        let ss = scaffold_skillset("my-skillset");
        assert_eq!(ss["name"], "my-skillset");
        assert!(ss["skills"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_scaffold_synonym_map() {
        let sm = scaffold_synonym_map("my-synonyms");
        assert_eq!(sm["name"], "my-synonyms");
        assert_eq!(sm["format"], "solr");
        assert_eq!(sm["synonyms"], "");
    }

    #[test]
    fn test_scaffold_alias() {
        let alias = scaffold_alias("my-alias", "my-index");
        assert_eq!(alias["name"], "my-alias");
        let indexes = alias["indexes"].as_array().unwrap();
        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0], "my-index");
    }

    #[test]
    fn test_scaffold_knowledge_base() {
        let kb = scaffold_knowledge_base("my-kb");
        assert_eq!(kb["name"], "my-kb");
        assert_eq!(kb["description"], "");
    }

    #[test]
    fn test_scaffold_knowledge_source_basic() {
        let ks = scaffold_knowledge_source("my-ks", "my-index", None);
        assert_eq!(ks["name"], "my-ks");
        assert_eq!(ks["indexName"], "my-index");
        assert!(ks.get("knowledgeBaseName").is_none());
    }

    #[test]
    fn test_scaffold_knowledge_source_with_kb() {
        let ks = scaffold_knowledge_source("my-ks", "my-index", Some("my-kb"));
        assert_eq!(ks["name"], "my-ks");
        assert_eq!(ks["indexName"], "my-index");
        assert_eq!(ks["knowledgeBaseName"], "my-kb");
    }

    #[test]
    fn test_scaffold_agent() {
        let agent = scaffold_agent("my-agent", "gpt-4o");
        assert_eq!(agent["name"], "my-agent");
        assert_eq!(agent["kind"], "prompt");
        assert_eq!(agent["model"], "gpt-4o");
        assert!(agent["instructions"].as_str().unwrap().len() > 0);
        assert!(agent["tools"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_scaffold_agent_custom_model() {
        let agent = scaffold_agent("my-agent", "gpt-4.1-mini");
        assert_eq!(agent["model"], "gpt-4.1-mini");
    }

    #[test]
    fn test_scaffold_index_valid_json() {
        // Verify the generated JSON can be serialized/deserialized cleanly
        let idx = scaffold_index("test", true, true);
        let json_str = serde_json::to_string_pretty(&idx).unwrap();
        let parsed: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["name"], "test");
    }

    #[test]
    fn test_scaffold_datasource_types() {
        for ds_type in &[
            "azureblob",
            "azuretable",
            "azuresql",
            "cosmosdb",
            "adlsgen2",
        ] {
            let ds = scaffold_datasource("test", ds_type, "my-container");
            assert_eq!(ds["type"].as_str().unwrap(), *ds_type);
        }
    }

    #[test]
    fn test_scaffold_agent_yaml_roundtrip() {
        use crate::resources::agent::{agent_to_yaml, yaml_to_agent};

        let agent = scaffold_agent("test-agent", "gpt-4o");
        let yaml = agent_to_yaml(&agent);
        let parsed = yaml_to_agent(&yaml).unwrap();

        assert_eq!(parsed["kind"], "prompt");
        assert_eq!(parsed["model"], "gpt-4o");
        assert!(parsed["instructions"].as_str().unwrap().len() > 0);
        // name is excluded from YAML (derived from filename)
        assert!(parsed.get("name").is_none());
    }

    #[test]
    fn test_scaffold_agentic_rag_naming() {
        let rag = scaffold_agentic_rag("my-system", "gpt-4o", "my-search", "azureBlob", "docs");
        assert_eq!(rag.agent_name, "my-system");
        assert_eq!(rag.knowledge_base_name, "my-system-kb");
        assert_eq!(rag.knowledge_source_name, "my-system-ks");
    }

    #[test]
    fn test_scaffold_agentic_rag_knowledge_base() {
        let rag = scaffold_agentic_rag("my-system", "gpt-4o", "my-search", "azureBlob", "docs");
        assert_eq!(rag.knowledge_base["name"], "my-system-kb");
        assert_eq!(rag.knowledge_base["outputMode"], "extractiveData");
    }

    #[test]
    fn test_scaffold_agentic_rag_knowledge_source() {
        let rag = scaffold_agentic_rag("my-system", "gpt-4o", "my-search", "azureBlob", "docs");
        assert_eq!(rag.knowledge_source["name"], "my-system-ks");
        assert_eq!(rag.knowledge_source["indexName"], "my-system-ks-index");
        assert_eq!(rag.knowledge_source["knowledgeBaseName"], "my-system-kb");
        assert_eq!(rag.knowledge_source["kind"], "azureBlob");
        assert_eq!(
            rag.knowledge_source["azureBlobParameters"]["containerName"],
            "docs"
        );
    }

    #[test]
    fn test_scaffold_agentic_rag_agent_has_mcp_tool() {
        let rag = scaffold_agentic_rag("my-system", "gpt-4o", "my-search", "azureBlob", "docs");
        assert_eq!(rag.agent["name"], "my-system");
        assert_eq!(rag.agent["model"], "gpt-4o");
        let tools = rag.agent["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "mcp");
        assert_eq!(tools[0]["server_label"], "my-system-kb");
        assert!(
            tools[0]["server_url"]
                .as_str()
                .unwrap()
                .contains("my-search.search.windows.net")
        );
        assert!(
            tools[0]["server_url"]
                .as_str()
                .unwrap()
                .contains("my-system-kb")
        );
    }

    #[test]
    fn test_scaffold_agentic_rag_agent_yaml_roundtrip() {
        use crate::resources::agent::{agent_to_yaml, yaml_to_agent};

        let rag = scaffold_agentic_rag("test", "gpt-4o", "svc", "azureBlob", "docs");
        let yaml = agent_to_yaml(&rag.agent);
        let parsed = yaml_to_agent(&yaml).unwrap();

        assert_eq!(parsed["kind"], "prompt");
        assert_eq!(parsed["model"], "gpt-4o");
        let tools = parsed["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["type"], "mcp");
    }

    #[test]
    fn test_scaffold_agentic_rag_custom_model() {
        let rag = scaffold_agentic_rag(
            "my-system",
            "gpt-4.1-mini",
            "my-search",
            "azureBlob",
            "docs",
        );
        assert_eq!(rag.agent["model"], "gpt-4.1-mini");
    }
}
