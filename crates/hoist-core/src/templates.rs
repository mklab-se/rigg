//! README generation templates

use std::path::Path;

use handlebars::Handlebars;
use serde::Serialize;
use thiserror::Error;

use crate::resources::ResourceKind;

/// Template errors
#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("Template error: {0}")]
    Render(#[from] handlebars::RenderError),
    #[error("Template registration error: {0}")]
    Registration(#[from] Box<handlebars::TemplateError>),
}

/// Template context for main README
#[derive(Debug, Serialize)]
pub struct ProjectContext {
    pub name: String,
    pub description: Option<String>,
    pub service_name: String,
    pub resource_counts: Vec<ResourceCount>,
    pub generated_at: String,
}

#[derive(Debug, Serialize)]
pub struct ResourceCount {
    pub kind: String,
    pub directory: String,
    pub count: usize,
}

/// Template context for resource-type README
#[derive(Debug, Serialize)]
pub struct ResourceTypeContext {
    pub kind: String,
    pub kind_plural: String,
    pub resources: Vec<ResourceSummary>,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct ResourceSummary {
    pub name: String,
    pub description: Option<String>,
    pub dependencies: Vec<String>,
}

/// Template context for SEARCH_CONFIG.md
#[derive(Debug, Serialize)]
pub struct SearchConfigContext {
    pub service_name: String,
    pub generated_at: String,
    pub indexes: Vec<IndexSummary>,
    pub index_count: usize,
    pub indexers: Vec<IndexerSummary>,
    pub indexer_count: usize,
    pub datasources: Vec<DataSourceSummary>,
    pub datasource_count: usize,
    pub skillsets: Vec<SkillsetSummary>,
    pub skillset_count: usize,
    pub synonym_maps: Vec<SynonymMapSummary>,
    pub synonym_map_count: usize,
    pub knowledge_bases: Vec<KnowledgeBaseSummary>,
    pub knowledge_base_count: usize,
    pub knowledge_sources: Vec<KnowledgeSourceSummary>,
    pub knowledge_source_count: usize,
}

#[derive(Debug, Serialize)]
pub struct IndexSummary {
    pub name: String,
    pub field_count: usize,
    pub key_field: Option<String>,
    pub has_vector_search: bool,
    pub has_semantic: bool,
}

#[derive(Debug, Serialize)]
pub struct IndexerSummary {
    pub name: String,
    pub data_source: String,
    pub target_index: String,
    pub skillset: Option<String>,
    pub has_schedule: bool,
}

#[derive(Debug, Serialize)]
pub struct DataSourceSummary {
    pub name: String,
    pub source_type: String,
    pub container: String,
}

#[derive(Debug, Serialize)]
pub struct SkillsetSummary {
    pub name: String,
    pub skill_count: usize,
    pub skills: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SynonymMapSummary {
    pub name: String,
    pub format: String,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeBaseSummary {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KnowledgeSourceSummary {
    pub name: String,
    pub index_name: String,
    pub knowledge_base: Option<String>,
}

/// README generator
pub struct ReadmeGenerator {
    handlebars: Handlebars<'static>,
}

impl ReadmeGenerator {
    pub fn new() -> Result<Self, TemplateError> {
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);

        // Register templates
        handlebars
            .register_template_string("project", PROJECT_README_TEMPLATE)
            .map_err(Box::new)?;
        handlebars
            .register_template_string("resource_type", RESOURCE_TYPE_README_TEMPLATE)
            .map_err(Box::new)?;
        handlebars
            .register_template_string("search_config", SEARCH_CONFIG_TEMPLATE)
            .map_err(Box::new)?;

        Ok(Self { handlebars })
    }

    /// Generate main project README
    pub fn generate_project_readme(&self, ctx: &ProjectContext) -> Result<String, TemplateError> {
        Ok(self.handlebars.render("project", ctx)?)
    }

    /// Generate resource-type README
    pub fn generate_resource_readme(
        &self,
        ctx: &ResourceTypeContext,
    ) -> Result<String, TemplateError> {
        Ok(self.handlebars.render("resource_type", ctx)?)
    }

    /// Generate SEARCH_CONFIG.md content
    pub fn generate_search_config(
        &self,
        ctx: &SearchConfigContext,
    ) -> Result<String, TemplateError> {
        Ok(self.handlebars.render("search_config", ctx)?)
    }
}

impl Default for ReadmeGenerator {
    fn default() -> Self {
        Self::new().expect("Failed to create ReadmeGenerator")
    }
}

/// Get description for a resource kind
pub fn resource_kind_description(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Index => {
            "Indexes define the schema for searchable content, including fields, analyzers, and scoring profiles."
        }
        ResourceKind::Indexer => {
            "Indexers automate data ingestion from supported data sources into search indexes."
        }
        ResourceKind::DataSource => {
            "Data sources define connections to external data stores for indexer ingestion."
        }
        ResourceKind::Skillset => {
            "Skillsets define AI enrichment pipelines that process content during indexing."
        }
        ResourceKind::SynonymMap => {
            "Synonym maps define equivalent terms to improve search relevance."
        }
        ResourceKind::KnowledgeBase => {
            "Knowledge bases (preview) provide structured knowledge for AI agent interactions."
        }
        ResourceKind::KnowledgeSource => {
            "Knowledge sources (preview) connect indexes to knowledge bases for agentic search."
        }
    }
}

const PROJECT_README_TEMPLATE: &str = r#"# {{name}}

{{#if description}}
{{description}}

{{/if}}
Azure AI Search configuration managed by [hoist](https://github.com/mklab-se/hoist).

## Service

- **Service name**: `{{service_name}}`

## Resources

| Type | Directory | Count |
|------|-----------|-------|
{{#each resource_counts}}
| {{kind}} | [`{{directory}}/`](./{{directory}}/) | {{count}} |
{{/each}}

## Usage

```bash
# Pull latest configuration from Azure
hoist pull

# Show differences between local and remote
hoist diff

# Push local changes to Azure
hoist push --dry-run
hoist push
```

---

*Generated by hoist on {{generated_at}}*
"#;

const RESOURCE_TYPE_README_TEMPLATE: &str = r#"# {{kind_plural}}

{{description}}

## Resources

{{#each resources}}
### {{name}}

{{#if description}}
{{description}}

{{/if}}
{{#if dependencies}}
**Dependencies:**
{{#each dependencies}}
- {{this}}
{{/each}}
{{/if}}

{{/each}}
"#;

const SEARCH_CONFIG_TEMPLATE: &str = r#"# Search Configuration

Azure AI Search service: **{{service_name}}**

This document provides a complete overview of all resource definitions managed by hoist.

## Indexes ({{index_count}})

{{#each indexes}}
### {{name}}

- **Fields**: {{field_count}}
- **Key field**: {{#if key_field}}`{{key_field}}`{{else}}(none){{/if}}
{{#if has_vector_search}}- Vector search enabled
{{/if}}
{{#if has_semantic}}- Semantic search enabled
{{/if}}

{{/each}}
## Indexers ({{indexer_count}})

{{#each indexers}}
### {{name}}

- **Data source**: `{{data_source}}`
- **Target index**: `{{target_index}}`
{{#if skillset}}- **Skillset**: `{{skillset}}`
{{/if}}
{{#if has_schedule}}- Scheduled
{{/if}}

{{/each}}
## Data Sources ({{datasource_count}})

{{#each datasources}}
### {{name}}

- **Type**: {{source_type}}
- **Container**: `{{container}}`

{{/each}}
## Skillsets ({{skillset_count}})

{{#each skillsets}}
### {{name}}

- **Skills** ({{skill_count}}):
{{#each skills}}
  - {{this}}
{{/each}}

{{/each}}
## Synonym Maps ({{synonym_map_count}})

{{#each synonym_maps}}
### {{name}}

- **Format**: {{format}}

{{/each}}
{{#if knowledge_bases.length}}
## Knowledge Bases ({{knowledge_base_count}})

{{#each knowledge_bases}}
### {{name}}

{{#if description}}
{{description}}

{{/if}}
{{/each}}
{{/if}}
{{#if knowledge_sources.length}}
## Knowledge Sources ({{knowledge_source_count}})

{{#each knowledge_sources}}
### {{name}}

- **Index**: `{{index_name}}`
{{#if knowledge_base}}- **Knowledge base**: `{{knowledge_base}}`
{{/if}}

{{/each}}
{{/if}}
## Dependencies

{{#each indexers}}
- **{{name}}** (indexer) -> `{{data_source}}` (data source), `{{target_index}}` (index){{#if skillset}}, `{{skillset}}` (skillset){{/if}}
{{/each}}
{{#each knowledge_sources}}
- **{{name}}** (knowledge source) -> `{{index_name}}` (index){{#if knowledge_base}}, `{{knowledge_base}}` (knowledge base){{/if}}
{{/each}}

---

*Generated by hoist on {{generated_at}}*
"#;

/// Build a `SearchConfigContext` by reading JSON files from the resource directory.
pub fn build_search_config_context(
    service_name: &str,
    resource_dir: &Path,
    include_preview: bool,
) -> Result<SearchConfigContext, Box<dyn std::error::Error>> {
    let generated_at = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();

    let indexes = read_summaries_from_dir(
        &resource_dir.join(ResourceKind::Index.directory_name()),
        parse_index_summary,
    );

    let indexers = read_summaries_from_dir(
        &resource_dir.join(ResourceKind::Indexer.directory_name()),
        parse_indexer_summary,
    );

    let datasources = read_summaries_from_dir(
        &resource_dir.join(ResourceKind::DataSource.directory_name()),
        parse_datasource_summary,
    );

    let skillsets = read_summaries_from_dir(
        &resource_dir.join(ResourceKind::Skillset.directory_name()),
        parse_skillset_summary,
    );

    let synonym_maps = read_summaries_from_dir(
        &resource_dir.join(ResourceKind::SynonymMap.directory_name()),
        parse_synonym_map_summary,
    );

    let (knowledge_bases, knowledge_sources) = if include_preview {
        let kbs = read_summaries_from_dir(
            &resource_dir.join(ResourceKind::KnowledgeBase.directory_name()),
            parse_knowledge_base_summary,
        );
        let kss = read_summaries_from_dir(
            &resource_dir.join(ResourceKind::KnowledgeSource.directory_name()),
            parse_knowledge_source_summary,
        );
        (kbs, kss)
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(SearchConfigContext {
        service_name: service_name.to_string(),
        generated_at,
        index_count: indexes.len(),
        indexes,
        indexer_count: indexers.len(),
        indexers,
        datasource_count: datasources.len(),
        datasources,
        skillset_count: skillsets.len(),
        skillsets,
        synonym_map_count: synonym_maps.len(),
        synonym_maps,
        knowledge_base_count: knowledge_bases.len(),
        knowledge_bases,
        knowledge_source_count: knowledge_sources.len(),
        knowledge_sources,
    })
}

/// Read all JSON files from a directory and parse each into a summary using the given function.
fn read_summaries_from_dir<T, F>(dir: &Path, parse: F) -> Vec<T>
where
    F: Fn(&serde_json::Value) -> T,
{
    let mut results = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return results,
    };

    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect();
    paths.sort();

    for path in paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
                results.push(parse(&value));
            }
        }
    }

    results
}

fn get_str(value: &serde_json::Value, key: &str) -> String {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

fn get_opt_str(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn parse_index_summary(value: &serde_json::Value) -> IndexSummary {
    let name = get_str(value, "name");
    let fields = value
        .get("fields")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let field_count = fields.len();
    let key_field = fields
        .iter()
        .find(|f| f.get("key").and_then(|k| k.as_bool()).unwrap_or(false))
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());
    let has_vector_search = value.get("vectorSearch").is_some();
    let has_semantic = value.get("semantic").is_some();

    IndexSummary {
        name,
        field_count,
        key_field,
        has_vector_search,
        has_semantic,
    }
}

fn parse_indexer_summary(value: &serde_json::Value) -> IndexerSummary {
    let name = get_str(value, "name");
    let data_source = get_str(value, "dataSourceName");
    let target_index = get_str(value, "targetIndexName");
    let skillset = get_opt_str(value, "skillsetName");
    let has_schedule = value.get("schedule").map(|s| !s.is_null()).unwrap_or(false);

    IndexerSummary {
        name,
        data_source,
        target_index,
        skillset,
        has_schedule,
    }
}

fn parse_datasource_summary(value: &serde_json::Value) -> DataSourceSummary {
    let name = get_str(value, "name");
    let source_type = get_str(value, "type");
    let container = value
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

fn parse_skillset_summary(value: &serde_json::Value) -> SkillsetSummary {
    let name = get_str(value, "name");
    let skills_arr = value
        .get("skills")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let skill_count = skills_arr.len();
    let skills: Vec<String> = skills_arr
        .iter()
        .filter_map(|s| {
            s.get("@odata.type")
                .and_then(|t| t.as_str())
                .map(|t| t.trim_start_matches('#').to_string())
        })
        .collect();

    SkillsetSummary {
        name,
        skill_count,
        skills,
    }
}

fn parse_synonym_map_summary(value: &serde_json::Value) -> SynonymMapSummary {
    let name = get_str(value, "name");
    let format = get_str(value, "format");

    SynonymMapSummary { name, format }
}

fn parse_knowledge_base_summary(value: &serde_json::Value) -> KnowledgeBaseSummary {
    let name = get_str(value, "name");
    let description = get_opt_str(value, "description");

    KnowledgeBaseSummary { name, description }
}

fn parse_knowledge_source_summary(value: &serde_json::Value) -> KnowledgeSourceSummary {
    let name = get_str(value, "name");
    let index_name = get_str(value, "indexName");
    let knowledge_base = get_opt_str(value, "knowledgeBaseName");

    KnowledgeSourceSummary {
        name,
        index_name,
        knowledge_base,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceKind;

    #[test]
    fn test_readme_generator_creation() {
        let gen = ReadmeGenerator::new();
        assert!(gen.is_ok());
    }

    #[test]
    fn test_project_readme_with_description() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = ProjectContext {
            name: "my-search".to_string(),
            description: Some("A search project for testing".to_string()),
            service_name: "my-search-svc".to_string(),
            resource_counts: vec![],
            generated_at: "2025-01-01".to_string(),
        };
        let output = gen.generate_project_readme(&ctx).unwrap();
        assert!(output.contains("my-search"));
        assert!(output.contains("A search project for testing"));
        assert!(output.contains("my-search-svc"));
    }

    #[test]
    fn test_project_readme_without_description() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = ProjectContext {
            name: "my-search".to_string(),
            description: None,
            service_name: "my-search-svc".to_string(),
            resource_counts: vec![],
            generated_at: "2025-01-01".to_string(),
        };
        let output = gen.generate_project_readme(&ctx).unwrap();
        assert!(output.contains("my-search"));
        assert!(output.contains("my-search-svc"));
    }

    #[test]
    fn test_project_readme_resource_counts() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = ProjectContext {
            name: "my-search".to_string(),
            description: None,
            service_name: "svc".to_string(),
            resource_counts: vec![
                ResourceCount {
                    kind: "Index".to_string(),
                    directory: "search-management/indexes".to_string(),
                    count: 3,
                },
                ResourceCount {
                    kind: "Skillset".to_string(),
                    directory: "search-management/skillsets".to_string(),
                    count: 1,
                },
            ],
            generated_at: "2025-01-01".to_string(),
        };
        let output = gen.generate_project_readme(&ctx).unwrap();
        assert!(output.contains("Index"));
        assert!(output.contains("search-management/indexes"));
        assert!(output.contains("3"));
        assert!(output.contains("Skillset"));
        assert!(output.contains("search-management/skillsets"));
        assert!(output.contains("1"));
    }

    #[test]
    fn test_resource_readme_with_dependencies() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = ResourceTypeContext {
            kind: "Indexer".to_string(),
            kind_plural: "Indexers".to_string(),
            resources: vec![ResourceSummary {
                name: "my-indexer".to_string(),
                description: Some("Indexes documents".to_string()),
                dependencies: vec![
                    "index/my-index".to_string(),
                    "data-source/my-ds".to_string(),
                ],
            }],
            description: "Indexers automate data ingestion.".to_string(),
        };
        let output = gen.generate_resource_readme(&ctx).unwrap();
        assert!(output.contains("Indexers"));
        assert!(output.contains("my-indexer"));
        assert!(output.contains("Indexes documents"));
        assert!(output.contains("index/my-index"));
        assert!(output.contains("data-source/my-ds"));
        assert!(output.contains("Dependencies"));
    }

    #[test]
    fn test_resource_readme_empty_resources() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = ResourceTypeContext {
            kind: "Index".to_string(),
            kind_plural: "Indexes".to_string(),
            resources: vec![],
            description: "Indexes define the schema.".to_string(),
        };
        let output = gen.generate_resource_readme(&ctx).unwrap();
        assert!(output.contains("Indexes"));
        assert!(output.contains("Indexes define the schema."));
    }

    #[test]
    fn test_resource_kind_description_all_kinds() {
        for kind in ResourceKind::all() {
            let desc = resource_kind_description(*kind);
            assert!(
                !desc.is_empty(),
                "Description for {:?} should not be empty",
                kind
            );
        }
    }
}
