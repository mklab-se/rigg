# Scoped `rigg adopt` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace all-or-nothing `pull --adopt` with a first-class `rigg adopt <project> <selector>...` verb that adopts exactly the selected unmanaged resources, with an optional `--with-deps` to also pull a resource's upstream dependency graph.

**Architecture:** A new `commands/adopt.rs` parses positional selectors (`all` | `<kind>` | `<kind>/<name>`) into a `Selector` enum, resolves them against the same remote snapshot `pull`/`status` use, filters by ownership (never adopting another project's resource), optionally expands upstream dependencies, confirms broad selections, and writes files + baselines exactly like the old adopt branch. `pull` loses `--adopt` entirely.

**Tech Stack:** Rust, clap 4.5 derive, existing `rigg-core` (`ResourceKind::from_directory_name`, `registry::extract_references`, `Store`, `ProjectState`), wiremock + assert_cmd tests.

## Global Constraints

- No backwards compatibility: `pull --adopt` is REMOVED, not aliased.
- `--with-deps` follows UPSTREAM references only (what a resource needs), never dependents.
- Adoption is Azure-read-only: writes local files + baselines, never mutates Azure.
- Ownership invariant: a resource owned by another project is NEVER adopted; an explicit `<kind>/<name>` naming one is a hard error (exit 1) naming the owner; a resource swept only by `<kind>`/`all`/deps is silently skipped.
- Exit codes: usage errors (bad/absent selector, unknown kind, broad-selector-in-non-interactive-without-`-y`) → 2; ownership conflict → 1.
- Broad selectors (`all` or bare `<kind>`) require confirmation (interactive) or `-y`; in non-interactive without `-y` they fail (exit 2). Specific `<kind>/<name>` selectors adopt directly.
- Selector `<kind>` uses resource DIRECTORY names (`indexes`, `data-sources`, `knowledge-bases`, ...), matching `rigg diff --only` / `describe` output.

---

### Task 1: `Selector` parser

**Files:**
- Create: `crates/rigg/src/commands/adopt.rs` (parser + unit tests only this task)
- Modify: `crates/rigg/src/commands/mod.rs` (add `pub mod adopt;`)

**Interfaces:**
- Produces: `pub enum Selector { All, Kind(ResourceKind), One(ResourceRef) }`
- Produces: `impl Selector { pub fn parse(s: &str) -> anyhow::Result<Selector>; pub fn is_broad(&self) -> bool }`
- Produces: `fn unknown_kind_msg(dir: &str) -> String`

- [ ] **Step 1: Create `crates/rigg/src/commands/adopt.rs`** with the parser and its tests:

```rust
//! `rigg adopt` — bring selected unmanaged remote resources into a project.

use anyhow::{Result, anyhow};

use rigg_core::resources::{ResourceKind, ResourceRef, validate_resource_name};

/// What the user asked to adopt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// Every unmanaged resource across both services.
    All,
    /// Every unmanaged resource of one kind.
    Kind(ResourceKind),
    /// One specific resource.
    One(ResourceRef),
}

impl Selector {
    pub fn parse(s: &str) -> Result<Selector> {
        if s == "all" {
            return Ok(Selector::All);
        }
        if let Some((dir, name)) = s.split_once('/') {
            let kind = ResourceKind::from_directory_name(dir)
                .ok_or_else(|| anyhow!(unknown_kind_msg(dir)))?;
            validate_resource_name(name)
                .map_err(|e| anyhow!("invalid resource name '{name}': {e}"))?;
            return Ok(Selector::One(ResourceRef::new(kind, name.to_string())));
        }
        let kind =
            ResourceKind::from_directory_name(s).ok_or_else(|| anyhow!(unknown_kind_msg(s)))?;
        Ok(Selector::Kind(kind))
    }

    /// Broad selectors (all / whole-kind) require confirmation before writing.
    pub fn is_broad(&self) -> bool {
        matches!(self, Selector::All | Selector::Kind(_))
    }
}

fn unknown_kind_msg(dir: &str) -> String {
    let kinds = ResourceKind::all()
        .iter()
        .map(|k| k.directory_name())
        .collect::<Vec<_>>()
        .join(", ");
    format!("unknown resource kind '{dir}'. Valid kinds: {kinds}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all() {
        assert_eq!(Selector::parse("all").unwrap(), Selector::All);
    }

    #[test]
    fn parses_bare_kind() {
        assert_eq!(
            Selector::parse("indexes").unwrap(),
            Selector::Kind(ResourceKind::Index)
        );
    }

    #[test]
    fn parses_kind_slash_name() {
        assert_eq!(
            Selector::parse("indexes/hotels").unwrap(),
            Selector::One(ResourceRef::new(ResourceKind::Index, "hotels".to_string()))
        );
    }

    #[test]
    fn unknown_kind_is_error_listing_kinds() {
        let err = Selector::parse("widgets").unwrap_err().to_string();
        assert!(err.contains("unknown resource kind 'widgets'"), "{err}");
        assert!(err.contains("indexes"), "lists valid kinds: {err}");
    }

    #[test]
    fn is_broad_classifies_correctly() {
        assert!(Selector::parse("all").unwrap().is_broad());
        assert!(Selector::parse("indexes").unwrap().is_broad());
        assert!(!Selector::parse("indexes/hotels").unwrap().is_broad());
    }
}
```

NOTE: verify the `ResourceKind` variant name for indexes is `ResourceKind::Index` by checking `crates/rigg-core/src/resources/traits.rs`; if the variant is named differently (e.g. `Index` vs `SearchIndex`), use the actual name in the tests.

- [ ] **Step 2: Register the module** in `crates/rigg/src/commands/mod.rs`, next to the other `pub mod` lines:

```rust
pub mod adopt;
```

- [ ] **Step 3: Run the unit tests**

Run: `cargo test -p rigg adopt:: 2>&1 | tail -20`
Expected: 5 tests pass. (They are written before the rest of the command exists; the module compiles because it has no other code yet.)

- [ ] **Step 4: fmt + clippy**

Run: `cargo fmt --all -- --check && cargo clippy -p rigg --all-targets -- -D warnings 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg/src/commands/adopt.rs crates/rigg/src/commands/mod.rs
git commit -m "feat: Selector parser for rigg adopt"
```

---

### Task 2: `rigg adopt` command (selectors, ownership, confirm, dry-run, JSON)

**Files:**
- Modify: `crates/rigg/src/commands/adopt.rs` (add `AdoptArgs`-driven `run`)
- Modify: `crates/rigg/src/cli.rs` (add `Adopt(AdoptArgs)` variant, `AdoptArgs`, dispatch)
- Test: `crates/rigg/tests/sync.rs`, `crates/rigg/tests/cli_surface.rs`

**Interfaces:**
- Consumes: `Selector` (Task 1), `Remote`/`ensure_any_connection` (commands::remote), `Store`, `ProjectState`, `resolve_env`, `load_workspace`, `assert_exclusive_ownership`, `confirm::prompt_yes_no`, `CommandError::Usage`.
- Produces: `pub async fn run(ctx: &GlobalContext, args: AdoptArgs) -> anyhow::Result<()>`
- Produces (cli.rs): `struct AdoptArgs { project: String, selectors: Vec<String>, dry_run: bool }` (the `with_deps` field is added in Task 3).

- [ ] **Step 1: Add `AdoptArgs` and the `Adopt` variant + dispatch** in `crates/rigg/src/cli.rs`.

In `enum Commands`, after the `Pull(PullArgs)` variant, add:

```rust
    /// Adopt selected unmanaged Azure resources into a project
    ///
    /// Selectors: `all`, a kind (e.g. `indexes`), or `<kind>/<name>`
    /// (e.g. `agents/regulus`). See `rigg concepts` for the project model.
    Adopt(AdoptArgs),
```

Add the args struct near `PullArgs`:

```rust
#[derive(Args)]
pub struct AdoptArgs {
    /// Project to adopt the resources into
    pub project: String,

    /// What to adopt: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`). Repeatable.
    #[arg(value_name = "SELECTOR")]
    pub selectors: Vec<String>,

    /// Preview what would be adopted; write nothing
    #[arg(long)]
    pub dry_run: bool,
}
```

Add the dispatch arm after the `Commands::Pull(...)` arm:

```rust
            Commands::Adopt(args) => commands::adopt::run(&ctx, args).await,
