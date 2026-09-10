//! CLI definition for rigg — project-scoped command surface (0.18+).

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::engine::ArgValueCandidates;

use crate::completion_dynamic as complete;

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
    #[arg(long, short = 'e', global = true, env = "RIGG_ENV",
          add = ArgValueCandidates::new(complete::envs))]
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

    /// Answer a question a guided flow would ask (repeatable): --answer <id>=<value>
    #[arg(long = "answer", global = true, value_name = "ID=VALUE")]
    pub answer: Vec<String>,

    /// JSON file of answers ({"<id>": "<value>", …})
    #[arg(long, global = true, value_name = "PATH")]
    pub answers_file: Option<std::path::PathBuf>,
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

    /// Translate one environment's project tree into another
    ///
    /// A→B and B→A are the same operation — the A/B sync + hot-swap
    /// workflow. Correlates resources by their file stem (logical id), not
    /// their physical (Azure) name, and TRANSLATES rather than copies: every
    /// infrastructure reference (storage, identity, model host, function
    /// app, key vault, api) is re-pointed at the target environment's
    /// binding of the same name, and every reference to a sibling that is
    /// physically named differently in the target follows that name. The
    /// target keeps its own `name`, the paths its own `x-rigg-pin`
    /// annotation lists, and its Web API auth carriers — the source's never
    /// cross. Anything rigg cannot decide (an unbound reference, a binding
    /// the target lacks) becomes a question instead of a guess. Resources
    /// that only exist in the target are left untouched. Always previews the
    /// rewiring before writing; `--dry-run` stops there.
    Promote(PromoteArgs),

    /// Prove a pushed project actually works against the live services
    ///
    /// Runs every indexer to completion, retrieves from every knowledge
    /// base and asks every agent a one-turn question. Failures that look
    /// like an authorization problem are attributed to the identity edge
    /// that would explain them. Exits 1 when anything fails. This is
    /// `rigg push --verify` on its own, for a stack that is already pushed.
    Verify(VerifyArgs),

    /// Operate the LIVE Azure resources: run indexers, query indexes,
    /// prompt knowledge bases and agents
    ///
    /// Unlike the config commands (push/pull/diff), these act on the cloud
    /// resources directly, addressed by physical name — no project
    /// ownership required.
    Az {
        #[command(subcommand)]
        command: AzCommands,
    },

    /// Migrate a resource to an explicit, fully rigg-managed shape
    Migrate {
        #[command(subcommand)]
        command: MigrateCommands,
    },

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
    ///
    /// Static script: `rigg completion zsh > ~/.zfunc/_rigg` (subcommands
    /// and flags). Dynamic completion (also completes project, environment
    /// and resource NAMES from your workspace files) is a one-liner in your
    /// shell rc instead:
    ///
    ///   zsh:  source <(COMPLETE=zsh rigg)
    ///   bash: source <(COMPLETE=bash rigg)
    ///   fish: COMPLETE=fish rigg | source
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
    /// Folder to store rigg's files in (projects/, apis/, .rigg/), recorded
    /// as `root:` in rigg.yaml. The workspace itself is always initialized
    /// in the current directory (default: files alongside rigg.yaml)
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

    /// Azure AD tenant id for the initial environment (default: read from
    /// `az account show` when logged in)
    #[arg(long)]
    pub tenant: Option<String>,

    /// Azure subscription id for the initial environment (default: read
    /// from `az account show` when logged in)
    #[arg(long)]
    pub subscription: Option<String>,

    /// Skip ARM discovery even when logged in
    #[arg(long)]
    pub no_discovery: bool,
}

#[derive(Args)]
pub struct NewArgs {
    /// What to scaffold: project | pipeline | api | a resource kind
    /// (data-source, index, skillset, indexer, synonym-map, alias,
    /// knowledge-source, knowledge-base, agent, deployment, connection, guardrail)
    #[arg(add = ArgValueCandidates::new(complete::new_kinds))]
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

    /// Use this environment's `identity` binding (a user-assigned managed
    /// identity) instead of the search service's system-assigned one
    /// (data-source, skillset)
    #[arg(long, value_name = "BINDING")]
    pub identity: Option<String>,
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
    #[arg(add = ArgValueCandidates::new(complete::projects))]
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
    #[arg(add = ArgValueCandidates::new(complete::projects))]
    pub project: Option<String>,

