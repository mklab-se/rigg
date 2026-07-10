# Direction-Neutral Diff Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rigg diff` renders a labeled local-vs-Azure table (no temporal words), prints dual-direction hints after drift, the AI summary goes direction-neutral, and `metadata.modified_at` stops polluting agent diffs.

**Architecture:** `rigg-diff::output` gains a `SideLabels` struct threaded through `format_report`; text becomes an aligned two-column table, markdown a Markdown table; JSON untouched. `diff.rs` supplies labels ("local" / "Azure (<env>)", or env names in compare mode) and prints hints. `ai_assist::explain_diff` gets a neutral prompt. Registry: one volatile field.

**Tech Stack:** Rust; rigg-diff, rigg, rigg-core; existing wiremock tests in sync.rs.

## Global Constraints

- The diff's internal orientation stays old=remote/right, new=local/left (diff.rs comment) — ONLY presentation changes. Column order: `new_side` (local) first, `old_side` (Azure) second.
- No temporal words ("was", "now") anywhere in text/markdown diff output.
- Hints: text format + local-vs-remote mode + drift present, only. Named project when exactly one project has drift; `<project>` placeholder otherwise. Never in compare-env/markdown/json.
- JSON diff format byte-identical to today.
- Existing sync.rs diff assertions MAY be updated to the new shape (sanctioned breaking change); everything else pinned.
- Every task leaves fmt/clippy(-D warnings)/`cargo test --workspace` green.

---

### Task 1: rigg-diff — labeled table renderers

**Files:**
- Modify: `crates/rigg-diff/src/output.rs`

**Interfaces:**
- Produces: `pub struct SideLabels { pub new_side: String, pub old_side: String }`
- Changes: `pub fn format_report(diffs: &[(String, DiffResult)], format: OutputFormat, labels: &SideLabels) -> String` (and `format_text`/`format_markdown` likewise; `format_json` ignores labels). Callers updated in Task 2 — this task updates output.rs's own unit tests only; expect the workspace build to break until Task 2 IF other crates call these. CHECK first: if `crates/rigg/src/commands/diff.rs` is the only caller, do Task 1+2 in ONE commit to keep every commit green (fold Task 2's diff.rs label-wiring — NOT the hints — into this commit and say so in the report).

- [ ] **Step 1: Read the current output.rs fully.** Note: `format_text` prints `"{resource}: N change(s)"` then per-change lines via `format_change_text`; `Change` has `path`, `kind` (Added/Removed/Modified), `old_value`, `new_value`, `description: Option<String>`. `format_report` dispatches by `OutputFormat`.

- [ ] **Step 2: Write failing unit tests** (output.rs `mod tests` — adapt to the existing test helpers you find there):

```rust
    #[test]
    fn table_renders_both_sides_with_labels_no_temporal_words() {
        let result = diff(
            &json!({"name": "a", "model": "gpt-5.6-luna"}),   // old = Azure
            &json!({"name": "a", "model": "gpt-5.2-chat"}),   // new = local
            "name",
        );
        let labels = SideLabels {
            new_side: "local".to_string(),
            old_side: "Azure (dev)".to_string(),
        };
        let out = format_text(&result, "regulus/agents/Regulus", &labels);
        assert!(out.contains("local"), "{out}");
        assert!(out.contains("Azure (dev)"), "{out}");
        assert!(out.contains("gpt-5.2-chat") && out.contains("gpt-5.6-luna"), "{out}");
        // local column before Azure column on the model row
        let row = out.lines().find(|l| l.contains("model")).unwrap();
        let li = row.find("gpt-5.2-chat").unwrap();
        let ri = row.find("gpt-5.6-luna").unwrap();
        assert!(li < ri, "local value first: {row}");
        assert!(!out.contains(" was "), "{out}");
        assert!(!out.contains(" now "), "{out}");
    }

    #[test]
    fn table_renders_absent_for_one_sided_values() {
        let result = diff(
            &json!({"name": "a", "reasoning": {"effort": "high"}}), // old/Azure has it
            &json!({"name": "a"}),                                   // new/local lacks it
            "name",
        );
        let labels = SideLabels { new_side: "local".into(), old_side: "Azure (dev)".into() };
        let out = format_text(&result, "r", &labels);
        assert!(out.contains("(absent)"), "{out}");
        assert!(out.contains("1 key)") && !out.contains("1 keys"), "pluralization: {out}");
    }

    #[test]
    fn markdown_is_a_table_with_side_columns() {
        let result = diff(
            &json!({"name": "a", "model": "x"}),
            &json!({"name": "a", "model": "y"}),
            "name",
        );
        let labels = SideLabels { new_side: "local".into(), old_side: "Azure (dev)".into() };
        let out = format_markdown(&[("p/agents/a".to_string(), result)], &labels);
        assert!(out.contains("| field |") || out.contains("| Field |"), "{out}");
        assert!(out.contains("| local |") || out.contains("local |"), "{out}");
        assert!(!out.contains(" was "), "{out}");
    }
```

