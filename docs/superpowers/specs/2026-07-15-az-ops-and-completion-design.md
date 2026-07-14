# `rigg az` operations commands + dynamic tab completion — design

**Date:** 2026-07-15
**Status:** approved

## Problem

Rigg manages the configuration plane (definitions in git, synced with
push/pull). Operating the resulting resources — triggering and watching an
indexer, smoke-testing an index, prompting a knowledge base or agent — still
requires the Azure portal or hand-rolled curl. During the regulatory
migration the portal had to fill this gap repeatedly (indexer run, status,
error inspection). And as commands grow longer, the lack of value-level tab
completion hurts.

## Decision summary

- New **`rigg az`** namespace: noun-first operational subcommands acting on
  the LIVE cloud resources (in contrast to the verb-first top-level config
  commands acting on files). Chosen over verb-first selectors and an "op"
  namespace; "az" over "cloud"/"live" per user preference.
- v1 nouns/verbs: `indexer run|reset|status`, `index query|stats`,
  `kb ask`, `agent ask` (kb = alias for knowledge-base).
- MCP exposure: `rigg_indexer_status`, `rigg_query`, `rigg_ask` read-only;
  `rigg_indexer_run` behind the force pattern; reset CLI-only.
- **Dynamic tab completion** for resource names / projects / envs /
  selectors, resolved from LOCAL workspace files (no network), via
  clap_complete's dynamic engine, alongside the existing static completions.

## 1. CLI surface

```
rigg az indexer run <name> [--watch] [--reset] [--yes] [--confirm-env ENV]
rigg az indexer reset <name> [--yes] [--confirm-env ENV]
rigg az indexer status <name>
rigg az index query <name> <text> [--top N] [--filter EXPR] [--select F1,F2]
rigg az index stats <name>
rigg az kb ask <name> <prompt>
rigg az agent ask <name> <prompt>
```

- Nouns match CLI kind names with aliases: `indexer`, `index`,
  `knowledge-base` (alias `kb`), `agent`.
- Environment resolution as everywhere: `-e` > `RIGG_ENV` > default env.
  Commands print the same target banner as push (service names + URLs).
- Resources are addressed by **physical name; project ownership is NOT
  required** (querying an unmanaged index is legitimate — these are runtime
  ops, not config ops). No `--project` flag.
- `--output json` (existing global flag) emits the raw API response for
  scripting; default output is human-formatted.

### Output shapes (text mode)

- `indexer status`: overall status, last run (status, start/end, duration,
  itemsProcessed/Failed), then per-document errors and warnings (message +
  document key), truncated at 20 with a count.
- `indexer run --watch`: triggers, then polls status every 5s printing
  state transitions ("running… → success (20 processed, 0 failed)");
  exits 0 on success, non-zero (exit 1) on a failed run with the errors
  rendered. Without `--watch`: fire-and-forget with a hint to
  `rigg az indexer status`.
- `index query`: one block per hit — `@search.score`, the `--select`ed
  fields (default: all retrievable fields, long strings truncated), total
  count. `--top` default 5.
- `index stats`: document count + storage size (human units).
- `kb ask`: the answer text, then a numbered source-reference list
  (document titles/ids as returned by the retrieval response).
- `agent ask`: the agent's reply text.

### Safety

- `reset` and `run --reset` require a default-No confirmation spelling out
  the cost: the next run reprocesses EVERY document (ingestion + embedding
  spend, and skill invocations). `--yes` satisfies it (recoverable, unlike
  replace). Non-interactive without `--yes` → usage error.
- Protected environments gate `run` and `reset` via the existing
  `confirm_protected_env` (`--confirm-env`).
- `status`, `stats`, `query`, `ask` are read-only and ungated.

## 2. Client layer (rigg-client)

Search data plane (`client.rs`, api-version per registry channel):

- `POST /indexers/{name}/run` → 202, no body
- `POST /indexers/{name}/reset` → 204
- `GET  /indexers/{name}/status`
- `GET  /indexes/{name}/stats`
- `POST /indexes/{name}/docs/search` — body `{search, top, filter, select, count: true}`
- Knowledge base retrieve: the agentic retrieval endpoint on
  `/knowledgebases/{name}` — exact path/body verified against the pinned
  api-version's REST reference during implementation (docs list it as the
  `retrieve` action; request carries the user prompt as messages, response
  carries an answer/response plus references).

