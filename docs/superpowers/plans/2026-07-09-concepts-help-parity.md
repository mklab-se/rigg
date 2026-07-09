# Concepts + Help Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make rigg explain its own workspace/project model — a single-source `CONCEPTS.md` rendered by a new `rigg concepts` command, help cross-references, and next-step hints on empty workspaces.

**Architecture:** `CONCEPTS.md` at the repo root is the one canonical explanation. The new `concepts` command embeds it via `include_str!` and renders it with `termimad` (styled on a TTY, plain otherwise, raw markdown under `--output json`). Three subcommands gain a one-line pointer to it; `status`/`describe` print a hint when a workspace has no projects yet.

**Tech Stack:** Rust, clap 4.5 (derive), `termimad` (new dep) for terminal markdown, `colored` (existing), assert_cmd + predicates for tests.

## Global Constraints

- No backwards-compatibility burden — single user; optimize for clarity.
- Single source of truth: concept prose lives ONLY in `CONCEPTS.md`; the CLI embeds that exact file. README/GETTING_STARTED carry links + a short teaser, never a second copy.
- JSON output must never contain prose: `describe --output json` on an empty workspace stays `[]`; `concepts --output json` returns raw markdown under a `concepts` key.
- Load-bearing invariant sentence, verbatim, must appear in `CONCEPTS.md`: **"A resource belongs to exactly one project."**
- Command run-fns return `anyhow::Result<()>`; `concepts` is synchronous and needs no workspace/network.
- `termimad` styling is gated on `std::io::stdout().is_terminal() && !ctx.no_color`.

---

### Task 1: `CONCEPTS.md` + `rigg concepts` command

**Files:**
- Create: `CONCEPTS.md`
- Create: `crates/rigg/src/commands/concepts.rs`
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]`)
- Modify: `crates/rigg/Cargo.toml` (crate deps)
- Modify: `crates/rigg/src/commands/mod.rs` (add `pub mod concepts;`)
- Modify: `crates/rigg/src/commands/mod.rs` (`GlobalContext` gains `no_color`)
- Modify: `crates/rigg/src/cli.rs` (`Concepts` variant + dispatch)
- Test: `crates/rigg/tests/cli_surface.rs`
- Test: unit test inside `concepts.rs` (parity guard)

**Interfaces:**
- Produces: `commands::concepts::run(ctx: &GlobalContext) -> anyhow::Result<()>`
- Produces: `const CONCEPTS_MD: &str` embedded in `concepts.rs`
- Consumes: `GlobalContext { json(), no_color }`

- [ ] **Step 1: Create `CONCEPTS.md`** (repo root). Exact content:

~~~markdown
# Concepts

rigg has two levels: a **workspace** and its **projects**. Understanding the
split is the key to using rigg well.

## Workspace vs project

- A **workspace** (`rigg.yaml`) is the top level. It declares your
  **environments** (dev, test, prod) and the **service connections** each
  environment points at — which Azure AI Search service, which Microsoft
  Foundry account/project — plus shared assets like `apis/`. A workspace holds
  *no* resource definitions itself.
- A **project** (`projects/<name>/`) is a **named group of resource
  definitions you pull, push, diff, review, and deploy as one unit**. Indexes,
  indexers, skillsets, knowledge bases, agents, and model deployments live as
  files inside a project.
- **A resource belongs to exactly one project.** rigg enforces this. It is what
  makes sync unambiguous: when you push a project, rigg knows exactly which
  remote resources that project owns — so it never half-syncs or fights another
  project over the same resource.

## Why two levels?

The workspace answers *"where do things go?"* — which services and
environments, shared across everything. Projects answer *"what do I manage
together?"* — the unit of change, review, and deployment.

Separating them means you can promote one coherent project from dev to prod
without dragging along unrelated resources, and different projects can be owned
and reviewed independently while sharing the same service and environment
configuration.

## One or many projects? Choosing boundaries

Use **one** project when your whole stack ships and is reviewed together — for
example, a single agent plus the retrieval pipeline it depends on.

Use **several** projects to draw boundaries you care about:

- **By deployable unit** — each agent or app that ships independently.
- **By ownership / review scope** — a team owns its project; pull requests stay
  focused on one project's files.
- **By lifecycle** — group things that change on the same cadence; separate
  things that don't.

Rule of thumb: **if you would pull, push, and review it as a unit, it is a
project.** If two things never need to deploy together, they can be separate
projects.

Because a resource lives in exactly one project, a *shared* resource goes in
the project that owns it; other projects refer to it by name and environment
rather than co-owning it.

## Workspace layout

```
rigg.yaml                     # workspace: environments + service connections
apis/<name>.json              # shared OpenAPI specs for custom Web API skills
projects/<name>/
  project.yaml                # metadata only — the directory IS the membership
  search/{data-sources,indexes,skillsets,indexers,synonym-maps,aliases,
          knowledge-sources,knowledge-bases}/<name>.json
  foundry/{agents,deployments,connections,guardrails}/<name>.json