```

- [ ] **Step 2: Write the failing CLI-surface tests** in `crates/rigg/tests/cli_surface.rs` (append):

```rust
#[test]
fn adopt_help_lists_selectors() {
    rigg().args(["adopt", "--help"]).assert().success()
        .stdout(predicate::str::contains("SELECTOR"))
        .stdout(predicate::str::contains("agents/regulus"));
}

#[test]
fn adopt_requires_a_selector() {
    let ws = workspace();
    rigg().current_dir(ws.path()).args(["adopt", "demo"]).assert().code(2);
}

#[test]
fn adopt_rejects_unknown_kind() {
    let ws = workspace();
    rigg().current_dir(ws.path()).args(["adopt", "demo", "widgets"]).assert()
        .code(2)
        .stderr(predicate::str::contains("unknown resource kind"));
}
```

(The `pull --adopt` removal is tested in Task 4, where the flag is actually removed — keeping this task's suite green.)

- [ ] **Step 3: Write the failing sync tests** in `crates/rigg/tests/sync.rs` (append). These need multiple unmanaged remote resources. Add:

```rust
#[tokio::test]
async fn adopt_named_selector_adopts_only_that_resource() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "hotels", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "cars",   "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        }))).mount(&server).await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    rigg(ws.path()).args(["adopt", "demo", "indexes/hotels"]).assert().success();

    assert!(ws.path().join("projects/demo/search/indexes/hotels.json").exists());
    assert!(!ws.path().join("projects/demo/search/indexes/cars.json").exists(),
        "only the named resource is adopted");
}

