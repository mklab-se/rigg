# Auto-Created Exclusion + Managed-Dep Visibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Auto-created KS sub-resources become invisible to adoption/unmanaged reporting (with reasoned skip when explicitly named); the wizard's dependency step shows already-managed dependencies.

**Architecture:** New `registry::auto_created_by(snapshot)` map (key → creating-KS name), applied at the same five choke points as `is_platform_managed`. `expand_deps` gains a third output: owned references it encountered, printed by the wizard.

**Tech Stack:** Rust; existing registry/adopt/status/pull; wiremock tests.

## Global Constraints

- Exclusion applies to adoption candidacy + unmanaged reporting ONLY — owned files, pull/push/diff of owned resources untouched.
- Explicit naming → reasoned skip: `auto-created by knowledge source '<ks>' — manage it via the knowledge source`; sweeps silent. (Mirror of ownership/platform-managed handling.)
- Non-wizard output unchanged. All pre-existing tests pass unmodified.
- Every task leaves fmt/clippy(-D warnings)/`cargo test --workspace` green.

---

### Task 1: rigg-core — `auto_created_by`

**Files:**
- Modify: `crates/rigg-core/src/registry.rs`

**Interfaces:**
- Produces: `pub fn auto_created_by(snapshot: &[(ResourceRef, Value)]) -> BTreeMap<String, String>` — key = `"<kind-dir>/<name>"` of an auto-created sub-resource, value = the creating knowledge source's name. (Import `std::collections::BTreeMap` and `crate::resources::ResourceRef` — check what registry.rs already imports.)

- [ ] **Step 1: Failing tests** (registry.rs `mod tests`):

```rust
    #[test]
    fn auto_created_by_finds_nested_created_resources() {
        // Live shape: createdResources nests under azureBlobParameters.
        let ks = serde_json::json!({
            "name": "regulatory",
            "kind": "azureBlob",
            "azureBlobParameters": {
                "containerName": "regulatory",
                "createdResources": {
                    "datasource": "regulatory-datasource",
                    "indexer": "regulatory-indexer",
                    "skillset": "regulatory-skillset",
                    "index": "regulatory-index",
                    "somethingFuture": "ignored-name"
                }
            }
        });
        let index_doc = serde_json::json!({"name": "regulatory-index"});
        let snapshot = vec![
            (ResourceRef::new(ResourceKind::KnowledgeSource, "regulatory".to_string()), ks),
            (ResourceRef::new(ResourceKind::Index, "regulatory-index".to_string()), index_doc),
        ];
        let map = auto_created_by(&snapshot);
        assert_eq!(map.get("indexes/regulatory-index").map(String::as_str), Some("regulatory"));
        assert_eq!(map.get("indexers/regulatory-indexer").map(String::as_str), Some("regulatory"));
        assert_eq!(map.get("data-sources/regulatory-datasource").map(String::as_str), Some("regulatory"));
        assert_eq!(map.get("skillsets/regulatory-skillset").map(String::as_str), Some("regulatory"));
        assert!(!map.values().any(|v| v == "ignored-name"), "unknown member names ignored: {map:?}");
        assert_eq!(map.len(), 4);
    }

    #[test]
    fn auto_created_by_ignores_non_knowledge_source_docs() {
        let idx = serde_json::json!({
            "name": "i",
            "createdResources": {"index": "x"}
        });
        let snapshot = vec![(ResourceRef::new(ResourceKind::Index, "i".to_string()), idx)];
        assert!(auto_created_by(&snapshot).is_empty());
    }
```

- [ ] **Step 2: Confirm RED** — `cargo test -p rigg-core auto_created 2>&1 | tail -6`.

- [ ] **Step 3: Implement** (registry.rs, near `is_platform_managed`):

```rust
/// Managed-ingestion knowledge sources auto-create their backing pipeline
/// (index, indexer, data source, skillset); Azure names them in the KS's
/// `createdResources`. Rigg never manages these sub-resources — the knowledge
/// source definition is their source of truth — so they are excluded from
/// adoption and unmanaged reporting. Returns resource key → creating KS name.
pub fn auto_created_by(
    snapshot: &[(ResourceRef, Value)],
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (r, doc) in snapshot {
        if r.kind != ResourceKind::KnowledgeSource {
            continue;
        }
        collect_created_resources(doc, &r.name, &mut out);
    }
    out
}

fn collect_created_resources(
    v: &Value,
    ks_name: &str,
    out: &mut std::collections::BTreeMap<String, String>,
) {
    if let Value::Object(map) = v {
        if let Some(Value::Object(created)) = map.get("createdResources") {
            for (member, name) in created {
                let kind = match member.as_str() {
                    "datasource" => Some(ResourceKind::DataSource),
                    "indexer" => Some(ResourceKind::Indexer),
                    "skillset" => Some(ResourceKind::Skillset),
                    "index" => Some(ResourceKind::Index),
                    _ => None, // future member names: ignore
                };
                if let (Some(kind), Some(name)) = (kind, name.as_str()) {
                    out.insert(
                        ResourceRef::new(kind, name.to_string()).key(),
                        ks_name.to_string(),
                    );
                }
            }
        }
        for (_, val) in map {
            collect_created_resources(val, ks_name, out);
        }
    } else if let Value::Array(arr) = v {
        for item in arr {
            collect_created_resources(item, ks_name, out);
        }
    }
}
```