.rigg/<env>/<project>/...     # per-environment sync state (gitignored)
```

## See also

- **Getting Started** (`GETTING_STARTED.md`) — build a stack from scratch.
- Run `rigg describe` to see how your resources connect, and `rigg status` to
  see what is in sync.
~~~

- [ ] **Step 2: Add `termimad` dependency.** In root `Cargo.toml`, under `[workspace.dependencies]`, after the `# Terminal colors` block add:

```toml
# Terminal markdown rendering
termimad = "0.31"
```

In `crates/rigg/Cargo.toml`, in `[dependencies]`, after `colored.workspace = true` add:

```toml
termimad.workspace = true
```

- [ ] **Step 3: Add `no_color` to `GlobalContext`.** In `crates/rigg/src/commands/mod.rs`, add the field to the struct (after `pub non_interactive: bool,`):

```rust
    pub no_color: bool,
```

And in `GlobalContext::from_cli`, after `non_interactive: ...,` add:

```rust
            no_color: cli.no_color,
```

- [ ] **Step 4: Write the failing test** in `crates/rigg/tests/cli_surface.rs` (append):

```rust
#[test]
fn concepts_explains_the_model() {
    // Runs anywhere — no workspace required.
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .arg("concepts")
        .assert()
        .success()
        .stdout(predicate::str::contains("Workspace"))
        .stdout(predicate::str::contains("exactly one project"));
}

#[test]
fn concepts_no_color_emits_no_ansi() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["concepts", "--no-color"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn concepts_json_returns_markdown_source() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["concepts", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"concepts\""))
        .stdout(predicate::str::contains("exactly one project"));
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p rigg --test cli_surface concepts 2>&1 | tail -20`
Expected: FAIL — `concepts` is an unknown subcommand (exit 2), predicates unmet.

- [ ] **Step 6: Create `crates/rigg/src/commands/concepts.rs`:**

```rust
//! `rigg concepts` — print rigg's workspace/project mental model.
//!
//! Single-sourced from the repo-root `CONCEPTS.md`, embedded at build time so
//! the CLI and the docs cannot drift.

use std::io::IsTerminal;

use anyhow::Result;
use serde_json::json;

use crate::commands::GlobalContext;

/// The canonical concept guide. Embedding `CONCEPTS.md` guarantees CLI/docs parity.
const CONCEPTS_MD: &str = include_str!("../../../../CONCEPTS.md");

pub fn run(ctx: &GlobalContext) -> Result<()> {
    if ctx.json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "concepts": CONCEPTS_MD }))?
        );
        return Ok(());
    }

    let styled = std::io::stdout().is_terminal() && !ctx.no_color;
    let skin = if styled {
        termimad::MadSkin::default()
    } else {
        termimad::MadSkin::no_style()
    };
    skin.print_text(CONCEPTS_MD);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::CONCEPTS_MD;

    /// Parity guard: the core invariant must survive any future doc rewrite.
    #[test]
    fn concepts_md_states_the_core_invariant() {
        assert!(
            CONCEPTS_MD.contains("A resource belongs to exactly one project."),
            "CONCEPTS.md must state the one-resource-one-project invariant verbatim"
        );
    }
}
```

- [ ] **Step 7: Register the module.** In `crates/rigg/src/commands/mod.rs`, next to the other `pub mod` declarations (e.g. after `pub mod completion;`), add:

```rust
pub mod concepts;
```

- [ ] **Step 8: Add the `Concepts` variant.** In `crates/rigg/src/cli.rs`, in `enum Commands`, add after the `Describe(DescribeArgs)` variant:

```rust
    /// Explain rigg's core model: workspace, projects, and how to choose boundaries
    Concepts,
```

- [ ] **Step 9: Dispatch it.** In `crates/rigg/src/cli.rs`, in the match block (after the `Commands::Describe(...)` arm, ~line 451), add:

```rust
            Commands::Concepts => commands::concepts::run(&ctx),
```

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test -p rigg --test cli_surface concepts 2>&1 | tail -20`
Expected: PASS (3 tests). Also run the unit test:
Run: `cargo test -p rigg concepts_md_states 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 11: Manual smoke check**

