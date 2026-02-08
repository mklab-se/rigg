//! Unified summary of all local resource definitions

use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

use hoist_core::resources::ResourceKind;

use crate::cli::OutputFormat;
use crate::commands::load_config;

/// Summary of an index field
#[derive(Debug, Clone)]
struct FieldSummary {
    name: String,
    field_type: String,
    is_key: bool,
    analyzer: Option<String>,
}

/// Summary of an index resource
#[derive(Debug, Clone)]
struct IndexSummary {
    name: String,
    fields: Vec<FieldSummary>,
    vector_profile_count: usize,
    has_semantic_config: bool,
}

/// Summary of a data source resource
#[derive(Debug, Clone)]
struct DataSourceSummary {
    name: String,
    source_type: String,
    container: String,
}

/// Summary of an indexer resource
#[derive(Debug, Clone)]
struct IndexerSummary {
    name: String,
    target_index: String,
    data_source: String,
    skillset: Option<String>,
}

/// Summary of a skill within a skillset
#[derive(Debug, Clone)]
struct SkillEntry {
    odata_type: String,
    name: Option<String>,
}

/// Summary of a skillset resource
#[derive(Debug, Clone)]
struct SkillsetSummary {
    name: String,
    skills: Vec<SkillEntry>,
}

/// Summary of a synonym map resource
#[derive(Debug, Clone)]
struct SynonymMapSummary {
    name: String,
    format: String,
}

/// Summary of an alias resource
#[derive(Debug, Clone)]
struct AliasSummary {
    name: String,
    indexes: Vec<String>,
}

/// Summary of a knowledge base resource
#[derive(Debug, Clone)]
struct KnowledgeBaseSummary {
    name: String,
}

/// Summary of a knowledge source resource
#[derive(Debug, Clone)]
struct KnowledgeSourceSummary {
    name: String,
    index_name: Option<String>,
    knowledge_base: Option<String>,
}

/// A dependency between resources
#[derive(Debug, Clone)]
struct Dependency {
    from: String,
    to: String,
    kind: String,
}

/// All project resource summaries collected together
#[derive(Debug, Default)]
struct ProjectSummary {
    service_name: String,
    indexes: Vec<IndexSummary>,
    data_sources: Vec<DataSourceSummary>,
    indexers: Vec<IndexerSummary>,
    skillsets: Vec<SkillsetSummary>,
    synonym_maps: Vec<SynonymMapSummary>,
    aliases: Vec<AliasSummary>,
    knowledge_bases: Vec<KnowledgeBaseSummary>,
    knowledge_sources: Vec<KnowledgeSourceSummary>,
    dependencies: Vec<Dependency>,
}

