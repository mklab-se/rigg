//! CLI definition for rigg — project-scoped command surface (0.18+).

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::commands;
use crate::commands::ExitCode;

#[derive(Parser)]
#[command(
    name = "rigg",
    about = "Configuration-as-code for Azure AI Search and Microsoft Foundry",
    long_about = "Configuration-as-code for Azure AI Search and Microsoft Foundry.\n\n\
    A rigg workspace holds one or more projects; each project owns its resource\n\
    definitions (indexes, indexers, skillsets, knowledge bases, Foundry agents,\n\
    deployments, ...) as JSON files. Pull, push, and diff operate on projects.\n\n\
    New here? Run `rigg concepts` for the workspace/project model.",
    version,
    propagate_version = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Environment to target (default: the environment marked `default: true`)
    #[arg(long, short = 'e', global = true, env = "RIGG_ENV")]
    pub env: Option<String>,

    /// Output format for machine consumption
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Text)]
    pub output: OutputFormat,

    /// Assume yes on all confirmation prompts
    #[arg(long, short = 'y', global = true)]
    pub yes: bool,

    /// Never prompt; fail instead (implied when stdout is not a terminal)
    #[arg(long, global = true)]
    pub non_interactive: bool,

    /// Increase logging verbosity (-v, -vv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Disable colored output
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Disable AI assistance for this invocation (even when `rigg ai` is enabled)
    #[arg(long, global = true)]
    pub no_ai: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new rigg workspace (discovers services via Azure CLI)
    Init(InitArgs),

    /// Scaffold a new project, resource, pipeline, or API spec
    ///
    /// See `rigg concepts` for what a project is and when to use several.
    New(NewArgs),

    /// Copy a resource file locally under a new name
    Copy(CopyArgs),

    /// Download resource definitions from Azure into project files
    ///
    /// Use `rigg adopt` to claim unmanaged remote resources into a project.
    /// See `rigg concepts` for the project model.
    Pull(PullArgs),

    /// Adopt selected unmanaged Azure resources into a project
    ///
    /// Selectors: `all`, a kind (e.g. `indexes`), or `<kind>/<name>`
    /// (e.g. `agents/regulus`). Naming a resource the project already manages
    /// together with --with-deps adopts its missing dependencies — useful
    /// after new references appear (e.g. added via the portal).
    /// See `rigg concepts` for the project model.
    Adopt(AdoptArgs),

    /// Upload local project files to Azure (create/update, in dependency order)
    Push(PushArgs),

    /// Compare local project files against live Azure services
    Diff(DiffArgs),

    /// Delete a project's resources from Azure
    Delete(DeleteArgs),

    /// Copy one environment's project tree into another, locally
    ///
    /// A→B and B→A are the same operation — the A/B sync + hot-swap
    /// workflow. Correlates resources by their file stem (logical id), not
    /// their physical (Azure) name. Pinned fields keep the target env's
    /// existing values instead of being overwritten: the resource's `name`
    /// (always), the kind's registry-default env-pinned fields (secrets,
    /// write-only fields, and a few genuinely per-environment fields like an
    /// Agent's `tools[].server_url`), and any extra paths named in the
    /// target file's own `x-rigg-pin` annotation. New-in-target resources
    /// are created verbatim from the source (stem preserved); resources that
    /// only exist in the target are left untouched. Always previews before
    /// writing; `--dry-run` stops there. Local only — never touches Azure;
    /// run `rigg diff`/`rigg push` against the target env afterward to sync
    /// it.
    Promote(PromoteArgs),

    /// Show sync status per project (incl. unmanaged remote resources)
    Status(StatusArgs),

    /// Describe the workspace: projects, resources, dependency graph
    Describe(DescribeArgs),

    /// Explain rigg's core model: workspace, projects, and how to choose boundaries
    Concepts,

    /// Validate local files: structure, references, ownership, secrets
    Validate(ValidateArgs),

    /// Manage deployment environments
    Env {
        #[command(subcommand)]
        command: EnvCommands,
    },

    /// Manage Azure authentication
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },

    /// Manage AI features (powered by ailloy)
    Ai {
        #[command(subcommand)]
        command: Option<AiCommands>,
    },

    /// MCP server for AI agents
    Mcp(McpArgs),

    /// CI/CD helpers
    Ci {
        #[command(subcommand)]
        command: CiCommands,
    },

    /// Developer utilities
    #[command(hide = true)]
    Dev {
        #[command(subcommand)]
        command: DevCommands,
    },

    /// Generate shell completions
    Completion {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Show version information
    Version,

    /// Print the rigg banner
    #[command(hide = true)]
    Logo,
}

#[derive(Args)]
pub struct InitArgs {
    /// Directory to initialize (default: current directory)
    #[arg(default_value = ".")]
    pub path: String,

