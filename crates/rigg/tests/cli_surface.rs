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
        std::fs::create_dir_all(dir.join("envs/dev/search/indexes")).unwrap();
        std::fs::write(dir.join("project.yaml"), "{}\n").unwrap();
        std::fs::write(
            dir.join("envs/dev/search/indexes/shared.json"),
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
fn adopt_help_documents_readoption() {
    rigg()
        .args(["adopt", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("missing dependencies"));
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
fn validate_rejects_secrets_exit_3() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/envs/dev/search/data-sources");
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
    let dir = ws.path().join("projects/demo/envs/dev/search/indexers");
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
    let index_path = ws
        .path()
        .join("projects/alpha/envs/dev/search/indexes/docs.json");
    assert!(index_path.is_file());
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(index_path).unwrap()).unwrap();
    assert_eq!(v["name"], "docs");
}

#[test]
fn new_resource_existence_is_by_physical_name_and_never_clobbers_a_stem() {
    let ws = workspace();
    // `foo.json` holds a RENAMED resource: physical name "bar", stem "foo".
    let dir = ws.path().join("projects/demo/envs/dev/search/indexes");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("foo.json"), r#"{"name": "bar", "fields": []}"#).unwrap();

    // physical name "bar" exists (under stem foo) → "already exists"
    rigg()
        .current_dir(ws.path())
        .args(["new", "index", "bar", "-p", "demo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    // physical name "foo" is free (the stem is taken, the NAME is not) →
    // succeeds without clobbering foo.json
    rigg()
        .current_dir(ws.path())
        .args(["new", "index", "foo", "-p", "demo"])
        .assert()
        .success();
    let original: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("foo.json")).unwrap()).unwrap();
    assert_eq!(original["name"], "bar", "renamed resource untouched");
    let disambiguated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("foo-2.json")).unwrap()).unwrap();
    assert_eq!(disambiguated["name"], "foo", "new resource at a free stem");
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
    let base = ws.path().join("projects/demo/envs/dev/search");
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
        .join("projects/demo/envs/dev/search/skillsets/rag-skills.json");
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
fn env_add_without_flags_non_interactive_is_usage_error() {
    // Regression guard: `rigg env add <name>` with no service flags on a
    // non-interactive session (assert_cmd's stdout is piped, never a TTY)
    // must fail with a usage error that points at the interactive wizard,
    // not silently create an empty environment.
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "add", "test"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("wizard"));
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
        &std::fs::read_to_string(
            ws.path()
                .join("projects/demo/envs/dev/search/indexes/dst-idx.json"),
        )
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
fn init_with_folder_keeps_workspace_in_cwd_and_stores_files_there() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["init", "rag", "--search-service", "unit-test-svc"])
        .assert()
        .success();
    // The current directory is the workspace: rigg.yaml lives here...
    assert!(tmp.path().join("rigg.yaml").is_file());
    let yaml = std::fs::read_to_string(tmp.path().join("rigg.yaml")).unwrap();
    assert!(yaml.contains("root: rag"));
    // ...but rigg's file trees live in the named folder.
    assert!(tmp.path().join("rag/projects").is_dir());
    assert!(tmp.path().join("rag/apis").is_dir());
    assert!(!tmp.path().join("projects").exists());
    let gi = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gi.contains("rag/.rigg/"));
    // Commands run from the workspace root and resolve files under the folder.
    rigg()
        .current_dir(tmp.path())
        .args(["status"])
        .assert()
        .success();
    rigg()
        .current_dir(tmp.path())
        .args(["new", "project", "demo"])
        .assert()
        .success();
    assert!(tmp.path().join("rag/projects/demo/project.yaml").is_file());
    assert!(!tmp.path().join("projects").exists());
    // Re-running init in the same workspace fails: already initialized.
    rigg()
        .current_dir(tmp.path())
        .args(["init", "other", "--search-service", "x"])
        .assert()
        .code(1);
}

