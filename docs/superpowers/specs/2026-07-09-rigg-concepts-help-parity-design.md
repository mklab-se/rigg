# rigg — Concept clarity: `rigg concepts`, help parity, empty-state hints

**Date:** 2026-07-09
**Status:** Design — awaiting review
**Workstream:** A of two (A = concept clarity; B = scoped adoption, separate spec)

## Problem

rigg's docs and help define a **project** only by its *mechanics* ("the unit of
sync", "operates on whole projects") and never by its *purpose*. A user cannot
answer, from anything written:

- What is a project, in one sentence, and why does it exist?
- Why would I have one project vs many? How do I choose boundaries?
- How do projects relate to the workspace — why are there two levels?

Evidence (current state):

- README defines a project mechanically: *"Create a project — the unit rigg
  syncs"* (`README.md:80`), heading *"Projects Are the Unit of Sync"*
  (`README.md:158`).
- CLI `long_about`: *"A rigg workspace holds one or more projects; each project
  owns its resource definitions … as JSON files"* (`cli.rs:12`) — mechanics only.
- The load-bearing invariant **a resource belongs to exactly one project** is
  enforced in code and stated in `CLAUDE.md`, but is **absent** from all
  user-facing docs and help.
- The two-level model (workspace vs project) is stated but never *motivated*.
- Secondary gap found while testing: `rigg describe` and `rigg status` print
  **nothing** on an empty workspace (text mode), reading as "did it even run?".

## Goals

1. One authoritative, purpose-first explanation of the workspace/project model.
2. Make it reachable **from the CLI** — "asking `rigg` should be enough"; no need
   to browse to GitHub.
3. Keep the CLI explanation and the docs **identical by construction** (no drift).
4. Fix the silent empty-workspace output as part of the same effort.

## Non-goals

- No change to adoption semantics — that is Workstream B (separate spec). This
  spec only *points at* concepts from `pull`/adopt help.
- No backwards-compatibility constraints (single user; optimize for clarity now).

## Canonical mental model (the content we commit to)

This text is the source material for `CONCEPTS.md` (see Single Source of Truth).

**Workspace vs project**

- A **workspace** (`rigg.yaml`) is the top level: it declares **environments**
  and the **service connections** each environment points at (which Azure AI
  Search service, which Foundry account/project), plus shared assets like
  `apis/`. It holds *no* resource definitions itself.
- A **project** (`projects/<name>/`) is a **named group of resource definitions
  you pull, push, diff, review, and deploy as one unit**. Indexes, indexers,
  agents, deployments, etc. are files inside a project.
- **A resource belongs to exactly one project** — rigg enforces this. It is what
  makes sync unambiguous: when you push a project, rigg knows exactly which
  remote resources that project owns.

**Why two levels?** The workspace answers *"where do things go"* (services and
environments, shared across everything); projects answer *"what do I manage
together"* (the unit of change, review, and deployment). Separating them lets you
promote one coherent project dev→prod without dragging along unrelated resources,
and lets projects be owned and reviewed independently while sharing the same
service/environment config.

**One or many projects? Choosing boundaries.** Use **one** project when the whole
stack ships and is reviewed together (an agent + its RAG pipeline). Use
**several** to draw boundaries you care about:

- **By deployable unit** — each agent/app that ships independently.
- **By ownership / review scope** — a team owns its project; PRs stay scoped.
- **By lifecycle** — things that change on different cadences.

Rule of thumb: *if you would pull/push/review it as a unit, it is a project.*
Because a resource lives in exactly one project, a shared resource goes in the
project that owns it; other projects reference it by name/environment rather than
co-owning it.

## Design

### Component 1 — `CONCEPTS.md` (single source of truth)

- New top-level `CONCEPTS.md`, in the same family as `GETTING_STARTED.md`,
  `INSTALL.md`, `MCP.md`. Markdown, carrying the canonical mental model above,
  plus a short "Workspace layout" recap and a "See also" footer.
- This file is the **only** place the prose lives. Both the CLI command and the
  docs derive from it.

### Component 2 — `rigg concepts` command

