# rigg — AI Agent Reference

## Tool Description

rigg is a configuration-as-code CLI for **Azure AI Search** and **Microsoft Foundry**. It pulls resource definitions from Azure as normalized JSON (or YAML for agents), lets you edit them locally under Git, and pushes changes back. Think of it as "Terraform for Azure Search and Foundry agents."

## Complete CLI Command Reference

### Project Lifecycle

| Command | Description |
|---------|-------------|
| `rigg init` | Initialize a new rigg project (creates `rigg.yaml`, directory structure) |
| `rigg status` | Show sync status, resource counts, environment info |
| `rigg describe` | Show a unified summary of all local resource definitions |
| `rigg validate [--strict] [--check-references]` | Validate local JSON files for structural and referential integrity |

### Sync Commands

| Command | Description |
|---------|-------------|
| `rigg pull [FLAGS]` | Download resource definitions from Azure to local JSON files |
| `rigg push [FLAGS]` | Upload local JSON files to Azure, creating or updating resources |
| `rigg diff [FLAGS]` | Compare local resource files against the live Azure service |
| `rigg pull-watch [FLAGS]` | Poll the server for changes and pull updates automatically |

### Resource Management

| Command | Description |
|---------|-------------|
| `rigg new <type> <name> [OPTIONS]` | Create a new resource file from a template (no network calls) |
| `rigg copy <source> <target> --<type>` | Copy a resource locally under a new name (no network calls) |
| `rigg delete --<type> <name> --target <remote\|local>` | Delete a resource from Azure or remove local files |

### Environment & Auth

| Command | Description |
|---------|-------------|
| `rigg env list` | List all configured environments |
| `rigg env show [name]` | Show details for an environment |
| `rigg env set-default <name>` | Set the default environment |
| `rigg env add <name>` | Add a new environment (interactive ARM discovery) |
| `rigg env remove <name>` | Remove an environment |
| `rigg auth login` | Authenticate with Azure |
| `rigg auth status` | Check current authentication status |
| `rigg auth logout` | Clear cached authentication |

### Configuration

| Command | Description |
|---------|-------------|
| `rigg config show` | Display current configuration from `rigg.yaml` |
| `rigg config set <key> <value>` | Set a configuration value |
| `rigg config init` | Interactive configuration setup |

### AI & MCP

| Command | Description |
|---------|-------------|
| `rigg ai` | Show AI feature status |
| `rigg ai enable` | Enable AI features |
| `rigg ai disable` | Disable AI features |
| `rigg ai config` | Interactive AI provider/model configuration |
| `rigg ai test [message]` | Test AI integration |
| `rigg ai skill` | Show skill setup guide |
| `rigg ai skill --emit` | Output skill markdown file to stdout |
| `rigg ai skill --reference` | Output this reference documentation |
| `rigg mcp serve` | Start MCP server (stdio transport) |
| `rigg mcp install [claude-code\|vs-code] [--scope workspace\|global]` | Register rigg as an MCP server |

## Resource Type Flags

These flags are shared across `pull`, `push`, `diff`, and `pull-watch` commands.

### Plural flags (select all of a type)

| Flag | Description |
|------|-------------|
| `--all` | Include all resource types |
| `--indexes` | Include indexes |
| `--indexers` | Include indexers |
| `--datasources` | Include data sources |
| `--skillsets` | Include skillsets |
| `--synonymmaps` | Include synonym maps |
| `--aliases` | Include aliases |
| `--knowledgebases` | Include knowledge bases (preview) |
| `--knowledgesources` | Include knowledge sources (preview) |
| `--agents` | Include Foundry agents |

### Singular flags (select one by name)

| Flag | Description |
|------|-------------|
| `--index <NAME>` | Operate on a single index |
| `--indexer <NAME>` | Operate on a single indexer |
| `--datasource <NAME>` | Operate on a single data source |
| `--skillset <NAME>` | Operate on a single skillset |
| `--synonymmap <NAME>` | Operate on a single synonym map |
| `--alias <NAME>` | Operate on a single alias |
| `--knowledgebase <NAME>` | Operate on a single knowledge base |
| `--knowledgesource <NAME>` | Operate on a single knowledge source |
| `--agent <NAME>` | Operate on a single Foundry agent |

### Service scope flags

| Flag | Description |
|------|-------------|
| `--search-only` | Only operate on Azure AI Search resources |
| `--foundry-only` | Only operate on Microsoft Foundry resources |

## File Structure on Disk

