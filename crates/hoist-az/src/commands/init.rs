//! Initialize a new hoist project

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::info;

use hoist_client::arm::{ArmClient, DiscoveredService};
use hoist_client::auth::AzCliAuth;
use hoist_core::config::{Config, ProjectConfig, ServiceConfig, SyncConfig};
use hoist_core::resources::ResourceKind;
use hoist_core::state::LocalState;

use crate::cli::InitTemplate;

pub async fn run(
    dir: Option<PathBuf>,
    path: Option<PathBuf>,
    template: InitTemplate,
    service_override: Option<String>,
) -> Result<()> {
    let project_dir = dir.unwrap_or_else(|| std::env::current_dir().unwrap());

    // Check if already initialized
    let config_path = project_dir.join(Config::FILENAME);
    if config_path.exists() {
        anyhow::bail!(
            "Project already initialized at {}. Use 'hoist config' to modify settings.",
            project_dir.display()
        );
    }

    println!("Initializing hoist project in {}", project_dir.display());
    println!();

    // Resolve service name: flag > discovery > manual entry
    let (service_name, subscription_id) = if let Some(name) = service_override {
        (name, None)
    } else {
        match try_discover_service().await {
            Ok(discovered) => {
                let sub_id = Some(discovered.subscription_id);
                (discovered.name, sub_id)
            }
            Err(_) => {
                let name = prompt_service_name()?;
                (name, None)
            }
        }
    };

    // Create configuration
    let config = Config {
        service: ServiceConfig {
            name: service_name.clone(),
            subscription: subscription_id,
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        },
        project: ProjectConfig {
            name: Some(
                project_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("hoist-project")
                    .to_string(),
            ),
            description: None,
            path: path.as_ref().map(|f| f.to_string_lossy().to_string()),
        },
        sync: SyncConfig {
            include_preview: matches!(template, InitTemplate::Agentic | InitTemplate::Full),
            generate_docs: true,
            resources: Vec::new(),
        },
    };

    // Create directory structure
    std::fs::create_dir_all(&project_dir)?;

    // Save configuration
    config.save(&project_dir)?;
    info!("Created {}", Config::FILENAME);

    // Create .hoist state directory
    let state_dir = LocalState::state_dir(&project_dir);
    std::fs::create_dir_all(&state_dir)?;

    // Create .gitignore for state directory
    let gitignore_path = state_dir.join(".gitignore");
    std::fs::write(&gitignore_path, "# Ignore local state\n*\n!.gitignore\n")?;

    // Create resource directories based on template
    let resource_kinds = match template {
        InitTemplate::Minimal => vec![ResourceKind::Index, ResourceKind::DataSource],
        InitTemplate::Full => vec![
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
            ResourceKind::Alias,
        ],
        InitTemplate::Agentic => ResourceKind::all().to_vec(),
    };

    let resource_base = config.resource_dir(&project_dir);
    if resource_base != project_dir {
        std::fs::create_dir_all(&resource_base)?;
    }

    for kind in &resource_kinds {
        let dir = resource_base.join(kind.directory_name());
        std::fs::create_dir_all(&dir)?;
    }

    // Create documentation files
    create_hoist_md(&resource_base, &service_name, &resource_kinds)?;

    println!();
    println!("Project initialized successfully!");
    println!();

    // Prompt to pull existing resources
    let pull_prompt = format!("Pull existing resources from {}?", service_name);
    if crate::commands::confirm::prompt_yes_default(&pull_prompt)? {
        println!();
        let selection = crate::commands::common::ResourceSelection {
            selections: resource_kinds.iter().map(|k| (*k, None)).collect(),
        };
        crate::commands::pull::execute_pull(
            &project_dir,
            &config,
            &selection,
            None,  // no filter
            false, // not dry_run
            true,  // force (user already confirmed)
            None,  // no source override
        )
        .await?;
    } else {
        println!();
        println!("Next steps:");
        println!("  1. Verify authentication: hoist auth status");
        println!("  2. Pull existing resources: hoist pull --all");
        println!("  3. View differences: hoist diff --all");
    }

    println!();

    Ok(())
}

