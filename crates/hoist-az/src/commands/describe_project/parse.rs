//! Parsing helpers for resource JSON/YAML into summary structs

use std::path::Path;

use serde_json::Value;

use super::{
    AgentSummary, AgentToolSummary, AliasSummary, DataSourceSummary, Dependency, FieldSummary,
    IndexSummary, IndexerSummary, KnowledgeBaseSummary, KnowledgeSourceSummary, SkillEntry,
    SkillsetSummary, SynonymMapSummary,
};

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

pub(super) fn get_name(v: &Value) -> String {
    v.get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("(unnamed)")
        .to_string()
}

pub(super) fn parse_index(file_path: &Path, v: &Value) -> IndexSummary {
    let name = get_name(v);

    let fields: Vec<FieldSummary> = v
        .get("fields")
        .and_then(|f| f.as_array())
        .map(|arr| arr.iter().map(parse_field).collect())
        .unwrap_or_default();

    let vector_profile_count = v
        .get("vectorSearch")
        .and_then(|vs| vs.get("profiles"))
        .and_then(|p| p.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let has_semantic_config = v
        .get("semantic")
        .and_then(|s| s.get("configurations"))
        .and_then(|c| c.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);

    IndexSummary {
        name,
        file_path: file_path.display().to_string(),
        fields,
        vector_profile_count,
        has_semantic_config,
    }
}

pub(super) fn parse_field(v: &Value) -> FieldSummary {
    let name = get_name(v);
    let field_type = v
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown")
        .to_string();
    let is_key = v.get("key").and_then(|k| k.as_bool()).unwrap_or(false);
    let analyzer = v
        .get("analyzer")
        .and_then(|a| a.as_str())
        .map(|s| s.to_string());

    FieldSummary {
        name,
        field_type,
        is_key,
        analyzer,
    }
}

pub(super) fn parse_data_source(file_path: &Path, v: &Value) -> DataSourceSummary {
    let name = get_name(v);
    let source_type = v
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("unknown")
        .to_string();
    let container = v
        .get("container")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();

    DataSourceSummary {
        name,
        file_path: file_path.display().to_string(),
        source_type,
        container,
    }
}

pub(super) fn parse_indexer(file_path: &Path, v: &Value) -> IndexerSummary {
    let name = get_name(v);
    let target_index = v
        .get("targetIndexName")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let data_source = v
        .get("dataSourceName")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let skillset = v
        .get("skillsetName")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    IndexerSummary {
        name,
        file_path: file_path.display().to_string(),
        target_index,
        data_source,
        skillset,
    }
}

pub(super) fn add_indexer_dependencies(indexer: &IndexerSummary, deps: &mut Vec<Dependency>) {
    if !indexer.data_source.is_empty() {
        deps.push(Dependency {
            from: indexer.name.clone(),
            to: indexer.data_source.clone(),
            kind: "Data Source".to_string(),
        });
    }
    if !indexer.target_index.is_empty() {
        deps.push(Dependency {
            from: indexer.name.clone(),
            to: indexer.target_index.clone(),
            kind: "Index".to_string(),
        });
    }
    if let Some(ref skillset) = indexer.skillset {
        deps.push(Dependency {
            from: indexer.name.clone(),
            to: skillset.clone(),
            kind: "Skillset".to_string(),
        });
    }
}

pub(super) fn parse_skillset(file_path: &Path, v: &Value) -> SkillsetSummary {
    let name = get_name(v);
    let skills: Vec<SkillEntry> = v
        .get("skills")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .map(|skill| {
                    let odata_type = skill
                        .get("@odata.type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let skill_name = skill
                        .get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string());
                    SkillEntry {
                        odata_type,
                        name: skill_name,
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    SkillsetSummary {
        name,
        file_path: file_path.display().to_string(),
        skills,
    }
}

pub(super) fn parse_synonym_map(file_path: &Path, v: &Value) -> SynonymMapSummary {
    let name = get_name(v);
    let format = v
        .get("format")
        .and_then(|f| f.as_str())
        .unwrap_or("solr")
        .to_string();
    SynonymMapSummary {
        name,
        file_path: file_path.display().to_string(),
        format,
    }
}

pub(super) fn parse_alias(file_path: &Path, v: &Value) -> AliasSummary {
    let name = get_name(v);
    let indexes = v
        .get("indexes")
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    AliasSummary {
        name,
        file_path: file_path.display().to_string(),
        indexes,
    }
}

pub(super) fn parse_knowledge_base(file_path: &Path, v: &Value) -> KnowledgeBaseSummary {
    let name = get_name(v);

    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let retrieval_instructions = v
        .get("retrievalInstructions")
        .and_then(|r| r.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let output_mode = v
        .get("outputMode")
        .and_then(|o| o.as_str())
        .map(String::from);

    let knowledge_sources = v
        .get("knowledgeSources")
        .and_then(|ks| ks.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|ks| ks.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    KnowledgeBaseSummary {
        name,
        file_path: file_path.display().to_string(),
        description,
        retrieval_instructions,
        output_mode,
        knowledge_sources,
    }
}

pub(super) fn parse_knowledge_source(file_path: &Path, v: &Value) -> KnowledgeSourceSummary {
    let name = get_name(v);

    let description = v
        .get("description")
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    let kind = v.get("kind").and_then(|k| k.as_str()).map(String::from);

    // Try top-level indexName first, then fall back to createdResources.index
    let index_name = v
        .get("indexName")
        .and_then(|n| n.as_str())
        .map(String::from)
        .or_else(|| {
            v.get("azureBlobParameters")
                .and_then(|b| b.get("createdResources"))
                .and_then(|cr| cr.get("index"))
                .and_then(|i| i.as_str())
                .map(String::from)
        });

    let knowledge_base = v
        .get("knowledgeBaseName")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    KnowledgeSourceSummary {
        name,
        file_path: file_path.display().to_string(),
        description,
        kind,
        index_name,
        knowledge_base,
    }
}

pub(super) fn parse_agent_yaml(yaml_path: &Path) -> Option<AgentSummary> {
    let name = yaml_path.file_stem().and_then(|n| n.to_str())?.to_string();

    let content = std::fs::read_to_string(yaml_path).ok()?;
    let value: Value = serde_yaml::from_str(&content).ok()?;

    let model = value
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    let (tool_count, tools) = value
        .get("tools")
        .and_then(|v| v.as_array())
        .map(|arr| {
            let tools: Vec<AgentToolSummary> = arr
                .iter()
                .map(|tool| {
                    let tool_type = tool
                        .get("type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let kb_name = if tool_type == "mcp" {
                        extract_kb_from_mcp_url(tool.get("server_url").and_then(|u| u.as_str()))
                    } else {
                        None
                    };
                    AgentToolSummary {
                        tool_type,
                        knowledge_base_name: kb_name,
                    }
                })
                .collect();
            (tools.len(), tools)
        })
        .unwrap_or((0, Vec::new()));

    let instructions = value
        .get("instructions")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();

    Some(AgentSummary {
        name,
        file_path: yaml_path.display().to_string(),
        model,
        tool_count,
        tools,
        instructions,
    })
}

/// Extract knowledge base name from an MCP server_url.
/// URL format: `https://{service}.search.windows.net/knowledgebases/{kb-name}/mcp?...`
pub(super) fn extract_kb_from_mcp_url(url: Option<&str>) -> Option<String> {
    let url = url?;
    let marker = "/knowledgebases/";
    let kb_start = url.find(marker)? + marker.len();
    let rest = &url[kb_start..];
    let kb_end = rest.find('/')?;
    Some(rest[..kb_end].to_string())
}

pub(super) fn add_knowledge_source_dependencies(
    ks: &KnowledgeSourceSummary,
    deps: &mut Vec<Dependency>,
) {
    if let Some(ref idx) = ks.index_name {
        deps.push(Dependency {
            from: ks.name.clone(),
            to: idx.clone(),
            kind: "Index".to_string(),
        });
    }
    if let Some(ref kb) = ks.knowledge_base {
        deps.push(Dependency {
            from: ks.name.clone(),
            to: kb.clone(),
            kind: "Knowledge Base".to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    fn test_path(name: &str) -> PathBuf {
        PathBuf::from(format!("/test/{}", name))
    }

    #[test]
    fn test_parse_index_basic() {
        let v = json!({
            "name": "hotels",
            "fields": [
                {"name": "hotelId", "type": "Edm.String", "key": true},
                {"name": "name", "type": "Edm.String"},
                {"name": "rating", "type": "Edm.Int32"}
            ]
        });
        let p = test_path("hotels.json");
        let idx = parse_index(&p, &v);
        assert_eq!(idx.name, "hotels");
        assert_eq!(idx.file_path, "/test/hotels.json");
        assert_eq!(idx.fields.len(), 3);
        assert!(idx.fields[0].is_key);
        assert_eq!(idx.fields[0].name, "hotelId");
        assert_eq!(idx.fields[0].field_type, "Edm.String");
        assert!(!idx.has_semantic_config);
        assert_eq!(idx.vector_profile_count, 0);
    }

    #[test]
    fn test_parse_index_with_vector_and_semantic() {
        let v = json!({
            "name": "docs",
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true}
            ],
            "vectorSearch": {
                "profiles": [
                    {"name": "vector-profile-1"}
                ]
            },
            "semantic": {
                "configurations": [
                    {"name": "default"}
                ]
            }
        });
        let p = test_path("docs.json");
        let idx = parse_index(&p, &v);
        assert_eq!(idx.name, "docs");
        assert_eq!(idx.vector_profile_count, 1);
        assert!(idx.has_semantic_config);
    }

    #[test]
    fn test_parse_field_with_analyzer() {
        let v = json!({
            "name": "title",
            "type": "Edm.String",
            "key": false,
            "analyzer": "en.lucene"
        });
        let f = parse_field(&v);
        assert_eq!(f.name, "title");
        assert_eq!(f.field_type, "Edm.String");
        assert!(!f.is_key);
        assert_eq!(f.analyzer.as_deref(), Some("en.lucene"));
    }

    #[test]
    fn test_parse_data_source() {
        let v = json!({
            "name": "cosmos-hotels",
            "type": "azureblob",
            "container": {"name": "docs"}
        });
        let p = test_path("cosmos-hotels.json");
        let ds = parse_data_source(&p, &v);
        assert_eq!(ds.name, "cosmos-hotels");
        assert_eq!(ds.source_type, "azureblob");
        assert_eq!(ds.container, "docs");
    }

    #[test]
    fn test_parse_data_source_no_container() {
        let v = json!({
            "name": "my-source",
            "type": "cosmosdb"
        });
        let p = test_path("my-source.json");
        let ds = parse_data_source(&p, &v);
        assert_eq!(ds.name, "my-source");
        assert_eq!(ds.source_type, "cosmosdb");
        assert_eq!(ds.container, "");
    }

    #[test]
    fn test_parse_indexer_with_skillset() {
        let v = json!({
            "name": "hotels-indexer",
            "targetIndexName": "hotels",
            "dataSourceName": "cosmos-hotels",
            "skillsetName": "enrichment"
        });
        let p = test_path("hotels-indexer.json");
        let idxr = parse_indexer(&p, &v);
        assert_eq!(idxr.name, "hotels-indexer");
        assert_eq!(idxr.target_index, "hotels");
        assert_eq!(idxr.data_source, "cosmos-hotels");
        assert_eq!(idxr.skillset.as_deref(), Some("enrichment"));
    }

    #[test]
    fn test_parse_indexer_without_skillset() {
        let v = json!({
            "name": "simple-indexer",
            "targetIndexName": "items",
            "dataSourceName": "items-ds"
        });
        let p = test_path("simple-indexer.json");
        let idxr = parse_indexer(&p, &v);
        assert_eq!(idxr.name, "simple-indexer");
        assert!(idxr.skillset.is_none());
    }

    #[test]
    fn test_add_indexer_dependencies() {
        let idxr = IndexerSummary {
            name: "hotels-indexer".to_string(),
            file_path: String::new(),
            target_index: "hotels".to_string(),
            data_source: "cosmos-hotels".to_string(),
            skillset: Some("enrichment".to_string()),
        };
        let mut deps = Vec::new();
        add_indexer_dependencies(&idxr, &mut deps);
        assert_eq!(deps.len(), 3);
        assert!(
            deps.iter()
                .any(|d| d.to == "cosmos-hotels" && d.kind == "Data Source")
        );
        assert!(deps.iter().any(|d| d.to == "hotels" && d.kind == "Index"));
        assert!(
            deps.iter()
                .any(|d| d.to == "enrichment" && d.kind == "Skillset")
        );
    }

    #[test]
    fn test_add_indexer_dependencies_no_skillset() {
        let idxr = IndexerSummary {
            name: "simple-indexer".to_string(),
            file_path: String::new(),
            target_index: "items".to_string(),
            data_source: "items-ds".to_string(),
            skillset: None,
        };
        let mut deps = Vec::new();
        add_indexer_dependencies(&idxr, &mut deps);
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_parse_skillset() {
        let v = json!({
            "name": "enrichment",
            "skills": [
                {
                    "@odata.type": "#Microsoft.Skills.Text.SplitSkill",
                    "name": "split-skill"
                },
                {
                    "@odata.type": "#Microsoft.Skills.Text.EntityRecognitionSkill",
                    "name": "entities"
                },
                {
                    "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill"
                }
            ]
        });
        let p = test_path("enrichment.json");
        let ss = parse_skillset(&p, &v);
        assert_eq!(ss.name, "enrichment");
        assert_eq!(ss.skills.len(), 3);
        assert_eq!(ss.skills[0].odata_type, "#Microsoft.Skills.Text.SplitSkill");
        assert_eq!(ss.skills[0].name.as_deref(), Some("split-skill"));
        assert!(ss.skills[2].name.is_none());
    }

    #[test]
    fn test_parse_synonym_map() {
        let v = json!({
            "name": "hotel-synonyms",
            "format": "solr"
        });
        let p = test_path("hotel-synonyms.json");
        let sm = parse_synonym_map(&p, &v);
        assert_eq!(sm.name, "hotel-synonyms");
        assert_eq!(sm.format, "solr");
    }

    #[test]
    fn test_parse_synonym_map_default_format() {
        let v = json!({
            "name": "my-synonyms"
        });
        let p = test_path("my-synonyms.json");
        let sm = parse_synonym_map(&p, &v);
        assert_eq!(sm.format, "solr");
    }

    #[test]
    fn test_parse_knowledge_base() {
        let v = json!({
            "name": "regulatory-kb",
            "description": "Official regulatory and legal texts",
            "retrievalInstructions": "You are a legal evidence retriever working over an EU regulatory knowledge base.",
            "outputMode": "extractiveData",
            "knowledgeSources": [{"name": "regulatory"}]
        });
        let p = test_path("regulatory-kb.json");
        let kb = parse_knowledge_base(&p, &v);
        assert_eq!(kb.name, "regulatory-kb");
        assert_eq!(
            kb.description.as_deref(),
            Some("Official regulatory and legal texts")
        );
        assert!(
            kb.retrieval_instructions
                .as_ref()
                .unwrap()
                .contains("legal evidence")
        );
        assert_eq!(kb.output_mode.as_deref(), Some("extractiveData"));
        assert_eq!(kb.knowledge_sources, vec!["regulatory"]);
    }

    #[test]
    fn test_parse_knowledge_base_minimal() {
        let v = json!({"name": "empty-kb"});
        let p = test_path("empty-kb.json");
        let kb = parse_knowledge_base(&p, &v);
        assert_eq!(kb.name, "empty-kb");
        assert!(kb.description.is_none());
        assert!(kb.retrieval_instructions.is_none());
        assert!(kb.output_mode.is_none());
        assert!(kb.knowledge_sources.is_empty());
    }

    #[test]
    fn test_parse_knowledge_source() {
        let v = json!({
            "name": "regulatory-docs",
            "description": "Legal and compliance documents",
            "kind": "azureBlob",
            "indexName": "regulatory-index",
            "knowledgeBaseName": "regulatory-kb"
        });
        let p = test_path("regulatory-docs.json");
        let ks = parse_knowledge_source(&p, &v);
        assert_eq!(ks.name, "regulatory-docs");
        assert_eq!(
            ks.description.as_deref(),
            Some("Legal and compliance documents")
        );
        assert_eq!(ks.kind.as_deref(), Some("azureBlob"));
        assert_eq!(ks.index_name.as_deref(), Some("regulatory-index"));
        assert_eq!(ks.knowledge_base.as_deref(), Some("regulatory-kb"));
    }

    #[test]
    fn test_parse_knowledge_source_created_resources_fallback() {
        let v = json!({
            "name": "regulatory",
            "kind": "azureBlob",
            "azureBlobParameters": {
                "createdResources": {
                    "index": "regulatory-index",
                    "indexer": "regulatory-indexer"
                }
            }
        });
        let p = test_path("regulatory.json");
        let ks = parse_knowledge_source(&p, &v);
        assert_eq!(ks.name, "regulatory");
        assert_eq!(ks.index_name.as_deref(), Some("regulatory-index"));
    }

    #[test]
    fn test_add_knowledge_source_dependencies() {
        let ks = KnowledgeSourceSummary {
            name: "regulatory-docs".to_string(),
            file_path: String::new(),
            description: None,
            kind: None,
            index_name: Some("regulatory-index".to_string()),
            knowledge_base: Some("regulatory-kb".to_string()),
        };
        let mut deps = Vec::new();
        add_knowledge_source_dependencies(&ks, &mut deps);
        assert_eq!(deps.len(), 2);
        assert!(
            deps.iter()
                .any(|d| d.to == "regulatory-index" && d.kind == "Index")
        );
        assert!(
            deps.iter()
                .any(|d| d.to == "regulatory-kb" && d.kind == "Knowledge Base")
        );
    }

    #[test]
    fn test_parse_agent_yaml_full() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("my-agent.yaml");

        std::fs::write(
            &yaml_path,
            "kind: prompt\nmodel: gpt-4o\ninstructions: You are a helpful assistant for regulatory compliance.\ntools:\n  - type: code_interpreter\n  - type: file_search\n",
        )
        .unwrap();

        let agent = parse_agent_yaml(&yaml_path).unwrap();
        assert_eq!(agent.name, "my-agent");
        assert_eq!(agent.model, "gpt-4o");
        assert_eq!(agent.tool_count, 2);
        assert_eq!(agent.tools.len(), 2);
        assert_eq!(agent.tools[0].tool_type, "code_interpreter");
        assert!(agent.tools[0].knowledge_base_name.is_none());
        assert!(
            agent
                .instructions
                .contains("helpful assistant for regulatory")
        );
    }

    #[test]
    fn test_parse_agent_yaml_minimal() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("minimal-agent.yaml");

        std::fs::write(&yaml_path, "kind: prompt\nmodel: gpt-4o-mini\n").unwrap();

        let agent = parse_agent_yaml(&yaml_path).unwrap();
        assert_eq!(agent.name, "minimal-agent");
        assert_eq!(agent.model, "gpt-4o-mini");
        assert_eq!(agent.tool_count, 0);
        assert!(agent.tools.is_empty());
        assert_eq!(agent.instructions, "");
    }

    #[test]
    fn test_parse_agent_yaml_long_instructions_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("verbose-agent.yaml");

        let long_text = "A".repeat(500);
        let yaml = format!("kind: prompt\nmodel: gpt-4o\ninstructions: {}\n", long_text);
        std::fs::write(&yaml_path, &yaml).unwrap();

        let agent = parse_agent_yaml(&yaml_path).unwrap();
        assert_eq!(agent.instructions.len(), 500);
        assert_eq!(agent.instructions, long_text);
    }

    #[test]
    fn test_get_name_present() {
        let v = json!({"name": "test-resource"});
        assert_eq!(get_name(&v), "test-resource");
    }

    #[test]
    fn test_get_name_missing() {
        let v = json!({"other": "field"});
        assert_eq!(get_name(&v), "(unnamed)");
    }

    #[test]
    fn test_extract_kb_from_mcp_url() {
        let url = "https://svc.search.windows.net/knowledgebases/regulatory-kb/mcp?api-version=2025-11-01-Preview";
        assert_eq!(
            extract_kb_from_mcp_url(Some(url)),
            Some("regulatory-kb".to_string())
        );
    }

    #[test]
    fn test_extract_kb_from_mcp_url_none() {
        assert_eq!(extract_kb_from_mcp_url(None), None);
        assert_eq!(
            extract_kb_from_mcp_url(Some("https://example.com/other")),
            None
        );
    }

    #[test]
    fn test_parse_agent_yaml_with_mcp_tools() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("rag-agent.yaml");

        std::fs::write(
            &yaml_path,
            "kind: prompt\nmodel: gpt-4o\ntools:\n  - type: mcp\n    server_label: kb_test\n    server_url: https://svc.search.windows.net/knowledgebases/my-kb/mcp?api-version=2025-11-01-Preview\n",
        )
        .unwrap();

        let agent = parse_agent_yaml(&yaml_path).unwrap();
        assert_eq!(agent.tool_count, 1);
        assert_eq!(agent.tools.len(), 1);
        assert_eq!(agent.tools[0].tool_type, "mcp");
        assert_eq!(agent.tools[0].knowledge_base_name.as_deref(), Some("my-kb"));
    }

    #[test]
    fn test_parse_index_no_fields() {
        let v = json!({"name": "empty-index"});
        let p = test_path("empty-index.json");
        let idx = parse_index(&p, &v);
        assert_eq!(idx.name, "empty-index");
        assert!(idx.fields.is_empty());
        assert_eq!(idx.vector_profile_count, 0);
        assert!(!idx.has_semantic_config);
    }
}
