//! CLI surface tests: command shape, exit codes, workspace-local behavior.
//!
//! These run the real binary against temp workspaces — no network.

use assert_cmd::Command;
use predicates::prelude::*;

fn rigg() -> Command {
    let mut cmd = Command::cargo_bin("rigg").expect("binary builds");
    cmd.env("RIGG_NO_UPDATE_CHECK", "1");
    cmd.env_remove("RIGG_ENV");
    cmd
}

/// Create a workspace with one project in a temp dir.
fn workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: unit-test-svc }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

#[test]
fn help_shows_project_scoped_surface() {
    rigg()
        .arg("push")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("[PROJECT]"))
        .stdout(predicate::str::contains("--prune"))
        .stdout(predicate::str::contains("--dry-run"));
}

#[test]
fn removed_flags_are_gone() {
    rigg()
        .arg("pull")
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--indexes").not())
        .stdout(predicate::str::contains("--adopt").not());
    // old resource-selection flag now errors
    rigg()
        .args(["pull", "--indexes"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn unknown_command_exits_2() {
    rigg().arg("definitely-not-a-command").assert().code(2);
}

#[test]
fn validate_empty_workspace_passes() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .success();
}

#[test]
fn validate_duplicate_ownership_exits_3() {
    let ws = workspace();
    // same index in two projects
    for p in ["demo", "other"] {
        let dir = ws.path().join("projects").join(p);
        std::fs::create_dir_all(dir.join("search/indexes")).unwrap();
        std::fs::write(dir.join("project.yaml"), "{}\n").unwrap();
        std::fs::write(
            dir.join("search/indexes/shared.json"),
            r#"{"name": "shared", "fields": []}"#,
        )
        .unwrap();
    }
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("exactly one project"));
}

#[test]
fn adopt_help_lists_selectors() {
    rigg()
        .args(["adopt", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("SELECTOR"))
        .stdout(predicate::str::contains("agents/regulus"));
}

#[test]
fn adopt_requires_a_selector() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["adopt", "demo"])
        .assert()
        .code(2);
}

#[test]
fn adopt_rejects_unknown_kind() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["adopt", "demo", "widgets"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown resource kind"));
}

#[test]
fn adopt_without_project_non_interactive_is_usage_error() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .arg("adopt")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("interactive").or(predicate::str::contains("project")));
}

#[test]
fn adopt_project_without_selector_non_interactive_still_usage_error() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["adopt", "demo"])
        .assert()
        .code(2);
}

#[test]
fn validate_rejects_secrets_exit_3() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/search/data-sources");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("bad.json"),
        r#"{"name": "bad", "type": "azureblob", "credentials": {"connectionString": "DefaultEndpointsProtocol=https;AccountName=x;AccountKey=abc123=="}}"#,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(
            predicate::str::contains("never stores secrets")
                .or(predicate::str::contains("AccountKey")),
        );
}

#[test]
fn validate_placeholder_reference_fails() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/search/indexers");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("i.json"),
        r#"{"name": "i", "dataSourceName": "<data-source-name>", "targetIndexName": "missing-index"}"#,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("placeholder"))
        .stderr(predicate::str::contains("missing-index"));
}

#[test]
fn new_project_and_resource_land_in_right_paths() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "project", "alpha"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["new", "index", "docs", "-p", "alpha"])
        .assert()
        .success();
    let index_path = ws.path().join("projects/alpha/search/indexes/docs.json");
    assert!(index_path.is_file());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(index_path).unwrap()).unwrap();
    assert_eq!(v["name"], "docs");
}

#[test]
fn new_datasource_type_validation() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args([
            "new",
            "data-source",
            "ds1",
            "-p",
            "demo",
            "--type",
            "cosmosdb",
        ])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args([
            "new",
            "data-source",
            "ds2",
            "-p",
            "demo",
            "--type",
            "sharepoint",
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("preview"));
    rigg()
        .current_dir(ws.path())
        .args(["new", "data-source", "ds3", "-p", "demo", "--type", "bogus"])
        .assert()
        .code(3);
}