#[tokio::test]
async fn adopt_kind_selector_needs_confirmation_and_yes_adopts_all_of_kind() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "hotels", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "cars",   "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        }))).mount(&server).await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    // broad selector, non-interactive (assert_cmd has no tty), no --yes → exit 2
    rigg(ws.path()).args(["adopt", "demo", "indexes"]).assert().code(2)
        .stderr(predicate::str::contains("--yes").or(predicate::str::contains("--dry-run")));
    assert!(!ws.path().join("projects/demo/search/indexes/hotels.json").exists());

    // with --yes → adopts all of the kind
    rigg(ws.path()).args(["adopt", "demo", "indexes", "--yes"]).assert().success();
    assert!(ws.path().join("projects/demo/search/indexes/hotels.json").exists());
    assert!(ws.path().join("projects/demo/search/indexes/cars.json").exists());
}

#[tokio::test]
async fn adopt_dry_run_writes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name":"hotels","fields":[{"name":"id","type":"Edm.String","key":true}]}]
        }))).mount(&server).await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    rigg(ws.path()).args(["adopt", "demo", "indexes/hotels", "--dry-run"]).assert().success();
    assert!(!ws.path().join("projects/demo/search/indexes/hotels.json").exists());
}
```

- [ ] **Step 4: Run the new tests to confirm they fail**

Run: `cargo test -p rigg --test sync adopt_ 2>&1 | tail -20` and `cargo test -p rigg --test cli_surface adopt_ 2>&1 | tail -20`
Expected: FAIL — `adopt` is not yet a real command (arg parses but `run` unimplemented / not compiling until Step 5).

- [ ] **Step 5: Implement `run` in `crates/rigg/src/commands/adopt.rs`.** Add these imports at the top (below the existing `use` lines):

```rust
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use rigg_core::store::{ProjectState, Store, assert_exclusive_ownership};