NOTE: verify registry.rs's existing imports for `ResourceRef` (it may only import via `crate::resources::…` paths — adapt). Verify variant names (`DataSource`, `Indexer`, `Skillset`, `Index`, `KnowledgeSource`).

- [ ] **Step 4: GREEN + full checks** — `cargo test -p rigg-core 2>&1 | tail -4 && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-core/src/registry.rs
git commit -m "feat: auto_created_by maps KS-generated sub-resources to their knowledge source"
```

---

### Task 2: wire exclusion into adopt/status/pull

**Files:**
- Modify: `crates/rigg/src/commands/adopt.rs`, `status.rs`, `pull.rs`
- Test: `crates/rigg/tests/sync.rs`

**Interfaces:**
- `wizard_candidates(snapshot, owned_by_any, auto_created: &BTreeMap<String, String>, target_project)` — new param, entries in the map are excluded. Update the three existing unit tests (pass `&BTreeMap::new()`).
- `expand_deps` gains the same `auto_created` param; entries never added.
- adopt/status/pull each compute `let auto_created = registry::auto_created_by(&snapshot);` right after taking the snapshot.

- [ ] **Step 1: Failing sync test** (`crates/rigg/tests/sync.rs`):

```rust
#[tokio::test]
async fn auto_created_subresources_are_not_adoptable() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/knowledgeSources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "regulatory",
                "kind": "azureBlob",
                "azureBlobParameters": {
                    "containerName": "c",
                    "createdResources": {"index": "regulatory-index"}
                }
            }]
        }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "regulatory-index", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "normal-index", "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        }))).mount(&server).await;
    for p in ["datasources","skillsets","indexers","synonymmaps","aliases","knowledgeBases"] {
        Mock::given(method("GET")).and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server).await;
    }
    let ws = workspace(&server.uri());

    // kind sweep: auto-created index silently skipped, normal index adopted
    rigg(ws.path()).args(["adopt", "demo", "indexes", "--yes"]).assert().success();
    let base = ws.path().join("projects/demo/search");
    assert!(base.join("indexes/normal-index.json").exists());
    assert!(!base.join("indexes/regulatory-index.json").exists(), "auto-created not swept");

    // explicit: reasoned skip naming the KS
    rigg(ws.path())
        .args(["adopt", "demo", "indexes/regulatory-index"])
        .assert()
        .success()
        .stdout(predicate::str::contains("auto-created by knowledge source 'regulatory'"));
    assert!(!base.join("indexes/regulatory-index.json").exists());

    // status: not listed as unmanaged (knowledge source itself + nothing else)
    rigg(ws.path()).args(["status"]).assert().success()
        .stdout(predicate::str::contains("regulatory-index").not());
}
```

NOTE: the status assertion requires the adopted `normal-index` and un-adopted KS; the KS ("knowledge-sources/regulatory") WILL be listed unmanaged — assert only that `regulatory-index` is absent. If the KS name collides in output ("regulatory" appears within "regulatory-index"), the `.not()` guard on the full string `regulatory-index` stays safe.

- [ ] **Step 2: Confirm RED** — `cargo test -p rigg --test sync auto_created 2>&1 | tail -8`.

- [ ] **Step 3: Implement.**

a) adopt.rs: after `let supported = remote.supported_kinds();` add
`let auto_created = registry::auto_created_by(&snapshot);`

b) Classification loop, in the unowned arm (where platform-managed is checked) — add the auto-created branch BEFORE the platform check or after (order irrelevant, both skip):

```rust
                None => {
                    if let Some(ks) = auto_created.get(key) {
                        if explicit.contains(key) {
                            skipped.push((
                                key.clone(),
                                format!("auto-created by knowledge source '{ks}' — manage it via the knowledge source"),
                            ));
                        }
                    } else if registry::is_platform_managed(r.kind, doc) {
                        ... existing ...
                    } else {
                        to_adopt.push((r.clone(), doc.clone()));
                    }
                }
```

c) `wizard_candidates` + `expand_deps`: new `auto_created: &BTreeMap<String, String>` param; filter `!auto_created.contains_key(&key)` alongside the platform-managed check. Update call sites and the three existing unit tests (pass `&BTreeMap::new()`); the Task-3 tests (from Workstream D) keep passing.

d) status.rs: after its `let snapshot = remote.snapshot().await?;` compute the map once per project and add `&& !auto_created.contains_key(&r.key())` to the unmanaged push. Import `rigg_core::registry` if absent.

e) pull.rs: same for the `unmanaged += 1` branch (compute the map after `let snapshot = remote.snapshot().await?;` in `pull_project`).

