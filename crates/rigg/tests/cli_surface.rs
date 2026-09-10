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
fn answer_flags_are_global() {
    rigg()
        .args(["push", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--answer <ID=VALUE>"))
        .stdout(predicate::str::contains("--answers-file"));
}

/// `rigg verify` triggers real indexer runs, so its surface matters: a
/// project or `--all`, and the same typed confirmation every other costly
/// command takes. An unknown project name fails before any network call.
#[test]
fn verify_surface_and_unknown_project() {
    rigg()
        .args(["verify", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[PROJECT]"))
        .stdout(predicate::str::contains("--all"))
        .stdout(predicate::str::contains("--confirm-env <ENV>"));
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .env("RIGG_NON_INTERACTIVE", "1")
        .args(["verify", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("nope"));
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

/// Workspace with two environments, so a command that must not guess which
/// one to act on has to either ask (interactive) or fail (non-interactive).
fn workspace_two_envs() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: unit-test-svc }\n  \
         prod:\n    policy: { protected: true }\n    search: { service: unit-test-svc-prod }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

/// `rigg adopt` with several environments and no `--env` is the cheapest
/// probe for the interactive/non-interactive decision: interactively it
/// offers a pick-list, non-interactively it is a usage error naming the
/// candidates — and it never touches the network either way.
#[test]
fn rigg_non_interactive_env_var_selects_script_mode() {
    let ws = workspace_two_envs();
    rigg()
        .current_dir(ws.path())
        .env("RIGG_NON_INTERACTIVE", "1")
        .args(["adopt", "demo", "all"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("multiple environments configured"))
        .stderr(predicate::str::contains("--env"));
}

#[test]
fn output_json_alone_selects_script_mode() {
    let ws = workspace_two_envs();
    rigg()
        .current_dir(ws.path())
        .args(["adopt", "demo", "all", "--output", "json"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("multiple environments configured"));
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
            "adlsgen2",
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
            "cosmosdb",
        ])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("azureblob, adlsgen2"));
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
fn reserved_binding_name_is_a_workspace_file_error_not_a_missing_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: unit-test-svc }\n    dependencies:\n      search: { storage: acct }\n",
    )
    .unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["status"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::contains("rigg.yaml found at")
                .and(predicate::str::contains("could not be read"))
                .and(predicate::str::contains("reserved name")),
        )
        .stderr(predicate::str::contains("not inside a rigg workspace").not());
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
fn init_records_tenant_and_subscription_flags() {
    let tmp = tempfile::tempdir().unwrap();
    rigg()
        .current_dir(tmp.path())
        .args([
            "init",
            "--search-service",
            "s",
            "--tenant",
            "t-1",
            "--subscription",
            "sub-1",
        ])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(tmp.path().join("rigg.yaml")).unwrap();
    assert!(
        yaml.contains("tenant: t-1") && yaml.contains("subscription: sub-1"),
        "{yaml}"
    );
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
fn removed_search_connection_pin_errors_clearly() {
    // `search-connection`/`foundry-connection` project.yaml pins are gone in
    // rigg.yaml 2.0 (single target per environment); serde's
    // deny_unknown_fields names the removed key in the error.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: unit-test-svc }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "search-connection: x\n").unwrap();
    rigg()
        .current_dir(tmp.path())
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("search-connection"));
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
fn promote_keeps_target_identity_applies_other_changes_and_creates_missing_files() {
    let ws = two_env_workspace();
    let dev_agents = ws.path().join("projects/demo/envs/dev/foundry/agents");
    let prod_agents = ws.path().join("projects/demo/envs/prod/foundry/agents");

    // Same logical resource (stem "helper"), diverged physical name in prod
    // (renamed there): the target's identity is never promoted over.
    write_json(
        &dev_agents.join("helper.json"),
        &serde_json::json!({
            "name": "helper",
            "model": "gpt-5-mini",
            "instructions": "Be helpful.",
            "tools": []
        }),
    );
    write_json(
        &prod_agents.join("helper.json"),
        &serde_json::json!({
            "name": "helper-PROD",
            "model": "gpt-4o-old",
            "instructions": "Be helpful.",
            "tools": []
        }),
    );

    // dev-only index: has no prod counterpart, must be created.
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
            "-y",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 changed"))
        .stdout(predicate::str::contains("1 new"))
        .stdout(predicate::str::contains("rigg validate demo"))
        .stdout(predicate::str::contains("rigg push demo -e prod"));

    let prod_helper = read_json(&prod_agents.join("helper.json"));
    assert_eq!(
        prod_helper["name"], "helper-PROD",
        "physical identity always stays the target's"
    );
    assert_eq!(
        prod_helper["model"], "gpt-5-mini",
        "everything else is promoted from dev"
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
        .args([
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
            // The target is an endpoint bound in neither environment:
            // promote asks before keeping it verbatim.
            "--answer",
            "promote.external.dev-endpoint=yes",
        ])
        .assert()
        .success();

    let merged = read_json(&prod_conns.join("c.json"));
    assert_eq!(
        merged["properties"]["category"], "RemoteTool",
        "unpinned field promoted"
    );
    assert_eq!(
        merged["properties"]["target"], "https://dev-endpoint",
        "2.0: nothing is pinned by kind — an external endpoint kept verbatim \
         promotes like any other field"
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
fn promote_dry_run_shows_rewiring_and_writes_nothing() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "devacct");
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--dry-run",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Rewiring")
                .and(predicate::str::contains("docs"))
                .and(predicate::str::contains("devacct"))
                .and(predicate::str::contains("prodacct"))
                .and(predicate::str::contains("dry run")),
        );
    assert!(
        !ws.path()
            .join("projects/demo/envs/prod/search/data-sources/ds.json")
            .exists(),
        "dry-run must not write anything"
    );
}

#[test]
fn promote_writes_translated_files_with_yes() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "devacct");
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
        ])
        .assert()
        .success();
    let prod = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/data-sources/ds.json"),
    );
    assert!(
        prod["credentials"]["connectionString"]
            .as_str()
            .unwrap()
            .contains("prodacct"),
        "the storage reference is rewired to prod's binding: {prod}"
    );
    // idempotent: a second promote has nothing to do
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to promote"));
}

