# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

`hoist` is a configuration-as-code CLI tool for Azure AI Search and Microsoft Foundry. It pulls/pushes resource definitions (indexes, indexers, skillsets, knowledge bases, Foundry agents, etc.) as normalized JSON files, enabling Git-based versioning of search and AI service configuration.

## Build & Test Commands

```bash
cargo build                          # Build all crates
cargo test                           # Run all tests (522 tests across 4 crates)
cargo test -p hoist-core             # Test a specific crate
cargo test test_name                 # Run a single test by name
cargo clippy                         # Lint
cargo run --bin hoist -- pull        # Run CLI directly
cargo install --path crates/hoist-az # Install binary
```

## Pre-Push Verification (REQUIRED)

Before pushing code or declaring a task complete, you MUST run all CI checks locally and confirm they pass:

```bash
cargo fmt --all -- --check           # Formatting
cargo clippy --workspace -- -D warnings  # Lints (warnings are errors)
cargo test --workspace               # All tests
```

All three must exit cleanly. Do not push if any of them fail — fix the issues first. These are the same checks that run in GitHub Actions CI.

## Architecture

Four crates with a clear dependency hierarchy:

```
hoist-az  →  hoist-core
     ↓              ↑
hoist-client ───┘
hoist-diff  (standalone)
```

**hoist-core** — Resource type system (`ResourceKind`, `ServiceDomain`), config (`hoist.yaml`), environment resolution (`ResolvedEnvironment`), state tracking (`.hoist/<env>/`), JSON normalization, constraints (immutability/dependency validation).

**hoist-client** — Azure Search REST API client (`client.rs`), Microsoft Foundry client (`foundry.rs`), Azure Resource Manager discovery (`arm.rs`), authentication via Azure CLI or service principal (`auth.rs`) with per-domain scoping.

**hoist-diff** — Semantic JSON diffing with identity-key-based array matching. Standalone, no Azure dependencies.

**hoist-az** — Clap-based CLI. Each command in `commands/` follows the pattern: load config → resolve environment → create client → perform operation → update state.

## Resource Type System

`ResourceKind` enum (in `resources/traits.rs`) is central to everything. Each resource type has:
- A `ServiceDomain` — `Search` or `Foundry`
- An API path (e.g., `indexes`, `agents`)
- A categorized directory name (e.g., `search-management/indexes`, `agentic-retrieval/knowledge-sources`, `agents`)
- Stable vs preview classification — preview resources (KnowledgeBase, KnowledgeSource) use `preview_api_version` (`2025-11-01-preview`); stable resources use `api_version` (`2024-07-01`)

The `Resource` trait on each struct defines volatile fields (stripped during normalization), dependencies (for push ordering and validation), and immutable fields (for change detection).

### Foundry Agents

Foundry agents are stored as a single YAML file per agent, matching the Foundry portal's YAML view:

```
foundry/agents/<agent-name>.yaml
```

Agent name is derived from the filename (not stored in the YAML). The `agent_to_yaml()` / `yaml_to_agent()` functions in `resources/agent.rs` handle conversion between API JSON and on-disk YAML. The `wrap_agent_payload()` / `flatten_agent_response()` functions in `foundry.rs` handle API format conversion.

## Directory Layout on Disk

```
hoist.yaml
.hoist/
  <env>/state.json, checksums.json    # Per-environment state (gitignored)
search/                                # Single search service
  search-management/                   # Stable search resources
    indexes/  indexers/  data-sources/  skillsets/  synonym-maps/  aliases/
  agentic-retrieval/                   # Preview agentic retrieval resources
    knowledge-bases/                   # Flat JSON files
    knowledge-sources/                 # Each KS is a directory with managed sub-resources
      <ks-name>/
        <ks-name>.json                 # The KS definition
        <ks-name>-index.json           # Managed index (from createdResources)
        <ks-name>-indexer.json         # Managed indexer
        <ks-name>-datasource.json      # Managed data source
        <ks-name>-skillset.json        # Managed skillset
foundry/                               # Single foundry service
  agents/
    <agent-name>.yaml                  # Single YAML file per agent (matches portal format)
```

