# Workstream 4a: Interaction primitives — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every rigg command one way to ask the user something that works on a terminal (prompt), in scripts (`--answer`/`--answers-file`), and for AI agents (a `needs-input` JSON document and exit code 6), so the bindings, promote and auth workstreams can build guided flows on it.

**Architecture:** A `Question`/`Answer` model and an `Asker` trait in `crates/rigg/src/commands/ask.rs`. `InteractiveAsker` prompts through the existing `inquire` wrappers but honours pre-supplied answers first; `ScriptedAsker` answers from the supplied map and otherwise returns a typed `NeedsInput` error that the top-level exit mapping renders as the protocol document with exit 6. `GlobalContext` gains the answers map and a factory. The protected-environment gate becomes the first consumer. The MCP server passes an `answers` object through as `--answer` flags and returns the `needs-input` document verbatim.

**Tech Stack:** Rust 2024 (MSRV 1.88), clap 4 (global repeatable `--answer`), inquire 0.9, serde_json, assert_cmd tests.

**Spec:** `docs/superpowers/specs/2026-09-09-interaction-model-design.md` §2 (modes), §3 (question protocol), §5 (exit codes); MCP paragraph in §3.

## Global Constraints

- Branch `rigg-2`; commit after every task with trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- Gate before every commit: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Exit codes: 0 ok · 1 error · 2 usage · 3 validation · 4 auth · 5 drift/conflict · **6 needs input** (new; the `exit_codes_are_stable` test pins it).
- Interactive mode = stdin and stdout are TTYs and none of `--yes`, `--non-interactive`, `--output json` given; `RIGG_NON_INTERACTIVE=1` forces non-interactive. `--yes` skips routine confirmations only: it never answers a question and never satisfies a protected-environment gate.
- `needs-input` document shape (stdout, pretty JSON, exit 6):
  ```json
  { "status": "needs-input", "command": "<command>", "context": { … }, "questions": [ { "id": "…", "kind": "choice|text|confirm|confirm-env", "prompt": "…", "candidates": [ { "value": "…", "label": "…" } ], "allow_other": true, "default": "…" } ] }
  ```
  `candidates`, `allow_other`, `default` are omitted when not applicable.
- Answers: `--answer <id>=<value>` (repeatable, global) and `--answers-file <path>` (JSON object id → string). Unknown ids are a usage error (exit 2). An answered question is never prompted, in either mode.
- Question ids are stable strings documented per command; the protected gate's id is `confirm.protected.<env>`.
- No api-version literals outside the registry (existing guard test).

---

### Task 1: Question model, `Asker` trait, scripted answers, exit 6

**Files:**
- Create: `crates/rigg/src/commands/ask.rs`
- Modify: `crates/rigg/src/commands/mod.rs` (module list; `ExitCode::NeedsInput = 6`; `CommandError::NeedsInput`; `exit_code_for`; `GlobalContext { answers, force_non_interactive }`, `from_cli`, `interactive()`, `asker()`), `crates/rigg/src/cli.rs:23-59` (`--answer`, `--answers-file`)
- Test: inline in `ask.rs` and `mod.rs`; `crates/rigg/tests/cli_surface.rs`

