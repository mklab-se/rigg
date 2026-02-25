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

        /// Azure AI Search service name (bypasses interactive discovery)
        #[arg(long)]
        search_service: Option<String>,

        /// Azure subscription ID (used with --search-service)
        #[arg(long)]
        search_subscription: Option<String>,

        /// Foundry AI Services account name (bypasses interactive discovery)
        #[arg(long)]
        foundry_account: Option<String>,

        /// Foundry project name (required with --foundry-account)
        #[arg(long)]
        foundry_project: Option<String>,

        /// Skip confirmation prompts (for CI/CD and scripted usage)
        #[arg(long, short = 'y')]
        yes: bool,
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

        /// Execute immediately without preview or confirmation
        #[arg(long)]
        force: bool,

        /// Suppress AI-generated explanations (enabled by default when ai: is configured)
        #[arg(long)]
        no_explain: bool,
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

        /// Execute immediately without preview or confirmation
        #[arg(long)]
        force: bool,

        /// Execute immediately without preview or confirmation (alias for --force)
        #[arg(long, short, hide = true)]
        yes: bool,

        /// Suppress AI-generated explanations (enabled by default when ai: is configured)
        #[arg(long)]
        no_explain: bool,
    },

    /// Create a new resource file from a template (no network calls)
    #[command(subcommand)]
    New(NewCommands),

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

    /// Delete a resource from Azure or remove local files
    Delete {
        /// Resource type and name to delete
        #[command(flatten)]
        resource: DeleteResource,

        /// Where to delete: "remote" (Azure only) or "local" (local files only)
        #[arg(long)]
        target: DeleteTarget,

        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
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

        /// Suppress AI-generated explanations (enabled by default when ai: is configured)
        #[arg(long)]
        no_explain: bool,

        /// Force AI explanations even when ai: is not configured (useful with --explain)
        #[arg(long, hide = true)]
        explain: bool,
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

    /// Configure AI features (Azure OpenAI integration)
    #[command(subcommand)]
    Ai(AiCommands),

    /// MCP (Model Context Protocol) server for AI agent integration
    #[command(subcommand)]
    Mcp(McpCommands),

    /// 🏗️
    #[command(hide = true)]
    Logo,
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// Start the MCP server (stdio transport)
    Serve,
    /// Register hoist as an MCP server with AI tools
    Install {
        #[arg(value_enum, default_value = "claude-code")]
        target: McpInstallTarget,

        /// Installation scope: workspace (project-level) or global (user-level)
        #[arg(long, value_enum, default_value = "workspace")]
        scope: McpInstallScope,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum McpInstallTarget {
    /// Register with Claude Code
    ClaudeCode,
    /// Register with VS Code
    VsCode,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum McpInstallScope {
    /// Project-level installation (available when this project is open)
    Workspace,
    /// User-level installation (available in all sessions)
    Global,
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

#[derive(Subcommand)]
pub enum AiCommands {
    /// Set up Azure OpenAI for AI-enhanced features
    Init {
        /// AI Services account name (bypasses interactive discovery)
        #[arg(long)]
        account: Option<String>,
        /// Model deployment name (bypasses interactive discovery)
        #[arg(long)]
        deployment: Option<String>,
    },
    /// Check AI configuration status
    Status,
    /// Remove AI configuration
    Remove,
}

#[derive(Subcommand)]
pub enum NewCommands {
    /// Create a new search index definition
    Index {
        /// Resource name
        name: String,

        /// Add vector search configuration (HNSW, cosine, 1536 dimensions)
        #[arg(long)]
        vector: bool,

        /// Add semantic search configuration
        #[arg(long)]
        semantic: bool,
    },

    /// Create a new data source definition
    Datasource {
        /// Resource name
        name: String,

        /// Data source type
        #[arg(long, default_value = "azureblob")]
        r#type: String,

        /// Container name
        #[arg(long, default_value = "documents")]
        container: String,
    },

    /// Create a new indexer definition
    Indexer {
        /// Resource name
        name: String,

        /// Data source to index from
        #[arg(long)]
        datasource: String,

        /// Target index to write to
        #[arg(long)]
        index: String,

        /// Optional skillset for AI enrichment
        #[arg(long)]
        skillset: Option<String>,

        /// Indexing schedule (ISO 8601 duration)
        #[arg(long, default_value = "PT5M")]
        schedule: String,
    },

    /// Create a new skillset definition
    Skillset {
        /// Resource name
        name: String,
    },

    /// Create a new synonym map definition
    SynonymMap {
        /// Resource name
        name: String,
    },

    /// Create a new alias definition
    Alias {
        /// Resource name
        name: String,

        /// Target index name
        #[arg(long)]
        index: String,
    },

    /// Create a new knowledge base definition
    KnowledgeBase {
        /// Resource name
        name: String,
    },

    /// Create a new knowledge source definition
    KnowledgeSource {
        /// Resource name
        name: String,

        /// Target index name
        #[arg(long)]
        index: String,

        /// Optional knowledge base name
        #[arg(long)]
        knowledge_base: Option<String>,
    },

    /// Create a new Foundry agent definition
    Agent {
        /// Agent name
        name: String,

        /// Model to use
        #[arg(long, default_value = "gpt-4o")]
        model: String,
    },

    /// Scaffold a complete Agentic RAG system (agent + knowledge base + knowledge source)
    AgenticRag {
        /// Base name for all resources (e.g., 'my-system' creates my-system agent, my-system-kb, my-system-ks)
        name: String,

        /// Model to use for the agent
        #[arg(long, default_value = "gpt-4o")]
        model: String,

        /// Data source type for the knowledge source
        #[arg(long, default_value = "azureBlob")]
        datasource_type: String,

        /// Container name for the data source
        #[arg(long, default_value = "documents")]
        container: String,
    },
}

/// Where to delete a resource from
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum DeleteTarget {
    /// Delete from the remote Azure service only (local files are kept)
    Remote,
    /// Delete local files only (Azure resource is kept)
    Local,
}

/// Resource type and name for deletion (exactly one must be specified)
#[derive(Args, Clone)]
pub struct DeleteResource {
    /// Delete an index by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub index: Option<String>,

    /// Delete an indexer by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub indexer: Option<String>,

    /// Delete a data source by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub datasource: Option<String>,

    /// Delete a skillset by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub skillset: Option<String>,

    /// Delete a synonym map by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub synonymmap: Option<String>,

    /// Delete an alias by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub alias: Option<String>,

    /// Delete a knowledge base by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub knowledgebase: Option<String>,

    /// Delete a knowledge source by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub knowledgesource: Option<String>,

    /// Delete a Foundry agent by name
    #[arg(long, value_name = "NAME", group = "delete_target")]
    pub agent: Option<String>,
}

impl DeleteResource {
    /// Extract the resource kind and name, if any was specified
    pub fn resolve(&self) -> Option<(hoist_core::resources::ResourceKind, String)> {
        use hoist_core::resources::ResourceKind;
        if let Some(ref n) = self.index {
            return Some((ResourceKind::Index, n.clone()));
        }
        if let Some(ref n) = self.indexer {
            return Some((ResourceKind::Indexer, n.clone()));
        }
        if let Some(ref n) = self.datasource {
            return Some((ResourceKind::DataSource, n.clone()));
        }
        if let Some(ref n) = self.skillset {
            return Some((ResourceKind::Skillset, n.clone()));
        }
        if let Some(ref n) = self.synonymmap {
            return Some((ResourceKind::SynonymMap, n.clone()));
        }
        if let Some(ref n) = self.alias {
            return Some((ResourceKind::Alias, n.clone()));
        }
        if let Some(ref n) = self.knowledgebase {
            return Some((ResourceKind::KnowledgeBase, n.clone()));
        }
        if let Some(ref n) = self.knowledgesource {
            return Some((ResourceKind::KnowledgeSource, n.clone()));
        }
        if let Some(ref n) = self.agent {
            return Some((ResourceKind::Agent, n.clone()));
        }
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use hoist_core::resources::ResourceKind;

    #[test]
    fn test_delete_resource_resolve_index() {
        let r = DeleteResource {
            index: Some("my-index".to_string()),
            indexer: None,
            datasource: None,
            skillset: None,
            synonymmap: None,
            alias: None,
            knowledgebase: None,
            knowledgesource: None,
            agent: None,
        };
        let (kind, name) = r.resolve().unwrap();
        assert_eq!(kind, ResourceKind::Index);
        assert_eq!(name, "my-index");
    }

    #[test]
    fn test_delete_resource_resolve_agent() {
        let r = DeleteResource {
            index: None,
            indexer: None,
            datasource: None,
            skillset: None,
            synonymmap: None,
            alias: None,
            knowledgebase: None,
            knowledgesource: None,
            agent: Some("my-agent".to_string()),
        };
        let (kind, name) = r.resolve().unwrap();
        assert_eq!(kind, ResourceKind::Agent);
        assert_eq!(name, "my-agent");
    }

    #[test]
    fn test_delete_resource_resolve_knowledge_source() {
        let r = DeleteResource {
            index: None,
            indexer: None,
            datasource: None,
            skillset: None,
            synonymmap: None,
            alias: None,
            knowledgebase: None,
            knowledgesource: Some("ks-1".to_string()),
            agent: None,
        };
        let (kind, name) = r.resolve().unwrap();
        assert_eq!(kind, ResourceKind::KnowledgeSource);
        assert_eq!(name, "ks-1");
    }

    #[test]
    fn test_delete_resource_resolve_all_kinds() {
        // Test each resource kind maps correctly
        let cases: Vec<(DeleteResource, ResourceKind)> = vec![
            (
                DeleteResource {
                    index: Some("x".into()),
                    indexer: None,
                    datasource: None,
                    skillset: None,
                    synonymmap: None,
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::Index,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: Some("x".into()),
                    datasource: None,
                    skillset: None,
                    synonymmap: None,
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::Indexer,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: Some("x".into()),
                    skillset: None,
                    synonymmap: None,
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::DataSource,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: None,
                    skillset: Some("x".into()),
                    synonymmap: None,
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::Skillset,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: None,
                    skillset: None,
                    synonymmap: Some("x".into()),
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::SynonymMap,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: None,
                    skillset: None,
                    synonymmap: None,
                    alias: Some("x".into()),
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::Alias,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: None,
                    skillset: None,
                    synonymmap: None,
                    alias: None,
                    knowledgebase: Some("x".into()),
                    knowledgesource: None,
                    agent: None,
                },
                ResourceKind::KnowledgeBase,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: None,
                    skillset: None,
                    synonymmap: None,
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: Some("x".into()),
                    agent: None,
                },
                ResourceKind::KnowledgeSource,
            ),
            (
                DeleteResource {
                    index: None,
                    indexer: None,
                    datasource: None,
                    skillset: None,
                    synonymmap: None,
                    alias: None,
                    knowledgebase: None,
                    knowledgesource: None,
                    agent: Some("x".into()),
                },
                ResourceKind::Agent,
            ),
        ];

        for (resource, expected_kind) in cases {
            let (kind, name) = resource.resolve().unwrap();
            assert_eq!(kind, expected_kind, "Expected {:?}", expected_kind);
            assert_eq!(name, "x");
        }
    }

    #[test]
    fn test_delete_resource_resolve_none() {
        let r = DeleteResource {
            index: None,
            indexer: None,
            datasource: None,
            skillset: None,
            synonymmap: None,
            alias: None,
            knowledgebase: None,
            knowledgesource: None,
            agent: None,
        };
        assert!(r.resolve().is_none());
    }
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        let env_override = self.env.as_deref();

        match self.command {
            Commands::Init {
                dir,
                template,
                files_path,
                search_service,
                search_subscription,
                foundry_account,
                foundry_project,
                yes,
            } => {
                commands::init::run(
                    dir,
                    template,
                    files_path,
                    search_service,
                    search_subscription,
                    foundry_account,
                    foundry_project,
                    yes,
                )
                .await
            }
            Commands::Config(cmd) => commands::config::run(cmd).await,
            Commands::Env(cmd) => commands::env::run(cmd).await,
            Commands::Auth(cmd) => commands::auth::run(cmd).await,
            Commands::Pull {
                resources,
                recursive,
                filter,
                force,
                no_explain,
            } => {
                commands::pull::run(
                    &resources,
                    recursive,
                    filter,
                    force,
                    no_explain,
                    env_override,
                )
                .await
            }
            Commands::Push {
                resources,
                recursive,
                filter,
                force,
                yes,
                no_explain,
            } => {
                commands::push::run(
                    &resources,
                    recursive,
                    filter,
                    force || yes,
                    no_explain,
                    env_override,
                )
                .await
            }
            Commands::Delete {
                resource,
                target,
                force,
            } => {
                let (kind, name) = resource.resolve().ok_or_else(|| {
                    anyhow::anyhow!("Specify a resource to delete (e.g., --index <name>)")
                })?;
                commands::delete::run(kind, &name, target, force, env_override).await
            }
            Commands::New(cmd) => commands::scaffold::run(cmd, env_override),
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
            Commands::Ai(cmd) => commands::ai::run(cmd).await,
            Commands::Diff {
                resources,
                format,
                exit_code,
                compare_env,
                no_explain,
                explain,
            } => {
                commands::diff::run(
                    &resources,
                    format,
                    exit_code,
                    env_override,
                    compare_env.as_deref(),
                    no_explain,
                    explain,
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
            Commands::Mcp(cmd) => crate::mcp::run(cmd).await,
            Commands::Logo => {
                crate::banner::print_banner_with_version();
                Ok(())
            }
        }
    }
}
