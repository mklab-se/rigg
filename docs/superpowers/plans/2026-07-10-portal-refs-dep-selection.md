# Portal Refs + Dependency Selection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `--with-deps` sees portal-authored cross-service references (KB MCP URLs, connection ids), works on already-owned resources (re-adoption of missing deps), and the wizard lets the user pick individual dependencies and select managed resources as seeds.

**Architecture:** rigg-core's `extract_references` gains an Agent-only pass for portal reference shapes; adopt.rs seeds `expand_deps` from explicitly-named owned resources; the wizard menu includes target-project-owned entries marked `(managed)` and replaces the deps yes/no with a pre-checked multi-select. Deployment volatile fields extended.

**Tech Stack:** Rust; existing rigg-core registry, adopt.rs, interactive.rs (inquire).

## Global Constraints

- Non-interactive behavior stays deterministic: `--with-deps` is all-or-nothing; all pinned exit codes/messages unchanged.
- Dependency additions remain: unmanaged-only, platform-managed-excluded, snapshot-bounded, cycle-safe.
- Resources owned by OTHER projects: never seeds, never menu entries, never adopted (unchanged hard-error/silent-skip).
- Naming an owned resource explicitly stays a no-op for the resource itself ("already managed by this project" skip message unchanged).
- Every task leaves `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green.

---

### Task 1: rigg-core — portal reference extraction + deployment volatile fields

**Files:**
- Modify: `crates/rigg-core/src/registry.rs`

**Interfaces:**
- `extract_references(ResourceKind::Agent, doc)` additionally returns `(KnowledgeBase, name)` for KB MCP `server_url`s and `(Connection, id)` for `project_connection_id`s.
- Deployment `volatile_fields` grows `"properties.currentCapacity"`, `"properties.deploymentState"`.

- [ ] **Step 1: Write failing unit tests** in registry.rs's `mod tests`:

```rust
    #[test]
    fn agent_extracts_portal_kb_url_and_connection_id() {
        let agent = serde_json::json!({
            "name": "Regulus",
            "model": "gpt-5.2-chat",
            "tools": [{
                "type": "mcp",
                "server_label": "kb_regulatory_kb",
                "server_url": "https://mklabsrch.search.windows.net/knowledgebases/regulatory-kb/mcp?api-version=2025-11-01-Preview",
                "project_connection_id": "kb-regulatory-kb-9kdyn"
            }]
        });
        let refs = extract_references(ResourceKind::Agent, &agent);
        assert!(refs.contains(&(ResourceKind::KnowledgeBase, "regulatory-kb".to_string())), "{refs:?}");
        assert!(refs.contains(&(ResourceKind::Connection, "kb-regulatory-kb-9kdyn".to_string())), "{refs:?}");
        assert!(refs.contains(&(ResourceKind::Deployment, "gpt-5.2-chat".to_string())), "{refs:?}");
    }

    #[test]
    fn agent_ignores_non_search_mcp_urls() {
        let agent = serde_json::json!({
            "name": "a",
            "tools": [{"type": "mcp", "server_url": "https://example.com/knowledgebases/x/mcp"}]
        });
        let refs = extract_references(ResourceKind::Agent, &agent);
        assert!(!refs.iter().any(|(k, _)| *k == ResourceKind::KnowledgeBase), "{refs:?}");
    }

    #[test]
    fn deployment_runtime_state_is_volatile() {
        let vf = meta(ResourceKind::Deployment).volatile_fields;
        assert!(vf.contains(&"properties.currentCapacity"));
        assert!(vf.contains(&"properties.deploymentState"));
    }
```

NOTE: verify the Agent kind's existing RefField for `model` (so the third assert matches reality — check the Agent KindMeta; if the model reference path differs, adjust). Verify variant names (`KnowledgeBase`, `Connection`) in resources/traits.rs.

- [ ] **Step 2: Run to confirm failures**

Run: `cargo test -p rigg-core agent_extracts 2>&1 | tail -6` (and the other two)
Expected: FAIL.

- [ ] **Step 3: Implement.** In registry.rs:

a) Deployment `volatile_fields`: append `"properties.currentCapacity", "properties.deploymentState",` after `"properties.model.callRateLimit",`.

b) Portal reference pass. First CHECK whether `collect_path` traverses arrays (read its implementation, ~line 432): if `collect_path(body, "tools.project_connection_id")` visits each array element, implement the connection id as a RefField row on the Agent KindMeta:

```rust
        RefField {
            path: "tools.project_connection_id",
            to: ResourceKind::Connection,
        },
```

If collect_path does NOT traverse arrays, extract it in the custom pass below instead.

c) KB URL pass — add to `extract_references` after `collect_x_rigg_refs(body, &mut out);`:

```rust
    if kind == ResourceKind::Agent {
        collect_portal_agent_refs(body, &mut out);
    }
