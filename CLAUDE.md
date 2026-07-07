# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

`rigg` is a configuration-as-code CLI for Azure AI Search and Microsoft Foundry. A **workspace** (`rigg.yaml`) holds environments and service connections; **projects** (`projects/<name>/`) own resource definitions as JSON files — indexes, indexers, data sources, skillsets, synonym maps, aliases, knowledge sources, knowledge bases (Search), agents, model deployments, connections, guardrails (Foundry). Pull/push/diff operate on whole projects, enabling Git-based versioning of the entire Agentic RAG stack.

The 1.0 design spec lives at `docs/superpowers/specs/2026-07-07-rigg-1.0-redesign-design.md`. Phases: 0.18 (core re-architecture — done), 0.19 (auth doctor, ci init, api watchdog), 0.20 (OpenAPI spec validation, AI features), 1.0.0 (samples, e2e, docs).

## Session start

Run `rigg dev api-check` (or ask the api-watchdog skill) to verify rigg's pinned Azure API versions are still current. Supported versions are constants in `crates/rigg-core/src/registry.rs`.

## Build & Test Commands

```bash
cargo build                          # Build all crates
cargo test                           # Run all tests
cargo test -p rigg-core              # Test a specific crate
cargo test -p rigg --test sync       # Sync-engine tests (wiremock, no Azure)
cargo clippy                         # Lint
cargo run --bin rigg -- status       # Run CLI directly
```

## Pre-Push Verification (REQUIRED)

Before pushing code or declaring a task complete, you MUST run all CI checks locally and confirm they pass:

```bash
cargo fmt --all -- --check                             # Formatting
cargo clippy --workspace --all-targets -- -D warnings  # Lints (warnings are errors)
cargo test --workspace                                 # All tests
```

## Architecture

Four crates:

```
rigg  →  rigg-core
     ↓          ↑
rigg-client ───┘
rigg-diff  (used by rigg-core & rigg)
```

**rigg-core** — the model:
- `registry.rs` — THE central declarative table: per-kind API paths, api-version channel, volatile/read-only/secret fields, reference extractors, data-source type validity. Updating rigg for a new Azure API version mostly means editing this file.
- `workspace.rs` — `rigg.yaml` + `project.yaml` model, environment resolution (flag > `RIGG_ENV` > `default: true`).
- `store.rs` — project file store (read/write/list with sidecar handling), exclusive-ownership check, `ProjectState` baselines (`.rigg/<env>/<project>/state.json`), `SyncClass` classification (InSync/LocalAhead/RemoteAhead/Conflict/…). Checksums are order-canonical and null-insensitive.
- `normalize.rs` — `normalize_for_disk` (strip volatile+read-only), `normalize_for_push` (also strip `x-rigg-*`), `semantic_eq`.
- `graph.rs` — reference-graph push/delete ordering (Kahn's algorithm over registry-extracted references).
- `sidecar.rs` — `{"$file": "x.md"}` inline/extract for long text fields.
- `scaffold.rs` — identity-first starter definitions for all 12 kinds, `scaffold_pipeline`, `scaffold_api_spec` (WebApiSkill contract).

**rigg-client** — Azure REST:
- `client.rs` — Search data plane; api-version per registry channel (stable `2026-04-01`, preview `2026-05-01-preview`).
- `foundry.rs` — Foundry v1 data plane (`https://{account}.services.ai.azure.com/api/projects/{project}?api-version=v1`), agents + versions, `Foundry-Features` header support.
- `arm_resources.rs` — generic ARM CRUD for deployments/connections/RAI policies (api-version `2026-05-01`) with LRO polling; `arm.rs` — typed ARM discovery.
- `auth.rs` — chain: `RIGG_ACCESS_TOKEN` static > service-principal env vars > Azure CLI; per-domain token scoping.

**rigg** — clap CLI. `commands/mod.rs` holds `GlobalContext`, exit codes (0/1/2/3/4/5), workspace loading, project selection. `commands/remote.rs` is the façade over the three clients used by all sync commands.

## Key invariants

- **A resource belongs to exactly one project** — validated everywhere.
- **Local files never contain secrets** — `validate` rejects key material; scaffolds use `ResourceId=` / `ProjectManagedIdentity`.
- **Push canonicalization** — after every successful push, the server document is GET'd back, normalized, and written to disk + baseline. Never skip this; it is what kills false-positive drift.
- **`x-rigg-*` keys are rigg-local** — kept on disk, stripped before any PUT/POST.
- **Deletes are explicit** — remote deletion requires `--prune` (orphans) or `rigg delete <project> --remote`.

## Workspace layout on disk

```
rigg.yaml                     # workspace: environments, connections (YAML)
apis/<name>.json              # shared OpenAPI specs (WebApiSkill contract)
projects/<name>/
  project.yaml                # metadata only; directory contents = membership
  search/{data-sources,indexes,skillsets,indexers,synonym-maps,aliases,
          knowledge-sources,knowledge-bases}/<name>.json
  foundry/{agents,deployments,connections,guardrails}/<name>.json
  foundry/agents/<name>.instructions.md   # $file sidecar
.rigg/<env>/<project>/state.json          # baselines, gitignored
```

## Testing patterns

- Unit tests inline per module (registry, graph, store, normalize, sidecar, workspace).
- `crates/rigg/tests/cli_surface.rs` — assert_cmd against temp workspaces, no network.
- `crates/rigg/tests/sync.rs` — wiremock fake Azure via `endpoint:` override + `RIGG_ACCESS_TOKEN`; covers pull normalization, push ordering/canonicalization, prune, conflicts (exit 5), diff formats, status classification.
- Live testing uses `mklabsrch` (Search) and `mklabaifndr`/`proj-default` (Foundry) — create resources inside them freely, always delete afterwards, keep SKUs/capacity minimal.

## Releasing

Releases are automated via `.github/workflows/release.yml`:

1. Bump `version` in the workspace `Cargo.toml` (incl. the internal `rigg-core`/`rigg-client`/`rigg-diff` dependency versions)
2. Update CHANGELOG.md
3. Commit and push to `main`
4. Tag and push: `git tag v0.X.Y && git push origin v0.X.Y`

Required secrets: `CARGO_REGISTRY_TOKEN` (crates.io env), `HOMEBREW_TAP_TOKEN`.

## AI Agent Integration

- MCP server: `rigg mcp serve` — 8 project-scoped stdio tools (`rigg_status`, `rigg_describe`, `rigg_env_list`, `rigg_validate`, `rigg_diff`, `rigg_pull`, `rigg_push`, `rigg_delete`). Mutating tools use the preview/`force: true` pattern. Tools shell out to `rigg --output json` subprocesses (stdout stays JSON-RPC clean).
- Skills in `.claude/skills/` (`rigg-guide` + slash commands) — being rewritten for the project model in the 1.0 phase.
- `rigg ai …` manages ailloy-powered features (explanations; conflict merge/NL scaffolding land in 0.20). `rigg new <kind> <name> --describe "…"` drafts definitions via AI when ailloy is enabled.