#[test]
fn promote_rewires_a_data_source_that_already_exists_in_the_target() {
    // The write-only `credentials.connectionString` is exactly what promote
    // translates; writing it must not carry the target's old (dev-pointing)
    // value back over, or promote would never converge.
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "devacct");
    write_ds(ws.path(), "prod", "ds", "devacct");
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
        ])
        .assert()
        .success();
    let prod = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/data-sources/ds.json"),
    );
    assert!(
        prod["credentials"]["connectionString"]
            .as_str()
            .unwrap()
            .contains("prodacct"),
        "the existing prod file is rewired to prod's binding: {prod}"
    );
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to promote"));
}

#[test]
fn promote_dry_run_does_not_record_answered_bindings() {
    let ws = workspace_two_envs_with_bindings();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "fn", "function-app:mklab-dev"])
        .assert()
        .success();
    write_skillset_with_webapi(
        ws.path(),
        "dev",
        "ss",
        "https://mklab-dev.azurewebsites.net/api/enrich",
    );
    let before = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--dry-run",
            "--offline",
            "--answer",
            "binding.prod.fn=same",
        ])
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap(),
        before,
        "--dry-run must leave rigg.yaml untouched"
    );
}

#[test]
fn promote_needs_input_exit_does_not_record_answered_bindings() {
    // One question is answered, a second one it uncovers is not: the run
    // exits 6 and must not have written the first answer to rigg.yaml.
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "otheracct");
    let before = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--offline",
            "--output",
            "json",
            "--answer",
            "promote.bind.dev.otheracct=other",
        ])
        .assert()
        .code(6);
    assert_eq!(
        std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap(),
        before,
        "an exit-6 run must leave rigg.yaml untouched"
    );
}

#[test]
fn promote_missing_target_binding_emits_needs_input_non_interactively() {
    let ws = workspace_two_envs_with_bindings();
    // a dev-only binding, and a file using it
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "fn", "function-app:mklab-dev"])
        .assert()
        .success();
    write_skillset_with_webapi(
        ws.path(),
        "dev",
        "ss",
        "https://mklab-dev.azurewebsites.net/api/enrich",
    );

    let out = rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--offline",
            "--output",
            "json",
        ])
        .assert()
        .code(6);
    let doc: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(doc["questions"][0]["id"], "binding.prod.fn");
    let cands: Vec<&str> = doc["questions"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        cands.contains(&"same") && cands.contains(&"skip"),
        "candidates: {cands:?}"
    );

    // answer: same → prod gets fn = mklab-dev (shared) and the skillset is written
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
            "--answer",
            "binding.prod.fn=same",
        ])
        .assert()
        .success();
    assert!(
        std::fs::read_to_string(ws.path().join("rigg.yaml"))
            .unwrap()
            .matches("mklab-dev")
            .count()
            >= 2,
        "both environments now bind mklab-dev"
    );
    assert!(
        ws.path()
            .join("projects/demo/envs/prod/search/skillsets/ss.json")
            .is_file()
    );
}