```
project-root/
  rigg.yaml                           # Project configuration
  .rigg/                              # Per-environment state (gitignored)
    <env>/
      state.json
      checksums.json
  search/                             # Azure AI Search resources
    search-management/                # Stable resources
      indexes/
        <name>.json
      indexers/
        <name>.json
      data-sources/
        <name>.json
      skillsets/
        <name>.json
      synonym-maps/
        <name>.json
      aliases/
        <name>.json
    agentic-retrieval/                # Preview resources
      knowledge-bases/
        <name>.json
      knowledge-sources/
        <ks-name>/
          <ks-name>.json              # Knowledge source definition
          <ks-name>-index.json        # Managed index
          <ks-name>-indexer.json      # Managed indexer
          <ks-name>-datasource.json   # Managed data source
          <ks-name>-skillset.json     # Managed skillset
  foundry/                            # Microsoft Foundry resources
    agents/
      <agent-name>.yaml              # One YAML file per agent
```

Multi-service layout (when an environment has multiple services per domain):
```
search/
  primary/search-management/indexes/...
  analytics/search-management/indexes/...
foundry/
  rag/agents/...
  chat/agents/...
```

## Key Workflows

### Pull -> Edit -> Validate -> Diff -> Push Cycle

1. **Pull** resources from Azure:
   ```
   rigg pull --all
   ```
2. **Edit** the local JSON/YAML files as needed.
3. **Validate** structural integrity:
   ```
   rigg validate --strict --check-references
   ```
4. **Diff** to see what will change:
   ```
   rigg diff --all
   ```
5. **Push** changes to Azure:
   ```
   rigg push --all
   ```

### Common flag patterns

- Use `--force` to skip confirmation prompts (useful for CI/CD).
- Without `--force`, pull and push show a preview and ask for confirmation.
- Use `--recursive` with singular flags to include dependent resources (e.g., `rigg pull --knowledgesource myks --recursive`).
- Use `--filter <substring>` to match resources by name.

## Environment Management

```bash
# List environments
rigg env list

# Show current environment details
rigg env show

# Show a specific environment
rigg env show staging

# Change default environment
rigg env set-default staging

# Add a new environment (interactive)
rigg env add staging

# Remove an environment
rigg env remove old-env
```

The `--env <name>` flag (or `RIGG_ENV` env var) on any command targets a specific environment. If omitted, the default environment is used.

## Knowledge Source Managed Sub-Resources (CRITICAL)

Knowledge sources are a special resource type in Azure AI Search's agentic retrieval feature. When Azure creates a knowledge source, it **auto-provisions** four managed sub-resources:

- **Index** — the search index for the knowledge source's data
- **Indexer** — the indexer that populates the index
- **Data source** — the data source connection
- **Skillset** — the AI enrichment pipeline

These are listed in the knowledge source's `createdResources` field.

### Key rules

1. **Do NOT push managed sub-resources separately.** They are owned by the knowledge source. Pushing them via `--indexes` or `--indexers` will skip managed resources automatically.

2. **Cascade push order.** When pushing a knowledge source, rigg pushes in dependency order: Knowledge Source -> Index -> Skillset -> Data Source -> Indexer.

3. **Known Azure update bug.** Azure sometimes fails to update knowledge sources in-place. When this happens, rigg detects the failure and offers to **delete and recreate** the knowledge source (which also recreates all managed sub-resources). This is safe — the data will be re-indexed automatically.

4. **Pull routes managed resources to KS subdirectories.** Managed resources are stored under `knowledge-sources/<ks-name>/` rather than in the top-level type directories.

5. **Standalone vs managed.** Resources not owned by a knowledge source remain in their top-level directories and can be pushed/pulled independently.

## rigg.yaml Configuration Format

```yaml
project:
  name: My RAG System        # Project name (descriptive)

sync:
  include_preview: true       # Include preview API resources (knowledge bases, knowledge sources)

environments:
  prod:
    default: true             # Mark as default environment
    search:
      - name: search-prod                                    # Azure AI Search service name
        subscription: "11111111-1111-1111-1111-111111111111"  # Azure subscription ID
    foundry:
      - name: ai-services-prod        # Foundry AI Services account name
        project: my-project            # Foundry project name

  staging:
    search:
      - name: search-staging
        subscription: "22222222-2222-2222-2222-222222222222"
    foundry:
      - name: ai-services-staging
        project: my-project-staging
```

### Configuration keys

- `project.name` — Project display name
- `sync.include_preview` — Whether to include preview API resources in sync operations
- `environments.<name>.default` — Whether this is the default environment
- `environments.<name>.search` — List of Azure AI Search service configs
- `environments.<name>.foundry` — List of Microsoft Foundry service configs

## MCP Tools Reference

The rigg MCP server exposes 9 tools. Start it with `rigg mcp serve` or install via `rigg mcp install claude-code`.

