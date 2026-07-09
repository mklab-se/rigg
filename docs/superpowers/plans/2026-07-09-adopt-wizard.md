# Interactive Adopt Wizard Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rigg adopt` with missing arguments becomes an interactive wizard — pick the project, pick unmanaged resources queried live from Azure (both services), opt into dependencies, confirm — while scriptable/CI behavior stays byte-identical.

**Architecture:** A thin `commands/interactive.rs` wraps `inquire` (already in the dep tree at 0.7.5 via ailloy). `adopt.rs::run` is reordered so workspace/snapshot load can precede selector acquisition; wizard steps fill in missing args when interactive. Pure helpers (`wizard_candidates`, `expand_deps`, `equivalent_command`) carry the logic and get unit tests; the TTY layer stays paper-thin.

**Tech Stack:** Rust, clap 4.5, `inquire 0.7` (Select/MultiSelect/Confirm/Text), existing rigg-core machinery.

## Global Constraints

- Non-interactive / `--output json` / `-y` behavior is UNCHANGED: bare `rigg adopt` or missing selectors → usage error exit 2; all Workstream B semantics intact. The empty-selector usage check MUST run before any workspace/network access in non-wizard mode (cli_surface tests run without a mock server).
- Wizard activation: `ctx.interactive() && !ctx.json() && (project missing || selectors empty)`.
- The wizard produces the same resolved set and runs the same classification/write path as the CLI — no second adoption code path.
- Multi-select items are domain-sorted and domain-prefixed (`[Foundry] agents/x`, `[Search] indexes/y`) with a service legend above the prompt (inquire cannot render non-selectable headers). Foundry groups before Search; within a domain sort by kind directory then name.
- If a configured service is unreachable, FAIL naming the service — never show a partial menu. (Already the behavior: `snapshot()` errors carry the failing kind's context; do not add fallback.)
- Esc/Ctrl-C in any prompt → clean abort, exit 1, nothing written.
- Wizard mode ALWAYS shows preview + confirm before writing (dep expansion can add unticked resources).
- After a successful wizard adoption, print the equivalent scriptable command.
- `inquire = "0.7"` added as a workspace dep; honors `--no-color` via `RenderConfig::empty()`.

---

### Task 1: `interactive.rs` prompt layer + inquire dependency

**Files:**
- Create: `crates/rigg/src/commands/interactive.rs`
- Modify: `Cargo.toml` (workspace deps), `crates/rigg/Cargo.toml`
- Modify: `crates/rigg/src/commands/mod.rs` (register module)

**Interfaces:**
- Produces: `pub fn select(prompt: &str, options: Vec<String>, plain: bool) -> Result<String>`
- Produces: `pub fn multi_select(prompt: &str, options: Vec<String>, plain: bool) -> Result<Vec<usize>>`
- Produces: `pub fn confirm_default_yes(prompt: &str, plain: bool) -> Result<bool>`
- Produces: `pub fn confirm_default_no(prompt: &str, plain: bool) -> Result<bool>`
- Produces: `pub fn text(prompt: &str, plain: bool) -> Result<String>`

- [ ] **Step 1: Add the dependency.** Root `Cargo.toml`, under `# Terminal markdown rendering`, add:

```toml
# Interactive prompts (already in-tree via ailloy config-tui)
inquire = "0.7"
```

`crates/rigg/Cargo.toml`, after `termimad.workspace = true`:

```toml
inquire.workspace = true
```

- [ ] **Step 2: Create `crates/rigg/src/commands/interactive.rs`:**

```rust
//! Thin wrappers around `inquire` prompts: consistent styling, `--no-color`
//! support, and clean abort (Esc/Ctrl-C → error, nothing written).

use anyhow::{Result, anyhow};
use inquire::ui::RenderConfig;
use inquire::{Confirm, InquireError, MultiSelect, Select, Text};

fn config(plain: bool) -> RenderConfig<'static> {
    if plain {
        RenderConfig::empty()
    } else {
        RenderConfig::default_colored()
    }
}

fn map_err(e: InquireError) -> anyhow::Error {
    match e {
        InquireError::OperationCanceled | InquireError::OperationInterrupted => {
            anyhow!("aborted")
        }
        other => anyhow!(other),
    }
}

pub fn select(prompt: &str, options: Vec<String>, plain: bool) -> Result<String> {
    Select::new(prompt, options)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}

/// Returns the indices of the chosen options (order of the input list).
pub fn multi_select(prompt: &str, options: Vec<String>, plain: bool) -> Result<Vec<usize>> {
    let indexed: Vec<String> = options;
    let chosen = MultiSelect::new(prompt, indexed.clone())
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)?;
    Ok(chosen
        .into_iter()
        .filter_map(|c| indexed.iter().position(|o| *o == c))
        .collect())
}

pub fn confirm_default_yes(prompt: &str, plain: bool) -> Result<bool> {
    Confirm::new(prompt)
        .with_default(true)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}

pub fn confirm_default_no(prompt: &str, plain: bool) -> Result<bool> {
    Confirm::new(prompt)
        .with_default(false)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}

pub fn text(prompt: &str, plain: bool) -> Result<String> {
    Text::new(prompt)
        .with_render_config(config(plain))
        .prompt()
        .map_err(map_err)
}
```

NOTE: check inquire 0.7's actual API — `RenderConfig::default_colored()` and `RenderConfig::empty()` exist in 0.7.x; if names differ, adapt minimally. The module will be dead code until Task 2 wires it: add `#![allow(dead_code)]` with a `// TODO(task 2)` comment OR (preferred) land Tasks 1+2 in one commit sequence where clippy runs at the end of Task 2. Since each task commits separately and clippy must pass per task, add the module-level allow with a comment and REMOVE it in Task 2.

- [ ] **Step 3: Register the module** in `crates/rigg/src/commands/mod.rs`:

```rust
pub mod interactive;
```

- [ ] **Step 4: Verify build + lint**

Run: `cargo build -p rigg 2>&1 | tail -3 && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/rigg/Cargo.toml crates/rigg/src/commands/interactive.rs crates/rigg/src/commands/mod.rs
git commit -m "feat: interactive prompt layer wrapping inquire"
```

---

### Task 2: Wizard orchestration in `adopt.rs`

**Files:**
- Modify: `crates/rigg/src/cli.rs` (`AdoptArgs.project` → `Option<String>`)
- Modify: `crates/rigg/src/commands/adopt.rs` (reorder `run`; wizard steps; pure helpers + unit tests)
- Modify: `crates/rigg/src/commands/new.rs` (make project creation callable: `pub fn create_project`)
- Modify: `crates/rigg/src/commands/interactive.rs` (remove the dead-code allow)
- Test: `crates/rigg/tests/cli_surface.rs` (non-interactive contract pinned)

**Interfaces:**
- Consumes: Task 1's `interactive::{select, multi_select, confirm_default_yes, confirm_default_no, text}`.
- Produces (adopt.rs, pure, unit-tested):
  - `fn wizard_candidates(snapshot: &[(ResourceRef, Value)], owned_by_any: &BTreeMap<String, String>) -> Vec<(ResourceRef, String)>` — unmanaged only, sorted Foundry-first then Search, by (kind directory, name) within domain; label = `"[Foundry] agents/x"` / `"[Search] indexes/y"`.
  - `fn expand_deps(to_adopt: &[(ResourceRef, Value)], owned_by_any: &BTreeMap<String, String>, snap_map: &BTreeMap<String, (ResourceRef, Value)>) -> (Vec<(ResourceRef, Value)>, BTreeSet<String>)` — the additions and their keys (extracted from today's inline loop, same algorithm).
  - `fn equivalent_command(project: &str, chosen: &[String], with_deps: bool) -> String` — e.g. `rigg adopt regulus agents/regulus --with-deps`.
- Produces (new.rs): `pub fn create_project(ws: &Workspace, name: &str) -> Result<()>` — extracted from `new_project` (which becomes a thin wrapper doing `load_workspace()` + call).

- [ ] **Step 1: Make the project argument optional.** In `crates/rigg/src/cli.rs`, `AdoptArgs`:

```rust
    /// Project to adopt the resources into (omit on a TTY for an interactive wizard)
    pub project: Option<String>,
```

- [ ] **Step 2: Extract `create_project` in `new.rs`.** Split the existing `new_project`:

```rust
fn new_project(name: &str) -> Result<()> {
    let ws = load_workspace()?;
    create_project(&ws, name)?;
    println!("Add resources with: rigg new <kind> <name> -p {name}");
    Ok(())
}

/// Create the project directory + manifest. Shared with the adopt wizard.
pub fn create_project(ws: &Workspace, name: &str) -> Result<()> {
    rigg_core::resources::validate_resource_name(name)?;
    let dir = ws.root.join(PROJECTS_DIR).join(name);
    if dir.exists() {
        bail!("project '{name}' already exists");
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(PROJECT_FILE),
        format!(
            "# Rigg project: {name}\n# The files in this directory ARE the project membership.\ndescription: \"\"\n"
        ),
    )?;
    println!("Created project '{}' at {}", name.bold(), dir.display());
    Ok(())
}
```

(Note: `validate_resource_name` is a new guard — project names share resource-name rules; import it. Task 3 documents this. The "Add resources" line stays in `new_project` only; Task 3 rewrites it.)

- [ ] **Step 3: Write the failing/pinning tests.** Append to `crates/rigg/tests/cli_surface.rs`:

```rust
#[test]
fn adopt_without_project_non_interactive_is_usage_error() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .arg("adopt")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("interactive").or(predicate::str::contains("project")));
}

#[test]
fn adopt_project_without_selector_non_interactive_still_usage_error() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["adopt", "demo"])
        .assert()
        .code(2);
}
```

And unit tests inside `adopt.rs`'s `mod tests` (they exercise the pure helpers directly):

```rust
    use serde_json::json;
    use std::collections::BTreeMap;

    fn snap() -> Vec<(ResourceRef, serde_json::Value)> {
        vec![
            (
                ResourceRef::new(ResourceKind::Index, "docs".into()),
                json!({"name": "docs"}),
            ),
            (
                ResourceRef::new(ResourceKind::Agent, "regulus".into()),
                json!({"name": "regulus"}),
            ),
            (
                ResourceRef::new(ResourceKind::Agent, "helper".into()),
                json!({"name": "helper"}),
            ),
        ]
    }

    #[test]
    fn wizard_candidates_filters_owned_and_groups_foundry_first() {
        let mut owned = BTreeMap::new();
        owned.insert("agents/helper".to_string(), "other".to_string());
        let items = wizard_candidates(&snap(), &owned);
        let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(labels, vec!["[Foundry] agents/regulus", "[Search] indexes/docs"]);
    }

    #[test]
    fn equivalent_command_reconstructs_invocation() {
        assert_eq!(
            equivalent_command("regulus", &["agents/regulus".into()], true),
            "rigg adopt regulus agents/regulus --with-deps"
        );
        assert_eq!(
            equivalent_command("p", &["indexes/a".into(), "indexes/b".into()], false),
            "rigg adopt p indexes/a indexes/b"
        );
    }
```

NOTE: verify `ResourceKind::Agent` is the real variant name (check `traits.rs`); adjust if different.

- [ ] **Step 4: Run to verify state**

Run: `cargo test -p rigg --test cli_surface adopt_ 2>&1 | tail -12` and `cargo test -p rigg wizard_ 2>&1 | tail -8`
Expected: the two new cli_surface tests — `adopt_without_project…` FAILS to compile or fails until `project` is optional and the usage message exists; unit tests FAIL (helpers undefined).

- [ ] **Step 5: Reorder `run` and add the wizard.** Restructure `adopt.rs::run` to this shape (existing logic blocks are MOVED, not rewritten — classification, expansion, broad-confirm, write, report all stay as they are today except where noted):

```rust
pub async fn run(ctx: &GlobalContext, args: AdoptArgs) -> Result<()> {
    // Parse any given selectors first — cheap, and usage errors must not
    // require a workspace or network.
    let mut selectors = args
        .selectors
        .iter()
        .map(|s| Selector::parse(s).map_err(|e| anyhow!(CommandError::Usage(e.to_string()))))
        .collect::<Result<Vec<_>>>()?;

    let wizard = ctx.interactive() && !ctx.json() && (args.project.is_none() || selectors.is_empty());
    if !wizard {
        if args.project.is_none() {
            return Err(anyhow!(CommandError::Usage(
                "name a project (rigg adopt <project> <selector>...), or run on an interactive terminal for the wizard".to_string()
            )));
        }
        if selectors.is_empty() {
            return Err(anyhow!(CommandError::Usage(
                "name at least one selector: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`)".to_string()
            )));
        }
    }

    let ws = load_workspace()?;
    assert_exclusive_ownership(&ws)?;
    let env = resolve_env(&ws, ctx)?;
    let plain = ctx.no_color;

    // ---- Wizard step 1: project ----
    let project_name = match &args.project {
        Some(p) => p.clone(),
        None => match ws.projects.len() {
            0 => {
                println!("No projects yet — a project groups the resources you manage together.");
                if !interactive::confirm_default_yes("Create one now?", plain)? {
                    return Err(anyhow!("aborted"));
                }
                let name = interactive::text("Project name (e.g. the agent or app it will own):", plain)?;
                crate::commands::new::create_project(&ws, &name)?;
                // Reload so ws.project() sees it.
                drop(ws);
                return Box::pin(run(
                    ctx,
                    AdoptArgs { project: Some(name), selectors: args.selectors.clone(), dry_run: args.dry_run, with_deps: args.with_deps },
                ))
                .await;
            }
            1 => {
                let name = ws.projects[0].name.clone();
                println!("Using project '{name}' (the only project in this workspace).");
                name
            }
            _ => interactive::select(
                "Adopt into which project?",
                ws.projects.iter().map(|p| p.name.clone()).collect(),
                plain,
            )?,
        },
    };
    let project = ws.project(&project_name)?;

    // ---- ownership map, remote, snapshot: EXACTLY as today ----
    // (owned_by_any loop, Remote::for_project, ensure_any_connection,
    //  snapshot, snap_map, supported — unchanged)

    // ---- Wizard step 2: resources ----
    let mut wizard_chosen: Vec<String> = Vec::new(); // keys, for the hint
    if selectors.is_empty() {
        let candidates = wizard_candidates(&snapshot, &owned_by_any);
        if candidates.is_empty() {
            println!("Nothing to adopt — everything visible is already managed.");
            return Ok(());
        }
        // Service legend
        if remote.has_foundry() {
            println!("Foundry: unmanaged resources from the configured account/project");
        }
        if remote.has_search() {
            println!("Search:  unmanaged resources from the configured service");
        }
        let labels: Vec<String> = candidates.iter().map(|(_, l)| l.clone()).collect();
        let picked = interactive::multi_select(
            "Select resources to adopt (space toggles, type to filter):",
            labels,
            plain,
        )?;
        if picked.is_empty() {
            println!("Nothing selected.");
            return Ok(());
        }
        for i in picked {
            let (r, _) = &candidates[i];
            wizard_chosen.push(r.key());
            selectors.push(Selector::One(r.clone()));
        }
    }

    // ---- selector resolution + classification: EXACTLY as today ----

    // ---- dependency expansion (shared helper) ----
    let mut dep_keys: BTreeSet<String> = BTreeSet::new();
    let mut with_deps = args.with_deps;
    if with_deps {
        let (adds, keys) = expand_deps(&to_adopt, &owned_by_any, &snap_map);
        to_adopt.extend(adds);
        dep_keys = keys;
    } else if wizard {
        let (adds, keys) = expand_deps(&to_adopt, &owned_by_any, &snap_map);
        if !adds.is_empty()
            && interactive::confirm_default_no(
                &format!("Also adopt their {} upstream dependency(ies)?", adds.len()),
                plain,
            )?
        {
            with_deps = true;
            to_adopt.extend(adds);
            dep_keys = keys;
        }
    }

    // ---- confirmation ----
    // Wizard: ALWAYS preview + confirm (unless dry-run). Non-wizard: existing broad gate.
    if wizard && !to_adopt.is_empty() && !args.dry_run {
        println!("Will adopt {} resource(s) into '{}':", to_adopt.len(), project.name);
        for (r, _) in &to_adopt {
            let tag = if dep_keys.contains(&r.key()) { " (dependency)" } else { "" };
            println!("  {r}{tag}");
        }
        if !interactive::confirm_default_yes("Proceed?", plain)? {
            println!("Aborted.");
            return Ok(());
        }
    } else {
        // existing broad-selector gate, unchanged
    }

    // ---- dry-run / write / report: EXACTLY as today ----

    // ---- teach the scriptable form ----
    if wizard && !wizard_chosen.is_empty() && !args.dry_run {
        println!();
        println!("hint: next time: {}", equivalent_command(&project.name, &wizard_chosen, with_deps));
    }
    Ok(())
}
```

Implementation notes (bind these exactly):
- `wizard_candidates` sorts by `(domain_rank, kind.directory_name(), name)` where `domain_rank` = 0 for `Domain::FoundryData | Domain::FoundryArm`, 1 for `Domain::Search` (via `registry::meta(kind).domain`); label prefix `[Foundry] ` / `[Search] `.
- The zero-project recursion happens at most once (after creation, `args.project` is `Some`). `Box::pin` is required for async recursion.
- The existing with-deps inline loop is REPLACED by the `expand_deps` helper (same algorithm, `to_adopt` passed by ref, additions returned) — the non-wizard `--with-deps` path must produce byte-identical results to today; the sync tests from Workstream B pin this.
- `ws.project(&project_name)` borrows `ws`; keep the zero-project branch before that borrow (it returns/recurses).
- Selector-less + `--dry-run` in wizard mode: allowed — wizard picks, then dry-run reports without confirm.

- [ ] **Step 6: Run all tests**

Run: `cargo test -p rigg 2>&1 | tail -15`
Expected: new unit tests pass; both new cli_surface tests pass; ALL Workstream B adopt tests (sync.rs) still pass unchanged — they run non-interactive, so the wizard never activates.

- [ ] **Step 7: fmt + clippy + full suite**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3 && cargo test --workspace 2>&1 | grep -cE 'test result: ok'`
Expected: clean; 9 suites ok.

