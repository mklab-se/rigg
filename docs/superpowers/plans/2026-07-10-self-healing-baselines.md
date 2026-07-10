# Self-Healing Baselines + Informed Conflict Prompt Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Baselines store the normalized document (checksums recomputed under current rules — no more false conflicts after rigg upgrades), and pull's conflict prompt shows a field summary with an in-place diff option.

**Architecture:** `store.rs` gains an untagged `Baseline` enum (`Doc(Value)` new / `Checksum(String)` legacy); `set_baseline` stores the compare-normalized canonical doc; `classify` recomputes via `baseline_checksum`. `pull.rs`'s conflict arm computes an in-process diff for a summary line and offers `o/k/d/a` via the existing `confirm::prompt_choice`, rendering the labeled table on `d`.

**Tech Stack:** Rust; rigg-core store, rigg pull; serde untagged enums; existing rigg-diff renderer.

## Global Constraints

- Old `state.json` files (string checksums) MUST load unchanged (serde untagged); their classify behavior is exactly today's. Keys of `baselines` unchanged (callers scan keys).
- New baselines never contain secrets (they are set from remote GETs / push-canonicalized docs where Azure redacts write-only fields) — do not add any secret-stripping logic beyond what `set_baseline`'s input already has, but DO store the compare-normalized form (which strips volatile/read-only noise).
- `--yes` and non-interactive pull behavior unchanged except the non-interactive conflict message gains a `rigg diff` pointer. Exit codes unchanged (conflict → 5).
- `a` (abort) must not write anything further and must not save partial state for resources not yet processed; state already saved for previously-processed resources in this run is acceptable (matches current per-resource processing).
- Every task leaves fmt/clippy(-D warnings)/`cargo test --workspace` green.

---

### Task 1: rigg-core — `Baseline` enum + recomputed checksums

**Files:**
- Modify: `crates/rigg-core/src/store.rs`
- Possibly touch callers of `ProjectState::baseline()` (grep: `crates/rigg/src/commands/pull.rs` uses `state.baseline(r).is_some()`; check for others).

**Interfaces:**
- Produces: `pub enum Baseline { Doc(Value), Checksum(String) }` (serde untagged; order matters — put `Checksum(String)` FIRST if untagged resolution would otherwise misparse, but a JSON string can never parse as `Value::Object`… note: `Value` deserializes ANY JSON including strings! So untagged with `Doc(Value)` first would swallow strings. Order: `Checksum(String)` first, then `Doc(Value)`. VERIFY with a round-trip test that a string loads as Checksum and an object as Doc.)
- Changes: `pub baselines: BTreeMap<String, Baseline>`; `set_baseline` stores the compare-normalized canonical doc; new `fn baseline_checksum(&self, r: &ResourceRef) -> Option<String>`; `classify` uses it; `baseline()` becomes `pub fn has_baseline(&self, r: &ResourceRef) -> bool` (update callers).

- [ ] **Step 1: Read store.rs fully** — especially `checksum` (line ~337: `canonical_form(&normalize_for_compare(kind, value))` then hash), `set_baseline`, `classify`, and how `normalize_for_compare` is imported. The stored doc form should be exactly `canonical_form(&normalize_for_compare(kind, value))` so `baseline_checksum` can hash it directly. IMPORTANT SUBTLETY: `baseline_checksum` for `Doc(v)` must RE-APPLY current normalization before hashing — i.e. compute `Self::checksum(kind, v)` (which re-runs `normalize_for_compare`) — because a doc stored under OLD rules may retain fields that are volatile TODAY; re-normalizing strips them. `normalize_for_compare` must be idempotent (verify: it strips fields — stripping twice is safe).

- [ ] **Step 2: Write failing unit tests** (store.rs `mod tests` — adapt to existing helpers; note `checksum` needs a `ResourceKind`, use `ResourceKind::Agent` and the real volatile field `metadata.modified_at`):

