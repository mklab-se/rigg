# Environments Implementation Plan (Workstream H)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Per-environment project trees (`projects/<p>/envs/<env>/…`), logical identity by path with free physical naming, a `rigg promote` command with pinned-field preservation, protected-environment policies, an `env add` wizard, and docs — per the approved spec `docs/superpowers/specs/2026-07-10-environments-design.md` (READ IT FIRST for every task).

**Architecture:** `Store` gains an environment root and physical-name lookup (`name` field authoritative, file stem = logical id). Every command threads the resolved env into `Store::new`. `promote` copies between env trees preserving pinned fields (registry defaults + `x-rigg-pin`). A `policy.protected` env gate guards cloud-mutating ops with typed confirmation / `--confirm-env`.

**Tech Stack:** Rust; existing rigg-core/rigg/rigg-diff; wiremock + assert_cmd tests.

## Global Constraints

- NO backwards compatibility with the flat layout: the binary reads ONLY `projects/<p>/envs/<env>/…`. All tests migrate.
- Physical identity for sync (baselines, snapshots, selectors, display) = the `name` FIELD; logical identity (cross-env correlation) = the file path/stem. Stem == name by default; divergence is user-authored.
- Two files with the same physical name in one env dir = validation/store error.
- Read-only ops never policy-gated; `--yes` never bypasses `protected`; `--confirm-env <name>` is the non-interactive consent and must match exactly.
- Every task leaves fmt / clippy(`-D warnings`) / `cargo test --workspace` green.
- Sidecar behavior, baselines format (Doc), exclusion rules (platform-managed, auto-created), wizard flows: unchanged except for pathing.

---

### Task 1: rigg-core — env-rooted Store, physical-name identity, policy model

**Files:**
- Modify: `crates/rigg-core/src/store.rs`, `crates/rigg-core/src/workspace.rs`, `crates/rigg-core/src/identity.rs`

**Interfaces (produced; every later task consumes these):**
- `Store::new(project: &'w Project, env: &str) -> Store<'w>` — root = `project.dir/envs/<env>/`.
- `Store::list() -> Result<Vec<(ResourceRef, PathBuf)>>` — `ResourceRef.name` is the PHYSICAL name read from the file's top-level `name` field (raw JSON parse; fall back to the file stem when the field is absent/non-string). Errors if two files in one kind dir carry the same physical name (`StoreError::DuplicatePhysicalName { name, first, second }` — new variant).
- `Store::locate(r: &ResourceRef) -> Result<Option<PathBuf>>` — find the file whose physical name == `r.name` (scan the kind dir; stem match is the common fast path but the name field decides).
- `Store::path_for(r)` KEEPS meaning "path for a NEW file named after the physical name" (used on create). `read`/`write`/`delete` use `locate` first and fall back to `path_for` on create (`write`) or NotFound-style behavior (`read` keeps returning the existing `StoreError::Io` shape when nothing matches — verify what callers expect today and preserve it).
- `Store::envs_of(project: &Project) -> Vec<String>` (associated fn): list `project.dir/envs/*` subdirectory names, sorted — used by `validate` (all envs) and the final sweep.
- `workspace.rs`: `Environment` gains `#[serde(default)] pub policy: Policy;` with `#[derive(Default…)] pub struct Policy { #[serde(default)] pub protected: bool }`. `ResolvedEnv` exposes it (e.g. `pub fn protected(&self) -> bool`).
- `assert_exclusive_ownership(ws, env)` — takes the env name; ownership is per env tree.
- `identity.rs`'s `Store::new` call gets the env (trace how it obtains a project — it must already run in an env context; thread the parameter).

- [ ] **Step 1: Read store.rs, workspace.rs fully.** Note `list()` currently derives names from stems (store.rs:88-120) and `path_for` builds `<project>/<domain>/<kind>/<name>.json` (store.rs:79-86). The new root inserts `envs/<env>` between project dir and domain dir. Add a `ENVS_DIR: &str = "envs"` const in workspace.rs next to `PROJECTS_DIR` etc.

- [ ] **Step 2 (TDD): rewrite store unit tests** for the new semantics before implementing: env rooting (`path_for` contains `envs/dev/`), physical-name listing (file `regulus.json` with `"name": "Regulus-Prod"` lists as `Regulus-Prod`), `locate` by physical when stem differs, duplicate-physical error, `envs_of`. Migrate every existing store test to pass an env (`"dev"`) and the new paths. Run: expect the new assertions RED against a mechanical port, then implement until GREEN.

