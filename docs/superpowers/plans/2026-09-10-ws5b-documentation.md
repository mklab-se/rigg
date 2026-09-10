# Documentation (workstream 5b) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete, drift-proof user documentation for rigg 2.0: a reference page for every configuration file and field, a generated CLI reference, four runnable tutorials, and tests that keep the docs honest.

**Architecture:** Docs move under `docs/reference/` and `docs/tutorials/`; root files become short entry points. Three `dev`/`mcp` subcommands generate the CLI reference, the infrastructure-reference table and the MCP tool table; guard tests in `cli_surface.rs` fail when the generated text drifts. A `docs_commands.rs` test parses every `rigg …` command block in the docs through clap so no page can name a flag that does not exist; a link checker keeps relative links valid. The tutorials are executed once against live Azure before the workstream closes.

**Tech Stack:** Rust (clap `CommandFactory` for the CLI walk, `rmcp` tool router for the MCP table), Markdown, assert_cmd tests, `az` CLI for the live run.

**Spec:** `docs/superpowers/specs/2026-09-10-documentation-design.md`

## Global Constraints

- Branch `rigg-2`; commit after every task with trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Gate before every commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Docs never claim behaviour that does not exist: every flag, subcommand, question id, exit code and environment variable named in a page must exist in the code at the commit that adds the page. Verify with `--help` and grep; the `docs_commands` test enforces the command surface mechanically.
- Every `rigg` command shown in a tutorial is one the reader can paste; placeholders use `<angle-brackets>`; expected output blocks are trimmed real output from a run, marked with a leading `# output` comment line.
- Generated regions are wrapped in `<!-- generated:<name>:start -->` / `<!-- generated:<name>:end -->` markers and are never hand-edited.
- No secrets, no real subscription ids, tenant ids, object ids or resource ids from Kristofer's subscription in committed docs; use `00000000-0000-0000-0000-000000000000`-style placeholders and `contoso-*` names in examples.
- `CONCEPTS.md` and `crates/rigg/CONCEPTS.md` stay identical (existing guard test).
- Nothing under `e2e-test/` or `.superpowers/` is ever committed.

---

### Task 1: Generators, guards and docs tests

