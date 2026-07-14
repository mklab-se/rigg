# Knowledge-Source Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rigg migrate knowledge-source <name>` (local file transformation) + a generic `replace` verb in `rigg push` that safely delete-and-recreates a knowledge source whose immutable `kind` changed, orchestrating temporary knowledge-base unlinking.

**Architecture:** Registry gains `immutable_fields` per kind. A new `rigg-core::migrate` module holds pure doc transforms (created-resources map, searchIndex KS shape, side-by-side name derivation). The CLI gains `commands/migrate.rs` (wizard, writes files only) and `push.rs` gains replace detection, a `--allow-replace` gate, and a bundle orchestrator with a `.rigg/<env>/<project>/replace-<ks>.json` recovery file.

**Tech Stack:** Rust workspace (rigg, rigg-core, rigg-client, rigg-diff), clap, inquire, wiremock + assert_cmd tests.

**Spec:** `docs/superpowers/specs/2026-07-14-ks-migrate-design.md`

## Global Constraints

- Pre-push verification: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` must all pass.
- Local files never contain secrets; `x-rigg-*` stripped before PUT; push canonicalization (GET-back after every PUT) is never skipped.
- `--yes` never satisfies the replace gate (mirrors `--confirm-env` philosophy).
- Sub-resource names come ONLY from the live KS `createdResources` — never derived by pattern.
- Live testing only on `test-ks` in mklabsrch; `regulatory` untouched.

---

### Task 1: Registry `immutable_fields`

**Files:**
- Modify: `crates/rigg-core/src/registry.rs`

**Interfaces:**
- Produces: `KindMeta.immutable_fields: &'static [&'static str]`; `pub fn immutable_diff(kind, local, remote) -> Vec<(&'static str, String, String)>` (path, remote value, local value; only differing, both-present-or-one-missing fields).

- [ ] Add `immutable_fields` to `KindMeta` (doc: "Fields the service will not change in place — a differing local value means the resource must be deleted and re-created (push shows `replace`)."). Set `&["kind"]` for `KnowledgeSource`, `&[]` for all others.
- [ ] Add extractor:

```rust
/// Immutable fields whose local and remote values differ — a non-empty
/// result means an in-place PUT cannot reconcile the two documents and the
/// resource must be replaced (delete + recreate). Missing-on-either-side
/// counts as a difference only when the other side has a value.
pub fn immutable_diff(
    kind: ResourceKind,
    local: &Value,
    remote: &Value,
) -> Vec<(&'static str, String, String)> {
    let mut out = Vec::new();
    for path in meta(kind).immutable_fields {
        let mut l = Vec::new();
        collect_path(local, path, &mut |v| l.push(v.clone()));
        let mut r = Vec::new();
        collect_path(remote, path, &mut |v| r.push(v.clone()));
        if l != r {
            let show = |vals: &[Value]| {
                vals.iter()
                    .map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            out.push((*path, show(&r), show(&l)));
        }
    }
    out
}
```

- [ ] Unit tests in registry `mod tests`: kind change detected (`azureBlob` vs `searchIndex` → one entry `("kind","azureBlob","searchIndex")`); same kind → empty; non-KS kind → empty.
- [ ] `cargo test -p rigg-core registry` passes; commit `feat(core): registry immutable_fields + immutable_diff`.

### Task 2: `rigg-core::migrate` doc transforms

**Files:**
- Create: `crates/rigg-core/src/migrate.rs`
- Modify: `crates/rigg-core/src/lib.rs` (add `pub mod migrate;`)

**Interfaces:**
- Produces:
  - `pub fn created_resources(ks_doc: &Value) -> BTreeMap<ResourceKind, String>` — kind → generated name from the doc's (nested) `createdResources`.
  - `pub fn to_search_index_ks(ks_doc: &Value, index_name: &str) -> Value` — `{name, kind:"searchIndex", description?, searchIndexParameters:{searchIndexName}}` preserving name/description.
  - `pub fn derive_names(old_ks: &str, new_ks: &str, created: &BTreeMap<ResourceKind, String>) -> BTreeMap<ResourceKind, String>` — prefix swap when the generated name starts with `old_ks`, else `{new_ks}-{index|indexer|datasource|skillset}`.
  - `pub fn is_indexed_with_created(ks_doc: &Value) -> bool`.

- [ ] Implement (reuse `registry::collect_created_resources` walk logic — refactor it to expose a per-doc variant rather than duplicating: change registry's private walker to call this module or move the walker here and have registry call it; keep `auto_created_by` behavior identical).
- [ ] Unit tests: nested `azureBlobParameters.createdResources` fixture (same shape as registry test) → 4 entries; name derivation prefix swap (`regulatory-index`/`regulatory`→`reg2` gives `reg2-index`) and non-prefix fallback (`weird-name` → `reg2-index` style default); `to_search_index_ks` preserves description, drops `azureBlobParameters`; ignores unknown createdResources members.
- [ ] `cargo test -p rigg-core migrate` passes; commit `feat(core): migrate doc transforms`.

### Task 3: `rigg migrate knowledge-source` command

**Files:**
- Modify: `crates/rigg/src/cli.rs` (Commands::Migrate + args + dispatch), `crates/rigg/src/commands/mod.rs` (`pub mod migrate;`)
- Create: `crates/rigg/src/commands/migrate.rs`
- Test: `crates/rigg/tests/cli_surface.rs` (arg/offline errors), `crates/rigg/tests/sync.rs` (wiremock behavior)

**CLI shape:**

```rust
/// Migrate a resource to an explicit, fully rigg-managed shape
Migrate {
    #[command(subcommand)]
    command: MigrateCommands,
},

#[derive(Subcommand)]
pub enum MigrateCommands {
    /// Convert an indexed knowledge source (azureBlob, azureSql, ...) to the
    /// explicit searchIndex kind, materializing its generated pipeline
    /// (data source, index, skillset, indexer) as project files
    #[command(alias = "ks")]
    KnowledgeSource(MigrateKsArgs),
}

#[derive(Args)]
pub struct MigrateKsArgs {
    /// Knowledge source to migrate
    pub name: String,
    /// Project owning the knowledge source
    #[arg(long, short = 'p')]
    pub project: Option<String>,
    /// In-place: keep all names; next push replaces the KS (index rebuild!)
    #[arg(long, conflicts_with = "rename")]
    pub in_place: bool,
    /// Side-by-side: create a parallel pipeline under this new KS name
    #[arg(long, value_name = "NEW-NAME")]
    pub rename: Option<String>,
}
```

**commands/migrate.rs flow (complete):**
1. `load_workspace`, `resolve_env`, project = `select_projects(&ws, args.project.as_deref(), false)?[0]` (reuse single-project selection), `Remote::for_project`, `ensure_any_connection`.
2. `remote.get(&ks_ref)` → None → error "knowledge source '<n>' not found remotely". `kind == "searchIndex"` → println "already searchIndex — nothing to migrate", Ok. `migrate::is_indexed_with_created` false → error "kind '<k>' has no generated pipeline (remote knowledge sources have nothing to migrate)".
3. Ownership: `store.locate(&ks_ref)?` is Some OR `state.has_baseline(&ks_ref)` else error suggesting `rigg adopt`.
4. Mode: flags, else interactive `interactive::select("Migration mode:", ["in-place (same names — next push REBUILDS the index)", "side-by-side (new names — old keeps serving until you cut over)"])`; non-interactive without flags → `CommandError::Usage`.
5. `let created = migrate::created_resources(&remote_ks)`; fetch each sub-resource doc `remote.get`; missing one → warn and skip it (it may have been deleted manually).
6. In-place: for each sub-doc `store.write(&r, &normalize_for_disk(...))` + `state.set_baseline` (they exist remotely with this content); rewrite KS file `store.write(&ks_ref, &migrate::to_search_index_ks(&remote_ks, &created[Index]))` (baseline NOT touched — stays azureBlob so status shows LocalAhead).
7. Side-by-side: names = `migrate::derive_names(...)`; interactive: `interactive::text` per name pre-filled hint with default (accept empty → default); validate each name unused locally (`store.locate`) and remotely (`remote.get`); write new sub-resources (renamed `name` field, indexer rewired: `dataSourceName`, `targetIndexName`, `skillsetName` remapped to the new names) and new KS (`to_search_index_ks` with new names); NO baselines (they're new). Old KS file untouched.
8. Credentials: after writing the data source file, if `credentials.connectionString` is null/missing/non-`ResourceId=`: interactive → offer `interactive::text("Storage connection (identity-based, e.g. ResourceId=/subscriptions/...):")`, write into file if given; always print warning otherwise.
9. Summary print + warnings (in-place: rebuild on push; side-by-side: next steps list).

- [ ] Write failing wiremock test `migrate_in_place_writes_explicit_pipeline` in sync.rs: mock KS GET (azureBlob with nested createdResources naming 4 resources), GETs for the 4 sub-docs, run `rigg migrate knowledge-source test-ks --in-place --yes` in a workspace where the KS file exists (write it first + push baseline via adopt-like: simplest — write the KS file before running; ownership check accepts file presence), assert: 4 new files exist with normalized content; KS file now `kind == "searchIndex"`, `searchIndexParameters.searchIndexName` = generated index name.
- [ ] Implement; test passes.
- [ ] Add sync.rs test `migrate_side_by_side_creates_new_files`: `--rename test-ks2` non-interactive → files `test-ks2.json` (searchIndex) + derived-name sub-resources with rewired indexer; old file untouched; remote-collision mock (GET 200 for one derived name) → command errors.
- [ ] Add cli_surface tests: `rigg migrate` without subcommand → usage error; `migrate knowledge-source x --in-place --rename y` → clap conflict error.
- [ ] `cargo test -p rigg` passes; commit `feat: rigg migrate knowledge-source command`.

### Task 4: Push replace detection, plan display, gates

**Files:**
- Modify: `crates/rigg/src/cli.rs` (PushArgs `--allow-replace`), `crates/rigg/src/commands/push.rs`
- Test: `crates/rigg/tests/sync.rs`

**Interfaces:**
- Produces: `struct ReplaceBundle { ks: ResourceRef, new_body: Value, remote_ks: Value, subresources: Vec<(ResourceRef, Value)>, kbs: Vec<Value> /* filled at exec */ }` (private to push.rs).

- [ ] During classification loop: when `kind == KnowledgeSource` and `remote_doc` is Some and `!registry::immutable_diff(r.kind, body, remote).is_empty()` → route into `replaces: Vec<ReplaceBundle>` instead of `to_push`. Bundle sub-resources: `migrate::created_resources(&remote_doc)` filtered to names having a local file in `items` — REMOVE those from `to_push` (they re-create inside the bundle regardless of their own SyncClass; note an InSync copy would otherwise be skipped and then lost to the cascade).
- [ ] Plan print after normal verbs:

```
  ~ replace knowledge-sources/test-ks   kind: azureBlob → searchIndex
      ⚠ deletes the knowledge source AND its generated pipeline, then
        recreates it explicitly. The index is REBUILT from source data:
        this takes time, costs ingestion/embeddings, and the source is
        unavailable to knowledge bases until repopulated.
      recreates: data-sources/test-ks-datasource, indexes/test-ks-index, ...
```

- [ ] Gates, after the protected-env gate: if replaces non-empty → interactive: `confirm_default_no("Proceed with N replace(s)? The index rebuild takes time and money.")`, decline → abort; non-interactive: `args.allow_replace` else `Err(CommandError::Usage("push plan contains replace(s); pass --allow-replace (in addition to --yes) to proceed"))`.
- [ ] Tests: `push_detects_kind_change_as_replace_dry_run` (dry-run output contains "replace" + "REBUILT", no mutating requests hit the mock); `push_replace_requires_allow_replace_flag` (`--yes` alone → exit 2, stderr mentions `--allow-replace`).
- [ ] `cargo test -p rigg --test sync` passes; commit `feat: push detects immutable-field change as replace (gated)`.

### Task 5: Push replace execution + recovery file

**Files:**
- Modify: `crates/rigg/src/commands/push.rs`
- Test: `crates/rigg/tests/sync.rs`

**Recovery file** `.rigg/<env>/<project>/replace-<ks>.json` (via `ws.state_dir`):

```json
{ "ks": "test-ks", "knowledge_bases": [ { ...original KB doc... } ] }
```

- [ ] Execution, after normal creates/updates, before prune — per bundle:
  1. `remote.list(KnowledgeBase)` → kbs referencing `ks.name` in `knowledgeSources[].name`. Print notice for each KB not owned by this project ("temporarily unlinking foreign knowledge base '<n>' — restored afterwards").
  2. Write recovery file (original docs).
  3. For each KB: build unlinked doc (filter the array); if array now empty → try PUT; on error → DELETE (both paths tested).
  4. `remote.delete(&ks_ref)`; `state.clear_baseline` for KS and each cascade-deleted sub-resource.
  5. PUT sub-resources in `graph::push_order`, canonicalize each (store.write + set_baseline + save).
  6. PUT new KS, canonicalize.
  7. Relink: for each saved KB, PUT the ORIGINAL doc (normalize_for_push'd); if the KB has a local file in this project, re-canonicalize it too.
  8. Remove recovery file. Print "index repopulating; knowledge bases may return thin results until the indexer finishes".
- [ ] Resume: at push start (after `ensure_any_connection`), glob `replace-*.json` in the state dir; for each: if the KS now exists remotely → relink its saved KBs (PUT), delete file, print "resumed: restored N knowledge base link(s)"; else keep the file and print that the replace will resume this run (bundle re-detection or plain creates handle the rest; relink retried at end of run — implement as: load leftover obligations into the run's relink queue).
- [ ] Error paths: any step failure → save state, print "replace of '<ks>' interrupted after <step>; re-run `rigg push` to resume (recovery file kept)". Return the error.
- [ ] Tests (wiremock, assert exact request order via `server.received_requests()`):
  - `push_replace_full_choreography`: KB referencing KS + another KS too (unlink keeps array non-empty). Assert order: PUT kb(unlinked) → DELETE ks → PUT ds → PUT idx → PUT ss → PUT idxr → PUT ks → PUT kb(original). Files canonicalized; recovery file gone; baselines updated.
  - `push_replace_empty_kb_falls_back_to_delete`: KB referencing only this KS; PUT of empty-array KB mocked 400 → DELETE kb, later re-created (PUT after new KS).
  - `push_replace_restores_foreign_kb`: KB not in project files; output mentions "foreign"; final PUT restores byte-identical `knowledgeSources`.
  - `push_resumes_after_crash`: pre-seed a recovery file + remote state where KS already searchIndex; push → relink PUT happens, file removed.
- [ ] `cargo test -p rigg --test sync` passes; commit `feat: push replace orchestration with KB unlink/relink + resume`.

### Task 6: diff notice, validate warning

**Files:**
- Modify: `crates/rigg/src/commands/diff.rs`, `crates/rigg/src/commands/validate.rs`
- Test: `crates/rigg/tests/sync.rs` (diff), validate unit/integration as per existing patterns

- [ ] diff (text format): when `immutable_diff` non-empty for a resource, append line: `note: 'kind' is immutable — push will REPLACE this resource (delete + recreate; for knowledge sources the index is rebuilt)`. Test: diff output contains "REPLACE".
- [ ] validate: data-source files whose `credentials.connectionString` is null/missing → warning (not error): "no credentials — push will fail; use identity-based ResourceId=..." (Follow validate's existing warning conventions.) Test per existing validate test patterns.
- [ ] Commit `feat: replace notices in diff and validate`.

### Task 7: MCP + docs

**Files:**
- Modify: `crates/rigg/src/mcp/tools.rs` (PushParams + rigg_push), `CHANGELOG.md`, `.claude/skills/rigg-guide/SKILL.md`, `.claude/skills/rigg-push/SKILL.md`, `crates/rigg/src/commands/concepts.rs` (if it enumerates verbs)

- [ ] `PushParams` gains `allow_replace: Option<bool>`; when `force` && `allow_replace` → push `--allow-replace`. Tool description sentence: "Plans containing a replace (e.g. knowledge-source kind change) additionally require allow_replace=true — the index is rebuilt."
- [ ] Skills + CHANGELOG (## Unreleased → migration feature, replace verb, --allow-replace).
- [ ] Full verification: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`. Commit `docs+mcp: knowledge-source migration surface`.

### Task 8: Live end-to-end on test-ks (mklabsrch)

- [ ] `cargo run --bin rigg --` in the live workspace (e2e-test/): pull current state; run `migrate knowledge-source test-ks --in-place`; inspect files; `push` with the gate (interactive-equivalent: `--yes --allow-replace`); verify in Azure: KS kind searchIndex, sub-resources exist, KB intact, indexer running. Verify a KS delete failure path was avoided (KB unlink worked).
- [ ] Side-by-side round: `migrate knowledge-source test-ks --rename test-ks-sxs` → push → verify → delete the sxs files + `push --prune` to clean up. Delete any leftovers. `regulatory` untouched.
- [ ] Fix anything found; commit fixes.

### Task 9: Release minor version

- [ ] Bump workspace `Cargo.toml` version → next minor (1.3.0) incl. internal dep versions; CHANGELOG section; commit `Release v1.3.0`; push main; `git tag v1.3.0 && git push origin v1.3.0`; watch release workflow to completion.

## Self-review

- Spec coverage: command UX (T3), replace detection/gates (T4), orchestration + recovery (T5), diff/validate (T6), MCP/docs (T7), live tests (T8), release (T9 per goal). Credential check in T3 step 8. Foreign-KB handling T5. ✓
- No placeholders; types consistent (`immutable_diff`, `created_resources`, `to_search_index_ks`, `derive_names`, `ReplaceBundle`). ✓
