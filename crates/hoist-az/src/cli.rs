//! CLI argument definitions using clap

use anyhow::Result;
use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::commands;
use crate::commands::common::SingularFlags;

/// Configuration-as-code for Azure AI Search and Microsoft Foundry
#[derive(Parser)]
#[command(name = "hoist")]
#[command(author, version, about)]
#[command(
    long_about = "Configuration-as-code for Azure AI Search and Microsoft Foundry.\n\n\
    Pull resource definitions (indexes, indexers, skillsets, agents, etc.) from Azure as JSON files,\n\
    edit them locally, and push changes back. Enables Git-based version control for your\n\
    search and AI service configuration."
)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Increase output verbosity (-v for debug, -vv for trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Path to hoist.yaml configuration file
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Deployment environment (overrides default from config)
    #[arg(long, short = 'e', global = true, env = "HOIST_ENV")]
    pub env: Option<String>,

    /// Azure subscription ID (overrides config)
    #[arg(long, global = true)]
    pub subscription: Option<String>,

    /// API version to use
    #[arg(long, global = true)]
    pub api_version: Option<String>,

    /// Output format
    #[arg(long, global = true, value_enum, default_value = "text")]
    pub output: OutputFormat,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Commands,
}

/// Shared resource type selection flags used by Pull, Push, Diff, and PullWatch
#[derive(Args, Clone, Default)]
pub struct ResourceTypeFlags {
    /// Include all resource types
    #[arg(long)]
    pub all: bool,

    // Search resource types (plural)
    /// Include indexes
    #[arg(long, help_heading = "Search Resources")]
    pub indexes: bool,

    /// Include indexers
    #[arg(long, help_heading = "Search Resources")]
    pub indexers: bool,

    /// Include data sources
    #[arg(long, help_heading = "Search Resources")]
    pub datasources: bool,

    /// Include skillsets
    #[arg(long, help_heading = "Search Resources")]
    pub skillsets: bool,

    /// Include synonym maps
    #[arg(long, help_heading = "Search Resources")]
    pub synonymmaps: bool,

    /// Include aliases
    #[arg(long, help_heading = "Search Resources")]
    pub aliases: bool,

    /// Include knowledge bases (preview API)
    #[arg(long, help_heading = "Search Resources")]
    pub knowledgebases: bool,

    /// Include knowledge sources (preview API)
    #[arg(long, help_heading = "Search Resources")]
    pub knowledgesources: bool,

    // Foundry resource types (plural)
    /// Include Foundry agents
    #[arg(long, help_heading = "Foundry Resources")]
    pub agents: bool,

    // Search resource types (singular — by name)
    /// Operate on a single index by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub index: Option<String>,

    /// Operate on a single indexer by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub indexer: Option<String>,

    /// Operate on a single data source by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub datasource: Option<String>,

    /// Operate on a single skillset by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub skillset: Option<String>,

    /// Operate on a single synonym map by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub synonymmap: Option<String>,

    /// Operate on a single alias by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub alias: Option<String>,

    /// Operate on a single knowledge base by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub knowledgebase: Option<String>,

    /// Operate on a single knowledge source by name
    #[arg(long, value_name = "NAME", help_heading = "Search Resources")]
    pub knowledgesource: Option<String>,

    // Foundry resource types (singular — by name)
    /// Operate on a single Foundry agent by name
    #[arg(long, value_name = "NAME", help_heading = "Foundry Resources")]
    pub agent: Option<String>,

    // Service scope
    /// Only operate on Azure AI Search resources
    #[arg(long, help_heading = "Service Scope")]
    pub search_only: bool,

    /// Only operate on Microsoft Foundry resources
    #[arg(long, help_heading = "Service Scope")]
    pub foundry_only: bool,
}