    /// What to adopt: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`). Repeatable.
    #[arg(value_name = "SELECTOR", add = ArgValueCandidates::new(complete::selectors))]
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
    #[arg(add = ArgValueCandidates::new(complete::projects))]
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

    /// Allow replace operations (delete + recreate, e.g. a knowledge-source
    /// kind change). Required non-interactively when the plan contains one:
    /// --yes alone is not enough, because a replace rebuilds the index
    /// (time, ingestion cost, downtime)
    #[arg(long)]
    pub allow_replace: bool,

    /// Also re-authorize Web API skills on IN-SYNC skillsets: re-resolve
    /// redacted function keys (interactive) and re-PUT annotated skills with
    /// a freshly fetched key. Ordinary pushes leave in-sync resources alone
    #[arg(long)]
    pub refresh_credentials: bool,

    /// After the push, prove the stack works: run every indexer to
    /// completion, retrieve from every knowledge base, and ask every agent
    /// (the same checks as `rigg verify`). Exits 1 on any failure
    #[arg(long)]
    pub verify: bool,

    /// Skip the plan-scoped auth preflight (the identity/RBAC verification
    /// that runs before anything is written). The escape hatch for a caller
    /// who knows the wiring is fine and cannot read ARM
    #[arg(long)]
    pub skip_auth_preflight: bool,
}

#[derive(Args)]
pub struct VerifyArgs {
    /// Project to verify (omit with --all, or when the workspace has one)
    #[arg(add = ArgValueCandidates::new(complete::projects))]
    pub project: Option<String>,

    /// Verify all projects
    #[arg(long)]
    pub all: bool,

    /// Typed confirmation for protected environments (must equal the env name)
    #[arg(long, value_name = "ENV")]
    pub confirm_env: Option<String>,
}

#[derive(Args)]
pub struct DiffArgs {
    /// Project to diff (omit with --all)
    #[arg(add = ArgValueCandidates::new(complete::projects))]
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
    #[arg(long, value_name = "KIND/NAME", add = ArgValueCandidates::new(complete::selectors))]
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
    #[arg(add = ArgValueCandidates::new(complete::projects))]
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
    /// Project to promote (omit when the workspace has exactly one)
    #[arg(add = ArgValueCandidates::new(complete::projects))]
    pub project: Option<String>,

    /// Source environment
    #[arg(long)]
    pub from: String,

    /// Target environment
    #[arg(long)]
    pub to: String,

    /// Preview only; write nothing. Still runs the online checks (pass
    /// `--offline` too for a network-free preview), so it may still ask
    /// questions
    #[arg(long)]
    pub dry_run: bool,

    /// Skip every Azure lookup (candidate lists, Web API auth re-derivation,
    /// deployment availability); unresolved items are reported instead
    #[arg(long)]
    pub offline: bool,
}

#[derive(Subcommand)]
pub enum MigrateCommands {
    /// Convert an indexed blob knowledge source (azureBlob) to the explicit
    /// searchIndex kind, materializing its Azure-generated pipeline
    /// (data source, index, skillset, indexer) as project files
    ///
    /// Local-only: writes/rewrites project files; the next `rigg push`
    /// applies the change. In-place migration keeps every name — push then
    /// REPLACES the knowledge source (delete + recreate), which rebuilds the
    /// index from source data (time, ingestion/embedding cost, and the
    /// source is unavailable until repopulated). Side-by-side (--rename)
    /// creates a parallel pipeline under new names; the old knowledge source
    /// keeps serving until you cut over and delete it.
    #[command(alias = "ks")]
    KnowledgeSource(MigrateKsArgs),
}

#[derive(Args)]
pub struct MigrateKsArgs {
    /// Knowledge source to migrate
    #[arg(add = ArgValueCandidates::new(complete::knowledge_sources))]
    pub name: String,

    /// Project owning the knowledge source (defaults to the only project)
    #[arg(long, short = 'p')]
    pub project: Option<String>,

    /// In-place: keep all names; the next push replaces the knowledge source
    /// and REBUILDS its index
    #[arg(long, conflicts_with = "rename")]
    pub in_place: bool,