- [ ] **Step 4: GREEN + all checks** — `cargo test -p rigg --test sync auto_created 2>&1 | tail -6 && cargo test --workspace 2>&1 | grep -c 'test result: ok' && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg/src/commands/adopt.rs crates/rigg/src/commands/status.rs \
        crates/rigg/src/commands/pull.rs crates/rigg/tests/sync.rs
git commit -m "feat: exclude KS auto-created sub-resources from adoption and unmanaged reporting"
```

---

### Task 3: managed-dependency visibility in the wizard

**Files:**
- Modify: `crates/rigg/src/commands/adopt.rs`
- Modify: `CONCEPTS.md`

**Interfaces:**
- `expand_deps(...) -> (Vec<(ResourceRef, Value)>, BTreeSet<String>, Vec<(String, String)>)` — third element: `(key, owner)` for references encountered but skipped because owned. Both existing call sites updated (non-wizard ignores it with `_`); dedup within.

- [ ] **Step 1: Failing unit test** (adopt.rs `mod tests`) — build a snapshot where an indexer references an index owned by the target project and a data source that's unmanaged:

```rust
    #[test]
    fn expand_deps_reports_owned_references() {
        let indexer = serde_json::json!({
            "name": "ix", "dataSourceName": "ds", "targetIndexName": "idx"
        });
        let ds = serde_json::json!({"name": "ds", "type": "azureblob"});
        let idx = serde_json::json!({"name": "idx"});
        let roots = vec![(ResourceRef::new(ResourceKind::Indexer, "ix".to_string()), indexer)];
        let mut owned = BTreeMap::new();
        owned.insert("indexes/idx".to_string(), "demo".to_string());
        let snap_map: BTreeMap<String, (ResourceRef, serde_json::Value)> = [
            (ResourceRef::new(ResourceKind::DataSource, "ds".to_string()), ds),
            (ResourceRef::new(ResourceKind::Index, "idx".to_string()), idx),
        ].into_iter().map(|(r, v)| (r.key(), (r, v))).collect();
        let (adds, _keys, owned_refs) = expand_deps(&roots, &owned, &snap_map, &BTreeMap::new());
        assert_eq!(adds.len(), 1, "only the unmanaged data source is added");
        assert!(owned_refs.contains(&("indexes/idx".to_string(), "demo".to_string())), "{owned_refs:?}");
    }
```

(Adapt the `expand_deps` argument list to the real post-Task-2 signature.)

- [ ] **Step 2: Confirm RED**, implement:

a) In `expand_deps`, where a reference is skipped because `owned_by_any.contains_key(&key)`, record `(key, owner)` into a deduped `Vec<(String, String)>` (skip auto-created/platform-managed refs — they're filtered before the owned check or simply not recorded). Return it third.

b) Wizard deps step — before the multi-select (and covering the empty-adds case):

```rust
        let managed_line = |owned_refs: &[(String, String)]| {
            let mine: Vec<&str> = owned_refs.iter()
                .filter(|(_, o)| *o == project.name)
                .map(|(k, _)| k.as_str()).collect();
            let theirs: Vec<String> = owned_refs.iter()
                .filter(|(_, o)| *o != project.name)
                .map(|(k, o)| format!("{k} (managed by '{o}')")).collect();
            (mine, theirs)
        };
```

Then:
- if `adds` non-empty: print `Already managed: <mine + theirs joined>` (only when non-empty) BEFORE the multi-select prompt;
- if `adds` empty AND owned refs non-empty: print `All dependencies of your selection are already managed: <list>`.
(Exact structure per the sketch; a plain inline implementation without the closure is fine — match the file's style.)

c) Non-wizard call site: destructure with `_owned_refs` (or `..`), no behavior change.

- [ ] **Step 3: CONCEPTS.md** — extend the platform-managed paragraph:

Replace its last sentence `Your resources reference them by name instead.` with:

```markdown
Your resources reference them by name instead. The same applies to
sub-resources that Azure creates automatically (for example the index and
indexer behind a managed-ingestion knowledge source) — manage the knowledge
source; Azure manages what it generates.
```

- [ ] **Step 4: GREEN + all checks** — `cargo test -p rigg 2>&1 | tail -6 && cargo test --workspace 2>&1 | grep -c 'test result: ok' && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`. (CONCEPTS.md parity tests unaffected — the invariant sentence is untouched.)

- [ ] **Step 5: Commit**

```bash
git add crates/rigg/src/commands/adopt.rs CONCEPTS.md
git commit -m "feat: wizard shows already-managed dependencies; document auto-created exclusion"
```

---

## Final Verification

`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`, then live: wizard re-run — unmanaged drops by 4 (regulatory machinery gone), re-selecting `agents/Regulus (managed)` prints the all-managed reassurance.

## Self-Review notes

- Spec §1 → Tasks 1-2; §2 → Task 3; §3 → Task 3 Step 3. Choke-point parity with platform-managed: classification/wizard/deps (Task 2 a-c), status (d), pull (e).
- Signature threads: `auto_created_by` (Task 1) consumed in Task 2; `expand_deps` third return (Task 3) with both call sites updated; `wizard_candidates` param updated with its three existing tests.
- Deliberate scope cut: owned files for auto-created resources are not retroactively rejected (matches guardrail stance).
