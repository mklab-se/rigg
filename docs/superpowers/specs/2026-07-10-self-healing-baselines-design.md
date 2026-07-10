# rigg — Self-healing baselines + informed conflict prompt

**Date:** 2026-07-10
**Status:** Design — approved (user-directed)
**Workstream:** G (from live Regulus testing).

## Problems

1. **False conflicts after rigg upgrades.** Baselines store only a checksum of
   the push-normalized doc (`store.rs:307`, `set_baseline`). When a rigg
   upgrade changes normalization rules (e.g. Workstream F made
   `metadata.modified_at` volatile), every existing baseline goes stale: the
   untouched local file now hashes differently than its baseline, so a
   remote-only change classifies as **Conflict** instead of RemoteAhead. Live
   repro: the user's `rigg pull` prompted to overwrite a file they never
   edited. Rule evolution is routine (twice this session) — checksum-only
   baselines structurally cannot survive it.
2. **The conflict prompt is information-free.** `conflict agents/Regulus
   differs locally and remotely — overwrite? [y/N]` demands consent without
   evidence; the user must abort, run `rigg diff`, and start over. Git never
   does this: information precedes decision.

## Design

### 1. Self-healing baselines (rigg-core, store.rs)

```rust
/// A sync baseline. Newer rigg versions store the compare-normalized
/// document so the checksum can be recomputed under CURRENT normalization
/// rules — surviving rule evolution across upgrades. Legacy entries hold
/// only the frozen checksum and behave as before until rewritten.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub enum Baseline {
    Doc(Value),
    Checksum(String),
}
```

- `baselines: BTreeMap<String, Baseline>` — untagged serde keeps old
  `state.json` files (string entries) loading unchanged.
- `set_baseline` stores `Baseline::Doc(<compare-normalized canonical form>)`
  (the same form the checksum hashes today, unhashed — a few KB per resource;
  never contains secrets, since baselines are set from remote GETs where
  Azure redacts write-only fields).
- `baseline_checksum(&self, r) -> Option<String>`: `Doc(v)` → recompute
  `checksum(kind, v)` under current rules; `Checksum(s)` → `s` verbatim.
- `classify` uses `baseline_checksum`. All other semantics unchanged.
- Existing `baseline()` presence-check callers (pull's deletion detection,
  `owned_by_any` key scans) keep working — keys are unchanged; adjust the
  accessor signature as needed.
- **Healing:** every successful pull/push/adopt already rewrites baselines;
  legacy checksum entries upgrade to `Doc` on the next sync of that resource.
  One legacy-era false conflict can still occur (nothing can distinguish a
  stale-rule hash from a real local edit) — the informed prompt (below) makes
  that one occurrence survivable.

### 2. Informed conflict prompt (pull.rs)

Interactive Untracked/Conflict arm becomes:

```
  conflict agents/Regulus — 4 field(s) differ (model, reasoning, metadata.…)
  [o]verwrite local with remote / [k]eep local / [d]iff / [a]bort pull ?  d

  <full labeled diff table renders here (local | Azure (<env>))>

  [o]verwrite local with remote / [k]eep local / [a]bort pull ?
```

- Summary line: change count + up to the first three field paths, computed
  in-process via `rigg_diff::semantic::diff` (old=remote, new=local — same
  orientation as `rigg diff`).
- Choices via the existing `confirm::prompt_choice`: `o` = overwrite local +
  set baseline (today's `y`); `k` = keep local, baseline untouched (today's
  `n`); `d` = render the full table with `SideLabels { local, Azure (<env>) }`
  then re-ask (without `d` the second time); `a` = abort the entire pull
  cleanly (error "aborted", nothing further written).
- `--yes` unchanged (overwrite). Non-interactive unchanged (exit 5) but the
  message gains the pointer: `run \`rigg diff <project>\` to inspect; pass
  --yes to overwrite`.

## Non-goals

- No merge tooling changes (`rigg ai` conflict merge is separate).
- No attempt to auto-migrate legacy checksums (impossible to disambiguate).

## Testing

- store.rs: legacy string entry loads and classifies exactly as today
  (construct state JSON with a deliberately stale hash → Conflict);
  `Baseline::Doc` self-heals — a baseline doc carrying a field that is
  volatile under current rules classifies an untouched local as
  InSync/RemoteAhead (the discriminating pair against the legacy entry);
  serde round-trip of mixed maps; new baselines serialize as objects.
- pull.rs: pure `conflict_summary(kind, local, remote) -> String` helper unit
  tested (count + first three fields + ellipsis). Non-interactive conflict
  message includes the `rigg diff` pointer (extend the existing exit-5 sync
  test). Interactive o/k/d/a is TTY-bound — live acceptance: the user's very
  next `rigg pull` (their legacy baseline + real portal drift) exercises the
  informed prompt, `d` in place, then heals the baseline to `Doc` form.

## Files touched

`crates/rigg-core/src/store.rs`; `crates/rigg/src/commands/pull.rs`;
tests inline + `crates/rigg/tests/sync.rs`.