Run: `cargo run -q -p rigg -- concepts | head -20`
Expected: styled headings and the "Workspace vs project" section render.

- [ ] **Step 12: Commit**

```bash
git add CONCEPTS.md crates/rigg/src/commands/concepts.rs crates/rigg/src/commands/mod.rs \
        crates/rigg/src/cli.rs Cargo.toml Cargo.lock crates/rigg/Cargo.toml \
        crates/rigg/tests/cli_surface.rs
git commit -m "feat: rigg concepts command + single-source CONCEPTS.md"
```

---

### Task 2: Help cross-references

**Files:**
- Modify: `crates/rigg/src/cli.rs` (root `long_about`; `New` and `Pull` variant doc comments)
- Test: `crates/rigg/tests/cli_surface.rs`

**Interfaces:**
- Consumes: the `Concepts` command from Task 1 (pointers reference `rigg concepts`).

- [ ] **Step 1: Write the failing test** (append to `cli_surface.rs`):

```rust
#[test]
fn help_points_at_concepts() {
    rigg().arg("--help").assert().success()
        .stdout(predicate::str::contains("rigg concepts"));
    rigg().args(["new", "--help"]).assert().success()
        .stdout(predicate::str::contains("concepts"));
    rigg().args(["pull", "--help"]).assert().success()
        .stdout(predicate::str::contains("concepts"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rigg --test cli_surface help_points_at_concepts 2>&1 | tail -15`
Expected: FAIL — "concepts" not found in help output.

- [ ] **Step 3: Extend the root `long_about`.** In `crates/rigg/src/cli.rs`, change the `long_about` string (ends with `... operate on projects.`) to append a sentence:

```rust
    long_about = "Configuration-as-code for Azure AI Search and Microsoft Foundry.\n\n\
    A rigg workspace holds one or more projects; each project owns its resource\n\
    definitions (indexes, indexers, skillsets, knowledge bases, Foundry agents,\n\
    deployments, ...) as JSON files. Pull, push, and diff operate on projects.\n\n\
    New here? Run `rigg concepts` for the workspace/project model.",
```

- [ ] **Step 4: Add a pointer to `New`.** In `crates/rigg/src/cli.rs`, replace the `New` variant's single doc line with a two-paragraph doc comment (first line = short help, rest = long help):

```rust
    /// Scaffold a new project, resource, pipeline, or API spec
    ///
    /// See `rigg concepts` for what a project is and when to use several.
    New(NewArgs),
```

- [ ] **Step 5: Add a pointer to `Pull`.** Likewise for the `Pull` variant:

```rust
    /// Download resource definitions from Azure into project files
    ///
    /// See `rigg concepts` for the project model that pull and --adopt rely on.
    Pull(PullArgs),
```

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p rigg --test cli_surface help_points_at_concepts 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rigg/src/cli.rs crates/rigg/tests/cli_surface.rs
git commit -m "docs: cross-reference rigg concepts from root/new/pull help"
```

---

### Task 3: Empty-state hints on `status` and `describe`

**Files:**
- Modify: `crates/rigg/src/commands/mod.rs` (add `print_no_projects_hint`)
- Modify: `crates/rigg/src/commands/describe.rs`
- Modify: `crates/rigg/src/commands/status.rs`
- Test: `crates/rigg/tests/cli_surface.rs`

**Interfaces:**
- Produces: `commands::print_no_projects_hint()` — prints the text-mode hint.
- Consumes: `Workspace { projects }`, `GlobalContext { json() }`.

- [ ] **Step 1: Write the failing tests** (append to `cli_surface.rs`). Add an empty-workspace helper and three tests:

```rust
/// A workspace with an environment but NO projects.
fn empty_workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: unit-test-svc }\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("projects")).unwrap();
    tmp
}

#[test]
fn status_empty_workspace_hints_next_steps() {
    let ws = empty_workspace();
    rigg()
        .current_dir(ws.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("No projects yet"))
        .stdout(predicate::str::contains("rigg concepts"))
        .stdout(predicate::str::contains("rigg new project"));
}

#[test]
fn describe_empty_workspace_hints_next_steps() {
    let ws = empty_workspace();
    rigg()
        .current_dir(ws.path())
        .arg("describe")
        .assert()
        .success()
        .stdout(predicate::str::contains("No projects yet"));
}