    /// Azure AI Search service name (skips discovery)
    #[arg(long)]
    pub search_service: Option<String>,

    /// Microsoft Foundry account name (skips discovery)
    #[arg(long)]
    pub foundry_account: Option<String>,

    /// Microsoft Foundry project name (with --foundry-account)
    #[arg(long)]
    pub foundry_project: Option<String>,

    /// Name for the initial environment
    #[arg(long, default_value = "dev")]
    pub env_name: String,

    /// Skip ARM discovery even when logged in
    #[arg(long)]
    pub no_discovery: bool,
}

#[derive(Args)]
pub struct NewArgs {
    /// What to scaffold: project | pipeline | api | a resource kind
    /// (data-source, index, skillset, indexer, synonym-map, alias,
    /// knowledge-source, knowledge-base, agent, deployment, connection, guardrail)
    pub kind: String,

    /// Name of the new project/resource/spec. Tip: name a project after the
    /// thing it owns (e.g. the agent's name). No `/` or `\`, max 260 chars.
    pub name: String,

    /// Project to place the resource in (required for resources and pipelines
    /// unless the workspace has exactly one project)
    #[arg(long, short = 'p')]
    pub project: Option<String>,

    /// Data source type for data-source/pipeline scaffolds
    #[arg(long = "type", value_name = "TYPE")]
    pub ds_type: Option<String>,

    /// Describe what you want in natural language; AI drafts the definition (requires ailloy)
    #[arg(long)]
    pub describe: Option<String>,
}

#[derive(Args)]
pub struct CopyArgs {
    /// Source: [project:]<kind-dir>/<name>  (e.g. indexes/my-index)
    pub source: String,
    /// Target: [project:]<name>
    pub target: String,
}

#[derive(Args)]
pub struct PullArgs {
    /// Project to pull (omit with --all)
    pub project: Option<String>,

    /// Pull all projects
    #[arg(long)]
    pub all: bool,

    /// Poll for remote changes and keep pulling
    #[arg(long)]
    pub watch: bool,

    /// Poll interval in seconds for --watch
    #[arg(long, default_value_t = 20)]
    pub interval: u64,
}

#[derive(Args)]
pub struct AdoptArgs {
    /// Project to adopt the resources into (omit on a TTY for an interactive wizard)
    pub project: Option<String>,

    /// What to adopt: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`). Repeatable.
    #[arg(value_name = "SELECTOR")]
    pub selectors: Vec<String>,

    /// Preview what would be adopted; write nothing
    #[arg(long)]
    pub dry_run: bool,

    /// Also adopt each selected resource's upstream dependencies
    #[arg(long)]
    pub with_deps: bool,
}

#[derive(Args)]
pub struct PushArgs {
    /// Project to push (omit with --all)
    pub project: Option<String>,

    /// Push all projects
    #[arg(long)]
    pub all: bool,

    /// Show what would change without pushing
    #[arg(long)]
    pub dry_run: bool,

    /// Delete remote resources whose local files were removed
    #[arg(long)]
    pub prune: bool,

    /// Typed confirmation for protected environments (must equal the env name)
    #[arg(long, value_name = "ENV")]
    pub confirm_env: Option<String>,
}

#[derive(Args)]
pub struct DiffArgs {
    /// Project to diff (omit with --all)
    pub project: Option<String>,

    /// Diff all projects
    #[arg(long)]
    pub all: bool,

    /// Exit with code 5 when differences are found (for CI)
    #[arg(long)]
    pub exit_code: bool,

    /// Output format
    #[arg(long, value_enum, default_value_t = DiffFormat::Text)]
    pub format: DiffFormat,

    /// Compare against another environment instead of local files
    #[arg(long, value_name = "ENV")]
    pub compare_env: Option<String>,

    /// Restrict to one resource: <kind-dir>/<name> (e.g. indexes/my-index)
    #[arg(long, value_name = "KIND/NAME")]
    pub only: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DiffFormat {
    Text,
    Json,
    Markdown,
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Project whose resources should be deleted
    pub project: String,

    /// Delete the project's resources from Azure (required)
    #[arg(long)]
    pub remote: bool,

    /// Typed confirmation for protected environments (must equal the env name)
    #[arg(long, value_name = "ENV")]
    pub confirm_env: Option<String>,
}

#[derive(Args)]
pub struct PromoteArgs {
    /// Project to promote
    pub project: String,

    /// Source environment
    #[arg(long)]
    pub from: String,

    /// Target environment
    #[arg(long)]
    pub to: String,

    /// Preview only; write nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct StatusArgs {
    /// Project to check (default: all)
    pub project: Option<String>,
}

#[derive(Args)]
pub struct DescribeArgs {
    /// Project to describe (default: all)
    pub project: Option<String>,
}

#[derive(Args)]
pub struct ValidateArgs {
    /// Project to validate (default: all)
    pub project: Option<String>,

