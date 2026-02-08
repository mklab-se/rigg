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
{{#if knowledge_base_count}}
## Knowledge Bases ({{knowledge_base_count}})

{{#each knowledge_bases}}
### {{name}}

{{#if description}}
{{description}}

{{/if}}
{{/each}}
{{/if}}
{{#if knowledge_source_count}}
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

    #[test]
    fn test_generate_search_config_renders_markdown() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = SearchConfigContext {
            service_name: "my-search-svc".to_string(),
            generated_at: "2025-06-01 12:00 UTC".to_string(),
            index_count: 1,
            indexes: vec![IndexSummary {
                name: "products".to_string(),
                field_count: 10,
                key_field: Some("id".to_string()),
                has_vector_search: true,
                has_semantic: false,
            }],
            indexer_count: 1,
            indexers: vec![IndexerSummary {
                name: "product-indexer".to_string(),
                data_source: "cosmosdb-products".to_string(),
                target_index: "products".to_string(),
                skillset: Some("enrichment".to_string()),
                has_schedule: true,
            }],
            datasource_count: 1,
            datasources: vec![DataSourceSummary {
                name: "cosmosdb-products".to_string(),
                source_type: "cosmosdb".to_string(),
                container: "products-container".to_string(),
            }],
            skillset_count: 1,
            skillsets: vec![SkillsetSummary {
                name: "enrichment".to_string(),
                skill_count: 2,
                skills: vec![
                    "Microsoft.Skills.Text.EntityRecognitionSkill".to_string(),
                    "Microsoft.Skills.Text.KeyPhraseExtractionSkill".to_string(),
                ],
            }],
            synonym_map_count: 1,
            synonym_maps: vec![SynonymMapSummary {
                name: "my-synonyms".to_string(),
                format: "solr".to_string(),
            }],
            knowledge_base_count: 0,
            knowledge_bases: vec![],
            knowledge_source_count: 0,
            knowledge_sources: vec![],
        };
        let output = gen.generate_search_config(&ctx).unwrap();

        assert!(output.contains("# Search Configuration"));
        assert!(output.contains("**my-search-svc**"));
        assert!(output.contains("### products"));
        assert!(output.contains("**Fields**: 10"));
        assert!(output.contains("`id`"));
        assert!(output.contains("Vector search enabled"));
        assert!(!output.contains("Semantic search enabled"));
        assert!(output.contains("### product-indexer"));
        assert!(output.contains("`cosmosdb-products`"));
        assert!(output.contains("`products`"));
        assert!(output.contains("`enrichment`"));
        assert!(output.contains("Scheduled"));
        assert!(output.contains("### cosmosdb-products"));
        assert!(output.contains("cosmosdb"));
        assert!(output.contains("`products-container`"));
        assert!(output.contains("### enrichment"));
        assert!(output.contains("Microsoft.Skills.Text.EntityRecognitionSkill"));
        assert!(output.contains("### my-synonyms"));
        assert!(output.contains("solr"));
        assert!(output.contains("## Dependencies"));
        assert!(output.contains("*Generated by hoist on 2025-06-01 12:00 UTC*"));
    }

    #[test]
    fn test_generate_search_config_empty_context() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = SearchConfigContext {
            service_name: "empty-svc".to_string(),
            generated_at: "2025-01-01".to_string(),
            index_count: 0,
            indexes: vec![],
            indexer_count: 0,
            indexers: vec![],
            datasource_count: 0,
            datasources: vec![],
            skillset_count: 0,
            skillsets: vec![],
            synonym_map_count: 0,
            synonym_maps: vec![],
            knowledge_base_count: 0,
            knowledge_bases: vec![],
            knowledge_source_count: 0,
            knowledge_sources: vec![],
        };
        let output = gen.generate_search_config(&ctx).unwrap();
        assert!(output.contains("# Search Configuration"));
        assert!(output.contains("**empty-svc**"));
    }

    #[test]
    fn test_generate_search_config_with_knowledge_bases() {
        let gen = ReadmeGenerator::new().unwrap();
        let ctx = SearchConfigContext {
            service_name: "kb-svc".to_string(),
            generated_at: "2025-01-01".to_string(),
            index_count: 0,
            indexes: vec![],
            indexer_count: 0,
            indexers: vec![],
            datasource_count: 0,
            datasources: vec![],
            skillset_count: 0,
            skillsets: vec![],
            synonym_map_count: 0,
            synonym_maps: vec![],
            knowledge_base_count: 1,
            knowledge_bases: vec![KnowledgeBaseSummary {
                name: "my-kb".to_string(),
                description: Some("A knowledge base".to_string()),
            }],
            knowledge_source_count: 1,
            knowledge_sources: vec![KnowledgeSourceSummary {
                name: "my-ks".to_string(),
                index_name: "products".to_string(),
                knowledge_base: Some("my-kb".to_string()),
            }],
        };
        let output = gen.generate_search_config(&ctx).unwrap();
        assert!(output.contains("## Knowledge Bases"));
        assert!(output.contains("### my-kb"));
        assert!(output.contains("A knowledge base"));
        assert!(output.contains("## Knowledge Sources"));
        assert!(output.contains("### my-ks"));
        assert!(output.contains("`products`"));
        assert!(output.contains("`my-kb`"));
    }

    #[test]
    fn test_build_search_config_context_with_sample_files() {
        let dir = tempfile::tempdir().unwrap();
        let resource_dir = dir.path();

        // Create index directory with a sample file
        let index_dir = resource_dir.join("search-management/indexes");
        std::fs::create_dir_all(&index_dir).unwrap();
        std::fs::write(
            index_dir.join("products.json"),
            r#"{
                "name": "products",
                "fields": [
                    {"name": "id", "type": "Edm.String", "key": true},
                    {"name": "title", "type": "Edm.String", "key": false},
                    {"name": "description", "type": "Edm.String", "key": false}
                ],
                "vectorSearch": {"profiles": []},
                "semantic": {"configurations": []}
            }"#,
        )
        .unwrap();

        // Create indexer directory with a sample file
        let indexer_dir = resource_dir.join("search-management/indexers");
        std::fs::create_dir_all(&indexer_dir).unwrap();
        std::fs::write(
            indexer_dir.join("product-indexer.json"),
            r#"{
                "name": "product-indexer",
                "dataSourceName": "cosmos-ds",
                "targetIndexName": "products",
                "skillsetName": "my-skillset",
                "schedule": {"interval": "PT5M"}
            }"#,
        )
        .unwrap();

        // Create data source directory
        let ds_dir = resource_dir.join("search-management/data-sources");
        std::fs::create_dir_all(&ds_dir).unwrap();
        std::fs::write(
            ds_dir.join("cosmos-ds.json"),
            r#"{
                "name": "cosmos-ds",
                "type": "cosmosdb",
                "container": {"name": "my-container"}
            }"#,
        )
        .unwrap();

        // Create skillset directory
        let skill_dir = resource_dir.join("search-management/skillsets");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("my-skillset.json"),
            r##"{
                "name": "my-skillset",
                "skills": [
                    {"@odata.type": "#Microsoft.Skills.Text.EntityRecognitionSkill", "name": "entity"},
                    {"@odata.type": "#Microsoft.Skills.Text.KeyPhraseExtractionSkill", "name": "keyphrase"}
                ]
            }"##,
        )
        .unwrap();

        // Create synonym map directory
        let syn_dir = resource_dir.join("search-management/synonym-maps");
        std::fs::create_dir_all(&syn_dir).unwrap();
        std::fs::write(
            syn_dir.join("my-synonyms.json"),
            r#"{"name": "my-synonyms", "format": "solr"}"#,
        )
        .unwrap();

        let ctx = build_search_config_context("test-svc", resource_dir, false).unwrap();

        assert_eq!(ctx.service_name, "test-svc");
        assert_eq!(ctx.indexes.len(), 1);
        assert_eq!(ctx.index_count, 1);
        assert_eq!(ctx.indexes[0].name, "products");
        assert_eq!(ctx.indexes[0].field_count, 3);
        assert_eq!(ctx.indexes[0].key_field, Some("id".to_string()));
        assert!(ctx.indexes[0].has_vector_search);
        assert!(ctx.indexes[0].has_semantic);

        assert_eq!(ctx.indexers.len(), 1);
        assert_eq!(ctx.indexer_count, 1);
        assert_eq!(ctx.indexers[0].name, "product-indexer");
        assert_eq!(ctx.indexers[0].data_source, "cosmos-ds");
        assert_eq!(ctx.indexers[0].target_index, "products");
        assert_eq!(ctx.indexers[0].skillset, Some("my-skillset".to_string()));
        assert!(ctx.indexers[0].has_schedule);

        assert_eq!(ctx.datasources.len(), 1);
        assert_eq!(ctx.datasource_count, 1);
        assert_eq!(ctx.datasources[0].name, "cosmos-ds");
        assert_eq!(ctx.datasources[0].source_type, "cosmosdb");
        assert_eq!(ctx.datasources[0].container, "my-container");

        assert_eq!(ctx.skillsets.len(), 1);
        assert_eq!(ctx.skillset_count, 1);
        assert_eq!(ctx.skillsets[0].name, "my-skillset");
        assert_eq!(ctx.skillsets[0].skill_count, 2);
        assert_eq!(ctx.skillsets[0].skills.len(), 2);
        assert!(ctx.skillsets[0]
            .skills
            .contains(&"Microsoft.Skills.Text.EntityRecognitionSkill".to_string()));

        assert_eq!(ctx.synonym_maps.len(), 1);
        assert_eq!(ctx.synonym_map_count, 1);
        assert_eq!(ctx.synonym_maps[0].name, "my-synonyms");
        assert_eq!(ctx.synonym_maps[0].format, "solr");

        // Preview resources should be empty when include_preview is false
        assert!(ctx.knowledge_bases.is_empty());
        assert!(ctx.knowledge_sources.is_empty());
    }

    #[test]
    fn test_build_search_config_context_with_preview() {
        let dir = tempfile::tempdir().unwrap();
        let resource_dir = dir.path();

        // Create knowledge base directory
        let kb_dir = resource_dir.join("agentic-retrieval/knowledge-bases");
        std::fs::create_dir_all(&kb_dir).unwrap();
        std::fs::write(
            kb_dir.join("my-kb.json"),
            r#"{"name": "my-kb", "description": "Test KB"}"#,
        )
        .unwrap();

        // Create knowledge source directory
        let ks_dir = resource_dir.join("agentic-retrieval/knowledge-sources");
        std::fs::create_dir_all(&ks_dir).unwrap();
        std::fs::write(
            ks_dir.join("my-ks.json"),
            r#"{"name": "my-ks", "indexName": "products", "knowledgeBaseName": "my-kb"}"#,
        )
        .unwrap();

        let ctx = build_search_config_context("test-svc", resource_dir, true).unwrap();

        assert_eq!(ctx.knowledge_bases.len(), 1);
        assert_eq!(ctx.knowledge_base_count, 1);
        assert_eq!(ctx.knowledge_bases[0].name, "my-kb");
        assert_eq!(
            ctx.knowledge_bases[0].description,
            Some("Test KB".to_string())
        );

        assert_eq!(ctx.knowledge_sources.len(), 1);
        assert_eq!(ctx.knowledge_source_count, 1);
        assert_eq!(ctx.knowledge_sources[0].name, "my-ks");
        assert_eq!(ctx.knowledge_sources[0].index_name, "products");
        assert_eq!(
            ctx.knowledge_sources[0].knowledge_base,
            Some("my-kb".to_string())
        );
    }

    #[test]
    fn test_build_search_config_context_missing_directories() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = build_search_config_context("test-svc", dir.path(), true).unwrap();

        assert!(ctx.indexes.is_empty());
        assert!(ctx.indexers.is_empty());
        assert!(ctx.datasources.is_empty());
        assert!(ctx.skillsets.is_empty());
        assert!(ctx.synonym_maps.is_empty());
        assert!(ctx.knowledge_bases.is_empty());
        assert!(ctx.knowledge_sources.is_empty());
    }

    #[test]
    fn test_build_search_config_context_renders_valid_markdown() {
        let dir = tempfile::tempdir().unwrap();
        let resource_dir = dir.path();

        let index_dir = resource_dir.join("search-management/indexes");
        std::fs::create_dir_all(&index_dir).unwrap();
        std::fs::write(
            index_dir.join("test-idx.json"),
            r#"{"name": "test-idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}"#,
        )
        .unwrap();

        let ctx = build_search_config_context("test-svc", resource_dir, false).unwrap();
        let gen = ReadmeGenerator::new().unwrap();
        let output = gen.generate_search_config(&ctx).unwrap();

        assert!(output.contains("# Search Configuration"));
        assert!(output.contains("### test-idx"));
        assert!(output.contains("**Fields**: 1"));
        assert!(output.contains("`id`"));
    }

    #[test]
    fn test_parse_index_summary_no_vector_no_semantic() {
        let value: serde_json::Value = serde_json::json!({
            "name": "simple-index",
            "fields": [
                {"name": "id", "type": "Edm.String", "key": true},
                {"name": "content", "type": "Edm.String", "key": false}
            ]
        });
        let summary = parse_index_summary(&value);
        assert_eq!(summary.name, "simple-index");
        assert_eq!(summary.field_count, 2);
        assert_eq!(summary.key_field, Some("id".to_string()));
        assert!(!summary.has_vector_search);
        assert!(!summary.has_semantic);
    }

    #[test]
    fn test_parse_indexer_summary_no_skillset_no_schedule() {
        let value: serde_json::Value = serde_json::json!({
            "name": "simple-indexer",
            "dataSourceName": "my-ds",
            "targetIndexName": "my-idx"
        });
        let summary = parse_indexer_summary(&value);
        assert_eq!(summary.name, "simple-indexer");
        assert_eq!(summary.data_source, "my-ds");
        assert_eq!(summary.target_index, "my-idx");
        assert!(summary.skillset.is_none());
        assert!(!summary.has_schedule);
    }
}
