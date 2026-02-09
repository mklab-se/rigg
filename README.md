<p align="center">
  <img src="https://raw.githubusercontent.com/mklab-se/hoist/main/media/hoist-horizontal.png" alt="hoist" width="600">
</p>

<h1 align="center">hoist</h1>

<p align="center">
  Configuration-as-code for <a href="https://learn.microsoft.com/en-us/azure/search/">Azure AI Search</a> and <a href="https://learn.microsoft.com/en-us/azure/ai-services/agents/">Microsoft Foundry</a>.<br>
  Manage your entire Agentic RAG stack — from agent definitions to knowledge bases to search indexes — as version-controlled files.
</p>

<p align="center">
  <a href="https://github.com/mklab-se/hoist/actions/workflows/ci.yml"><img src="https://github.com/mklab-se/hoist/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/hoist-az"><img src="https://img.shields.io/crates/v/hoist-az.svg" alt="crates.io"></a>
  <a href="https://github.com/mklab-se/hoist/releases/latest"><img src="https://img.shields.io/github/v/release/mklab-se/hoist" alt="GitHub Release"></a>
  <a href="https://github.com/mklab-se/hoist/blob/main/LICENSE.md"><img src="https://img.shields.io/crates/l/hoist-az.svg" alt="License"></a>
</p>

## The Problem

Building an Agentic RAG (Retrieval-Augmented Generation) system in Azure means configuring resources across two services: **Azure AI Search** for the retrieval layer — indexes, skillsets, indexers, knowledge bases — and **Microsoft Foundry** for the agent layer — agent definitions, instructions, tools, and knowledge connections. Together, they form a pipeline where agents query knowledge bases, which route to knowledge sources, which search indexes built from your data.

None of this configuration is managed by traditional IaC tools. ARM, Bicep, and Terraform provision the *services*, but the configuration *inside* them — the index schemas, skillset pipelines, agent instructions, and knowledge base retrieval rules that actually determine how your system behaves — lives in REST APIs and portal blades.

For relational databases, this gap was solved long ago with migration tools like Flyway, Liquibase, and Alembic. Azure AI Search and Microsoft Foundry have no equivalent. In practice, this means:

- **No change history** — Azure doesn't track who changed an index schema, agent instruction, or knowledge base configuration. When something breaks, there's no way to see what happened or roll back.
- **Portal drift** — The portal makes ad-hoc changes frictionless. In team environments, configurations silently diverge between services and between what's deployed and what anyone remembers deploying.
- **No review process** — Agent instructions, scoring profiles, skillset configurations, and knowledge base retrieval rules go live without review, even though they fundamentally shape how your AI system responds.
- **Fragmented view** — The full picture of how your agents, knowledge bases, knowledge sources, indexes, skillsets, and data sources connect is spread across two services, multiple portal blades, and REST endpoints. No one — human or AI — can easily reason about the system as a whole.
- **Manual environment promotion** — Copying configurations from dev to staging to production means manually exporting JSON across both services, updating cross-resource references, and hoping nothing was missed.

## What Hoist Does

`hoist` treats your entire Agentic RAG infrastructure as code. It pulls resource definitions from Azure AI Search and Microsoft Foundry as local files, versions them in Git, and pushes changes back. Whether you use both services together for a full RAG stack, or either one independently, hoist gives you:

- **Version control** — track who changed what, when, and why via Git history across both your retrieval and agent layers
- **Code review** — review agent instructions, knowledge base retrieval rules, index schema changes, and skillset updates in pull requests before they go live
- **Unified project view** — `hoist describe` shows the full dependency chain from agents through knowledge bases to indexes, so humans and AI tools can reason about the complete system
- **Drift detection** — diff local files against live services to catch manual portal changes across both Azure AI Search and Foundry
- **Environment promotion** — copy resources between services (dev to staging to prod) with automatic reference rewriting
- **AI-assisted development** — with every resource definition available as a local file, AI coding tools like Claude Code, GitHub Copilot, and others can read your entire search and agent configuration in context, understand how resources relate, and help you develop and troubleshoot — no portal access required

You can use hoist for **Azure AI Search alone**, **Microsoft Foundry alone**, or **both together**. The init flow lets you choose which services to manage, and you can add the other later.

## Quick Start

```bash
# Install
cargo install hoist-az
```

On macOS, you can also install via Homebrew:

```bash
brew install mklab-se/tap/hoist
```

See [INSTALL.md](INSTALL.md) for all installation methods, pre-built binaries, and shell completions.

```bash
# Initialize a project (discovers your services via Azure CLI)
hoist init .

# Pull all resources as JSON files
hoist pull --all

# Edit locally, then push changes back
hoist push --all
```