pub async fn run(output: OutputFormat) -> Result<()> {
    let (project_root, config) = load_config()?;

    let include_preview = config.sync.include_preview;
    let resource_base = config.resource_dir(&project_root);

    let kinds: Vec<ResourceKind> = if include_preview {
        ResourceKind::all().to_vec()
    } else {
        ResourceKind::stable().to_vec()
    };

    let mut summary = ProjectSummary {
        service_name: config.service.name.clone(),
        ..Default::default()
    };

    for kind in &kinds {
        let resource_dir = resource_base.join(kind.directory_name());
        if !resource_dir.exists() {
            continue;
        }

        let values = read_json_files(&resource_dir)?;

        match kind {
            ResourceKind::Index => {
                for v in &values {
                    summary.indexes.push(parse_index(v));
                }
            }
            ResourceKind::DataSource => {
                for v in &values {
                    summary.data_sources.push(parse_data_source(v));
                }
            }
            ResourceKind::Indexer => {
                for v in &values {
                    let indexer = parse_indexer(v);
                    add_indexer_dependencies(&indexer, &mut summary.dependencies);
                    summary.indexers.push(indexer);
                }
            }
            ResourceKind::Skillset => {
                for v in &values {
                    summary.skillsets.push(parse_skillset(v));
                }
            }
            ResourceKind::SynonymMap => {
                for v in &values {
                    summary.synonym_maps.push(parse_synonym_map(v));
                }
            }
            ResourceKind::Alias => {
                for v in &values {
                    summary.aliases.push(parse_alias(v));
                }
            }
            ResourceKind::KnowledgeBase => {
                for v in &values {
                    summary.knowledge_bases.push(parse_knowledge_base(v));
                }
            }
            ResourceKind::KnowledgeSource => {
                for v in &values {
                    let ks = parse_knowledge_source(v);
                    add_knowledge_source_dependencies(&ks, &mut summary.dependencies);
                    summary.knowledge_sources.push(ks);
                }
            }
        }
    }

    // Sort each resource list by name for deterministic output
    summary.indexes.sort_by(|a, b| a.name.cmp(&b.name));
    summary.data_sources.sort_by(|a, b| a.name.cmp(&b.name));
    summary.indexers.sort_by(|a, b| a.name.cmp(&b.name));
    summary.skillsets.sort_by(|a, b| a.name.cmp(&b.name));
    summary.synonym_maps.sort_by(|a, b| a.name.cmp(&b.name));
    summary.aliases.sort_by(|a, b| a.name.cmp(&b.name));
    summary.knowledge_bases.sort_by(|a, b| a.name.cmp(&b.name));
    summary
        .knowledge_sources
        .sort_by(|a, b| a.name.cmp(&b.name));
    summary.dependencies.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.to.cmp(&b.to))
    });

    match output {
        OutputFormat::Text => print_text(&summary),
        OutputFormat::Json => print_json(&summary),
    }

    Ok(())
}

/// Read all JSON files from a directory and parse them as serde_json::Value
fn read_json_files(dir: &Path) -> Result<Vec<Value>> {
    let mut values = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&content)?;
        values.push(value);
    }
    Ok(values)
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

fn get_name(v: &Value) -> String {
    v.get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("(unnamed)")
        .to_string()
}

fn parse_index(v: &Value) -> IndexSummary {
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
        fields,
        vector_profile_count,
        has_semantic_config,
    }
}

fn parse_field(v: &Value) -> FieldSummary {
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

fn parse_data_source(v: &Value) -> DataSourceSummary {
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
        source_type,
        container,
    }
}

fn parse_indexer(v: &Value) -> IndexerSummary {
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
        target_index,
        data_source,
        skillset,
    }
}

fn add_indexer_dependencies(indexer: &IndexerSummary, deps: &mut Vec<Dependency>) {
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

fn parse_skillset(v: &Value) -> SkillsetSummary {
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

    SkillsetSummary { name, skills }
}

fn parse_synonym_map(v: &Value) -> SynonymMapSummary {
    let name = get_name(v);
    let format = v
        .get("format")
        .and_then(|f| f.as_str())
        .unwrap_or("solr")
        .to_string();
    SynonymMapSummary { name, format }
}

fn parse_alias(v: &Value) -> AliasSummary {
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

    AliasSummary { name, indexes }
}

fn parse_knowledge_base(v: &Value) -> KnowledgeBaseSummary {
    let name = get_name(v);
    KnowledgeBaseSummary { name }
}

fn parse_knowledge_source(v: &Value) -> KnowledgeSourceSummary {
    let name = get_name(v);
    let index_name = v
        .get("indexName")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());
    let knowledge_base = v
        .get("knowledgeBaseName")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    KnowledgeSourceSummary {
        name,
        index_name,
        knowledge_base,
    }
}