**Interfaces:**
- Produces:
  ```rust
  // ask.rs
  pub enum QuestionKind { Choice, Text, Confirm, ConfirmEnv }
  pub struct Candidate { pub value: String, pub label: String }
  pub struct Question { pub id: String, pub kind: QuestionKind, pub prompt: String, pub candidates: Vec<Candidate>, pub allow_other: bool, pub default: Option<String> }
  impl Question {
      pub fn choice(id: impl Into<String>, prompt: impl Into<String>, candidates: Vec<Candidate>) -> Self;
      pub fn text(id, prompt) -> Self; pub fn confirm(id, prompt, default_yes: bool) -> Self; pub fn confirm_env(env: &str, operation: &str) -> Self; // id = confirm.protected.<env>
      pub fn with_default(self, d: impl Into<String>) -> Self; pub fn allow_other(self) -> Self;
  }
  pub enum Answer { Choice(String), Text(String), Confirm(bool) }
  impl Answer { pub fn as_str(&self) -> Option<&str>; pub fn as_bool(&self) -> Option<bool>; }
  pub trait Asker { fn ask(&mut self, q: &Question) -> anyhow::Result<Answer>; fn ask_all(&mut self, qs: &[Question]) -> anyhow::Result<Vec<Answer>>; }
  pub struct ScriptedAsker { answers: BTreeMap<String, String>, command: String, context: serde_json::Value }
  pub struct NeedsInput { pub command: String, pub context: serde_json::Value, pub questions: Vec<Question> }  // thiserror; Display = short summary; `to_json()` = protocol document
  pub fn parse_answer_flag(s: &str) -> anyhow::Result<(String, String)>; // "id=value"
  pub fn load_answers(flags: &[String], file: Option<&Path>) -> anyhow::Result<BTreeMap<String, String>>;
  pub fn coerce(q: &Question, raw: &str) -> anyhow::Result<Answer>; // choice: must be a candidate value unless allow_other; confirm: yes/no/true/false/y/n; confirm-env: raw must equal the env name (else Err usage)
  // mod.rs
  ExitCode::NeedsInput = 6; CommandError::NeedsInput(ask::NeedsInput);
  GlobalContext { …, pub answers: BTreeMap<String, String> }
  impl GlobalContext { pub fn interactive(&self) -> bool /* also false when RIGG_NON_INTERACTIVE is set or output is json */; pub fn scripted_asker(&self, command: &str, context: serde_json::Value) -> ask::ScriptedAsker }
  ```
  `exit_code_for` prints the `needs-input` document to **stdout** (not stderr) for `CommandError::NeedsInput` and returns `ExitCode::NeedsInput`; the human-readable summary goes to stderr only when `--output` is text.

- [ ] **Step 1: Write the failing tests**

`ask.rs` tests:

```rust
#[test]
fn scripted_asker_answers_from_map_and_collects_the_rest() {
    let mut asker = ScriptedAsker::new(
        [("binding.prod.docs-storage".to_string(), "same".to_string())].into_iter().collect(),
        "promote", json!({"from": "dev", "to": "prod"}),
    );
    let q1 = Question::choice("binding.prod.docs-storage", "Which storage?", vec![Candidate { value: "same".into(), label: "same as dev".into() }]);
    let q2 = Question::text("binding.prod.enrich-fn", "Function app for prod?");
    assert_eq!(asker.ask(&q1).unwrap().as_str(), Some("same"));
    let err = asker.ask_all(&[q1.clone(), q2.clone()]).unwrap_err();
    let ni = err.downcast_ref::<NeedsInput>().expect("NeedsInput");
    assert_eq!(ni.questions.iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), vec!["binding.prod.enrich-fn"]);
    let doc = ni.to_json();
    assert_eq!(doc["status"], "needs-input");
    assert_eq!(doc["command"], "promote");
    assert_eq!(doc["questions"][0]["kind"], "text");
    assert!(doc["questions"][0].get("candidates").is_none(), "omitted when empty");
}

#[test]
fn coerce_enforces_candidates_and_confirm_env() {
    let q = Question::choice("q", "?", vec![Candidate { value: "a".into(), label: "A".into() }]);
    assert!(coerce(&q, "b").is_err());
    assert_eq!(coerce(&q.clone().allow_other(), "b").unwrap().as_str(), Some("b"));
    let c = Question::confirm("c", "?", true);
    assert_eq!(coerce(&c, "no").unwrap().as_bool(), Some(false));
    let e = Question::confirm_env("prod", "push");
    assert_eq!(e.id, "confirm.protected.prod");
    assert!(coerce(&e, "prd").is_err());
    assert_eq!(coerce(&e, "prod").unwrap().as_bool(), Some(true));
}

#[test]
fn load_answers_merges_flags_over_file_and_rejects_bad_flags() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("a.json");
    std::fs::write(&f, r#"{"x": "1", "y": "2"}"#).unwrap();
    let m = load_answers(&["y=3".to_string()], Some(&f)).unwrap();
    assert_eq!(m["x"], "1"); assert_eq!(m["y"], "3");
    assert!(parse_answer_flag("novalue").is_err());
}
```