- New subcommand `concepts` (top-level, alongside `describe`, `status`).
- Embeds `CONCEPTS.md` at build time via `include_str!` and renders it to the
  terminal with a Markdown renderer (**`termimad`**) — styled headings, lists,
  code spans, tables.
- Rendering rules:
  - TTY + color enabled → styled render via `termimad`.
  - `--no-color`, or non-interactive/non-TTY stdout → plain, unstyled render
    (termimad skin with styling stripped) so piped/CI output stays clean.
  - `--output json` → `{"concepts": "<raw markdown string>"}` for machine use
    (no ANSI). Rendering is a presentation concern; JSON returns the source.
- No network, no workspace required — it must work anywhere, even outside a
  workspace (it is how a new user learns the model before `init`).

### Component 3 — Help cross-references (pointers, not prose)

Add a single line *"See `rigg concepts` for the workspace/project model."* to:

- Root `long_about` (`cli.rs`) — appended after the existing summary.
- `new` command help — so `rigg new project` nudges toward the model.
- `pull` command help — where `--adopt` lives; the concept of "which project"
  matters most there.

No duplicated explanation — pointers only, to keep help output lean and the
prose single-sourced.

### Component 4 — Empty-state hints

When the resolved workspace has **zero projects**:

- `rigg status` and `rigg describe`, **text mode only**, print:
  > `No projects yet. A project groups the resources you manage together —`
  > `see `rigg concepts`, then `rigg new project <name>`.`
- `--output json` is unchanged: `describe` still emits `[]`, `status` its empty
  structure. Machine consumers must not get prose.

## Single Source of Truth & anti-drift

- Prose lives once, in `CONCEPTS.md`. `rigg concepts` embeds that exact file.
- **README** replaces/augments the mechanical "Projects Are the Unit of Sync"
  framing with a short **Concepts** subsection: 2–3 sentences of purpose + a link
  to `CONCEPTS.md` and a mention of `rigg concepts`. README does not re-explain
  the full model (avoids a second copy that can drift).
- **GETTING_STARTED.md** adds a one-line link to `CONCEPTS.md` near its first
  mention of "project".
- Because the CLI renders the same bytes that `CONCEPTS.md` contains, CLI and the
  canonical doc cannot diverge. README/GETTING_STARTED hold only *links* + a short
  teaser, not a copy.

## Dependencies

- Add `termimad` (workspace dependency; used only by the `rigg` crate). Pulls in
  `crossterm`. Pin to the current release at implementation time.

## Testing

`crates/rigg/tests/cli_surface.rs` (assert_cmd, no network):

1. `rigg concepts` exits 0 and its output contains the load-bearing invariant
   sentence and the section headings ("Workspace", "project").
2. `rigg concepts --no-color` output contains no ANSI escape sequences.
3. `rigg concepts --output json` is valid JSON with a `concepts` key.
4. On a freshly `init`ed workspace with no projects, `rigg status` and
   `rigg describe` (text) contain "No projects yet" and the `rigg concepts` /
   `rigg new project` hint.
5. The same two commands with `--output json` emit valid JSON with no prose
   (`describe` → `[]`).
6. Parity guard: a test asserts the invariant sentence *"a resource belongs to
   exactly one project"* is present in the embedded `CONCEPTS.md` (guards against
   someone rewriting the doc and dropping the core rule). Docs↔CLI parity is
   structural (same file embedded), so no cross-file string diff is needed.

## Rollout / files touched

- `CONCEPTS.md` (new)
- `crates/rigg/src/commands/concepts.rs` (new) + wiring in `commands/mod.rs`
- `crates/rigg/src/cli.rs` — new `Concepts` subcommand; help pointers
- `crates/rigg/src/commands/status.rs`, `describe.rs` — empty-state hints
- `Cargo.toml` (workspace) + `crates/rigg/Cargo.toml` — `termimad`
- `README.md`, `GETTING_STARTED.md` — Concepts subsection + links
- `crates/rigg/tests/cli_surface.rs` — tests above

## Open questions

- None blocking. `termimad` version pinned at implementation.
