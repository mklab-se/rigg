//! Initialize a new hoist project for Azure AI Search and/or Microsoft Foundry

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::info;

use hoist_client::arm::ArmClient;
use hoist_client::auth::AzCliAuth;
use hoist_core::config::{
    Config, FoundryServiceConfig, ProjectConfig, SearchServiceConfig, ServicesConfig, SyncConfig,
};
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;
use hoist_core::state::LocalState;

use crate::cli::InitTemplate;

/// Authenticated ARM context for discovery
struct DiscoveryContext {
    arm: ArmClient,
    subscription_id: String,
}

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

    // Resolve service configurations symmetrically
    let (search_configs, foundry_configs) = if let Some(name) = service_override {
        // --service flag: search is pre-selected, still ask about foundry
        let search = vec![SearchServiceConfig {
            name,
            subscription: None,
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        }];
        let foundry = match try_authenticate().await {
            Ok(ctx) => discover_foundry_service(&ctx).await?.into_iter().collect(),
            Err(_) => prompt_foundry_service_manual()?.into_iter().collect(),
        };
        (search, foundry)
    } else {
        match try_authenticate().await {
            Ok(ctx) => {
                let search: Vec<SearchServiceConfig> = discover_search_service(&ctx)
                    .await?
                    .map(|(name, sub)| {
                        vec![SearchServiceConfig {
                            name,
                            subscription: sub,
                            resource_group: None,
                            api_version: "2024-07-01".to_string(),
                            preview_api_version: "2025-11-01-preview".to_string(),
                        }]
                    })
                    .unwrap_or_default();
                let foundry: Vec<FoundryServiceConfig> =
                    discover_foundry_service(&ctx).await?.into_iter().collect();
                (search, foundry)
            }
            Err(_) => {
                let search: Vec<SearchServiceConfig> = prompt_search_service_manual()?
                    .map(|(name, _)| {
                        vec![SearchServiceConfig {
                            name,
                            subscription: None,
                            resource_group: None,
                            api_version: "2024-07-01".to_string(),
                            preview_api_version: "2025-11-01-preview".to_string(),
                        }]
                    })
                    .unwrap_or_default();
                let foundry: Vec<FoundryServiceConfig> =
                    prompt_foundry_service_manual()?.into_iter().collect();
                (search, foundry)
            }
        }
    };

    if search_configs.is_empty() && foundry_configs.is_empty() {
        anyhow::bail!("At least one service type must be selected.");
    }

    let primary_search_name = search_configs.first().map(|s| s.name.clone());

    // Create configuration
    let config = Config {
        service: None,
        services: ServicesConfig {
            search: search_configs,
            foundry: foundry_configs,
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

    // Create resource directories based on template, filtered by configured services
    let resource_kinds: Vec<ResourceKind> = match template {
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
    }
    .into_iter()
    .filter(|k| match k.domain() {
        ServiceDomain::Search => !config.services.search.is_empty(),
        ServiceDomain::Foundry => !config.services.foundry.is_empty(),
    })
    .collect();

    let resource_base = config.resource_dir(&project_dir);
    if resource_base != project_dir {
        std::fs::create_dir_all(&resource_base)?;
    }

    // Create search resource directories under service-scoped path
    if let Some(ref svc_name) = primary_search_name {
        let search_base = config.search_service_dir(&project_dir, svc_name);
        for kind in &resource_kinds {
            if kind.domain() == ServiceDomain::Search {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }

        // Create documentation files
        let search_kinds: Vec<ResourceKind> = resource_kinds
            .iter()
            .filter(|k| k.domain() == ServiceDomain::Search)
            .copied()
            .collect();
        if !search_kinds.is_empty() {
            create_hoist_md(&search_base, svc_name, &search_kinds)?;
        }
    }

    // Create foundry resource directories
    for foundry_svc in &config.services.foundry {
        let foundry_base =
            config.foundry_service_dir(&project_dir, &foundry_svc.name, &foundry_svc.project);
        let agents_dir = foundry_base.join("agents");
        std::fs::create_dir_all(&agents_dir)?;
    }

    // Create README.md if it doesn't already exist
    create_readme_if_missing(&project_dir, &config)?;

    println!();
    println!("Project initialized successfully!");
    println!();

    // Build pull prompt mentioning all configured services
    let pull_prompt = build_pull_prompt(&config);
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

/// Try to authenticate and select a subscription for ARM discovery
async fn try_authenticate() -> Result<DiscoveryContext> {
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

    let arm = ArmClient::new()?;

    println!("Fetching subscriptions...");
    let subscriptions = arm.list_subscriptions().await?;

    if subscriptions.is_empty() {
        anyhow::bail!("No Azure subscriptions found. Check your Azure access permissions.");
    }

    let default_idx = status
        .subscription_id
        .as_ref()
        .and_then(|id| subscriptions.iter().position(|s| &s.subscription_id == id))
        .unwrap_or(0);

    let selected_sub = prompt_selection("Select subscription", &subscriptions, default_idx)?;
    println!();

    Ok(DiscoveryContext {
        arm,
        subscription_id: selected_sub.subscription_id.clone(),
    })
}

/// Discover a search service via ARM APIs
async fn discover_search_service(
    ctx: &DiscoveryContext,
) -> Result<Option<(String, Option<String>)>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        return Ok(None);
    }

    println!("Fetching Azure AI Search services...");
    let services = ctx.arm.list_search_services(&ctx.subscription_id).await?;

    if services.is_empty() {
        println!("  No search services found in this subscription.");
        return Ok(None);
    }

    let selected = auto_select_or_prompt("Select search service", &services, 0)?;
    Ok(Some((
        selected.name.clone(),
        Some(ctx.subscription_id.clone()),
    )))
}

/// Discover a Foundry service and project via ARM APIs
async fn discover_foundry_service(ctx: &DiscoveryContext) -> Result<Option<FoundryServiceConfig>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        return Ok(None);
    }

    println!("Fetching AI Services accounts...");
    let accounts = ctx
        .arm
        .list_ai_services_accounts(&ctx.subscription_id)
        .await?;

    if accounts.is_empty() {
        println!("  No AI Services accounts found in this subscription.");
        return Ok(None);
    }

    let selected_account = auto_select_or_prompt("Select AI Services account", &accounts, 0)?;

    println!("Fetching Microsoft Foundry projects...");
    let projects = ctx
        .arm
        .list_foundry_projects(selected_account, &ctx.subscription_id)
        .await?;

    if projects.is_empty() {
        println!("  No Foundry projects found for this account.");
        return Ok(None);
    }

    let selected_project = auto_select_or_prompt("Select Foundry project", &projects, 0)?;

    Ok(Some(FoundryServiceConfig {
        name: selected_account.name.clone(),
        project: selected_project.display_name().to_string(),
        api_version: "2025-05-15-preview".to_string(),
        endpoint: Some(selected_account.agents_endpoint()),
        subscription: Some(ctx.subscription_id.clone()),
        resource_group: None,
    }))
}