use crate::cli::AdoptArgs;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{CommandError, GlobalContext, confirm, load_workspace, resolve_env};
```

Then add the `run` function (and a small ordered-insert helper) below the `impl Selector` block:

```rust
pub async fn run(ctx: &GlobalContext, args: AdoptArgs) -> Result<()> {
    if args.selectors.is_empty() {
        return Err(anyhow!(CommandError::Usage(
            "name at least one selector: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`)"
                .to_string()
        )));
    }
    let selectors = args
        .selectors
        .iter()
        .map(|s| Selector::parse(s))
        .collect::<Result<Vec<_>>>()?;

    let ws = load_workspace()?;
    assert_exclusive_ownership(&ws)?;
    let env = resolve_env(&ws, ctx)?;
    let project = ws.project(&args.project)?;

    // Every resource key already owned by ANY project → its owner's name.
    let mut owned_by_any: BTreeMap<String, String> = BTreeMap::new();
    for p in &ws.projects {
        for (r, _) in Store::new(p).list()? {
            owned_by_any.insert(r.key(), p.name.clone());
        }
        let st = ProjectState::load(&ws, &env.name, &p.name);
        for k in st.baselines.keys() {
            owned_by_any.entry(k.clone()).or_insert_with(|| p.name.clone());
        }
    }

    let remote = Remote::for_project(&env, project);
    ensure_any_connection(&remote, project)?;
    let snapshot = remote.snapshot().await?;
    let snap_map: BTreeMap<String, (rigg_core::resources::ResourceRef, Value)> = snapshot
        .iter()
        .map(|(r, v)| (r.key(), (r.clone(), v.clone())))
        .collect();
    let supported = remote.supported_kinds();

    // Resolve selectors → ordered, unique candidate keys; track explicitly-named ones.
    let mut selected: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut explicit: BTreeSet<String> = BTreeSet::new();
    let mut push = |key: String, selected: &mut Vec<String>, seen: &mut BTreeSet<String>| {
        if seen.insert(key.clone()) {
            selected.push(key);
        }
    };
    for sel in &selectors {
        match sel {
            Selector::All => {
                for (r, _) in &snapshot {
                    push(r.key(), &mut selected, &mut seen);
                }
            }
            Selector::Kind(k) => {
                if !supported.contains(k) {
                    return Err(anyhow!(CommandError::Usage(format!(
                        "no connection for kind '{}' in environment '{}'",
                        k.directory_name(),
                        env.name
                    ))));
                }
                for (r, _) in &snapshot {
                    if r.kind == *k {
                        push(r.key(), &mut selected, &mut seen);
                    }
                }
            }
            Selector::One(rf) => {
                if !supported.contains(&rf.kind) {
                    return Err(anyhow!(CommandError::Usage(format!(
                        "no connection for kind '{}' in environment '{}'",
                        rf.kind.directory_name(),
                        env.name
                    ))));
                }
                explicit.insert(rf.key());
                push(rf.key(), &mut selected, &mut seen);
            }
        }
    }

    // Classify each candidate.
    let mut to_adopt: Vec<(rigg_core::resources::ResourceRef, Value)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for key in &selected {
        match snap_map.get(key) {
            None => {
                // Only reachable for an explicit One selector (kind sweeps come from snapshot).
                skipped.push((key.clone(), "no matching unmanaged remote resource".to_string()));
            }
            Some((r, doc)) => match owned_by_any.get(key) {
                Some(owner) if owner == &project.name => {
                    if explicit.contains(key) {
                        skipped.push((key.clone(), "already managed by this project".to_string()));
                    }
                }
                Some(owner) => {
                    if explicit.contains(key) {
                        return Err(anyhow!(
                            "{r} is owned by project '{owner}' — a resource belongs to exactly one project"
                        ));
                    }
                    // swept in by a kind/all selector → silently skip another project's resource
                }
                None => to_adopt.push((r.clone(), doc.clone())),
            },
        }
    }

    // Confirmation for broad selections.
    let broad = selectors.iter().any(Selector::is_broad);
    if !to_adopt.is_empty() && broad && !ctx.yes && !args.dry_run {
        if ctx.interactive() {
            println!("Would adopt {} resource(s) into '{}':", to_adopt.len(), project.name);
            for (r, _) in &to_adopt {
                println!("  {r}");
            }
            if !confirm::prompt_yes_no("Adopt these?")? {
                println!("Aborted.");
                return Ok(());
            }
        } else {
            return Err(anyhow!(CommandError::Usage(
                "broad selector in non-interactive mode: pass --yes to adopt, or --dry-run to preview"
                    .to_string()
            )));
        }
    }

    if args.dry_run {
        report(ctx, &to_adopt, &skipped, true)?;
        return Ok(());
    }

    let store = Store::new(project);
    let mut state = ProjectState::load(&ws, &env.name, &project.name);
    for (r, doc) in &to_adopt {
        store.write(r, doc)?;
        state.set_baseline(r, doc);
    }
    state.save(&ws, &env.name, &project.name)?;
    report(ctx, &to_adopt, &skipped, false)?;
    Ok(())
}