- [ ] **Step 8: Commit**

```bash
git add crates/rigg/src/cli.rs crates/rigg/src/commands/adopt.rs crates/rigg/src/commands/new.rs \
        crates/rigg/src/commands/interactive.rs crates/rigg/tests/cli_surface.rs
git commit -m "feat: interactive adopt wizard — pick project, resources, deps from live Azure"
```

---

### Task 3: Signposting, naming guidance, docs

**Files:**
- Modify: `crates/rigg/src/commands/new.rs` (success output)
- Modify: `crates/rigg/src/cli.rs` (NewArgs name doc)
- Modify: `CONCEPTS.md` (naming sentence)
- Modify: `README.md` (wizard mention in adopt examples)
- Test: `crates/rigg/tests/cli_surface.rs`

**Interfaces:** none new.

- [ ] **Step 1: Failing test.** Append to `cli_surface.rs`:

```rust
#[test]
fn new_project_signposts_adopt_path() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "project", "p2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rigg adopt p2"))
        .stdout(predicate::str::contains("rigg new"));
}

#[test]
fn concepts_includes_naming_guidance() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .arg("concepts")
        .assert()
        .success()
        .stdout(predicate::str::contains("Name a project after"));
}
```

- [ ] **Step 2: Run to verify both fail**

Run: `cargo test -p rigg --test cli_surface signposts 2>&1 | tail -8` and `... naming_guidance ...`
Expected: FAIL.