#[test]
fn promote_summarizes_references_kept_from_the_source() {
    // A skipped binding leaves the written file pointing at dev's function
    // app: the run succeeds, but must not end without saying so.
    let ws = workspace_two_envs_with_bindings();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "fn", "function-app:mklab-dev"])
        .assert()
        .success();
    write_skillset_with_webapi(
        ws.path(),
        "dev",
        "ss",
        "https://mklab-dev.azurewebsites.net/api/enrich",
    );

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
            "--answer",
            "binding.prod.fn=skip",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 reference(s) kept from 'dev'"))
        .stdout(predicate::str::contains("rigg validate"));
    let written = std::fs::read_to_string(
        ws.path()
            .join("projects/demo/envs/prod/search/skillsets/ss.json"),
    )
    .unwrap();
    assert!(
        written.contains("mklab-dev"),
        "the reference really is kept: {written}"
    );
}

#[test]
fn promote_into_unknown_env_non_interactive_points_at_env_add() {
    let ws = workspace_two_envs_with_bindings();
    rigg()
        .current_dir(ws.path())
        .args(["promote", "--from", "dev", "--to", "staging", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("rigg env add staging --like dev"))
        .stderr(predicate::str::contains(
            "(adjust the targets for 'staging')",
        ));
}

#[test]
fn promote_renames_sibling_references_in_the_target() {
    let ws = two_env_workspace();
    // dev: the index is physically named docs-index-dev; the indexer points
    // at that name. prod already has the same logical index, named
    // docs-index.
    write_json(
        &ws.path()
            .join("projects/demo/envs/dev/search/indexes/docs-index.json"),
        &serde_json::json!({"name": "docs-index-dev", "fields": []}),
    );
    write_json(
        &ws.path()
            .join("projects/demo/envs/dev/search/indexers/ix.json"),
        &serde_json::json!({
            "name": "ix",
            "dataSourceName": "ds",
            "targetIndexName": "docs-index-dev"
        }),
    );
    write_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/indexes/docs-index.json"),
        &serde_json::json!({"name": "docs-index", "fields": [{"name": "old"}]}),
    );

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Renamed siblings")
                .and(predicate::str::contains("docs-index-dev → docs-index")),
        );

    let ix = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/indexers/ix.json"),
    );
    assert_eq!(
        ix["targetIndexName"], "docs-index",
        "the reference follows the sibling's physical name in prod"
    );
    assert_eq!(
        read_json(
            &ws.path()
                .join("projects/demo/envs/prod/search/indexes/docs-index.json")
        )["name"],
        "docs-index",
        "prod keeps its own physical name"
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
fn promote_rejects_unknown_source_env() {
    let ws = two_env_workspace();
    rigg()
        .current_dir(ws.path())
        .args(["promote", "demo", "--from", "staging", "--to", "prod", "-y"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("staging").and(predicate::str::contains("rigg env list")));
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
        .args([
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("nothing to promote"));
}

#[test]
fn promote_help_documents_translation_and_offline() {
    rigg()
        .args(["promote", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--offline"))
        .stdout(predicate::str::contains("x-rigg-pin"));
}

#[test]
fn promote_pinned_array_path_preserves_target_only_tools_end_to_end() {
    // CRITICAL data-loss regression: prod's agent carries tools dev doesn't
    // have (an extra file_search tool). With the tool list pinned by the
    // target's own `x-rigg-pin`, promote must keep them — the restore
    // appends target-only array elements wholesale.
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
            ],
            "x-rigg-pin": ["tools[].server_url"]
        }),
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
            "-y",
            "--offline",
        ])
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
    assert_eq!(prod["model"], "gpt-5-mini", "unpinned field promoted");
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
        .args([
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
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
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let v: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("stdout must be pure JSON: {e}"));

    assert_eq!(
        v["resources"]["changed"],
        serde_json::json!(["indexes/docs"])
    );
    assert_eq!(v["resources"]["new"], serde_json::json!(["agents/helper"]));
    assert_eq!(
        v["resources"]["kept_only_in_to"],
        serde_json::json!(["aliases/docs-alias"])
    );
    assert_eq!(v["targets"]["search"]["from"], "dev-svc");
    assert_eq!(v["targets"]["search"]["to"], "prod-svc");
    for key in ["rewiring", "renamed", "checks", "questions"] {
        assert!(v[key].is_array(), "documented key '{key}' missing: {v}");
    }
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

#[test]
fn promote_defaults_to_the_only_project() {
    let ws = two_env_workspace();
    let dev_agents = ws.path().join("projects/demo/envs/dev/foundry/agents");
    write_json(
        &dev_agents.join("helper.json"),
        &serde_json::json!({"name": "helper", "model": "gpt-5-mini"}),
    );

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
        .assert()
        .success();
    assert!(
        ws.path()
            .join("projects/demo/envs/prod/foundry/agents/helper.json")
            .is_file(),
        "the only project is promoted without naming it"
    );
}

#[test]
fn promote_multi_project_without_name_is_usage_error() {
    let ws = two_env_workspace();
    let other = ws.path().join("projects/other");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("project.yaml"), "{}\n").unwrap();

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("name one"));
}