```

and:

```rust
/// Portal-authored agents reference Search knowledge bases by raw MCP URL
/// (`https://<svc>.search.windows.net/knowledgebases/<name>/mcp?...`) rather
/// than an `x-rigg-ref` annotation. Recognize the shape so dependency
/// expansion can cross the service boundary.
fn collect_portal_agent_refs(v: &Value, out: &mut Vec<(ResourceKind, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(url) = map.get("server_url").and_then(Value::as_str) {
                if let Some(kb) = parse_kb_mcp_url(url) {
                    out.push((ResourceKind::KnowledgeBase, kb));
                }
            }
            for (_, val) in map {
                collect_portal_agent_refs(val, out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_portal_agent_refs(item, out);
            }
        }
        _ => {}
    }
}

/// `https://<host>.search.windows.net/knowledgebases/<name>/mcp[?...]` → name.
fn parse_kb_mcp_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if !host.to_ascii_lowercase().ends_with(".search.windows.net") {
        return None;
    }
    let path = path.split('?').next().unwrap_or(path);
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let (a, name, c) = (segs.next()?, segs.next()?, segs.next()?);
    (a.eq_ignore_ascii_case("knowledgebases") && c.eq_ignore_ascii_case("mcp"))
        .then(|| name.to_string())
}
```

- [ ] **Step 4: Tests pass + full checks**

Run: `cargo test -p rigg-core 2>&1 | tail -4 && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2 && cargo test --workspace 2>&1 | grep -c 'test result: ok'`
Expected: all green (the volatile-field change may affect normalize tests — if a fixture asserts deployment normalization, update expectations only if the test was asserting the OLD behavior).

- [ ] **Step 5: Commit**

```bash
git add crates/rigg-core/src/registry.rs
git commit -m "feat: extract portal-authored agent refs (KB MCP URL, connection id); deployment runtime fields volatile"
```

---

### Task 2: adopt.rs — expansion seeds from explicitly-named owned resources

**Files:**
- Modify: `crates/rigg/src/commands/adopt.rs`
- Test: `crates/rigg/tests/sync.rs`

**Interfaces:**
- `expand_deps(seeds: &[(ResourceRef, Value)], owned_by_any, snap_map) -> (Vec<(ResourceRef, Value)>, BTreeSet<String>)` — UNCHANGED signature, but callers now pass `to_adopt ∪ owned_seeds` as the walk roots. (Verify the current parameter name; the walk must start from all roots but only ever ADD unmanaged resources.)

- [ ] **Step 1: Write failing sync test** (wiremock, Search side — an owned indexer whose deps are unmanaged):

```rust
#[tokio::test]
async fn adopt_with_deps_on_owned_resource_adopts_missing_deps() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/indexers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "docs-indexer",
                "dataSourceName": "docs-ds",
                "targetIndexName": "docs-index"
            }]
        }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs-index", "fields": [{"name":"id","type":"Edm.String","key":true}]}]
        }))).mount(&server).await;
    Mock::given(method("GET")).and(path("/datasources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs-ds", "type": "azureblob",
                       "container": {"name": "c"},
                       "credentials": {"connectionString": "ResourceId=/x;"}}]
        }))).mount(&server).await;
    for p in ["skillsets","synonymmaps","aliases","knowledgeSources","knowledgeBases"] {
        Mock::given(method("GET")).and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server).await;
    }

    let ws = workspace(&server.uri());
    // First: adopt ONLY the indexer (no deps).
    rigg(ws.path()).args(["adopt", "demo", "indexers/docs-indexer"]).assert().success();
    let base = ws.path().join("projects/demo/search");
    assert!(base.join("indexers/docs-indexer.json").exists());
    assert!(!base.join("indexes/docs-index.json").exists(), "deps not adopted yet");

    // Change of mind: same command with --with-deps must now adopt the missing deps.
    rigg(ws.path())
        .args(["adopt", "demo", "indexers/docs-indexer", "--with-deps"])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexes/docs-index"))
        .stdout(predicate::str::contains("already managed"));
    assert!(base.join("indexes/docs-index.json").exists(), "index adopted as dep of owned seed");
    assert!(base.join("data-sources/docs-ds.json").exists(), "data source adopted as dep of owned seed");
}
```

- [ ] **Step 2: Run to confirm the second half fails**

Run: `cargo test -p rigg --test sync adopt_with_deps_on_owned 2>&1 | tail -10`
Expected: FAIL — today the owned seed is dropped and no deps are found.

- [ ] **Step 3: Implement seeding.** In the classification loop, the arm for `Some(owner) if owner == &project.name` currently only pushes the skip message for explicit selectors. Additionally collect the doc as a seed:

Before the loop: `let mut owned_seeds: Vec<(ResourceRef, Value)> = Vec::new();`

In that arm (explicit or not — but only explicit matters, since sweeps skip owned silently; collect ONLY for explicit selectors to keep sweep semantics unchanged):

```rust
                Some(owner) if owner == &project.name => {
                    if explicit.contains(key) {
                        skipped.push((key.clone(), "already managed by this project".to_string()));
                        owned_seeds.push((r.clone(), doc.clone()));
                    }
                }