- [ ] **Step 3: Implement.** Keep the diff surgical: a private `root(&self) -> PathBuf` = `project.dir.join(ENVS_DIR).join(&self.env)`; all existing path builders go through it. `list()` opens each JSON with a lightweight `serde_json::from_str::<Value>` on the raw text (NOT the sidecar-inlining read — only `name` is needed) and validates physical names with `validate_resource_name`. Sidecar handling in `read`/`write` unchanged.

- [ ] **Step 4: Policy model** in workspace.rs (+ a serde round-trip unit test: env with `policy: { protected: true }` parses; absent policy defaults to unprotected).

- [ ] **Step 5:** `cargo test -p rigg-core` green; `cargo build --workspace` will FAIL (rigg crate not yet threaded) — that is expected ONLY if you and the controller agreed to land Tasks 1+2 as one commit. DEFAULT: implement Task 1 and Task 2 in ONE commit to keep the workspace green (single implementer dispatch covers both — see Task 2).

*(Commit happens at the end of Task 2.)*

---

### Task 2: rigg CLI — thread the environment everywhere; migrate all tests

**Files:**
- Modify: every `Store::new` site — `adopt.rs`, `copy.rs`, `delete.rs`, `describe.rs`, `diff.rs`, `new.rs`, `pull.rs`, `push.rs`, `status.rs`, `validate.rs` (grep `Store::new` for the authoritative list)
- Modify: `crates/rigg/tests/cli_surface.rs`, `crates/rigg/tests/sync.rs` (layout migration)

**Interfaces:** consumes Task 1's signatures. No new public interfaces.

- [ ] **Step 1: Thread `&env.name`** into every `Store::new(project)` call. Commands already resolve the env (`resolve_env(&ws, ctx)`), EXCEPT:
  - `describe.rs` and `validate.rs` currently don't resolve an env — `describe` gains `let env = resolve_env(&ws, ctx)?;`, uses it for the store, and prints the env in its header (`{project} (env: {env})`), text mode. JSON gains an `"env"` field per project object.
  - `validate.rs` validates ALL envs: for each project, loop `Store::envs_of(project)`; when a project has no env dirs, report nothing for it (empty project). Per-env findings prefix `[env <name>]`. The `--strict` semantics per env unchanged. Reference resolution ("does the referenced resource exist") resolves within the SAME env across projects.
  - `copy.rs`: resolve env; both source lookup (its ownership scan must use `locate`, not `path_for(...).exists()`) and target write in that env. `new.rs` (resource/pipeline scaffolds): write into the resolved env.
  - `exclusive ownership` call sites pass the env.
- [ ] **Step 2: Migrate the test helpers.** `cli_surface.rs::workspace()` creates `projects/demo/project.yaml` only — unchanged, but any test writing/asserting resource paths moves to `projects/demo/envs/dev/...`. `sync.rs::write_resource` writes to `projects/demo/envs/dev/search/<dir>/...`; the second-project ownership fixture (`projects/other/...`) likewise; every `assert!(....join("projects/demo/search/...")` becomes `envs/dev/search/...`. The adopt/pull/push/diff semantics tests must pass UNCHANGED apart from paths — they are the pin that sync survived re-rooting.
- [ ] **Step 3:** Full battery green. Commit (Tasks 1+2 together):

```bash
git add crates/rigg-core crates/rigg/src crates/rigg/tests
git commit -m "feat!: per-environment project trees — envs/<env>/ layout, physical-name identity"
```

---

### Task 3: registry `env_pinned` + `rigg promote`

**Files:**
- Modify: `crates/rigg-core/src/registry.rs`
- Create: `crates/rigg/src/commands/promote.rs`
- Modify: `crates/rigg/src/commands/mod.rs`, `crates/rigg/src/cli.rs`
- Test: registry + promote unit tests inline; `crates/rigg/tests/cli_surface.rs` (temp-workspace promote end-to-end — promote is purely local, no wiremock needed)

**Interfaces:**
- `registry::env_pinned(kind) -> Vec<&'static str>` = that kind's `secret_fields` ∪ `write_only_fields` ∪ a new per-kind `env_pinned_extra` table entry (Agent: `["tools[].server_url", "tools[].project_connection_id"]`; verify which fields DataSource/KnowledgeSource/Connection already cover via secret/write-only lists and add extras ONLY where a genuinely environmental field is uncovered). `"name"` is pinned by the promote code itself, not the registry.
- `registry::X_RIGG_PIN: &str = "x-rigg-pin"` — array-of-dot-paths annotation, read from the TARGET file, stripped on push like all `x-rigg-*` (verify `normalize_for_push` strips all `x-rigg-*` keys generically — it should; add a test).
- `commands::promote::run(ctx, args) -> Result<()>`; `PromoteArgs { project: String, from: String, to: String, dry_run: bool }` (clap: `--from`, `--to` required; `Promote` variant help documents the pinned-field behavior and that nothing touches Azure).