/// Workspace whose two environments each bind a `fn` function app.
fn workspace_two_envs_with_function_apps() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: s-dev }\n    dependencies:\n      fn: { function-app: fn-dev }\n  prod:\n    search: { service: s-prod }\n    dependencies:\n      fn: { function-app: fn-prod }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects/demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

#[test]
fn promote_translates_the_function_url_and_keeps_the_targets_auth_carrier() {
    let ws = workspace_two_envs_with_function_apps();
    let dev = ws.path().join("projects/demo/envs/dev/search/skillsets");
    let prod = ws.path().join("projects/demo/envs/prod/search/skillsets");
    write_json(
        &dev.join("enrich.json"),
        &serde_json::json!({
            "name": "enrich",
            "description": "v2 with better prompts",
            "skills": [{
                "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                "name": "ExtractMetadata",
                "uri": "https://fn-dev.azurewebsites.net/api/ExtractMetadata",
                "x-rigg-auth": "function-key",
                "httpHeaders": {"x-functions-key": "<redacted>"},
                "inputs": [],
                "outputs": []
            }]
        }),
    );
    write_json(
        &prod.join("enrich.json"),
        &serde_json::json!({
            "name": "enrich",
            "description": "old",
            "skills": [{
                "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                "name": "ExtractMetadata",
                "uri": "https://fn-prod.azurewebsites.net/api/ExtractMetadata",
                "authResourceId": "api://prod-fn",
                "inputs": [],
                "outputs": []
            }]
        }),
    );

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("fn-dev").and(predicate::str::contains("fn-prod")));

    let promoted = read_json(&prod.join("enrich.json"));
    assert_eq!(
        promoted["description"], "v2 with better prompts",
        "content promotes"
    );
    assert_eq!(
        promoted["skills"][0]["uri"], "https://fn-prod.azurewebsites.net/api/ExtractMetadata",
        "the URL is translated through the 'fn' binding, not copied"
    );
    assert_eq!(
        promoted["skills"][0]["authResourceId"], "api://prod-fn",
        "the target env keeps its own auth carrier"
    );
    assert!(
        promoted["skills"][0].get("x-rigg-auth").is_none(),
        "the source env's auth annotation must not leak into the target"
    );
    assert!(
        promoted["skills"][0]
            .get("httpHeaders")
            .and_then(|h| h.get("x-functions-key"))
            .is_none(),
        "the source env's key header must not leak into the target"
    );
}

#[test]
fn promote_new_skillset_translates_the_function_url_and_reports_the_auth_carrier() {
    let ws = workspace_two_envs_with_function_apps();
    write_skillset_with_webapi(
        ws.path(),
        "dev",
        "enrich",
        "https://fn-dev.azurewebsites.net/api/enrich?code=<redacted>",
    );

    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "-y",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("auth carrier"));

    let promoted = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/skillsets/enrich.json"),
    );
    assert_eq!(
        promoted["skills"][0]["uri"], "https://fn-prod.azurewebsites.net/api/enrich",
        "the new file points at the TARGET env's function app, key stripped"
    );
}