Foundry data plane (`foundry.rs`): agent invocation via the v1 responses
API for a single-shot prompt against a named agent; exact contract verified
against the Foundry v1 reference during implementation.

`FoundryConnection` gains an `endpoint:` override (like `SearchConnection`
already has) if it lacks one, so foundry ops are wiremock-testable.

The `Remote` façade gains thin wrappers for these ops (search-domain ones
generic over name; kb/agent specific).

## 3. MCP tools (crates/rigg/src/mcp/tools.rs)

- `rigg_indexer_status {indexer, env?}` — read-only.
- `rigg_query {index, search, top?, filter?, select?, env?}` — read-only.
- `rigg_ask {knowledge_base? | agent?, prompt, env?}` — read-only; exactly
  one of knowledge_base/agent required.
- `rigg_indexer_run {indexer, env?, force?}` — without force: returns the
  current status + what would run (preview pattern); with force: triggers
  and returns the fire-and-forget acknowledgement.
- Tool descriptions teach the post-push verification flow (push → run →
  status → query → ask). All shell out to `rigg az ... --output json` like
  the existing tools.

## 4. Dynamic tab completion

Two layers:

1. **Static** (existing, unchanged): `rigg completion <shell>` generates
   clap_complete scripts covering subcommands and flags.
2. **Dynamic values** (new): clap_complete's dynamic engine
   (`unstable-dynamic` feature, `CompleteEnv`) — registered per shell with
   one line, e.g. `source <(COMPLETE=zsh rigg)`. The shell then invokes the
   rigg binary for candidates. Custom completers attach to arguments:
   - `rigg az indexer run|reset|status <TAB>` → indexer names; same
     per-noun for index/kb/agent commands.
   - Project-name positionals (push/pull/diff/status/validate/delete/
     promote/adopt) → project names.
   - `-e/--env` → environment names from rigg.yaml.
   - Selector positions (`adopt` selectors, `diff --only`) →
     `<kind-dir>/<name>` across the env tree.
   - `rigg new <TAB>` → kinds (+ `project`, `pipeline`, `api`);
     `rigg migrate knowledge-source <TAB>` → knowledge-source names.

   Candidates come from LOCAL workspace files only (workspace discovered by
   walking up from cwd, env = `RIGG_ENV` or the default; parsing `-e` from
   the in-flight command line is attempted best-effort). No network calls —
   unmanaged resources don't complete (documented). Outside a workspace,
   value completion yields nothing (silently).
3. `rigg completion --help` and the `rigg init` next-steps output teach the
   dynamic registration one-liner.

Caveat recorded: clap_complete's dynamic API is behind the
`unstable-dynamic` feature; the workspace pins clap_complete's version, so
API drift only surfaces on deliberate upgrades.

## 5. Architecture

- `crates/rigg/src/commands/az/mod.rs` — `AzCommands` dispatch +
  shared helpers (env banner, name arg).
- `crates/rigg/src/commands/az/{indexer,index,kb,agent}.rs` — one file per
  noun.
- `crates/rigg/src/completion_dynamic.rs` — candidate functions (pure:
  workspace path in, Vec<String> out; unit-testable) + CompleteEnv hookup
  in `main.rs`.
- Registry untouched — operations are not configuration.

## 6. Testing

- **Unit**: completion candidate functions against temp workspaces; output
  formatters (status/query rendering) against fixture JSON.
- **`tests/sync.rs` (wiremock)**: indexer run (202) + reset (204) +
  status rendering incl. errors; `--watch` polling transitions and the
  non-zero exit on a failed run (poll interval env-tunable for tests,
  `RIGG_WATCH_INTERVAL_SECS`); query/stats; kb ask; reset confirmation
  gating (non-interactive without --yes → exit 2); protected-env gating.
  Foundry agent ask via wiremock if the endpoint override lands, else unit
  + live only.
- **`tests/cli_surface.rs`**: az subtree arg shapes, kb alias, completion
  registration smoke (`COMPLETE=zsh rigg` emits a script).
- **Live** (mklabsrch/mklabaifndr): `rigg az indexer status
  test-ks-indexer`, a query against `test-ks-index`, `kb ask` against a
  test KB, `agent ask Regulus`; completion tried interactively by
  Kristofer.

## Out of scope (backlog)

- `rigg az agent chat` — interactive multi-turn REPL.
- `rigg az indexer resetdocs` (per-document reset), debug-session support.
- Network-backed completion (unmanaged resource names).
- `rigg open <selector>` — portal deep-links.