#[test]
fn describe_empty_workspace_json_stays_empty_array() {
    let ws = empty_workspace();
    rigg()
        .current_dir(ws.path())
        .args(["describe", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"^\s*\[\s*\]\s*$").unwrap());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rigg --test cli_surface empty_workspace 2>&1 | tail -20`
Expected: FAIL — `status`/`describe` print nothing; "No projects yet" absent.

- [ ] **Step 3: Add the shared helper.** In `crates/rigg/src/commands/mod.rs`, add a public function near the other free functions (e.g. just after `load_workspace`):

```rust
/// Text-mode hint printed when the workspace has no projects yet.
pub fn print_no_projects_hint() {
    println!(
        "No projects yet. A project groups the resources you manage together —\n\
         see `rigg concepts`, then `rigg new project <name>`."
    );
}
```

- [ ] **Step 4: Hook into `describe`.** In `crates/rigg/src/commands/describe.rs`, immediately after the `let projects: Vec<_> = match args.project.as_deref() { ... };` block, add:

```rust
    if ws.projects.is_empty() && args.project.is_none() && !ctx.json() {
        crate::commands::print_no_projects_hint();
        return Ok(());
    }
```

- [ ] **Step 5: Hook into `status`.** In `crates/rigg/src/commands/status.rs`, immediately after the `let projects: Vec<_> = match args.project.as_deref() { ... };` block (before the `owned_by_any` loop), add:

```rust
    if ws.projects.is_empty() && args.project.is_none() && !ctx.json() {
        crate::commands::print_no_projects_hint();
        return Ok(());
    }
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p rigg --test cli_surface empty_workspace 2>&1 | tail -20`
Expected: PASS (3 tests).

- [ ] **Step 7: Commit**

```bash
git add crates/rigg/src/commands/mod.rs crates/rigg/src/commands/describe.rs \
        crates/rigg/src/commands/status.rs crates/rigg/tests/cli_surface.rs
git commit -m "feat: next-step hint on empty workspace for status/describe"
```

---

### Task 4: Docs — link `CONCEPTS.md` from README & GETTING_STARTED

**Files:**
- Modify: `README.md`
- Modify: `GETTING_STARTED.md`

**Interfaces:** none (docs only). No code; no automated test. Verified by reading.

- [ ] **Step 1: Add a Concepts subsection to README.** In `README.md`, immediately after the `## What Rigg Does` section (just before `## Quick Start`), insert:

```markdown
## Concepts

rigg has two levels. A **workspace** (`rigg.yaml`) holds your environments and
service connections; a **project** is a group of resource definitions you pull,
push, review, and deploy as one unit — and every resource belongs to exactly one
project. That single rule is what keeps sync unambiguous.

New to the model, or unsure whether to use one project or several? Read
**[CONCEPTS.md](CONCEPTS.md)** — or run `rigg concepts` for the same guide in
your terminal.
```

- [ ] **Step 2: Link from GETTING_STARTED.** In `GETTING_STARTED.md`, find the line ending `rigg manages all of these as JSON files in a **project**, so you version, review, and deploy them together.` and append a sentence:

```markdown

> New to the workspace/project model? See **[CONCEPTS.md](CONCEPTS.md)** (or run `rigg concepts`).
```

- [ ] **Step 3: Verify links resolve**

Run: `grep -n "CONCEPTS.md" README.md GETTING_STARTED.md && test -f CONCEPTS.md && echo OK`
Expected: both files reference `CONCEPTS.md`, file exists, prints `OK`.

- [ ] **Step 4: Commit**

```bash
git add README.md GETTING_STARTED.md
git commit -m "docs: link CONCEPTS.md from README and GETTING_STARTED"
```

---

## Final Verification (run before declaring complete)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Expected: all pass. Then a manual render check: `cargo run -q -p rigg -- concepts`.

## Self-Review notes

- **Spec coverage:** Component 1 (CONCEPTS.md) → Task 1 Step 1. Component 2 (concepts cmd, styled/plain/json, no workspace) → Task 1 Steps 4–11. Component 3 (help pointers) → Task 2. Component 4 (empty-state, JSON clean) → Task 3. Single-source/anti-drift → Task 1 (include_str! + parity unit test). Docs links → Task 4. Testing section → tests in Tasks 1–3.
- **Type consistency:** `run(ctx: &GlobalContext) -> Result<()>` for concepts (sync, matches dispatch arm without `.await`); `print_no_projects_hint()` referenced via `crate::commands::` in both describe.rs and status.rs; `CONCEPTS_MD` used in command body and its own unit test.
- **JSON safety:** empty-state hints gated on `!ctx.json()`; explicit test that `describe --output json` stays `[]`.
