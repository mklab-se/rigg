<p align="center">
  <img src="https://raw.githubusercontent.com/mklab-se/rigg/main/media/rigg-horizontal.png" alt="rigg" width="600">
</p>

<h1 align="center">rigg</h1>

<p align="center"><em>Previously known as <strong>hoist</strong>.</em></p>

<p align="center">
  Configuration-as-code for <a href="https://learn.microsoft.com/en-us/azure/search/">Azure AI Search</a> and <a href="https://learn.microsoft.com/en-us/azure/ai-services/agents/">Microsoft Foundry</a>.<br>
  Version control your entire Agentic RAG stack — and give AI tools like Claude Code and Copilot the context to help you build it.
</p>

<p align="center">
  <a href="https://github.com/mklab-se/rigg/actions/workflows/ci.yml"><img src="https://github.com/mklab-se/rigg/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/rigg"><img src="https://img.shields.io/crates/v/rigg.svg" alt="crates.io"></a>
  <a href="https://github.com/mklab-se/rigg/releases/latest"><img src="https://img.shields.io/github/v/release/mklab-se/rigg" alt="GitHub Release"></a>
  <a href="https://github.com/mklab-se/homebrew-tap/blob/main/Formula/rigg.rb"><img src="https://img.shields.io/badge/dynamic/regex?url=https%3A%2F%2Fraw.githubusercontent.com%2Fmklab-se%2Fhomebrew-tap%2Fmain%2FFormula%2Frigg.rb&search=%5Cd%2B%5C.%5Cd%2B%5C.%5Cd%2B&label=homebrew&prefix=v&color=orange" alt="Homebrew"></a>
  <a href="https://github.com/mklab-se/rigg/blob/main/LICENSE.md"><img src="https://img.shields.io/crates/l/rigg.svg" alt="License"></a>
</p>

## The Problem

Building an Agentic RAG (Retrieval-Augmented Generation) system in Azure means configuring resources across two services: **Azure AI Search** for the retrieval layer — indexes, skillsets, indexers, knowledge bases — and **Microsoft Foundry** for the agent layer — agent definitions, instructions, tools, and knowledge connections. Together, they form a pipeline where agents query knowledge bases, which route to knowledge sources, which search indexes built from your data.

None of this configuration is managed by traditional IaC tools. ARM, Bicep, and Terraform provision the *services*, but the configuration *inside* them — the index schemas, skillset pipelines, agent instructions, and knowledge base retrieval rules that actually determine how your system behaves — lives in REST APIs and portal blades.

For relational databases, this gap was solved long ago with migration tools like Flyway, Liquibase, and Alembic. Azure AI Search and Microsoft Foundry have no equivalent. In practice, this means:

- **Fragmented view** — The full picture of how your agents, knowledge bases, knowledge sources, indexes, skillsets, and data sources connect is spread across two services, multiple portal blades, and REST endpoints. No one can reason about the system as a whole — and neither can your AI coding tools. Ask Claude Code or Copilot to help optimize your agent's retrieval pipeline, and they can't see any of it. Your RAG configuration is trapped behind APIs and portal blades that AI tools have no access to.
- **No change history** — Azure doesn't track who changed an index schema, agent instruction, or knowledge base configuration. When something breaks, there's no way to see what happened or roll back.
- **Portal drift** — The portal makes ad-hoc changes frictionless. In team environments, configurations silently diverge between services and between what's deployed and what anyone remembers deploying.
- **No review process** — Agent instructions, scoring profiles, skillset configurations, and knowledge base retrieval rules go live without review, even though they fundamentally shape how your AI system responds.
- **No CI/CD pipeline** — There's no way to validate configuration in a pull request, auto-deploy on merge, or detect drift on a schedule. Every deployment is manual.
- **Manual environment promotion** — Copying configurations from dev to staging to production means manually exporting JSON across both services, updating cross-resource references, and hoping nothing was missed.

## What Rigg Does

`rigg` makes your entire Agentic RAG infrastructure visible, reviewable, and AI-accessible. It pulls resource definitions from Azure AI Search and Microsoft Foundry as local files, versions them in Git, and pushes changes back. The same `rigg pull` that gives you Git history also gives Claude Code the context to help you optimize your agent.

Whether you use both services together for a full RAG stack, or either one independently, rigg serves two audiences at once:

**For you and your team:**

