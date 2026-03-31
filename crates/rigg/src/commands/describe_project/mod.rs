//! Unified summary of all local resource definitions

mod json_output;
mod parse;
mod text_output;

use anyhow::Result;
use serde_json::Value;
use std::path::Path;

use rigg_core::resources::ResourceKind;

use crate::cli::OutputFormat;
use crate::commands::load_config_and_env;

use parse::{
    add_indexer_dependencies, add_knowledge_source_dependencies, parse_agent_yaml, parse_alias,
    parse_data_source, parse_index, parse_indexer, parse_knowledge_base, parse_knowledge_source,
    parse_skillset, parse_synonym_map,
};

// ---------------------------------------------------------------------------
// Summary structs
// ---------------------------------------------------------------------------

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
    file_path: String,
    fields: Vec<FieldSummary>,
    vector_profile_count: usize,
    has_semantic_config: bool,
}

/// Summary of a data source resource
#[derive(Debug, Clone)]
struct DataSourceSummary {
    name: String,
    file_path: String,
    source_type: String,
    container: String,
}

/// Summary of an indexer resource
#[derive(Debug, Clone)]
struct IndexerSummary {
    name: String,
    file_path: String,
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
    file_path: String,
    skills: Vec<SkillEntry>,
}

/// Summary of a synonym map resource
#[derive(Debug, Clone)]
struct SynonymMapSummary {
    name: String,
    file_path: String,
    format: String,
}

/// Summary of an alias resource
#[derive(Debug, Clone)]
struct AliasSummary {
    name: String,
    file_path: String,
    indexes: Vec<String>,
}

/// Summary of a knowledge base resource
#[derive(Debug, Clone)]
struct KnowledgeBaseSummary {
    name: String,
    file_path: String,
    description: Option<String>,
    retrieval_instructions: Option<String>,
    output_mode: Option<String>,
    knowledge_sources: Vec<String>,
}

/// Summary of a knowledge source resource
#[derive(Debug, Clone)]
struct KnowledgeSourceSummary {
    name: String,
    file_path: String,
    description: Option<String>,
    kind: Option<String>,
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

/// Summary of a tool used by an agent
#[derive(Debug, Clone)]
struct AgentToolSummary {
    tool_type: String,
    knowledge_base_name: Option<String>,
}

/// Summary of a Foundry agent resource
#[derive(Debug, Clone)]
struct AgentSummary {
    name: String,
    file_path: String,
    model: String,
    tool_count: usize,
    tools: Vec<AgentToolSummary>,
    instructions: String,
}

/// All project resource summaries collected together
#[derive(Debug, Default)]
struct ProjectSummary {
    project_name: String,
    search_services: Vec<String>,
    foundry_services: Vec<String>,
    indexes: Vec<IndexSummary>,
    data_sources: Vec<DataSourceSummary>,
    indexers: Vec<IndexerSummary>,
    skillsets: Vec<SkillsetSummary>,
    synonym_maps: Vec<SynonymMapSummary>,
    aliases: Vec<AliasSummary>,
    knowledge_bases: Vec<KnowledgeBaseSummary>,
    knowledge_sources: Vec<KnowledgeSourceSummary>,
    agents: Vec<AgentSummary>,
    dependencies: Vec<Dependency>,
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

pub async fn run(output: OutputFormat, env_override: Option<&str>) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    let include_preview = env.sync.include_preview;

    let kinds: Vec<ResourceKind> = if include_preview {
        ResourceKind::all().to_vec()
    } else {
        ResourceKind::stable().to_vec()
    };

    let mut summary = ProjectSummary {
        project_name: config
            .project
            .name
            .clone()
            .unwrap_or_else(|| "rigg project".to_string()),
        search_services: env.search.iter().map(|s| s.name.clone()).collect(),
        foundry_services: env
            .foundry
            .iter()
            .map(|f| format!("{}/{}", f.name, f.project))
            .collect(),
        ..Default::default()
    };

    // Scan search resources from each configured search service
    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_root, search_svc);

