<p align="center">
  <img src="https://raw.githubusercontent.com/mklab-se/hoist/main/media/hoist-horizontal.png" alt="hoist" width="600">
</p>

<h1 align="center">hoist</h1>

<p align="center">
  Configuration-as-code for <a href="https://learn.microsoft.com/en-us/azure/search/">Azure AI Search</a>.<br>
  Pull resource definitions from your search service as normalized JSON files,<br>
  version them in Git, and push changes back.
</p>

<p align="center">
  <a href="https://github.com/mklab-se/hoist/actions/workflows/ci.yml"><img src="https://github.com/mklab-se/hoist/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/hoist-az"><img src="https://img.shields.io/crates/v/hoist-az.svg" alt="crates.io"></a>
  <a href="https://github.com/mklab-se/hoist/releases/latest"><img src="https://img.shields.io/github/v/release/mklab-se/hoist" alt="GitHub Release"></a>
  <a href="https://github.com/mklab-se/hoist/blob/main/LICENSE.md"><img src="https://img.shields.io/crates/l/hoist-az.svg" alt="License"></a>
</p>

## The Problem

Infrastructure-as-Code tools like ARM, Bicep, and Terraform are great at provisioning an Azure AI Search *service* — but they stop at the front door. The configuration *inside* the service — index schemas, skillsets, indexer schedules, knowledge base definitions — is what actually determines how your application behaves, and none of these tools manage it.

For traditional relational databases, this gap was filled long ago. SQL provides a standardized language for defining tables, indexes, and stored procedures, and a mature ecosystem of migration tools (Flyway, Liquibase, EF Migrations, Alembic) has evolved around it. Azure AI Search has no equivalent. It's a specialized search and vector database with a REST/JSON API, no schema definition language, and no migration framework.

In practice, this means search configurations are managed through the Azure portal or one-off scripts, which creates real problems:

- **No change history** — Azure doesn't track who changed an index schema or when. If a field type change breaks your application, there's no way to see what happened or roll back.
- **Portal drift** — The portal makes ad-hoc changes frictionless. In team environments, configurations silently diverge between services and between what's deployed and what anyone remembers deploying.
- **No review process** — Index schema changes, scoring profile updates, and skillset modifications go live without review, even though they can fundamentally change application behavior.
- **Manual environment promotion** — Copying configurations from dev to staging to production means manually exporting JSON, updating cross-resource references, and hoping nothing was missed.
- **Growing stakes with agentic AI** — As Azure AI Search becomes the retrieval backbone for AI agents in Azure AI Foundry, these configurations become critical infrastructure. A misconfigured knowledge base or skillset silently degrades agent quality — making version control and review more important, not less.
- **Configuration locked behind APIs** — There is no single place to see how your indexes, skillsets, indexers, and knowledge bases fit together. The full picture is spread across portal blades and REST endpoints, making it hard for anyone — human or AI — to reason about the service as a whole.

## What Hoist Does

`hoist` bridges the gap by treating your search service configuration as code. It pulls resource definitions as normalized JSON files, versions them in Git, and pushes changes back:

- **Version control** — track who changed what, when, and why via Git history
- **Code review** — review index schema changes, skillset updates, and knowledge base configurations in pull requests
- **Environment promotion** — copy resources between services (dev → staging → prod) with automatic reference rewriting
- **Drift detection** — diff local files against the live service to catch manual portal changes
- **AI-assisted development** — with every resource definition available as a local file, AI coding tools like Claude Code, GitHub Copilot, Codex, and others can read your entire search configuration in context, understand how resources relate to each other, and help you develop, troubleshoot, and evolve your implementation — no API calls or portal access required

## Quick Start

```bash
# Install (pick one)
brew install mklab-se/tap/hoist   # Homebrew
cargo binstall hoist-az           # cargo-binstall (pre-built binary)
cargo install hoist-az            # cargo install (compile from source)
```

See [INSTALL.md](INSTALL.md) for all installation methods, pre-built binaries, and shell completions.

```bash
# Initialize a project (discovers your service via Azure CLI)
hoist init . --path search

# Pull all resources as JSON files
hoist pull --all

# Edit locally, then push changes back
hoist push --all
```

After `init`, your project looks like this:

```
hoist.toml                     # Project configuration
.hoist/                        # Sync state (gitignored)
search/
  search-management/
    indexes/
      hotels.json
    indexers/
      hotels-indexer.json
    data-sources/
      cosmos-hotels.json
    skillsets/
      enrichment-pipeline.json
    synonym-maps/
      hotel-synonyms.json
  agentic-retrieval/
    knowledge-bases/
      regulatory-kb.json
    knowledge-sources/
      regulatory-docs.json
```

Each JSON file is a normalized, deterministic representation of the resource — credentials stripped, properties in Azure's canonical order, arrays sorted by identity key.

## Features

### Pull & Push

Download resource definitions from Azure and upload local changes back:

```bash
# Pull everything
hoist pull --all

# Pull specific resource types
hoist pull --indexes --skillsets

# Pull a single resource by name
hoist pull --index hotels

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

| Resource | Flag | Singular | API |
|---|---|---|---|
| Index | `--indexes` | `--index <NAME>` | Stable |
| Indexer | `--indexers` | `--indexer <NAME>` | Stable |
| Data Source | `--datasources` | `--datasource <NAME>` | Stable |
| Skillset | `--skillsets` | `--skillset <NAME>` | Stable |
| Synonym Map | `--synonymmaps` | `--synonymmap <NAME>` | Stable |
| Knowledge Base | `--knowledgebases` | `--knowledgebase <NAME>` | Preview |
| Knowledge Source | `--knowledgesources` | `--knowledgesource <NAME>` | Preview |

Preview resources (Knowledge Bases and Knowledge Sources) use the `2025-11-01-preview` API and are included by default with the `agentic` template.

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
[service]
name = "my-search-service"
subscription_id = "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"

[sync]
resource_dir = "search"
include_preview = true

[api]
api_version = "2024-07-01"
preview_api_version = "2025-11-01-preview"
```

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
hoist-azent ────┘
hoist-diff  (standalone)
```

| Crate | Purpose |
|---|---|
| `hoist-core` | Resource types, config, state tracking, JSON normalization, copy/rename logic |
| `hoist-azent` | Azure Search REST API client, ARM discovery, authentication |
| `hoist-diff` | Semantic JSON diffing with identity-key-based array matching |
| `hoist-az` | Clap-based CLI, command implementations |

## License

MIT — see [LICENSE.md](LICENSE.md).