- **Version control** — track who changed what, when, and why via Git history across both your retrieval and agent layers
- **Code review** — review agent instructions, knowledge base retrieval rules, index schema changes, and skillset updates in pull requests before they go live
- **Drift detection** — diff local files against live services to catch manual portal changes across both Azure AI Search and Foundry
- **Environment promotion** — `rigg promote` copies a project's tree from dev to staging to prod, keeping each environment's pinned fields (secrets, per-env values); environment-specific references (like knowledge-base MCP endpoints) are injected at push time; protected environments (e.g. prod) require explicit confirmation before anything is pushed or deleted
- **CI/CD** — validate configuration in pull requests, deploy on merge, detect drift on a schedule — with OIDC federated login and no stored secrets

**For your AI coding tools:**

- **Full project understanding** — `rigg describe` gives AI tools the complete dependency graph from agents through knowledge bases to indexes in a single call
- **Direct access** — a built-in [MCP server](#ai-agent-integration) lets Claude Code, GitHub Copilot, and other AI tools pull, push, diff, and explore your resources through structured tool calls
- **File-level context** — with every definition as a local file, AI can read and reason about your entire stack. No portal access, no REST API calls, no blind spots

You can use rigg for **Azure AI Search alone**, **Microsoft Foundry alone**, or **both together**. The init flow lets you choose which services to manage, and you can add the other later.

## Concepts

rigg has two levels. A **workspace** (`rigg.yaml`) holds your environments and
service connections; a **project** is a group of resource definitions you pull,
push, review, and deploy as one unit — and every resource belongs to exactly one
project. That single rule is what keeps sync unambiguous.

New to the model, or unsure whether to use one project or several? Read
**[CONCEPTS.md](CONCEPTS.md)** — or run `rigg concepts` for the same guide in
your terminal.

## Quick Start

```bash
# Install
cargo install rigg
```

On macOS, you can also install via Homebrew:

```bash
brew install mklab-se/tap/rigg
```

See [INSTALL.md](INSTALL.md) for all installation methods, pre-built binaries, and shell completions.

```bash
# Initialize a workspace (discovers your services via Azure CLI)
rigg init .
# …or keep rigg's files in a subfolder of the workspace: rigg init rag

# Create a project — the unit rigg syncs
rigg new project my-rag

# Adopt existing Azure resources into it — à la carte…
rigg adopt my-rag                     # interactive: pick resources from a live menu
rigg adopt my-rag all                 # everything unmanaged
rigg adopt my-rag agents/my-agent     # just one resource
rigg adopt my-rag indexes --with-deps # a whole kind + its dependencies

# Later: capture newly-added dependencies of something you already manage
rigg adopt my-rag agents/my-agent --with-deps

# …or scaffold an explicit RAG pipeline from scratch
rigg new pipeline docs -p my-rag

# Validate, review, push
rigg validate my-rag
rigg push my-rag --dry-run
rigg push my-rag
```

A workspace (`rigg.yaml`) defines environments and service connections; each **project** under `projects/` owns its resource definitions exclusively, and `pull`/`push`/`diff` always operate on whole projects — no more half-synced states. During `init`, rigg discovers your Azure AI Search services and Microsoft Foundry projects via ARM APIs and lets you choose which to manage. If you're not logged in to Azure CLI, you can enter service names manually.

For a complete greenfield walkthrough — building an Agentic RAG system from scratch — see **[Getting Started](GETTING_STARTED.md)**.

**Connect your AI tool** (optional but recommended):

```bash
# Register rigg's MCP server with Claude Code
rigg mcp install claude-code

# Or VS Code (GitHub Copilot)
rigg mcp install vs-code
```

Now your AI tool can see your entire RAG stack — run `/rigg-status` to try it. See [MCP.md](MCP.md) for the full reference.

## Workspace Layout

After scaffolding or pulling, a workspace looks like this:

```
rigg.yaml                        # workspace: environments + service connections (YAML)
apis/
  doc-enrichment.json            # shared OpenAPI specs for custom Web API skills
projects/
  my-rag/
    project.yaml                 # metadata only — the directory IS the membership
    envs/
      dev/
        search/
          data-sources/docs-ds.json
          indexes/docs-index.json
          skillsets/docs-skills.json
          indexers/docs-indexer.json
          knowledge-sources/docs-ks.json
          knowledge-bases/docs-kb.json
        foundry/
          deployments/docs-model.json
          agents/docs-agent.json
          agents/docs-agent.instructions.md   # $file sidecar for long text
      prod/
        search/...
        foundry/...
.rigg/                           # per-environment sync state (gitignored)
```

Every project keeps a **separate resource tree per environment**, under `envs/<env>/` — dev and prod are never one shared file with overlay patches, so their divergence is visible and diffable. See [Deployment Environments](#deployment-environments) below and the [Environments chapter](CONCEPTS.md#environments) of CONCEPTS.md for the full model, including how a file's *path* (not its `name` field) is a resource's identity across environments.

Every resource is a normalized, deterministic JSON file that belongs to exactly one project — rigg enforces this. Long text fields like agent instructions live in Markdown sidecars (`{"$file": "docs-agent.instructions.md"}`) so they diff and review like prose. Credentials are never written to disk; write-only fields (like data source connection strings) are preserved locally and never echoed back by Azure.

Use `rigg describe` to see how everything connects — every resource, its dependencies, and the custom APIs your skillsets expect you to implement:

```
my-rag
  data-sources/docs-ds
  indexes/docs-index
  skillsets/docs-skills
  indexers/docs-indexer -> data-sources/docs-ds, indexes/docs-index, skillsets/docs-skills
  knowledge-sources/docs-ks -> indexes/docs-index
  knowledge-bases/docs-kb -> knowledge-sources/docs-ks
  agents/docs-agent -> knowledge-bases/docs-kb, deployments/docs-model
  deployments/docs-model -> guardrails/default-guardrail
  guardrails/default-guardrail

  APIs to implement (specs in apis/):
    doc-enrichment (used by skillsets/docs-skills)
```

`rigg describe --output json` returns the same graph with full definitions and file paths — the fastest way for an AI tool to understand the workspace.

## Features

### Whole-Project Sync

Pull, push, and diff always operate on whole projects (see [Concepts](#concepts)), so local and remote can never end up half-synced:

```bash
rigg pull my-rag                # pull the project's resources from Azure
rigg adopt my-rag <selector>    # adopt selected unmanaged resources (all | <kind> | <kind>/<name>)
rigg pull my-rag --watch        # keep polling for remote changes

rigg push my-rag --dry-run      # show the dependency-ordered plan, change nothing
rigg push my-rag                # create/update, in dependency order
rigg push my-rag --prune        # also delete remote resources whose files were removed

rigg migrate knowledge-source <name> --in-place       # convert a portal-created (azureBlob, ...)
                                #   knowledge source to explicit searchIndex form; the next push
                                #   REPLACES it (index rebuild — gated by --allow-replace)
rigg migrate ks <name> --rename <new>   # or build a side-by-side pipeline under new names

rigg az indexer run <name> --watch      # trigger a live indexer and follow the run
rigg az indexer status <name>           # execution state + per-document errors
rigg az index query <name> "gdpr"       # smoke-test retrieval against the live index
rigg az kb ask <name> "What does..."    # agentic retrieval: grounding + references
rigg az agent ask <name> "Summarize..." # single-shot prompt to a Foundry agent

rigg delete my-rag --remote     # delete the project's resources from Azure (files kept)
rigg status                     # per-resource sync state across all projects
```

After every successful push, rigg fetches the document back from Azure, normalizes it, and updates the local file and sync baseline — so server-side defaults never show up as false drift.

### Semantic Diff

Compare local files against the live service with field-level change descriptions. Volatile server fields are ignored and array order doesn't matter:

```bash
rigg diff my-rag
```

```
docs-index — differs (2 field(s))

  field                                    local                Azure (dev)
  fields[rating]                           {...} (2 keys)       (absent)
  fields[chunk].type                       "Edm.Int32"          "Edm.String"

hint: rigg pull my-rag — update local files to match Azure
      rigg push my-rag — make Azure match your local files
```

Each row is labeled by side (`local` / `Azure (<env>)`), never by "was"/"now" — the diff itself doesn't assume which direction you're headed. The hint spells out both.

```bash
rigg diff --all --exit-code                    # CI: exit 5 when drift is found
rigg diff my-rag --format markdown             # PR-comment friendly output
rigg diff my-rag --only indexes/docs-index     # one resource only
rigg diff my-rag -e test --compare-env prod    # environment vs environment
```

### Scaffolding

Create projects, resources, pipelines, and API specs from identity-first templates — no Azure connection required:

```bash
rigg new project my-rag

# Full explicit retrieval chain: data source → index → skillset → indexer
# → knowledge source → knowledge base
rigg new pipeline docs -p my-rag --type azureblob

# Individual resources (12 kinds across both services)
rigg new index products -p my-rag
rigg new data-source orders -p my-rag --type cosmosdb
rigg new agent helper -p my-rag
rigg new deployment gpt-4-1-mini -p my-rag

# OpenAPI 3.1 spec for a custom WebApiSkill, shared workspace-wide in apis/
rigg new api doc-enrichment
```

With [AI features](#ai-assistance) enabled, `--describe` drafts the definition for you:

```bash
rigg new index hotels -p my-rag --describe "hotel search with vector fields and semantic ranking"
```

### Copy

Copy a resource file locally under a new name — within or across projects — then review and push:

```bash
rigg copy indexes/docs-index docs-index-v2
rigg copy my-rag:agents/docs-agent other-project:docs-agent
```

### Validation

Check local files before pushing — JSON structure, name/filename consistency, exclusive ownership, reference resolution, valid data source types, and **no-secrets enforcement** (key-based credentials are rejected; use `ResourceId=` connection strings and managed identity instead):

```bash
rigg validate                # all projects
rigg validate my-rag --strict
```

`validate` also checks WebApiSkills linked via `"x-rigg-api"` against their OpenAPI spec in `apis/` — skill URIs must match a spec path, and skill inputs/outputs must exist in the request/response schemas.

### Samples

The [`samples/`](samples/) directory is a complete working workspace with three projects: [`quickstart-blob`](samples/projects/quickstart-blob/) (the minimal explicit pipeline), [`agentic-stack`](samples/projects/agentic-stack/) (the full showcase — custom Web API skill, knowledge base, Foundry agent + deployment + guardrail), and [`cosmos-sql-patterns`](samples/projects/cosmos-sql-patterns/) (Cosmos DB and Azure SQL change/deletion-detection done right).

### Deployment Environments

Each environment is a named Azure target with its own resource tree (`envs/<env>/` — see [Workspace Layout](#workspace-layout)), so dev and prod never share a JSON file. Add one interactively (ARM discovery, same pick-lists as `rigg init`) or non-interactively with flags:

```bash
rigg env add test                          # interactive wizard
rigg env add test --search-service my-search-test
rigg env list
rigg env set-default prod
```

The `--env`/`-e` flag (or the `RIGG_ENV` environment variable) works with all commands. When omitted, rigg uses the environment marked `default: true` in `rigg.yaml`:

```yaml
environments:
  dev:
    default: true
    search: { service: my-search-dev }
    foundry: { account: my-foundry, project: my-project-dev }
  prod:
    policy: { protected: true }
    search: { service: my-search-prod }
    foundry: { account: my-foundry, project: my-project-prod }
```

`rigg promote` copies one environment's project tree into another, locally — preserving the target's pinned fields (`name`, secrets/write-only fields, `x-rigg-pin`-annotated paths) instead of overwriting them:

```bash
rigg promote my-rag --from dev --to prod --dry-run   # preview
rigg promote my-rag --from dev --to prod             # write prod's tree
rigg push my-rag --env prod                          # then sync it to Azure
rigg diff my-rag -e test --compare-env prod          # or just compare, env vs env
```

Marking an environment `policy: { protected: true }` (as `prod` is above) requires an explicit, typed confirmation before rigg mutates it — `--yes` alone is never enough, since it only skips the routine "apply N changes?" prompt:

```bash
rigg push my-rag --env prod --yes                       # blocked: prod is protected
rigg push my-rag --env prod --yes --confirm-env prod    # proceeds
```

### Authentication

`rigg` is identity-first — no keys, no secrets in files, ever:

```bash
rigg auth login       # delegates to Azure CLI
rigg auth status
rigg auth doctor      # verify service-to-service identities and RBAC
rigg auth doctor --fix
```

`auth doctor` derives the identity graph from your workspace files — data source connections, knowledge-base model wiring, agent-to-KB grounding — verifies managed identities and RBAC role assignments via ARM, and repairs them with `--fix` (or prints the exact `az` commands). Cosmos/SQL data-plane permissions are reported with guidance. For stacks spanning multiple services, prefer a shared **user-assigned managed identity** — role assignments survive service re-creation.

In CI or automation, rigg also accepts service-principal environment variables (`AZURE_CLIENT_ID`/`AZURE_TENANT_ID`/…) or a static bearer token via `RIGG_ACCESS_TOKEN`. Sovereign clouds and test rigs can override the service endpoint with `endpoint:` on a connection in `rigg.yaml`.

### CI/CD

One command scaffolds a complete GitHub Actions setup:

```bash
rigg ci init github
```

This creates three workflows:

- **Validate on PR** — `rigg validate --strict` plus a markdown diff posted as a PR comment, so reviewers see exactly what merging would change in Azure
- **Deploy on merge** — `rigg push --all --yes` on `main`, authenticated with OIDC federated login (no stored secrets)
- **Nightly drift detection** — `rigg diff --all --exit-code --format markdown`; opens or updates a GitHub issue when the portal has drifted from Git

The target environment is baked into the workflows at scaffold time (pass `--env`, or your default environment is used). Finish the setup by creating an Entra app registration with federated credentials and adding `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, and `AZURE_SUBSCRIPTION_ID` as repository variables — `rigg ci init` prints the exact steps.

### AI Assistance

rigg has opt-in AI features powered by [ailloy](https://crates.io/crates/ailloy) — bring your own provider:

```bash
rigg ai enable        # turn on AI features
rigg ai config        # choose provider/model (interactive)
rigg ai status
```

Once enabled:

- **Diff summaries** — `rigg diff` appends a plain-language summary of what pushing would do, including cost/risk callouts
- **Conflict merging** — the interactive push conflict menu gains an AI merge proposal, shown diffed against both local and remote before anything is written
- **Doctor advice** — `rigg auth doctor` failures get tailored remediation notes
- **Drafting** — `rigg new <kind> <name> --describe "…"` drafts resource definitions from natural language

Pass `--no-ai` on any command to disable AI assistance for that invocation.

## Resource Kinds

rigg manages 12 resource kinds. All are served by stable APIs — Azure AI Search `2026-04-01` (agentic retrieval is GA), preview `2026-05-01-preview` only for preview-gated features, Microsoft Foundry `v1` data plane, ARM `2026-05-01`.

| Azure AI Search | Microsoft Foundry |
|---|---|
| `index` | `agent` |
| `indexer` | `deployment` (model deployments) |
| `data-source` | `connection` |
| `skillset` | `guardrail` (RAI policies) |
| `synonym-map` | |
| `alias` | |
| `knowledge-source` | |
| `knowledge-base` | |

Knowledge sources are **explicit**: they point at an existing index you define and own, so the whole retrieval chain is visible, reviewable files — nothing is auto-provisioned behind your back.

## AI Agent Integration

Your Agentic RAG stack is a graph: agents connect to knowledge bases, which route to knowledge sources, which search indexes fed by indexers and skillsets. Understanding one piece in isolation isn't enough — and that's exactly the limitation AI tools hit when your configuration lives only in Azure portals and REST APIs.

rigg solves this by making every resource a local file *and* exposing a structured [MCP](https://modelcontextprotocol.io/) server with 8 project-scoped tools. `rigg describe` returns the full workspace graph — every resource, dependency, agent instruction, and file path — in a single call. Mutating tools are safe by default: they return a preview until called with `force: true`.

Any MCP-compatible AI tool works: Claude Code, GitHub Copilot, Cursor, Codex, Gemini CLI.

```bash
rigg mcp install claude-code            # or vs-code
rigg mcp install claude-code --scope global
```

Once connected, use slash commands for common workflows:

| Command | What it does |
|---------|--------------|
| `/rigg-status` | Sync state per project, environments, drift, unmanaged resources |
| `/rigg-pull` | Pull from Azure with preview and confirmation |
| `/rigg-push` | Safe push: validate, review the plan, confirm, then push |

See [MCP.md](MCP.md) for the MCP tool reference, and [SKILLS.md](SKILLS.md) for the full list of agent skills.

## Exit Codes

Standardized for scripting and CI (`--non-interactive` guarantees rigg never blocks on a prompt):

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Error |
| 2 | Usage error |
| 3 | Validation failed |
| 4 | Auth / permission denied |
| 5 | Drift or conflict detected |

## Architecture

Four crates with a clear dependency hierarchy:

```
rigg  →  rigg-core
     ↓          ↑
rigg-client ───┘
rigg-diff  (used by rigg-core & rigg)
```

| Crate | Purpose |
|---|---|
| `rigg-core` | Workspace/project model, the metadata registry (API routing, volatile/secret fields, references), normalization, sync-state baselines, dependency graph, scaffolds |
| `rigg-client` | Azure AI Search, Foundry, and ARM REST clients; authentication chain |
| `rigg-diff` | Semantic JSON diffing with identity-key-based array matching |
| `rigg` | Clap-based CLI, command implementations, MCP server |

## License

MIT — see [LICENSE.md](LICENSE.md).
