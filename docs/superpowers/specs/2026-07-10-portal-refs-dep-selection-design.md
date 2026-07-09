# rigg — Portal-authored references, dependency selection, re-adoption

**Date:** 2026-07-10
**Status:** Design — approved (user-directed)
**Workstream:** D (A concepts, B scoped adopt, C wizard — all merged).

## Problems (all found by live-testing the Regulus adoption)

1. **Portal-built agents hide their cross-service dependencies.** Regulus's
   MCP tool carries a raw `server_url`
   (`https://mklabsrch.search.windows.net/knowledgebases/regulatory-kb/mcp?…`)
   and a `project_connection_id` (`kb-regulatory-kb-9kdyn`). rigg's reference
   extractor only understands `x-rigg-ref` annotations (rigg-authored files)
   and the RefField table — so `--with-deps` stopped at the model deployment
   and missed the entire Search retrieval stack plus the connection.
2. **Dependency adoption is all-or-nothing.** The wizard asks one yes/no for
   the whole dependency set; the user may want the knowledge base but not the
   rest.
3. **"Change my mind later" doesn't work.** `rigg adopt regulus agents/Regulus
   --with-deps` after Regulus is already owned classifies the agent as
   "already managed" and drops it — dependency expansion then seeds from an
   empty set and finds nothing. Adopting missing dependencies of an owned
   resource requires manually naming each one.
4. **Server state leaks into deployment files.** `properties.currentCapacity`
   and `properties.deploymentState` are runtime state, not configuration —
   they will cause phantom drift.

## Conceptual decisions (settled with the user)

- **`pull` vs `adopt` division stands**: pull refreshes what you own; adopt
  changes ownership. The portal scenario ("someone wires a new KB into the
  agent via the portal") is `pull` (captures the agent's changed JSON) then
  `adopt` (claims the newly referenced, unmanaged resource).
- **Re-capturing dependencies is still adoption** — same verb, no new command:
  naming an owned resource with `--with-deps` means "adopt this resource's
  missing dependencies" (the resource itself stays a no-op). This makes the
  wizard's `hint:` line idempotent and re-runnable.
- **Hints teach**: the post-success hint printing the equivalent scriptable
  command stays and remains correct under re-runs; hints suggesting the next
  likely command are a design principle to apply where natural.

## Design

### 1. Portal-authored reference extraction (rigg-core)

`registry::extract_references` learns two agent-tool reference shapes:

- **KB MCP URL**: any string field `server_url` (in the agent doc) whose value
  parses as `https://<host>/knowledgebases/<name>/mcp[?…]` where `<host>` ends
  with `.search.windows.net` (path segments matched case-insensitively) →
  reference `(KnowledgeBase, <name>)`. Environment-safety is inherent: if the
  URL points at a different Search service than the environment's, the target
  is not in the snapshot, so dependency expansion naturally drops it.
- **Connection id**: agent tool `project_connection_id: <id>` →
  `(Connection, <id>)`. Implemented as a RefField table row
  (`tools.project_connection_id`) if `collect_path` traverses arrays (verify);
  otherwise in the same custom pass as the URL.

Both live in a kind-gated pass (Agent only) alongside `collect_x_rigg_refs`.
Unit tests: the real Regulus tool shape extracts both; a non-Search URL
(e.g. `https://example.com/mcp`) extracts nothing; non-agent kinds unchanged.

### 2. Dependency expansion seeds from explicitly-named owned resources

In the adopt classification loop, an explicit `<kind>/<name>` selector that is
owned **by this project** still skips adoption (no-op, message unchanged) but
is collected as an expansion **seed** (its snapshot doc). `expand_deps` starts
its walk from `to_adopt ∪ seeds`; additions remain unmanaged-only,
platform-managed-excluded, snapshot-bounded. Resources owned by *other*
projects are never seeds (unchanged hard error / silent skip).

Effect: `rigg adopt regulus agents/Regulus --with-deps` works before AND after
Regulus is adopted, and captures new portal-added dependencies after a `pull`.

### 3. Wizard: managed resources visible, dependencies selectable

- **Menu**: in addition to unmanaged resources, resources owned by the
  **target project** appear, marked — label suffix ` (managed)` — so the user
  can select one to trigger dependency capture. Resources owned by *other*
  projects stay hidden. Selecting only managed resources with no missing deps
  ends with "Nothing to adopt" (exit 0).
- **Dependency step**: instead of yes/no on the whole set, the computed
  dependency closure is shown as a **multi-select with every item
  pre-checked** (Enter = adopt all, space to drop individual items — e.g.
  keep the knowledge base, skip the rest). Deselecting an item does not
  remove other items that were reachable only through it — the closure is a
  flat list; à la carte means the user's picks are final.
- Non-interactive `--with-deps` stays all-or-nothing (deterministic scripts).
- The final `hint:` includes `--with-deps` when any dependency was adopted and
  lists the resources the user actually picked from the menu (managed picks
  included — re-running is a no-op-plus-new-deps, which is the point).

### 4. Deployment volatile fields

Add `properties.currentCapacity` and `properties.deploymentState` to the
Deployment `volatile_fields` in the registry. Note for existing files: the
user's already-adopted `gpt-5.2-chat.json` contains both fields; after this
change they are ignored in diff and stripped on the next pull/push
canonicalization — no migration needed.

### 5. Help/docs

- `rigg adopt --help`: document that naming an already-managed resource with
  `--with-deps` adopts its missing dependencies.
- README/GETTING_STARTED: one-line mention of the re-run pattern.

## Testing

- rigg-core unit tests for the two new reference shapes (+ negative cases).
- Wiremock sync tests (Search-side, no Foundry needed): owned indexer +
  `--with-deps` adopts its unmanaged data source/index; without `--with-deps`
  stays a pure no-op; deps of owned seeds respect ownership + platform rules.
- adopt.rs unit tests: wizard candidate labels include ` (managed)` for
  target-project resources and exclude other-project resources; dependency
  multi-select list building.
- Registry test for the new volatile fields.
- Live acceptance: re-run the wizard; select `agents/Regulus (managed)`;
  expect the full regulatory stack offered as pre-checked dependencies.

## Files touched

- `crates/rigg-core/src/registry.rs` (reference extraction, volatile fields)
- `crates/rigg/src/commands/adopt.rs` (seeding, wizard menu, dep multi-select)
- `crates/rigg/src/cli.rs` (adopt help text)
- `README.md`, `GETTING_STARTED.md`
- tests: registry inline, adopt.rs inline, `crates/rigg/tests/sync.rs`
