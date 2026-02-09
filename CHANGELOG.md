# Changelog

All notable changes to this project will be documented in this file.

## [0.2.6] - 2026-02-09

### Changed

- **Preserve original array order in pulled JSON** — removed automatic sorting of JSON arrays by identity key during normalization. Pulled configuration files now preserve the exact element order returned by the Azure API, making it easier to compare local files with the portal. Volatile field stripping, credential redaction, and property order preservation are unchanged

### Tests

- 449 tests across workspace (up from 446)

## [0.2.5] - 2026-02-09

### Added

- **ASCII art banner** — `hoist init` and `hoist version` now display a bold block-letter HOIST logo
- **`hoist logo` easter egg** — hidden command that prints the banner

## [0.2.4] - 2026-02-09

### Added

- **Native TLS root certificates** — switched from bundled Mozilla CA roots to OS-native certificate stores (`rustls-tls-native-roots`). Fixes `UnknownIssuer` TLS errors on corporate networks using TLS inspection with custom CA certificates. Certificates are now read from macOS Keychain, Windows Certificate Store, or Linux system cert paths
- **Re-runnable `hoist init`** — running `hoist init` in an existing project now discovers and adds new services instead of bailing with "already initialized". Shows already-configured services, lists newly-discovered ones, and lets you select which to add. Existing configuration is preserved
- **Multi-select during init** — `hoist init` now supports selecting multiple search services and Foundry projects at once (comma-separated numbers)
- **Foundry endpoint refresh** — re-running `hoist init` refreshes endpoint URLs for existing Foundry configs using current ARM data

### Tests

- 446 tests across workspace (up from 439)

### Improved

- **TLS error diagnostics** — certificate verification failures now show a specific message explaining the likely cause (corporate TLS inspection) with OS-specific fix instructions for macOS, Linux, and Windows

## [0.2.3] - 2026-02-09

### Fixed

- **Foundry endpoint discovery from ARM** — `hoist init` now reads the actual endpoint URL from the AI Services account's ARM properties instead of constructing it from the resource name. This fixes connection failures when the account's custom subdomain differs from its resource name. The discovered endpoint is stored in `hoist.toml` as `endpoint` under `[[services.foundry]]`

### Improved

- **Connection error diagnostics** — HTTP connection failures now explain the likely cause (DNS resolution, private endpoint/VNet, firewall) and suggest re-running `hoist init` to rediscover the endpoint. The full error source chain is written to `hoist-error.log`

### Tests

- 439 tests across workspace (up from 434)

## [0.2.2] - 2026-02-09

### Improved

- **Better 403 troubleshooting guidance** — error message now explains the three most common causes: RBAC not enabled on the data plane (with the exact portal and CLI steps to enable it), missing role assignments (now recommends both Search Service Contributor and Search Index Data Contributor), and IP firewall restrictions

## [0.2.1] - 2026-02-09

### Improved

- **Rich 403 error handling** — access-denied errors now show the service name, a clear explanation, the exact `az role assignment create` command to fix it, common RBAC role names, and a link to Microsoft's RBAC documentation. Previously displayed as an empty `Error: API error (403):` with no guidance
- **Error log file** — when a client error occurs, detailed diagnostics (timestamp, response body, suggestion) are appended to `hoist-error.log` instead of flooding the terminal
- **Empty error body fallback** — API errors with no response body now show `HTTP <status> with no error details` instead of a blank message

### Tests

- 434 tests across workspace (up from 426)

## [0.2.0] - 2026-02-08

### Added

- **Microsoft Foundry support** — manage Foundry agent configurations alongside search resources in a single Git repository. Pull/push agent definitions including instructions, tools, and knowledge configurations
- **Multi-service configuration** — new `[[services.search]]` and `[[services.foundry]]` config format supports multiple services. Legacy `[service]` format auto-migrates on load
- **Symmetric init flow** — `hoist init` now discovers both Azure AI Search services and Microsoft Foundry projects via ARM APIs. Auto-selects when there's only one option. Either service type is optional — you can use hoist for Search only, Foundry only, or both together
- **ARM discovery for Foundry** — `hoist init` lists AI Services accounts and Microsoft Foundry projects from Azure subscriptions, matching the existing Search service discovery
- **Agent file decomposition** — Foundry agents are stored as human-friendly decomposed files: `config.json`, `instructions.md` (editable Markdown), `tools.json`, and `knowledge.json`
- **`--agents` / `--agent <NAME>` flags** — pull, push, diff, and pull-watch commands support Foundry agent resources
- **`--search-only` / `--foundry-only` flags** — scope operations to a single service domain
- **`ServiceDomain` enum** — internal architecture for routing operations to Search vs Foundry APIs
- **`FoundryClient`** — new REST API client for Microsoft Foundry project-scoped `/agents` endpoint (API version `2025-05-15-preview`)
- **Agent sections in `hoist status` and `hoist describe`** — shows Foundry service info, agent counts, model, tool count, and instruction previews
- **Agentic RAG Flows in `hoist describe`** — traces the full dependency chain from agent through knowledge base to knowledge source to index, showing descriptions and retrieval instructions at each level
- **Agent tools parsing** — `hoist describe` extracts knowledge base connections from MCP tool definitions in agent `tools.json`
- **Foundry agent push diff check** — `hoist push --agents` now compares local vs remote agent definitions and only pushes agents that have actually changed, matching how search resources work
- **Foundry API payload wrapping** — agent create/update uses the correct `{"definition": {...}}` wrapper format and `/agents/{name}/versions` endpoint