    /// Side-by-side: create a parallel pipeline under this new knowledge
    /// source name (old one keeps serving until you cut over)
    #[arg(long, value_name = "NEW-NAME")]
    pub rename: Option<String>,
}

#[derive(Subcommand)]
pub enum AzCommands {
    /// Indexer operations (run, reset, status)
    Indexer {
        #[command(subcommand)]
        command: AzIndexerCommands,
    },
    /// Index operations (query, stats)
    Index {
        #[command(subcommand)]
        command: AzIndexCommands,
    },
    /// Knowledge-base operations (ask)
    #[command(name = "knowledge-base", alias = "kb")]
    KnowledgeBase {
        #[command(subcommand)]
        command: AzKbCommands,
    },
    /// Agent operations (ask)
    Agent {
        #[command(subcommand)]
        command: AzAgentCommands,
    },
}

#[derive(Subcommand)]
pub enum AzIndexerCommands {
    /// Trigger a run now
    Run(AzIndexerRunArgs),
    /// Clear change-tracking state — the NEXT run reprocesses every document
    Reset(AzIndexerResetArgs),
    /// Execution state, last run result, per-document errors
    Status {
        /// Indexer name
        #[arg(add = ArgValueCandidates::new(complete::indexers))]
        name: String,
    },
}

#[derive(Args)]
pub struct AzIndexerRunArgs {
    /// Indexer name
    #[arg(add = ArgValueCandidates::new(complete::indexers))]
    pub name: String,

    /// Poll until the run completes; exit non-zero if it fails
    #[arg(long)]
    pub watch: bool,

    /// Reset change tracking first (confirm-gated: full reprocess, costs
    /// ingestion/embeddings)
    #[arg(long)]
    pub reset: bool,

    /// Typed confirmation for protected environments (must equal the env name)
    #[arg(long, value_name = "ENV")]
    pub confirm_env: Option<String>,
}

#[derive(Args)]
pub struct AzIndexerResetArgs {
    /// Indexer name
    #[arg(add = ArgValueCandidates::new(complete::indexers))]
    pub name: String,

    /// Typed confirmation for protected environments (must equal the env name)
    #[arg(long, value_name = "ENV")]
    pub confirm_env: Option<String>,
}

#[derive(Subcommand)]
pub enum AzIndexCommands {
    /// Run a search query against the live index
    Query(AzIndexQueryArgs),
    /// Document count and storage size
    Stats {
        /// Index name
        #[arg(add = ArgValueCandidates::new(complete::indexes))]
        name: String,
    },
}

#[derive(Args)]
pub struct AzIndexQueryArgs {
    /// Index name
    #[arg(add = ArgValueCandidates::new(complete::indexes))]
    pub name: String,

    /// Search text (* matches all documents)
    pub search: String,

    /// Number of results
    #[arg(long, default_value_t = 5)]
    pub top: u32,

    /// OData filter expression
    #[arg(long)]
    pub filter: Option<String>,

    /// Comma-separated fields to return (default: all retrievable)
    #[arg(long)]
    pub select: Option<String>,
}

#[derive(Subcommand)]
pub enum AzKbCommands {
    /// Retrieve grounding content for a prompt (agentic retrieval)
    Ask {
        /// Knowledge base name
        #[arg(add = ArgValueCandidates::new(complete::knowledge_bases))]
        name: String,
        /// The question / semantic intent
        prompt: String,
    },
}

#[derive(Subcommand)]
pub enum AzAgentCommands {
    /// Send a single prompt to the agent and print its reply
    Ask {
        /// Agent name
        #[arg(add = ArgValueCandidates::new(complete::agents))]
        name: String,
        /// The prompt
        prompt: String,
    },
}

#[derive(Args)]
pub struct StatusArgs {
    /// Project to check (default: all)
    #[arg(add = ArgValueCandidates::new(complete::projects))]
    pub project: Option<String>,

    /// Also verify the identity graph per environment (one line per env;
    /// `rigg auth doctor` for the detail)
    #[arg(long)]
    pub auth: bool,
}

#[derive(Args)]
pub struct DescribeArgs {
    /// Project to describe (default: all)
    #[arg(add = ArgValueCandidates::new(complete::projects))]
    pub project: Option<String>,
}

#[derive(Args)]
pub struct ValidateArgs {
    /// Project to validate (default: all)
    #[arg(add = ArgValueCandidates::new(complete::projects))]
    pub project: Option<String>,