fn add_knowledge_source_dependencies(ks: &KnowledgeSourceSummary, deps: &mut Vec<Dependency>) {
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

// ---------------------------------------------------------------------------
// Text output
// ---------------------------------------------------------------------------

fn print_text(summary: &ProjectSummary) {
    println!("Service Configuration: {}", summary.service_name);
    println!("{}", "=".repeat(23 + summary.service_name.len()));
    println!();

    // Indexes
    if !summary.indexes.is_empty() {
        println!("Indexes ({}):", summary.indexes.len());
        for idx in &summary.indexes {
            let key_field = idx.fields.iter().find(|f| f.is_key);
            let key_info = key_field
                .map(|f| format!(", key: {}", f.name))
                .unwrap_or_default();
            println!("  {} ({} fields{})", idx.name, idx.fields.len(), key_info);

            // Field listing
            let field_strs: Vec<String> = idx
                .fields
                .iter()
                .map(|f| {
                    let mut s = format!("{} ({}", f.name, f.field_type);
                    if f.is_key {
                        s.push_str(", key");
                    }
                    if let Some(ref a) = f.analyzer {
                        s.push_str(&format!(", analyzer: {}", a));
                    }
                    s.push(')');
                    s
                })
                .collect();
            // Show up to 5 fields inline, then ellipsis
            if field_strs.len() <= 5 {
                println!("    Fields: {}", field_strs.join(", "));
            } else {
                let shown: Vec<&str> = field_strs.iter().take(5).map(|s| s.as_str()).collect();
                println!("    Fields: {}, ...", shown.join(", "));
            }

            if idx.vector_profile_count > 0 {
                println!("    Vector search: {} profile(s)", idx.vector_profile_count);
            }
            if idx.has_semantic_config {
                println!("    Semantic: default config");
            }
        }
        println!();
    }

    // Data Sources
    if !summary.data_sources.is_empty() {
        println!("Data Sources ({}):", summary.data_sources.len());
        for ds in &summary.data_sources {
            if ds.container.is_empty() {
                println!("  {} ({})", ds.name, ds.source_type);
            } else {
                println!("  {} ({} -> {})", ds.name, ds.source_type, ds.container);
            }
        }
        println!();
    }

    // Indexers
    if !summary.indexers.is_empty() {
        println!("Indexers ({}):", summary.indexers.len());
        for idxr in &summary.indexers {
            println!("  {}", idxr.name);
            let skillset_part = idxr
                .skillset
                .as_ref()
                .map(|s| format!(" | Skillset: {}", s))
                .unwrap_or_default();
            println!(
                "    Index: {} | Data Source: {}{}",
                idxr.target_index, idxr.data_source, skillset_part
            );
        }
        println!();
    }

    // Skillsets
    if !summary.skillsets.is_empty() {
        println!("Skillsets ({}):", summary.skillsets.len());
        for ss in &summary.skillsets {
            println!("  {} ({} skills)", ss.name, ss.skills.len());
            for skill in &ss.skills {
                match &skill.name {
                    Some(n) => println!("    - {} ({})", skill.odata_type, n),
                    None => println!("    - {}", skill.odata_type),
                }
            }
        }
        println!();
    }

    // Synonym Maps
    if !summary.synonym_maps.is_empty() {
        println!("Synonym Maps ({}):", summary.synonym_maps.len());
        for sm in &summary.synonym_maps {
            println!("  {} ({} format)", sm.name, sm.format);
        }
        println!();
    }

    // Aliases
    if !summary.aliases.is_empty() {
        println!("Aliases ({}):", summary.aliases.len());
        for alias in &summary.aliases {
            println!("  {} -> {}", alias.name, alias.indexes.join(", "));
        }
        println!();
    }

    // Knowledge Bases
    if !summary.knowledge_bases.is_empty() {
        println!("Knowledge Bases ({}):", summary.knowledge_bases.len());
        for kb in &summary.knowledge_bases {
            println!("  {}", kb.name);
        }
        println!();
    }

    // Knowledge Sources
    if !summary.knowledge_sources.is_empty() {
        println!("Knowledge Sources ({}):", summary.knowledge_sources.len());
        for ks in &summary.knowledge_sources {
            let mut parts = Vec::new();
            if let Some(ref idx) = ks.index_name {
                parts.push(format!("Index: {}", idx));
            }
            if let Some(ref kb) = ks.knowledge_base {
                parts.push(format!("KB: {}", kb));
            }
            if parts.is_empty() {
                println!("  {}", ks.name);
            } else {
                println!("  {} -> {}", ks.name, parts.join(", "));
            }
        }
        println!();
    }

    // Dependencies
    if !summary.dependencies.is_empty() {
        println!("Dependencies:");
        // Group dependencies by source
        let mut grouped: std::collections::BTreeMap<&str, Vec<(&str, &str)>> =
            std::collections::BTreeMap::new();
        for dep in &summary.dependencies {
            grouped
                .entry(&dep.from)
                .or_default()
                .push((&dep.to, &dep.kind));
        }
        for (from, targets) in &grouped {
            let target_strs: Vec<String> = targets
                .iter()
                .map(|(to, kind)| format!("{} ({})", to, kind))
                .collect();
            println!("  {} -> {}", from, target_strs.join(", "));
        }
        println!();
    }
}

// ---------------------------------------------------------------------------
// JSON output
// ---------------------------------------------------------------------------

fn print_json(summary: &ProjectSummary) {
    let indexes: Vec<Value> = summary
        .indexes
        .iter()
        .map(|idx| {
            let fields: Vec<Value> = idx
                .fields
                .iter()
                .map(|f| {
                    let mut field = json!({
                        "name": f.name,
                        "type": f.field_type,
                        "key": f.is_key,
                    });
                    if let Some(ref a) = f.analyzer {
                        field["analyzer"] = json!(a);
                    }
                    field
                })
                .collect();
            json!({
                "name": idx.name,
                "field_count": idx.fields.len(),
                "key_field": idx.fields.iter().find(|f| f.is_key).map(|f| &f.name),
                "fields": fields,
                "vector_profile_count": idx.vector_profile_count,
                "has_semantic_config": idx.has_semantic_config,
            })
        })
        .collect();

    let data_sources: Vec<Value> = summary
        .data_sources
        .iter()
        .map(|ds| {
            json!({
                "name": ds.name,
                "type": ds.source_type,
                "container": ds.container,
            })
        })
        .collect();

    let indexers: Vec<Value> = summary
        .indexers
        .iter()
        .map(|idxr| {
            json!({
                "name": idxr.name,
                "target_index": idxr.target_index,
                "data_source": idxr.data_source,
                "skillset": idxr.skillset,
            })
        })
        .collect();

    let skillsets: Vec<Value> = summary
        .skillsets
        .iter()
        .map(|ss| {
            let skills: Vec<Value> = ss
                .skills
                .iter()
                .map(|s| {
                    json!({
                        "type": s.odata_type,
                        "name": s.name,
                    })
                })
                .collect();
            json!({
                "name": ss.name,
                "skill_count": ss.skills.len(),
                "skills": skills,
            })
        })
        .collect();

    let synonym_maps: Vec<Value> = summary
        .synonym_maps
        .iter()
        .map(|sm| {
            json!({
                "name": sm.name,
                "format": sm.format,
            })
        })
        .collect();

    let aliases: Vec<Value> = summary
        .aliases
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "indexes": a.indexes,
            })
        })
        .collect();

    let knowledge_bases: Vec<Value> = summary
        .knowledge_bases
        .iter()
        .map(|kb| {
            json!({
                "name": kb.name,
            })
        })
        .collect();

    let knowledge_sources: Vec<Value> = summary
        .knowledge_sources
        .iter()
        .map(|ks| {
            json!({
                "name": ks.name,
                "index_name": ks.index_name,
                "knowledge_base": ks.knowledge_base,
            })
        })
        .collect();

    let deps: Vec<Value> = summary
        .dependencies
        .iter()
        .map(|d| {
            json!({
                "from": d.from,
                "to": d.to,
                "kind": d.kind,
            })
        })
        .collect();

    let output = json!({
        "service_name": summary.service_name,
        "indexes": indexes,
        "data_sources": data_sources,
        "indexers": indexers,
        "skillsets": skillsets,
        "synonym_maps": synonym_maps,
        "aliases": aliases,
        "knowledge_bases": knowledge_bases,
        "knowledge_sources": knowledge_sources,
        "dependencies": deps,
    });

    println!("{}", serde_json::to_string_pretty(&output).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        let idx = parse_index(&v);
        assert_eq!(idx.name, "hotels");
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
        let idx = parse_index(&v);
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
        let ds = parse_data_source(&v);
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
        let ds = parse_data_source(&v);
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
        let idxr = parse_indexer(&v);
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
        let idxr = parse_indexer(&v);
        assert_eq!(idxr.name, "simple-indexer");
        assert!(idxr.skillset.is_none());
    }

    #[test]
    fn test_add_indexer_dependencies() {
        let idxr = IndexerSummary {
            name: "hotels-indexer".to_string(),
            target_index: "hotels".to_string(),
            data_source: "cosmos-hotels".to_string(),
            skillset: Some("enrichment".to_string()),
        };
        let mut deps = Vec::new();
        add_indexer_dependencies(&idxr, &mut deps);
        assert_eq!(deps.len(), 3);
        assert!(deps
            .iter()
            .any(|d| d.to == "cosmos-hotels" && d.kind == "Data Source"));
        assert!(deps.iter().any(|d| d.to == "hotels" && d.kind == "Index"));
        assert!(deps
            .iter()
            .any(|d| d.to == "enrichment" && d.kind == "Skillset"));
    }

    #[test]
    fn test_add_indexer_dependencies_no_skillset() {
        let idxr = IndexerSummary {
            name: "simple-indexer".to_string(),
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
        let ss = parse_skillset(&v);
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
        let sm = parse_synonym_map(&v);
        assert_eq!(sm.name, "hotel-synonyms");
        assert_eq!(sm.format, "solr");
    }

    #[test]
    fn test_parse_synonym_map_default_format() {
        let v = json!({
            "name": "my-synonyms"
        });
        let sm = parse_synonym_map(&v);
        assert_eq!(sm.format, "solr");
    }

    #[test]
    fn test_parse_knowledge_base() {
        let v = json!({
            "name": "regulatory-kb"
        });
        let kb = parse_knowledge_base(&v);
        assert_eq!(kb.name, "regulatory-kb");
    }

    #[test]
    fn test_parse_knowledge_source() {
        let v = json!({
            "name": "regulatory-docs",
            "indexName": "regulatory-index",
            "knowledgeBaseName": "regulatory-kb"
        });
        let ks = parse_knowledge_source(&v);
        assert_eq!(ks.name, "regulatory-docs");
        assert_eq!(ks.index_name.as_deref(), Some("regulatory-index"));
        assert_eq!(ks.knowledge_base.as_deref(), Some("regulatory-kb"));
    }

    #[test]
    fn test_add_knowledge_source_dependencies() {
        let ks = KnowledgeSourceSummary {
            name: "regulatory-docs".to_string(),
            index_name: Some("regulatory-index".to_string()),
            knowledge_base: Some("regulatory-kb".to_string()),
        };
        let mut deps = Vec::new();
        add_knowledge_source_dependencies(&ks, &mut deps);
        assert_eq!(deps.len(), 2);
        assert!(deps
            .iter()
            .any(|d| d.to == "regulatory-index" && d.kind == "Index"));
        assert!(deps
            .iter()
            .any(|d| d.to == "regulatory-kb" && d.kind == "Knowledge Base"));
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
    fn test_parse_index_no_fields() {
        let v = json!({"name": "empty-index"});
        let idx = parse_index(&v);
        assert_eq!(idx.name, "empty-index");
        assert!(idx.fields.is_empty());
        assert_eq!(idx.vector_profile_count, 0);
        assert!(!idx.has_semantic_config);
    }
}
