# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

`hoist` is a configuration-as-code CLI tool for Azure AI Search and Microsoft Foundry. It pulls/pushes resource definitions (indexes, indexers, skillsets, knowledge bases, Foundry agents, etc.) as normalized JSON files, enabling Git-based versioning of search and AI service configuration.

## Build & Test Commands

```bash
cargo build                          # Build all crates
cargo test                           # Run all tests (403 tests across 4 crates)
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

**hoist-core** — Resource type system (`ResourceKind`, `ServiceDomain`), config (`hoist.toml`), state tracking (`.hoist/`), JSON normalization, constraints (immutability/dependency validation), agent file decomposition.

**hoist-client** — Azure Search REST API client (`client.rs`), Microsoft Foundry client (`foundry.rs`), Azure Resource Manager discovery (`arm.rs`), authentication via Azure CLI or service principal (`auth.rs`) with per-domain scoping.

**hoist-diff** — Semantic JSON diffing with identity-key-based array matching. Standalone, no Azure dependencies.

**hoist-az** — Clap-based CLI. Each command in `commands/` follows the pattern: load config → create client → perform operation → update state.

## Resource Type System

`ResourceKind` enum (in `resources/traits.rs`) is central to everything. Each resource type has:
- A `ServiceDomain` — `Search` or `Foundry`
- An API path (e.g., `indexes`, `assistants`)
- A directory path under the user's project (e.g., `search-management/indexes`, `agents`)
- Stable vs preview classification — preview resources (KnowledgeBase, KnowledgeSource) use `preview_api_version` (`2025-11-01-preview`); stable resources use `api_version` (`2024-07-01`)

The `Resource` trait on each struct defines volatile fields (stripped during normalization), dependencies (for push ordering and validation), and immutable fields (for change detection).

### Foundry Agents

Foundry agents are decomposed into human-friendly files per agent directory:

```
foundry-resources/<service>/<project>/agents/<agent-name>/
  config.json        # id, name, model, temperature, metadata
  instructions.md    # Agent instructions as Markdown
  tools.json         # Tools array (code_interpreter, azure_search, etc.)
  knowledge.json     # tool_resources object
```

The `compose_agent()` / `decompose_agent()` functions in `resources/agent.rs` handle reassembling/splitting the API payload.

## Directory Layout on Disk

```
hoist.toml
.hoist/          # state.json, checksums.json (gitignored)
search-resources/
  <search-service>/
    search-management/
      indexes/  indexers/  data-sources/  skillsets/  synonym-maps/  aliases/
    agentic-retrieval/
      knowledge-bases/  knowledge-sources/
foundry-resources/
  <foundry-service>/
    <project>/
      agents/
        <agent-name>/
          config.json  instructions.md  tools.json  knowledge.json
```

Legacy projects using `[service]` config and a custom path (e.g., `search/`) continue to work unchanged.

## Configuration

Multi-service config (`hoist.toml`):

```toml
[project]
name = "My RAG System"

[[services.search]]
name = "my-search-service"
api_version = "2024-07-01"
preview_api_version = "2025-11-01-preview"

[[services.foundry]]
name = "my-ai-service"
project = "my-project"
api_version = "2025-05-15-preview"

[sync]
include_preview = true
generate_docs = true
```

Legacy `[service]` format auto-migrates to `services.search[0]` on load.

## Key Patterns

- **Checksum-based change detection**: Pull skips writing files when content hasn't changed, but always verifies the file exists on disk (stale checksums don't suppress re-writes).
- **JSON normalization**: Strips volatile fields (`@odata.etag`, `@odata.context`, credentials), preserves Azure's property ordering (via `serde_json` `preserve_order` feature), sorts arrays by identity key, redacts secrets.
- **Auth chain**: Environment variables (service principal) take priority, then Azure CLI. Auth is scoped per service domain (`search.azure.com` for Search, `ai.azure.com` for Foundry). ARM discovery uses a separate token scoped to `management.azure.com`.
- **Fallback behavior**: `init` tries ARM discovery first; falls back to manual service name entry if not logged in. `pull` without flags pulls all Search resource types respecting the `include_preview` config. Foundry agents require explicit `--agents` or `--all` flag.
- **CLI flags**: Resource type flags (`--indexes`, `--agents`, etc.) are defined once in `ResourceTypeFlags` struct and shared via `clap(flatten)` across pull, push, diff, and pull-watch commands.

## Test Projects

The `test-projects/` directory (gitignored) is available for manual testing of the `hoist` CLI. Use it to run `hoist init`, `hoist pull`, etc. against real or mock service configurations without polluting the repo. Create subdirectories per test scenario as needed.

## Releasing

Releases are automated via `.github/workflows/release.yml`. To publish a new version:

1. Bump `version` in the workspace `Cargo.toml` (all crates share it via `version.workspace = true`)
2. Update the internal crate dependency versions (`hoist-core`, `hoist-client`, `hoist-diff`) to match
3. Commit and push to `main`
4. Tag and push: `git tag v0.X.Y && git push origin v0.X.Y`

The workflow runs CI, builds release binaries for Linux/macOS/Windows, creates a GitHub Release, and publishes all four crates to crates.io in dependency order. Do NOT run `cargo publish` manually.
