# Cosmos DB → Knowledge Source Wizard

**Status:** approved design, not yet implemented
**Date:** 2026-05-08
**Scope (v1):** Cosmos DB → Azure AI Search Knowledge Source. Blob Storage, Knowledge Base, agent, and prompt flows are explicitly out of scope and addressed in later specs.

## Problem

A user has a Cosmos DB collection and wants to build a RAG pipeline over it using Azure AI Search. They're stuck because:

- The Azure Portal does not (yet) support creating a Cosmos-backed Knowledge Source.
- They don't know which fields to index, which cognitive skills to apply, or how to wire the resources together.
- Existing rigg gives them push/pull plumbing but no guidance for designing the configuration in the first place.

The user (also rigg's author) has this need today, urgently. v1 must unblock that path.

A second problem this spec addresses: the broader vision is for rigg to support guided RAG configuration via two interaction modes — its own AI-driven wizard, and a "skill" external AI agents install. The two modes need to share a substrate without forcing one's UX onto the other.

## Goals

- A user with no rigg project, no documentation knowledge, and only a Cosmos endpoint should be able to design and push a working KS in one session.
- The wizard works when run by rigg's own AI (Ailloy) **or** by an external agent (Claude Code, Copilot, Codex, Cursor, Gemini) using a skill emitted by rigg.
- Helper subcommands are independently useful — scriptable, no wizard required.
- The emitted skill is a thin pointer; the rigg binary serves the live reference content. Skill behavior improves automatically when rigg upgrades.
- Backwards compatibility for every existing rigg user. New surface is purely additive.

## Non-goals (v1)

- Blob Storage, SQL DB, or other data source types beyond Cosmos DB.
- Knowledge Base, Foundry agent, or RAG prompt configuration. Each gets its own spec.
- A web UI, TUI library, or graphical experience. Wizard is plain stdin/stdout.
- Live integration tests against a real Cosmos account in CI.
- Resumable wizard sessions (state persisted across runs).
- A non-interactive flag-driven wizard mode (the helper subcommands cover that case).

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                          rigg wiz                               │
│  Rust state machine. 14 states. Calls Ailloy at three points.   │
│   1. ensure rigg.yaml + env exist                               │
│   2. pick data source type (v1: cosmosdb only)                  │
│   3. collect connection info (AAD → connection string → paste)  │
│   4. sample data                                                │
│   5. interview (structured Q&A)                                 │
│   6. synthesize user profile (Ailloy)                           │
│   7. suggest fields (Ailloy or heuristic)                       │
│   8. suggest skills (Ailloy or heuristic)                       │
│   9. resource naming                                            │
│  10. scaffold files                                             │
│  11. preview push (existing rigg push pipeline)                 │
│  12. confirm                                                    │
│  13. push                                                       │
│  14. summary + next-step hints                                  │
└─────────────────────────────────────────────────────────────────┘
        │              │              │              │
        ▼              ▼              ▼              ▼
   rigg analyze   rigg suggest   rigg new <kind>   rigg push
   (new)          (new)          (existing,        (existing)
                                  Cosmos templates
                                  added)
        │              │
        ▼              ▼
   rigg-client    rigg-client::ai
   ::cosmos       (Ailloy, existing)
   (new module)
```

### Crates affected

- `rigg-core` — extend `scaffold.rs` with Cosmos-aware data source and KS templates; extend `resources/datasource.rs` and `resources/knowledge_source.rs` to validate `cosmosdb` type; static reference markdown lives in `references/` and is bundled via `include_str!`.
- `rigg-client` — new `cosmos.rs` module: AAD token via existing `auth.rs` chain (scoped to `https://cosmos.azure.com`), connection-string fallback, sample-N-docs via Cosmos REST API. No SDK dependency.
- `rigg` — new commands `wiz/`, `analyze/`, `suggest/`. Updates to `commands/skill.rs` to emit the new thin-pointer bundle and serve topic-scoped reference. No rename of existing commands.

### Command surface (additions only — no breaking changes)

```
rigg wiz                                           # interactive wizard

rigg analyze cosmos --endpoint <url> --database <db> --container <c>
                    [--connection-string <s>] [--sample-size 20]
                    [--output json|text]
rigg analyze sample <path-or-stdin>                # paste/file fallback

rigg suggest fields --schema <schema.json> [--profile <profile.json>] [--output json]
rigg suggest skills --schema <schema.json> [--profile <profile.json>] [--output json]

rigg new datasource --type cosmosdb …              # extends existing
rigg new knowledgesource --type cosmosdb …         # adds --type, default azureblob

rigg ai skill --emit [--out-dir <dir>]             # extended (was: single file → now: thin bundle)
rigg ai skill --reference [TOPIC]                  # extended (TOPIC arg new; no-topic prints today's content)
```

`rigg ai skill --emit` and `rigg ai skill --reference` keep their existing names and flag shape. Behavior gets smarter; no rename.

## Components

### 1. Cosmos introspection (`rigg analyze`)

**Purpose:** sample documents from a Cosmos collection (or pasted file) and infer field schema.

**`rigg analyze cosmos`** flow:
1. Resolve auth: AAD token via `rigg-client::auth` (scoped to `https://cosmos.azure.com`), reusing the same Azure CLI session rigg already uses.
2. If AAD fails, accept `--connection-string` from CLI/stdin/env.
3. If both fail, exit non-zero with a structured error and a hint to use `rigg analyze sample`.
4. Issue a small SQL query (`SELECT TOP @n * FROM c`) via Cosmos REST API.
5. Run inference (deterministic Rust code) over the sampled docs.
6. Emit a JSON document.

**`rigg analyze sample`** runs the same inference pass over a pasted JSON file or stdin. Accepts a single document or an array.

**Output JSON shape:**
```json
{
  "endpoint": "...",
  "database": "...",
  "container": "...",
  "partition_key_path": "/id",
  "sample_count": 20,
  "inferred_schema": {
    "fields": [
      {"name": "id", "types": ["string"], "presence": 1.0, "nullable": false, "max_length": 36},
      {"name": "title", "types": ["string"], "presence": 1.0, "max_length": 240},
      {"name": "tags", "types": ["array<string>"], "presence": 0.85},
      {"name": "createdAt", "types": ["string"], "presence": 1.0, "format_hint": "iso8601"},
      {"name": "embedding", "types": ["array<number>"], "presence": 0.6, "dimensions": 1536}
    ],
    "warnings": ["createdAt looks like a date but is stored as string"]
  }
}
```

**Inference rules** (deterministic):
- Walk every sampled doc, union types per field name.
- Compute presence (fraction of docs containing the field), nullability, max string length, array element type.
- Heuristics: ISO-8601-looking strings get `format_hint: "iso8601"`; arrays of numbers with consistent length get `dimensions: <n>` and a hint of "embedding".
- Warnings are factual observations, not recommendations.

### 2. Suggestions (`rigg suggest`)

**Purpose:** turn an inferred schema (and optional user profile) into recommendations for index fields and cognitive skills.

**Inputs:** `--schema <path-or-stdin>`, optional `--profile <path-or-stdin>`.
**Output:** JSON document. Each suggestion includes a one-line `rationale`.

**Field suggestions** map inferred shapes to Edm types and search attributes (key/searchable/filterable/facetable/sortable/retrievable, plus vector profile when applicable).

**Skill suggestions** propose a skillset based on field shapes (e.g., long text fields → `SplitSkill` for chunking; profile-driven RAG style → embedding skill).

**Ailloy integration:** the suggester builds a system prompt explaining the goal, a user prompt with the schema and profile JSON, and asks Ailloy to return JSON conforming to a described schema. Output is parsed via `serde_json` and validated by a deterministic post-pass that drops hallucinated field types and enforces invariants (one key field, vector dim consistency, etc.).

**Heuristic fallback when Ailloy is not configured:**
- String fields → searchable + retrievable.
- Single string at full presence with unique-looking values → key.
- Array-of-strings → filterable + facetable.
- Array-of-numbers with consistent length → vector field with `dimensions` from the inference.
- ISO-8601 strings → filterable + sortable, type `Edm.DateTimeOffset` (with a warning if storage type is string).

When the heuristic fallback is used, the command's output includes a visible notice on stderr and in the JSON `warnings` array: *"AI disabled — suggestions are basic; run `rigg ai enable` for better recommendations."*

### 3. Wizard (`rigg wiz`)

A 14-state Rust state machine. Plain stdin/stdout, no TUI library. Each interactive prompt accepts `b` to back up one state and `q` to quit.

| # | State | Description | Ailloy? |
|---|-------|-------------|---------|
| 1 | `EnsureProject` | If `rigg.yaml` missing, run streamlined init (project name, environment, search service via existing ARM discovery). Reuses `commands::init` internals. | no |
| 2 | `PickDataSourceType` | v1: cosmosdb only. Future versions add other types. | no |
| 3 | `CollectConnection` | Endpoint/db/container. AAD attempted first; on failure, offer connection string or paste-sample fallback. | no |
| 4 | `Sample` | Calls `rigg analyze cosmos` (or `rigg analyze sample <file>`). Shows summary. | no |
| 5 | `Interview` | Structured multi-choice Q&A: use case, audience, languages, latency vs cost preference, RAG style. | no |
| 6 | `SynthesizeProfile` | Sends interview answers to Ailloy: "Write a one-paragraph summary of this user's RAG goals." Stored in profile JSON alongside the structured fields. | **yes** |
| 7 | `SuggestFields` | Calls `rigg suggest fields --schema ... --profile ...`. Renders a checklist with rationales. User toggles on/off, edits names/types in place via accept-or-reject + open-in-$EDITOR escape hatch. | **yes** |
| 8 | `SuggestSkills` | Same UX as state 7 over skills. | **yes** |
| 9 | `ResourceNaming` | Prompts for a base name (default from container name). Computes `<base>-ks`, `<base>-idx`, `<base>-idxr`, `<base>-ds`, `<base>-skillset`. | no |
| 10 | `Scaffold` | Writes JSON files via `rigg-core::scaffold` plus the new Cosmos-aware KS template. No network. | no |
| 11 | `Preview` | Runs `commands::push::preview` directly — shows the diff the user will push. | no |
| 12 | `Confirm` | y/n. | no |
| 13 | `Push` | Calls `commands::push::execute` — same code path as `rigg push --force`. | no |
| 14 | `Done` | Summary + next-step hints. | no |

**Profile JSON shape** (produced by states 5+6, also hand-authorable):
```json
{
  "use_case": "customer-support-chatbot",
  "audience": "external-users",
  "languages": ["en"],
  "latency_priority": "low",
  "cost_priority": "balanced",
  "rag_style": "vector+keyword-hybrid",
  "summary": "Customer support chatbot answering product/return questions over a Cosmos collection of knowledge articles. Hybrid retrieval, English only, low latency."
}
```

**Failure recovery:**
- Cosmos connect failure (state 3) → paste-sample fallback without losing progress.
- Ailloy failure (states 6/7/8) → heuristic fallback with visible notice; wizard continues.
- Push failure (state 13) → files remain on disk; user fixes and runs `rigg push` directly.
- User aborts mid-flow → partial files kept; "wizard aborted, partial files at <paths>" printed.

**Non-interactive mode:** out of scope. Scripted users go through the helper subcommands directly or through the emitted skill via their own agent.

### 4. Skill emission (`rigg ai skill --emit`)

The emitted skill is **thin** — a stable pointer that delegates live reference content to the rigg binary at runtime. When rigg upgrades, the skill's behavior improves automatically because every invocation of `rigg ai skill --reference <topic>` returns the new binary's view.

**Bundle structure** (stdout by default; one markdown file with sections):
```markdown
# SKILL.md
<frontmatter: name, description, when-to-use>
<short capability summary>
<pointer to playbook + reference commands>

# playbook.md
<step-by-step conversational flow for the agent>
<at each step that needs domain reference, says: "run `rigg ai skill --reference <topic>` for current details">
```

`rigg ai skill --emit` defaults to stdout (single markdown file). With `--out-dir <dir>`, writes the bundle as separate files. The user installs by piping the command's output through their agent ("set up a skill for me, run `rigg ai skill --emit`"); the agent figures out the platform-appropriate location.

**Live reference (`rigg ai skill --reference [TOPIC]`):**

| Topic | Source | Content |
|-------|--------|---------|
| (no topic) | back-compat | Today's reference dump (lists topics + general overview) |
| `commands` | clap-generated | All helper commands with current flags + examples — always in sync with the binary |
| `cosmos-ks-schema` | `rigg-core/references/` static md | Schema of a Cosmos-backed KS: required fields, query parameters, partition key handling |
| `edm-types` | `rigg-core/references/` static md | Edm.* field types and when each is used |
| `cognitive-skills` | `rigg-core/references/` static md | Catalog of supported cognitive skills |
| `playbook cosmos-to-ks` | `rigg-core/references/` static md | Same content as the bundled `playbook.md`, accessible at runtime |
| `troubleshooting` | `rigg-core/references/` static md | Common errors and recovery hints |

Defaults to markdown output. `--format json` flag added for programmatic consumers.

**Migration of existing `--reference` (no topic) behavior:** kept as the default so existing scripts that pipe `rigg ai skill --reference` continue to work.

### 5. Cosmos-aware resource scaffolding

Three additions in `rigg-core/src/scaffold.rs`:

**Data source template** (`rigg new datasource --type cosmosdb`):
```json
{
  "name": "<name>",
  "type": "cosmosdb",
  "credentials": { "connectionString": "<placeholder>" },
  "container": { "name": "<container>", "query": "SELECT * FROM c" },
  "dataChangeDetectionPolicy": {
    "@odata.type": "#Microsoft.Azure.Search.HighWaterMarkChangeDetectionPolicy",
    "highWaterMarkColumnName": "_ts"
  }
}
```

**Knowledge source template** (`rigg new knowledgesource --type cosmosdb …`): new `--type` flag (default `azureblob` for back-compat). For `cosmosdb`, the KS payload encodes the right `kind` and connection block per Azure's preview API. The wizard fills in auto-provisioned managed sub-resources via the existing `createdResources` pipeline.

**Index template** suited to retrieval-style use cases: when the wizard sees array-of-number fields (embedding) or text-heavy fields, the index template includes vector search profile defaults and (if interview indicates text search) a semantic search config.

**Validation** (`rigg-core::validate/lint.rs`): warn — don't block — if a Cosmos data source has no `dataChangeDetectionPolicy` or if `query` is missing. Don't enforce partition-key filtering; that's situational.

## Data flow

```
User                  rigg wiz             rigg analyze         Cosmos
  │                       │                      │                │
  │── runs wiz ──────────▶│                      │                │
  │                       │── spawn ────────────▶│                │
  │── prompts ◀──────────│                      │                │
  │── connection info ───▶│                      │                │
  │                       │── pass info ────────▶│                │
  │                       │                      │── AAD or key ─▶│
  │                       │                      │◀── sample docs│
  │                       │◀── inferred schema ──│                │
  │── interview answers ─▶│                      │                │
  │                       │ ┐                                       │
  │                       │ │ rigg-client::ai (Ailloy)              │
  │                       │ │ profile synthesis                     │
  │                       │ ┘                                       │
  │                       │── spawn ────────────▶│ rigg suggest    │
  │                       │                      │ fields/skills    │
  │                       │                      │ + Ailloy         │
  │                       │◀── suggestions ──────│                  │
  │── accept/reject ─────▶│                      │                  │
  │                       │── scaffold files     │                  │
  │                       │── preview push       │                  │
  │── confirm ───────────▶│                      │                  │
  │                       │── rigg push ─────────────────▶ Azure   │
  │◀── done ──────────────│                                          │
```

The wizard spawns helper subcommands as subprocesses (matching the existing pattern in MCP tools) so any future caller — MCP, external agent, scripted — uses the same code paths.

## Auth

- Cosmos sampling: AAD via existing `rigg-client::auth` chain, scoped to `https://cosmos.azure.com`. Falls back to `--connection-string`. Never persists secrets.
- Search push: existing `rigg-client::client` AAD chain, scoped to `https://search.azure.com`. Unchanged.
- Both flows share the same Azure CLI session — one login, all flows work.

## Error handling

| Failure | Behavior |
|---------|----------|
| Cosmos AAD + connection-string both fail | Wizard offers paste-sample fallback. Helpers exit non-zero with a structured error. |
| Sample query times out | Retry once with smaller `--sample-size`. On second failure, fall back to paste mode. |
| Schema inference produces zero fields | Show "no fields found in sampled docs — paste a richer sample?"; return to state 4. |
| Ailloy not configured | Heuristic fallback. Visible notice. Suggester still produces output. |
| Ailloy returns malformed JSON | One re-prompt with stricter instructions. On second failure, heuristic fallback + warn. |
| Push fails after wizard | Files remain on disk. Print error + "fix issues and run `rigg push`." Exit non-zero. |
| User aborts mid-wizard | Files written so far are kept. Print "wizard aborted, partial files at <paths>." |

All wizard steps are idempotent and resumable manually — a partial flow can be completed via the regular `rigg new …` + `rigg push` commands.

## Testing

**Unit tests:**
- `rigg-core::scaffold` — Cosmos data source + KS templates. Extends the existing pattern at `scaffold.rs:415`.
- Schema inference — table tests of (sample docs → expected inferred schema). No network.
- Heuristic suggestion fallback — table tests of (inferred schema → expected suggestions).
- Profile-synthesis prompt builder — golden tests on the prompt text Ailloy receives.

**Integration tests:**
- `rigg analyze sample <fixture.json>` end-to-end against checked-in JSON fixtures.
- `rigg suggest fields/skills` end-to-end with a stubbed Ailloy. A test-only feature flag swaps Ailloy for a deterministic mock that returns canned JSON.
- Wizard state machine driven by a scripted input source — verify each transition produces the expected files.

**Manual / live:**
- Real Cosmos collection in `test-projects/cosmos-wiz/` (gitignored).
- The author's urgent need is the v1 alpha test.
- Fold into the existing `test-complete-enduser-experience` skill once stable.

**No live Cosmos in CI.** Deterministic mocks cover the code paths.

## Backwards compatibility

| Surface | Before | After |
|---------|--------|-------|
| `rigg ai skill --emit` | Single static file | Thin-pointer bundle to stdout (or `--out-dir`). Same flag name. |
| `rigg ai skill --reference` | Static dump | Optional `[TOPIC]` argument. No topic = today's behavior. Same flag name. |
| `rigg new datasource --type azureblob` | Default | Unchanged. |
| `rigg new datasource --type cosmosdb` | Existed in scaffold but never wired | Now produces a complete, valid Cosmos data source. |
| `rigg new knowledgesource` | Blob-default | Adds `--type` flag, defaults to `azureblob`. |
| KnowledgeSource resource struct | Existing fields | May grow optional fields for Cosmos. Existing serialized files round-trip. |
| `rigg pull/push/diff/validate` | Existing | Unchanged paths; now also work for `cosmosdb` data sources. |
| MCP server (`rigg mcp`) | 9 tools | Unchanged. New helpers are NOT auto-exposed as MCP tools in v1 — focus is the skill-driven external-agent flow. |
| State / checksums format | `.rigg/<env>/{state.json,checksums.json}` | Unchanged. |
| `rigg.yaml` config | Existing schema | Unchanged. |
| Existing skills (`rigg-guide`, `rigg-pull`, etc.) | Hand-authored | Unchanged. The new `rigg-build-ks` skill is additive. |

## Implementation order

Each phase is independently shippable.

**Phase 1 — Cosmos plumbing**
1. `rigg-client::cosmos` module: AAD + connection-string sampling.
2. `rigg new datasource --type cosmosdb` produces a valid file.
3. `rigg new knowledgesource --type cosmosdb` produces a valid file.
4. Validation hooks for `cosmosdb` data sources.

**End of phase 1:** the user can manually configure Cosmos → KS using `rigg new` + handcrafted edits + `rigg push`. The urgent need is unblocked.

**Phase 2 — Standalone helpers**
5. `rigg analyze cosmos` and `rigg analyze sample` (deterministic schema inference).
6. `rigg suggest fields` and `rigg suggest skills` — heuristic fallback first, then Ailloy path.

**End of phase 2:** an external agent driven by a human can drive the full flow by calling subcommands.

**Phase 3 — Skill emission and live reference**
7. Restructure `rigg ai skill --emit` to write the thin-pointer bundle.
8. Extend `rigg ai skill --reference` to take an optional `[TOPIC]` argument.
9. Static reference markdown for `cosmos-ks-schema`, `edm-types`, `cognitive-skills`, `troubleshooting`.
10. Clap-generated `commands` reference.

**End of phase 3:** any agent that loads the emitted skill can drive the flow conversationally with version-current reference.

**Phase 4 — `rigg wiz`**
11. Wizard state machine (states 1–14).
12. Interview questionnaire.
13. Ailloy profile synthesis.
14. Interactive checklist UX with accept-or-reject + open-in-$EDITOR escape hatch.
15. Backup-step (`b`) navigation.

**End of phase 4:** the in-house Ailloy-driven wizard ships.

If a phase is blocked (Cosmos preview API quirks, Ailloy schema validation issues), each phase below the blocker is still independently useful.

## Open implementation questions

These are deliberately deferred to implementation, not design:

- Exact JSON shape of a Cosmos-backed Knowledge Source per Azure's `2025-11-01-preview` API. The KnowledgeSource resource struct may need optional fields added — to be discovered by writing a manual KS file and pushing it during phase 1.
- Whether the Cosmos REST sampling needs `x-ms-documentdb-isquery: True` header semantics around partition keys for cross-partition queries. Pick at implementation time.
- Whether the `commands` reference topic warrants a separate clap visitor or can be derived from the existing `--help` plumbing.

These are noted here so the implementation plan can pull on them at the right time.