    /// Enable stricter checks (cross-service reference resolution)
    #[arg(long)]
    pub strict: bool,
}

#[derive(Subcommand)]
pub enum EnvCommands {
    /// List configured environments
    List,
    /// Show environment details
    Show { name: Option<String> },
    /// Set the default environment
    SetDefault { name: String },
    /// Add a new environment
    Add {
        name: String,
        /// Azure AI Search service name
        #[arg(long)]
        search_service: Option<String>,
        /// Foundry account name
        #[arg(long)]
        foundry_account: Option<String>,
        /// Foundry project name
        #[arg(long)]
        foundry_project: Option<String>,
    },
    /// Remove an environment
    Remove { name: String },
}

#[derive(Subcommand)]
pub enum AuthCommands {
    /// Log in to Azure (delegates to Azure CLI)
    Login {
        /// Use service principal from environment variables
        #[arg(long)]
        service_principal: bool,
        /// Use managed identity
        #[arg(long)]
        identity: bool,
    },
    /// Show authentication status
    Status,
    /// Log out
    Logout,
    /// Verify service-to-service identities and RBAC for the workspace
    Doctor {
        /// Attempt to fix missing role assignments / identities
        #[arg(long)]
        fix: bool,
    },
}

#[derive(Subcommand)]
pub enum AiCommands {
    /// Test AI connectivity with a message
    Test { message: Option<String> },
    /// Enable AI features for rigg
    Enable,
    /// Disable AI features for rigg
    Disable,
    /// Configure the AI provider (interactive)
    Config,
    /// Show AI status
    Status,
    /// Emit the rigg agent skill
    Skill {
        /// Write skill markdown to stdout
        #[arg(long)]
        emit: bool,
        /// Print the AI reference document
        #[arg(long)]
        reference: bool,
    },
}

#[derive(Args)]
pub struct McpArgs {
    #[command(subcommand)]
    pub command: McpCommands,
}

#[derive(Subcommand)]
pub enum McpCommands {
    /// Run the MCP server on stdio
    Serve,
    /// Register rigg's MCP server with an AI tool
    Install {
        /// Tool to install for
        #[arg(value_enum, default_value_t = McpTarget::ClaudeCode)]
        target: McpTarget,
        /// Installation scope
        #[arg(long, value_enum, default_value_t = McpScope::Workspace)]
        scope: McpScope,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpTarget {
    ClaudeCode,
    VsCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpScope {
    Workspace,
    Global,
}

#[derive(Subcommand)]
pub enum CiCommands {
    /// Scaffold CI workflows (validate on PR, deploy on merge, nightly drift)
    Init {
        /// CI provider
        #[arg(default_value = "github")]
        provider: String,
        /// Overwrite existing workflow files
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
pub enum DevCommands {
    /// Check whether newer Azure API versions are available
    ApiCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

impl Cli {
    /// Execute the parsed command. Returns the process exit code.
    pub async fn run(self) -> ExitCode {
        let ctx = commands::GlobalContext::from_cli(&self);
        let result = match self.command {
            Commands::Init(args) => commands::init::run(&ctx, args).await,
            Commands::New(args) => commands::new::run(&ctx, args).await,
            Commands::Copy(args) => commands::copy::run(&ctx, args),
            Commands::Pull(args) => commands::pull::run(&ctx, args).await,
            Commands::Adopt(args) => commands::adopt::run(&ctx, args).await,
            Commands::Push(args) => commands::push::run(&ctx, args).await,
            Commands::Diff(args) => commands::diff::run(&ctx, args).await,
            Commands::Delete(args) => commands::delete::run(&ctx, args).await,
            Commands::Promote(args) => commands::promote::run(&ctx, args),
            Commands::Status(args) => commands::status::run(&ctx, args).await,
            Commands::Describe(args) => commands::describe::run(&ctx, args),
            Commands::Concepts => commands::concepts::run(&ctx),
            Commands::Validate(args) => commands::validate::run(&ctx, args),
            Commands::Env { command } => commands::env::run(&ctx, command),
            Commands::Auth { command } => commands::auth::run(&ctx, command).await,
            Commands::Ai { command } => commands::ai::run(command).await,
            Commands::Mcp(args) => commands::mcp_cmd::run(&ctx, args).await,
            Commands::Ci { command } => commands::ci::run(&ctx, command),
            Commands::Dev { command } => commands::dev::run(&ctx, command).await,
            Commands::Completion { shell } => commands::completion::run(shell),
            Commands::Version => {
                crate::banner::print_banner_with_version();
                Ok(())
            }
            Commands::Logo => {
                crate::banner::print_banner_with_version();
                Ok(())
            }
        };
        commands::exit_code_for(result)
    }
}