#[test]
fn promote_unbound_source_reference_asks_to_bind_it() {
    let ws = workspace_two_envs_with_bindings();
    // A storage account no environment binds: promote cannot translate it.
    write_ds(ws.path(), "dev", "ds", "otheracct");

    let out = rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--offline",
            "--output",
            "json",
        ])
        .assert()
        .code(6);
    let doc: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(doc["questions"][0]["id"], "promote.bind.dev.otheracct");
    assert_eq!(doc["questions"][0]["default"], "otheracct");
    assert!(
        !ws.path()
            .join("projects/demo/envs/prod/search/data-sources/ds.json")
            .exists(),
        "nothing is written while a question is open"
    );

    // Answering it records the binding in the SOURCE environment.
    rigg()
        .current_dir(ws.path())
        .args([
            "promote",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
            "--answer",
            "promote.bind.dev.otheracct=other",
            "--answer",
            "binding.prod.other=same",
        ])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(
        yaml.contains("other:"),
        "binding recorded in rigg.yaml: {yaml}"
    );
}

#[test]
fn unknown_answer_id_is_a_usage_error() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["status", "--answer", "nope=1"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown answer id 'nope'"));
}

/// The ARM id `write_ds` embeds for a storage account, and the value the
/// `docs` bindings below declare — a storage reference can only be
/// *rewritten* (by `rigg promote`) when the target binding carries the full
/// id, so the bindings declare ids rather than bare names.
fn storage_id(account: &str) -> String {
    format!(
        "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/{account}"
    )
}

/// Workspace with two environments, each declaring a `docs` storage
/// dependency binding pointing at a different physical account — used by
/// the infra-reference classification and promote tests below.
fn workspace_two_envs_with_bindings() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "environments:\n  dev:\n    default: true\n    search: {{ service: s-dev }}\n    dependencies:\n      docs: {{ storage: {} }}\n  prod:\n    policy: {{ protected: true }}\n    search: {{ service: s-prod }}\n    dependencies:\n      docs: {{ storage: {} }}\n",
            storage_id("devacct"),
            storage_id("prodacct"),
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects/demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

/// Write a data source file in `env` referencing storage account `account`
/// via an identity-based `ResourceId=` connection string.
fn write_ds(ws: &std::path::Path, env: &str, name: &str, account: &str) {
    let d = ws.join(format!("projects/demo/envs/{env}/search/data-sources"));
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join(format!("{name}.json")), format!(r#"{{"name":"{name}","type":"azureblob","credentials":{{"connectionString":"ResourceId={};"}},"container":{{"name":"c"}}}}"#, storage_id(account))).unwrap();
}

/// Write a skillset in `env` whose single Web API skill calls `uri`.
fn write_skillset_with_webapi(ws: &std::path::Path, env: &str, name: &str, uri: &str) {
    let d = ws.join(format!("projects/demo/envs/{env}/search/skillsets"));
    std::fs::create_dir_all(&d).unwrap();
    write_json(
        &d.join(format!("{name}.json")),
        &serde_json::json!({
            "name": name,
            "skills": [{
                "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
                "name": "enrich",
                "uri": uri,
                "inputs": [],
                "outputs": []
            }]
        }),
    );
}

#[test]
fn validate_flags_a_prod_file_pointing_at_dev_storage_as_a_leak() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "prod", "ds", "devacct");
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains(
            "bound in environment 'dev' as 'docs' but not in 'prod'",
        ));
}