impl ResourceTypeFlags {
    /// Extract singular flags for resource selection
    pub fn singular_flags(&self) -> SingularFlags {
        SingularFlags {
            index: self.index.clone(),
            indexer: self.indexer.clone(),
            datasource: self.datasource.clone(),
            skillset: self.skillset.clone(),
            synonymmap: self.synonymmap.clone(),
            alias: self.alias.clone(),
            knowledgebase: self.knowledgebase.clone(),
            knowledgesource: self.knowledgesource.clone(),
            agent: self.agent.clone(),
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new hoist project for Azure AI Search and/or Foundry
    Init {
        /// Directory to initialize (defaults to current directory)
        dir: Option<PathBuf>,

        /// Project template determining which resource types to include
        #[arg(long, value_enum, default_value = "agentic")]
        template: InitTemplate,

        /// Subdirectory for resource files (search/, foundry/ dirs).
        /// Config (hoist.yaml) and state (.hoist/) remain at the init directory.
        #[arg(long)]
        files_path: Option<String>,
    },

    /// View and modify project configuration (hoist.yaml)
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Manage deployment environments
    #[command(subcommand)]
    Env(EnvCommands),

    /// Manage Azure authentication
    #[command(subcommand)]
    Auth(AuthCommands),

    /// Download resource definitions from Azure to local JSON files
    Pull {
        #[command(flatten)]
        resources: ResourceTypeFlags,

        /// Include dependent and child resources (use with singular flags)
        #[arg(long)]
        recursive: bool,

        /// Filter resources by name (substring match)
        #[arg(long, short)]
        filter: Option<String>,

        /// Preview changes without writing files
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },

    /// Upload local JSON files to Azure, creating or updating resources
    Push {
        #[command(flatten)]
        resources: ResourceTypeFlags,

        /// Include dependent and child resources (use with singular flags)
        #[arg(long)]
        recursive: bool,

        /// Filter resources by name (substring match)
        #[arg(long, short)]
        filter: Option<String>,

        /// Preview changes without modifying Azure
        #[arg(long)]
        dry_run: bool,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,

        /// Skip confirmation prompt (alias for --force)
        #[arg(long, short, hide = true)]
        yes: bool,
    },

    /// Copy a resource locally under a new name (no network calls)
    Copy {
        /// Source resource name
        source: String,

        /// Target resource name
        target: String,

        /// Copy a knowledge source and all its managed sub-resources
        #[arg(long, group = "resource_type")]
        knowledgesource: bool,

        /// Copy a knowledge base
        #[arg(long, group = "resource_type")]
        knowledgebase: bool,

        /// Copy a standalone index
        #[arg(long, group = "resource_type")]
        index: bool,

        /// Copy a standalone indexer
        #[arg(long, group = "resource_type")]
        indexer: bool,

        /// Copy a standalone data source
        #[arg(long, group = "resource_type")]
        datasource: bool,

        /// Copy a standalone skillset
        #[arg(long, group = "resource_type")]
        skillset: bool,

        /// Copy a standalone synonym map
        #[arg(long, group = "resource_type")]
        synonymmap: bool,

        /// Copy a standalone alias
        #[arg(long, group = "resource_type")]
        alias: bool,
    },

    /// Compare local resource files against the live Azure service
    Diff {
        #[command(flatten)]
        resources: ResourceTypeFlags,

        /// Diff output format
        #[arg(long, value_enum, default_value = "text")]
        format: DiffFormat,

        /// Exit with code 5 if differences are detected (useful in CI)
        #[arg(long)]
        exit_code: bool,

        /// Compare against a second environment (instead of local files)
        #[arg(long)]
        compare_env: Option<String>,
    },

    /// Validate local JSON files for structural and referential integrity
    Validate {
        /// Enable strict validation (warn on unknown fields)
        #[arg(long)]
        strict: bool,

        /// Verify that cross-resource references are valid
        #[arg(long)]
        check_references: bool,
    },

    /// Poll the server for changes and pull updates automatically
    PullWatch {
        #[command(flatten)]
        resources: ResourceTypeFlags,

        /// Filter resources by name (substring match)
        #[arg(long, short)]
        filter: Option<String>,

        /// Automatically write changes without confirmation
        #[arg(long)]
        force: bool,

        /// Polling interval in seconds
        #[arg(long, default_value = "20")]
        interval: u64,
    },

    /// Show a unified summary of all local resource definitions
    Describe,

    /// Show sync status and local resource summary
    Status,

    /// Generate shell completions for bash, zsh, fish, or PowerShell
    Completion {
        /// Target shell
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Show version information
    Version,

    /// 🏗️
    #[command(hide = true)]
    Logo,
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Display current configuration from hoist.yaml
    Show,

    /// Set a configuration value
    Set {
        /// Configuration key (e.g., project.name, sync.include_preview)
        key: String,

        /// Value to set
        value: String,
    },

    /// Interactive configuration setup
    Init,
}

#[derive(Subcommand)]
pub enum EnvCommands {
    /// List all configured environments
    List,

    /// Show details for an environment
    Show {
        /// Environment name (uses default if omitted)
        name: Option<String>,
    },

    /// Set the default environment
    SetDefault {
        /// Environment name to set as default
        name: String,
    },

    /// Add a new environment (interactive ARM discovery)
    Add {
        /// Name for the new environment
        name: String,
    },

    /// Remove an environment
    Remove {
        /// Environment name to remove
        name: String,
    },
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Authenticate with Azure (Azure CLI, service principal, or managed identity)
    Login {
        /// Use service principal authentication (requires AZURE_CLIENT_ID, AZURE_CLIENT_SECRET, AZURE_TENANT_ID)
        #[arg(long)]
        service_principal: bool,

        /// Use managed identity authentication
        #[arg(long)]
        identity: bool,
    },

    /// Check current authentication status and identity
    Status,

    /// Clear cached authentication
    Logout,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum InitTemplate {
    /// Indexes and data sources only
    Minimal,
    /// All stable resource types (indexes, indexers, data sources, skillsets, synonym maps, aliases)
    Full,
    /// All resource types including preview (knowledge bases, knowledge sources)
    Agentic,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum DiffFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        let env_override = self.env.as_deref();

        match self.command {
            Commands::Init {
                dir,
                template,
                files_path,
            } => commands::init::run(dir, template, files_path).await,
            Commands::Config(cmd) => commands::config::run(cmd).await,
            Commands::Env(cmd) => commands::env::run(cmd).await,
            Commands::Auth(cmd) => commands::auth::run(cmd).await,
            Commands::Pull {
                resources,
                recursive,
                filter,
                dry_run,
                force,
            } => {
                commands::pull::run(&resources, recursive, filter, dry_run, force, env_override)
                    .await
            }
            Commands::Push {
                resources,
                recursive,
                filter,
                dry_run,
                force,
                yes,
            } => {
                commands::push::run(
                    &resources,
                    recursive,
                    filter,
                    dry_run,
                    force || yes,
                    env_override,
                )
                .await
            }
            Commands::Copy {
                source,
                target,
                knowledgesource,
                knowledgebase,
                index,
                indexer,
                datasource,
                skillset,
                synonymmap,
                alias,
            } => commands::copy::run(
                &source,
                &target,
                knowledgesource,
                knowledgebase,
                index,
                indexer,
                datasource,
                skillset,
                synonymmap,
                alias,
                env_override,
            ),
            Commands::Diff {
                resources,
                format,
                exit_code,
                compare_env,
            } => {
                commands::diff::run(
                    &resources,
                    format,
                    exit_code,
                    env_override,
                    compare_env.as_deref(),
                )
                .await
            }
            Commands::Validate {
                strict,
                check_references,
            } => commands::validate::run(strict, check_references, self.output, env_override).await,
            Commands::PullWatch {
                resources,
                filter,
                force,
                interval,
            } => commands::pull_watch::run(&resources, filter, force, interval, env_override).await,
            Commands::Describe => commands::describe_project::run(self.output, env_override).await,
            Commands::Status => commands::status::run(self.output, env_override).await,
            Commands::Completion { shell } => commands::completion::run(shell),
            Commands::Version => {
                crate::banner::print_banner_with_version();
                Ok(())
            }
            Commands::Logo => {
                crate::banner::print_banner_with_version();
                Ok(())
            }
        }
    }
}
