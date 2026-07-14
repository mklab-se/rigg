# Knowledge-source migration to `searchIndex` — design

**Date:** 2026-07-14
**Status:** approved

## Problem

Knowledge sources created in the Azure portal as indexed types (`azureBlob`,
`azureSql`, `oneLake`, indexed SharePoint, `file`) auto-generate their pipeline
(data source, index, skillset, indexer) and own it: the generated objects are
invisible to rigg (filtered via `createdResources`) and **deleting the
knowledge source deletes all generated objects, including the index**. The
`searchIndex` kind gives full control — every pipeline piece is an explicit,
rigg-managed resource — and deleting a `searchIndex` KS never touches its
index.

Users need a smooth path from any indexed KS type to `searchIndex`. Doing this
in the portal is destructive and manual.

### API facts (verified against docs, stable `2026-04-01` + preview how-tos)

- `DELETE /knowledgesources('{name}')` takes no parameters beyond the name.
  **There is no API to decouple generated sub-resources from a KS.** An
  in-place migration therefore always implies a full index rebuild.
- A KS delete **fails** (listing affected knowledge bases) while any KB still
  references it. Temporary KB unlinking is mandatory.
- Deleting a `searchIndex`-type KS does not delete the referenced index.

## Decision summary

- New command **`rigg migrate knowledge-source <name>`** (alias `ks`), a
  **local-only** transformation. Nothing mutates Azure until `rigg push`.
- **`rigg push` gains a generic `replace` verb**: same-named resource whose
  local doc differs from remote on an *immutable field* (registry concept;
  for KnowledgeSource: `kind`). No marker files; a hand-edited kind change
  triggers the same path.
- Two migration modes:
  - **In-place**: same KS name, same sub-resource names (read from live
    `createdResources`, never guessed). Next push replaces the KS and rebuilds
    the index — explicitly warned (time, ingestion/embedding cost, downtime).
  - **Side-by-side**: new names for KS + sub-resources, interactively
    confirmed. Push is plain creates; the old KS keeps serving. **Cutover is
    manual** (user re-points KBs and deletes the old KS file + `push --prune`
    when satisfied).
- Push-time safety gate for replaces: interactive default-No confirmation;
  non-interactively `--yes` is NOT sufficient — a dedicated **`--allow-replace`**
  flag is required (pattern mirrors `--confirm-env`). MCP `rigg_push` gains a
  matching `allow_replace` parameter.

## 1. `rigg migrate knowledge-source <name>`

Project-scoped like `adopt`/`promote`; honors `-e/--env`, `--project`,
`--yes`, `--non-interactive`, `--no-color`.

### Preflight

1. Load workspace/project, connect, GET the live KS.
2. Kind checks:
   - already `searchIndex` → exit 0, "nothing to migrate";
   - remote kinds (web, MCP, Work IQ, …) → error: no generated pipeline;
   - indexed kinds must expose `createdResources` (nested under
     `<kind>Parameters`; `registry::collect_created_resources` already finds it
     at any depth).
3. Ownership: the KS must be owned by the project (file or baseline);
   otherwise error pointing at `rigg adopt`.

### Mode selection

Interactive select, or flags: `--in-place` / `--rename <new-ks-name>`.

**In-place**

- KS name unchanged; sub-resource names taken verbatim from
  `createdResources`.
- Writes into the project:
  - the four sub-resource JSONs copied from the **live generated definitions**
    (`normalize_for_disk`), preserving chunking/embedding configuration —
    scaffolds would silently change retrieval behavior;
  - the KS file rewritten to the `searchIndex` shape
    (`kind: "searchIndex"`, `searchIndexParameters.searchIndexName = <generated
    index name>`, `description` preserved).
- KB files untouched (KS name is stable).
- Final output: explicit warning that the next push deletes and rebuilds the
  index.

**Side-by-side**

- Prompt for new KS name; derive suggested sub-resource names by swapping the
  old-KS-name prefix in each generated name (`regulatory-index` →
  `regulatory-v2-index`); each name individually editable (interactive text
  prompts). Non-interactive: derived defaults.
- Validate no name collisions, locally or remotely.
- Writes only new files (new `searchIndex` KS + four sub-resources); old KS
  file untouched.
- Next-steps output: push → verify → re-point KB → delete old KS file and
  `push --prune`.

### Credential fidelity (both modes)

Copied data-source definitions have credentials redacted on disk. If the
generated pipeline used key-based auth, the wizard flags it and offers an
identity-based rewrite (`ResourceId=`); declining leaves the file as-is and
`rigg validate` warns until fixed.

## 2. Push: `replace` verb

### Detection

