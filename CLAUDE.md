# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

`rigg` is a configuration-as-code CLI for Azure AI Search and Microsoft Foundry. A **workspace** (`rigg.yaml`) holds environments — each with its targets, dependencies and policy; **projects** (`projects/<name>/`) own resource definitions as JSON files — indexes, indexers, data sources, skillsets, synonym maps, aliases, knowledge sources, knowledge bases (Search), agents, model deployments, connections, guardrails (Foundry). Pull/push/diff operate on whole projects, enabling Git-based versioning of the entire Agentic RAG stack.

The 1.0 design spec lives at `docs/superpowers/specs/2026-07-07-rigg-1.0-redesign-design.md`. Phases: 0.18 (core re-architecture — done), 0.19 (auth doctor, ci init, api watchdog), 0.20 (OpenAPI spec validation, AI features), 1.0.0 (samples, e2e, docs).

## Session start

Run `rigg dev api-check` (or ask the api-watchdog skill) to verify rigg's pinned Azure API versions are still current. Every API version is in the registry provider table (`providers()` in `crates/rigg-core/src/registry.rs`).

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
- `schema.rs` — pinned-version OpenAPI schema fixtures; `unknown_top_level_fields` is the pull/adopt API-drift canary.

**rigg-client** — Azure REST:
- `client.rs` — Search data plane; api-version per registry channel (stable `2026-04-01`, preview `2026-08-01-preview`).
- `foundry.rs` — Foundry v1 data plane (`https://{account}.services.ai.azure.com/api/projects/{project}?api-version=v1`), agents + versions, `Foundry-Features` header support.
- `arm_resources.rs` — generic ARM CRUD for deployments/connections/RAI policies (api-version `2026-05-01`) with LRO polling; `arm.rs` — typed ARM discovery.
- `auth.rs` — chain: `RIGG_ACCESS_TOKEN` static > service-principal env vars > Azure CLI; per-domain token scoping.

**rigg** — clap CLI. `commands/mod.rs` holds `GlobalContext`, exit codes (0/1/2/3/4/5), workspace loading, project selection. `commands/remote.rs` is the façade over the three clients used by all sync commands. `commands/dev.rs` — `rigg dev api-check`, the watchdog that verifies pinned Azure API versions are current; `commands/dev_spec.rs` — `rigg dev api-diff` / `api-fixture`, fetching and diffing OpenAPI documents from azure-rest-api-specs.

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
  envs/<env>/
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

## Keeping docs current

Three pieces of user documentation are printed from the binary, never hand-written; a change to the CLI surface, the registry's `InfraRef` table, or an MCP tool means regenerating them:

```bash
cargo run -q --bin rigg -- dev cli-reference > docs/reference/cli.md
cargo run -q --bin rigg -- dev infra-table          # → docs/reference/resource-files.md
cargo run -q --bin rigg -- mcp tools --markdown     # → MCP.md
```

The last two replace the text between `<!-- generated:<name>:start -->` / `<!-- generated:<name>:end -->` markers (`infra-table`, `mcp-tools`); never edit inside the markers by hand.

`crates/rigg/tests/docs_guards.rs` fails when any of the three drifts, and drives the fourth guard:

```bash
cargo run -q --bin rigg -- dev docs-check           # defaults to the current directory
```

`rigg dev docs-check` walks `README.md`, `GETTING_STARTED.md`, `CONCEPTS.md`, `MCP.md`, `docs/**` (minus `docs/superpowers/`, which is plans and specs), `samples/**` and `.claude/skills/*/SKILL.md`, then: parses every `rigg …` line in a `bash`/`sh`/`shell`/untagged fence through clap, resolves every relative link and `#anchor`, and — once those pages exist — checks that every `RIGG_*`/`AZURE_*` variable the code reads appears in `docs/reference/environment-variables.md` and every `ask::KNOWN_ID_PREFIXES` entry in `docs/reference/exit-codes-and-questions.md`. A block that shows command *output* rather than input must be tagged (```` ```text ````) so it is not read as a command. Fix the docs, never the parser.

## Releasing

Releases are driven by the `/release` skill (`.claude/skills/release/SKILL.md`, run with `major`/`minor`/`patch`): it runs `cargo update` and the pre-flight gates, bumps `version` in the workspace `Cargo.toml` (incl. the internal `rigg-core`/`rigg-client`/`rigg-diff` dependency versions), dates the `[Unreleased]` changelog section, commits `Release vX.Y.Z`, pushes, and tags `vX.Y.Z`.

Pushing the tag triggers `.github/workflows/release.yml`: it re-runs CI, builds [auditable](https://github.com/rust-secure-code/cargo-auditable) binaries for Linux/macOS/Windows with a CycloneDX 1.5 SBOM per target (`rigg-vX.Y.Z-<target>.cdx.json`), creates the GitHub Release, publishes `rigg-diff` → `rigg-core` → `rigg-client` → `rigg` to crates.io, and updates the Homebrew formula in `mklab-se/homebrew-tap`.

Required secrets: `CARGO_REGISTRY_TOKEN` (crates.io env), `HOMEBREW_TAP_TOKEN`.

MSRV is `rust-version = "1.88"` in the workspace `Cargo.toml` (set by Ailloy 2.x / `rmcp` 3.x); CI runs latest stable. `reqwest` stays on 0.12 to share Ailloy's TLS stack.

## AI Agent Integration

- MCP server: `rigg mcp serve` — 14 stdio tools: 9 config-plane (`rigg_status`, `rigg_describe`, `rigg_env_list`, `rigg_validate`, `rigg_diff`, `rigg_pull`, `rigg_push`, `rigg_promote`, `rigg_delete`) + 5 runtime (`rigg_indexer_run`, `rigg_indexer_status`, `rigg_query`, `rigg_ask`, `rigg_verify`). Mutating tools use the preview/`force: true` pattern. Tools shell out to `rigg --output json` subprocesses (stdout stays JSON-RPC clean).
- Skills in `.claude/skills/` (`rigg-guide` + slash commands) — being rewritten for the project model in the 1.0 phase.
- `rigg ai …` manages ailloy-powered features (explanations; conflict merge/NL scaffolding land in 0.20). `rigg new <kind> <name> --describe "…"` drafts definitions via AI when ailloy is enabled.