(Adjust `diff(old, new, "name")` argument names/order to the real `semantic::diff` signature; the point is old=Azure-side, new=local-side.)

- [ ] **Step 3: Confirm RED**, then implement:

a) `SideLabels` struct (with doc comments per the spec).

b) `format_text(result, resource_name, labels)`:
- Header line: `"{resource_name} — differs ({n} field(s))"` followed by a blank line.
- Column layout: compute the field-column width from the longest path (cap ~40 chars; longer paths get their own line with values on the next line, or simply let the row overflow — pick the simpler: fixed-width `{:<40}` and let long paths push the row wider). Then `{:<40} {:<20} {}`-style: field, new_side value, old_side value. Header row uses the label strings.
- Modified: both previews. Added (only in new/local): new preview + `(absent)`. Removed (only in old/Azure): `(absent)` + old preview.
- `description`-carrying changes: print the description as a full-width row (indented, no columns).

c) `format_markdown`: per resource a `### {resource}` heading + `| field | {new_side} | {old_side} |` table with the same cell rules (escape `|` in values by replacing with `\|`).

d) `format_value_preview`: fix `{{...}} ({} keys)` → singular/plural (`1 key` / `n keys`).

e) `format_report` threads `labels` to text/markdown; JSON path untouched.

f) Update existing output.rs tests that assert the old shape ("was") to the new expectations.

- [ ] **Step 4: Wire the caller** (fold from Task 2 if needed to keep the commit green): in `crates/rigg/src/commands/diff.rs`, build labels:
- local mode: `SideLabels { new_side: "local".into(), old_side: format!("Azure ({})", env_b.name) }`
- compare-env mode: `SideLabels { new_side: env_a.name.clone(), old_side: env_b.name.clone() }` (verify against the pair construction: `pairs.push((r, a, b))` then `diff(&right_n /*b*/, &left_n /*a*/)` → new=a, old=b).
Pass to `format_report`. Update sync.rs diff-shape assertions (grep for tests asserting `was`/`change(s)` in diff output) to the new table shape.

- [ ] **Step 5: GREEN + full checks** — `cargo test -p rigg-diff 2>&1 | tail -4 && cargo test --workspace 2>&1 | grep -c 'test result: ok' && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`.

- [ ] **Step 6: Commit**

```bash
git add crates/rigg-diff/src/output.rs crates/rigg/src/commands/diff.rs crates/rigg/tests/sync.rs
git commit -m "feat: direction-neutral diff — labeled local/Azure table, no temporal words"
```

---

### Task 2: hints after drift

**Files:**
- Modify: `crates/rigg/src/commands/diff.rs`
- Test: `crates/rigg/tests/sync.rs`

**Interfaces:** none new.

- [ ] **Step 1: Failing sync test** — a drifted local-vs-remote diff must print both hints; a clean diff must not:

```rust
#[tokio::test]
async fn diff_prints_dual_direction_hints_on_drift() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs", "fields": [{"name":"id","type":"Edm.String","key":true}]}]
        }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/indexes/docs"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "docs",
            "fields": [{"name":"id","type":"Edm.String","key":true},
                       {"name":"extra","type":"Edm.String"}]
        }))).mount(&server).await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());
    // local file WITHOUT the extra field → drift
    write_resource(ws.path(), "indexes", "docs", &json!({
        "name": "docs", "fields": [{"name":"id","type":"Edm.String","key":true}]
    }));

    rigg(ws.path()).args(["diff", "demo"]).assert()
        .stdout(predicate::str::contains("rigg pull demo"))
        .stdout(predicate::str::contains("rigg push demo"))
        .stdout(predicate::str::contains("update local files to match Azure"));
}
```