#[test]
fn crate_concepts_doc_matches_repo_root_copy() {
    // `rigg concepts` embeds crates/rigg/CONCEPTS.md because cargo publish
    // cannot package files outside the crate; the repo-root CONCEPTS.md is
    // the one people read and edit. Keep them identical (cp CONCEPTS.md
    // crates/rigg/CONCEPTS.md after editing).
    let root_copy = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../CONCEPTS.md");
    if !root_copy.exists() {
        return; // published tarball: only the crate copy exists
    }
    let crate_copy = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("CONCEPTS.md");
    assert_eq!(
        std::fs::read_to_string(&root_copy).unwrap(),
        std::fs::read_to_string(&crate_copy).unwrap(),
        "CONCEPTS.md drifted: run `cp CONCEPTS.md crates/rigg/CONCEPTS.md`"
    );
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

    let dir = ws.path().join("projects/demo/envs/dev/search/skillsets");
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
                .join("projects/demo/envs/dev/search/data-sources/blob-ds.json"),
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
    let dir = ws.path().join("projects/demo/envs/dev/search/data-sources");
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
fn init_output_explains_the_environment() {
    // Regression guard: init's success output must explain the environment
    // it just created (name, that -e/RIGG_ENV select others) and point at
    // `rigg env add` for adding more.
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
        .success()
        .stdout(predicate::str::contains("dev"))
        .stdout(predicate::str::contains("RIGG_ENV"))
        .stdout(predicate::str::contains("rigg env add"));
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

#[test]
fn concepts_includes_environments_chapter() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .arg("concepts")
        .assert()
        .success()
        .stdout(predicate::str::contains("Environments"))
        .stdout(predicate::str::contains("physical"));
}

/// A workspace with two environments (dev + prod) and one project, no
/// resources yet — tests populate `envs/<env>/...` files directly for full
/// control over the dev/prod divergence being exercised.
fn two_env_workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  \
         dev:\n    default: true\n    search: { service: dev-svc }\n    foundry: { account: dev-acct, project: dev-proj }\n  \
         prod:\n    search: { service: prod-svc }\n    foundry: { account: prod-acct, project: prod-proj }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

fn write_json(path: &std::path::Path, value: &serde_json::Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_string_pretty(value).unwrap()).unwrap();
}

fn read_json(path: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn promote_keeps_pinned_fields_applies_other_changes_and_creates_missing_files() {
    let ws = two_env_workspace();
    let dev_agents = ws.path().join("projects/demo/envs/dev/foundry/agents");
    let prod_agents = ws.path().join("projects/demo/envs/prod/foundry/agents");

    // Same logical resource (stem "helper"), diverged physical name in prod
    // (renamed there) plus a pinned tool field — both must survive promote.
    write_json(
        &dev_agents.join("helper.json"),
        &serde_json::json!({
            "name": "helper",
            "model": "gpt-5-mini",
            "instructions": "Be helpful.",
            "tools": [{
                "type": "mcp",
                "server_url": "https://dev.example.search.windows.net/mcp",
                "project_connection_id": "conn-dev"
            }]
        }),
    );
    write_json(
        &prod_agents.join("helper.json"),
        &serde_json::json!({
            "name": "helper-PROD",
            "model": "gpt-4o-old",
            "instructions": "Be helpful.",
            "tools": [{
                "type": "mcp",
                "server_url": "https://prod.example.search.windows.net/mcp",
                "project_connection_id": "conn-prod"
            }]
        }),
    );

    // dev-only index: has no prod counterpart, must be created verbatim.
    let dev_indexes = ws.path().join("projects/demo/envs/dev/search/indexes");
    write_json(
        &dev_indexes.join("docs.json"),
        &serde_json::json!({"name": "docs", "fields": []}),
    );

    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "-y"])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 changed"))
        .stdout(predicate::str::contains("1 new"))
        .stdout(predicate::str::contains("rigg diff demo -e prod"))
        .stdout(predicate::str::contains("rigg push demo -e prod"));

    let prod_helper = read_json(&prod_agents.join("helper.json"));
    assert_eq!(
        prod_helper["name"], "helper-PROD",
        "physical name stays pinned to the target's"
    );
    assert_eq!(
        prod_helper["model"], "gpt-5-mini",
        "non-pinned field promoted from dev"
    );
    assert_eq!(
        prod_helper["tools"][0]["server_url"], "https://prod.example.search.windows.net/mcp",
        "registry-pinned tool field kept from target"
    );
    assert_eq!(
        prod_helper["tools"][0]["project_connection_id"], "conn-prod",
        "registry-pinned connection id kept from target"
    );

    let prod_index_path = ws
        .path()
        .join("projects/demo/envs/prod/search/indexes/docs.json");
    assert!(
        prod_index_path.is_file(),
        "missing resource created in prod"
    );
    assert_eq!(read_json(&prod_index_path)["name"], "docs");
}