    /// Enable stricter checks (cross-service reference resolution)
    #[arg(long)]
    pub strict: bool,

    /// Also list bound and shared infrastructure references
    #[arg(long)]
    pub show_bindings: bool,
}

#[derive(Subcommand)]
pub enum EnvCommands {
    /// List configured environments
    List,
    /// Show environment details: targets, policy, and dependency bindings
    Show {
        /// Environment to show (default: the selected one)
        #[arg(add = ArgValueCandidates::new(complete::envs))]
        name: Option<String>,
        /// Re-resolve every binding against Azure and refresh the cache
        #[arg(long)]
        refresh: bool,
    },
    /// Set the default environment
    SetDefault { name: String },
    /// Add a new environment
    Add {
        /// Name for the new environment
        name: String,
        /// Azure AD tenant the environment's resources live in
        #[arg(long)]
        tenant: Option<String>,
        /// Azure subscription the environment's resources live in
        #[arg(long)]
        subscription: Option<String>,
        /// Azure AI Search service name
        #[arg(long)]
        search_service: Option<String>,
        /// Foundry account name
        #[arg(long)]
        foundry_account: Option<String>,
        /// Foundry project name
        #[arg(long)]
        foundry_project: Option<String>,
        /// Require typed confirmation for cloud changes in this environment
        #[arg(long)]
        protected: bool,
        /// Declare a dependency binding (repeatable): --bind <name>=<type>:<value>
        #[arg(long = "bind", value_name = "NAME=TYPE:VALUE")]
        bind: Vec<String>,
        /// Model the new environment on an existing one, copying its bindings
        #[arg(long, value_name = "ENV", add = ArgValueCandidates::new(complete::envs))]
        like: Option<String>,
        /// With --like: copy this binding verbatim (repeatable; the default
        /// for every binding not named by --bind or --skip)
        #[arg(long = "same", value_name = "NAME", requires = "like")]
        same: Vec<String>,
        /// With --like: do not copy this binding (repeatable)
        #[arg(long = "skip", value_name = "NAME", requires = "like")]
        skip: Vec<String>,
    },
    /// Remove an environment
    Remove {
        name: String,
        /// Also remove the role assignments rigg created for it (the flag
        /// is the confirmation — they are deleted without a further prompt)
        #[arg(long)]
        clean_roles: bool,
    },
    /// Declare a dependency binding for an environment
    ///
    ///   rigg env bind dev docs storage:contosodocs
    ///   rigg env bind dev --learn        # propose bindings from the files
    #[command(verbatim_doc_comment)]
    Bind {
        /// Environment to bind in
        #[arg(add = ArgValueCandidates::new(complete::envs))]
        env: String,
        /// Binding name (omit with --learn)
        name: Option<String>,
        /// <type>:<value>, e.g. storage:contosodocs or api:https://x/v1
        value: Option<String>,
        /// Propose bindings from the infrastructure references in this
        /// environment's files instead of taking one on the command line
        #[arg(long)]
        learn: bool,
    },
    /// Remove a dependency binding from an environment
    Unbind {
        /// Environment to remove the binding from
        #[arg(add = ArgValueCandidates::new(complete::envs))]
        env: String,
        /// Binding name
        name: String,
    },
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
    /// Verify service-to-service identities, settings and RBAC for an environment
    ///
    /// Reports every role, setting and network condition this environment's
    /// files require — for the service identities and for you — with the
    /// exact `az` command for each gap. `--fix` applies the ones rigg owns.
    Doctor {
        /// Apply the fixes rigg can make (role assignments, identities,
        /// search auth options, storage firewall/soft delete)
        #[arg(long)]
        fix: bool,
        /// Check operator rights for this object id instead of your own
        /// (e.g. the CI service principal)
        #[arg(long, value_name = "OBJECT_ID")]
        principal: Option<String>,
        /// Only the resources a push would create or update
        #[arg(long)]
        plan: bool,
        /// Also read each indexer's last run and attribute auth failures
        #[arg(long)]
        live: bool,
        /// Typed confirmation for protected environments (must equal the env
        /// name) — required by `--fix`
        #[arg(long, value_name = "ENV")]
        confirm_env: Option<String>,
    },
    /// Enable Microsoft Entra authentication on a bound function app
    ///
    /// Registers (or reuses) an Entra application for the app, merges Easy
    /// Auth into its authsettingsV2 so it accepts `api://<app-id>` from this
    /// environment's search identity, and rewrites the Web API skills that
    /// call it to be keyless. Nothing is pushed — run `rigg push` after.
    ///
    ///   rigg auth easy-auth enrich-fn -e dev
    #[command(verbatim_doc_comment)]
    EasyAuth {
        /// Name of the `function-app` dependency binding to wire (from rigg.yaml)
        #[arg(value_name = "BINDING")]
        function_app: String,
        /// Reuse this existing app registration instead of creating one
        #[arg(long, value_name = "CLIENT_ID")]
        client_id: Option<String>,
        /// Typed confirmation for protected environments (must equal the env name)
        #[arg(long, value_name = "ENV")]
        confirm_env: Option<String>,
    },
    /// Manage the role assignments rigg created
    Roles {
        #[command(subcommand)]
        command: RolesCommands,
    },
}