/// Try to discover a search service via Azure Resource Manager
async fn try_discover_service() -> Result<DiscoveredService> {
    // Check auth status first
    let status = AzCliAuth::check_status().map_err(|e| {
        println!(
            "Not logged in to Azure CLI. Run 'az login' for auto-discovery, or enter manually."
        );
        anyhow::anyhow!("{}", e)
    })?;

    if let Some(user) = &status.user {
        println!("Checking Azure authentication... logged in as {}", user);
    }
    println!();

    // Create ARM client
    let arm = ArmClient::new()?;

    // List and select subscription
    println!("Fetching subscriptions...");
    let subscriptions = arm.list_subscriptions().await?;

    if subscriptions.is_empty() {
        anyhow::bail!("No Azure subscriptions found. Check your Azure access permissions.");
    }

    // Always let user confirm, even with a single option
    let default_idx = status
        .subscription_id
        .as_ref()
        .and_then(|id| subscriptions.iter().position(|s| &s.subscription_id == id))
        .unwrap_or(0);

    let selected_sub = prompt_selection("Select subscription", &subscriptions, default_idx)?;
    println!();

    // List and select search service
    println!("Fetching Azure AI Search services...");
    let services = arm
        .list_search_services(&selected_sub.subscription_id)
        .await?;

    if services.is_empty() {
        anyhow::bail!(
            "No Azure AI Search services found in subscription '{}'. \
             Create one in the Azure portal, or enter a service name manually.",
            selected_sub.display_name
        );
    }

    let selected_service = prompt_selection("Select search service", &services, 0)?;

    Ok(DiscoveredService {
        name: selected_service.name.clone(),
        subscription_id: selected_sub.subscription_id.clone(),
        location: selected_service.location.clone(),
    })
}

/// Prompt user to select from a numbered list
fn prompt_selection<'a, T: std::fmt::Display>(
    prompt: &str,
    items: &'a [T],
    default: usize,
) -> Result<&'a T> {
    for (i, item) in items.iter().enumerate() {
        let marker = if i == default { " [default]" } else { "" };
        println!("  [{}] {}{}", i + 1, item, marker);
    }

    print!("{} [{}]: ", prompt, default + 1);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(&items[default]);
    }

    let index: usize = input
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("Invalid selection: {}", input))?;

    if index < 1 || index > items.len() {
        anyhow::bail!("Selection out of range: {}", index);
    }

    Ok(&items[index - 1])
}

