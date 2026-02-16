//! Initialize a new hoist project for Azure AI Search and/or Microsoft Foundry

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use tracing::info;

use hoist_client::arm::{AiServicesAccount, ArmClient, SearchService};
use hoist_client::auth::AzCliAuth;
use hoist_core::config::{
    Config, EnvironmentConfig, FoundryServiceConfig, ProjectConfig, SearchServiceConfig, SyncConfig,
};
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;

use crate::cli::InitTemplate;

/// Authenticated ARM context for discovery
pub(crate) struct DiscoveryContext {
    pub(crate) arm: ArmClient,
    pub(crate) subscription_id: String,
}

pub async fn run(
    dir: Option<PathBuf>,
    template: InitTemplate,
    files_path: Option<String>,
) -> Result<()> {
    let project_dir = dir.unwrap_or_else(|| std::env::current_dir().unwrap());
    let config_path = project_dir.join(Config::FILENAME);

    if config_path.exists() {
        // Additive update: discover and add new services to existing config
        run_additive(&project_dir).await
    } else {
        // Fresh init: full setup from scratch
        run_fresh(project_dir, template, files_path).await
    }
}

/// Fresh initialization of a new hoist project
async fn run_fresh(
    project_dir: PathBuf,
    template: InitTemplate,
    files_path: Option<String>,
) -> Result<()> {
    crate::banner::print_banner();
    println!();
    println!("Initializing hoist project in {}", project_dir.display());
    println!();

    // Resolve service configurations
    let (search_configs, foundry_configs) = match try_authenticate().await {
        Ok(ctx) => {
            let search = discover_search_services_fresh(&ctx).await?;
            let foundry: Vec<FoundryServiceConfig> =
                discover_foundry_service(&ctx).await?.into_iter().collect();
            (search, foundry)
        }
        Err(_) => {
            let search: Vec<SearchServiceConfig> = prompt_search_service_manual()?
                .map(|(name, _)| {
                    vec![SearchServiceConfig {
                        name,
                        label: None,
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
    };

    if search_configs.is_empty() && foundry_configs.is_empty() {
        anyhow::bail!("At least one service type must be selected.");
    }

    // Create configuration
    let env_name = "prod".to_string();
    let config = Config {
        project: ProjectConfig {
            name: Some(
                project_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("hoist-project")
                    .to_string(),
            ),
            description: None,
            files_path: files_path.clone(),
        },
        sync: SyncConfig {
            include_preview: matches!(template, InitTemplate::Agentic | InitTemplate::Full),
            resources: Vec::new(),
        },
        environments: std::collections::BTreeMap::from([(
            env_name.clone(),
            EnvironmentConfig {
                default: true,
                description: None,
                search: search_configs,
                foundry: foundry_configs,
            },
        )]),
    };
    let env = config
        .resolve_env(None)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // Create directory structure
    std::fs::create_dir_all(&project_dir)?;

    // Compute files root (where resource dirs go)
    let files_dir = config.files_root(&project_dir);
    if files_dir != project_dir {
        std::fs::create_dir_all(&files_dir)?;
    }

    // Save configuration (always in project_dir)
    config.save(&project_dir)?;
    info!("Created {}", Config::FILENAME);

    // Create .hoist state directory (per-environment, always in project_dir)
    let state_dir = project_dir.join(".hoist").join(&env_name);
    std::fs::create_dir_all(&state_dir)?;

    // Create .gitignore for .hoist directory
    let hoist_dir = project_dir.join(".hoist");
    std::fs::create_dir_all(&hoist_dir)?;
    let gitignore_path = hoist_dir.join(".gitignore");
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
        ServiceDomain::Search => env.has_search(),
        ServiceDomain::Foundry => env.has_foundry(),
    })
    .collect();

    // Create search resource directories (under files_dir)
    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_dir, search_svc);
        for kind in &resource_kinds {
            if kind.domain() == ServiceDomain::Search {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }
    }

    // Create foundry resource directories (under files_dir)
    for foundry_svc in &env.foundry {
        let foundry_base = env.foundry_service_dir(&files_dir, foundry_svc);
        let agents_dir = foundry_base.join("agents");
        std::fs::create_dir_all(&agents_dir)?;
    }

    // Create README.md if it doesn't already exist (in project_dir)
    let project_name = config.project.name.as_deref().unwrap_or("hoist project");
    create_readme_if_missing(&project_dir, &env, project_name, &resource_kinds)?;

    // Create README.md in files-path directory if separate from project root
    if files_dir != project_dir {
        create_files_path_readme_if_missing(&files_dir)?;
    }

    println!();
    println!("Project initialized successfully!");
    println!();

    // Ask separately for each service type
    let mut pull_kinds: Vec<ResourceKind> = Vec::new();

    if env.has_search() {
        let search_name = env
            .search
            .first()
            .map(|s| s.name.as_str())
            .unwrap_or("search");
        let prompt = format!(
            "Pull existing resources from Azure AI Search service '{}'?",
            search_name
        );
        if crate::commands::confirm::prompt_yes_default(&prompt)? {
            pull_kinds.extend(
                resource_kinds
                    .iter()
                    .filter(|k| k.domain() == ServiceDomain::Search),
            );
        }
    }

    if env.has_foundry() {
        let foundry_name = env
            .foundry
            .first()
            .map(|s| format!("{}/{}", s.name, s.project))
            .unwrap_or_else(|| "foundry".to_string());
        let prompt = format!(
            "Pull existing resources from Microsoft Foundry project '{}'?",
            foundry_name
        );
        if crate::commands::confirm::prompt_yes_default(&prompt)? {
            pull_kinds.extend(
                resource_kinds
                    .iter()
                    .filter(|k| k.domain() == ServiceDomain::Foundry),
            );
        }
    }

    if !pull_kinds.is_empty() {
        println!();
        let selection = crate::commands::common::ResourceSelection {
            selections: pull_kinds.iter().map(|k| (*k, None)).collect(),
        };
        crate::commands::pull::execute_pull(
            &project_dir,
            &files_dir,
            &env,
            &selection,
            None,  // no filter
            false, // not dry_run
            true,  // force (user already confirmed)
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

/// Additive update: discover and add new services to an existing project
async fn run_additive(project_dir: &Path) -> Result<()> {
    let mut config = Config::load(project_dir)?;
    crate::banner::print_banner();
    println!();
    println!("Updating hoist project in {}", project_dir.display());
    println!();

    let ctx = try_authenticate().await?;

    // Find the default environment to update
    let env_name = config
        .default_env_name()
        .ok_or_else(|| anyhow::anyhow!("No default environment set"))?
        .to_string();
    let files_dir = config.files_root(project_dir);
    let env_config = config
        .environments
        .get_mut(&env_name)
        .ok_or_else(|| anyhow::anyhow!("Environment '{}' not found", env_name))?;

    // Discover and add new search services
    if crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        let new_search = discover_new_search_services(&ctx, &env_config.search).await?;
        let resolved = config
            .resolve_env(Some(&env_name))
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for svc in &new_search {
            let search_base = resolved.search_service_dir(&files_dir, svc);
            for kind in ResourceKind::search_kinds() {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }
        let env_config = config.environments.get_mut(&env_name).unwrap();
        env_config.search.extend(new_search);
    }

    // Discover and add new foundry services
    if crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        let env_config = config.environments.get_mut(&env_name).unwrap();
        let accounts = ctx
            .arm
            .list_ai_services_accounts(&ctx.subscription_id)
            .await?;
        refresh_foundry_endpoints(&mut env_config.foundry, &accounts);

        let new_foundry =
            discover_new_foundry_services(&ctx, &env_config.foundry, &accounts).await?;
        let resolved = config
            .resolve_env(Some(&env_name))
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for svc in &new_foundry {
            let foundry_base = resolved.foundry_service_dir(&files_dir, svc);
            std::fs::create_dir_all(foundry_base.join("agents"))?;
        }
        let env_config = config.environments.get_mut(&env_name).unwrap();
        env_config.foundry.extend(new_foundry);
    }

    // Save updated config
    config.save(project_dir)?;
    println!();
    println!("Configuration updated.");

    Ok(())
}

/// Try to authenticate and select a subscription for ARM discovery
pub(crate) async fn try_authenticate() -> Result<DiscoveryContext> {
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
        label: None,
        api_version: "2025-05-15-preview".to_string(),
        endpoint: Some(selected_account.agents_endpoint()),
        subscription: Some(ctx.subscription_id.clone()),
        resource_group: None,
    }))
}

/// Discover search services for fresh init (multi-select)
async fn discover_search_services_fresh(
    ctx: &DiscoveryContext,
) -> Result<Vec<SearchServiceConfig>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        return Ok(vec![]);
    }

    println!("Fetching Azure AI Search services...");
    let services = ctx.arm.list_search_services(&ctx.subscription_id).await?;

    if services.is_empty() {
        println!("  No search services found in this subscription.");
        return Ok(vec![]);
    }

    let selected = prompt_multi_selection("Add services", &services)?;
    Ok(selected
        .into_iter()
        .map(|s| SearchServiceConfig {
            name: s.name.clone(),
            label: None,
            subscription: Some(ctx.subscription_id.clone()),
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        })
        .collect())
}