- Registry: new `immutable_fields: &[&str]` per kind (KnowledgeSource:
  `["kind"]`; empty elsewhere for now).
- Plan building: a would-be `update` whose local/remote docs differ on an
  immutable field becomes `replace`.

### Plan display

```
  ~ replace  knowledge-sources/regulatory   kind: azureBlob → searchIndex
             ⚠ deletes the knowledge source and its generated index/indexer/
               data source/skillset, then recreates the pipeline explicitly.
               The index rebuilds from scratch: time, ingestion/embedding
               cost, and the source is unavailable until repopulated.
```

`--dry-run` and `rigg diff` surface the same verb + warning without mutating.

### Gates

- Interactive: dedicated default-No confirmation for replaces, separate from
  the normal push confirm.
- Non-interactive: `--allow-replace` required; `--yes` alone → error exit.
- Protected envs: existing `confirm_protected_env` applies additionally.

### Execution order

Normal creates/updates first, then each replace bundle sequentially, then
prunes. Per bundle:

1. **Snapshot referencing KBs** from the remote snapshot — *every* KB naming
   the KS in `knowledgeSources[]`, including KBs the project doesn't own.
   Foreign KBs are unlinked temporarily and restored byte-for-byte, with a
   printed notice.
2. **Write recovery file** `.rigg/<env>/<project>/replace-<ks>.json`: original
   KB docs + bundle plan.
3. **Unlink**: PUT each KB minus the KS entry. If `knowledgeSources` would be
   empty and the service rejects that, DELETE the KB (original in the recovery
   file). Which path Azure requires is verified live on `test-ks`; wiremock
   covers both.
4. **DELETE the old KS** (Azure cascades the generated sub-resources away).
5. **Create the new pipeline** in `graph::push_order` (dependencies before
   dependents — the indexer last, after its data source, index, and skillset;
   ties broken by registry declaration order), each PUT followed by GET-back
   canonicalization to disk
   + baseline. The indexer's initial run starts on creation.
6. **PUT the new `searchIndex` KS.**
7. **Relink**: restore each KB exactly as saved (PUT, or re-create if deleted
   in step 3).
8. Delete the recovery file; print that the index is repopulating and KBs
   return thin results until the indexer finishes.

Side-by-side migrations never enter this path (plain creates, no gate).

## 3. Failure recovery

Invariant: **after any partial failure, re-running `rigg push` completes the
migration.**

- Push checks for leftover `replace-*.json` recovery files at start and
  finishes pending relinks first.
- Failure before the KS delete: kind mismatch still detected → bundle resumes;
  unlink PUTs are idempotent.
- Failure after the KS delete: local resources classify as plain creates; the
  recovery file still knows which KBs to relink (essential for foreign KBs,
  whose original docs exist nowhere else).
- Every step's error states what completed and that re-running push resumes.

## 4. Edge cases

- KB in the same push plan that references the KS: pushed **after** relink, so
  the desired local doc wins over the temporary unlinked state.
- Multiple replaces in one push: bundles run sequentially, one recovery file
  each.
- `rigg validate`: warns on data-source files with redacted key-based
  credentials.
- Migrate command never guesses sub-resource names from patterns — only
  `createdResources` (portal conventions differ from rigg's scaffold suffixes).

## 5. Testing

- **rigg-core unit**: immutable-field diff detection; replace classification;
  side-by-side name derivation (prefix swap); generated-definition
  sanitization.
- **`crates/rigg/tests/sync.rs` (wiremock)**: full replace choreography with
  exact request-order assertions (unlink → KS delete → graph-order creates →
  KS create → relink); empty-KB delete/recreate path; foreign-KB restore;
  crash-resume from leftover recovery file; `--allow-replace` enforcement
  (non-interactive replace without it errors); side-by-side push gate-free.
- **`crates/rigg/tests/cli_surface.rs`**: `migrate knowledge-source` parsing,
  non-interactive behavior, remote-type/unowned-KS errors.
- **Live** (mklabsrch): full in-place flow on `test-ks`; one side-by-side
  round with cleanup. `regulatory` untouched — reserved for the user's manual
  test.

## 6. Docs & integration

- CHANGELOG entry; `rigg concepts`/`describe` text for the replace verb.
- `.claude/skills` updates: `rigg-guide` and `rigg-push` mention migration and
  `--allow-replace`.
- MCP `rigg_push` tool: new `allow_replace` boolean passed through as
  `--allow-replace`.

## Out of scope

- Copying index *documents* between indexes (backup/restore-style migration
  that would avoid re-running the skillset). Could be a future optimization
  for side-by-side; today the indexer rebuilds from source data.
- Automated side-by-side cutover (KB re-pointing + old-KS deletion) — manual
  by explicit decision.