- [ ] **Step 1 (TDD): unit tests first** for the pure merge:

```rust
    // promote.rs
    /// A's doc becomes B's, except pinned paths keep B's values.
    fn merge_promote(source: &Value, target: Option<&Value>, pinned: &[String]) -> Value
```
  Tests: name kept from target; registry-pinned path kept (use a real Agent doc with differing `tools[].server_url`); `x-rigg-pin`-listed extra path kept; target None → source verbatim; the `x-rigg-pin` annotation itself survives from the target. Path get/set must honor the `[]` array convention (reuse/extend `registry::collect_path`; you need a SET counterpart — implement `set_path(dst, path, value)` mirroring collect_path's traversal, applying the value at each matching site by POSITION pairing with the target's matching sites (for `[]` paths, pair source/target array elements by index; when lengths differ, apply to the min prefix)).
- [ ] **Step 2: the command flow.** Resolve project; check `from`/`to` are configured environments (rigg.yaml) — usage error otherwise; equal envs → usage error. Build both stores. Correlate by LOGICAL id (relative path stem per kind — from `list()` paths, NOT physical names). For each logical resource in FROM: read both (sidecars inlined), compute merged target via `merge_promote` with `pinned = ["name"] + registry::env_pinned(kind) + target's x-rigg-pin`; diff current-target vs merged (rigg_diff) for the preview table with `SideLabels { new_side: format!("{from} (incoming)"), old_side: to_env }`… keep labels simple: new = what B will become, old = B today. Classify: changed / new (no target file) / unchanged. Only-in-TO resources: report `kept (only in {to})`, never touched.
- Preview always prints (count + tables for changed, list for new/kept). `--dry-run` stops. Interactive: `Proceed? [Y/n]` (promote is local + git-less env dirs are user's responsibility; still confirm because it overwrites files). Non-interactive without `-y`: exit 2 with guidance. Apply: `store_to.write(...)` per merged doc (physical ref = merged doc's `name`); new files: write with the SOURCE's stem (locate-or-create handles it — verify `write` creates at `path_for(physical)`; for stem≠name new copies you must create at the SOURCE STEM path instead: add `Store::write_at_stem(stem, r_kind, doc)` or write via the path directly + sidecar extract — pick the cleanest store-level primitive and document it).
- After apply: hints — `rigg diff {project} -e {to}` / `rigg push {project} -e {to}`; plus, when new files were created, list the env-pinned fields worth reviewing on them.
- JSON output: `{ "promoted": [...], "created": [...], "kept_only_in_to": [...], "pinned_kept": {resource: [paths]} }`.
- [ ] **Step 3: cli_surface end-to-end test**: temp workspace with two envs (dev+prod in rigg.yaml), a dev resource with a `name`-divergent prod copy + pinned field, run `promote p --from dev --to prod -y`, assert prod file keeps its `name` + pinned value but gains dev's other changes; `--dry-run` writes nothing; promoting creates missing files.
- [ ] **Step 4:** battery green; commit `feat: rigg promote — reviewable env-to-env promotion with pinned fields`.

---

### Task 4: protected-environment policy gate

**Files:**
- Modify: `crates/rigg/src/commands/push.rs`, `delete.rs`, `cli.rs` (`--confirm-env` on PushArgs + DeleteArgs), `commands/mod.rs` (shared gate helper)
- Test: `crates/rigg/tests/sync.rs`

**Interfaces:**
- `commands::confirm_protected_env(ctx, env: &ResolvedEnv, confirm_env: Option<&str>, operation: &str) -> Result<()>` — no-op when unprotected. Protected: `--confirm-env` matching env name → Ok; else interactive → `prompt_line("Environment '{name}' is protected. Type its name to confirm {operation}: ")`, mismatch → Err("aborted"); non-interactive → `CommandError::Usage("environment '{name}' is protected: pass --confirm-env {name} to proceed")` (exit 2). `--yes` deliberately NOT consulted.