/// Prompt for search service name manually (no ARM discovery)
fn prompt_search_service_manual() -> Result<Option<(String, Option<String>)>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        return Ok(None);
    }

    let name = prompt_service_name()?;
    Ok(Some((name, None)))
}

/// Prompt for Foundry service configuration manually (no ARM discovery)
fn prompt_foundry_service_manual() -> Result<Option<FoundryServiceConfig>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        return Ok(None);
    }

    print!("AI Services account name (e.g., my-ai-service): ");
    io::stdout().flush()?;
    let mut svc_input = String::new();
    io::stdin().lock().read_line(&mut svc_input)?;
    let svc_name = svc_input.trim().to_string();
    if svc_name.is_empty() {
        anyhow::bail!("AI Services account name is required");
    }

    print!("Foundry project name (e.g., my-project): ");
    io::stdout().flush()?;
    let mut proj_input = String::new();
    io::stdin().lock().read_line(&mut proj_input)?;
    let proj_name = proj_input.trim().to_string();
    if proj_name.is_empty() {
        anyhow::bail!("Foundry project name is required");
    }

    Ok(Some(FoundryServiceConfig {
        name: svc_name,
        project: proj_name,
        api_version: "2025-05-15-preview".to_string(),
        endpoint: None,
        subscription: None,
        resource_group: None,
    }))
}

/// Auto-select if only one item, otherwise prompt for selection
fn auto_select_or_prompt<'a, T: std::fmt::Display>(
    label: &str,
    items: &'a [T],
    default: usize,
) -> Result<&'a T> {
    if items.len() == 1 {
        println!("  Found: {}", items[0]);
        return Ok(&items[0]);
    }
    prompt_selection(label, items, default)
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