`mod.rs` test: extend `exit_codes_are_stable` with `assert_eq!(ExitCode::NeedsInput as u8, 6);`.

`cli_surface.rs`:

```rust
#[test]
fn unknown_answer_id_is_a_usage_error() {
    let ws = workspace();
    rigg().current_dir(ws.path())
        .args(["status", "--answer", "nope=1"])
        .assert().code(2)
        .stderr(predicate::str::contains("unknown answer id 'nope'"));
}
```

(For this test, `status` must validate answers before doing anything else: put the check in `GlobalContext::from_cli` → `Cli::run`, comparing against the ids the command *declares* — see Step 3.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg ask:: exit_codes_are_stable unknown_answer_id`
Expected: compile errors / FAIL.

- [ ] **Step 3: Implement**

`ask.rs`: the types above. `ScriptedAsker::ask` looks up `q.id`; found → `coerce`; missing → `Err(NeedsInput { questions: vec![q.clone()] })`. `ask_all` collects every unanswered question into one `NeedsInput` (answered ones are coerced; if all answered returns the answers). `NeedsInput` implements `std::error::Error` (thiserror) with `Display` = `"{n} question(s) need an answer: {ids}"`, and `to_json()` builds the protocol document (serialize `Question` with `#[serde(skip_serializing_if = "Vec::is_empty")]` on `candidates`, `skip_serializing_if = "Option::is_none"` on `default`, and `allow_other` only when true — implement with a manual `to_value` rather than fighting serde attributes).

`cli.rs`: add global args

```rust
/// Answer a question a guided flow would ask (repeatable): --answer <id>=<value>
#[arg(long = "answer", global = true, value_name = "ID=VALUE")]
pub answer: Vec<String>,
/// JSON file of answers ({"<id>": "<value>", …})
#[arg(long, global = true, value_name = "PATH")]
pub answers_file: Option<std::path::PathBuf>,
```

`mod.rs`: `GlobalContext.answers` loaded via `ask::load_answers` in `from_cli` (errors become `CommandError::Usage`); `interactive()` = `!self.non_interactive && !self.yes && !self.json()`; `from_cli` sets `non_interactive` also when `RIGG_NON_INTERACTIVE` is set or stdin is not a terminal (`std::io::stdin().is_terminal()`). Unknown-id check: because commands declare questions lazily, validate ids against a **registry of known id prefixes** exported by `ask::KNOWN_ID_PREFIXES: &[&str] = &["confirm.protected."]` (later tasks and workstreams append `binding.`, `env.`, `promote.`, `auth.`, `new.`); an answer whose id matches no prefix is a usage error at startup. `exit_code_for`: match `CommandError::NeedsInput(ni)` → `println!("{}", serde_json::to_string_pretty(&ni.to_json()))`, and if the output format is text also `eprintln!("{ni}\nAnswer with --answer <id>=<value> (or --answers-file) and re-run.")`; return `ExitCode::NeedsInput`. `exit_code_for` needs the output format: change its signature to `exit_code_for(result, output: OutputFormat)` and update the caller in `cli.rs`/`main.rs`.

- [ ] **Step 4: Run the gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): question protocol — Asker trait, --answer/--answers-file, needs-input exit 6

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: `InteractiveAsker` and the protected-environment gate on the protocol

**Files:**
- Modify: `crates/rigg/src/commands/ask.rs` (`InteractiveAsker`), `crates/rigg/src/commands/mod.rs` (`GlobalContext::asker()`, `confirm_protected_env`), `crates/rigg/src/commands/interactive.rs` (no change expected; reuse)
- Test: `ask.rs` inline (scripted path of the interactive asker), `crates/rigg/tests/sync.rs` protected-env tests (`protected_env_push_blocks_non_interactive_without_confirm_env`, `protected_env_push_succeeds_with_confirm_env`, and the delete pair)

