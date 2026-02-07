# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

`hoist` is a configuration-as-code CLI tool for Azure AI Search. It pulls/pushes resource definitions (indexes, indexers, skillsets, knowledge bases, etc.) as normalized JSON files, enabling Git-based versioning of search service configuration.

## Build & Test Commands

```bash
cargo build                          # Build all crates
cargo test                           # Run all tests (226 tests across 4 crates)
cargo test -p hoist-core          # Test a specific crate
cargo test test_name                 # Run a single test by name
cargo clippy                         # Lint
cargo run --bin hoist -- pull     # Run CLI directly
cargo install --path crates/hoist-az   # Install binary
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
hoist-azent ────┘
hoist-diff  (standalone)
```

**hoist-core** — Resource type system, config (`hoist.toml`), state tracking (`.hoist/`), JSON normalization, constraints (immutability/dependency validation).

**hoist-azent** — Azure Search REST API client (`client.rs`), Azure Resource Manager discovery (`arm.rs`), authentication via Azure CLI or service principal (`auth.rs`).

**hoist-diff** — Semantic JSON diffing with identity-key-based array matching. Standalone, no Azure dependencies.

**hoist-az** — Clap-based CLI. Each command in `commands/` follows the pattern: load config → create client → perform operation → update state.

## Resource Type System

`ResourceKind` enum (in `resources/traits.rs`) is central to everything. Each resource type has:
- An API path (e.g., `indexes`, `knowledgebases`)
- A directory path under the user's project (e.g., `search-management/indexes`, `agentic-retrieval/knowledge-bases`)
- Stable vs preview classification — preview resources (KnowledgeBase, KnowledgeSource) use `preview_api_version` (`2025-11-01-preview`); stable resources use `api_version` (`2024-07-01`)

The `Resource` trait on each struct defines volatile fields (stripped during normalization), dependencies (for push ordering and validation), and immutable fields (for change detection).

## Directory Layout on Disk

When a user runs `hoist init . --folder search`, the structure is:

```
hoist.toml
.hoist/          # state.json, checksums.json (gitignored)
search/
  search-management/
    indexes/
    indexers/
    data-sources/
    skillsets/
    synonym-maps/
  agentic-retrieval/
    knowledge-bases/
    knowledge-sources/
```

## Key Patterns

- **Checksum-based change detection**: Pull skips writing files when content hasn't changed, but always verifies the file exists on disk (stale checksums don't suppress re-writes).
- **JSON normalization**: Strips volatile fields (`@odata.etag`, `@odata.context`, credentials), preserves Azure's property ordering (via `serde_json` `preserve_order` feature), sorts arrays by identity key, redacts secrets. Property order is enforced naturally: next `pull` restores Azure's canonical ordering if a user reorders keys locally.
- **Auth chain**: Environment variables (service principal) take priority, then Azure CLI. ARM discovery for `init` uses a separate token scoped to `management.azure.com`.
- **Fallback behavior**: `init` tries ARM discovery first; falls back to manual service name entry if not logged in. `pull` without flags pulls all resource types respecting the `include_preview` config.

## Releasing

Releases are automated via `.github/workflows/release.yml`. To publish a new version:

1. Bump `version` in the workspace `Cargo.toml` (all crates share it via `version.workspace = true`)
2. Update the internal crate dependency versions (`hoist-core`, `hoist-client`, `hoist-diff`) to match
3. Commit and push to `main`
4. Tag and push: `git tag v0.X.Y && git push origin v0.X.Y`

The workflow runs CI, builds release binaries for Linux/macOS/Windows, creates a GitHub Release, and publishes all four crates to crates.io in dependency order. Do NOT run `cargo publish` manually.