- [ ] **Step 3: Update `new_project` output** in `new.rs` (replacing the single "Add resources" line):

```rust
    println!("Next steps:");
    println!("  rigg adopt {name}                    # adopt existing Azure resources (interactive)");
    println!("  rigg new <kind> <name> -p {name}     # or scaffold new ones");
```

- [ ] **Step 4: Naming guidance.** In `CONCEPTS.md`, at the end of the "One or many projects? Choosing boundaries" section (after the "Because a resource lives in exactly one project…" paragraph), add:

```markdown
**Naming:** Name a project after the thing it owns — a project holding the
`regulus` agent and its retrieval stack is naturally called `regulus`. Names
follow the same rules as resource names (no `/` or `\`, at most 260
characters).
```

In `cli.rs` `NewArgs`:

```rust
    /// Name of the new project/resource/spec. Tip: name a project after the
    /// thing it owns (e.g. the agent's name). No `/` or `\`, max 260 chars.
    pub name: String,
```

- [ ] **Step 5: README wizard mention.** In the Quick Start adopt block, add a first line:

```markdown
rigg adopt my-rag                     # interactive: pick resources from a live menu
```

- [ ] **Step 6: Run tests + full checks**

Run: `cargo test -p rigg --test cli_surface 2>&1 | tail -6 && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all pass. NOTE: the CONCEPTS.md edit changes the embedded `include_str!` content — confirm `concepts` tests still pass (they assert on invariant sentence + headings, which are untouched).

- [ ] **Step 7: Commit**

```bash
git add crates/rigg/src/commands/new.rs crates/rigg/src/cli.rs CONCEPTS.md README.md crates/rigg/tests/cli_surface.rs
git commit -m "docs: signpost adopt from new project; project naming guidance"
```

---

## Final Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Then LIVE acceptance (controller, not subagent): run `rigg adopt` interactively in the e2e-test workspace against real Azure — the Regulus scenario.

## Self-Review notes

- Spec coverage: activation matrix → Task 2 Step 5 + pinning tests; project step incl. zero-project create → Task 2; cross-service grouped menu → `wizard_candidates` + legend; deps ask-only-if-adds → Task 2; always-confirm in wizard → Task 2; hint → `equivalent_command`; unreachable-service hard fail → unchanged `snapshot()` (constraint documents it); signpost + naming → Task 3; prompt layer + no-color + abort mapping → Task 1.
- Type consistency: `interactive::` fn signatures used in Task 2 match Task 1; `create_project(&ws, &name)` matches Task 2 Step 2; `AdoptArgs.project: Option<String>` threaded through the recursion struct literal.
- The wizard never runs in tests (no TTY) — Workstream B tests remain the behavioral pin for everything scriptable.