**Interfaces:**
- Produces:
  ```rust
  pub struct InteractiveAsker { answers: BTreeMap<String, String>, plain: bool }  // pre-supplied answers win; otherwise prompts via interactive::{select, text_with_default, text, confirm_default_yes/no}
  impl GlobalContext { pub fn asker(&self, command: &str, context: serde_json::Value) -> Box<dyn Asker> } // Interactive when interactive(), else Scripted
  pub fn confirm_protected_env(ctx: &GlobalContext, env: &ResolvedEnv, confirm_env: Option<&str>, operation: &str) -> Result<bool> // unchanged signature; `--confirm-env <name>` is treated as a pre-supplied answer to confirm.protected.<name>
  ```

- [ ] **Step 1: Write the failing tests**

`ask.rs`:

```rust
#[test]
fn interactive_asker_uses_presupplied_answers_without_prompting() {
    // No TTY in tests: a prompt would fail. A pre-supplied answer must short-circuit.
    let mut a = InteractiveAsker::new([("confirm.protected.prod".to_string(), "prod".to_string())].into_iter().collect(), true);
    let q = Question::confirm_env("prod", "push");
    assert_eq!(a.ask(&q).unwrap().as_bool(), Some(true));
}
```

`tests/sync.rs`: add next to the existing protected-env tests:

```rust
#[tokio::test]
async fn protected_env_push_non_interactive_emits_needs_input_with_exit_6() {
    let server = MockServer::start().await;
    mount_empty_lists_except(&server, "").await;
    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(ws.path(), "prod", "indexes", "idx", &json!({"name": "idx", "fields": []}));
    let out = rigg(ws.path()).args(["push", "demo", "-e", "prod", "--yes", "--output", "json"]).assert().code(6);
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let doc: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["status"], "needs-input");
    assert_eq!(doc["questions"][0]["id"], "confirm.protected.prod");
    assert_eq!(doc["questions"][0]["kind"], "confirm-env");
}

#[tokio::test]
async fn protected_env_push_accepts_answer_flag() {
    let server = MockServer::start().await;
    mount_empty_lists_except(&server, "").await;
    Mock::given(method("PUT")).and(path("/indexes/idx")).respond_with(ResponseTemplate::new(201).set_body_json(json!({"name": "idx", "fields": []}))).mount(&server).await;
    Mock::given(method("GET")).and(path("/indexes/idx")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"name": "idx", "fields": []}))).mount(&server).await;
    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(ws.path(), "prod", "indexes", "idx", &json!({"name": "idx", "fields": []}));
    rigg(ws.path()).args(["push", "demo", "-e", "prod", "--yes", "--answer", "confirm.protected.prod=prod"]).assert().success();
}
```