Multi-service layout (when environment has multiple services per domain, labels create subdirs):

```
search/
  primary/search-management/indexes/...
  analytics/search-management/indexes/...
foundry/
  rag/agents/...
  chat/agents/...
```

Knowledge source managed sub-resources (auto-provisioned by Azure via `createdResources`) are nested under their parent KS directory. Standalone resources (not managed by a KS) remain in top-level directories.

## Deployment Environments

Named environments are first-class config concepts. Config uses YAML (`hoist.yaml`):

```yaml
project:
  name: My RAG System

sync:
  include_preview: true

environments:
  prod:
    default: true
    search:
      - name: search-prod
        subscription: "11111111-1111-1111-1111-111111111111"
    foundry:
      - name: ai-services-prod
        project: my-project

  test:
    search:
      - name: search-test
    foundry:
      - name: ai-services-test
        project: my-project-test
```

### Environment resolution

- `--env <name>` flag (or `HOIST_ENV` env var) on any command to target a specific environment
- If omitted, uses the environment marked `default: true`, or the only environment if there's just one
- `ResolvedEnvironment` is the central abstraction all commands work through
- `Config::resolve_env(name)` resolves an environment by name or default
- `load_config_and_env()` in `commands/mod.rs` is the standard entry point

### Environment management

- `hoist env list` — list all environments
- `hoist env show [name]` — show environment details
- `hoist env set-default <name>` — set default environment

### Per-environment state

State files live in `.hoist/<env>/state.json` and `.hoist/<env>/checksums.json`. Methods: `LocalState::load_env()` / `save_env()`, `Checksums::load_env()` / `save_env()`.

## Key Patterns

- **Managed resources**: Knowledge sources auto-provision sub-resources (index, indexer, data source, skillset) listed in `createdResources`. The `managed.rs` module tracks ownership via `ManagedMap` (`HashMap<(ResourceKind, String), String>` mapping `(kind, azure_name)` to `ks_name`). Pull routes managed resources to KS subdirectories; push does cascade push (KS → Index → Skillset → DataSource → Indexer); diff reads from managed-aware paths; standalone flags (`--indexes`) skip managed resources.
- **Drop-and-recreate**: When pushing an index with removed fields (immutable in Azure), hoist detects `ViolationSeverity::RequiresRecreate` and offers to delete and recreate the resource.
- **Checksum-based change detection**: Pull skips writing files when content hasn't changed, but always verifies the file exists on disk (stale checksums don't suppress re-writes).
- **JSON normalization**: Strips volatile fields (`@odata.etag`, `@odata.context`, credentials), preserves Azure's property ordering (via `serde_json` `preserve_order` feature), sorts arrays by identity key, redacts secrets.
- **Auth chain**: Environment variables (service principal) take priority, then Azure CLI. Auth is scoped per service domain (`search.azure.com` for Search, `ai.azure.com` for Foundry). ARM discovery uses a separate token scoped to `management.azure.com`.
- **Fallback behavior**: `init` tries ARM discovery first; falls back to manual service name entry if not logged in. `pull` without flags pulls all Search resource types respecting the `include_preview` config. Foundry agents require explicit `--agents` or `--all` flag.
- **CLI flags**: Resource type flags (`--indexes`, `--agents`, etc.) are defined once in `ResourceTypeFlags` struct and shared via `clap(flatten)` across pull, push, diff, and pull-watch commands.
- **Client construction**: `AzureSearchClient::from_service_config(&SearchServiceConfig)` creates clients from resolved environment service configs. `FoundryClient::new(&FoundryServiceConfig)` for Foundry.
- **Push conflict detection**: Before pushing, compares the remote resource checksum against the stored pull baseline. If the server has changed since last pull, shows a warning listing conflicting resources. Uses the same `Checksums::calculate()` / volatile field normalization as pull.
- **Pull overwrite warning**: Before overwriting local files, compares disk content checksum against stored pull baseline. If local files were modified since last pull, warns before overwriting.
- **Delete command**: `hoist delete --<kind> <name> --target <remote|local>` operates on one target at a time. `--target remote` deletes from Azure only (local files untouched). `--target local` removes local files only (Azure untouched). The `--target` flag is required — no default. After deleting, use push/pull to sync. Knowledge source deletion (remote) removes the entire KS and its managed sub-resources. `DeleteResource` struct in `cli.rs` with `resolve()` method.

