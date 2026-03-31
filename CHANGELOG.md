# Changelog

All notable changes to this project will be documented in this file.

## [0.15.0] - 2026-03-20

### Changed

- **Replaced custom AI commands with ailloy config TUI** — `rigg ai config` now launches the interactive ailloy configuration UI instead of opening the config file in an editor. The `rigg ai init`, `rigg ai status`, and `rigg ai remove` subcommands are removed — ailloy handles all provider/model configuration through its built-in TUI. This eliminates ~160 lines of custom AI setup code
- **Added `rigg ai status` shortcut** — `rigg ai status` is now an alias that shows AI status (same as running `rigg ai` without a subcommand)
- **Added `rigg ai skill` subcommand** — generates AI agent skill files and reference documentation for rigg. `--emit` outputs a ready-to-save skill markdown file; `--reference` outputs full command reference. Without flags, shows a setup guide

## [0.14.0] - 2026-03-06

### Changed

- **Replaced custom AI providers with ailloy** — removed the `local_agent`, `ollama`, and `openai` modules from rigg-client in favor of the [`ailloy`](https://crates.io/crates/ailloy) crate for multi-provider AI support. This eliminates ~580 lines of custom provider code while maintaining the same functionality

## [0.13.1] - 2026-03-03

### Improved

- **AI explanation prompts** — rewrote system prompts for diff/push/pull AI explanations to be factual and proportional. Explanations now state what changed without speculating about purpose or impact. Obvious intent is stated plainly (e.g., "added Norwegian support") but editorializing is eliminated (no "this enhances...", "this improves..."). Single-line changes get 1-2 sentences; large restructurings get longer explanations. Unchanged parts of resources are no longer described

## [0.13.0] - 2026-03-03

### Fixed

- **Azure CLI auth scope bug** — `rigg ai status` reported "Not logged in" even after `az login` because the Cognitive Services scope included an invalid `/.default` suffix. Removed the suffix so `az account get-access-token --resource https://cognitiveservices.azure.com` works correctly
- **AADSTS error swallowed** — Azure AD token errors (AADSTS codes) were mapped to a generic "Not logged in" message. Now shows the actual error detail, the `az` debug command to run manually, and a fix suggestion (e.g., assign 'Cognitive Services User' role)

### Added

- **Multi-provider AI support** — `rigg ai init` now offers a choice of AI providers for diff explanations: Claude, Codex, Copilot, Ollama, and Azure OpenAI. Previously only Azure OpenAI was available
- **`AiProvider` enum** — new type in rigg-core with 5 variants, display names, descriptions, binary detection, and default model recommendations
- **Local CLI agent integration** — new `local_agent` module in rigg-client invokes claude/codex/copilot as subprocesses with timeout, output capture, and error handling
- **Ollama support** — new `ollama` module in rigg-client with HTTP API client for listing models and chat completions against locally running LLMs
- **Unified AI dispatcher** — new `ai::generate_text()` / `ai::generate_text_with_limit()` functions route to the correct backend based on the configured provider
- **`--provider` and `--model` flags** on `rigg ai init` for non-interactive setup
- **Provider-specific status** — `rigg ai status` shows provider name, model, and checks availability (binary on PATH for CLI agents, TCP connectivity for Ollama, token acquisition for Azure OpenAI)

### Changed

- **`AiConfig` fields** — `account` and `deployment` are now `Option<String>` (only needed for Azure OpenAI). Added `provider`, `model`, and `ollama_url` fields. Backward compatible: existing configs without `provider` default to Azure OpenAI
- **AI call sites use unified dispatcher** — `explain.rs`, `diff/ai.rs`, `push/explain.rs`, and `pull/output.rs` now route through `ai::generate_text()` instead of calling `AzureOpenAIClient` directly

### Tests

- 614 tests across workspace (up from 606)

## [0.12.0] - 2026-03-03

### Changed

- **Narrative AI explanations** — AI-powered diff explanations are now generated as a single cohesive narrative covering all changed resources, replacing the previous per-resource 2-3 sentence summaries. The AI receives full local and remote resource content (not just change paths) and frames its explanation based on the operation direction (push, pull, or diff). This produces much more useful explanations that describe what configurations actually *do* and how they differ, rather than listing which JSON fields changed
- **Richer AI prompts** — value summaries sent to the AI now include array item names and object keys (e.g., `[3 items: field1, field2, field3]` instead of `[3 items]`), and long strings include up to 2,000 characters of content. Simple scalar changes are filtered from raw details to reduce noise
- **Configurable token limit for AI completions** — the Azure OpenAI client now supports a custom `max_tokens` parameter via `chat_completion_with_limit()`, allowing narrative explanations to use up to 4,000 tokens while keeping per-resource summaries at 500

### Tests

- 606 tests across workspace (up from 595)

## [0.11.1] - 2026-02-25

### Changed

- **Improved readability of text diffs** — consecutive Modified pairs (side-by-side `-`/`+` comparisons) in long text diffs are now separated by blank lines, making it much easier to visually parse which old line corresponds to which new line. Applies to both colored terminal output and plain text output

## [0.11.0] - 2026-02-25

### Added

- **AI-enhanced explanations** — `rigg diff`, `rigg pull`, and `rigg push` now provide natural language explanations of configuration changes using Azure OpenAI. Instead of just showing structural diffs, rigg explains *what the changes mean* — e.g., "The agent's instructions were refined to emphasize a polite tone" instead of just "instructions differ (3 lines changed)"
- **`rigg ai init` command** — interactive setup for Azure OpenAI integration. Discovers AI Services accounts and model deployments via ARM, recommends gpt-4o-mini, and stores configuration in the `ai:` section of `rigg.yaml`
- **`rigg ai status` command** — check AI configuration and test connectivity to the Azure OpenAI endpoint
- **`rigg ai remove` command** — remove AI configuration from `rigg.yaml`
- **`AiConfig` in `rigg.yaml`** — new top-level `ai:` section for Azure OpenAI configuration (account, deployment, endpoint, subscription, resource group, api_version)
- **Azure OpenAI client** — new `AzureOpenAIClient` in rigg-client with `chat_completion()` for LLM calls via Cognitive Services auth scope
- **ARM discovery for model deployments** — `list_model_deployments()` and `create_model_deployment()` on the ARM client for Azure AI Services accounts
- **`explain` parameter on MCP `rigg_diff` tool** — AI agents can request AI-enhanced explanations by passing `explain: true` (opt-in for MCP, opt-out for CLI)
- **`--no-explain` flag** — disable AI explanations on `rigg diff`, `rigg pull`, and `rigg push` when AI is configured (AI explanations are on by default when the `ai:` config exists)

### Changed

- **AI explanations are opt-out on CLI** — when the `ai:` section is configured in `rigg.yaml`, diff/pull/push commands include AI explanations by default. Use `--no-explain` to disable. MCP tools remain opt-in (`explain: true`)
- **Concurrent LLM calls** — AI explanations for multiple resources are fetched concurrently using `futures::future::join_all` for faster results

### Tests

- 595 tests across workspace (up from 543)

## [0.10.1] - 2026-02-24

### Changed

- **Word-level highlighting for text diffs** — long text properties (like agent instructions) now highlight individual changed words within lines instead of showing entire lines as changed. Uses bold + color for changed words within context
- **Truncated context in text diffs** — large text diffs show only the relevant changed sections with `... (N lines unchanged) ...` markers instead of displaying the entire text
- **Visual separation for diff sections** — clearer visual distinction between structural JSON diffs and inline text diffs

## [0.10.0] - 2026-02-24

### Added

- **Line-level text diffs for long text properties** — properties with multi-line text (like agent instructions, skillset descriptions) now show line-by-line diffs instead of just "values differ (N lines)". Uses the `similar` crate for unified diff output with +/- markers, integrated into both text and JSON output formats

## [0.9.0] - 2026-02-23

### Added

- **Non-interactive `rigg init`** — `rigg init` can now be run fully non-interactively by passing `--search <name>` and/or `--foundry <name> --project <project>` flags, enabling CI/CD and scripted setups
- **Edm type validation** — `rigg validate` now checks index field `type` values against the set of valid Azure Search Edm types and warns about unrecognized types
- **E2E test skill** — comprehensive end-to-end test skill for validating the full rigg user experience against live Azure services

### Changed

- UX polish across status, validate, pull, push, and delete commands
- Improved error messages and progress feedback

## [0.8.0] - 2026-02-23

### Added

- **Human-readable diff descriptions** — `rigg diff` now produces clear English descriptions instead of cryptic symbols and arrows. Every change includes context-aware language: `"Index field 'language' is enabled on the server for filtering (was disabled locally)"` instead of `filterable: false → true`. Covers all resource types with 100+ specific description patterns for fields, skills, mappings, schedules, and more
- **Enhanced diff JSON format** — JSON diff output now includes `resource_type`, `resource_name`, `status` (`modified`/`local_only`/`remote_only`/`unchanged`), `summary`, and per-change `description` fields. AI agents get structured, self-describing output without needing to interpret raw JSON paths
- **Expressive pull/push previews** — pull and push previews now show action-oriented descriptions: `"Index 'products' has 3 differences — pulling will update your local file"` with per-change detail lines. Push previews warn about immutable field changes that require drop-and-recreate
- **`rigg_delete` MCP tool** — AI agents can now delete resources via MCP. Supports `target='remote'` (Azure only) and `target='local'` (local files only) with the same preview/force pattern as push and pull. 9 MCP tools total (up from 8)
- **`rigg mcp install --scope` flag** — workspace-level (`--scope workspace`, default) and global (`--scope global`) MCP installation. Workspace scope registers rigg for the current project only; global scope registers for all sessions. Works with both Claude Code and VS Code targets
- **Knowledge source creation fix** — creating a new knowledge source from scratch no longer fails. Previously, rigg pushed managed sub-resources (index, indexer, data source, skillset) before the KS, causing Azure to reject the KS creation with "resources already exist." Now detects new-vs-existing KS and skips sub-resource push for new ones (Azure auto-provisions them)
- **Knowledge source update guidance** — when a KS push fails due to the known Azure managed-resource recreation bug, rigg now detects the error pattern and provides specific workaround steps with environment name. Push previews proactively warn when updating existing knowledge sources

### Changed

- **Breaking: `rigg delete` requires `--target`** — the delete command no longer deletes from Azure and removes local files in one operation. You must specify `--target remote` (deletes from Azure only, local files kept) or `--target local` (removes local files only, Azure untouched). This prevents accidental dual-deletion and makes each operation's scope explicit
- **Local file deletion messaging** — `rigg delete --target local` now clearly states that local files are shared across all environments and that Azure resources are not affected in any environment
- **MCP server instructions** — updated with environment guidance, delete tool documentation, and knowledge source lifecycle information. All references now use MCP tool names instead of CLI commands
- **Description engine rewrite** — `describe.rs` replaced with a resource-aware English description engine. Labels adapt to context: diff uses `"locally"` / `"on the server"`, push uses `"on the server"` / `"locally"`, cross-env diff uses environment names

### Tests

- 543 tests across workspace (up from 522)

## [0.7.0] - 2026-02-22

See git tag `v0.7.0` for details. Resource scaffolding, agentic-rag composite command, DX improvements.

## [0.6.0] - 2026-02-19

### Added

- **MCP server (`rigg mcp serve`)** — built-in [Model Context Protocol](https://modelcontextprotocol.io/) server that lets AI coding tools (Claude Code, GitHub Copilot, Cursor, Codex, Gemini CLI) interact with rigg directly through structured tool calls. Exposes 8 tools: `rigg_status`, `rigg_describe`, `rigg_env_list`, `rigg_validate`, `rigg_list`, `rigg_diff`, `rigg_pull`, `rigg_push`. Communicates over stdio using JSON-RPC
- **`rigg mcp install` command** — register rigg as an MCP server with `claude-code` or `vs-code` targets. Creates user-level MCP configuration so rigg is available across all projects
- **`.mcp.json` auto-discovery** — projects with `.mcp.json` in the repo root are automatically discovered by Claude Code and VS Code. No manual setup needed
- **`rigg list` command** — list resource names by type from local disk, remote Azure, or both. Useful for quick enumeration and drift detection without the full detail of `rigg describe`
- **Agent skills (`.claude/skills/`)** — cross-platform [agent skills](https://agentskills.io/) that work with Claude Code, GitHub Copilot, Codex, Cursor, and Gemini CLI. Three user-invocable workflows (`/rigg-pull`, `/rigg-push`, `/rigg-status`) plus an auto-loaded reference guide
- **`rigg describe` precision for AI** — JSON output now includes `file_path` for every resource and full `instructions` for agents (previously truncated to first line). AI agents can read any file path for complete content
- **[MCP.md](MCP.md)** — comprehensive documentation for AI agent integration: setup, tool reference, parameters, example workflows

### Changed

- **Breaking: `--dry-run` removed** — mutating commands (`pull`, `push`) now use a unified `--force` pattern. Without `--force`, the command shows a preview and asks for confirmation. With `--force`, it executes immediately. This replaces the confusing three-mode system (`--dry-run`, default, `--force`)

### Tests

- 522 tests across workspace (up from 516)

## [0.5.3] - 2026-02-16

### Added

- **`rigg delete` command** — delete a resource from Azure and remove the corresponding local file in one step. Supports all resource types (`--index <name>`, `--agent <name>`, etc.). Knowledge source deletion warns about managed sub-resources and cleans up the entire KS directory. Includes confirmation prompt (skippable with `--force`)
- **Push conflict detection** — `rigg push` now detects when a resource has been modified on the server since your last pull (by comparing the remote checksum against the stored pull baseline). Shows a clear warning listing conflicting resources and suggests running `rigg pull` first to review remote changes before overwriting
- **Pull overwrite warning** — `rigg pull` now detects when local files have been modified since the last pull (by comparing the on-disk checksum against the stored pull baseline). Shows a warning listing locally modified resources before overwriting, so you can commit or stash local changes first

### Fixed

- **Release workflow Windows build** — GitHub Actions release builds now work on Windows. The build and package steps explicitly use `shell: bash` to avoid PowerShell environment variable expansion issues (`$TARGET` vs `$env:TARGET`)
- **README directory structure** — the directory layout example now matches the actual categorized directory structure (`search-management/`, `agentic-retrieval/`) instead of the old flat layout

### Changed

- GitHub repository now has a description and topic tags for discoverability

### Tests

- 516 tests across workspace (up from 505)

## [0.5.2] - 2026-02-15

### Fixed

- **Foundry client API version** — updated to `2025-05-15-preview` (was `2025-05-01`)
- **Resource files path** — `files_path` config option now correctly resolves the files root relative to the project root
- **Init categorized directories** — `rigg init` now creates the correct categorized directory structure (`search-management/indexes/`, `agentic-retrieval/knowledge-sources/`) matching pull output

### Changed

- Minor improvements to init messaging and error handling

## [0.5.1] - 2026-02-14

### Added

- **`--files-path` on `rigg init`** — separate config location (`rigg.yaml`, `.rigg/`) from resource files (`search/`, `foundry/`). Useful for monorepos where search config lives in a subdirectory

### Changed

- **Categorized directory structure** — resource directories restored to categorized layout: `search-management/indexes/`, `agentic-retrieval/knowledge-sources/` etc. The v0.5.0 flat layout (`search/indexes/`) was too shallow for clarity
- `ResourceKind::directory_name()` now returns categorized paths

## [0.5.0] - 2026-02-13

### Added

- **Deployment environments** — named environments (prod, test, staging, etc.) are now first-class config concepts. Each environment has its own set of search and foundry services, enabling multi-target management from a single project
- **`rigg env` subcommand** — `rigg env list`, `rigg env show [name]`, `rigg env set-default <name>`, `rigg env add <name>`, `rigg env remove <name>` for managing environments
- **`--env` global flag** — target a specific environment on any command (also available via `RIGG_ENV` environment variable). When omitted, uses the environment marked `default: true`
- **Cross-environment diff** — `rigg diff --env test --compare-env prod` fetches resources from both environments' remote servers and diffs them in memory, without involving local files
- **Per-environment state** — state and checksum files are now stored per environment in `.rigg/<env>/state.json` and `.rigg/<env>/checksums.json`
- **Service labels** — when an environment has multiple services in the same domain, each must have a `label` that creates a subdirectory (e.g., `search/primary/indexes/`, `search/analytics/indexes/`)

### Changed

- **Breaking: config format switched to YAML** — `rigg.toml` replaced by `rigg.yaml`. The new format uses an `environments:` map instead of flat `[[services.search]]` arrays. No migration from v0.4.0 — delete old config and re-init
- **Breaking: flat directory structure** — resource directories simplified from `search-resources/<service>/search-management/indexes/` to `search/indexes/`. Foundry from `foundry-resources/<service>/<project>/agents/` to `foundry/agents/`. Re-pull after upgrading
- **Breaking: state directory restructured** — `.rigg/state.json` replaced by `.rigg/<env>/state.json`. State is now per-environment
- **`ResolvedEnvironment` abstraction** — all commands now work through `ResolvedEnvironment` instead of accessing config directly, providing consistent environment resolution across the codebase
- **Client construction** — `AzureSearchClient::from_service_config()` replaces the old `new(&Config)` constructor, enabling per-environment client creation
- Removed legacy `[service]` config format migration (was auto-migrating since v0.2.0)
- Removed `toml` dependency from rigg-core, replaced by `serde_yaml`

### Tests

- 483 tests across workspace (up from 481)

## [0.4.0] - 2026-02-10

### Changed

- **Agent YAML format** — Foundry agents are now stored as a single `.yaml` file per agent (e.g., `agents/research-assistant.yaml`), matching the Foundry portal's YAML view. The previous 4-file decomposition (`config.json`, `instructions.md`, `tools.json`, `knowledge.json`) is removed
- Added `serde_yaml` dependency for agent YAML serialization
- `strip_agent_empty_fields()` normalizes empty optional fields for consistent diff/push behavior

### Tests

- 481 tests across workspace (up from 470)

## [0.3.0] - 2026-02-09

### Added

- **Managed sub-resources for knowledge sources** — knowledge sources auto-provision index, indexer, data source, and skillset sub-resources. These are now stored nested under the parent KS directory (`agentic-retrieval/knowledge-sources/<ks-name>/`) instead of mixed into `search-management/`. Rigg detects managed resources from the `createdResources` field in the KS definition and routes files automatically
- **Cascade push** — `rigg push --knowledgesources` pushes the KS first (triggering Azure to provision/reset sub-resources), then overlays customizations for the managed index, skillset, data source, and indexer in dependency order
- **Drop-and-recreate for immutable index changes** — when an index has removed or changed fields that Azure won't allow in-place, push now offers to drop and recreate the index (with a clear data-loss warning)
- **Knowledge source drop-and-recreate** — when a KS cascade push fails due to Azure's managed resource conflict bug (can't update a managed index with fewer fields), push offers to delete and re-provision the KS and all its sub-resources
- **`rigg copy` command** — local-only resource copying that replaces `push --copy`. Copies files and rewrites all names and cross-references without making network calls. Supports knowledge source copy (KS + all managed sub-resources) and standalone resource copy
- **Data source credential auto-discovery** — push now auto-discovers Azure Blob Storage connection strings via ARM `listKeys` API when credentials are missing (previously only worked in copy mode)
- **Managed-aware diff/status/describe/validate** — all commands understand the nested directory layout and distinguish managed vs standalone resources

### Changed

- **Breaking: directory layout** — managed sub-resources moved from `search-management/` to `agentic-retrieval/knowledge-sources/<ks-name>/`. Existing v0.2 projects should re-pull to migrate
- **Breaking: `push --copy` removed** — use `rigg copy` followed by `rigg push` instead. The `--suffix` and `--answers` flags are also removed
- **`--knowledgesources` flag expands scope** — on pull, push, and diff, this flag now automatically includes managed sub-resource types (index, indexer, data source, skillset)
- **Standalone flags skip managed** — `--indexes`, `--skillsets`, etc. only operate on standalone resources in `search-management/`, not managed sub-resources

### Tests

- 470 tests across workspace (up from 448)

## [0.2.12] - 2026-02-09

### Fixed

- **Reverted `2025-08-01-preview` API pin for knowledge resources** — v0.2.11 pinned knowledge base and knowledge source operations to `2025-08-01-preview`, but that API version uses different endpoint paths (`/agents/` instead of `/knowledgebases/`), causing all knowledge resource operations to fail with "api-version does not exist". All resources now use `2025-11-01-preview` again, matching the Azure portal's current API version

### Note

- Knowledge sources created through the Azure portal before December 2025 use the older `2025-08-01-preview` schema. These resources cannot be updated through the current `2025-11-01-preview` API (neither by rigg nor the portal) due to breaking schema changes in the Azure platform. The fix is to recreate affected knowledge sources through the portal, which now uses `2025-11-01-preview`. See [Microsoft's migration guide](https://learn.microsoft.com/en-us/azure/search/agentic-retrieval-how-to-migrate)

### Tests

- 448 tests across workspace

## [0.2.11] - 2026-02-09 [yanked]

### Fixed

- **Knowledge source corruption with `2025-11-01-preview` API** — knowledge base and knowledge source API calls are now pinned to `2025-08-01-preview`, which is compatible with existing agentic retrieval resources. The `2025-11-01-preview` API introduced breaking schema changes (fields like `language`, `production_family`, `embeddingModel`, `chatCompletionModel` reorganized into `ingestionParameters`; `sourceDataSelect` renamed to `sourceDataFields`) that made it impossible to update knowledge sources created with the older schema — even from the Azure portal. Other resource types (indexes, indexers, skillsets, etc.) continue to use `2025-11-01-preview`

### Tests

- 448 tests across workspace

## [0.2.10] - 2026-02-09

### Fixed

- **Skillset push with preview skills** — all Azure Search API calls now use the preview API version (`2025-11-01-preview`), which is a superset of the stable version. This fixes `rigg push --skillsets` failing with a 400 error when a skillset contains preview-only skill types like `ChatCompletionSkill`

### Changed

- Removed the `api_version` field from the internal search client struct — only `preview_api_version` is needed since all requests use it

### Tests

- 448 tests across workspace (one redundant stable-version test removed)

## [0.2.9] - 2026-02-09

### Fixed

- **False drift on agents with empty tools/tool_resources** — `rigg diff` no longer reports phantom changes for agents after a fresh `rigg init` + `rigg pull`. Both `compose_agent()` (local side) and `flatten_agent_response()` (remote side) now always include `tools` and `tool_resources` fields with empty defaults (`[]`/`{}`), ensuring consistent shape regardless of whether the API omits or includes these fields

## [0.2.8] - 2026-02-09 [yanked]

### Fixed

- **False drift on agents with empty tools** — partial fix; only addressed `compose_agent()` but not the remote side (`flatten_agent_response()`), which could still omit `tool_resources` when the API doesn't return it

## [0.2.7] - 2026-02-09

### Changed

- **Single README.md** — consolidated `RIGG.md` and category `README.md` files (previously generated in `search-management/` and `agentic-retrieval/` subdirectories) into the project root `README.md`. The root README now includes the directory layout, JSON file conventions, and full resource type reference with links to API docs. Foundry agent documentation is also included when Foundry services are configured

## [0.2.6] - 2026-02-09

### Changed

- **Preserve original array order in pulled JSON** — removed automatic sorting of JSON arrays by identity key during normalization. Pulled configuration files now preserve the exact element order returned by the Azure API, making it easier to compare local files with the portal. Volatile field stripping, credential redaction, and property order preservation are unchanged

### Tests

- 449 tests across workspace (up from 446)

## [0.2.5] - 2026-02-09

### Added

- **ASCII art banner** — `rigg init` and `rigg version` now display a bold block-letter RIGG logo
- **`rigg logo` easter egg** — hidden command that prints the banner

## [0.2.4] - 2026-02-09

### Added

- **Native TLS root certificates** — switched from bundled Mozilla CA roots to OS-native certificate stores (`rustls-tls-native-roots`). Fixes `UnknownIssuer` TLS errors on corporate networks using TLS inspection with custom CA certificates. Certificates are now read from macOS Keychain, Windows Certificate Store, or Linux system cert paths
- **Re-runnable `rigg init`** — running `rigg init` in an existing project now discovers and adds new services instead of bailing with "already initialized". Shows already-configured services, lists newly-discovered ones, and lets you select which to add. Existing configuration is preserved
- **Multi-select during init** — `rigg init` now supports selecting multiple search services and Foundry projects at once (comma-separated numbers)
- **Foundry endpoint refresh** — re-running `rigg init` refreshes endpoint URLs for existing Foundry configs using current ARM data

### Tests

- 446 tests across workspace (up from 439)

### Improved

- **TLS error diagnostics** — certificate verification failures now show a specific message explaining the likely cause (corporate TLS inspection) with OS-specific fix instructions for macOS, Linux, and Windows

## [0.2.3] - 2026-02-09

### Fixed

- **Foundry endpoint discovery from ARM** — `rigg init` now reads the actual endpoint URL from the AI Services account's ARM properties instead of constructing it from the resource name. This fixes connection failures when the account's custom subdomain differs from its resource name. The discovered endpoint is stored in `rigg.toml` as `endpoint` under `[[services.foundry]]`

### Improved

- **Connection error diagnostics** — HTTP connection failures now explain the likely cause (DNS resolution, private endpoint/VNet, firewall) and suggest re-running `rigg init` to rediscover the endpoint. The full error source chain is written to `rigg-error.log`

### Tests

- 439 tests across workspace (up from 434)

## [0.2.2] - 2026-02-09

### Improved

- **Better 403 troubleshooting guidance** — error message now explains the three most common causes: RBAC not enabled on the data plane (with the exact portal and CLI steps to enable it), missing role assignments (now recommends both Search Service Contributor and Search Index Data Contributor), and IP firewall restrictions

## [0.2.1] - 2026-02-09

### Improved

- **Rich 403 error handling** — access-denied errors now show the service name, a clear explanation, the exact `az role assignment create` command to fix it, common RBAC role names, and a link to Microsoft's RBAC documentation. Previously displayed as an empty `Error: API error (403):` with no guidance
- **Error log file** — when a client error occurs, detailed diagnostics (timestamp, response body, suggestion) are appended to `rigg-error.log` instead of flooding the terminal
- **Empty error body fallback** — API errors with no response body now show `HTTP <status> with no error details` instead of a blank message

### Tests

- 434 tests across workspace (up from 426)

## [0.2.0] - 2026-02-08

### Added

- **Microsoft Foundry support** — manage Foundry agent configurations alongside search resources in a single Git repository. Pull/push agent definitions including instructions, tools, and knowledge configurations
- **Multi-service configuration** — new `[[services.search]]` and `[[services.foundry]]` config format supports multiple services. Legacy `[service]` format auto-migrates on load
- **Symmetric init flow** — `rigg init` now discovers both Azure AI Search services and Microsoft Foundry projects via ARM APIs. Auto-selects when there's only one option. Either service type is optional — you can use rigg for Search only, Foundry only, or both together
- **ARM discovery for Foundry** — `rigg init` lists AI Services accounts and Microsoft Foundry projects from Azure subscriptions, matching the existing Search service discovery
- **Agent file decomposition** — Foundry agents are stored as human-friendly decomposed files: `config.json`, `instructions.md` (editable Markdown), `tools.json`, and `knowledge.json`
- **`--agents` / `--agent <NAME>` flags** — pull, push, diff, and pull-watch commands support Foundry agent resources
- **`--search-only` / `--foundry-only` flags** — scope operations to a single service domain
- **`ServiceDomain` enum** — internal architecture for routing operations to Search vs Foundry APIs
- **`FoundryClient`** — new REST API client for Microsoft Foundry project-scoped `/agents` endpoint (API version `2025-05-15-preview`)
- **Agent sections in `rigg status` and `rigg describe`** — shows Foundry service info, agent counts, model, tool count, and instruction previews
- **Agentic RAG Flows in `rigg describe`** — traces the full dependency chain from agent through knowledge base to knowledge source to index, showing descriptions and retrieval instructions at each level
- **Agent tools parsing** — `rigg describe` extracts knowledge base connections from MCP tool definitions in agent `tools.json`
- **Foundry agent push diff check** — `rigg push --agents` now compares local vs remote agent definitions and only pushes agents that have actually changed, matching how search resources work
- **Foundry API payload wrapping** — agent create/update uses the correct `{"definition": {...}}` wrapper format and `/agents/{name}/versions` endpoint

### Changed

- `rigg init` no longer requires Azure AI Search — at least one of Search or Foundry must be selected
- CLI description updated to reflect dual-service support: "Configuration-as-code for Azure AI Search and Microsoft Foundry"
- CLI resource type flags deduplicated via `clap(flatten)` — eliminates ~200 lines of repeated flag definitions across pull, push, diff, and pull-watch commands
- `ResourceKind` enum extended with `Agent` variant (9 total kinds)
- Authentication refactored to support multiple resource scopes (`search.azure.com` vs `ai.azure.com`)

### Tests

- 426 tests across workspace (up from 348)

## [0.1.7] - 2026-02-08

### Fixed

- Alias resource now correctly uses the preview API version — the aliases endpoint only exists in preview, so requesting it with the stable `2024-07-01` version caused `rigg pull` to fail with "api-version does not exist"
- `--aliases` flag now respects `include_preview` setting, consistent with other preview resource flags

### Changed

- Added `test-projects/` to `.gitignore` for local manual testing

## [0.1.6] - 2026-02-08

### Fixed

- Shell completion command no longer prints install instructions to stderr on every invocation, which cluttered terminal startup when sourced from `.zshrc`

## [0.1.5] - 2026-02-08

### Added

- **`rigg describe` command** — unified summary of all local resource definitions with text and JSON output, including field schemas, indexer wiring, skillset pipelines, and cross-resource dependency graph
- **SEARCH_CONFIG.md generation** — auto-generated markdown overview of the entire search configuration after `pull --all` when `sync.generate_docs = true`
- **Index alias support** — new `Alias` resource type for zero-downtime index swapping, with `--aliases`/`--alias` flags on pull, push, diff, and pull-watch
- **Parallel API calls** — concurrent resource fetching in pull (max 5 in-flight requests) for faster operations on large services
- **Retry logic** — automatic exponential backoff retry for transient Azure API errors (429 rate limiting, 503 service unavailable), max 3 retries
- **Resource linting** — `rigg validate` now warns about common misconfigurations: missing key fields, indexers without schedules, indexes with >50 fields, empty container names
- **`--output json` for status and validate** — machine-readable structured output for CI/CD and AI agent consumption

### Fixed

- README configuration example now matches actual code (`[project].path` not `[sync].resource_dir`, `[service]` not `[api]`)
- `rigg validate` now checks preview resources (KnowledgeBase, KnowledgeSource) when `include_preview = true`

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
- Semantic JSON diffing with `rigg diff`
- Git-based version control workflow for search configuration
- Azure CLI and service principal authentication
- ARM-based service discovery during `rigg init`
- Checksum-based change detection to minimize unnecessary writes
- Cross-platform release binaries (Linux, macOS, Windows)
- Published to crates.io as four crates: rigg, rigg-core, rigg-client, rigg-diff
