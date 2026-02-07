//! CLI argument definitions using clap

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::commands;
use crate::commands::common::SingularFlags;

/// Manage Azure AI Search resources as code
#[derive(Parser)]
#[command(name = "hoist")]
#[command(author, version, about)]
#[command(long_about = "Manage Azure AI Search resources as code.\n\n\
    Pull resource definitions (indexes, indexers, skillsets, etc.) from Azure as JSON files,\n\
    edit them locally, and push changes back. Enables Git-based version control for your\n\
    search service configuration.")]
#[command(propagate_version = true)]
pub struct Cli {
    /// Increase output verbosity (-v for debug, -vv for trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Path to hoist.toml configuration file
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Azure Search service name (overrides config)
    #[arg(long, global = true)]
    pub service: Option<String>,

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

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new hoist project with directory structure and config
    Init {
        /// Directory to initialize (defaults to current directory)
        dir: Option<PathBuf>,

        /// Subdirectory for resource files (e.g., --path search)
        #[arg(long, value_name = "SUBDIR")]
        path: Option<PathBuf>,

        /// Project template determining which resource types to include
        #[arg(long, value_enum, default_value = "agentic")]
        template: InitTemplate,
    },

    /// View and modify project configuration (hoist.toml)
    #[command(subcommand)]
    Config(ConfigCommands),

    /// Manage Azure authentication
    #[command(subcommand)]
    Auth(AuthCommands),

    /// Download resource definitions from Azure to local JSON files
    Pull {
        /// Pull all resource types
        #[arg(long)]
        all: bool,

        /// Include indexes
        #[arg(long, help_heading = "Resource Types")]
        indexes: bool,

        /// Include indexers
        #[arg(long, help_heading = "Resource Types")]
        indexers: bool,

        /// Include data sources
        #[arg(long, help_heading = "Resource Types")]
        datasources: bool,

        /// Include skillsets
        #[arg(long, help_heading = "Resource Types")]
        skillsets: bool,

        /// Include synonym maps
        #[arg(long, help_heading = "Resource Types")]
        synonymmaps: bool,

        /// Include knowledge bases (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgebases: bool,

        /// Include knowledge sources (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgesources: bool,

        /// Pull a single index by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        index: Option<String>,

        /// Pull a single indexer by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        indexer: Option<String>,

        /// Pull a single data source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        datasource: Option<String>,

        /// Pull a single skillset by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        skillset: Option<String>,

        /// Pull a single synonym map by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        synonymmap: Option<String>,

        /// Pull a single knowledge base by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgebase: Option<String>,

        /// Pull a single knowledge source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgesource: Option<String>,

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

        /// Pull from a different server instead of the configured one
        #[arg(long)]
        source: Option<String>,
    },

    /// Upload local JSON files to Azure, creating or updating resources
    Push {
        /// Push all resource types
        #[arg(long)]
        all: bool,

        /// Include indexes
        #[arg(long, help_heading = "Resource Types")]
        indexes: bool,

        /// Include indexers
        #[arg(long, help_heading = "Resource Types")]
        indexers: bool,

        /// Include data sources
        #[arg(long, help_heading = "Resource Types")]
        datasources: bool,

        /// Include skillsets
        #[arg(long, help_heading = "Resource Types")]
        skillsets: bool,

        /// Include synonym maps
        #[arg(long, help_heading = "Resource Types")]
        synonymmaps: bool,

        /// Include knowledge bases (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgebases: bool,

        /// Include knowledge sources (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgesources: bool,

        /// Push a single index by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        index: Option<String>,

        /// Push a single indexer by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        indexer: Option<String>,

        /// Push a single data source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        datasource: Option<String>,

        /// Push a single skillset by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        skillset: Option<String>,

        /// Push a single synonym map by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        synonymmap: Option<String>,

        /// Push a single knowledge base by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgebase: Option<String>,

        /// Push a single knowledge source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgesource: Option<String>,

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

        /// Push to a different server instead of the configured one
        #[arg(long)]
        target: Option<String>,

        /// Copy resources under new names instead of updating in place
        #[arg(long)]
        copy: bool,

        /// Copy with auto-generated names by appending a suffix (implies --copy)
        #[arg(long)]
        suffix: Option<String>,

        /// Copy with names from a JSON mapping file (implies --copy)
        #[arg(long)]
        answers: Option<PathBuf>,
    },