- [ ] **Step 1 (TDD): sync tests** — workspace yaml gains a second env `prod: { policy: { protected: true }, search: {...mock...} }`: non-interactive `push demo -e prod --yes` (with a creatable resource) → exit 2 + message contains `--confirm-env prod`; same with `--confirm-env prod` → success; `delete demo --remote -e prod --yes` → exit 2; unprotected dev push unchanged. Gate must fire BEFORE any mutating call but AFTER the plan/dry-run print (dry-run never gated — verify placement so `push --dry-run -e prod` still works ungated).
- [ ] **Step 2:** implement + wire; `env show` prints the policy; battery green; commit `feat: protected environments — typed/--confirm-env consent for cloud mutations`.

---

### Task 5: env add wizard, init messaging, concepts & docs

**Files:**
- Modify: `crates/rigg/src/commands/env.rs`, `init.rs` (extract discovery helpers to `commands/discovery.rs` or make them `pub(crate)` — implementer's judgment, no duplication), `CONCEPTS.md`, `README.md`, `GETTING_STARTED.md`
- Test: `crates/rigg/tests/cli_surface.rs`

- [ ] **Step 1:** `rigg env add <name>` with no service flags: non-interactive → today's usage error, now mentioning the wizard; interactive → init's ARM discovery pick-lists for search/foundry, then `Protect this environment (require typed confirmation for cloud changes)? [y/N]`, then writes the env (reusing `edit_workspace_yaml`), printing what was written + `rigg env set-default` hint.
- [ ] **Step 2:** `init.rs` success output explains the env: `environment: dev (default) — rigg commands target it unless -e/RIGG_ENV say otherwise; add more with rigg env add`. Guard test in cli_surface (init output mentions `rigg env add`).
- [ ] **Step 3:** CONCEPTS.md gains an `## Environments` chapter: env = named Azure target + optional policy; per-env trees under `envs/`; path = logical identity, `name` field = physical; promote + pinned fields; protected envs. Update the existing "Workspace layout" block in CONCEPTS.md to the new tree. cli_surface: `concepts` output contains "Environments" and "physical"; existing invariant tests untouched.
- [ ] **Step 4:** README: layout diagram + environments/promotion section rewritten (promote example, protected example); GETTING_STARTED: layout mentions + a short promote step. Verify no stale `projects/<p>/search/` path remains in either.
- [ ] **Step 5:** battery green; commit `feat: env add wizard, init env messaging, environments concepts + docs`.

---

### Task 6: samples + skills + repo-doc sweep

**Files:**
- Modify: `samples/projects/*` (move `search|foundry` under `envs/demo/`), `samples/README.md` if it shows paths
- Modify: `.claude/skills/rigg-guide/SKILL.md` (layout section), any other skill/doc showing the old tree (`grep -rn 'search/{data-sources' ; grep -rn 'projects/<name>/search'` across repo), `CLAUDE.md` workspace-layout section, `MCP.md` if affected
- Test: none new — `git grep` sweep is the verification

- [ ] **Step 1:** `git mv` the sample resource dirs under `envs/demo/` (samples env name is `demo` per samples/rigg.yaml). Any sample doc paths updated.
- [ ] **Step 2:** Sweep: `grep -rn "projects/.*/search/\|projects/.*/foundry/" README.md GETTING_STARTED.md CLAUDE.md CONCEPTS.md MCP.md SKILLS.md .claude/skills/ samples/ | grep -v envs/` → zero hits.
- [ ] **Step 3:** battery green (samples aren't compiled, but cli tests may reference them — check); commit `docs: migrate samples and skills to the envs/<env> layout`.

---

## Final Verification

Full battery; final whole-branch review (opus) with cross-cutting checks (MCP subprocess paths, ci templates, graph/push-order under physical names, adopt wizard paths, identity.rs). Then merge to main (NO push to origin, NO tag/release — explicit user instruction). After merge: migrate the untracked `e2e-test/` workspace (`mkdir envs/dev` + `git`-less `mv` of search/foundry per project), run live read-only smoke (`status`, `validate`, `describe`) to confirm the user can test on return.

## Self-Review notes

- Spec coverage: layout+identity → T1/T2; promote+pins → T3; policies → T4; wizard/init/docs → T5; samples/skills → T6; e2e-test migration → post-merge controller step.
- The riskiest interface (physical-vs-logical in Store) is defined once in T1 and consumed everywhere; T2's migrated sync tests are the behavioral pin.
- Promote's `write_at_stem` need is called out rather than discovered mid-task.
