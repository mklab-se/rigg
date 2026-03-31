//! Initialize a new rigg project for Azure AI Search and/or Microsoft Foundry

mod additive;
mod discovery;
mod prompts;
mod readme;

use std::path::PathBuf;

use anyhow::Result;
use tracing::info;

use rigg_core::config::{
    Config, EnvironmentConfig, FoundryServiceConfig, ProjectConfig, SearchServiceConfig, SyncConfig,
};
use rigg_core::resources::ResourceKind;
use rigg_core::service::ServiceDomain;

use crate::cli::InitTemplate;

// Re-export pub(crate) items used by other modules (env.rs)
pub(crate) use discovery::{
    discover_new_foundry_services, discover_new_search_services, try_authenticate,
};

/// Options for non-interactive initialization.
pub struct NonInteractiveOptions {
    pub search_service: Option<String>,
    pub search_subscription: Option<String>,
    pub foundry_account: Option<String>,
    pub foundry_project: Option<String>,
    pub yes: bool,
}

impl NonInteractiveOptions {
    /// Returns true if any non-interactive flags were provided.
    pub fn has_flags(&self) -> bool {
        self.search_service.is_some() || self.foundry_account.is_some() || self.yes
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    dir: Option<PathBuf>,
    template: InitTemplate,
    files_path: Option<String>,
    search_service: Option<String>,
    search_subscription: Option<String>,
    foundry_account: Option<String>,
    foundry_project: Option<String>,
    yes: bool,
) -> Result<()> {
    let project_dir = dir.unwrap_or_else(|| std::env::current_dir().unwrap());
    let config_path = project_dir.join(Config::FILENAME);

    let opts = NonInteractiveOptions {
        search_service,
        search_subscription,
        foundry_account,
        foundry_project,
        yes,
    };

    if config_path.exists() {
        // Additive update: discover and add new services to existing config
        additive::run_additive(&project_dir).await
    } else if opts.has_flags() {
        // Non-interactive: build config from CLI flags
        run_non_interactive(project_dir, template, files_path, &opts).await
    } else {
        // Fresh init: full interactive setup from scratch
        run_fresh(project_dir, template, files_path).await
    }
}

/// Fresh initialization of a new rigg project
async fn run_fresh(
    project_dir: PathBuf,
    template: InitTemplate,
    files_path: Option<String>,
) -> Result<()> {
    crate::banner::print_banner();
    println!();
    println!("Initializing rigg project in {}", project_dir.display());
    println!();

    // Resolve service configurations
    let (search_configs, foundry_configs) = match try_authenticate().await {
        Ok(ctx) => {
            let search = discovery::discover_search_services_fresh(&ctx).await?;
            let foundry: Vec<FoundryServiceConfig> = discovery::discover_foundry_service(&ctx)
                .await?
                .into_iter()
                .collect();
            (search, foundry)
        }
        Err(_) => {
            let search: Vec<SearchServiceConfig> = prompts::prompt_search_service_manual()?
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
            let foundry: Vec<FoundryServiceConfig> = prompts::prompt_foundry_service_manual()?
                .into_iter()
                .collect();
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
                    .unwrap_or("rigg-project")
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

    // Create .rigg state directory (per-environment, always in project_dir)
    let state_dir = project_dir.join(".rigg").join(&env_name);
    std::fs::create_dir_all(&state_dir)?;

    // Create .gitignore for .rigg directory
    let rigg_dir = project_dir.join(".rigg");
    std::fs::create_dir_all(&rigg_dir)?;
    let gitignore_path = rigg_dir.join(".gitignore");
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
    let project_name = config.project.name.as_deref().unwrap_or("rigg project");
    readme::create_readme_if_missing(&project_dir, &env, project_name, &resource_kinds)?;

    // Create README.md in files-path directory if separate from project root
    if files_dir != project_dir {
        readme::create_files_path_readme_if_missing(&files_dir)?;
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
            true,  // force (user already confirmed)
            false, // no AI explanations during init
        )
        .await?;
    } else {
        println!();
        println!("Next steps:");
        println!("  1. Verify authentication: rigg auth status");
        println!("  2. Pull existing resources: rigg pull --all");
        match template {
            InitTemplate::Agentic => {
                println!("  3. Or create new resources from scratch:");
                println!("       rigg new knowledge-base my-kb");
                println!("       rigg new knowledge-source my-ks --index my-index");
                println!("       rigg new agent my-agent --model gpt-4o");
            }
            _ => {
                println!(
                    "  3. Or create a new resource: rigg new index my-first-index --vector --semantic"
                );
            }
        }
        println!("  4. View differences: rigg diff --all");
    }

    println!();

    Ok(())
}

/// Non-interactive initialization from CLI flags (for CI/CD and scripted usage)
async fn run_non_interactive(
    project_dir: PathBuf,
    template: InitTemplate,
    files_path: Option<String>,
    opts: &NonInteractiveOptions,
) -> Result<()> {
    println!("Initializing rigg project in {}", project_dir.display());
    println!();

    // Validate flag combinations
    if opts.foundry_account.is_some() != opts.foundry_project.is_some() {
        anyhow::bail!("--foundry-account and --foundry-project must be specified together");
    }

    // Build service configs from flags
    let search_configs: Vec<SearchServiceConfig> = if let Some(ref name) = opts.search_service {
        vec![SearchServiceConfig {
            name: name.clone(),
            label: None,
            subscription: opts.search_subscription.clone(),
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        }]
    } else {
        vec![]
    };

    let foundry_configs: Vec<FoundryServiceConfig> =
        if let (Some(account), Some(project)) = (&opts.foundry_account, &opts.foundry_project) {
            vec![FoundryServiceConfig {
                name: account.clone(),
                project: project.clone(),
                label: None,
                api_version: "2025-05-15-preview".to_string(),
                endpoint: None,
                subscription: None,
                resource_group: None,
            }]
        } else {
            vec![]
        };

    if search_configs.is_empty() && foundry_configs.is_empty() {
        anyhow::bail!(
            "At least one service must be specified. Use --search-service and/or --foundry-account + --foundry-project"
        );
    }

    // Create configuration
    let env_name = "prod".to_string();
    let config = Config {
        project: ProjectConfig {
            name: Some(
                project_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("rigg-project")
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
    let files_dir = config.files_root(&project_dir);
    if files_dir != project_dir {
        std::fs::create_dir_all(&files_dir)?;
    }

    // Save configuration
    config.save(&project_dir)?;

    // Create .rigg state directory
    let state_dir = project_dir.join(".rigg").join(&env_name);
    std::fs::create_dir_all(&state_dir)?;
    let rigg_dir = project_dir.join(".rigg");
    let gitignore_path = rigg_dir.join(".gitignore");
    std::fs::write(&gitignore_path, "# Ignore local state\n*\n!.gitignore\n")?;

    // Create resource directories based on template
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

    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_dir, search_svc);
        for kind in &resource_kinds {
            if kind.domain() == ServiceDomain::Search {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }
    }
    for foundry_svc in &env.foundry {
        let foundry_base = env.foundry_service_dir(&files_dir, foundry_svc);
        std::fs::create_dir_all(foundry_base.join("agents"))?;
    }

    println!("Project initialized successfully!");
    println!();

    // Auto-pull if --yes was specified
    if opts.yes {
        let pull_kinds: Vec<ResourceKind> = resource_kinds.to_vec();
        if !pull_kinds.is_empty() {
            let selection = crate::commands::common::ResourceSelection {
                selections: pull_kinds.iter().map(|k| (*k, None)).collect(),
            };
            crate::commands::pull::execute_pull(
                &project_dir,
                &files_dir,
                &env,
                &selection,
                None,
                true,  // force -- non-interactive
                false, // no AI explanations during init
            )
            .await?;
        }
    } else {
        println!("Next steps:");
        println!("  rigg pull --all    # Pull existing resources from Azure");
        println!("  rigg status        # View project status");
    }

    Ok(())
}