    /// Compare local resource files against the live Azure service
    Diff {
        /// Diff all resource types
        #[arg(long)]
        all: bool,

        /// Include indexes
        #[arg(long, help_heading = "Resource Types")]
        indexes: bool,

        /// Include indexers
        #[arg(long, help_heading = "Resource Types")]
        indexers: bool,

        /// Include data sources
        #[arg(long, help_heading = "Resource Types")]
        datasources: bool,

        /// Include skillsets
        #[arg(long, help_heading = "Resource Types")]
        skillsets: bool,

        /// Include synonym maps
        #[arg(long, help_heading = "Resource Types")]
        synonymmaps: bool,

        /// Include knowledge bases (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgebases: bool,

        /// Include knowledge sources (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgesources: bool,

        /// Diff a single index by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        index: Option<String>,

        /// Diff a single indexer by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        indexer: Option<String>,

        /// Diff a single data source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        datasource: Option<String>,

        /// Diff a single skillset by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        skillset: Option<String>,

        /// Diff a single synonym map by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        synonymmap: Option<String>,

        /// Diff a single knowledge base by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgebase: Option<String>,

        /// Diff a single knowledge source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgesource: Option<String>,

        /// Diff output format
        #[arg(long, value_enum, default_value = "text")]
        format: DiffFormat,

        /// Exit with code 5 if differences are detected (useful in CI)
        #[arg(long)]
        exit_code: bool,
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
        /// Pull all resource types
        #[arg(long)]
        all: bool,

        /// Include indexes
        #[arg(long, help_heading = "Resource Types")]
        indexes: bool,

        /// Include indexers
        #[arg(long, help_heading = "Resource Types")]
        indexers: bool,

        /// Include data sources
        #[arg(long, help_heading = "Resource Types")]
        datasources: bool,

        /// Include skillsets
        #[arg(long, help_heading = "Resource Types")]
        skillsets: bool,

        /// Include synonym maps
        #[arg(long, help_heading = "Resource Types")]
        synonymmaps: bool,

        /// Include knowledge bases (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgebases: bool,

        /// Include knowledge sources (preview API)
        #[arg(long, help_heading = "Resource Types")]
        knowledgesources: bool,

        /// Watch a single index by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        index: Option<String>,

        /// Watch a single indexer by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        indexer: Option<String>,

        /// Watch a single data source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        datasource: Option<String>,

        /// Watch a single skillset by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        skillset: Option<String>,

        /// Watch a single synonym map by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        synonymmap: Option<String>,

        /// Watch a single knowledge base by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgebase: Option<String>,

        /// Watch a single knowledge source by name
        #[arg(long, value_name = "NAME", help_heading = "Resource Types")]
        knowledgesource: Option<String>,

        /// Filter resources by name (substring match)
        #[arg(long, short)]
        filter: Option<String>,

        /// Automatically write changes without confirmation
        #[arg(long)]
        force: bool,

        /// Pull from a different server instead of the configured one
        #[arg(long)]
        source: Option<String>,

        /// Polling interval in seconds
        #[arg(long, default_value = "20")]
        interval: u64,
    },

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
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Display current configuration from hoist.toml
    Show,

    /// Set a configuration value (e.g., hoist config set service.name my-svc)
    Set {
        /// Configuration key (e.g., service.name, sync.include_preview)
        key: String,

        /// Value to set
        value: String,
    },