#[test]
fn promote_x_rigg_pin_annotation_keeps_extra_path_and_itself() {
    let ws = two_env_workspace();
    let dev_conns = ws.path().join("projects/demo/envs/dev/foundry/connections");
    let prod_conns = ws
        .path()
        .join("projects/demo/envs/prod/foundry/connections");

    write_json(
        &dev_conns.join("c.json"),
        &serde_json::json!({
            "name": "c",
            "properties": {
                "category": "RemoteTool",
                "target": "https://dev-endpoint",
                "description": "dev description"
            }
        }),
    );
    write_json(
        &prod_conns.join("c.json"),
        &serde_json::json!({
            "name": "c",
            "properties": {
                "category": "RemoteTool-OLD",
                "target": "https://prod-endpoint",
                "description": "prod description — do not overwrite"
            },
            "x-rigg-pin": ["properties.description"]
        }),
    );

    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "-y"])
        .assert()
        .success();

    let merged = read_json(&prod_conns.join("c.json"));
    assert_eq!(
        merged["properties"]["category"], "RemoteTool",
        "non-pinned field promoted"
    );
    assert_eq!(
        merged["properties"]["target"], "https://prod-endpoint",
        "properties.target is env-pinned by default for Connection"
    );
    assert_eq!(
        merged["properties"]["description"], "prod description — do not overwrite",
        "x-rigg-pin-listed path kept"
    );
    assert_eq!(
        merged["x-rigg-pin"],
        serde_json::json!(["properties.description"]),
        "the annotation itself survives the promote"
    );
}

#[test]
fn promote_dry_run_writes_nothing() {
    let ws = two_env_workspace();
    let dev_indexes = ws.path().join("projects/demo/envs/dev/search/indexes");
    write_json(
        &dev_indexes.join("docs.json"),
        &serde_json::json!({"name": "docs", "fields": []}),
    );

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("dry run"));

    assert!(
        !ws.path()
            .join("projects/demo/envs/prod/search/indexes/docs.json")
            .exists(),
        "dry-run must not write anything"
    );
}

#[test]
fn promote_non_interactive_without_yes_is_usage_error() {
    let ws = two_env_workspace();
    let dev_indexes = ws.path().join("projects/demo/envs/dev/search/indexes");
    write_json(
        &dev_indexes.join("docs.json"),
        &serde_json::json!({"name": "docs", "fields": []}),
    );

    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "prod"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--yes"));

    assert!(
        !ws.path()
            .join("projects/demo/envs/prod/search/indexes/docs.json")
            .exists()
    );
}

#[test]
fn promote_rejects_same_env() {
    let ws = two_env_workspace();
    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "dev", "-y"])
        .assert()
        .code(2);
}

#[test]
fn promote_rejects_unknown_env() {
    let ws = two_env_workspace();
    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "staging", "-y"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("staging"));
}