**Files:**
- Modify: `crates/rigg/src/cli.rs` (`DevCommands::{CliReference, InfraTable}`, `McpCommands::Tools { markdown: bool }`)
- Create: `crates/rigg/src/commands/docgen.rs` (`cli_reference_markdown() -> String`, `infra_table_markdown() -> String`)
- Modify: `crates/rigg/src/commands/dev.rs` (dispatch), `crates/rigg/src/mcp/mod.rs` (`tools_markdown() -> String` from the tool router's `list_all()`: name, description, parameter names with types and whether required, from each tool's JSON schema)
- Create: `docs/reference/cli.md` (generated), `docs/reference/resource-files.md` skeleton holding only the generated infra table between markers (Task 2 writes the prose), `MCP.md` generated tool-table region (replace the hand-written per-tool sections' *parameter lists* with the generated table; keep the prose)
- Create: `crates/rigg/tests/docs_guards.rs`
- Modify: `CLAUDE.md` (a "Keeping docs current" subsection listing the three generator commands and the guards)

**Interfaces:**
- Produces: `rigg dev cli-reference`, `rigg dev infra-table`, `rigg mcp tools --markdown` (all print Markdown to stdout, exit 0); `docs_guards.rs` helpers `generated_region(file, name) -> String` and `assert_region_current(file, name, generated)`.

**Behaviour:**
- `cli_reference_markdown`: walk `Cli::command()` recursively. For each command: `## rigg <path>` heading, the `about`/`long_about`, a `Usage:` line (clap's `render_usage`), an **Arguments** table (name, required, help), an **Options** table (long/short, value name, default, env var, help) excluding clap's built-in `help`/`version`, and a **Subcommands** list linking to the child headings. Global options are listed once under `## rigg` and referenced from children with one line. Hidden commands/args are skipped. Output is deterministic (clap order).
- `infra_table_markdown`: for each `ResourceKind` with non-empty `registry::infra_refs(kind)`: a `### <kind>` heading and a table `path · binding type · form · only for @odata.type` using `InfraForm`'s binding-type mapping (the same mapping `infra.rs` uses to classify) — expose it as `InfraForm::binding_type_label(&self) -> &'static str` in rigg-core if not already public.
- `tools_markdown`: `| Tool | Purpose | Parameters |` table; parameters rendered as `name: type` with `*` after required ones; the purpose is the first sentence of the tool description.
- `docs_guards.rs`:
  - `cli_reference_is_current`: run `rigg dev cli-reference` via assert_cmd, compare with `docs/reference/cli.md` (both trimmed per line). On mismatch the assertion message says `run: cargo run -q --bin rigg -- dev cli-reference > docs/reference/cli.md`.
  - `infra_table_is_current` and `mcp_tool_table_is_current`: same, against the marked region.
  - `docs_commands_parse` and `relative_links_resolve` run `rigg dev docs-check` (the `rigg` crate has no lib target, so clap parsing must happen inside the binary): assert exit 0 and that its stdout ends with `docs-check: ok`. `rigg dev docs-check [--root <dir>]` (hidden from `rigg --help`, shown in `rigg dev --help`) walks `README.md`, `GETTING_STARTED.md`, `CONCEPTS.md`, `MCP.md`, `docs/**/*.md`, `samples/**/*.md`, `.claude/skills/*/SKILL.md` under the root and performs:
    1. **command parse** — inside fenced blocks tagged `bash`/`sh`/`shell`/untagged, every line whose first token is `rigg` (after optional `$ `): join `\` continuations, strip `# comments`, replace `<placeholders>` with `x`, split with the `shlex` crate (already in the lock file — add it to `crates/rigg/Cargo.toml`), and `Cli::try_parse_from`. Lines containing `…` or `...` are skipped. Each failure prints `file:line: <clap message>`.
    2. **relative links** — every `[text](path#anchor)` whose path is not `http(s)://` or `mailto:` must exist relative to the file; an anchor must match a heading's GitHub slug in the target (lowercase, spaces→`-`, strip everything except `[a-z0-9-]`).
    3. **env vars** (Task 2 turns this on) — every `RIGG_[A-Z_0-9]+` / `AZURE_[A-Z_0-9]+` string literal found in `crates/*/src/**/*.rs` must appear in `docs/reference/environment-variables.md`; missing page = skipped with a note in Task 1, error from Task 2 on.
    4. **question prefixes** (Task 2 turns this on) — every entry of `ask::KNOWN_ID_PREFIXES` must appear in `docs/reference/exit-codes-and-questions.md`.
    Exit 1 with all findings listed when anything fails; `docs-check: ok` otherwise. `RIGG_DOCS_ROOT` is not needed — the test passes `--root` with `CARGO_MANIFEST_DIR/../..`.

- [ ] Step 1: write `docs_guards.rs` with the five tests above (RED: commands/pages missing).
- [ ] Step 2: implement `docgen.rs`, the CLI/dev/mcp wiring, `InfraForm::binding_type_label`.
- [ ] Step 3: generate `docs/reference/cli.md`, the `resource-files.md` skeleton region, the `MCP.md` region; run the guards (GREEN). Fix any existing doc command lines that fail `docs_commands_parse` (there will be some from 1.x prose — fix the doc, never the parser).
- [ ] Step 4: `CLAUDE.md` subsection; gate; commit `docs: generated CLI/MCP/infra references with drift guards; docs command and link tests`.

---

### Task 2: Reference pages

**Files:**
- Create: `docs/README.md`, `docs/reference/rigg-yaml.md`, `docs/reference/project-yaml.md`, `docs/reference/annotations.md`, `docs/reference/apis.md`, `docs/reference/state.md`, `docs/reference/environment-variables.md`, `docs/reference/exit-codes-and-questions.md`
- Modify: `docs/reference/resource-files.md` (prose around the generated table)
- Modify: `crates/rigg/src/commands/docgen.rs` (`docs-check` checks 3 and 4 become errors now that the pages exist; test-only variables such as `RIGG_ARM_ENDPOINT` are listed in the page under a "Test-only" heading — the check does not distinguish, the page does).

**Interfaces:**
- Consumes: Task 1 markers and guards.
- Produces: page anchors used by Task 3: `rigg-yaml.md#environments`, `rigg-yaml.md#dependencies`, `rigg-yaml.md#policy`, `annotations.md#x-rigg-auth`, `annotations.md#x-rigg-ref`, `state.md#bindings-cache`, `exit-codes-and-questions.md#needs-input`.

**Content contract (per spec §3):** every page = purpose paragraph → complete annotated example → field table (`key · type · required · default · meaning`) → "common mistakes" with the exact CLI output. Sources of truth to read while writing: `crates/rigg-core/src/workspace.rs` (all structs and their serde attributes — default values and renames), `crates/rigg-core/src/registry.rs` (`X_RIGG_*` constants, `providers()`, `infra_refs`), `crates/rigg-core/src/store.rs` (sidecars, state.json shape), `crates/rigg-core/src/binding.rs` (bindings.json shape), `crates/rigg/src/commands/ask.rs` (protocol JSON shape, `--answer`, `--answers-file`), `crates/rigg/src/commands/mod.rs` (exit codes), `crates/rigg/src/commands/credentials.rs` and `easy_auth.rs` (auth carriers), `crates/rigg-client/src/auth.rs` (env vars). Copy a real `state.json` and `bindings.json` from `e2e-test/.rigg/dev/` and anonymise them for the examples.

`docs/README.md` is the index: a "start here" reading order (CONCEPTS → tutorial 1 → how-rigg-works → the rest), a table "I want to… → page", and the list of reference pages with one line each.

- [ ] Step 1: make `docs-check` checks 3 and 4 hard errors (RED until the pages exist).
- [ ] Step 2: write the pages; run `cargo test -p rigg --test docs_guards` until GREEN (commands parse, links resolve, env vars and prefixes covered).
- [ ] Step 3: gate; commit `docs(reference): rigg.yaml, project.yaml, resource files, annotations, apis, state, env vars, exit codes`.

---

### Task 3: Narrative, tutorials and root entry points

**Files:**
- Create: `docs/how-rigg-works.md`, `docs/tutorials/01-pull-an-existing-solution.md`, `docs/tutorials/02-build-from-scratch.md`, `docs/tutorials/03-add-an-environment-and-promote.md`, `docs/tutorials/04-push-to-protected-production.md`
- Modify: `README.md` (≤ 200 lines: problem, what rigg does, install, quick start, Documentation section, exit codes, license), `GETTING_STARTED.md` (pointer page), `.claude/skills/rigg-guide/SKILL.md` (link reference pages), `CHANGELOG.md` (`### Docs` under 2.0.0), `samples/README.md` and both sample project READMEs (point at tutorials; replace portal steps with `rigg az indexer run --watch` / `rigg verify`)

**Interfaces:**
- Consumes: Task 2 anchors; `rigg --help` output for every command used.

**Tutorial contract (spec §5):** each tutorial opens with *Prerequisites* (az login, roles the reader needs, what Azure resources must already exist, estimated cost), then numbered steps; each step = the command, an `# output` block, and 2–3 sentences of why; closes with *What you have now*, *Clean up*, *Next*. Commands and outputs are written from real runs in Task 4 — in this task write the steps with expected outputs taken from the wiremock tests' fixtures and existing `e2e-test/` runs, and mark every output block with `<!-- verify-live -->` so Task 4 can find and replace them.

Tutorial content:
1. **Pull an existing solution** — `mkdir`, `rigg init` (discovery prompts and the non-interactive `--search/--foundry` form), `rigg new project`, `rigg adopt <project> all`, the learn offer → `rigg env bind dev --learn`, `rigg status`, `git init && git add && git commit`, `rigg describe`; round trip: `rigg delete <project> --remote` (dry run then `--yes`), `rigg push <project>` (auth preflight shown), `rigg verify <project>`.
2. **Build from scratch** — `rigg new pipeline docs --source blob` (check the real flag names), `--identity`, edit the data source `ResourceId=`, `rigg validate`, `rigg auth doctor -e dev` then `--fix`, `rigg push`, `rigg az indexer run docs-indexer --watch`, `rigg az knowledge-base ask docs-kb "…"`, then `rigg new agent docs-agent` with `x-rigg-ref`, push, `rigg az agent ask`.
3. **Add an environment and promote** — `rigg env add staging --like dev` (walkthrough of the same/different/skip questions), `rigg env show staging`, `rigg promote <project> --from dev --to staging` (rewiring preview; the `binding.` and `promote.` questions; the `--answer` form), `rigg auth doctor -e staging --fix`, `rigg push -e staging`, `rigg verify -e staging`.
4. **Push to protected production** — `policy: { protected: true, strict-bindings: true }`, `rigg push -e prod` showing the `confirm.protected.prod` question, `--confirm-env prod`, why `--yes` is not enough, `push --verify`, `rigg auth roles list -e prod`, `rigg ci init` and the GitHub Actions workflow it writes with OIDC, and the MCP `needs-input` loop (one JSON exchange).

`how-rigg-works.md` sections: sync classes and baselines; the binding layer and the InfraRef table; the identity requirement graph (link CONCEPTS "How rigg handles authentication"); promote as translation; the question protocol; what rigg never does (provision services, store secrets, grant the operator's roles).

- [ ] Step 1: write all pages; `cargo test -p rigg --test docs_guards` GREEN (every command parses, every link resolves).
- [ ] Step 2: root README/GETTING_STARTED/skill/CHANGELOG/samples; guards GREEN; gate; commit `docs: how rigg works, four tutorials, README as entry point`.

---

### Task 4: Live tutorial run and acceptance skill

**Files:**
- Modify: the four tutorial pages (replace every `<!-- verify-live -->` output with real trimmed output; fix steps that did not work), any small code fix the run exposes (each in its own commit with a test), `.claude/skills/test-complete-enduser-experience/SKILL.md` (rewrite to walk the four tutorials, with the teardown checklist), `docs/superpowers/plans/2026-09-10-ws5b-documentation.md` (Execution record is appended by the controller, not this task)

**Live run rules (authorised by Kristofer; keep cost minimal, clean up):** work in `/Users/kristofer/repos/mklab-se/rigg/e2e-test/tutorials/<n>/` (git-excluded); Search `mklabsrch`, Foundry `mklabaifndr`/`proj-default`, storage `mklabstorageacc`; name everything `tut-*`; tutorial 1 adopts the existing `regulus` resources into a fresh workspace (do NOT delete them remotely — for the round trip, delete and re-push a `tut-` synonym map you created instead, and say so in the tutorial as "we use a throwaway resource here"); tutorial 2 creates a `tut-docs` container with two small text blobs and a `tut-*` pipeline + agent, deletes them at the end (`rigg delete … --remote --yes`, container delete, `rigg auth roles remove`); tutorial 3 adds `staging` pointing at the same services with `tut-staging-*` names; tutorial 4 adds `prod` = same services with `protected: true` and stops at the dry-run + `--confirm-env` demonstration without pushing anything new. Record every command, its exit code and trimmed output in the report; paste anonymised output into the pages (replace subscription/tenant/object ids with zeros, `mklab*` with `contoso-*`).

- [ ] Step 1: run tutorial 1..4 in order, fixing pages as you go.
- [ ] Step 2: rewrite the acceptance skill; guards GREEN; gate; commit `docs(tutorials): verified against live Azure; acceptance skill walks the tutorials`.
- [ ] Step 3: teardown checklist executed and pasted.

---

## Execution record (2026-09-10)

Executed on branch `rigg-2`, commits f57fff0..e36d33e. Rulings:

- Task 1 (generators + guards, 0eb24fb): the Agent tool's worktree isolation cut from `main`, so the task ran on a hand-made worktree from `rigg-2`. `docs-check` lives in `commands/docs_check.rs`; `docs/superpowers/` is excluded from its walk (plans and specs describe unbuilt commands); `rigg mcp tools` is user-visible. The infra-table guard caught its first real drift the same hour (the auth fix wave added `encryptionKey.identity` rows) — regenerated in 1e14524. Review approved.
- Task 2 (reference pages, c54797b): `x-rigg-note` does not exist in code — documented as an inert unknown key; no "since" column for a first release; `rigg env learn` → `rigg env bind <env> --learn`. Review: two Criticals (`x-rigg-pin` claimed registry-default pins; deployment capacity is `sku.capacity`) plus eight Importants (OpenAPI openness rule, `consumed_by`, promote is not a state writer, sidecar top-level-only naming, comment loss on `rigg.yaml` writes, needs-input examples on flows that really ask) — all fixed (0969560); the combined re-review found one new inconsistency (the promote example listed one question, the text answered two) fixed by the controller (e5fd2a8).
- Task 3 (narrative, tutorials, root docs, db4d0c3): written in parallel with Task 2 on a worktree (disjoint files). Review Critical: tutorial 1's round trip used `rigg delete <project> --remote`, which deletes every resource the project owns. Ruling: the live run was stopped before it reached that step (it had created only a throwaway synonym map, regulus verified intact); the round trip is a throwaway synonym map via file delete + `push --prune`; the whole-project delete appears only in a warning (1d43bf0). Also `ci init -e prod`, honest cost lines, the eight-command protected-gate list.
- Task 4 (live run, a4e376e..a9eb03d): all four tutorials executed on `mklabsrch` / `mklabaifndr` / `mklabstorageacc` with `tut-*` names and torn down (inventory identical to pre-run). Five defects found and fixed with tests: `diff` phantom drift on data-source connection strings; `validate --strict` rejecting built-in `Microsoft.*` guardrails; MCP tool `server_label` derivation; knowledge-base scaffold without `models`; absolute paths in learn/adopt proposals. Two doc-only gaps (KB-grounded agent needs a Foundry connection + `project_connection_id`; indexer `fieldMappings`) became tutorial 2 steps. Tutorials 3 and 4 were rescoped to a second environment on the same services with renamed resources.
- Final review (fable): code approved incl. the five fixes; blockers = a real subscription id in CONCEPTS.md (since WS1), the 0.x-era `crates/rigg/doc/ai-reference.md` shipped in the crate, missing changelog `Fixed` entries. Fix wave (b804def, 1d511d4, e36d33e): placeholders everywhere; `rigg ai skill --reference` generated from the binary with a docs-check guard over the emitted output; changelog; CLAUDE.md for 2.0; and the review's Important — a change to a write-only field alone (data-source connection string) is now classified LocalAhead against the `.rigg` baseline and pushed. The first attempt at this wave stalled on an infrastructure watchdog with nothing written and was re-dispatched as two agents.
- Acceptance ruling: issue #6 (fully automated Bicep two-environment e2e) stays open post-2.0.0 — duplicate Search/Foundry/Function infrastructure is the expensive footprint Kristofer asked to avoid; the live tutorial run on existing services is the 2.0.0 acceptance and the `test-complete-enduser-experience` skill now walks the four tutorials.

Deferred (backlog): grounding an agent on a knowledge base still needs three hand-written files; `diff --compare-env` ignores project ownership; `diff --output json` prints prose when empty; `rigg delete` has no `--dry-run`; `validate --strict` is workspace-wide despite `-e`; Azure back-fills skill defaults so a fresh skillset reads `remote ahead` once; `relink_knowledge_bases`/`adopt` record no write-only value (inert while only data sources have one).