#[test]
fn validate_warns_on_unbound_in_dev_but_errors_in_protected_prod() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "otheracct");
    rigg()
        .current_dir(ws.path())
        .args(["validate"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("no environment binds")
                .and(predicate::str::contains("rigg env bind dev --learn")),
        );
    write_ds(ws.path(), "prod", "ds2", "otheracct");
    rigg()
        .current_dir(ws.path())
        .args(["validate", "--output", "json"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("\"valid\": false"));
}

#[test]
fn validate_leak_hint_uses_a_real_binding_type_keyword() {
    // A model-host leak must suggest `ai-services:<name>` — `model host` is
    // the display word for the target, not something `rigg env bind` accepts.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: s-dev }\n    dependencies:\n      enrichment: { ai-services: devaisrvc }\n  prod:\n    search: { service: s-prod }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects/demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    let d = tmp.path().join("projects/demo/envs/prod/search/skillsets");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("enrich.json"),
        r##"{"name":"enrich","skills":[{"@odata.type":"#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill","resourceUri":"https://devaisrvc.openai.azure.com","inputs":[],"outputs":[]}]}"##,
    )
    .unwrap();

    rigg()
        .current_dir(tmp.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(
            predicate::str::contains("rigg env bind prod enrichment ai-services:devaisrvc")
                .and(predicate::str::contains("model host:devaisrvc").not()),
        );
}

#[test]
fn validate_leak_onto_another_environments_foundry_account_suggests_the_target() {
    // The other environment's match is its implicit `foundry` binding, which
    // no `rigg env bind` can declare — the fix is the target or the file.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    search: { service: s-dev }\n    foundry: { account: devfndr, project: p }\n  prod:\n    search: { service: s-prod }\n    foundry: { account: prodfndr, project: p }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects/demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    let d = tmp.path().join("projects/demo/envs/prod/search/skillsets");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("enrich.json"),
        r##"{"name":"enrich","skills":[{"@odata.type":"#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill","resourceUri":"https://devfndr.openai.azure.com","inputs":[],"outputs":[]}]}"##,
    )
    .unwrap();

    rigg()
        .current_dir(tmp.path())
        .args(["validate"])
        .assert()
        .code(3)
        .stdout(
            predicate::str::contains("another environment's Foundry account")
                .and(predicate::str::contains("env bind").not()),
        );
}

#[test]
fn unreadable_rigg_yaml_is_not_reported_as_a_missing_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "defaults:\n  identity: legacy\nenvironments:\n  dev:\n    default: true\n    search: { service: s }\n",
    )
    .unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["status"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("rigg.yaml found at")
                .and(predicate::str::contains("unknown field `defaults`"))
                .and(predicate::str::contains("run `rigg init`").not()),
        );
}

#[test]
fn validate_show_bindings_lists_bound_and_shared() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "ds", "devacct");
    rigg()
        .current_dir(ws.path())
        .args(["validate", "--show-bindings"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bound 'docs'"));
}

#[test]
fn env_bind_and_unbind_edit_rigg_yaml() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "storage:mklabstorageacc"])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(
        yaml.contains("docs:") && yaml.contains("storage: mklabstorageacc"),
        "{yaml}"
    );
    // reserved binding name
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "search", "storage:x"])
        .assert()
        .code(2);
    // unknown binding type
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "cosmos:x"])
        .assert()
        .code(2);
    rigg()
        .current_dir(ws.path())
        .args(["env", "unbind", "dev", "docs"])
        .assert()
        .success();
    assert!(
        !std::fs::read_to_string(ws.path().join("rigg.yaml"))
            .unwrap()
            .contains("docs:")
    );
}

#[test]
fn env_bind_on_an_already_bound_name_replaces_it() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "storage:acct-a"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "storage:acct-b"])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert_eq!(
        yaml.matches("docs:").count(),
        1,
        "rebinding 'docs' replaces the entry rather than duplicating it: {yaml}"
    );
    assert!(
        yaml.contains("storage: acct-b") && !yaml.contains("acct-a"),
        "{yaml}"
    );
}

#[test]
fn env_bind_preserves_the_environments_other_fields() {
    // edit_workspace_yaml round-trip: writing a binding must not drop the
    // environment's tenant/subscription/policy fields.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  dev:\n    default: true\n    tenant: t-1\n    subscription: s-1\n    policy: { protected: true }\n    search: { service: unit-test-svc }\n",
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    rigg()
        .current_dir(tmp.path())
        .args(["env", "bind", "dev", "docs", "storage:x"])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(tmp.path().join("rigg.yaml")).unwrap();
    assert!(yaml.contains("tenant: t-1"), "{yaml}");
    assert!(yaml.contains("subscription: s-1"), "{yaml}");
    assert!(yaml.contains("protected: true"), "{yaml}");
}