#[test]
fn promote_nothing_to_do_when_envs_already_match() {
    let ws = two_env_workspace();
    for env in ["dev", "prod"] {
        write_json(
            &ws.path()
                .join(format!("projects/demo/envs/{env}/search/indexes/docs.json")),
            &serde_json::json!({"name": "docs", "fields": []}),
        );
    }
    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "-y"])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to promote"));
}

#[test]
fn promote_help_documents_local_only_and_pinned_fields() {
    rigg()
        .args(["promote", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("pinned"))
        .stdout(predicate::str::contains("never touches Azure"));
}

#[test]
fn promote_preserves_target_only_tools_end_to_end() {
    // CRITICAL data-loss regression: prod's agent carries tools dev doesn't
    // have (an extra file_search tool). Promote must keep them — the pinned
    // merge appends target-only array elements wholesale.
    let ws = two_env_workspace();
    let dev_agents = ws.path().join("projects/demo/envs/dev/foundry/agents");
    let prod_agents = ws.path().join("projects/demo/envs/prod/foundry/agents");
    write_json(
        &dev_agents.join("helper.json"),
        &serde_json::json!({
            "name": "helper",
            "model": "gpt-5-mini",
            "tools": [{"type": "mcp", "server_url": "https://dev.search.windows.net/mcp"}]
        }),
    );
    write_json(
        &prod_agents.join("helper.json"),
        &serde_json::json!({
            "name": "helper",
            "model": "gpt-4o-old",
            "tools": [
                {"type": "mcp", "server_url": "https://prod.search.windows.net/mcp"},
                {"type": "file_search", "vector_store_ids": ["vs-prod-only"]}
            ]
        }),
    );

    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "-y"])
        .assert()
        .success();

    let prod = read_json(&prod_agents.join("helper.json"));
    let tools = prod["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "prod-only tool survives: {tools:?}");
    assert_eq!(
        tools[0]["server_url"], "https://prod.search.windows.net/mcp",
        "paired tool keeps prod's pinned server_url"
    );
    assert_eq!(
        tools[1],
        serde_json::json!({"type": "file_search", "vector_store_ids": ["vs-prod-only"]}),
        "prod-only tool kept wholesale"
    );
    assert_eq!(prod["model"], "gpt-5-mini", "non-pinned field promoted");
}

#[test]
fn promote_leaves_only_in_to_resources_byte_identical() {
    let ws = two_env_workspace();
    // dev has one index; prod has that index PLUS a prod-only synonym map.
    for env in ["dev", "prod"] {
        write_json(
            &ws.path()
                .join(format!("projects/demo/envs/{env}/search/indexes/docs.json")),
            &serde_json::json!({"name": "docs", "fields": [{"name": env}]}),
        );
    }
    let prod_only = ws
        .path()
        .join("projects/demo/envs/prod/search/synonym-maps/brands.json");
    write_json(
        &prod_only,
        &serde_json::json!({"name": "brands", "format": "solr", "synonyms": "a,b"}),
    );
    let before = std::fs::read(&prod_only).unwrap();

    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "-y"])
        .assert()
        .success()
        .stdout(predicate::str::contains("kept (only in 'prod'"))
        .stdout(predicate::str::contains("synonym-maps/brands"));

    let after = std::fs::read(&prod_only).unwrap();
    assert_eq!(before, after, "only-in-TO file must be byte-identical");
    // and the promoted index did change
    assert_eq!(
        read_json(
            &ws.path()
                .join("projects/demo/envs/prod/search/indexes/docs.json")
        )["fields"][0]["name"],
        "dev"
    );
}