fn report(
    ctx: &GlobalContext,
    to_adopt: &[(rigg_core::resources::ResourceRef, Value)],
    skipped: &[(String, String)],
    dry_run: bool,
) -> Result<()> {
    if ctx.json() {
        let key = if dry_run { "would_adopt" } else { "adopted" };
        let value = json!({
            key: to_adopt.iter().map(|(r, _)| r.key()).collect::<Vec<_>>(),
            "skipped": skipped
                .iter()
                .map(|(k, why)| json!({ "resource": k, "reason": why }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if to_adopt.is_empty() {
        println!("Nothing to adopt (no unmanaged resources matched).");
    }
    for (r, _) in to_adopt {
        if dry_run {
            println!("  would adopt {r}");
        } else {
            println!("  + adopted {r}");
        }
    }
    for (k, why) in skipped {
        println!("  - skipped {k} ({why})");
    }
    Ok(())
}
```

NOTE: confirm `ResourceRef` implements `Display` (used as `{r}`). `pull.rs` prints refs the same way (`println!("  {} adopted {}", ..., r)`), so it does. Confirm the exact `resolve_env` / `ws.project` / `Remote::for_project` signatures against `pull.rs:34-72` — they are used identically there.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rigg --test sync adopt_ 2>&1 | tail -20`
Expected: 3 sync tests pass.
Run: `cargo test -p rigg --test cli_surface adopt_ 2>&1 | tail -20`
Expected: `adopt_help_lists_selectors`, `adopt_requires_a_selector`, `adopt_rejects_unknown_kind` all pass.

- [ ] **Step 7: fmt + clippy + focused suite**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 8: Commit**

```bash
git add crates/rigg/src/commands/adopt.rs crates/rigg/src/cli.rs \
        crates/rigg/tests/sync.rs crates/rigg/tests/cli_surface.rs
git commit -m "feat: rigg adopt command with scoped selectors, confirm, dry-run"
```

---

### Task 3: `--with-deps` upstream dependency expansion

**Files:**
- Modify: `crates/rigg/src/cli.rs` (`AdoptArgs` gains `with_deps`)
- Modify: `crates/rigg/src/commands/adopt.rs` (expand deps; tag them)
- Test: `crates/rigg/tests/sync.rs`

**Interfaces:**
- Consumes: `registry::extract_references(kind, &Value) -> Vec<(ResourceKind, String)>`.
- Changes: `run` honors `args.with_deps`; `report` tags dependency-sourced refs.

- [ ] **Step 1: Add the flag** to `AdoptArgs` in `crates/rigg/src/cli.rs` (after `dry_run`):

```rust
    /// Also adopt each selected resource's upstream dependencies
    #[arg(long)]
    pub with_deps: bool,
```

- [ ] **Step 2: Write the failing test** in `crates/rigg/tests/sync.rs` (append). An index that an indexer depends on; adopting the indexer with `--with-deps` should also adopt the index and data source, but a second unrelated index should NOT be adopted:

```rust
#[tokio::test]
async fn adopt_with_deps_pulls_upstream_chain_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/indexers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "docs-indexer",
                "dataSourceName": "docs-ds",
                "targetIndexName": "docs-index"
            }]
        }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "docs-index", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "unrelated",  "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/datasources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs-ds", "type": "azureblob",
                       "container": {"name": "c"},
                       "credentials": {"connectionString": "ResourceId=/x;"}}]
        }))).mount(&server).await;
    // remaining kinds empty
    for p in ["skillsets","synonymmaps","aliases","knowledgeSources","knowledgeBases"] {
        Mock::given(method("GET")).and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server).await;
    }

    let ws = workspace(&server.uri());
    rigg(ws.path()).args(["adopt", "demo", "indexers/docs-indexer", "--with-deps"]).assert().success();

    let base = ws.path().join("projects/demo/search");
    assert!(base.join("indexers/docs-indexer.json").exists(), "the named resource");
    assert!(base.join("indexes/docs-index.json").exists(), "referenced index (dependency)");
    assert!(base.join("data-sources/docs-ds.json").exists(), "referenced data source (dependency)");
    assert!(!base.join("indexes/unrelated.json").exists(), "unrelated resource NOT adopted");
}
```

NOTE: verify the data-source scaffold's required fields against `crates/rigg-core/src/registry.rs` / existing sync tests; if the mock doc is rejected by normalization, mirror the minimal shape another sync test uses for `datasources`. Verify `registry::extract_references` returns the indexer's `dataSourceName` and `targetIndexName` as `(DataSource, "docs-ds")` and `(Index, "docs-index")` by reading its implementation; adjust field names in the mock to whatever the extractor reads.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p rigg --test sync adopt_with_deps 2>&1 | tail -20`
Expected: FAIL — only `docs-indexer.json` written; dependencies missing.

- [ ] **Step 4: Implement expansion.** In `crates/rigg/src/commands/adopt.rs`, immediately AFTER the candidate-classification loop that builds `to_adopt` (before the confirmation block), insert:

```rust
    // Optionally pull each candidate's upstream dependency graph.
    let mut dep_keys: BTreeSet<String> = BTreeSet::new();
    if args.with_deps {
        let mut in_set: BTreeSet<String> = to_adopt.iter().map(|(r, _)| r.key()).collect();
        let mut queue: Vec<(rigg_core::resources::ResourceRef, Value)> = to_adopt.clone();
        while let Some((r, doc)) = queue.pop() {
            for (dk, dn) in registry::extract_references(r.kind, &doc) {
                let dref = rigg_core::resources::ResourceRef::new(dk, dn);
                let key = dref.key();
                if in_set.contains(&key) || owned_by_any.contains_key(&key) {
                    continue; // already selected, or owned by someone → not an unmanaged dep
                }
                if let Some((rr, dv)) = snap_map.get(&key) {
                    in_set.insert(key.clone());
                    dep_keys.insert(key.clone());
                    to_adopt.push((rr.clone(), dv.clone()));
                    queue.push((rr.clone(), dv.clone()));
                }
            }
        }
    }
```

Add `use rigg_core::registry;` to the imports if not already present.

Then update the text branch of `report` to tag dependencies. Change its signature to accept `dep_keys` and mark them. Replace the `report` call sites and the text loop:

- Change `fn report(ctx, to_adopt, skipped, dry_run)` to `fn report(ctx, to_adopt, skipped, dep_keys: &BTreeSet<String>, dry_run)`.
- In the text loop, compute `let tag = if dep_keys.contains(&r.key()) { " (dependency)" } else { "" };` and append `{tag}` to the printed line.
- In the JSON branch, add `"dependencies": dep_keys.iter().cloned().collect::<Vec<_>>()`.
- Update both `report(...)` calls to pass `&dep_keys`.

(For the dry-run call, `dep_keys` is computed the same way since expansion runs before the dry-run check.)

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p rigg --test sync adopt_with_deps 2>&1 | tail -20`
Expected: PASS.

- [ ] **Step 6: fmt + clippy + full adopt tests**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test -p rigg adopt 2>&1 | tail -8`
Expected: clean; all adopt unit + sync tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/rigg/src/cli.rs crates/rigg/src/commands/adopt.rs crates/rigg/tests/sync.rs
git commit -m "feat: rigg adopt --with-deps upstream dependency expansion"
```

---

### Task 4: Remove `pull --adopt`; update hints; migrate old test

**Files:**
- Modify: `crates/rigg/src/cli.rs` (remove `PullArgs.adopt`)
- Modify: `crates/rigg/src/commands/pull.rs` (remove adopt branch; update unmanaged hint)
- Modify: `crates/rigg/src/commands/status.rs` (update unmanaged hint)
- Modify: `crates/rigg/tests/sync.rs` (migrate the seeding test off `pull --adopt`)

**Interfaces:** none new. `pull` no longer adopts.

- [ ] **Step 1: Remove the flag** from `PullArgs` in `crates/rigg/src/cli.rs` — delete the `adopt` field:

```rust
    /// Adopt unmanaged remote resources into the given project
    #[arg(long, value_name = "PROJECT")]
    pub adopt: Option<String>,
```

- [ ] **Step 2: Remove the adopt branch** in `crates/rigg/src/commands/pull.rs`. Delete the `let adopting = args.adopt.as_deref() == Some(project.name.as_str());` line (pull.rs:90). In the `if !owned_by_this { ... }` block (pull.rs:102-111), remove the `if adopting { ... } else { unmanaged += 1; }` split so it always counts unmanaged:

```rust
        if !owned_by_this {
            unmanaged += 1;
            continue;
        }
```

- [ ] **Step 3: Update the unmanaged hint** in `crates/rigg/src/commands/pull.rs` (pull.rs:194-201). Replace the message so it points at the new verb:

```rust
    if unmanaged > 0 {
        println!(
            "  {} {unmanaged} unmanaged remote resource(s) — adopt with `rigg adopt {} <selector>` (e.g. `all`, `indexes`, `agents/name`)",
            "i".blue(),
            project.name,
        );
    }
```

- [ ] **Step 4: Update the status hint** in `crates/rigg/src/commands/status.rs` (the line `println!("    adopt with: rigg pull <project> --adopt <project>");`). Replace with:

```rust
            println!("    adopt with: rigg adopt <project> <selector>  (e.g. all, indexes, agents/name)");
```

- [ ] **Step 5: Migrate the seeding test** in `crates/rigg/tests/sync.rs`. In `pull_writes_normalized_files_and_skips_volatile_noise`, replace the adopt-seeding call:

```rust
    rigg(ws.path())
        .args(["pull", "demo", "--adopt", "demo", "--yes"])
        .assert()
        .success();
```

with:

```rust
    rigg(ws.path())
        .args(["adopt", "demo", "indexes/docs"])
        .assert()
        .success();
```

- [ ] **Step 5b: Add the flag-removal test** in `crates/rigg/tests/cli_surface.rs` (append):

```rust
#[test]
fn pull_adopt_flag_is_gone() {
    rigg().args(["pull", "--adopt", "demo"]).assert().code(2);
}
```

- [ ] **Step 6: Run the affected tests**

Run: `cargo test -p rigg --test cli_surface pull_adopt_flag_is_gone 2>&1 | tail -6`
Expected: PASS (flag removed).
Run: `cargo test -p rigg --test sync 2>&1 | tail -12`
Expected: all sync tests pass, including the migrated one.

- [ ] **Step 7: fmt + clippy + full suite**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -E 'test result: (ok|FAILED)'`
Expected: clean; zero failures.

- [ ] **Step 8: Commit**

```bash
git add crates/rigg/src/cli.rs crates/rigg/src/commands/pull.rs \
        crates/rigg/src/commands/status.rs crates/rigg/tests/sync.rs
git commit -m "feat!: remove pull --adopt in favor of rigg adopt; update hints"
```

---

### Task 5: Docs — README & GETTING_STARTED

**Files:**
- Modify: `README.md`
- Modify: `GETTING_STARTED.md`

**Interfaces:** none (docs only). Verified by reading + grep.

- [ ] **Step 1: Update README adopt references.** In `README.md`, replace the two adopt mentions.

Quick Start (`README.md` around line 82) — replace:

```markdown
# Adopt existing Azure resources into it…
rigg pull my-rag --adopt my-rag
```

with:

```markdown
# Adopt existing Azure resources into it — à la carte…
rigg adopt my-rag all                 # everything unmanaged
rigg adopt my-rag agents/my-agent     # just one resource
rigg adopt my-rag indexes --with-deps # a whole kind + its dependencies
```

In the `### Whole-Project Sync` command block (around `README.md:174`), replace the line:

```markdown
rigg pull --adopt my-rag        # adopt unmanaged remote resources into the project
```

with:

```markdown
rigg adopt my-rag <selector>    # adopt selected unmanaged resources (all | <kind> | <kind>/<name>)
```

- [ ] **Step 2: Update GETTING_STARTED.** In `GETTING_STARTED.md`, find the line:

```markdown
- **Existing resources?** — `rigg pull --adopt <project>` brings unmanaged Azure resources into a project
```

Replace with:

```markdown
- **Existing resources?** — `rigg adopt <project> <selector>` brings selected unmanaged Azure resources into a project (a single `<kind>/<name>`, a whole `<kind>`, or `all`; add `--with-deps` to also pull a resource's dependencies)
```

- [ ] **Step 3: Verify no stale references remain**

Run: `grep -rn "pull --adopt\|pull .*--adopt" README.md GETTING_STARTED.md || echo "no stale pull --adopt references"`
Expected: prints the "no stale" message.

- [ ] **Step 4: Commit**

```bash
git add README.md GETTING_STARTED.md
git commit -m "docs: document rigg adopt selectors, replace pull --adopt"
```

---

## Final Verification (before declaring complete)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
grep -rn "adopt" crates/rigg/src/commands/pull.rs || echo "pull.rs no longer references adopt"
```

Expected: all green; `pull.rs` has no `adopt` logic left (only the hint text mentioning `rigg adopt`).

## Self-Review notes

- **Spec coverage:** selectors → Task 1 + Task 2 Step 5; `all`/kind/named + ownership + confirm + dry-run + JSON → Task 2; `--with-deps` upstream-only → Task 3; remove `pull --adopt` + hints → Task 4; docs → Task 5. Tests 1-11 from the spec map to Task 2 (2,3,5,6,7,8,10,11), Task 3 (4), Task 4 (9 via `pull_adopt_flag_is_gone`).
- **Type consistency:** `Selector`/`Selector::parse`/`is_broad` defined Task 1, used Task 2; `run(ctx, AdoptArgs)` signature stable; `report` signature changes in Task 3 (adds `dep_keys`) — both call sites updated in the same task.
- **Ordering safety:** every task leaves `cargo test --workspace` green. The `pull_adopt_flag_is_gone` test is added in Task 4, the same task that removes the flag, so no task ever ships a red suite.