```rust
    #[test]
    fn legacy_checksum_baseline_still_loads_and_classifies() {
        // A state.json written by an older rigg: baseline is a bare string.
        let json = r#"{"baselines": {"agents/a": "deadbeef"}}"#;
        let state: ProjectState = serde_json::from_str(json).unwrap();
        let r = ResourceRef::new(ResourceKind::Agent, "a".to_string());
        assert!(state.has_baseline(&r));
        // Stale hash + differing local/remote → Conflict (today's behavior).
        let local = serde_json::json!({"name": "a", "model": "x"});
        let remote = serde_json::json!({"name": "a", "model": "y"});
        assert_eq!(state.classify(&r, Some(&local), Some(&remote)), SyncClass::Conflict);
    }

    #[test]
    fn doc_baseline_self_heals_across_rule_changes() {
        // Simulate a baseline stored BEFORE metadata.modified_at became
        // volatile: the stored doc still carries the field. Under current
        // rules the recomputed checksum strips it, so an untouched local
        // (without the field) plus a remote-only change classifies as
        // RemoteAhead — NOT Conflict.
        let r = ResourceRef::new(ResourceKind::Agent, "a".to_string());
        let old_doc = serde_json::json!({
            "name": "a", "model": "x",
            "metadata": {"modified_at": "111", "logo": "l.svg"}
        });
        let mut state = ProjectState::default();
        state.baselines.insert(r.key(), Baseline::Doc(old_doc));
        let local = serde_json::json!({
            "name": "a", "model": "x", "metadata": {"logo": "l.svg"}
        });
        let remote = serde_json::json!({
            "name": "a", "model": "CHANGED", "metadata": {"logo": "l.svg"}
        });
        assert_eq!(state.classify(&r, Some(&local), Some(&remote)), SyncClass::RemoteAhead);
    }

    #[test]
    fn baseline_serde_mixed_roundtrip() {
        let r = ResourceRef::new(ResourceKind::Agent, "new".to_string());
        let mut state = ProjectState::default();
        state.baselines.insert("agents/legacy".to_string(), Baseline::Checksum("abc".to_string()));
        state.set_baseline(&r, &serde_json::json!({"name": "new", "model": "m"}));
        let text = serde_json::to_string(&state).unwrap();
        let back: ProjectState = serde_json::from_str(&text).unwrap();
        assert!(matches!(back.baselines.get("agents/legacy"), Some(Baseline::Checksum(s)) if s == "abc"));
        assert!(matches!(back.baselines.get("agents/new"), Some(Baseline::Doc(_))));
    }
```

(If `ProjectState` lacks `Default`, derive it or construct via serde. Adjust `SyncClass` import to the real path. If `metadata.modified_at` is not the ideal volatile field for Agent in `normalize_for_compare`, pick any field the compare-normalization strips for that kind — verify by reading `normalize_for_compare`.)

- [ ] **Step 3: Confirm RED**, implement:

```rust
/// A sync baseline. Newer rigg versions store the compare-normalized
/// document so the checksum can be recomputed under CURRENT normalization
/// rules — surviving rule evolution across rigg upgrades. Legacy entries
/// hold only the frozen checksum and behave as before until the resource
/// next syncs (every successful pull/push/adopt rewrites its baseline).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Baseline {
    /// Legacy: frozen checksum (string MUST be tried first — `Value`
    /// deserializes any JSON, including strings).
    Checksum(String),
    /// Compare-normalized canonical document.
    Doc(Value),
}
```

- `set_baseline`: `Baseline::Doc(canonical_form(&normalize_for_compare(r.kind, value)))` — reuse the exact functions `checksum` uses.
- `baseline_checksum(&self, r)`: `Checksum(s)` → `Some(s.clone())`; `Doc(v)` → `Some(Self::checksum(r.kind, v))`.
- `classify`: replace `self.baseline(r)` with `self.baseline_checksum(r)` (adjust the `Option<&str>` match to `Option<String>`).
- `has_baseline(&self, r) -> bool`; update pull.rs's `state.baseline(r).is_some()` (grep for `.baseline(` across crates — update every caller; keep behavior identical).