#[test]
fn new_pipeline_scaffolds_explicit_chain() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "pipeline", "rag", "-p", "demo"])
        .assert()
        .success();
    let base = ws.path().join("projects/demo/search");
    for f in [
        "data-sources/rag-ds.json",
        "indexes/rag-index.json",
        "skillsets/rag-skills.json",
        "indexers/rag-indexer.json",
        "knowledge-sources/rag-ks.json",
        "knowledge-bases/rag-kb.json",
    ] {
        assert!(base.join(f).is_file(), "missing {f}");
    }
    // and the whole thing validates (references resolve within the workspace)
    rigg()
        .current_dir(ws.path())
        .args(["validate", "demo"])
        .assert()
        .success();
}

#[test]
fn new_api_scaffolds_openapi_spec() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "api", "doc-enrichment"])
        .assert()
        .success();
    let spec_path = ws.path().join("apis/doc-enrichment.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(spec_path).unwrap()).unwrap();
    assert_eq!(v["openapi"], "3.1.0");
}

#[test]
fn describe_lists_dependencies_and_apis() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "pipeline", "rag", "-p", "demo"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["new", "api", "enrich"])
        .assert()
        .success();
    // link the skillset to the api
    let sk_path = ws
        .path()
        .join("projects/demo/search/skillsets/rag-skills.json");
    let mut sk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&sk_path).unwrap()).unwrap();
    sk["skills"][0]["x-rigg-api"] = serde_json::json!("enrich");
    std::fs::write(&sk_path, serde_json::to_string_pretty(&sk).unwrap()).unwrap();

    rigg()
        .current_dir(ws.path())
        .args(["describe", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("apis_to_implement"))
        .stdout(predicate::str::contains("enrich"))
        .stdout(predicate::str::contains("rag-indexer"));
}

#[test]
fn env_commands_roundtrip() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "add", "prod", "--search-service", "prod-svc"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["env", "list", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prod-svc"));
    rigg()
        .current_dir(ws.path())
        .args(["env", "set-default", "prod"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["env", "remove", "prod"])
        .assert()
        .success();
}

#[test]
fn copy_within_project() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "index", "src-idx", "-p", "demo"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["copy", "indexes/src-idx", "dst-idx"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ws.path().join("projects/demo/search/indexes/dst-idx.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["name"], "dst-idx");
}

#[test]
fn delete_requires_remote_flag() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["delete", "demo"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--remote"));
}

#[test]
fn outside_workspace_errors_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["status"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("not inside a rigg workspace"));
}

#[test]
fn init_writes_workspace_files() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args([
            "init",
            ".",
            "--search-service",
            "unit-test-svc",
            "--env-name",
            "dev",
        ])
        .assert()
        .success();
    assert!(tmp.path().join("rigg.yaml").is_file());
    assert!(tmp.path().join("projects").is_dir());
    assert!(tmp.path().join("apis").is_dir());
    let gi = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gi.contains(".rigg/"));
    // idempotence guard
    rigg()
        .current_dir(tmp.path())
        .args(["init", ".", "--search-service", "x"])
        .assert()
        .code(1);
}