#[test]
fn env_bind_learn_proposes_from_files_and_writes_with_yes() {
    let ws = workspace();
    write_ds(ws.path(), "dev", "ds", "mklabstorageacc");
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "--learn", "--yes"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("mklabstorageacc").and(predicate::str::contains("storage")),
        );
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    // The file carried a full ARM id, so the learned binding keeps it — no
    // by-name ARM lookup (and no subscription guess) needed later.
    assert!(
        yaml.contains(
            "storage: /subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/mklabstorageacc"
        ),
        "{yaml}"
    );

    // non-interactive without --yes: needs-input with learn.dev.otheracct
    write_ds(ws.path(), "dev", "ds2", "otheracct");
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "--learn", "--output", "json"])
        .assert()
        .code(6)
        .stdout(predicate::str::contains("learn.dev.otheracct"));
    // …and answering it writes the binding under the given name
    rigg()
        .current_dir(ws.path())
        .args([
            "env",
            "bind",
            "dev",
            "--learn",
            "--answer",
            "learn.dev.otheracct=archive",
        ])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(
        yaml.contains("archive:") && yaml.contains("storageAccounts/otheracct"),
        "{yaml}"
    );
}

#[test]
fn env_bind_learn_skips_a_proposal_answered_with_skip() {
    let ws = workspace();
    write_ds(ws.path(), "dev", "ds", "otheracct");
    rigg()
        .current_dir(ws.path())
        .args([
            "env",
            "bind",
            "dev",
            "--learn",
            "--answer",
            "learn.dev.otheracct=skip",
        ])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert!(!yaml.contains("otheracct"), "{yaml}");
}

#[test]
fn env_add_with_like_flags_copies_and_overrides_bindings() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "storage:devacct"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "fn", "function-app:mklab"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args([
            "env",
            "add",
            "prod",
            "--search-service",
            "s-prod",
            "--like",
            "dev",
            "--same",
            "fn",
            "--bind",
            "docs=storage:prodacct",
            "--protected",
        ])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["env", "show", "prod"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("docs")
                .and(predicate::str::contains("prodacct"))
                .and(predicate::str::contains("fn"))
                .and(predicate::str::contains("shared with: dev"))
                .and(predicate::str::contains("protected: true")),
        );
}

#[test]
fn env_add_like_skip_drops_a_binding_and_unknown_same_is_a_usage_error() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "storage:devacct"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args([
            "env",
            "add",
            "prod",
            "--search-service",
            "s-prod",
            "--like",
            "dev",
            "--skip",
            "docs",
        ])
        .assert()
        .success();
    let yaml = std::fs::read_to_string(ws.path().join("rigg.yaml")).unwrap();
    assert_eq!(
        yaml.matches("devacct").count(),
        1,
        "only dev keeps the binding: {yaml}"
    );
    rigg()
        .current_dir(ws.path())
        .args([
            "env",
            "add",
            "stage",
            "--search-service",
            "s",
            "--like",
            "dev",
            "--same",
            "nope",
        ])
        .assert()
        .code(2);
    // --same/--skip without --like is a usage error
    rigg()
        .current_dir(ws.path())
        .args([
            "env",
            "add",
            "other",
            "--search-service",
            "s",
            "--skip",
            "docs",
        ])
        .assert()
        .code(2);
}

#[test]
fn env_add_like_with_no_bindings_and_no_targets_is_a_usage_error() {
    // `dev` here has no dependencies at all, so `--like dev` alone gives
    // `env add` nothing to write — that must fail loudly, not silently
    // create a target-less, binding-less environment.
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "add", "empty", "--like", "dev"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("nothing to add"));
}

#[test]
fn describe_lists_infrastructure() {
    let ws = workspace();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "docs", "storage:devacct"])
        .assert()
        .success();
    rigg()
        .current_dir(ws.path())
        .args(["describe", "--output", "json"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("\"infrastructure\"").and(predicate::str::contains("devacct")),
        );
    rigg()
        .current_dir(ws.path())
        .args(["describe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Infrastructure:").and(predicate::str::contains("docs")));
}

/// `rigg ci init`'s role list comes from the identity graph, not from a
/// canned paragraph: a bound storage dependency is named with its role AND
/// the ARM scope the grant has to be made at, and a scope the bindings cache
/// cannot resolve says so instead of being invented.
#[test]
fn ci_init_role_list_names_the_bound_storage_role_with_its_scope() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "dev", "docs-ds", "devacct");
    rigg()
        .current_dir(ws.path())
        .env("RIGG_NON_INTERACTIVE", "1")
        .args(["ci", "init"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("Storage Blob Data Reader")
                .and(predicate::str::contains(storage_id("devacct")))
                .and(predicate::str::contains("Search Service Contributor"))
                .and(predicate::str::contains("rigg env bind dev --learn")),
        );
}