#[test]
fn promote_json_output_has_documented_keys() {
    let ws = two_env_workspace();
    // one changed (index), one created (agent), one kept-only-in-to (alias)
    for (env, field) in [("dev", "new"), ("prod", "old")] {
        write_json(
            &ws.path()
                .join(format!("projects/demo/envs/{env}/search/indexes/docs.json")),
            &serde_json::json!({"name": "docs", "fields": [{"name": field}]}),
        );
    }
    write_json(
        &ws.path()
            .join("projects/demo/envs/dev/foundry/agents/helper.json"),
        &serde_json::json!({"name": "helper", "model": "m"}),
    );
    write_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/aliases/docs-alias.json"),
        &serde_json::json!({"name": "docs-alias", "indexes": ["docs"]}),
    );

    let output = rigg()
        .current_dir(ws.path())
        .args([
            "promote", "demo", "--from", "dev", "--to", "prod", "-y", "--output", "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let v: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON: {e}"));

    assert_eq!(v["promoted"], serde_json::json!(["indexes/docs"]));
    assert_eq!(v["created"], serde_json::json!(["agents/helper"]));
    assert_eq!(
        v["kept_only_in_to"],
        serde_json::json!(["aliases/docs-alias"])
    );
    let pinned = v["pinned_kept"]["indexes/docs"].as_array().unwrap();
    assert!(
        pinned.iter().any(|p| p == "name"),
        "pinned_kept lists the pin paths used: {pinned:?}"
    );
    assert_eq!(v["dry_run"], serde_json::json!(false));

    // the files actually changed
    assert_eq!(
        read_json(
            &ws.path()
                .join("projects/demo/envs/prod/search/indexes/docs.json")
        )["fields"][0]["name"],
        "new"
    );
    assert!(
        ws.path()
            .join("projects/demo/envs/prod/foundry/agents/helper.json")
            .is_file()
    );
}

#[test]
fn migrate_requires_subcommand_and_rejects_conflicting_modes() {
    // no subcommand → clap usage error
    rigg().arg("migrate").assert().code(2);
    // --in-place conflicts with --rename
    rigg()
        .args([
            "migrate",
            "knowledge-source",
            "x",
            "--in-place",
            "--rename",
            "y",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
    // `ks` alias parses (fails later on missing workspace, not on parsing)
    rigg()
        .current_dir(std::env::temp_dir())
        .args(["migrate", "ks", "x", "--in-place"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("workspace").or(predicate::str::contains("rigg init")));
}

#[test]
fn validate_warns_on_datasource_without_credentials() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/envs/dev/search/data-sources");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("copied.json"),
        r#"{"name": "copied", "type": "azureblob", "credentials": {"connectionString": null}, "container": {"name": "docs"}}"#,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .success()
        .stderr(predicate::str::contains("no credentials.connectionString"))
        .stderr(predicate::str::contains("ResourceId="));
}

#[test]
fn dynamic_completion_emits_registration_script() {
    // COMPLETE=<shell> with no args makes the binary print the registration
    // script and exit 0 (clap_complete dynamic engine).
    let out = rigg().env("COMPLETE", "zsh").output().unwrap();
    assert!(out.status.success());
    let script = String::from_utf8_lossy(&out.stdout);
    assert!(script.contains("rigg"), "script: {script}");
    assert!(!script.trim().is_empty());
}

#[test]
fn az_surface_parses() {
    rigg()
        .args(["az", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexer"))
        .stdout(predicate::str::contains("knowledge-base"));
    // kb alias resolves
    rigg()
        .args(["az", "kb", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ask"));
    // reset without name fails parse
    rigg().args(["az", "indexer", "reset"]).assert().code(2);
}

#[test]
fn validate_rejects_real_functions_key_header_exit_3() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/envs/dev/search/skillsets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("enrich.json"),
        r##"{"name": "enrich", "skills": [{
            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
            "uri": "https://fn.azurewebsites.net/api/enrich",
            "httpHeaders": {"X-Functions-Key": "abc123realkey=="}
        }]}"##,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("never stores secrets"));
}

#[test]
fn validate_accepts_redacted_functions_key_header() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/envs/dev/search/skillsets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("enrich.json"),
        r##"{"name": "enrich", "skills": [{
            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
            "uri": "https://fn.azurewebsites.net/api/enrich",
            "x-rigg-auth": "function-key",
            "httpHeaders": {"x-functions-key": "<redacted>"}
        }]}"##,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .success();
}