```

Then wherever `expand_deps(&to_adopt, ...)` is called (both the `--with-deps` path and the wizard ask-path), build the roots as:

```rust
        let mut roots: Vec<(ResourceRef, Value)> = to_adopt.clone();
        roots.extend(owned_seeds.iter().cloned());
        let (adds, keys) = expand_deps(&roots, &owned_by_any, &snap_map);
```

(Verify how `expand_deps` uses its first argument: it seeds `in_set` from the roots' keys AND walks from them; since owned seeds are in `owned_by_any`, they can never be re-added as additions — confirm by reading the function. The additions extend `to_adopt` exactly as today.)

- [ ] **Step 4: Tests pass + full checks**

Run: `cargo test -p rigg --test sync adopt 2>&1 | tail -8 && cargo test --workspace 2>&1 | grep -c 'test result: ok' && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2`
Expected: all green, all pre-existing adopt tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/rigg/src/commands/adopt.rs crates/rigg/tests/sync.rs
git commit -m "feat: adopt --with-deps seeds from explicitly-named owned resources"
```

---

### Task 3: wizard — managed entries + per-dependency multi-select

**Files:**
- Modify: `crates/rigg/src/commands/adopt.rs`
- Modify: `crates/rigg/src/commands/interactive.rs` (multi_select gains pre-checked defaults)
- Modify: `crates/rigg/src/cli.rs` (adopt long help: re-adoption + managed entries)
- Test: unit tests in adopt.rs; `crates/rigg/tests/cli_surface.rs` (help text)

**Interfaces:**
- `wizard_candidates(snapshot, owned_by_any, target_project: &str) -> Vec<(ResourceRef, String)>` — NEW param; includes target-project-owned entries with label suffix `" (managed)"`, still excludes other-project-owned and platform-managed.
- `interactive::multi_select_checked(prompt, options: Vec<String>, checked: bool, plain: bool) -> Result<Vec<usize>>` — like multi_select but with all items pre-checked when `checked` (inquire: `.with_all_selected_by_default()` or `.with_default(&indices)` — verify the 0.7 API and use what exists).

- [ ] **Step 1: Write failing unit test** for the candidates change (adopt.rs `mod tests`):

```rust
    #[test]
    fn wizard_candidates_marks_target_project_owned_as_managed() {
        let mut owned = BTreeMap::new();
        owned.insert("agents/regulus".to_string(), "regulus".to_string()); // target project
        owned.insert("agents/helper".to_string(), "other".to_string());    // other project
        let snap = vec![
            (ResourceRef::new(ResourceKind::Agent, "regulus".to_string()), serde_json::json!({"name": "regulus"})),
            (ResourceRef::new(ResourceKind::Agent, "helper".to_string()), serde_json::json!({"name": "helper"})),
            (ResourceRef::new(ResourceKind::Agent, "newbie".to_string()), serde_json::json!({"name": "newbie"})),
        ];
        let items = wizard_candidates(&snap, &owned, "regulus");
        let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
        assert!(labels.contains(&"[Foundry] agents/regulus (managed)"), "{labels:?}");
        assert!(labels.contains(&"[Foundry] agents/newbie"), "{labels:?}");
        assert!(!labels.iter().any(|l| l.contains("helper")), "other-project resources hidden: {labels:?}");
    }
```

Update the two existing `wizard_candidates` tests for the new parameter (pass a project name that owns nothing, e.g. `"demo"`).

- [ ] **Step 2: Run to confirm failure** (`cargo test -p rigg wizard_candidates 2>&1 | tail -6`).

- [ ] **Step 3: Implement.**

a) `wizard_candidates`: new `target_project: &str` param. Filter becomes: skip platform-managed; skip owned-by-OTHER (`owned_by_any.get(key)` is Some and != target_project); include unmanaged (plain label) and owned-by-target (label + `" (managed)"`).

b) Wizard step 2: pass `&project.name`; picked managed entries flow into `selectors` as `Selector::One` exactly like the rest (Task 2's seeding then handles them: skip + seed). Do NOT add managed picks to `wizard_chosen`… actually DO add them — the hint should reproduce the invocation (`rigg adopt regulus agents/Regulus --with-deps` re-runs idempotently by design). Add them.

c) Dependency step: replace the yes/no with a pre-checked multi-select. Current shape (wizard branch):