## Test Projects

The `test-projects/` directory (gitignored) is available for manual testing of the `hoist` CLI. Use it to run `hoist init`, `hoist pull`, etc. against real or mock service configurations without polluting the repo. Create subdirectories per test scenario as needed.

## Releasing

Releases are automated via `.github/workflows/release.yml`. To publish a new version:

1. Bump `version` in the workspace `Cargo.toml` (all crates share it via `version.workspace = true`)
2. Update the internal crate dependency versions (`hoist-core`, `hoist-client`, `hoist-diff`) to match
3. Commit and push to `main`
4. Tag and push: `git tag v0.X.Y && git push origin v0.X.Y`

The workflow runs CI, builds release binaries for Linux/macOS/Windows, creates a GitHub Release, publishes all four crates to crates.io in dependency order, and updates the Homebrew tap formula with new SHA256 hashes. Do NOT run `cargo publish` manually.

### Required secrets

- `CARGO_REGISTRY_TOKEN` — crates.io publish token (in the `crates-io` environment)
- `HOMEBREW_TAP_TOKEN` — GitHub PAT with `repo` scope for pushing to `mklab-se/homebrew-tap`. Without this, the Homebrew update step is silently skipped and the formula must be updated manually

## AI Agent Integration

hoist exposes an MCP (Model Context Protocol) server and agent skills for AI-assisted workflows.

### MCP Server (`hoist mcp serve`)

Starts a stdio-based MCP server with 9 tools. Any MCP-compatible client (Claude Code, VS Code Copilot, Claude Desktop) can call these tools directly.

| Tool | Description |
|------|-------------|
| `hoist_status` | Project status, auth state, resource counts |
| `hoist_describe` | Full project description with all resources, dependencies, agent configs |
| `hoist_env_list` | List configured environments |
| `hoist_validate` | Validate local resource files |
| `hoist_list` | List resource names by type (local/remote/both) |
| `hoist_diff` | Compare local vs remote (JSON diff) |
| `hoist_pull` | Pull from Azure (preview without `force`, execute with `force: true`) |
| `hoist_push` | Push to Azure (preview without `force`, execute with `force: true`) |
| `hoist_delete` | Delete from Azure (`target='remote'`) or remove local files (`target='local'`). Preview without `force`, execute with `force: true` |

**Code location:** `crates/hoist-az/src/mcp/` — `mod.rs` (server lifecycle, install commands), `tools.rs` (all 9 tool implementations).

**Auto-discovery:** `.mcp.json` in the repo root auto-registers the MCP server with Claude Code and VS Code when the project is opened.

**Manual install:** `hoist mcp install [claude-code|vs-code] [--scope workspace|global]` registers hoist as an MCP server. Defaults to workspace scope (project-level).

### Agent Skills (`.claude/skills/`)

Skills are cross-platform (Claude Code, GitHub Copilot, Codex, Cursor, Gemini CLI). They reference MCP tools for structured execution.

| Skill | Type | Description |
|-------|------|-------------|
| `hoist-guide` | Auto-loaded | Reference guide, loaded when hoist context is detected |
| `hoist-pull` | User-invoked (`/hoist-pull`) | Pull workflow with preview → confirm → execute |
| `hoist-push` | User-invoked (`/hoist-push`) | Safe push: validate → diff → confirm → push |
| `hoist-status` | User-invoked (`/hoist-status`) | Environment inspection |

### Key design decisions

- **`force` flag pattern:** Mutating MCP tools (pull/push) without `force` return a preview. With `force: true` they execute. Same semantics as CLI `--force`.
- **Subprocess isolation:** Tools that produce stdout (describe, validate, diff, pull, push) spawn a subprocess `hoist --output json` to avoid stdout contamination (MCP uses stdout for JSON-RPC).
- **`hoist_describe` precision:** JSON output includes `file_path` for every resource and full `instructions` for agents (not truncated). AI agents can `Read` any file path for complete content.