### rigg_status

Returns project status including auth state, environment info, and resource counts.

- **Parameters:** none
- **Returns:** JSON with project status

### rigg_describe

Returns a full project description with all resources, their dependencies, file paths, and agent configurations.

- **Parameters:** none
- **Returns:** JSON with complete project description

### rigg_env_list

Lists all configured environments.

- **Parameters:** none
- **Returns:** JSON list of environments with their service configs

### rigg_validate

Validates local resource files for structural and referential integrity.

- **Parameters:** none
- **Returns:** Validation results (errors and warnings)

### rigg_list

Lists resource names by type.

- **Parameters:**
  - `source` (optional): `"local"`, `"remote"`, or `"both"` (default: `"local"`)
  - `resource_type` (optional): filter by resource type (e.g., `"indexes"`, `"agents"`)
- **Returns:** JSON list of resource names

### rigg_diff

Compares local resource files against the live Azure service.

- **Parameters:**
  - `resource_type` (optional): filter by type
  - `name` (optional): diff a single resource by name
- **Returns:** JSON diff output

### rigg_pull

Downloads resource definitions from Azure. Without `force`, returns a preview of what would be pulled. With `force: true`, executes the pull.

- **Parameters:**
  - `resource_type` (optional): filter by type
  - `name` (optional): pull a single resource by name
  - `force` (optional, boolean): execute without confirmation (default: false)
- **Returns:** Preview or execution result

### rigg_push

Uploads local resource files to Azure. Without `force`, returns a preview of what would be pushed. With `force: true`, executes the push.

- **Parameters:**
  - `resource_type` (optional): filter by type
  - `name` (optional): push a single resource by name
  - `force` (optional, boolean): execute without confirmation (default: false)
- **Returns:** Preview or execution result

### rigg_delete

Deletes a resource from Azure or removes local files. Without `force`, returns a preview. With `force: true`, executes the deletion.

- **Parameters:**
  - `resource_type` (required): the resource type
  - `name` (required): the resource name
  - `target` (required): `"remote"` (Azure) or `"local"` (files)
  - `force` (optional, boolean): execute without confirmation (default: false)
- **Returns:** Preview or execution result

### Force flag pattern

All mutating tools (`rigg_pull`, `rigg_push`, `rigg_delete`) follow the same pattern:
- **Without `force`:** Returns a preview of what would happen (dry run).
- **With `force: true`:** Executes the operation.

This two-step pattern allows AI agents to show previews to the user before committing changes.

## Safety Rules

1. **Always validate before push:** Run `rigg validate` (or `rigg_validate` MCP tool) before pushing to catch structural errors.
2. **Always diff before push:** Run `rigg diff` (or `rigg_diff` MCP tool) to review changes before pushing.
3. **Pull before push for conflict detection:** If you haven't pulled recently, pull first so rigg can detect if the remote has changed since your last sync.
4. **Use `--force` only in CI/CD or after previewing:** In interactive sessions, let rigg show the preview and ask for confirmation.
5. **Never push managed sub-resources directly:** They are owned by their parent knowledge source.

## Delete Command Semantics

```bash
# Delete from Azure (local files are kept)
rigg delete --index my-index --target remote

# Delete local files (Azure resource is kept)
rigg delete --index my-index --target local
```

- The `--target` flag is **required** — there is no default.
- `--target remote` deletes from Azure only. Local files remain.
- `--target local` removes local files only. The Azure resource remains.
- Knowledge source deletion (`--target remote`) removes the KS and all its managed sub-resources.
- Use `--force` to skip the confirmation prompt.

## New Command (Scaffold Templates)

The `rigg new` command creates resource files from templates without making any network calls.

```bash
# Create a new index (with optional vector/semantic search)
rigg new index my-index --vector --semantic

# Create a new data source
rigg new datasource my-ds --type azureblob --container documents

# Create a new indexer
rigg new indexer my-indexer --datasource my-ds --index my-index --skillset my-skillset

# Create a new skillset
rigg new skillset my-skillset

# Create a new synonym map
rigg new synonym-map my-synonyms

# Create a new alias
rigg new alias my-alias --index my-index

# Create a new knowledge base
rigg new knowledge-base my-kb

# Create a new knowledge source
rigg new knowledge-source my-ks --index my-index

# Create a new Foundry agent
rigg new agent my-agent --model gpt-4o

# Scaffold a complete Agentic RAG system
rigg new agentic-rag my-system --model gpt-4o --datasource-type azureBlob --container documents
```

The `agentic-rag` scaffold creates a complete system: agent + knowledge base + knowledge source with consistent naming (`<name>`, `<name>-kb`, `<name>-ks`).
