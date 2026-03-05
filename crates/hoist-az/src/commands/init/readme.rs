//! README generation for newly initialized projects

use std::path::Path;

use anyhow::Result;
use tracing::info;

use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;

/// Create a README.md in the project root if one doesn't already exist.
/// Includes project overview, CLI quick start, directory layout, JSON conventions,
/// and resource type reference for all configured service domains.
pub(super) fn create_readme_if_missing(
    project_dir: &Path,
    env: &hoist_core::config::ResolvedEnvironment,
    project_name: &str,
    resource_kinds: &[ResourceKind],
) -> Result<()> {
    let readme_path = project_dir.join("README.md");
    if readme_path.exists() {
        return Ok(());
    }

    let mut services_section = String::new();
    for svc in &env.search {
        services_section.push_str(&format!("- **Azure AI Search**: `{}`\n", svc.name));
    }
    for svc in &env.foundry {
        services_section.push_str(&format!(
            "- **Microsoft Foundry**: `{}` (project: `{}`)\n",
            svc.name, svc.project
        ));
    }

    // Build directory rows for the project structure table
    let search_kinds: Vec<&ResourceKind> = resource_kinds
        .iter()
        .filter(|k| k.domain() == ServiceDomain::Search)
        .collect();
    let has_foundry = env.has_foundry();

    let mut directory_rows = String::new();
    directory_rows.push_str(
        "| `hoist.yaml` | Project configuration: service name, API versions, sync settings |\n",
    );
    directory_rows.push_str("| `.hoist/` | Local state directory (auto-managed, gitignored) |\n");
    for kind in &search_kinds {
        directory_rows.push_str(&format!(
            "| `search/{}/` | {} resource definitions. [API reference]({}) |\n",
            kind.directory_name(),
            kind.display_name(),
            api_doc_url(**kind)
        ));
    }
    if has_foundry {
        directory_rows.push_str("| `foundry/agents/` | Microsoft Foundry agent definitions |\n");
    }

    // Build resource type reference sections
    let has_search_management = search_kinds.iter().any(|k| {
        matches!(
            **k,
            ResourceKind::Index
                | ResourceKind::Indexer
                | ResourceKind::DataSource
                | ResourceKind::Skillset
                | ResourceKind::SynonymMap
                | ResourceKind::Alias
        )
    });
    let has_agentic_retrieval = search_kinds.iter().any(|k| {
        matches!(
            **k,
            ResourceKind::KnowledgeBase | ResourceKind::KnowledgeSource
        )
    });

    let mut resource_reference = String::new();
    if has_search_management {
        resource_reference.push_str(SEARCH_MANAGEMENT_SECTION);
    }
    if has_agentic_retrieval {
        resource_reference.push_str(AGENTIC_RETRIEVAL_SECTION);
    }
    if has_foundry {
        resource_reference.push_str(FOUNDRY_AGENTS_SECTION);
    }

    let readme = format!(
        r#"# {project_name}

Configuration-as-code managed by [hoist](https://github.com/mklab-se/hoist).

## Services

{services_section}
## Quick Start

```bash
# Check authentication status
hoist auth status

# Pull all resource definitions from Azure
hoist pull --all

# Show what's configured
hoist status

# View detailed service description
hoist describe

# Show differences between local files and Azure
hoist diff --all

# Push local changes to Azure (preview first)
hoist push --all --dry-run
hoist push --all

# Watch for remote changes
hoist pull-watch --all
```

## Validating Configuration

```bash
# Validate local resource files for errors
hoist validate --all
```

## Project Structure

| Path | Description |
|------|-------------|
{directory_rows}
## JSON File Conventions

Each `.json` file represents one resource. The files follow these conventions:

- **Filename = resource name.** A file named `my-index.json` defines the resource named `my-index`.
- **Same schema as the REST API.** The JSON structure matches the request/response body of the corresponding Azure REST API endpoint.
- **Property order is preserved.** Properties appear in the order returned by the Azure API.
- **Volatile fields are stripped.** `@odata.etag` and `@odata.context` are removed to keep files environment-independent and diff-friendly.
- **Secrets are excluded.** Connection strings, credentials, and storage secrets are never stored in these files.

## Resource Type Reference

{resource_reference}## Learn More

- Run `hoist --help` for all available commands
- Run `hoist <command> --help` for command-specific options
- [Azure AI Search REST API](https://learn.microsoft.com/en-us/rest/api/searchservice/)
- [Azure AI Search overview](https://learn.microsoft.com/en-us/azure/search/search-what-is-azure-search)
"#,
        project_name = project_name,
        services_section = services_section,
        directory_rows = directory_rows,
        resource_reference = resource_reference,
    );

    std::fs::write(&readme_path, readme)?;
    info!("Created README.md");

    Ok(())
}

/// Create a README.md in the files-path directory explaining what the files are.
/// Only created if one doesn't already exist.
pub(super) fn create_files_path_readme_if_missing(files_dir: &Path) -> Result<()> {
    let readme_path = files_dir.join("README.md");
    if readme_path.exists() {
        return Ok(());
    }

    let readme = r#"# Hoist Resource Configuration

This directory contains Azure AI Search and Microsoft Foundry resource definitions managed by [Hoist](https://github.com/mklab-se/hoist).

These files are pulled from and pushed to Azure services using the `hoist` CLI tool. They enable version-controlled, configuration-as-code management of search indexes, indexers, skillsets, and other resources.

## Directory Structure

- `search/` — Azure AI Search resources (indexes, indexers, data sources, skillsets, etc.)
- `foundry/` — Microsoft Foundry resources (agents)

## Getting Started

```sh
hoist pull   # Pull latest resource definitions from Azure
hoist push   # Push local changes to Azure
hoist diff   # Compare local vs remote
hoist status # Show sync status
```

For more information, see the [Hoist documentation](https://github.com/mklab-se/hoist).
"#;

    std::fs::write(&readme_path, readme)?;
    info!("Created README.md in files directory");

    Ok(())
}

pub(super) fn api_doc_url(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Index => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/indexes/create-or-update"
        }
        ResourceKind::Indexer => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/indexers/create-or-update"
        }
        ResourceKind::DataSource => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/data-sources/create-or-update"
        }
        ResourceKind::Skillset => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/skillsets/create-or-update"
        }
        ResourceKind::SynonymMap => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/synonym-maps/create-or-update"
        }
        ResourceKind::Alias => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/indexes/create-or-update"
        }
        ResourceKind::KnowledgeBase => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-bases/create-or-update?view=rest-searchservice-2025-05-01-preview"
        }
        ResourceKind::KnowledgeSource => {
            "https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-sources/create-or-update?view=rest-searchservice-2025-05-01-preview"
        }
        ResourceKind::Agent => "https://learn.microsoft.com/en-us/azure/ai-services/agents/",
    }
}