/// The environment is the one selected, not always the default one — the
/// workflows bake it in, so `-e prod` must produce prod's scopes.
#[test]
fn ci_init_uses_the_selected_environment() {
    let ws = workspace_two_envs_with_bindings();
    write_ds(ws.path(), "prod", "docs-ds", "prodacct");
    rigg()
        .current_dir(ws.path())
        .env("RIGG_NON_INTERACTIVE", "1")
        .args(["ci", "init", "-e", "prod"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains(storage_id("prodacct"))
                .and(predicate::str::contains(storage_id("devacct")).not()),
        );
    let deploy =
        std::fs::read_to_string(ws.path().join(".github/workflows/rigg-deploy.yml")).unwrap();
    assert!(deploy.contains("prod"), "deploy workflow targets prod");
}

/// Every scaffolded model deployment names Azure's built-in RAI policy
/// (`Microsoft.DefaultV2`). That is a platform resource, never a workspace
/// file, so `validate --strict` must not treat it as a dangling reference —
/// otherwise strict mode rejects rigg's own scaffold output.
#[test]
fn validate_strict_accepts_builtin_guardrail_references() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/envs/dev/foundry/deployments");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("gpt.json"),
        r#"{"name": "gpt", "sku": {"name": "GlobalStandard", "capacity": 1},
            "properties": {"model": {"format": "OpenAI", "name": "gpt", "version": "1"},
                           "raiPolicyName": "Microsoft.DefaultV2"}}"#,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate", "--strict"])
        .assert()
        .success()
        .stdout(predicate::str::contains("all checks passed"));

    // A guardrail that is not one of Azure's built-ins still has to exist.
    std::fs::write(
        dir.join("gpt.json"),
        r#"{"name": "gpt", "sku": {"name": "GlobalStandard", "capacity": 1},
            "properties": {"model": {"format": "OpenAI", "name": "gpt", "version": "1"},
                           "raiPolicyName": "house-policy"}}"#,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["validate", "--strict"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("guardrails/house-policy"));
}

/// The proposal table names files the way the rest of the CLI does —
/// relative to the workspace root. An absolute path makes the table
/// unreadable and leaks the operator's home directory into transcripts.
#[test]
fn learned_bindings_name_files_relative_to_the_workspace() {
    let ws = workspace();
    let dir = ws.path().join("projects/demo/envs/dev/search/skillsets");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("s.json"),
        r##"{"name": "s", "skills": [{"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
             "name": "enrich", "context": "/document",
             "uri": "https://docs-enrich.azurewebsites.net/api/enrich",
             "inputs": [], "outputs": []}]}"##,
    )
    .unwrap();
    rigg()
        .current_dir(ws.path())
        .args(["env", "bind", "dev", "--learn", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "projects/demo/envs/dev/search/skillsets/s.json:skills[0].uri",
        ))
        .stdout(predicate::str::contains(ws.path().display().to_string().as_str()).not());
}

/// `rigg ai skill` is what an AI agent reads to learn rigg, so it must not
/// teach commands the binary does not have. Both halves are written into a
/// scratch docs tree and run through `rigg dev docs-check`, the same
/// mechanical honesty check the repository's own pages pass: every `rigg …`
/// line in a fence parses through clap and every link resolves.
#[test]
fn ai_skill_output_passes_docs_check() {
    let tmp = tempfile::tempdir().unwrap();
    let docs = tmp.path().join("docs");
    std::fs::create_dir_all(&docs).unwrap();

    for (args, name) in [
        (vec!["ai", "skill"], "ai-skill-guide.md"),
        (vec!["ai", "skill", "--emit"], "ai-skill.md"),
        (vec!["ai", "skill", "--reference"], "ai-reference.md"),
    ] {
        let out = rigg().args(&args).assert().success();
        std::fs::write(docs.join(name), &out.get_output().stdout).unwrap();
    }

    let out = rigg()
        .args(["dev", "docs-check", "--root"])
        .arg(tmp.path())
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("UTF-8");
    assert!(
        stdout.trim_end().ends_with("docs-check: ok"),
        "docs-check did not report ok:\n{stdout}"
    );
}
