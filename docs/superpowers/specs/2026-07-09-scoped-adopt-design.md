# rigg — Scoped adoption: `rigg adopt`

**Date:** 2026-07-09
**Status:** Design — approved to implement (decisions locked below)
**Workstream:** B of two (A = concept clarity, merged; B = this).

## Problem

Today adoption is all-or-nothing. `rigg pull <project> --adopt <project>` adopts
**every** unmanaged remote resource across **both** services (`pull.rs:95-112`,
`remote.rs:159-177`). There is no way to adopt just one agent, just several
indexes, or just one knowledge base. The surface is also clunky — the project
name is typed twice — and buried under `pull`, so it is hard to discover.

## Goals

Let the user adopt exactly what they choose, à la carte, without being forced to
take a resource's dependency graph:

- adopt a single knowledge base (no Foundry involvement),
- adopt several indexes and nothing else,
- adopt all data sources (a whole kind),
- adopt one or several agents without their dependency graph,
- and *optionally* pull a selected resource's dependency graph — never forced.

## Non-goals / locked decisions

- **No backwards compatibility.** `pull --adopt` is removed outright, not aliased.
- **`--with-deps` follows upstream dependencies only** (what a resource *needs*),
  never downstream dependents (what needs it).
- Adoption remains **Azure-read-only**: it writes local files + baselines, never
  mutates Azure. (Unchanged from today.)

## Design

### Command

```
rigg adopt <project> [<selector>...] [--with-deps] [--dry-run]
```

A first-class top-level verb (shows in `rigg --help`), replacing `pull --adopt`.
Honors global `-e/--env`, `-y/--yes`, `--non-interactive`, `--output`.

### Selectors (positional, one or more)

Same `<kind>/<name>` vocabulary as `rigg diff --only` and `describe` output,
where `<kind>` is a resource **directory name**
(`ResourceKind::from_directory_name`):

- **`<kind>`** — all unmanaged resources of that kind: `indexes`,
  `data-sources`, `agents`, `knowledge-bases`, …
- **`<kind>/<name>`** — one specific resource: `agents/regulus`.
- **`all`** — every unmanaged resource across both services (today's behavior,
  now explicit and opt-in).

If no selector is given, that is a usage error (exit 2) instructing the user to
name a selector or pass `all` — we never adopt-everything by omission.

Domain is implied by kind (`agents`→Foundry, `indexes`→Search). Only services
with a configured connection for the environment are queried; a selector naming
a kind whose service is not configured is a clear error.

### Selector semantics

Parse each selector string into one of:

- `Selector::All`
- `Selector::Kind(ResourceKind)` — from `from_directory_name`; unknown → error
  listing valid kinds.
- `Selector::One(ResourceRef)` — `<kind>/<name>`; unknown kind → same error.

Resolve selectors against the remote snapshot (same snapshot `pull`/`status`
use) to a candidate set of `(ResourceRef, Value)`.

### `--with-deps` (off by default)

For each candidate resource, transitively add its **upstream** references
(`registry::extract_references` on the remote doc), keeping only references that
are (a) present in the snapshot and (b) currently unmanaged. Added resources are
reported tagged `(dependency)`. Bounded by the snapshot; cycles guarded by a
visited-set.

### Ownership rules (invariant preserved)

For each resolved candidate:

- **Owned by another project** → never adopted. If a *specific* `<kind>/<name>`
  selector names such a resource, hard error naming the owner (exit 1). If it is
  only swept in by a `<kind>`/`all`/dependency, silently skip it.
- **Owned by this project already** → no-op (report "already managed" only for an
  explicit named selector).
- **Unmanaged** → adopt: `store.write(r, doc)` + `state.set_baseline(r, doc)`
  (identical mechanics to today's adopt branch), print `+ adopted <ref>`.

A selector (specific or kind) that matches nothing unmanaged prints a warning.

### Confirmation & dry-run

- **Specific `<kind>/<name>` selectors** adopt directly.
- **Broad selectors** (`all`, or a bare `<kind>`) preview the matched list and
  ask to confirm (interactive). `-y/--yes` skips the prompt. In non-interactive
  mode a broad selector **without** `-y` fails (exit 2) telling the user to pass
  `-y` or `--dry-run` — so CI is predictable and never silently bulk-adopts.
- **`--dry-run`** lists what would be adopted for any selector, writes nothing,
  exits 0. Works in every mode.
- Mixed invocations (some specific, some broad selectors in one command) gate on
  the broad ones: preview+confirm the whole resolved set once.

### JSON output

`--output json` emits `{ "adopted": [refs], "skipped": [{ref, reason}],
"would_adopt": [refs] }` (the last only under `--dry-run`) and never prompts
(implies non-interactive rules above).

## Changes elsewhere

- **`pull`**: remove `--adopt` from `PullArgs` and the adopt branch from
  `pull.rs`. `pull` now only syncs resources the project already owns. Its "N
  unmanaged remote resource(s)" line points at `rigg adopt <project> <selector>`.
- **`status`**: the unmanaged-resources hint references `rigg adopt`.
- **`cli.rs`**: new `Adopt(AdoptArgs)` variant + dispatch (async, like `pull`).
- **New `crates/rigg/src/commands/adopt.rs`** holds the command; a small
  `Selector` parser lives with it.
- **Shared snapshot/ownership**: `adopt` computes `owned_by_any` and the snapshot
  the same way `pull`/`status` do. If duplication is more than trivial, factor a
  helper (e.g. `remote`/`store` helper) — decided during planning.
- **Docs**: `rigg adopt --help` long_about with examples; update README and
  GETTING_STARTED adopt references; the `pull` help pointer to `rigg concepts`
  stays.

## Testing

`crates/rigg/tests/sync.rs` (wiremock, no Azure — replace the existing
`pull --adopt` test):

1. `adopt <project> <kind>/<name>` adopts exactly that resource, writes the file,
   records the baseline.
2. `adopt <project> <kind>` adopts all unmanaged of that kind and nothing else.
3. `adopt <project> all -y` adopts the whole unmanaged set.
4. `adopt <project> <ref> --with-deps` also adopts the ref's upstream chain, and
   tags dependencies; a resource outside the chain is NOT adopted.
5. Ownership conflict: adopting a `<kind>/<name>` owned by another project errors
   (exit 1) naming the owner; a `<kind>` sweep skips it silently.
6. `--dry-run` writes no files and records no baseline; exits 0.
7. Broad selector, non-interactive, no `-y` → exit 2 with the "pass -y or
   --dry-run" message; with `-y` → adopts.

`crates/rigg/tests/cli_surface.rs`:

8. `rigg adopt --help` lists selectors and shows examples.
9. `rigg pull --adopt x` now errors (exit 2 — flag removed).
10. `rigg adopt p bogus-kind` errors cleanly listing valid kinds.
11. `rigg adopt p` (no selector) errors (exit 2) asking for a selector or `all`.

## Rollout / files touched

- `crates/rigg/src/commands/adopt.rs` (new)
- `crates/rigg/src/commands/pull.rs` (remove adopt branch; update hint)
- `crates/rigg/src/commands/status.rs` (update unmanaged hint)
- `crates/rigg/src/commands/mod.rs` (register module)
- `crates/rigg/src/cli.rs` (Adopt variant/args, remove PullArgs.adopt, dispatch)
- `crates/rigg/tests/sync.rs`, `crates/rigg/tests/cli_surface.rs`
- `README.md`, `GETTING_STARTED.md`

## Open questions

- None blocking. Snapshot/ownership helper extraction decided during planning.