### Changed

- `hoist init` no longer requires Azure AI Search — at least one of Search or Foundry must be selected
- CLI description updated to reflect dual-service support: "Configuration-as-code for Azure AI Search and Microsoft Foundry"
- CLI resource type flags deduplicated via `clap(flatten)` — eliminates ~200 lines of repeated flag definitions across pull, push, diff, and pull-watch commands
- `ResourceKind` enum extended with `Agent` variant (9 total kinds)
- Authentication refactored to support multiple resource scopes (`search.azure.com` vs `ai.azure.com`)

### Tests

- 426 tests across workspace (up from 348)

## [0.1.7] - 2026-02-08

### Fixed

- Alias resource now correctly uses the preview API version — the aliases endpoint only exists in preview, so requesting it with the stable `2024-07-01` version caused `hoist pull` to fail with "api-version does not exist"
- `--aliases` flag now respects `include_preview` setting, consistent with other preview resource flags

### Changed

- Added `test-projects/` to `.gitignore` for local manual testing

## [0.1.6] - 2026-02-08

### Fixed

- Shell completion command no longer prints install instructions to stderr on every invocation, which cluttered terminal startup when sourced from `.zshrc`

## [0.1.5] - 2026-02-08

### Added

- **`hoist describe` command** — unified summary of all local resource definitions with text and JSON output, including field schemas, indexer wiring, skillset pipelines, and cross-resource dependency graph
- **SEARCH_CONFIG.md generation** — auto-generated markdown overview of the entire search configuration after `pull --all` when `sync.generate_docs = true`
- **Index alias support** — new `Alias` resource type for zero-downtime index swapping, with `--aliases`/`--alias` flags on pull, push, diff, and pull-watch
- **Parallel API calls** — concurrent resource fetching in pull (max 5 in-flight requests) for faster operations on large services
- **Retry logic** — automatic exponential backoff retry for transient Azure API errors (429 rate limiting, 503 service unavailable), max 3 retries
- **Resource linting** — `hoist validate` now warns about common misconfigurations: missing key fields, indexers without schedules, indexes with >50 fields, empty container names
- **`--output json` for status and validate** — machine-readable structured output for CI/CD and AI agent consumption

### Fixed

- README configuration example now matches actual code (`[project].path` not `[sync].resource_dir`, `[service]` not `[api]`)
- `hoist validate` now checks preview resources (KnowledgeBase, KnowledgeSource) when `include_preview = true`

### Tests

- Added 114 new tests (234 -> 348 total): auth (10), validate (24), init (11), templates (16), skillset (6), synonym map (5), normalize (10), alias (6), resource traits (12), describe (19), retry logic (8)

## [0.1.4] - 2026-02-04

### Added

- Background update notification — checks for newer versions and notifies the user
- Document release process in CLAUDE.md

### Changed

- Prioritize `cargo install` over Homebrew in Quick Start documentation
- Move `cargo-binstall` to less prominent position in install docs

## [0.1.3] - 2026-01-31

### Added

- Multiple installation methods: Homebrew, cargo-binstall, cargo install
- Shell completions documentation (bash, zsh, fish, PowerShell)
- Dedicated INSTALL.md with comprehensive installation guide

## [0.1.2] - 2026-01-28

### Added

- Sub-crate README files for crates.io listings
- Badges to main README (crates.io, docs.rs, CI, license)

## [0.1.1] - 2026-01-25

### Added

- README and logo for crates.io listing

## [0.1.0] - 2026-01-22

### Added

- Initial release
- Pull/push Azure AI Search resources as normalized JSON files
- Support for indexes, indexers, data sources, skillsets, synonym maps
- Preview API support for knowledge bases and knowledge sources
- Semantic JSON diffing with `hoist diff`
- Git-based version control workflow for search configuration
- Azure CLI and service principal authentication
- ARM-based service discovery during `hoist init`
- Checksum-based change detection to minimize unnecessary writes
- Cross-platform release binaries (Linux, macOS, Windows)
- Published to crates.io as four crates: hoist-az, hoist-core, hoist-client, hoist-diff