NOTE: check how existing sync diff tests mock the per-resource GET (`remote_b.get(r)`) — mirror their mock paths exactly (the `/indexes/docs` single-GET path and any api-version matchers). If an existing drifted-diff test exists, extend IT instead of writing a new mock scaffold.

- [ ] **Step 2: Confirm RED**, implement in diff.rs `run`: after printing the report, when `has_drift && args.format == DiffFormat::Text && compare_env.is_none()` (verify the actual field name for compare mode), collect the drifted project names from the diff results (the `String` keys are `"{project}/{kind}/{name}"` — split on first '/'); if exactly one distinct project, use it, else `<project>`:

```rust
        println!();
        println!("hint: rigg pull {p} — update local files to match Azure");
        println!("      rigg push {p} — make Azure match your local files");
```

- [ ] **Step 3: GREEN + full checks** (same command battery).

- [ ] **Step 4: Commit**

```bash
git add crates/rigg/src/commands/diff.rs crates/rigg/tests/sync.rs
git commit -m "feat: dual-direction pull/push hints after drifted diff"
```

---

### Task 3: neutral AI prompt, volatile modified_at, README

**Files:**
- Modify: `crates/rigg/src/commands/ai_assist.rs`
- Modify: `crates/rigg-core/src/registry.rs` (+ its tests)
- Modify: `README.md`

- [ ] **Step 1: Rewrite `explain_diff`'s system prompt** (ai_assist.rs:18-20):

```rust
    let system = "You explain configuration differences between a developer's LOCAL files and \
                  what is currently in AZURE (Azure AI Search / Microsoft Foundry). The report \
                  labels each side — attribute every value to the correct side and NEVER assume \
                  the user intends to push or pull. Structure your answer as: one or two lines on \
                  what differs (interpret, don't restate every field); then 'If you pull:' — what \
                  the local files would become; then 'If you push:' — what would change in Azure, \
                  flagging risks under that direction only (deletions, immutable index fields, \
                  SKU/capacity/billing). Max 150 words.";
```

- [ ] **Step 2: Registry** — Agent KindMeta `volatile_fields` gains `"metadata.modified_at"`. Add a registry test:

```rust
    #[test]
    fn agent_portal_timestamp_is_volatile() {
        assert!(meta(ResourceKind::Agent).volatile_fields.contains(&"metadata.modified_at"));
    }
```

Check the Agent's current volatile list location and any normalize test that would now strip it (none expected — say so in the report if one needed updating).

- [ ] **Step 3: README** — find the Semantic Diff sample block (the fenced block after "```bash\nrigg diff my-rag\n```") showing `~ Index 'docs-index' (modified) …` and replace the sample output with the new table + hint shape:

```
docs-index — differs (2 field(s))

  field                                    local            Azure (dev)
  fields[3].type                           Edm.Int32        Edm.String
  fields[7] 'rating'                       (present)        (absent)

hint: rigg pull my-rag — update local files to match Azure
      rigg push my-rag — make Azure match your local files
```

(Keep it illustrative — match the real renderer's shape from Task 1; run a quick local render if unsure and paste reality, not hope.)

- [ ] **Step 4: Full checks + commit**

```bash
cargo test --workspace && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
git add crates/rigg/src/commands/ai_assist.rs crates/rigg-core/src/registry.rs README.md
git commit -m "feat: neutral dual-direction AI diff summary; agent modified_at volatile"
```

---

## Final Verification

Full battery + live acceptance (user re-runs `rigg diff regulus` with the portal-side model change still in place): table shows `model  "gpt-5.2-chat"  "gpt-5.6-luna"` under `local | Azure (dev)` headers, hints present, AI summary attributes sides correctly and covers both directions, and `metadata.modified_at` no longer appears.

## Self-Review notes

- Spec §1 → Task 1; §2 → Task 2; §3-5 → Task 3. JSON untouched (§1) is a stated global constraint.
- Signature thread: `SideLabels` defined Task 1, consumed by diff.rs in the same commit (green-commit rule); hints purely additive in Task 2.
- Sanctioned breakage: sync.rs/output.rs shape assertions updated in Task 1 only.