/// Discover search services not yet configured (additive mode)
pub(crate) async fn discover_new_search_services(
    ctx: &DiscoveryContext,
    existing: &[SearchServiceConfig],
) -> Result<Vec<SearchServiceConfig>> {
    println!("Fetching Azure AI Search services...");
    let all_services = ctx.arm.list_search_services(&ctx.subscription_id).await?;

    // Show already configured
    for svc in existing {
        println!("  [x] {} (already configured)", svc.name);
    }

    // Filter to not-yet-configured
    let existing_names: Vec<&str> = existing.iter().map(|s| s.name.as_str()).collect();
    let new_services: Vec<&SearchService> = all_services
        .iter()
        .filter(|s| !existing_names.contains(&s.name.as_str()))
        .collect();

    if new_services.is_empty() {
        if existing.is_empty() {
            println!("  No search services found.");
        } else {
            println!("  No additional search services found.");
        }
        return Ok(vec![]);
    }

    let selected = prompt_multi_selection("Add services", &new_services)?;
    Ok(selected
        .into_iter()
        .map(|s| SearchServiceConfig {
            name: s.name.clone(),
            label: None,
            subscription: Some(ctx.subscription_id.clone()),
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        })
        .collect())
}

/// Discover foundry services/projects not yet configured (additive mode)
pub(crate) async fn discover_new_foundry_services(
    ctx: &DiscoveryContext,
    existing: &[FoundryServiceConfig],
    accounts: &[AiServicesAccount],
) -> Result<Vec<FoundryServiceConfig>> {
    // Show already configured
    for svc in existing {
        println!("  [x] {} / {} (already configured)", svc.name, svc.project);
    }

    if accounts.is_empty() {
        println!("  No AI Services accounts found.");
        return Ok(vec![]);
    }

    let selected_accounts = prompt_multi_selection("Add accounts", accounts)?;

    let mut new_configs = Vec::new();
    for account in selected_accounts {
        println!("Fetching projects for {}...", account.name);
        let projects = ctx
            .arm
            .list_foundry_projects(account, &ctx.subscription_id)
            .await?;

        // Filter out already-configured project/account pairs
        let new_projects: Vec<_> = projects
            .iter()
            .filter(|p| {
                !existing
                    .iter()
                    .any(|e| e.name == account.name && e.project == p.display_name())
            })
            .collect();

        if new_projects.is_empty() {
            println!("  No new projects found.");
            continue;
        }

        let selected_projects = prompt_multi_selection("Add projects", &new_projects)?;
        for project in selected_projects {
            new_configs.push(FoundryServiceConfig {
                name: account.name.clone(),
                project: project.display_name().to_string(),
                label: None,
                api_version: "2025-05-15-preview".to_string(),
                endpoint: Some(account.agents_endpoint()),
                subscription: Some(ctx.subscription_id.clone()),
                resource_group: None,
            });
        }
    }
    Ok(new_configs)
}