#[test]
fn validate_checks_webapi_skill_contract() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "api", "translate"])
        .assert()
        .success();
    // close the contract: no additionalProperties, specific props
    let spec_path = ws.path().join("apis/translate.json");
    let mut spec: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&spec_path).unwrap()).unwrap();
    let schemas = &mut spec["components"]["schemas"];
    schemas["EnrichmentRequest"]["properties"]["values"]["items"]["properties"]["data"] = serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}, "additionalProperties": false});
    schemas["EnrichmentResponse"]["properties"]["values"]["items"]["properties"]["data"] = serde_json::json!({"type": "object", "properties": {"translation": {"type": "string"}}, "additionalProperties": false});
    std::fs::write(&spec_path, serde_json::to_string_pretty(&spec).unwrap()).unwrap();

    let dir = ws.path().join("projects/demo/search/skillsets");
    std::fs::create_dir_all(&dir).unwrap();
    // conforming skill passes
    std::fs::write(
        dir.join("good.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "good",
            "skills": [{
                "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                "x-rigg-api": "translate",
                "uri": "https://fn.example.com/api/enrich",
                "inputs": [{"name": "text", "source": "/document/content"}],
                "outputs": [{"name": "translation", "targetName": "translation"}]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate", "demo"])
        .assert()
        .success();

    // wrong input name + wrong uri path fails with exit 3
    std::fs::write(
        dir.join("good.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "good",
            "skills": [{
                "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                "x-rigg-api": "translate",
                "uri": "https://fn.example.com/api/wrong-path",
                "inputs": [{"name": "nonexistent", "source": "/document/content"}],
                "outputs": [{"name": "translation", "targetName": "t"}]
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate", "demo"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("wrong-path"))
        .stdout(predicate::str::contains("nonexistent"));
}

#[test]
fn datasource_scaffolds_include_deletion_tracking_and_validate_warns_when_missing() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args([
            "new",
            "data-source",
            "blob-ds",
            "-p",
            "demo",
            "--type",
            "azureblob",
        ])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            ws.path()
                .join("projects/demo/search/data-sources/blob-ds.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(
        v["dataDeletionDetectionPolicy"]["@odata.type"]
            .as_str()
            .unwrap()
            .contains("NativeBlobSoftDelete"),
        "blob scaffold must default to deletion tracking"
    );

    // strip the policy → validate warns (but does not fail)
    let dir = ws.path().join("projects/demo/search/data-sources");
    std::fs::write(
        dir.join("no-del.json"),
        r#"{"name": "no-del", "type": "azureblob", "credentials": {"connectionString": "ResourceId=/subscriptions/x;"}, "container": {"name": "c"}}"#,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate", "demo"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no deletion tracking"));
}

#[test]
fn concepts_explains_the_model() {
    // Runs anywhere — no workspace required.
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .arg("concepts")
        .assert()
        .success()
        .stdout(predicate::str::contains("Workspace"))
        .stdout(predicate::str::contains("exactly one project"));
}

#[test]
fn concepts_no_color_emits_no_ansi() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["concepts", "--no-color"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\u{1b}[").not());
}

#[test]
fn concepts_json_returns_markdown_source() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["concepts", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"concepts\""))
        .stdout(predicate::str::contains("exactly one project"));
}

#[test]
fn help_points_at_concepts() {
    rigg()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("rigg concepts"));
    rigg()
        .args(["new", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("concepts"));
    rigg()
        .args(["pull", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("concepts"));
}

/// A workspace with an environment but NO projects.
fn empty_workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: unit-test-svc }\n",
    )
    .unwrap();
    std::fs::create_dir_all(tmp.path().join("projects")).unwrap();
    tmp
}

#[test]
fn status_empty_workspace_hints_next_steps() {
    let ws = empty_workspace();
    rigg()
        .current_dir(ws.path())
        .arg("status")
        .assert()
        .success()
        .stdout(predicate::str::contains("No projects yet"))
        .stdout(predicate::str::contains("rigg concepts"))
        .stdout(predicate::str::contains("rigg new project"));
}

#[test]
fn describe_empty_workspace_hints_next_steps() {
    let ws = empty_workspace();
    rigg()
        .current_dir(ws.path())
        .arg("describe")
        .assert()
        .success()
        .stdout(predicate::str::contains("No projects yet"));
}

#[test]
fn describe_empty_workspace_json_stays_empty_array() {
    let ws = empty_workspace();
    rigg()
        .current_dir(ws.path())
        .args(["describe", "--output", "json"])
        .assert()
        .success()
        .stdout(predicate::str::is_match(r"^\s*\[\s*\]\s*$").unwrap());
}

#[test]
fn pull_adopt_flag_is_gone() {
    rigg().args(["pull", "--adopt", "demo"]).assert().code(2);
}

#[test]
fn init_next_steps_reference_live_commands() {
    // Regression guard: init's "Next steps" must never point at removed flags
    // (it once suggested the deleted `pull --adopt`).
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["init", ".", "--search-service", "unit-test-svc"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rigg adopt"))
        .stdout(predicate::str::contains("--adopt").not());
}

#[test]
fn new_project_signposts_adopt_path() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["new", "project", "p2"])
        .assert()
        .success()
        .stdout(predicate::str::contains("rigg adopt p2"))
        .stdout(predicate::str::contains("rigg new"));
}

#[test]
fn concepts_includes_naming_guidance() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .arg("concepts")
        .assert()
        .success()
        .stdout(predicate::str::contains("Name a project after"));
}