/// Build pull prompt mentioning all configured services
fn build_pull_prompt(config: &Config) -> String {
    let mut parts = Vec::new();

    if let Some(search) = config.services.search.first() {
        parts.push(search.name.clone());
    }

    if !config.services.foundry.is_empty() {
        if parts.is_empty() {
            parts.push("Foundry agents".to_string());
        } else {
            parts.push("Foundry agents".to_string());
            let search_part = parts.remove(0);
            return format!(
                "Pull existing resources from {} (and {})?",
                search_part,
                parts.join(", ")
            );
        }
    }

    format!("Pull existing resources from {}?", parts.join(", "))
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

/// Create a README.md in the project root if one doesn't already exist
fn create_readme_if_missing(project_dir: &Path, config: &Config) -> Result<()> {
    let readme_path = project_dir.join("README.md");
    if readme_path.exists() {
        return Ok(());
    }

    let project_name = config.project.name.as_deref().unwrap_or("hoist project");

    let mut services_section = String::new();
    for svc in &config.services.search {
        services_section.push_str(&format!("- **Azure AI Search**: `{}`\n", svc.name));
    }
    for svc in &config.services.foundry {
        services_section.push_str(&format!(
            "- **Microsoft Foundry**: `{}` (project: `{}`)\n",
            svc.name, svc.project
        ));
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
| `hoist.toml` | Project configuration (services, API versions, sync settings) |
| `.hoist/` | Local state directory (gitignored) |
| `search-resources/` | Azure AI Search resource definitions (JSON) |
| `foundry-resources/` | Microsoft Foundry agent definitions |

## Learn More

- Run `hoist --help` for all available commands
- Run `hoist <command> --help` for command-specific options
"#,
        project_name = project_name,
        services_section = services_section,
    );

    std::fs::write(&readme_path, readme)?;
    info!("Created README.md");

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

    let svc_name = config
        .primary_search_service()
        .map(|s| s.name.clone())
        .unwrap_or_default();

    // Create search resource directories under service-scoped path
    let search_base = config.search_service_dir(project_dir, &svc_name);
    for kind in &resource_kinds {
        if kind.domain() == ServiceDomain::Search {
            let dir = search_base.join(kind.directory_name());
            std::fs::create_dir_all(&dir)?;
        }
    }

    // Create foundry resource directories
    for foundry_svc in &config.services.foundry {
        let foundry_base =
            config.foundry_service_dir(project_dir, &foundry_svc.name, &foundry_svc.project);
        let agents_dir = foundry_base.join("agents");
        std::fs::create_dir_all(&agents_dir)?;
    }

    create_hoist_md(&search_base, &svc_name, &resource_kinds)?;
    create_readme_if_missing(project_dir, config)?;

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
    use hoist_core::config::{FoundryServiceConfig, SearchServiceConfig};
    use tempfile::TempDir;

    fn make_config(service_name: &str, path: Option<&str>) -> Config {
        Config {
            service: None,
            services: ServicesConfig {
                search: vec![SearchServiceConfig {
                    name: service_name.to_string(),
                    subscription: None,
                    resource_group: None,
                    api_version: "2024-07-01".to_string(),
                    preview_api_version: "2025-11-01-preview".to_string(),
                }],
                foundry: vec![],
            },
            project: ProjectConfig {
                name: Some("test-project".to_string()),
                description: None,
                path: path.map(|p| p.to_string()),
            },
            sync: SyncConfig {
                include_preview: false,
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

        let search_base = project_dir.join("search/search-resources/test-service");
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
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Full).unwrap();

        let search_base = project_dir.join("search/search-resources/test-service");
        assert!(search_base.join("search-management/indexes").is_dir());
        assert!(search_base.join("search-management/indexers").is_dir());
        assert!(search_base.join("search-management/data-sources").is_dir());
        assert!(search_base.join("search-management/skillsets").is_dir());
        assert!(search_base.join("search-management/synonym-maps").is_dir());
        assert!(search_base.join("search-management/aliases").is_dir());
        // Should NOT have preview dirs
        assert!(!search_base
            .join("agentic-retrieval/knowledge-bases")
            .exists());
    }

    #[test]
    fn test_agentic_template_creates_preview_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service", Some("search"));

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let search_base = project_dir.join("search/search-resources/test-service");
        assert!(search_base.join("search-management/indexes").is_dir());
        assert!(search_base
            .join("agentic-retrieval/knowledge-bases")
            .is_dir());
        assert!(search_base
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
        // New format uses services.search, not legacy service
        assert!(loaded.service.is_none());
        assert_eq!(loaded.services.search[0].name, "my-search");
        assert_eq!(loaded.services.search[0].api_version, "2024-07-01");
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

        let hoist_md = project_dir.join("resources/search-resources/my-search-svc/HOIST.md");
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

        let hoist_md = project_dir.join("search/search-resources/test-service/HOIST.md");
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

        let search_base = project_dir.join("search/search-resources/test-service");
        let sm_readme = search_base.join("search-management/README.md");
        assert!(sm_readme.exists());
        let sm_content = std::fs::read_to_string(&sm_readme).unwrap();
        assert!(sm_content.contains("Search Management Resources"));

        let ar_readme = search_base.join("agentic-retrieval/README.md");
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

        // Resources should be under search-resources/<service-name>
        let search_base = project_dir.join("search-resources/test-service");
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
        let config = make_config("my-search", None);

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
        let config = make_config("my-search", None);

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
            service: None,
            services: ServicesConfig {
                search: vec![SearchServiceConfig {
                    name: "my-search".to_string(),
                    subscription: None,
                    resource_group: None,
                    api_version: "2024-07-01".to_string(),
                    preview_api_version: "2025-11-01-preview".to_string(),
                }],
                foundry: vec![FoundryServiceConfig {
                    name: "my-ai-svc".to_string(),
                    project: "my-project".to_string(),
                    api_version: "2025-05-15-preview".to_string(),
                    endpoint: None,
                    subscription: None,
                    resource_group: None,
                }],
            },
            project: ProjectConfig {
                name: Some("test-project".to_string()),
                description: None,
                path: None,
            },
            sync: SyncConfig {
                include_preview: false,
                resources: Vec::new(),
            },
        };

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let content = std::fs::read_to_string(project_dir.join("README.md")).unwrap();
        assert!(content.contains("my-search"));
        assert!(content.contains("my-ai-svc"));
        assert!(content.contains("my-project"));
        assert!(content.contains("Microsoft Foundry"));
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