const SEARCH_MANAGEMENT_SECTION: &str = r##"### search-management/

Core search service resources. These use the stable API version.

#### indexes/

Defines the schema for searchable content: fields, data types, analyzers, vector search
configuration, and semantic ranking. Each index is a self-contained search corpus.

Key fields: `name`, `fields` (with `type`, `key`, `searchable`, `filterable`, etc.),
`vectorSearch`, `semantic`, `scoringProfiles`, `similarity`.

Note: existing field types cannot be changed after index creation. New fields can be added.

- [Create or Update Index](https://learn.microsoft.com/en-us/rest/api/searchservice/indexes/create-or-update)
- [Index schema reference](https://learn.microsoft.com/en-us/azure/search/search-what-is-an-index)

#### indexers/

Controls automated data ingestion from a data source into an index. Defines the schedule,
field mappings, and optional AI enrichment through a skillset.

Key fields: `name`, `dataSourceName`, `targetIndexName`, `skillsetName`, `schedule`,
`parameters`, `fieldMappings`, `outputFieldMappings`, `disabled`.

Dependencies: requires a data source (`dataSourceName`) and an index (`targetIndexName`).
Optionally references a skillset (`skillsetName`).

- [Create or Update Indexer](https://learn.microsoft.com/en-us/rest/api/searchservice/indexers/create-or-update)
- [Indexer overview](https://learn.microsoft.com/en-us/azure/search/search-indexer-overview)

#### data-sources/

Specifies the external data store that an indexer reads from (Azure Blob Storage, SQL, Cosmos DB, etc.)
and the change/deletion detection policies for incremental indexing.

Key fields: `name`, `type`, `container` (with `name` and `query`),
`dataChangeDetectionPolicy`, `dataDeletionDetectionPolicy`, `identity`.

Note: the `credentials` field (connection strings) is excluded from these files for security.
Manage credentials through the Azure portal or `az` CLI.

- [Create or Update Data Source](https://learn.microsoft.com/en-us/rest/api/searchservice/data-sources/create-or-update)
- [Data source types](https://learn.microsoft.com/en-us/azure/search/search-data-sources-gallery)

#### skillsets/

Defines an AI enrichment pipeline applied during indexing. Skills can split text, generate
embeddings, extract entities, translate content, project data into secondary indexes, and more.

Key fields: `name`, `skills` (each with `@odata.type`, `name`, `context`, `inputs`, `outputs`),
`indexProjections`, `knowledgeStore`.

Note: the `cognitiveServices` field is excluded from these files. Configure AI service
keys through the Azure portal.

- [Create or Update Skillset](https://learn.microsoft.com/en-us/rest/api/searchservice/skillsets/create-or-update)
- [Built-in skills reference](https://learn.microsoft.com/en-us/azure/search/cognitive-search-predefined-skills)

#### synonym-maps/

Defines synonym rules for query-time term expansion, allowing searches to match related terms.

Key fields: `name`, `format` (always `"solr"`), `synonyms` (one rule per line).

Synonym rule syntax:
- Equivalent: `"USA, United States, United States of America"`
- Explicit mapping: `"Washington, Wash. => WA"`

- [Create or Update Synonym Map](https://learn.microsoft.com/en-us/rest/api/searchservice/synonym-maps/create-or-update)
- [Synonym maps in Azure AI Search](https://learn.microsoft.com/en-us/azure/search/search-synonyms)

"##;

const AGENTIC_RETRIEVAL_SECTION: &str = r#"### agentic-retrieval/ (preview)

Resources for the Agentic Retrieval feature, which enables AI agents to query structured
knowledge bases. These use the preview API version and may change before general availability.

#### knowledge-bases/

Represents a curated collection of knowledge sources that AI agents can query. Defines
retrieval instructions, answer generation settings, and output mode.

Key fields: `name`, `description`, `retrievalInstructions`, `answerInstructions`,
`outputMode`, `knowledgeSources`, `models`.

Note: the `storageConnectionStringSecret` field is excluded from these files for security.

- [Create or Update Knowledge Base](https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-bases/create-or-update?view=rest-searchservice-2025-05-01-preview)

#### knowledge-sources/

Connects a data source to a knowledge base, defining how content is indexed and queried
by AI agents. Can reference Azure Blob storage, SharePoint, web content, and other sources.

Key fields: `name`, `kind`, `description`, `azureBlobParameters`, `searchIndexParameters`.

Dependencies: belongs to a knowledge base, which is specified during creation.

- [Create or Update Knowledge Source](https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-sources/create-or-update?view=rest-searchservice-2025-05-01-preview)

"#;

const FOUNDRY_AGENTS_SECTION: &str = r#"### agents/

Microsoft Foundry agent definitions. Each agent is stored as a directory of decomposed files
for easier editing and review.

#### Agent directory structure

```
agents/<agent-name>/
  config.json        # Agent metadata: id, name, model, temperature
  instructions.md    # Agent instructions as editable Markdown
  tools.json         # Tools array (code_interpreter, azure_search, etc.)
  knowledge.json     # Tool resources (knowledge base connections)
```

Key fields (in `config.json`): `name`, `model`, `temperature`, `top_p`, `metadata`.

The `instructions.md` file contains the agent's system prompt and can be edited directly.

- [Microsoft Foundry Agents documentation](https://learn.microsoft.com/en-us/azure/ai-services/agents/)

"#;

/// Create directory structure for an init template without prompts or ARM discovery.
/// Used internally for testing.
#[cfg(test)]
pub(super) fn create_project_dirs(
    project_dir: &Path,
    config: &hoist_core::config::Config,
    template: crate::cli::InitTemplate,
) -> Result<()> {
    std::fs::create_dir_all(project_dir)?;
    config.save(project_dir)?;

    let env = config
        .resolve_env(None)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // Compute files root (where resource dirs go)
    let files_dir = config.files_root(project_dir);
    if files_dir != project_dir.to_path_buf() {
        std::fs::create_dir_all(&files_dir)?;
    }

    // Create .hoist state directory (always in project_dir)
    let hoist_dir = project_dir.join(".hoist");
    let state_dir = hoist_dir.join(&env.name);
    std::fs::create_dir_all(&state_dir)?;
    let gitignore_path = hoist_dir.join(".gitignore");
    std::fs::write(&gitignore_path, "# Ignore local state\n*\n!.gitignore\n")?;

    let resource_kinds = match template {
        crate::cli::InitTemplate::Minimal => {
            vec![ResourceKind::Index, ResourceKind::DataSource]
        }
        crate::cli::InitTemplate::Full => vec![
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
            ResourceKind::Alias,
        ],
        crate::cli::InitTemplate::Agentic => ResourceKind::all().to_vec(),
    };

    // Create search directories (under files_dir)
    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_dir, search_svc);
        for kind in &resource_kinds {
            if kind.domain() == ServiceDomain::Search {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }
    }

    // Create foundry directories (under files_dir)
    for foundry_svc in &env.foundry {
        let foundry_base = env.foundry_service_dir(&files_dir, foundry_svc);
        std::fs::create_dir_all(foundry_base.join("agents"))?;
    }

    let project_name = config.project.name.as_deref().unwrap_or("hoist project");
    create_readme_if_missing(project_dir, &env, project_name, &resource_kinds)?;

    // Create README.md in files-path directory if separate from project root
    if files_dir != project_dir.to_path_buf() {
        create_files_path_readme_if_missing(&files_dir)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoist_core::config::{
        Config, EnvironmentConfig, FoundryServiceConfig, ProjectConfig, SearchServiceConfig,
        SyncConfig,
    };
    use tempfile::TempDir;

    use crate::cli::InitTemplate;

    fn make_config(service_name: &str) -> Config {
        Config {
            project: ProjectConfig {
                name: Some("test-project".to_string()),
                description: None,
                files_path: None,
            },
            sync: SyncConfig {
                include_preview: false,
                resources: Vec::new(),
            },
            environments: std::collections::BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: true,
                    description: None,
                    search: vec![SearchServiceConfig {
                        name: service_name.to_string(),
                        label: None,
                        subscription: None,
                        resource_group: None,
                        api_version: "2024-07-01".to_string(),
                        preview_api_version: "2025-11-01-preview".to_string(),
                    }],
                    foundry: vec![],
                },
            )]),
        }
    }

    #[test]
    fn test_minimal_template_creates_index_and_datasource_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let search_base = project_dir.join("search");
        assert!(search_base.join("search-management/indexes").is_dir());
        assert!(search_base.join("search-management/data-sources").is_dir());
        // Should NOT have indexers, skillsets, synonym-maps
        assert!(!search_base.join("search-management/indexers").exists());
        assert!(!search_base.join("search-management/skillsets").exists());
    }

    #[test]
    fn test_full_template_creates_all_stable_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Full).unwrap();

        let search_base = project_dir.join("search");
        assert!(search_base.join("search-management/indexes").is_dir());
        assert!(search_base.join("search-management/indexers").is_dir());
        assert!(search_base.join("search-management/data-sources").is_dir());
        assert!(search_base.join("search-management/skillsets").is_dir());
        assert!(search_base.join("search-management/synonym-maps").is_dir());
        assert!(search_base.join("search-management/aliases").is_dir());
        // Should NOT have preview dirs
        assert!(
            !search_base
                .join("agentic-retrieval/knowledge-bases")
                .exists()
        );
    }

    #[test]
    fn test_agentic_template_creates_preview_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let search_base = project_dir.join("search");
        assert!(search_base.join("search-management/indexes").is_dir());
        assert!(
            search_base
                .join("agentic-retrieval/knowledge-bases")
                .is_dir()
        );
        assert!(
            search_base
                .join("agentic-retrieval/knowledge-sources")
                .is_dir()
        );
    }

    #[test]
    fn test_config_file_created() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("my-search");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let config_path = project_dir.join(Config::FILENAME);
        assert!(config_path.exists());

        let loaded = Config::load(project_dir).unwrap();
        let env = loaded.resolve_env(None).unwrap();
        assert_eq!(env.search[0].name, "my-search");
        assert_eq!(env.search[0].api_version, "2024-07-01");
    }

    #[test]
    fn test_gitignore_created_in_state_dir() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let gitignore = project_dir.join(".hoist/.gitignore");
        assert!(gitignore.exists());

        let content = std::fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("*"));
        assert!(content.contains("!.gitignore"));
    }

    #[test]
    fn test_readme_includes_resource_reference() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("my-search-svc");

        create_project_dirs(project_dir, &config, InitTemplate::Full).unwrap();

        let content = std::fs::read_to_string(project_dir.join("README.md")).unwrap();
        assert!(content.contains("my-search-svc"));
        assert!(content.contains("indexes"));
        assert!(content.contains("Resource Type Reference"));
    }

    #[test]
    fn test_readme_includes_agentic_section_for_agentic_template() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let content = std::fs::read_to_string(project_dir.join("README.md")).unwrap();
        assert!(content.contains("knowledge-bases"));
        assert!(content.contains("Agentic Retrieval"));
    }

    #[test]
    fn test_no_subdirectory_readmes_created() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let search_base = project_dir.join("search");
        // No HOIST.md or category READMEs -- all content is in root README.md
        assert!(!search_base.join("HOIST.md").exists());
        assert!(!search_base.join("README.md").exists());
    }

    #[test]
    fn test_creates_dirs_under_search() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Resources should be under search/
        let search_base = project_dir.join("search");
        assert!(search_base.join("search-management/indexes").is_dir());
        assert!(search_base.join("search-management/data-sources").is_dir());
    }

    #[test]
    fn test_api_doc_url_returns_valid_urls() {
        for kind in ResourceKind::all() {
            let url = api_doc_url(*kind);
            assert!(url.starts_with("https://"));
            assert!(url.contains("learn.microsoft.com"));
        }
    }

    #[test]
    fn test_readme_created_during_init() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("my-search");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let readme = project_dir.join("README.md");
        assert!(readme.exists());

        let content = std::fs::read_to_string(&readme).unwrap();
        assert!(content.contains("hoist"));
        assert!(content.contains("my-search"));
        assert!(content.contains("hoist pull"));
        assert!(content.contains("hoist diff"));
        assert!(content.contains("hoist push"));
    }

    #[test]
    fn test_readme_not_overwritten_if_exists() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("my-search");

        // Create a pre-existing README.md
        std::fs::write(project_dir.join("README.md"), "# My Existing Project\n").unwrap();

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let content = std::fs::read_to_string(project_dir.join("README.md")).unwrap();
        assert_eq!(content, "# My Existing Project\n");
    }

    #[test]
    fn test_readme_includes_foundry_service() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = Config {
            project: ProjectConfig {
                name: Some("test-project".to_string()),
                description: None,
                files_path: None,
            },
            sync: SyncConfig {
                include_preview: false,
                resources: Vec::new(),
            },
            environments: std::collections::BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: true,
                    description: None,
                    search: vec![SearchServiceConfig {
                        name: "my-search".to_string(),
                        label: None,
                        subscription: None,
                        resource_group: None,
                        api_version: "2024-07-01".to_string(),
                        preview_api_version: "2025-11-01-preview".to_string(),
                    }],
                    foundry: vec![FoundryServiceConfig {
                        name: "my-ai-svc".to_string(),
                        project: "my-project".to_string(),
                        label: None,
                        api_version: "2025-05-15-preview".to_string(),
                        endpoint: None,
                        subscription: None,
                        resource_group: None,
                    }],
                },
            )]),
        };

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let content = std::fs::read_to_string(project_dir.join("README.md")).unwrap();
        assert!(content.contains("my-search"));
        assert!(content.contains("my-ai-svc"));
        assert!(content.contains("my-project"));
        assert!(content.contains("Microsoft Foundry"));
    }

    #[test]
    fn test_existing_config_detected() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        // First init should work
        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Config file should exist (additive mode would be triggered)
        let config_path = project_dir.join(Config::FILENAME);
        assert!(config_path.exists());

        // Verify config can be loaded for additive update
        let loaded = Config::load(project_dir).unwrap();
        let env = loaded.resolve_env(None).unwrap();
        assert_eq!(env.search[0].name, "test-service");
    }

    #[test]
    fn test_additive_init_creates_new_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();

        let config = make_config("svc-1");
        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Verify we can load and resolve the config
        let loaded = Config::load(project_dir).unwrap();
        let env = loaded.resolve_env(None).unwrap();
        assert_eq!(env.search[0].name, "svc-1");
        assert!(
            project_dir
                .join("search/search-management/indexes")
                .is_dir()
        );
    }

    #[test]
    fn test_files_path_creates_dirs_under_subdir() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let mut config = make_config("test-service");
        config.project.files_path = Some("hoist".to_string());

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Config should be at project root
        assert!(project_dir.join(Config::FILENAME).exists());
        // State should be at project root
        assert!(project_dir.join(".hoist/.gitignore").exists());
        // Resource dirs should be under hoist/
        assert!(
            project_dir
                .join("hoist/search/search-management/indexes")
                .is_dir()
        );
        assert!(
            project_dir
                .join("hoist/search/search-management/data-sources")
                .is_dir()
        );
        // Resource dirs should NOT be at project root
        assert!(!project_dir.join("search").exists());
    }

    #[test]
    fn test_files_path_config_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let mut config = make_config("test-service");
        config.project.files_path = Some("hoist".to_string());

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let loaded = Config::load(project_dir).unwrap();
        assert_eq!(loaded.project.files_path, Some("hoist".to_string()));
    }

    #[test]
    fn test_files_path_none_uses_project_dir() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Resource dirs should be at project root (no files_path)
        assert!(
            project_dir
                .join("search/search-management/indexes")
                .is_dir()
        );
    }

    #[test]
    fn test_files_path_with_foundry() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = Config {
            project: ProjectConfig {
                name: Some("test-project".to_string()),
                description: None,
                files_path: Some("resources".to_string()),
            },
            sync: SyncConfig {
                include_preview: false,
                resources: Vec::new(),
            },
            environments: std::collections::BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: true,
                    description: None,
                    search: vec![],
                    foundry: vec![FoundryServiceConfig {
                        name: "my-ai-svc".to_string(),
                        project: "my-project".to_string(),
                        label: None,
                        api_version: "2025-05-15-preview".to_string(),
                        endpoint: None,
                        subscription: None,
                        resource_group: None,
                    }],
                },
            )]),
        };

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        // Foundry agents dir should be under resources/
        assert!(project_dir.join("resources/foundry/agents").is_dir());
        // Should NOT be at project root
        assert!(!project_dir.join("foundry").exists());
    }

    #[test]
    fn test_files_path_readme_created() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let mut config = make_config("test-service");
        config.project.files_path = Some("resources".to_string());

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let readme = project_dir.join("resources/README.md");
        assert!(readme.exists());

        let content = std::fs::read_to_string(&readme).unwrap();
        assert!(content.contains("Hoist Resource Configuration"));
        assert!(content.contains("hoist pull"));
        assert!(content.contains("github.com/mklab-se/hoist"));
    }

    #[test]
    fn test_files_path_readme_not_created_when_no_files_path() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Only the project-root README should exist, not a separate files-path README
        assert!(project_dir.join("README.md").exists());
    }

    #[test]
    fn test_files_path_readme_not_overwritten() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let mut config = make_config("test-service");
        config.project.files_path = Some("resources".to_string());

        // Create the files dir and an existing README
        std::fs::create_dir_all(project_dir.join("resources")).unwrap();
        std::fs::write(
            project_dir.join("resources/README.md"),
            "# My Custom README\n",
        )
        .unwrap();

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let content = std::fs::read_to_string(project_dir.join("resources/README.md")).unwrap();
        assert_eq!(content, "# My Custom README\n");
    }

    #[test]
    fn test_hoist_yaml_contains_repo_url() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let content = std::fs::read_to_string(project_dir.join(Config::FILENAME)).unwrap();
        assert!(content.contains("https://github.com/mklab-se/hoist"));
    }
}