        for kind in &kinds {
            if kind.domain() != rigg_core::service::ServiceDomain::Search {
                continue;
            }
            let resource_dir = search_base.join(kind.directory_name());
            if !resource_dir.exists() {
                continue;
            }

            let values = read_json_files(&resource_dir)?;

            match kind {
                ResourceKind::Index => {
                    for (path, v) in &values {
                        summary.indexes.push(parse_index(path, v));
                    }
                }
                ResourceKind::DataSource => {
                    for (path, v) in &values {
                        summary.data_sources.push(parse_data_source(path, v));
                    }
                }
                ResourceKind::Indexer => {
                    for (path, v) in &values {
                        let indexer = parse_indexer(path, v);
                        add_indexer_dependencies(&indexer, &mut summary.dependencies);
                        summary.indexers.push(indexer);
                    }
                }
                ResourceKind::Skillset => {
                    for (path, v) in &values {
                        summary.skillsets.push(parse_skillset(path, v));
                    }
                }
                ResourceKind::SynonymMap => {
                    for (path, v) in &values {
                        summary.synonym_maps.push(parse_synonym_map(path, v));
                    }
                }
                ResourceKind::Alias => {
                    for (path, v) in &values {
                        summary.aliases.push(parse_alias(path, v));
                    }
                }
                ResourceKind::KnowledgeBase => {
                    for (path, v) in &values {
                        summary.knowledge_bases.push(parse_knowledge_base(path, v));
                    }
                }
                ResourceKind::KnowledgeSource => {
                    // KS are now stored as subdirectories; read from each subdir
                    let ks_values = read_ks_from_dirs(&resource_dir);
                    for (path, v) in &ks_values {
                        let ks = parse_knowledge_source(path, v);
                        add_knowledge_source_dependencies(&ks, &mut summary.dependencies);
                        summary.knowledge_sources.push(ks);
                    }
                }
                ResourceKind::Agent => {}
            }
        }
    }

    // Scan Foundry agents from YAML files
    if env.has_foundry() {
        for foundry_config in &env.foundry {
            let agents_dir = env
                .foundry_service_dir(&files_root, foundry_config)
                .join("agents");
            if agents_dir.exists() {
                if let Ok(entries) = std::fs::read_dir(&agents_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                            if let Some(agent) = parse_agent_yaml(&path) {
                                summary.agents.push(agent);
                            }
                        }
                    }
                }
            }
        }
    }

    // Build agent -> KB dependencies
    for agent in &summary.agents {
        for tool in &agent.tools {
            if let Some(ref kb_name) = tool.knowledge_base_name {
                summary.dependencies.push(Dependency {
                    from: agent.name.clone(),
                    to: kb_name.clone(),
                    kind: "Knowledge Base".to_string(),
                });
            }
        }
    }

    // Build KB -> KS dependencies from knowledge base data
    for kb in &summary.knowledge_bases {
        for ks_name in &kb.knowledge_sources {
            summary.dependencies.push(Dependency {
                from: kb.name.clone(),
                to: ks_name.clone(),
                kind: "Knowledge Source".to_string(),
            });
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
    summary.agents.sort_by(|a, b| a.name.cmp(&b.name));
    summary.dependencies.sort_by(|a, b| {
        a.from
            .cmp(&b.from)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.to.cmp(&b.to))
    });

    match output {
        OutputFormat::Text => text_output::print_text(&summary),
        OutputFormat::Json => json_output::print_json(&summary),
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// File reading helpers
// ---------------------------------------------------------------------------

/// Read all JSON files from a directory and parse them as serde_json::Value
fn read_json_files(dir: &Path) -> Result<Vec<(std::path::PathBuf, Value)>> {
    let mut values = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&content)?;
        values.push((path, value));
    }
    Ok(values)
}

/// Read knowledge source definitions from subdirectories.
/// Each KS is stored as `<ks-name>/<ks-name>.json` within the knowledge-sources dir.
fn read_ks_from_dirs(ks_base: &Path) -> Vec<(std::path::PathBuf, Value)> {
    let mut values = Vec::new();
    let entries = match std::fs::read_dir(ks_base) {
        Ok(e) => e,
        Err(_) => return values,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let ks_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        let ks_file = path.join(format!("{}.json", ks_name));
        if let Ok(content) = std::fs::read_to_string(&ks_file) {
            if let Ok(value) = serde_json::from_str::<Value>(&content) {
                values.push((ks_file, value));
            }
        }
    }
    values
}