During `init`, hoist discovers your Azure AI Search services and Microsoft Foundry projects via ARM APIs and lets you choose which to manage. When there's only one option, it auto-selects. If you're not logged in to Azure CLI, you can enter service names manually.

After pulling, your project contains normalized, version-control-friendly representations of every resource:

```
hoist.toml                          # Project configuration
.hoist/                             # Sync state (gitignored)

search-resources/
  my-search-service/
    search-management/
      indexes/
        regulatory-index.json       # Index schema (fields, vector search, semantic config)
      indexers/
        regulatory-indexer.json     # Indexer schedule and mapping
      data-sources/
        regulatory-datasource.json  # Data source connection
      skillsets/
        regulatory-skillset.json    # AI enrichment pipeline
      synonym-maps/
        terms.json
    agentic-retrieval/
      knowledge-bases/
        regulatory-kb.json          # KB description, retrieval instructions, linked sources
      knowledge-sources/
        regulatory.json             # Source definition, ingestion config, created resources

foundry-resources/
  my-ai-service/
    my-project/
      agents/
        research-assistant/
          config.json               # Agent id, name, model, temperature
          instructions.md           # Agent instructions (editable Markdown)
          tools.json                # MCP tools, code interpreter, file search
          knowledge.json            # Knowledge/tool resources
```

Each JSON file is normalized and deterministic — credentials stripped, properties in Azure's canonical order, arrays sorted by identity key. Agent instructions are stored as Markdown for easy editing and diffing.

Use `hoist describe` to see how everything connects:

```
My RAG System
=============

Services:
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
hoist pull --all

# Pull specific resource types
hoist pull --indexes --skillsets
hoist pull --agents

# Pull a single resource by name
hoist pull --index hotels
hoist pull --agent research-assistant

# Scope to one service domain
hoist pull --search-only
hoist pull --foundry-only

# Push with dry-run preview
hoist push --all --dry-run

# Push a single resource
hoist push --indexer hotels-indexer
```

### Semantic Diff

Compare local files against the live service with field-level change descriptions:

```bash
hoist diff --all
```

```
~ Index 'hotels' (modified)
    fields[3].type: Edm.String → Edm.Int32
    fields[7]: added 'rating'
    scoringProfiles[0].functions: 2 → 3 items
```

### Cross-Service Copy

Copy resources between search services with automatic name remapping and reference rewriting:

```bash
# Copy with a suffix (auto-generates new names)
hoist push --all --target prod-search --suffix "-v2"

# Copy with interactive name prompts
hoist push --knowledgebase my-kb --recursive --copy --target staging

# Copy with a pre-built name mapping
hoist push --all --target prod-search --answers name-map.json
```

The `--recursive` flag automatically includes dependent and child resources. For example, `--knowledgebase my-kb --recursive` includes the knowledge base, all its knowledge sources, and their referenced indexes.

### Watch Mode

Continuously poll for server-side changes:

```bash
hoist pull-watch --all --interval 30
```

### Validation

Check local files for structural issues and referential integrity before pushing:

```bash
hoist validate
```

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

`hoist` authenticates using the Azure CLI or service principal credentials:

```bash
# Option 1: Azure CLI (recommended for development)
az login
hoist pull --all

# Option 2: Service principal (for CI/CD)
export AZURE_CLIENT_ID=...
export AZURE_CLIENT_SECRET=...
export AZURE_TENANT_ID=...
hoist pull --all
```

## Configuration

Project settings live in `hoist.toml`:

```toml
[project]
name = "My RAG System"

# Search service (at least one service type required)
[[services.search]]
name = "my-search-service"
api_version = "2024-07-01"                    # default
preview_api_version = "2025-11-01-preview"    # default

# Foundry service (at least one service type required)
[[services.foundry]]
name = "my-ai-service"
project = "my-project"
api_version = "2025-05-15-preview"              # default

[sync]
include_preview = true
generate_docs = true
```

The legacy `[service]` format from v0.1.x is still supported and auto-migrates on load.

View and modify settings with the `config` command:

```bash
hoist config get service.name
hoist config set sync.include_preview false
```

## Architecture

Four crates with a clear dependency hierarchy:

```
hoist-az  →  hoist-core
     ↓              ↑
hoist-client ───┘
hoist-diff  (standalone)
```

| Crate | Purpose |
|---|---|
| `hoist-core` | Resource types, config, state tracking, JSON normalization, agent decomposition, copy/rename logic |
| `hoist-client` | Azure Search and Foundry REST API clients, ARM discovery, authentication |
| `hoist-diff` | Semantic JSON diffing with identity-key-based array matching |
| `hoist-az` | Clap-based CLI, command implementations |

## License

MIT — see [LICENSE.md](LICENSE.md).