#[derive(Subcommand)]
pub enum RolesCommands {
    /// List the role assignments rigg created for this environment
    List,
    /// Remove the role assignments rigg created for this environment
    Remove {
        /// Typed confirmation for protected environments (must equal the env name)
        #[arg(long, value_name = "ENV")]
        confirm_env: Option<String>,
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
    /// List the tools the MCP server exposes
    Tools {
        /// Print the MCP.md tool table as Markdown
        #[arg(long)]
        markdown: bool,
    },
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
#[allow(clippy::enum_variant_names)] // Api* mirrors the `api-*` subcommand names on purpose.
pub enum DevCommands {
    /// Check whether newer Azure API versions are available
    ApiCheck,
    /// Show what changed between two versions of a provider's OpenAPI definitions
    ApiDiff {
        provider: String,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        to: Option<String>,
    },
    /// Regenerate the pinned schema fixtures under crates/rigg-core/fixtures/schema
    ApiFixture { provider: String },
    /// Print docs/reference/cli.md — the generated command reference
    CliReference,
    /// Print the infrastructure-reference table for docs/reference/resource-files.md
    InfraTable,
    /// Check the docs: every `rigg` command line parses, every link resolves
    DocsCheck {
        /// Workspace root to check (default: the current directory)
        #[arg(long, value_name = "DIR")]
        root: Option<std::path::PathBuf>,
    },
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
        let output = self.output;
        let ctx = match commands::GlobalContext::from_cli(&self) {
            Ok(ctx) => ctx,
            Err(err) => return commands::exit_code_for(Err(err), output),
        };
        let result = match self.command {
            Commands::Init(args) => commands::init::run(&ctx, args).await,
            Commands::New(args) => commands::new::run(&ctx, args).await,
            Commands::Copy(args) => commands::copy::run(&ctx, args),
            Commands::Pull(args) => commands::pull::run(&ctx, args).await,
            Commands::Adopt(args) => commands::adopt::run(&ctx, args).await,
            Commands::Push(args) => commands::push::run(&ctx, args).await,
            Commands::Diff(args) => commands::diff::run(&ctx, args).await,
            Commands::Delete(args) => commands::delete::run(&ctx, args).await,
            Commands::Promote(args) => commands::promote::run(&ctx, args).await,
            Commands::Verify(args) => commands::verify::run(&ctx, args).await,
            Commands::Az { command } => commands::az::run(&ctx, command).await,
            Commands::Migrate { command } => commands::migrate::run(&ctx, command).await,
            Commands::Status(args) => commands::status::run(&ctx, args).await,
            Commands::Describe(args) => commands::describe::run(&ctx, args),
            Commands::Concepts => commands::concepts::run(&ctx),
            Commands::Validate(args) => commands::validate::run(&ctx, args),
            Commands::Env { command } => commands::env::run(&ctx, command).await,
            Commands::Auth { command } => commands::auth::run(&ctx, command).await,
            Commands::Ai { command } => commands::ai::run(command).await,
            Commands::Mcp(args) => commands::mcp_cmd::run(&ctx, args).await,
            Commands::Ci { command } => commands::ci::run(&ctx, command).await,
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
        commands::exit_code_for(result, output)
    }
}