```rust
        if !adds.is_empty() {
            ...println list + confirm_default_no(question)...
        }
```

becomes:

```rust
        if !adds.is_empty() {
            let labels: Vec<String> = adds.iter().map(|(r, _)| r.to_string()).collect();
            let picked = interactive::multi_select_checked(
                "Upstream dependencies found — adopt these too? (all selected; space to drop)",
                labels,
                true,
                plain,
            )?;
            if !picked.is_empty() {
                with_deps = true;
                for i in &picked {
                    let (r, doc) = &adds[*i];
                    dep_keys.insert(r.key());
                    to_adopt.push((r.clone(), doc.clone()));
                }
            }
        }
```

(`keys` from expand_deps is no longer used wholesale in the wizard path — only the picked subset. The non-wizard `--with-deps` path keeps using the full `adds`/`keys` as today.)

d) `interactive.rs`:

```rust
/// Multi-select with every option pre-checked when `checked` is true.
pub fn multi_select_checked(
    prompt: &str,
    options: Vec<String>,
    checked: bool,
    plain: bool,
) -> Result<Vec<usize>> {
    let indexed: Vec<String> = options;
    let mut ms = MultiSelect::new(prompt, indexed.clone()).with_render_config(config(plain));
    let all: Vec<usize> = (0..indexed.len()).collect();
    if checked {
        ms = ms.with_default(&all);
    }
    let chosen = ms.prompt().map_err(map_err)?;
    Ok(chosen
        .into_iter()
        .filter_map(|c| indexed.iter().position(|o| *o == c))
        .collect())
}
```

(Verify `with_default(&[usize])` exists in inquire 0.7 MultiSelect — check the vendored source; adapt if the API takes a different form.)

e) `cli.rs` Adopt long help — extend the doc comment:

```rust
    /// Adopt selected unmanaged Azure resources into a project
    ///
    /// Selectors: `all`, a kind (e.g. `indexes`), or `<kind>/<name>`
    /// (e.g. `agents/regulus`). Naming a resource the project already manages
    /// together with --with-deps adopts its missing dependencies — useful
    /// after new references appear (e.g. added via the portal).
    /// See `rigg concepts` for the project model.
    Adopt(AdoptArgs),
```

- [ ] **Step 4: cli_surface test** for the help addition:

```rust
#[test]
fn adopt_help_documents_readoption() {
    rigg().args(["adopt", "--help"]).assert().success()
        .stdout(predicate::str::contains("missing dependencies"));
}
```

- [ ] **Step 5: All tests + checks**

Run: `cargo test -p rigg 2>&1 | tail -8 && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -2 && cargo test --workspace 2>&1 | grep -c 'test result: ok'`
Expected: green.

- [ ] **Step 6: Commit**

```bash
git add crates/rigg/src/commands/adopt.rs crates/rigg/src/commands/interactive.rs \
        crates/rigg/src/cli.rs crates/rigg/tests/cli_surface.rs
git commit -m "feat: wizard shows managed resources and per-dependency selection"
```

---

### Task 4: docs

**Files:**
- Modify: `README.md`, `GETTING_STARTED.md`

- [ ] **Step 1: README** — in the Quick Start adopt block, after the `--with-deps` example line, add:

```markdown
# Later: capture newly-added dependencies of something you already manage
rigg adopt my-rag agents/my-agent --with-deps
```

- [ ] **Step 2: GETTING_STARTED** — extend the "Existing resources?" bullet's final sentence with: `; re-run with an already-managed resource to capture dependencies added later (e.g. via the portal)`.

- [ ] **Step 3: Verify + commit**

Run: `grep -n "capture" README.md GETTING_STARTED.md`
Expected: both hits present.

```bash
git add README.md GETTING_STARTED.md
git commit -m "docs: document re-adoption of dependencies via adopt --with-deps"
```

---

## Final Verification

`cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` — then live acceptance: wizard re-run selecting `agents/Regulus (managed)`, expecting the regulatory stack (KB, KS, index, connection — indexer/data-source/skillset only if reachable from those roots) as pre-checked dependencies.

## Self-Review notes

- Spec §1 → Task 1; §2 → Task 2; §3 → Task 3; §4 → Task 1; §5 → Tasks 3-4.
- Signature threads: `wizard_candidates(..., target_project)` updated with both existing tests; `multi_select_checked` consumed in Task 3c; `expand_deps` roots built at both call sites.
- Known semantic nuance (intentional): from the agent, the dependency closure reaches KB→KS→index and the connection, but NOT the indexer/data-source/skillset (they reference the index, not vice versa) — capturing those still requires naming the indexer. Documented reality; not silently glossed.