Check the existing helper names in `sync.rs` (`mount_empty_lists_except`, `workspace_with_protected_prod`, `write_resource_env`) and the existing PUT/GET mock shapes used by other push tests; mirror them. The existing test `protected_env_push_blocks_non_interactive_without_confirm_env` currently expects exit 2 — it now expects exit **6** in json mode and exit 6 in text mode too (the gate is a question); update its assertion and message check (`stderr` contains `confirm.protected.prod`). `--confirm-env prod` keeps working (it is sugar for the answer).

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg --test sync protected_env`
Expected: the new tests fail (exit 2 today; no `--answer` handling).

- [ ] **Step 3: Implement**

`InteractiveAsker::ask`: if `answers` has `q.id` → `coerce`; else by kind: `Choice` → `interactive::select(prompt, labels, plain)` mapping label → value (when `allow_other`, append an "enter another value" row that falls through to `interactive::text`); `Text` → `text_with_default`/`text`; `Confirm` → `confirm_default_yes/no` per `default`; `ConfirmEnv` → `interactive::text` and compare with the env name (mismatch → `Ok(Answer::Confirm(false))`). `ask_all` = sequential `ask`.

`confirm_protected_env`: unchanged when unprotected; otherwise build `Question::confirm_env(&env.name, operation)`, merge `confirm_env` (if `Some(name)`) into the asker's answers as `confirm.protected.<name>=<name>`, and `asker.ask(&q)`; `Ok(false)` on a declined interactive answer; in scripted mode the `NeedsInput` error propagates (exit 6). Remove the old `CommandError::Usage` path for the non-interactive case. `ctx.yes` still never satisfies the gate (the question is asked regardless of `--yes`; `interactive()` is false with `--yes`, so the scripted asker runs and needs the answer).

- [ ] **Step 4: Run the gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: green; the four existing protected-env tests updated and passing.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cli): interactive asker; protected-environment gate speaks the question protocol

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: MCP `answers` pass-through and documentation

**Files:**
- Modify: `crates/rigg/src/mcp/tools.rs` (`answers: Option<BTreeMap<String,String>>` on `ProjectParams`, `DiffParams`, and the push/pull/delete param structs; `rigg_cli` maps exit 6 to the raw stdout document; `with_common` appends `--answer id=value` pairs), `CONCEPTS.md` (exit-code table + a short "When rigg needs an answer" paragraph), `README.md` (exit codes list; `--answer`/`--answers-file` in the CI/automation section), `crates/rigg/CONCEPTS.md` (copy; the guard test enforces identical content), `.claude/skills/rigg-guide/SKILL.md` (exit code 6, `--answer`), `CHANGELOG.md`
- Test: `crates/rigg/src/mcp/tools.rs` inline test for `with_common` answers; `cli_surface.rs` help test that `--answer` appears in `rigg push --help`

- [ ] **Step 1: Write the failing tests**

```rust
// tools.rs
#[test]
fn with_common_appends_answer_flags_in_sorted_order() {
    let answers: std::collections::BTreeMap<String, String> = [("b".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())].into_iter().collect();
    let args = with_common_answers(vec!["push"], &None, true, Some(&answers));
    assert!(args.windows(2).any(|w| w == ["--answer", "a=1"]));
    assert!(args.windows(2).any(|w| w == ["--answer", "b=2"]));
}
```

```rust
// cli_surface.rs
#[test]
fn answer_flags_are_global() {
    rigg().args(["push", "--help"]).assert().success()
        .stdout(predicate::str::contains("--answer <ID=VALUE>"))
        .stdout(predicate::str::contains("--answers-file"));
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p rigg with_common_appends answer_flags_are_global`
Expected: FAIL.

- [ ] **Step 3: Implement**

`tools.rs`: `with_common_answers(args, env, json, answers: Option<&BTreeMap<String,String>>) -> Vec<String>` (owned strings since answers are formatted); keep `with_common` as a thin wrapper for tools without answers. Add `#[schemars(default)] pub answers: Option<BTreeMap<String, String>>` with doc "Answers to questions a previous call returned as `needs-input` (id → value)" to `ProjectParams` and the mutating tools' params. In `rigg_cli`, map exit code `6` to `stdout` unchanged (the JSON document is the tool result) — add a line in the module doc and in the server instructions string (grep `ServerInfo`/instructions in `mcp/mod.rs` or `tools.rs`) explaining the needs-input loop.

Docs: CONCEPTS "Exit codes" gains `6 — needs input`; new paragraph:

> **When rigg needs an answer.** Guided flows ask questions. On a terminal rigg prompts. In scripts and from AI agents rigg cannot prompt, so it prints a `needs-input` JSON document listing the questions (id, prompt, candidates) and exits 6. Re-run with `--answer <id>=<value>` (repeatable) or `--answers-file <path>`; answered questions are never asked again. A protected environment's typed confirmation is such a question (`confirm.protected.<env>`); `--confirm-env <env>` remains as shorthand.

Copy to `crates/rigg/CONCEPTS.md`. README: same in short. CHANGELOG `[Unreleased] — 2.0.0` → `### Added` bullet for the protocol and exit 6.

- [ ] **Step 4: Run the gate**

Run: `cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: green.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(mcp,docs): answers pass-through for MCP tools; document the question protocol and exit 6

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```