    /// Interactive configuration setup
    Init,
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
    /// All stable resource types (indexes, indexers, data sources, skillsets, synonym maps)
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
        match self.command {
            Commands::Init {
                dir,
                path,
                template,
            } => commands::init::run(dir, path, template, self.service).await,
            Commands::Config(cmd) => commands::config::run(cmd).await,
            Commands::Auth(cmd) => commands::auth::run(cmd).await,
            Commands::Pull {
                all,
                indexes,
                indexers,
                datasources,
                skillsets,
                synonymmaps,
                knowledgebases,
                knowledgesources,
                index,
                indexer,
                datasource,
                skillset,
                synonymmap,
                knowledgebase,
                knowledgesource,
                recursive,
                filter,
                dry_run,
                force,
                source,
            } => {
                let singular = SingularFlags {
                    index,
                    indexer,
                    datasource,
                    skillset,
                    synonymmap,
                    knowledgebase,
                    knowledgesource,
                };
                commands::pull::run(
                    all,
                    indexes,
                    indexers,
                    datasources,
                    skillsets,
                    synonymmaps,
                    knowledgebases,
                    knowledgesources,
                    &singular,
                    recursive,
                    filter,
                    dry_run,
                    force,
                    source,
                )
                .await
            }
            Commands::Push {
                all,
                indexes,
                indexers,
                datasources,
                skillsets,
                synonymmaps,
                knowledgebases,
                knowledgesources,
                index,
                indexer,
                datasource,
                skillset,
                synonymmap,
                knowledgebase,
                knowledgesource,
                recursive,
                filter,
                dry_run,
                force,
                yes,
                target,
                copy,
                suffix,
                answers,
            } => {
                let singular = SingularFlags {
                    index,
                    indexer,
                    datasource,
                    skillset,
                    synonymmap,
                    knowledgebase,
                    knowledgesource,
                };
                commands::push::run(
                    all,
                    indexes,
                    indexers,
                    datasources,
                    skillsets,
                    synonymmaps,
                    knowledgebases,
                    knowledgesources,
                    &singular,
                    recursive,
                    filter,
                    dry_run,
                    force || yes,
                    target,
                    copy,
                    suffix,
                    answers,
                )
                .await
            }
            Commands::Diff {
                all,
                indexes,
                indexers,
                datasources,
                skillsets,
                synonymmaps,
                knowledgebases,
                knowledgesources,
                index,
                indexer,
                datasource,
                skillset,
                synonymmap,
                knowledgebase,
                knowledgesource,
                format,
                exit_code,
            } => {
                let singular = SingularFlags {
                    index,
                    indexer,
                    datasource,
                    skillset,
                    synonymmap,
                    knowledgebase,
                    knowledgesource,
                };
                commands::diff::run(
                    all,
                    indexes,
                    indexers,
                    datasources,
                    skillsets,
                    synonymmaps,
                    knowledgebases,
                    knowledgesources,
                    &singular,
                    format,
                    exit_code,
                )
                .await
            }
            Commands::Validate {
                strict,
                check_references,
            } => commands::validate::run(strict, check_references).await,
            Commands::PullWatch {
                all,
                indexes,
                indexers,
                datasources,
                skillsets,
                synonymmaps,
                knowledgebases,
                knowledgesources,
                index,
                indexer,
                datasource,
                skillset,
                synonymmap,
                knowledgebase,
                knowledgesource,
                filter,
                force,
                source,
                interval,
            } => {
                let singular = SingularFlags {
                    index,
                    indexer,
                    datasource,
                    skillset,
                    synonymmap,
                    knowledgebase,
                    knowledgesource,
                };
                commands::pull_watch::run(
                    all,
                    indexes,
                    indexers,
                    datasources,
                    skillsets,
                    synonymmaps,
                    knowledgebases,
                    knowledgesources,
                    &singular,
                    filter,
                    force,
                    source,
                    interval,
                )
                .await
            }
            Commands::Status => commands::status::run().await,
            Commands::Completion { shell } => commands::completion::run(shell),
            Commands::Version => {
                println!("hoist {}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
        }
    }
}
