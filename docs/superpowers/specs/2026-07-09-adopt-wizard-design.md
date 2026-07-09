# rigg — Interactive adopt wizard + adopt-path discoverability

**Date:** 2026-07-09
**Status:** Design — approved
**Workstream:** C (A = concept clarity, merged; B = scoped adopt, merged).

## Problem

Two discoverability gaps remain on the adoption path, found by critically
walking the real user journey:

1. **You must already know your resource names.** `rigg adopt regulus
   agents/regulus` requires knowing that Regulus is an *agent* and what it is
   called in Azure — knowledge that today requires opening the Azure or Foundry
   portal. The tool has this information (the remote snapshot) but never offers
   it.
2. **Nothing routes you to `adopt`.** After `rigg new project`, the success
   output only mentions scaffolding new resources (`rigg new <kind> …`); the
   adopt path is invisible. And nothing explains how to *name* a project.

(A third gap — `init` suggesting the removed `pull --adopt` — was a regression,
already fixed with a guard test in this workstream: commit 1e466ff.)

## Goals

- `rigg adopt` (with missing arguments) becomes a **wizard**: it asks for
  whatever you didn't type — project, then resources (queried live from Azure),
  then dependencies — so the portal is never needed to discover what exists.
- Both services — Azure AI Search and Microsoft Foundry — supported **each
  alone and both at once**, in one wizard run.
- Signpost the adopt path from `rigg new project`; add project-naming guidance.
- Scriptable/CI behavior is completely unchanged.

## Non-goals

- Wizards for other commands (`new`, `delete`, …). The prompt layer is built to
  be reusable, but only `adopt` gets a wizard now (YAGNI).
- No change to adopt's core semantics (selectors, ownership rules, `--with-deps`
  expansion, JSON output) — the wizard is a front-end that produces the same
  resolved set and runs the same write path.

## Design

### Wizard activation

The wizard fires only when ALL hold:

- `ctx.interactive()` is true (TTY stdout, no `--non-interactive`, no `--yes` —
  `-y` means "ask me nothing", so it disables the wizard along with every other
  prompt; a selector-less `-y` invocation stays a usage error) and NOT
  `--output json`;
- something is missing: no project argument, or no selectors.

Otherwise behavior is exactly as shipped in Workstream B: bare `rigg adopt`
or `rigg adopt <project>` without selectors in non-interactive mode remains a
usage error (exit 2). CI is unaffected.

### Wizard flow

1. **Project** (skipped if given as argument):
   - exactly one project in the workspace → auto-select it and say so;
   - several → `Select` menu of project names;
   - zero → offer to create one: prompt for a name, run the same scaffolding as
     `rigg new project`, continue. (The wizard never dead-ends a fresh
     workspace.)
2. **Resources** (skipped if selectors given): fetch the same remote snapshot
   the CLI path uses; list **unmanaged** resources in a `MultiSelect` (inquire:
   space toggles, typing fuzzy-filters), as `<kind>/<name>` entries **grouped
   by service** with header rows:
   - `── Microsoft Foundry (<account>/<project>) ──` then agents, deployments,
     connections, guardrails;
   - `── Azure AI Search (<service>) ──` then indexes, indexers, data sources,
     skillsets, synonym maps, aliases, knowledge sources, knowledge bases.
   - One run may tick resources from both services (mixed adoption is already
     supported by the resolved-set machinery).
   - If the environment configures only one service, only that service is
     queried and shown — no empty section, no error.
   - If a configured service is unreachable (auth/network), FAIL with an error
     naming that service. Never show a silently partial list — an incomplete
     menu would read as "the resource doesn't exist" when the truth is "auth is
     broken".
   - If nothing is unmanaged: print "Nothing to adopt — everything visible is
     already managed." and exit 0.
3. **Dependencies** (skipped if `--with-deps` given): compute the upstream
   expansion for the ticked set first; only if it would add ≥1 resource, ask
   `Also adopt their N upstream dependencies? [y/N]`. No noise question when
   there is nothing to add.
4. **Preview + confirm** (always in wizard mode, even for specific picks,
   because dependency expansion can add resources the user did not tick):
   final list with `(dependency)` tags, then `Proceed? [Y/n]`.
5. **Adopt** via the exact same code path as the CLI (write files + baselines,
   same report output).
6. **Teach the scriptable form**: after success, print
   `hint: next time: rigg adopt <project> <sel> [<sel>…] [--with-deps]`
   reconstructing the equivalent non-interactive invocation from what was
   chosen.

### Prompt layer

New `crates/rigg/src/commands/interactive.rs` wrapping `inquire` (`Select`,
`MultiSelect`, `Confirm`) behind small functions that:

- honor `--no-color` (plain `RenderConfig`);
- return `anyhow::Result` and translate inquire's cancel/interrupt
  (Esc/Ctrl-C) into a clean "aborted" error → exit 1, nothing written.

`inquire` is already in the dependency tree at 0.7.5 via ailloy's `config-tui`
feature — adding it as a direct dependency compiles no new code.

The existing hand-rolled helpers in `confirm.rs` stay for the plain yes/no
paths used elsewhere; the wizard uses inquire throughout for consistency of
look and feel within a single flow.

### Signposting + naming guidance

- `rigg new project <name>` success output becomes two next-step lines:
  ```
  Adopt existing Azure resources:  rigg adopt <name>          (interactive)
  Or scaffold new ones:            rigg new <kind> <name> -p <name>
  ```
- CONCEPTS.md "Choosing boundaries" gains one sentence: *Name a project after
  the thing it owns — e.g. a project holding the `regulus` agent and its
  retrieval stack is naturally called `regulus`.*
- `rigg new --help` `name` argument doc gains the naming rules hint (letters/
  digits/dashes typical; no `/`, `\`; ≤260 chars) and the "name it after what
  it owns" pointer.

## Testing

Wizard prompts are TTY-bound (assert_cmd cannot drive inquire), so the split
is:

- **Pure logic, unit/wiremock tested**: building the grouped candidate list
  from a snapshot (ordering, service grouping, unmanaged filtering); the
  equivalent-command hint string; wizard-activation predicate (interactive ×
  json × missing-args matrix).
- **Pinned by existing tests**: non-interactive bare `adopt` → exit 2;
  `adopt <project>` without selectors non-interactive → exit 2; all Workstream
  B semantics.
- **cli_surface additions**: `new project` output mentions `rigg adopt`;
  CONCEPTS.md contains the naming sentence.
- **Live acceptance**: the wizard itself is verified against real Azure in the
  e2e-test workspace (the Regulus adoption scenario) — the deliberate purpose
  of this session's e2e exercise.

## Files touched

- `crates/rigg/src/commands/interactive.rs` (new)
- `crates/rigg/src/commands/adopt.rs` (wizard orchestration; refactor `run`
  so the resolved-set + write path is callable from both entries)
- `crates/rigg/src/commands/new.rs` (signpost), `crates/rigg/src/cli.rs`
  (name-arg doc)
- `CONCEPTS.md` (naming sentence)
- `Cargo.toml` + `crates/rigg/Cargo.toml` (`inquire = "0.7"`)
- `crates/rigg/tests/cli_surface.rs`, `crates/rigg/tests/sync.rs`

## Open questions

None blocking.