- [ ] **Step 4: GREEN + full checks** — `cargo test -p rigg-core 2>&1 | tail -4 && cargo test --workspace 2>&1 | grep -c 'test result: ok' && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`.
NOTE: sync.rs integration tests exercise baselines heavily (adopt → pull cycles) — they must pass UNCHANGED; if one fails, the enum or classify port is wrong, not the test.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-core/src/store.rs crates/rigg/src/commands/pull.rs
git commit -m "feat: baselines store normalized docs — checksums recomputed under current rules"
```

(Include any other `.baseline(`-caller files the grep found.)

---

### Task 2: informed conflict prompt

**Files:**
- Modify: `crates/rigg/src/commands/pull.rs`
- Test: `crates/rigg/tests/sync.rs` (non-interactive message), unit test in pull.rs or a suitable module for the summary helper

**Interfaces:**
- Produces (pull.rs): `fn conflict_summary(kind: ResourceKind, local: &Value, remote: &Value) -> String` — e.g. `4 field(s) differ (model, reasoning, metadata.microsoft.voice-live.enabled, …)`: count + up to 3 field paths + `, …` when more.
- Consumes: `rigg_diff::semantic::diff`, `rigg_diff::output::{format_text, SideLabels}`, `confirm::prompt_choice`, `normalize_for_push`.

- [ ] **Step 1: Write the failing tests.**

a) Unit (pull.rs `mod tests` — create the module if absent):

```rust
    #[test]
    fn conflict_summary_counts_and_names_fields() {
        let local = serde_json::json!({"name": "a", "model": "x", "p": 1, "q": 2, "r": 3});
        let remote = serde_json::json!({"name": "a", "model": "y", "p": 9, "q": 8, "r": 7});
        let s = conflict_summary(ResourceKind::Agent, &local, &remote);
        assert!(s.starts_with("4 field(s) differ ("), "{s}");
        assert!(s.contains("model"), "{s}");
        assert!(s.ends_with(", …)") || s.matches(',').count() >= 2, "at most 3 named: {s}");
    }
```

b) sync.rs: find the existing non-interactive conflict test (exit 5); extend its stdout assertion with the new pointer:

```rust
        .stdout(predicate::str::contains("rigg diff"))
```

- [ ] **Step 2: Confirm RED**, implement in pull.rs's `SyncClass::Untracked | SyncClass::Conflict` arm:

```rust
            SyncClass::Untracked | SyncClass::Conflict => {
                let summary = local
                    .as_ref()
                    .map(|l| conflict_summary(r.kind, l, doc))
                    .unwrap_or_else(|| "differs locally and remotely".to_string());
                if ctx.yes {
                    store.write(r, doc)?;
                    state.set_baseline(r, doc);
                    println!("  {} overwrote {}", "~".cyan(), r);
                    written += 1;
                } else if ctx.interactive() {
                    println!("  {} {} — {}", "conflict".red().bold(), r, summary);
                    let mut show_diff_option = true;
                    loop {
                        let opts: &[char] = if show_diff_option {
                            &['o', 'k', 'd', 'a']
                        } else {
                            &['o', 'k', 'a']
                        };
                        let label = if show_diff_option {
                            "  [o]verwrite local with remote / [k]eep local / [d]iff / [a]bort pull ?"
                        } else {
                            "  [o]verwrite local with remote / [k]eep local / [a]bort pull ?"
                        };
                        match confirm::prompt_choice(label, opts)? {
                            'o' => {
                                store.write(r, doc)?;
                                state.set_baseline(r, doc);
                                println!("  {} overwrote {}", "~".cyan(), r);
                                written += 1;
                                break;
                            }
                            'k' => {
                                println!("  kept local {r}");
                                break;
                            }
                            'd' => {
                                if let Some(l) = &local {
                                    let result = rigg_diff::semantic::diff(
                                        &normalize_for_push(r.kind, doc),
                                        &normalize_for_push(r.kind, l),
                                        "name",
                                    );
                                    let labels = rigg_diff::output::SideLabels {
                                        new_side: "local".to_string(),
                                        old_side: format!("Azure ({})", env.name),
                                    };
                                    println!();
                                    print!("{}", rigg_diff::output::format_text(&result, &r.to_string(), &labels));
                                    println!();
                                }
                                show_diff_option = false;
                            }
                            'a' => {
                                state.save(ws, &env.name, &project.name)?;
                                return Err(anyhow!("aborted"));
                            }
                            _ => unreachable!(),
                        }
                    }
                } else {
                    println!(
                        "  {} {} — {} (run `rigg diff {}` to inspect; pass --yes to overwrite)",
                        "conflict".red().bold(),
                        r,
                        summary,
                        project.name
                    );
                    any_conflict = true;
                }
            }
```

And the helper:

```rust
/// One-line conflict summary: change count + first few differing fields.
fn conflict_summary(kind: ResourceKind, local: &Value, remote: &Value) -> String {
    let result = rigg_diff::semantic::diff(
        &normalize_for_push(kind, remote),
        &normalize_for_push(kind, local),
        "name",
    );
    let n = result.changes.len();
    let mut fields: Vec<&str> = result.changes.iter().take(3).map(|c| c.path.as_str()).collect();
    let suffix = if n > 3 {
        fields.push("…");
        ""
    } else {
        ""
    };
    format!("{n} field(s) differ ({}{})", fields.join(", "), suffix)
}
```

(Clean up the suffix logic — the sketch's intent: up to 3 names, then `, …` when more. Match the file's existing imports: `normalize_for_push` is already imported in pull.rs's diff-adjacent code? VERIFY — pull.rs may not import it; add `use rigg_core::normalize::normalize_for_push;` per the actual module path used in diff.rs. Adjust the existing `--yes` branch restructure carefully: today `ctx.yes || (interactive && prompt)` is one combined condition — the new structure splits it; preserve exact `--yes` and non-interactive semantics, and keep the existing baseline/save flow.)

NOTE on `'a'`: saving state before erroring matches the constraint (resources already processed keep their refreshed baselines); verify `state.save` is idempotent with the function's final save (early return skips it — hence the explicit save).

- [ ] **Step 3: GREEN + full checks** — the full battery; sync.rs conflict tests must pass with the extended assertion; all other pinned behavior unchanged.

- [ ] **Step 4: Commit**

```bash
git add crates/rigg/src/commands/pull.rs crates/rigg/tests/sync.rs
git commit -m "feat: informed pull conflict prompt — field summary, in-place diff, abort"
```

---

## Final Verification

Full battery; then live acceptance (the user): `rigg pull` in e2e-test → the legacy-baseline conflict prompts ONCE with the new summary + `d` option; after resolving with `o`, the baseline heals to Doc form; subsequent portal-only changes classify RemoteAhead (no prompt).

## Self-Review notes

- Spec §1 → Task 1 (untagged order Checksum-first is the critical serde subtlety; discriminating test pair legacy-Conflict vs doc-RemoteAhead). Spec §2 → Task 2 (o/k/d/a; d re-ask without d; abort saves processed state; non-interactive pointer).
- Signature threads: `has_baseline` replaces `baseline()` with all callers updated in Task 1; `conflict_summary` unit-tested; diff orientation old=remote/new=local matches `rigg diff`.
- Sanctioned test changes: only the sync.rs conflict-message assertion extension.