/// Refresh endpoint URLs for existing Foundry configs using ARM data
pub(crate) fn refresh_foundry_endpoints(
    existing: &mut [FoundryServiceConfig],
    accounts: &[AiServicesAccount],
) {
    for config in existing.iter_mut() {
        if let Some(account) = accounts.iter().find(|a| a.name == config.name) {
            config.endpoint = Some(account.agents_endpoint());
        }
    }
}

/// Prompt user to select one or more items from a list.
/// Auto-selects if there is exactly one item.
fn prompt_multi_selection<'a, T: std::fmt::Display>(
    prompt: &str,
    items: &'a [T],
) -> Result<Vec<&'a T>> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    if items.len() == 1 {
        println!("  Found: {}", items[0]);
        return Ok(vec![&items[0]]);
    }
    for (i, item) in items.iter().enumerate() {
        println!("  [{}] {}", i + 1, item);
    }
    print!("{} (comma-separated, Enter to skip): ", prompt);
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();
    if input.is_empty() {
        return Ok(vec![]);
    }
    let mut selected = Vec::new();
    for part in input.split(',') {
        let idx: usize = part
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid selection: {}", part.trim()))?;
        if idx < 1 || idx > items.len() {
            anyhow::bail!("Selection out of range: {}", idx);
        }
        selected.push(&items[idx - 1]);
    }
    Ok(selected)
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
        label: None,
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

