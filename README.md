<p align="center">
  <img src="https://raw.githubusercontent.com/mklab-se/rigg/main/media/rigg-horizontal.png" alt="rigg" width="600">
</p>

<h1 align="center">rigg</h1>

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
- **Environment promotion** — copy resources between services (dev to staging to prod) with automatic reference rewriting
- **CI/CD** — validate configuration in pull requests, push on merge, detect drift on a schedule, all with service principal auth

**For your AI coding tools:**

- **Full project understanding** — `rigg describe` gives AI tools the complete dependency graph from agents through knowledge bases to indexes in a single call
- **Direct access** — a built-in [MCP server](#ai-agent-integration) lets Claude Code, GitHub Copilot, and other AI tools pull, push, diff, and explore your resources through structured tool calls
- **File-level context** — with every definition as a local file, AI can read and reason about your entire stack. No portal access, no REST API calls, no blind spots

You can use rigg for **Azure AI Search alone**, **Microsoft Foundry alone**, or **both together**. The init flow lets you choose which services to manage, and you can add the other later.

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
# Initialize a project (discovers your services via Azure CLI)
rigg init .

# Pull all resources as local files
rigg pull --all

# Edit locally, then push changes back
rigg push --all
```

During `init`, rigg discovers your Azure AI Search services and Microsoft Foundry projects via ARM APIs and lets you choose which to manage. It creates a named environment (default: `prod`) and sets up the directory structure. If you're not logged in to Azure CLI, you can enter service names manually.

For a complete greenfield walkthrough — building an Agentic RAG system from scratch — see **[Getting Started](GETTING_STARTED.md)**.

**Connect your AI tool** (optional but recommended):

```bash
# Register rigg's MCP server with Claude Code
rigg mcp install claude-code

# Or VS Code (GitHub Copilot)
rigg mcp install vs-code
```

Now your AI tool can see your entire RAG stack — run `/rigg-status` to try it. See [MCP.md](MCP.md) for the full reference.

After pulling, your project contains normalized, version-control-friendly representations of every resource:

```
rigg.yaml                                     # Project configuration
.rigg/                                        # Per-environment sync state (gitignored)

search/
  search-management/                          # Stable search resources
    indexes/
      regulatory-index.json                   # Index schema (fields, vector search, semantic config)
    indexers/
      regulatory-indexer.json                 # Indexer schedule and mapping
    data-sources/
      regulatory-datasource.json              # Data source connection
    skillsets/
      regulatory-skillset.json                # AI enrichment pipeline
    synonym-maps/
      terms.json
  agentic-retrieval/                          # Preview agentic retrieval resources
    knowledge-bases/
      regulatory-kb.json                      # KB description, retrieval instructions, linked sources
    knowledge-sources/
      regulatory/
        regulatory.json                       # KS definition, ingestion config, created resources
        regulatory-index.json                 # Managed index (auto-provisioned by Azure)
        regulatory-indexer.json               # Managed indexer
        regulatory-datasource.json            # Managed data source
        regulatory-skillset.json              # Managed skillset

foundry/
  agents/
    research-assistant.yaml                   # Agent definition (single YAML file, matches portal)
```

Each JSON file is normalized and deterministic — credentials stripped, properties in Azure's canonical order, arrays sorted by identity key. Foundry agents are stored as single YAML files matching the Foundry portal format.

Use `rigg describe` to see how everything connects:

```
My RAG System
=============

Services:
  Environment: prod (default)
  Azure AI Search: my-search-service
  Microsoft Foundry: my-ai-service/my-project

Foundry Agents (1):

  research-assistant (gpt-4o)
    Tools: mcp -> regulatory-kb
    Instructions: You are a research assistant specialized in regulatory compliance...

Agentic RAG Flows:

  research-assistant
  └─ Knowledge Base: regulatory-kb
        Description:
        Official regulatory and legal texts for EU digital law...
        Output: extractiveData
        Retrieval instructions:
        You are a legal evidence retriever. Find and return relevant legal passages...
        └─ Knowledge Source: regulatory (azureBlob)
              Regulatory PDFs with structured metadata and vector search...
              └─ Index: regulatory-index (13 fields, key: uid)
                 1 vector profile(s), semantic search

Indexes (1):
  regulatory-index (13 fields, key: uid)
    ...
```

## Features

### Pull & Push

Download resource definitions from Azure and upload local changes back:

```bash
# Pull everything (search + foundry)
rigg pull --all

# Pull specific resource types
rigg pull --indexes --skillsets
rigg pull --agents

# Pull a single resource by name
rigg pull --index hotels
rigg pull --agent research-assistant

# Scope to one service domain
rigg pull --search-only
rigg pull --foundry-only

# Push (shows preview, asks for confirmation)
rigg push --all

# Push without confirmation
rigg push --all --force

# Push a single resource
rigg push --indexer hotels-indexer
```

### Semantic Diff

Compare local files against the live service with field-level change descriptions:

```bash
rigg diff --all
```

```
~ Index 'hotels' (modified)
    fields[3].type: Edm.String → Edm.Int32
    fields[7]: added 'rating'
    scoringProfiles[0].functions: 2 → 3 items
```

### Copy

Copy resources locally under new names, then push separately:

```bash
# Copy a knowledge source and all its managed sub-resources
rigg copy my-ks my-new-ks --knowledgesource

# Copy a standalone index
rigg copy hotels hotels-v2 --index

# Then push the copy
rigg push --knowledgesources
```

Knowledge source copy automatically renames all managed sub-resources (index, indexer, data source, skillset) and rewrites cross-references. No network calls — files are created locally for review before pushing.

### Scaffolding

Create new resource files from templates — no Azure connection required:

```bash
# Create individual resources
rigg new index my-index --vector --semantic
rigg new agent my-agent --model gpt-4o
rigg new knowledge-source my-ks --index my-index

# Scaffold a complete Agentic RAG system in one command
rigg new agentic-rag my-system --model gpt-4o --container documents
```

The `agentic-rag` command creates a pre-wired agent, knowledge base, and knowledge source — all connected and ready to push.

### Watch Mode

Continuously poll for server-side changes:

```bash
rigg pull-watch --all --interval 30
```

### Validation

Check local files for structural issues and referential integrity before pushing:

```bash
rigg validate
```

### CI/CD

Use rigg in your CI/CD pipeline to validate, deploy, and detect drift:

```yaml
# GitHub Actions example
- name: Validate
  run: rigg validate --strict
  env:
    AZURE_CLIENT_ID: ${{ secrets.AZURE_CLIENT_ID }}
    AZURE_CLIENT_SECRET: ${{ secrets.AZURE_CLIENT_SECRET }}
    AZURE_TENANT_ID: ${{ secrets.AZURE_TENANT_ID }}

- name: Push
  if: github.ref == 'refs/heads/main'
  run: rigg push --all --force
```

- **PR gate** — `rigg validate --strict` in CI catches schema errors and broken references before merge
- **Auto-deploy** — `rigg push --all --force` on merge to `main` deploys changes automatically
- **Drift detection** — schedule `rigg diff --all` to catch portal changes between deployments
- **Service principal auth** — set `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, and `AZURE_TENANT_ID` environment variables

## Resource Types

### Azure AI Search

| Resource | Flag | Singular | API |
|---|---|---|---|
| Index | `--indexes` | `--index <NAME>` | Stable |
| Indexer | `--indexers` | `--indexer <NAME>` | Stable |
| Data Source | `--datasources` | `--datasource <NAME>` | Stable |
| Skillset | `--skillsets` | `--skillset <NAME>` | Stable |
| Synonym Map | `--synonymmaps` | `--synonymmap <NAME>` | Stable |
| Alias | `--aliases` | `--alias <NAME>` | Preview |
| Knowledge Base | `--knowledgebases` | `--knowledgebase <NAME>` | Preview |
| Knowledge Source | `--knowledgesources` | `--knowledgesource <NAME>` | Preview |

### Microsoft Foundry

| Resource | Flag | Singular | API |
|---|---|---|---|
| Agent | `--agents` | `--agent <NAME>` | Preview (`2025-05-15-preview`) |

Use `--search-only` or `--foundry-only` to scope operations to a single service domain. Preview resources require `include_preview = true` in config (enabled by default with the `agentic` init template).

## Authentication

`rigg` authenticates using the Azure CLI or service principal credentials:

```bash
# Option 1: Azure CLI (recommended for development)
az login
rigg pull --all

# Option 2: Service principal (for CI/CD)
export AZURE_CLIENT_ID=...
export AZURE_CLIENT_SECRET=...
export AZURE_TENANT_ID=...
rigg pull --all
```

## Configuration

Project settings live in `rigg.yaml`:

```yaml
project:
  name: My RAG System

sync:
  include_preview: true

environments:
  prod:
    default: true
    search:
      - name: my-search-service
        api_version: "2024-07-01"                    # default
        preview_api_version: "2025-11-01-preview"    # default
    foundry:
      - name: my-ai-service
        project: my-project
        api_version: "2025-05-15-preview"            # default

  test:
    search:
      - name: my-search-test
    foundry:
      - name: my-ai-service-test
        project: my-project-test
```

View and modify settings with the `config` command:

```bash
rigg config show
rigg config set sync.include_preview false
```

### Deployment Environments

Manage the same resource definitions across multiple Azure targets:

```bash
# Add a new environment
rigg env add test

# List environments
rigg env list

# Pull from a specific environment
rigg pull --all --env test

# Push to a specific environment
rigg push --all --env prod

# Compare two environments
rigg diff --all --env test --compare-env prod

# Set the default environment
rigg env set-default prod
```

The `--env` flag (or `RIGG_ENV` environment variable) works with all commands. When omitted, rigg uses the environment marked `default: true` in the config.

## AI Agent Integration

Your Agentic RAG stack is a graph: agents connect to knowledge bases, which route to knowledge sources, which index data through skillsets. Understanding one piece in isolation isn't enough — and that's exactly the limitation AI tools hit when your configuration lives only in Azure portals and REST APIs.

rigg solves this by making every resource a local file *and* exposing a structured [MCP](https://modelcontextprotocol.io/) server that gives AI coding tools the complete picture. `rigg describe` returns the full project graph — every resource, dependency, agent instruction, and file path — in a single call. With this context, your AI tool can help you optimize agent instructions, debug retrieval pipelines, plan schema changes, and deploy across environments.

Any MCP-compatible AI tool works: Claude Code, GitHub Copilot, Cursor, Codex, Gemini CLI.

```bash
# Register with Claude Code
rigg mcp install claude-code

# Or VS Code (GitHub Copilot)
rigg mcp install vs-code
```

Projects with a `.mcp.json` file in the repo root are auto-discovered — no manual setup needed.

Once connected, use slash commands for common workflows:

| Command | What it does |
|---------|--------------|
| `/rigg-status` | Show environment info, auth state, and resource inventory |
| `/rigg-pull` | Pull from Azure with preview and confirmation |
| `/rigg-push` | Safe push: validate, diff, confirm, then push |

See [MCP.md](MCP.md) for the MCP tool reference, and [SKILLS.md](SKILLS.md) for the full list of agent skills and slash commands.

## Architecture

Four crates with a clear dependency hierarchy:

```
rigg  →  rigg-core
     ↓          ↑
rigg-client ───┘
rigg-diff  (standalone)
```

| Crate | Purpose |
|---|---|
| `rigg-core` | Resource types, config, environment resolution, state tracking, JSON normalization, copy/rename logic |
| `rigg-client` | Azure Search and Foundry REST API clients, ARM discovery, authentication |
| `rigg-diff` | Semantic JSON diffing with identity-key-based array matching |
| `rigg` | Clap-based CLI, command implementations |

## License

MIT — see [LICENSE.md](LICENSE.md).