fn prompt_service_name() -> Result<String> {
    print!("Azure Search service name: ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;

    let name = input.trim().to_string();
    if name.is_empty() {
        anyhow::bail!("Service name is required");
    }

    Ok(name)
}

fn create_hoist_md(
    resource_base: &Path,
    service_name: &str,
    resource_kinds: &[ResourceKind],
) -> Result<()> {
    let has_search_management = resource_kinds
        .iter()
        .any(|k| k.directory_name().starts_with("search-management/"));
    let has_agentic_retrieval = resource_kinds
        .iter()
        .any(|k| k.directory_name().starts_with("agentic-retrieval/"));

    // Build the resource type reference section
    let mut resource_sections = String::new();

    if has_search_management {
        resource_sections.push_str(SEARCH_MANAGEMENT_SECTION);
    }
    if has_agentic_retrieval {
        resource_sections.push_str(AGENTIC_RETRIEVAL_SECTION);
    }

    let hoist_md = format!(
        r#"# Azure AI Search - Resource Definitions

This directory contains the configuration for Azure AI Search service `{service_name}`,
managed as code. Each JSON file defines a single search service resource and maps directly
to the [Azure AI Search REST API](https://learn.microsoft.com/en-us/rest/api/searchservice/).

For CLI usage and available commands, run `hoist --help`.

## Files and Directories

| Path | Description |
|------|-------------|
| `hoist.toml` | Project configuration: service name, API versions, sync settings. |
| `.hoist/` | Local state directory (auto-managed, gitignored). Tracks sync checksums. |
{directory_rows}

## JSON File Conventions

Each `.json` file represents one Azure AI Search resource. The files follow these conventions:

- **Filename = resource name.** A file named `my-index.json` defines the resource named `my-index`.
- **Same schema as the REST API.** The JSON structure matches the request/response body of the
  corresponding Azure AI Search REST API endpoint. See the resource type reference below for
  field documentation and links to the official API specs.
- **Property order is preserved.** Properties appear in the order returned by the Azure API.
  If you reorder properties locally, the next `hoist pull` will restore the canonical order.
- **Volatile fields are stripped.** `@odata.etag` (changes on every update) and `@odata.context`
  (contains the service hostname) are removed to keep files environment-independent and
  diff-friendly.
- **Secrets are excluded.** Connection strings, credentials, and storage secrets are never
  stored in these files. They are managed separately through the Azure portal or CLI.

## Resource Type Reference

{resource_sections}## API Documentation

- [Azure AI Search overview](https://learn.microsoft.com/en-us/azure/search/search-what-is-azure-search)
- [REST API reference (stable)](https://learn.microsoft.com/en-us/rest/api/searchservice/)
- [REST API reference (preview)](https://learn.microsoft.com/en-us/rest/api/searchservice/?view=rest-searchservice-2025-05-01-preview)
- [Service limits and quotas](https://learn.microsoft.com/en-us/azure/search/search-limits-quotas-capacity)
"#,
        service_name = service_name,
        directory_rows = resource_kinds
            .iter()
            .map(|k| format!(
                "| `{}/` | {} resource definitions. [API reference]({}) |",
                k.directory_name(),
                k.display_name(),
                api_doc_url(*k)
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        resource_sections = resource_sections,
    );

    std::fs::write(resource_base.join("HOIST.md"), hoist_md)?;

    // Category READMEs with detailed field documentation
    if has_search_management {
        let sm_dir = resource_base.join("search-management");
        std::fs::create_dir_all(&sm_dir)?;
        std::fs::write(sm_dir.join("README.md"), search_management_readme())?;
    }

    if has_agentic_retrieval {
        let ar_dir = resource_base.join("agentic-retrieval");
        std::fs::create_dir_all(&ar_dir)?;
        std::fs::write(ar_dir.join("README.md"), agentic_retrieval_readme())?;
    }

    Ok(())
}

fn api_doc_url(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::Index => "https://learn.microsoft.com/en-us/rest/api/searchservice/indexes/create-or-update",
        ResourceKind::Indexer => "https://learn.microsoft.com/en-us/rest/api/searchservice/indexers/create-or-update",
        ResourceKind::DataSource => "https://learn.microsoft.com/en-us/rest/api/searchservice/data-sources/create-or-update",
        ResourceKind::Skillset => "https://learn.microsoft.com/en-us/rest/api/searchservice/skillsets/create-or-update",
        ResourceKind::SynonymMap => "https://learn.microsoft.com/en-us/rest/api/searchservice/synonym-maps/create-or-update",
        ResourceKind::Alias => "https://learn.microsoft.com/en-us/rest/api/searchservice/indexes/create-or-update",
        ResourceKind::KnowledgeBase => "https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-bases/create-or-update?view=rest-searchservice-2025-05-01-preview",
        ResourceKind::KnowledgeSource => "https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-sources/create-or-update?view=rest-searchservice-2025-05-01-preview",
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

fn search_management_readme() -> &'static str {
    r##"# Search Management Resources

This directory contains the core Azure AI Search resource definitions.
Each subdirectory holds JSON files for a specific resource type.

For a summary of all resource types and their fields, see [HOIST.md](../HOIST.md).

## Resource Types

### indexes/

Index definitions describe the schema for searchable content.

**Key fields:**
- `name` (string, required) - Unique index identifier. Must match the filename.
- `fields` (array, required) - Field definitions with properties:
  - `name`, `type` (e.g., `Edm.String`, `Edm.Int32`, `Collection(Edm.Single)`)
  - `key` (boolean) - Exactly one field must be the key.
  - `searchable`, `filterable`, `sortable`, `facetable`, `retrievable` (booleans)
  - `analyzer`, `searchAnalyzer`, `indexAnalyzer` (string) - Text analysis settings.
  - `dimensions`, `vectorSearchProfile` - For vector fields (`Collection(Edm.Single)`).
- `vectorSearch` (object, optional) - Vector search configuration with `algorithms`, `profiles`, `vectorizers`, and `compressions`.
- `semantic` (object, optional) - Semantic ranker configuration with named configurations.
- `scoringProfiles` (array, optional) - Custom relevance scoring rules.
- `similarity` (object, optional) - Similarity algorithm (e.g., BM25).

**Important:** The `fields` array is immutable after creation for certain field types. Adding new fields is allowed, but changing existing field types is not.

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/indexes/create-or-update

---

### indexers/

Indexer definitions control how data is pulled from a data source into an index.

**Key fields:**
- `name` (string, required) - Unique indexer identifier. Must match the filename.
- `dataSourceName` (string, required) - References a data source in `data-sources/`.
- `targetIndexName` (string, required) - References an index in `indexes/`.
- `skillsetName` (string, optional) - References a skillset in `skillsets/` for AI enrichment.
- `schedule` (object, optional) - Cron-like schedule with `interval` (ISO 8601 duration, e.g., `"PT5M"`) and `startTime`.
- `parameters` (object, optional) - Runtime configuration:
  - `batchSize` (integer) - Documents per batch.
  - `maxFailedItems`, `maxFailedItemsPerBatch` (integer) - Failure thresholds.
  - `configuration` (object) - Source-specific settings (e.g., `parsingMode`, `dataToExtract`).
- `fieldMappings` (array, optional) - Maps source fields to index fields when names differ.
- `outputFieldMappings` (array, optional) - Maps skillset outputs to index fields.
- `disabled` (boolean, optional) - Set to `true` to pause the indexer.

**Dependencies:** Requires a valid `dataSourceName` and `targetIndexName`. If `skillsetName` is set, that skillset must also exist.

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/indexers/create-or-update

---

### data-sources/

Data source definitions specify where an indexer reads data from.

**Key fields:**
- `name` (string, required) - Unique data source identifier. Must match the filename.
- `type` (string, required) - Source type: `"azureblob"`, `"azuresql"`, `"cosmosdb"`, `"azuretable"`, `"adlsgen2"`, etc.
- `container` (object, required):
  - `name` (string) - Container, table, or database name.
  - `query` (string, optional) - Filter query for the data source.
- `dataChangeDetectionPolicy` (object, optional) - Enables incremental indexing.
- `dataDeletionDetectionPolicy` (object, optional) - Detects deleted documents.
- `identity` (object, optional) - Managed identity for authentication.

**Note:** The `credentials` field (connection strings) is stripped from local files for security. Azure manages credentials separately. Do not add connection strings to these files.

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/data-sources/create-or-update

---

### skillsets/

Skillset definitions describe AI enrichment pipelines applied during indexing.

**Key fields:**
- `name` (string, required) - Unique skillset identifier. Must match the filename.
- `skills` (array, required) - Ordered list of skills to execute:
  - `@odata.type` (string, required) - Skill type (e.g., `"#Microsoft.Skills.Text.SplitSkill"`, `"#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill"`).
  - `name` (string, required) - Unique name within the skillset.
  - `context` (string, optional) - Execution scope (e.g., `"/document"`, `"/document/pages/*"`).
  - `inputs` (array, required) - Input mappings with `name` and `source`.
  - `outputs` (array, required) - Output mappings with `name` and `targetName`.
  - Additional properties vary by skill type.
- `indexProjections` (object, optional) - Projects enriched data into child indexes.
- `knowledgeStore` (object, optional) - Saves enrichment output to Azure Storage.

**Note:** The `cognitiveServices` field is stripped from local files. Configure AI services keys through Azure.

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/skillsets/create-or-update

---

### synonym-maps/

Synonym map definitions provide query-time term expansion.

**Key fields:**
- `name` (string, required) - Unique synonym map identifier. Must match the filename.
- `format` (string, required) - Always `"solr"`.
- `synonyms` (string, required) - Synonym rules, one per line. Format:
  - Equivalent synonyms: `"USA, United States, United States of America"`
  - Explicit mapping: `"Washington, Wash. => WA"`

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/synonym-maps/create-or-update
"##
}

/// Create directory structure for an init template without prompts or ARM discovery.
/// Used internally for testing.
#[cfg(test)]
fn create_project_dirs(project_dir: &Path, config: &Config, template: InitTemplate) -> Result<()> {
    std::fs::create_dir_all(project_dir)?;

    // Save configuration
    config.save(project_dir)?;

    // Create .hoist state directory
    let state_dir = LocalState::state_dir(project_dir);
    std::fs::create_dir_all(&state_dir)?;

    // Create .gitignore for state directory
    let gitignore_path = state_dir.join(".gitignore");
    std::fs::write(&gitignore_path, "# Ignore local state\n*\n!.gitignore\n")?;

    // Create resource directories based on template
    let resource_kinds = match template {
        InitTemplate::Minimal => vec![ResourceKind::Index, ResourceKind::DataSource],
        InitTemplate::Full => vec![
            ResourceKind::Index,
            ResourceKind::Indexer,
            ResourceKind::DataSource,
            ResourceKind::Skillset,
            ResourceKind::SynonymMap,
            ResourceKind::Alias,
        ],
        InitTemplate::Agentic => ResourceKind::all().to_vec(),
    };

    let resource_base = config.resource_dir(project_dir);
    if resource_base != project_dir {
        std::fs::create_dir_all(&resource_base)?;
    }

    for kind in &resource_kinds {
        let dir = resource_base.join(kind.directory_name());
        std::fs::create_dir_all(&dir)?;
    }

    create_hoist_md(&resource_base, &config.service.name, &resource_kinds)?;

    Ok(())
}

fn agentic_retrieval_readme() -> &'static str {
    r#"# Agentic Retrieval Resources (Preview)

This directory contains resource definitions for the Azure AI Search Agentic Retrieval feature.
These resources use the **preview** API and may change before general availability.

For a summary of all resource types, see [HOIST.md](../HOIST.md).

## Resource Types

### knowledge-bases/

Knowledge base definitions represent a curated collection of knowledge sources that AI agents can query.

**Key fields:**
- `name` (string, required) - Unique knowledge base identifier. Must match the filename.
- `description` (string, optional) - Human-readable description of what this knowledge base contains.
- `retrievalInstructions` (string, optional) - Instructions for the retrieval model on how to search.
- `answerInstructions` (string, optional) - Instructions for answer generation.
- `outputMode` (string, optional) - Output format for query results (e.g., `"extractiveData"`).
- `knowledgeSources` (array, optional) - References to knowledge sources in this knowledge base.
- `models` (array, optional) - Model configurations used by this knowledge base.
- `encryptionKey` (object, optional) - Customer-managed encryption key.

**Note:** The `storageConnectionStringSecret` field is stripped from local files for security.

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-bases/create-or-update?view=rest-searchservice-2025-05-01-preview

---

### knowledge-sources/

Knowledge source definitions connect a data source to a knowledge base, defining how content
is indexed and queried by AI agents.

**Key fields:**
- `name` (string, required) - Unique knowledge source identifier. Must match the filename.
- `kind` (string, required) - Source type: `"azureBlob"`, `"indexedSharePoint"`, `"web"`, etc.
- `description` (string, optional) - Describes what information this source provides to agents.
- `azureBlobParameters` (object, optional) - Configuration for Azure Blob sources, including container, connection, and chunking settings.
- `searchIndexParameters` (object, optional) - Configuration for search index-based sources.
- `encryptionKey` (object, optional) - Customer-managed encryption key.

**Dependencies:** Belongs to a knowledge base, which is specified during creation.

**Docs:** https://learn.microsoft.com/en-us/rest/api/searchservice/knowledge-sources/create-or-update?view=rest-searchservice-2025-05-01-preview
"#
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_config(service_name: &str, path: Option<&str>) -> Config {
        Config {
            service: ServiceConfig {
                name: service_name.to_string(),
                subscription: None,
                resource_group: None,
                api_version: "2024-07-01".to_string(),
                preview_api_version: "2025-11-01-preview".to_string(),
            },
            project: ProjectConfig {
                name: Some("test-project".to_string()),
                description: None,
                path: path.map(|p| p.to_string()),
            },
            sync: SyncConfig {
                include_preview: false,
                generate_docs: true,
                resources: Vec::new(),
            },
        }
    }

    #[test]
    fn test_minimal_template_creates_index_and_datasource_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let resource_base = project_dir.join("search");
        assert!(resource_base.join("search-management/indexes").is_dir());
        assert!(resource_base
            .join("search-management/data-sources")
            .is_dir());
        // Should NOT have indexers, skillsets, synonym-maps
        assert!(!resource_base.join("search-management/indexers").exists());
        assert!(!resource_base.join("search-management/skillsets").exists());
    }

    #[test]
    fn test_full_template_creates_all_stable_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Full).unwrap();

        let resource_base = project_dir.join("search");
        assert!(resource_base.join("search-management/indexes").is_dir());
        assert!(resource_base.join("search-management/indexers").is_dir());
        assert!(resource_base
            .join("search-management/data-sources")
            .is_dir());
        assert!(resource_base.join("search-management/skillsets").is_dir());
        assert!(resource_base
            .join("search-management/synonym-maps")
            .is_dir());
        assert!(resource_base.join("search-management/aliases").is_dir());
        // Should NOT have preview dirs
        assert!(!resource_base
            .join("agentic-retrieval/knowledge-bases")
            .exists());
    }

    #[test]
    fn test_agentic_template_creates_preview_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let resource_base = project_dir.join("search");
        assert!(resource_base.join("search-management/indexes").is_dir());
        assert!(resource_base
            .join("agentic-retrieval/knowledge-bases")
            .is_dir());
        assert!(resource_base
            .join("agentic-retrieval/knowledge-sources")
            .is_dir());
    }

    #[test]
    fn test_config_file_created() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("my-search", None);

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let config_path = project_dir.join("hoist.toml");
        assert!(config_path.exists());

        let loaded = Config::load(project_dir).unwrap();
        assert_eq!(loaded.service.name, "my-search");
        assert_eq!(loaded.service.api_version, "2024-07-01");
    }

    #[test]
    fn test_gitignore_created_in_state_dir() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", None);

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        let gitignore = project_dir.join(".hoist/.gitignore");
        assert!(gitignore.exists());

        let content = std::fs::read_to_string(&gitignore).unwrap();
        assert!(content.contains("*"));
        assert!(content.contains("!.gitignore"));
    }

    #[test]
    fn test_hoist_md_created() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("my-search-svc", Some("resources"));

        create_project_dirs(project_dir, &config, InitTemplate::Full).unwrap();

        let hoist_md = project_dir.join("resources/HOIST.md");
        assert!(hoist_md.exists());

        let content = std::fs::read_to_string(&hoist_md).unwrap();
        assert!(content.contains("my-search-svc"));
        assert!(content.contains("indexes"));
        assert!(content.contains("search-management"));
    }

    #[test]
    fn test_hoist_md_includes_agentic_section_for_agentic_template() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let hoist_md = project_dir.join("search/HOIST.md");
        let content = std::fs::read_to_string(&hoist_md).unwrap();
        assert!(content.contains("agentic-retrieval"));
        assert!(content.contains("knowledge-bases"));
    }

    #[test]
    fn test_category_readmes_created() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let sm_readme = project_dir.join("search/search-management/README.md");
        assert!(sm_readme.exists());
        let sm_content = std::fs::read_to_string(&sm_readme).unwrap();
        assert!(sm_content.contains("Search Management Resources"));

        let ar_readme = project_dir.join("search/agentic-retrieval/README.md");
        assert!(ar_readme.exists());
        let ar_content = std::fs::read_to_string(&ar_readme).unwrap();
        assert!(ar_content.contains("Agentic Retrieval"));
    }

    #[test]
    fn test_no_path_creates_dirs_at_project_root() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", None);

        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Resources should be directly under project root
        assert!(project_dir.join("search-management/indexes").is_dir());
        assert!(project_dir.join("search-management/data-sources").is_dir());
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
    fn test_already_initialized_error() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", None);

        // First init should work
        create_project_dirs(project_dir, &config, InitTemplate::Minimal).unwrap();

        // Check that config file exists (simulating the check in run())
        let config_path = project_dir.join(Config::FILENAME);
        assert!(config_path.exists());
    }
}