/// Create a README.md in the project root if one doesn't already exist.
/// Includes project overview, CLI quick start, directory layout, JSON conventions,
/// and resource type reference for all configured service domains.
fn create_readme_if_missing(
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
fn create_files_path_readme_if_missing(files_dir: &Path) -> Result<()> {
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
fn create_project_dirs(project_dir: &Path, config: &Config, template: InitTemplate) -> Result<()> {
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
    use hoist_core::config::{EnvironmentConfig, FoundryServiceConfig, SearchServiceConfig};
    use tempfile::TempDir;

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
        assert!(!search_base
            .join("agentic-retrieval/knowledge-bases")
            .exists());
    }

    #[test]
    fn test_agentic_template_creates_preview_dirs() {
        let tmp = TempDir::new().unwrap();
        let project_dir = tmp.path();
        let config = make_config("test-service");

        create_project_dirs(project_dir, &config, InitTemplate::Agentic).unwrap();

        let search_base = project_dir.join("search");
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
        // No HOIST.md or category READMEs — all content is in root README.md
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
    fn test_prompt_multi_selection_empty_items() {
        let items: Vec<String> = vec![];
        let result = prompt_multi_selection("Select", &items).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_multi_select_single_item_auto_selects() {
        let items = vec!["only-one".to_string()];
        let result = prompt_multi_selection("Select", &items).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(*result[0], "only-one");
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
        assert!(project_dir
            .join("search/search-management/indexes")
            .is_dir());
    }

    #[test]
    fn test_refresh_foundry_endpoints() {
        use hoist_client::arm::{AiServicesAccount, AiServicesAccountProperties};

        let mut configs = vec![FoundryServiceConfig {
            name: "my-ai-svc".to_string(),
            project: "proj-1".to_string(),
            label: None,
            api_version: "2025-05-15-preview".to_string(),
            endpoint: None,
            subscription: None,
            resource_group: None,
        }];

        let accounts = vec![AiServicesAccount {
            name: "my-ai-svc".to_string(),
            location: "eastus".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties {
                endpoint: Some("https://custom-sub.cognitiveservices.azure.com/".to_string()),
            },
        }];

        refresh_foundry_endpoints(&mut configs, &accounts);

        assert_eq!(
            configs[0].endpoint.as_deref(),
            Some("https://custom-sub.services.ai.azure.com")
        );
    }

    #[test]
    fn test_refresh_foundry_endpoints_no_match() {
        use hoist_client::arm::{AiServicesAccount, AiServicesAccountProperties};

        let mut configs = vec![FoundryServiceConfig {
            name: "my-ai-svc".to_string(),
            project: "proj-1".to_string(),
            label: None,
            api_version: "2025-05-15-preview".to_string(),
            endpoint: Some("https://old-endpoint.services.ai.azure.com".to_string()),
            subscription: None,
            resource_group: None,
        }];

        let accounts = vec![AiServicesAccount {
            name: "different-svc".to_string(),
            location: "eastus".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties::default(),
        }];

        refresh_foundry_endpoints(&mut configs, &accounts);

        // Should not change — no matching account
        assert_eq!(
            configs[0].endpoint.as_deref(),
            Some("https://old-endpoint.services.ai.azure.com")
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
        assert!(project_dir
            .join("hoist/search/search-management/indexes")
            .is_dir());
        assert!(project_dir
            .join("hoist/search/search-management/data-sources")
            .is_dir());
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
        assert!(project_dir
            .join("search/search-management/indexes")
            .is_dir());
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
