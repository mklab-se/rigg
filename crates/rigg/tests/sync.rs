//! Sync-engine tests against a mocked Azure AI Search service (wiremock).
//!
//! Uses `endpoint:` connection override + `RIGG_ACCESS_TOKEN` static auth so
//! the real binary talks to the mock over HTTP with no Azure involved.

use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::{Value, json};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn rigg(dir: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("rigg").expect("binary builds");
    cmd.current_dir(dir);
    cmd.env("RIGG_NO_UPDATE_CHECK", "1");
    cmd.env("RIGG_ACCESS_TOKEN", "test-token");
    cmd.env_remove("RIGG_ENV");
    cmd
}

/// Workspace with one project pointed at the mock server.
fn workspace(endpoint: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "environments:\n  dev:\n    default: true\n    search: {{ service: mock, endpoint: \"{endpoint}\" }}\n"
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

fn write_resource(ws: &std::path::Path, dir: &str, name: &str, body: &Value) {
    write_resource_env(ws, "dev", dir, name, body);
}

fn write_resource_env(ws: &std::path::Path, env: &str, dir: &str, name: &str, body: &Value) {
    let d = ws
        .join("projects/demo/envs")
        .join(env)
        .join("search")
        .join(dir);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join(format!("{name}.json")),
        serde_json::to_string_pretty(body).unwrap(),
    )
    .unwrap();
}

/// Workspace with a `dev` (default, unprotected) and `prod` (protected) env,
/// both pointed at the same mock server.
fn workspace_with_protected_prod(endpoint: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        format!(
            "environments:\n  dev:\n    default: true\n    search: {{ service: mock, endpoint: \"{endpoint}\" }}\n  prod:\n    policy: {{ protected: true }}\n    search: {{ service: mock, endpoint: \"{endpoint}\" }}\n"
        ),
    )
    .unwrap();
    let proj = tmp.path().join("projects").join("demo");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(proj.join("project.yaml"), "{}\n").unwrap();
    tmp
}

/// Mock empty list responses for every search kind except overridden ones.
async fn mock_empty_lists(server: &MockServer) {
    for p in [
        "datasources",
        "indexes",
        "skillsets",
        "indexers",
        "synonymmaps",
        "aliases",
        "knowledgeSources",
        "knowledgeBases",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn pull_writes_normalized_files_and_skips_volatile_noise() {
    let server = MockServer::start().await;
    // one remote index with volatile fields (mounted first: first match wins)
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "@odata.etag": "\"0x8D0\"",
                "name": "docs",
                "fields": [{"name": "id", "type": "Edm.String", "key": true}]
            }]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    // adopt the resource (it is unmanaged before adoption)
    rigg(ws.path())
        .args(["adopt", "demo", "indexes/docs"])
        .assert()
        .success();

    let file = ws
        .path()
        .join("projects/demo/envs/dev/search/indexes/docs.json");
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    assert!(v.get("@odata.etag").is_none(), "volatile stripped");
    assert_eq!(v["name"], "docs");

    // second pull with only volatile noise changed → file untouched
    let mtime = std::fs::metadata(&file).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    rigg(ws.path())
        .args(["pull", "demo", "--yes"])
        .assert()
        .success();
    let mtime2 = std::fs::metadata(&file).unwrap().modified().unwrap();
    assert_eq!(mtime, mtime2, "no rewrite when nothing semantic changed");
}

#[tokio::test]
async fn push_orders_dependencies_and_canonicalizes() {
    let server = MockServer::start().await;
    mock_empty_lists(&server).await;

    // GETs for the specific resources: 404 before creation
    for p in ["/datasources/ds", "/indexes/idx", "/indexers/idxr"] {
        Mock::given(method("GET"))
            .and(path(p.to_string()))
            .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
            .mount(&server)
            .await;
    }
    // PUTs succeed and echo the body + a server-added default and etag
    let put_recorder = |extra: Value| {
        move |req: &Request| {
            let mut body: Value = serde_json::from_slice(&req.body).unwrap();
            if let Some(obj) = body.as_object_mut() {
                obj.insert("@odata.etag".into(), json!("\"0xNEW\""));
                if let Some(extra_obj) = extra.as_object() {
                    for (k, v) in extra_obj {
                        obj.entry(k.clone()).or_insert(v.clone());
                    }
                }
            }
            ResponseTemplate::new(201).set_body_json(body)
        }
    };
    for p in ["/datasources/ds", "/indexes/idx", "/indexers/idxr"] {
        Mock::given(method("PUT"))
            .and(path(p.to_string()))
            .respond_with(put_recorder(json!({"serverDefault": true})))
            .mount(&server)
            .await;
    }

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "indexers",
        "idxr",
        &json!({"name": "idxr", "dataSourceName": "ds", "targetIndexName": "idx"}),
    );
    write_resource(
        ws.path(),
        "data-sources",
        "ds",
        &json!({"name": "ds", "type": "azureblob", "credentials": {"connectionString": "ResourceId=/subscriptions/x;"}, "container": {"name": "c"}}),
    );
    write_resource(
        ws.path(),
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );

    rigg(ws.path())
        .args(["push", "demo", "--yes"])
        .assert()
        .success();

    // ordering: indexer PUT must come after ds and idx PUTs
    let requests = server.received_requests().await.unwrap();
    let puts: Vec<&str> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .map(|r| r.url.path())
        .collect();
    let pos = |p: &str| puts.iter().position(|x| *x == p).unwrap_or(usize::MAX);
    assert!(
        pos("/datasources/ds") < pos("/indexers/idxr"),
        "puts: {puts:?}"
    );
    assert!(
        pos("/indexes/idx") < pos("/indexers/idxr"),
        "puts: {puts:?}"
    );

    // canonicalization: server-added default written back to disk, etag not
    let idx_file: Value = serde_json::from_str(
        &std::fs::read_to_string(
            ws.path()
                .join("projects/demo/envs/dev/search/indexes/idx.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(idx_file["serverDefault"], json!(true));
    assert!(idx_file.get("@odata.etag").is_none());
}

#[tokio::test]
async fn cloud_mutations_show_target_service_and_url() {
    // Issue #3: push/pull/delete must name the actual Azure target (service
    // and resolved base URL) so the user can verify where changes go.
    let server = MockServer::start().await;
    mock_empty_lists(&server).await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );

    let banner = format!("Search:  mock → {}", server.uri());
    rigg(ws.path())
        .args(["push", "demo", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&banner));
    rigg(ws.path())
        .args(["pull", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains(&banner));
}

#[tokio::test]
async fn push_dry_run_sends_nothing_and_prune_deletes() {
    let server = MockServer::start().await;
    mock_empty_lists(&server).await;
    Mock::given(method("GET"))
        .and(path("/indexes/ghost"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"name": "ghost", "fields": []})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/indexes/ghost"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    // baseline says we own "ghost", but the local file is gone → orphan
    std::fs::create_dir_all(ws.path().join(".rigg/dev/demo")).unwrap();
    std::fs::write(
        ws.path().join(".rigg/dev/demo/state.json"),
        json!({"baselines": {"indexes/ghost": "deadbeef"}}).to_string(),
    )
    .unwrap();

    // dry run: reports the orphan, no DELETE sent
    rigg(ws.path())
        .args(["push", "demo", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("ghost"));
    let deletes = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .count();
    assert_eq!(deletes, 0, "dry run must not delete");

    // without --prune: orphan reported, still no delete
    rigg(ws.path())
        .args(["push", "demo", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--prune"));

    // with --prune: DELETE issued
    rigg(ws.path())
        .args(["push", "demo", "--yes", "--prune"])
        .assert()
        .success();
    let deletes = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .count();
    assert_eq!(deletes, 1);
}

#[tokio::test]
async fn conflict_fails_non_interactive_with_exit_5() {
    let server = MockServer::start().await;
    mock_empty_lists(&server).await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "idx",
            "fields": [{"name": "remote-change", "type": "Edm.String", "key": true}]
        })))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "local-change", "type": "Edm.String", "key": true}]}),
    );
    // baseline differs from both local and remote → conflict
    std::fs::create_dir_all(ws.path().join(".rigg/dev/demo")).unwrap();
    std::fs::write(
        ws.path().join(".rigg/dev/demo/state.json"),
        json!({"baselines": {"indexes/idx": "0000000000000000"}}).to_string(),
    )
    .unwrap();

    rigg(ws.path())
        .args(["push", "demo"])
        .assert()
        .code(5)
        .stdout(predicate::str::contains("conflict"));
    // nothing was pushed
    let puts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .count();
    assert_eq!(puts, 0);
}

#[tokio::test]
async fn pull_conflict_fails_non_interactive_with_rigg_diff_pointer() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "idx",
                "fields": [{"name": "remote-change", "type": "Edm.String", "key": true}]
            }]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "local-change", "type": "Edm.String", "key": true}]}),
    );
    // baseline differs from both local and remote → conflict
    std::fs::create_dir_all(ws.path().join(".rigg/dev/demo")).unwrap();
    std::fs::write(
        ws.path().join(".rigg/dev/demo/state.json"),
        json!({"baselines": {"indexes/idx": "0000000000000000"}}).to_string(),
    )
    .unwrap();

    rigg(ws.path())
        .args(["pull", "demo"])
        .assert()
        .code(5)
        .stdout(predicate::str::contains("conflict"))
        .stdout(predicate::str::contains("rigg diff"));
    // local file left untouched
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string(
            ws.path()
                .join("projects/demo/envs/dev/search/indexes/idx.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(v["fields"][0]["name"], "local-change");
}

#[tokio::test]
async fn diff_reports_drift_with_exit_code_and_markdown() {
    let server = MockServer::start().await;
    mock_empty_lists(&server).await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "@odata.etag": "\"0x1\"",
            "name": "idx",
            "fields": [{"name": "id", "type": "Edm.String", "key": true}]
        })))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    // identical local (etag noise only) → clean diff, exit 0
    write_resource(
        ws.path(),
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );
    rigg(ws.path())
        .args(["diff", "demo", "--exit-code"])
        .assert()
        .success()
        .stdout(predicate::str::contains("hint:").not());

    // local edit → drift, exit 5, markdown mentions the resource
    write_resource(
        ws.path(),
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}, {"name": "extra", "type": "Edm.String"}]}),
    );
    rigg(ws.path())
        .args(["diff", "demo", "--exit-code", "--format", "markdown"])
        .assert()
        .code(5)
        .stdout(predicate::str::contains("indexes/idx"))
        .stdout(predicate::str::contains("| field | local |"))
        .stdout(predicate::str::contains("hint:").not());

    // same drift, default text format → dual-direction hint naming the project
    rigg(ws.path())
        .args(["diff", "demo", "--exit-code"])
        .assert()
        .code(5)
        .stdout(predicate::str::contains("hint: rigg pull demo"))
        .stdout(predicate::str::contains(
            "update local files to match Azure",
        ))
        .stdout(predicate::str::contains("rigg push demo"))
        .stdout(predicate::str::contains(
            "make Azure match your local files",
        ));
}

#[tokio::test]
async fn status_classifies_and_reports_unmanaged() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "mine", "fields": []},
                {"name": "somebody-elses", "fields": []}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/indexes/mine"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"name": "mine", "fields": []})),
        )
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "indexes",
        "mine",
        &json!({"name": "mine", "fields": []}),
    );

    rigg(ws.path())
        .args(["status", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("in sync"))
        .stdout(predicate::str::contains("somebody-elses"))
        .stdout(predicate::str::contains("unmanaged"));
}

#[tokio::test]
async fn adopt_named_selector_adopts_only_that_resource() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "hotels", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "cars",   "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    rigg(ws.path())
        .args(["adopt", "demo", "indexes/hotels"])
        .assert()
        .success();

    assert!(
        ws.path()
            .join("projects/demo/envs/dev/search/indexes/hotels.json")
            .exists()
    );
    assert!(
        !ws.path()
            .join("projects/demo/envs/dev/search/indexes/cars.json")
            .exists(),
        "only the named resource is adopted"
    );
}

#[tokio::test]
async fn adopt_kind_selector_needs_confirmation_and_yes_adopts_all_of_kind() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "hotels", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "cars",   "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    // broad selector, non-interactive (assert_cmd has no tty), no --yes → exit 2
    rigg(ws.path())
        .args(["adopt", "demo", "indexes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--yes").or(predicate::str::contains("--dry-run")));
    assert!(
        !ws.path()
            .join("projects/demo/envs/dev/search/indexes/hotels.json")
            .exists()
    );

    // with --yes → adopts all of the kind
    rigg(ws.path())
        .args(["adopt", "demo", "indexes", "--yes"])
        .assert()
        .success();
    assert!(
        ws.path()
            .join("projects/demo/envs/dev/search/indexes/hotels.json")
            .exists()
    );
    assert!(
        ws.path()
            .join("projects/demo/envs/dev/search/indexes/cars.json")
            .exists()
    );
}

#[tokio::test]
async fn adopt_all_selector_adopts_everything_unmanaged() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "a", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "b", "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    rigg(ws.path())
        .args(["adopt", "demo", "all", "--yes"])
        .assert()
        .success();

    assert!(
        ws.path()
            .join("projects/demo/envs/dev/search/indexes/a.json")
            .exists()
    );
    assert!(
        ws.path()
            .join("projects/demo/envs/dev/search/indexes/b.json")
            .exists()
    );
}

#[tokio::test]
async fn adopt_dry_run_writes_nothing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name":"hotels","fields":[{"name":"id","type":"Edm.String","key":true}]}]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    rigg(ws.path())
        .args(["adopt", "demo", "indexes/hotels", "--dry-run"])
        .assert()
        .success();
    assert!(
        !ws.path()
            .join("projects/demo/envs/dev/search/indexes/hotels.json")
            .exists()
    );
}

#[tokio::test]
async fn adopt_never_steals_another_projects_resource() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "hotels", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "cars",   "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    // Second project "other" already owns "hotels".
    let other_indexes = ws.path().join("projects/other/envs/dev/search/indexes");
    std::fs::create_dir_all(&other_indexes).unwrap();
    std::fs::write(ws.path().join("projects/other/project.yaml"), "{}\n").unwrap();
    std::fs::write(
        other_indexes.join("hotels.json"),
        serde_json::to_string_pretty(&json!({
            "name": "hotels",
            "fields": [{"name":"id","type":"Edm.String","key":true}]
        }))
        .unwrap(),
    )
    .unwrap();

    // Explicitly naming another project's resource is a hard error (exit 1).
    rigg(ws.path())
        .args(["adopt", "demo", "indexes/hotels"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("owned by project 'other'"));
    assert!(
        !ws.path()
            .join("projects/demo/envs/dev/search/indexes/hotels.json")
            .exists()
    );

    // A broad selector sweeps in only the unowned resource, silently skipping the owned one.
    rigg(ws.path())
        .args(["adopt", "demo", "indexes", "--yes"])
        .assert()
        .success();
    assert!(
        ws.path()
            .join("projects/demo/envs/dev/search/indexes/cars.json")
            .exists()
    );
    assert!(
        !ws.path()
            .join("projects/demo/envs/dev/search/indexes/hotels.json")
            .exists(),
        "hotels is owned by 'other' and must not be adopted into 'demo'"
    );
}

#[tokio::test]
async fn adopt_json_output_lists_adopted() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name":"hotels","fields":[{"name":"id","type":"Edm.String","key":true}]}]
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;
    let ws = workspace(&server.uri());

    let output = rigg(ws.path())
        .args(["adopt", "demo", "indexes/hotels", "--output", "json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let value: Value = serde_json::from_slice(&output).expect("stdout is valid JSON");
    let adopted = value["adopted"].as_array().expect("adopted is an array");
    assert!(
        adopted.iter().any(|v| v.as_str() == Some("indexes/hotels")),
        "adopted should contain 'indexes/hotels': {value:?}"
    );
    assert!(
        value.get("skipped").is_some(),
        "skipped key present: {value:?}"
    );
}

#[tokio::test]
async fn adopt_with_deps_pulls_upstream_chain_only() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "docs-indexer",
                "dataSourceName": "docs-ds",
                "targetIndexName": "docs-index"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "docs-index", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "unrelated",  "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/datasources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs-ds", "type": "azureblob",
                       "container": {"name": "c"},
                       "credentials": {"connectionString": "ResourceId=/x;"}}]
        })))
        .mount(&server)
        .await;
    // remaining kinds empty
    for p in [
        "skillsets",
        "synonymmaps",
        "aliases",
        "knowledgeSources",
        "knowledgeBases",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
    }

    let ws = workspace(&server.uri());
    rigg(ws.path())
        .args(["adopt", "demo", "indexers/docs-indexer", "--with-deps"])
        .assert()
        .success();

    let base = ws.path().join("projects/demo/envs/dev/search");
    assert!(
        base.join("indexers/docs-indexer.json").exists(),
        "the named resource"
    );
    assert!(
        base.join("indexes/docs-index.json").exists(),
        "referenced index (dependency)"
    );
    assert!(
        base.join("data-sources/docs-ds.json").exists(),
        "referenced data source (dependency)"
    );
    assert!(
        !base.join("indexes/unrelated.json").exists(),
        "unrelated resource NOT adopted"
    );
}

#[tokio::test]
async fn adopt_with_deps_on_owned_resource_adopts_missing_deps() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexers"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "docs-indexer",
                "dataSourceName": "docs-ds",
                "targetIndexName": "docs-index"
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs-index", "fields": [{"name":"id","type":"Edm.String","key":true}]}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/datasources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{"name": "docs-ds", "type": "azureblob",
                       "container": {"name": "c"},
                       "credentials": {"connectionString": "ResourceId=/x;"}}]
        })))
        .mount(&server)
        .await;
    for p in [
        "skillsets",
        "synonymmaps",
        "aliases",
        "knowledgeSources",
        "knowledgeBases",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
    }

    let ws = workspace(&server.uri());
    // First: adopt ONLY the indexer (no deps).
    rigg(ws.path())
        .args(["adopt", "demo", "indexers/docs-indexer"])
        .assert()
        .success();
    let base = ws.path().join("projects/demo/envs/dev/search");
    assert!(base.join("indexers/docs-indexer.json").exists());
    assert!(
        !base.join("indexes/docs-index.json").exists(),
        "deps not adopted yet"
    );

    // Change of mind: same command with --with-deps must now adopt the missing deps.
    rigg(ws.path())
        .args(["adopt", "demo", "indexers/docs-indexer", "--with-deps"])
        .assert()
        .success()
        .stdout(predicate::str::contains("indexes/docs-index"))
        .stdout(predicate::str::contains("already managed"));
    assert!(
        base.join("indexes/docs-index.json").exists(),
        "index adopted as dep of owned seed"
    );
    assert!(
        base.join("data-sources/docs-ds.json").exists(),
        "data source adopted as dep of owned seed"
    );
}

#[tokio::test]
async fn auto_created_subresources_are_not_adoptable() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/knowledgeSources"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "regulatory",
                "kind": "azureBlob",
                "azureBlobParameters": {
                    "containerName": "c",
                    "createdResources": {"index": "regulatory-index"}
                }
            }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/indexes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [
                {"name": "regulatory-index", "fields": [{"name":"id","type":"Edm.String","key":true}]},
                {"name": "normal-index", "fields": [{"name":"id","type":"Edm.String","key":true}]}
            ]
        })))
        .mount(&server)
        .await;
    for p in [
        "datasources",
        "skillsets",
        "indexers",
        "synonymmaps",
        "aliases",
        "knowledgeBases",
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/{p}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"value": []})))
            .mount(&server)
            .await;
    }
    let ws = workspace(&server.uri());

    // kind sweep: auto-created index silently skipped, normal index adopted
    rigg(ws.path())
        .args(["adopt", "demo", "indexes", "--yes"])
        .assert()
        .success();
    let base = ws.path().join("projects/demo/envs/dev/search");
    assert!(base.join("indexes/normal-index.json").exists());
    assert!(
        !base.join("indexes/regulatory-index.json").exists(),
        "auto-created not swept"
    );

    // explicit: reasoned skip naming the KS
    rigg(ws.path())
        .args(["adopt", "demo", "indexes/regulatory-index"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "auto-created by knowledge source 'regulatory'",
        ));
    assert!(!base.join("indexes/regulatory-index.json").exists());

    // status: not listed as unmanaged (knowledge source itself + nothing else)
    rigg(ws.path())
        .args(["status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("regulatory-index").not());
}

#[tokio::test]
async fn protected_env_push_blocks_non_interactive_without_confirm_env() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(
        ws.path(),
        "prod",
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );

    rigg(ws.path())
        .args(["push", "demo", "-e", "prod", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--confirm-env prod"));

    let puts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .count();
    assert_eq!(
        puts, 0,
        "protected env must not be mutated without confirmation"
    );
}

#[tokio::test]
async fn protected_env_push_succeeds_with_confirm_env() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;
    Mock::given(method("PUT"))
        .and(path("/indexes/idx"))
        .respond_with(|req: &Request| {
            let mut body: Value = serde_json::from_slice(&req.body).unwrap();
            if let Some(obj) = body.as_object_mut() {
                obj.insert("@odata.etag".into(), json!("\"0xNEW\""));
            }
            ResponseTemplate::new(201).set_body_json(body)
        })
        .mount(&server)
        .await;

    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(
        ws.path(),
        "prod",
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );

    rigg(ws.path())
        .args([
            "push",
            "demo",
            "-e",
            "prod",
            "--yes",
            "--confirm-env",
            "prod",
        ])
        .assert()
        .success();

    let puts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .count();
    assert_eq!(puts, 1);

    // canonicalized to the prod tree, not dev
    assert!(
        ws.path()
            .join("projects/demo/envs/prod/search/indexes/idx.json")
            .exists()
    );
}

#[tokio::test]
async fn protected_env_push_dry_run_is_ungated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(
        ws.path(),
        "prod",
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );

    rigg(ws.path())
        .args(["push", "demo", "-e", "prod", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("idx"));

    let puts = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "PUT")
        .count();
    assert_eq!(puts, 0, "dry run must never mutate, gated or not");
}

#[tokio::test]
async fn protected_env_delete_blocks_non_interactive_without_confirm_env() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"name": "idx", "fields": []})),
        )
        .mount(&server)
        .await;

    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(
        ws.path(),
        "prod",
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": []}),
    );

    rigg(ws.path())
        .args(["delete", "demo", "--remote", "-e", "prod", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--confirm-env prod"));

    let deletes = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .count();
    assert_eq!(
        deletes, 0,
        "protected env must not be deleted without confirmation"
    );
}

#[tokio::test]
async fn protected_env_delete_succeeds_with_confirm_env() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/indexes/idx"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"name": "idx", "fields": []})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/indexes/idx"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let ws = workspace_with_protected_prod(&server.uri());
    write_resource_env(
        ws.path(),
        "prod",
        "indexes",
        "idx",
        &json!({"name": "idx", "fields": []}),
    );

    rigg(ws.path())
        .args([
            "delete",
            "demo",
            "--remote",
            "-e",
            "prod",
            "--yes",
            "--confirm-env",
            "prod",
        ])
        .assert()
        .success();

    let deletes = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() == "DELETE")
        .count();
    assert_eq!(deletes, 1);
}

// ---------------------------------------------------------------------------
// rigg migrate knowledge-source
// ---------------------------------------------------------------------------

/// An azureBlob knowledge source with the live nested createdResources shape.
fn blob_ks_doc(name: &str) -> Value {
    json!({
        "@odata.etag": "\"0xKS\"",
        "name": name,
        "kind": "azureBlob",
        "description": "Test knowledge source.",
        "azureBlobParameters": {
            "connectionString": null,
            "containerName": "docs",
            "createdResources": {
                "datasource": format!("{name}-datasource"),
                "indexer": format!("{name}-indexer"),
                "skillset": format!("{name}-skillset"),
                "index": format!("{name}-index")
            }
        }
    })
}

/// Mount GETs for a blob KS and its four generated sub-resources.
async fn mount_blob_ks(server: &MockServer, name: &str) {
    Mock::given(method("GET"))
        .and(path(format!("/knowledgeSources/{name}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(blob_ks_doc(name)))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/datasources/{name}-datasource")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "@odata.etag": "\"0xDS\"",
            "name": format!("{name}-datasource"),
            "type": "azureblob",
            "credentials": {"connectionString": null},
            "container": {"name": "docs"}
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/indexes/{name}-index")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "@odata.etag": "\"0xIDX\"",
            "name": format!("{name}-index"),
            "fields": [{"name": "id", "type": "Edm.String", "key": true}]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/skillsets/{name}-skillset")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "@odata.etag": "\"0xSS\"",
            "name": format!("{name}-skillset"),
            "skills": []
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/indexers/{name}-indexer")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "@odata.etag": "\"0xIXR\"",
            "name": format!("{name}-indexer"),
            "status": "running",
            "dataSourceName": format!("{name}-datasource"),
            "targetIndexName": format!("{name}-index"),
            "skillsetName": format!("{name}-skillset")
        })))
        .mount(server)
        .await;
}

fn read_json(ws: &std::path::Path, rel: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(ws.join(rel)).unwrap()).unwrap()
}

#[tokio::test]
async fn migrate_in_place_writes_explicit_pipeline() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;

    let ws = workspace(&server.uri());
    // The KS must be project-managed: its (pulled, normalized) file exists.
    write_resource(
        ws.path(),
        "knowledge-sources",
        "test-ks",
        &json!({"name": "test-ks", "kind": "azureBlob",
                "azureBlobParameters": {"containerName": "docs"}}),
    );

    rigg(ws.path())
        .args([
            "migrate",
            "knowledge-source",
            "test-ks",
            "--in-place",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("REPLACE"));

    let base = "projects/demo/envs/dev/search";
    let ds = read_json(
        ws.path(),
        &format!("{base}/data-sources/test-ks-datasource.json"),
    );
    assert_eq!(ds["name"], "test-ks-datasource");
    assert!(ds.get("@odata.etag").is_none(), "volatile stripped");
    let idxr = read_json(ws.path(), &format!("{base}/indexers/test-ks-indexer.json"));
    assert!(idxr.get("status").is_none(), "read-only stripped");
    read_json(ws.path(), &format!("{base}/indexes/test-ks-index.json"));
    read_json(
        ws.path(),
        &format!("{base}/skillsets/test-ks-skillset.json"),
    );

    let ks = read_json(ws.path(), &format!("{base}/knowledge-sources/test-ks.json"));
    assert_eq!(ks["kind"], "searchIndex");
    assert_eq!(
        ks["searchIndexParameters"]["searchIndexName"],
        "test-ks-index"
    );
    assert!(ks.get("azureBlobParameters").is_none());
}

#[tokio::test]
async fn migrate_side_by_side_creates_new_files_and_keeps_old() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    // New names must not exist remotely.
    for p in [
        "/knowledgeSources/test-ks2",
        "/datasources/test-ks2-datasource",
        "/indexes/test-ks2-index",
        "/skillsets/test-ks2-skillset",
        "/indexers/test-ks2-indexer",
    ] {
        Mock::given(method("GET"))
            .and(path(p.to_string()))
            .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
            .mount(&server)
            .await;
    }

    let ws = workspace(&server.uri());
    let old_ks = json!({"name": "test-ks", "kind": "azureBlob",
                        "azureBlobParameters": {"containerName": "docs"}});
    write_resource(ws.path(), "knowledge-sources", "test-ks", &old_ks);

    rigg(ws.path())
        .args([
            "migrate",
            "knowledge-source",
            "test-ks",
            "--rename",
            "test-ks2",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Next steps"));

    let base = "projects/demo/envs/dev/search";
    let ks2 = read_json(
        ws.path(),
        &format!("{base}/knowledge-sources/test-ks2.json"),
    );
    assert_eq!(ks2["kind"], "searchIndex");
    assert_eq!(
        ks2["searchIndexParameters"]["searchIndexName"],
        "test-ks2-index"
    );
    let idxr = read_json(ws.path(), &format!("{base}/indexers/test-ks2-indexer.json"));
    assert_eq!(idxr["dataSourceName"], "test-ks2-datasource");
    assert_eq!(idxr["targetIndexName"], "test-ks2-index");
    assert_eq!(idxr["skillsetName"], "test-ks2-skillset");

    // old KS file untouched
    let old = read_json(ws.path(), &format!("{base}/knowledge-sources/test-ks.json"));
    assert_eq!(old, old_ks);
}

#[tokio::test]
async fn migrate_side_by_side_rejects_remote_name_collision() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    Mock::given(method("GET"))
        .and(path("/knowledgeSources/test-ks2"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;
    // one derived name collides remotely
    Mock::given(method("GET"))
        .and(path("/datasources/test-ks2-datasource"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"name": "test-ks2-datasource", "type": "azureblob"})),
        )
        .mount(&server)
        .await;
    for p in [
        "/indexes/test-ks2-index",
        "/skillsets/test-ks2-skillset",
        "/indexers/test-ks2-indexer",
    ] {
        Mock::given(method("GET"))
            .and(path(p.to_string()))
            .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
            .mount(&server)
            .await;
    }

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "knowledge-sources",
        "test-ks",
        &json!({"name": "test-ks", "kind": "azureBlob"}),
    );

    rigg(ws.path())
        .args([
            "migrate",
            "knowledge-source",
            "test-ks",
            "--rename",
            "test-ks2",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists remotely"));
}

#[tokio::test]
async fn migrate_rejects_unmanaged_and_remote_kinds() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    Mock::given(method("GET"))
        .and(path("/knowledgeSources/web-ks"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"name": "web-ks", "kind": "web", "webParameters": {}})),
        )
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    // unmanaged: no local file, no baseline
    rigg(ws.path())
        .args([
            "migrate",
            "knowledge-source",
            "test-ks",
            "--in-place",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("adopt"));

    // remote kind: nothing to migrate
    write_resource(
        ws.path(),
        "knowledge-sources",
        "web-ks",
        &json!({"name": "web-ks", "kind": "web"}),
    );
    rigg(ws.path())
        .args([
            "migrate",
            "knowledge-source",
            "web-ks",
            "--in-place",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no Azure-generated pipeline"));
}

#[tokio::test]
async fn migrate_requires_mode_non_interactively() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "knowledge-sources",
        "test-ks",
        &json!({"name": "test-ks", "kind": "azureBlob"}),
    );
    rigg(ws.path())
        .args(["migrate", "knowledge-source", "test-ks"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--in-place or --rename"));
}

// ---------------------------------------------------------------------------
// push replace (knowledge-source kind change)
// ---------------------------------------------------------------------------

/// Local project files for an in-place-migrated KS: searchIndex KS + the
/// four explicit sub-resources under the generated names.
fn write_migrated_files(ws: &std::path::Path, name: &str) {
    write_resource(
        ws,
        "knowledge-sources",
        name,
        &json!({"name": name, "kind": "searchIndex",
                "description": "Test knowledge source.",
                "searchIndexParameters": {"searchIndexName": format!("{name}-index")}}),
    );
    write_resource(
        ws,
        "data-sources",
        &format!("{name}-datasource"),
        &json!({"name": format!("{name}-datasource"), "type": "azureblob",
                "credentials": {"connectionString": "ResourceId=/subscriptions/x;"},
                "container": {"name": "docs"}}),
    );
    write_resource(
        ws,
        "indexes",
        &format!("{name}-index"),
        &json!({"name": format!("{name}-index"),
                "fields": [{"name": "id", "type": "Edm.String", "key": true}]}),
    );
    write_resource(
        ws,
        "skillsets",
        &format!("{name}-skillset"),
        &json!({"name": format!("{name}-skillset"), "skills": []}),
    );
    write_resource(
        ws,
        "indexers",
        &format!("{name}-indexer"),
        &json!({"name": format!("{name}-indexer"),
                "dataSourceName": format!("{name}-datasource"),
                "targetIndexName": format!("{name}-index"),
                "skillsetName": format!("{name}-skillset")}),
    );
}

/// Echo PUT (201) for a path.
async fn mount_put_echo(server: &MockServer, p: &str) {
    Mock::given(method("PUT"))
        .and(path(p.to_string()))
        .respond_with(|req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            ResponseTemplate::new(201).set_body_json(body)
        })
        .mount(server)
        .await;
}

#[tokio::test]
async fn push_detects_kind_change_as_replace_dry_run() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await; // remote is still azureBlob
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    write_migrated_files(ws.path(), "test-ks");

    rigg(ws.path())
        .args(["push", "demo", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("replace"))
        .stdout(predicate::str::contains("kind: azureBlob → searchIndex"))
        .stdout(predicate::str::contains("REBUILT"))
        .stdout(predicate::str::contains("recreates:"));

    let mutations = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .count();
    assert_eq!(mutations, 0, "dry run must not mutate");
}

#[tokio::test]
async fn push_replace_requires_allow_replace_flag() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    write_migrated_files(ws.path(), "test-ks");

    rigg(ws.path())
        .args(["push", "demo", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--allow-replace"));

    let mutations = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .count();
    assert_eq!(mutations, 0, "gated push must not mutate");
}

#[tokio::test]
async fn push_replace_full_choreography() {
    let server = MockServer::start().await;
    // Foreign KB referencing the KS (and another, so unlink stays non-empty).
    Mock::given(method("GET"))
        .and(path("/knowledgeBases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "@odata.etag": "\"0xKB\"",
                "name": "kb1",
                "knowledgeSources": [{"name": "test-ks"}, {"name": "other-ks"}]
            }]
        })))
        .mount(&server)
        .await;
    mount_blob_ks(&server, "test-ks").await;
    mock_empty_lists(&server).await;
    mount_put_echo(&server, "/knowledgeBases/kb1").await;
    Mock::given(method("DELETE"))
        .and(path("/knowledgeSources/test-ks"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;
    for p in [
        "/datasources/test-ks-datasource",
        "/indexes/test-ks-index",
        "/skillsets/test-ks-skillset",
        "/indexers/test-ks-indexer",
        "/knowledgeSources/test-ks",
    ] {
        mount_put_echo(&server, p).await;
    }

    let ws = workspace(&server.uri());
    write_migrated_files(ws.path(), "test-ks");

    rigg(ws.path())
        .args(["push", "demo", "--yes", "--allow-replace"])
        .assert()
        .success()
        .stdout(predicate::str::contains("foreign knowledge base 'kb1'"))
        .stdout(predicate::str::contains("repopulating"));

    // Exact mutation order.
    let requests = server.received_requests().await.unwrap();
    let muts: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect();
    assert_eq!(
        muts,
        vec![
            "PUT /knowledgeBases/kb1",             // unlink
            "DELETE /knowledgeSources/test-ks",    // cascade delete
            "PUT /datasources/test-ks-datasource", // recreate in graph order
            "PUT /indexes/test-ks-index",
            "PUT /skillsets/test-ks-skillset",
            "PUT /indexers/test-ks-indexer",
            "PUT /knowledgeSources/test-ks", // new searchIndex KS
            "PUT /knowledgeBases/kb1",       // relink
        ],
        "unexpected order: {muts:?}"
    );

    // Unlink removed only the replaced KS; relink restored the original.
    let kb_puts: Vec<Value> = requests
        .iter()
        .filter(|r| r.method.as_str() == "PUT" && r.url.path() == "/knowledgeBases/kb1")
        .map(|r| serde_json::from_slice(&r.body).unwrap())
        .collect();
    assert_eq!(
        kb_puts[0]["knowledgeSources"],
        json!([{"name": "other-ks"}])
    );
    assert_eq!(
        kb_puts[1]["knowledgeSources"],
        json!([{"name": "test-ks"}, {"name": "other-ks"}])
    );

    // Recovery file removed on success.
    assert!(
        !ws.path()
            .join(".rigg/dev/demo/replace-test-ks.json")
            .exists()
    );
}

#[tokio::test]
async fn push_replace_empty_kb_falls_back_to_delete() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/knowledgeBases"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "value": [{
                "name": "kb1",
                "knowledgeSources": [{"name": "test-ks"}]
            }]
        })))
        .mount(&server)
        .await;
    mount_blob_ks(&server, "test-ks").await;
    mock_empty_lists(&server).await;
    // PUT with an empty knowledgeSources list is rejected by the service.
    Mock::given(method("PUT"))
        .and(path("/knowledgeBases/kb1"))
        .respond_with(|req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            let empty = body["knowledgeSources"]
                .as_array()
                .map(|a| a.is_empty())
                .unwrap_or(true);
            if empty {
                ResponseTemplate::new(400).set_body_json(
                    json!({"error": {"message": "knowledgeSources must not be empty"}}),
                )
            } else {
                ResponseTemplate::new(201).set_body_json(body)
            }
        })
        .mount(&server)
        .await;
    for p in ["/knowledgeBases/kb1", "/knowledgeSources/test-ks"] {
        Mock::given(method("DELETE"))
            .and(path(p.to_string()))
            .respond_with(ResponseTemplate::new(204))
            .mount(&server)
            .await;
    }
    for p in [
        "/datasources/test-ks-datasource",
        "/indexes/test-ks-index",
        "/skillsets/test-ks-skillset",
        "/indexers/test-ks-indexer",
        "/knowledgeSources/test-ks",
    ] {
        mount_put_echo(&server, p).await;
    }

    let ws = workspace(&server.uri());
    write_migrated_files(ws.path(), "test-ks");

    rigg(ws.path())
        .args(["push", "demo", "--yes", "--allow-replace"])
        .assert()
        .success()
        .stdout(predicate::str::contains("deleted knowledge-bases/kb1"));

    let requests = server.received_requests().await.unwrap();
    let muts: Vec<String> = requests
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .map(|r| format!("{} {}", r.method, r.url.path()))
        .collect();
    // rejected empty PUT → DELETE kb → ... → final PUT recreates kb
    assert_eq!(muts[0], "PUT /knowledgeBases/kb1");
    assert_eq!(muts[1], "DELETE /knowledgeBases/kb1");
    assert_eq!(muts[2], "DELETE /knowledgeSources/test-ks");
    assert_eq!(muts.last().unwrap(), "PUT /knowledgeBases/kb1");
    // the final PUT restores the original reference list
    let last_kb: Value = serde_json::from_slice(
        &requests
            .iter()
            .rfind(|r| r.method.as_str() == "PUT" && r.url.path() == "/knowledgeBases/kb1")
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(last_kb["knowledgeSources"], json!([{"name": "test-ks"}]));
}

#[tokio::test]
async fn push_resumes_interrupted_replace_relink() {
    let server = MockServer::start().await;
    // Remote KS already replaced (searchIndex) — only the relink is pending.
    Mock::given(method("GET"))
        .and(path("/knowledgeSources/test-ks"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "name": "test-ks", "kind": "searchIndex",
            "searchIndexParameters": {"searchIndexName": "test-ks-index"}
        })))
        .mount(&server)
        .await;
    mock_empty_lists(&server).await;
    mount_put_echo(&server, "/knowledgeBases/kb1").await;

    let ws = workspace(&server.uri());
    let state_dir = ws.path().join(".rigg/dev/demo");
    std::fs::create_dir_all(&state_dir).unwrap();
    std::fs::write(
        state_dir.join("replace-test-ks.json"),
        serde_json::to_string_pretty(&json!({
            "ks": "test-ks",
            "knowledge_bases": [{
                "name": "kb1",
                "knowledgeSources": [{"name": "test-ks"}]
            }]
        }))
        .unwrap(),
    )
    .unwrap();

    rigg(ws.path())
        .args(["push", "demo", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("interrupted replace"))
        .stdout(predicate::str::contains("restored 1 knowledge base link"));

    let requests = server.received_requests().await.unwrap();
    let kb_put = requests
        .iter()
        .find(|r| r.method.as_str() == "PUT" && r.url.path() == "/knowledgeBases/kb1");
    assert!(kb_put.is_some(), "relink PUT sent");
    assert!(!state_dir.join("replace-test-ks.json").exists());
}

#[tokio::test]
async fn diff_notes_immutable_kind_change_as_replace() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    write_migrated_files(ws.path(), "test-ks");

    rigg(ws.path())
        .args(["diff", "demo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("'kind' is immutable"))
        .stdout(predicate::str::contains("REPLACE"));
}

#[tokio::test]
async fn push_refuses_creating_datasource_without_credentials() {
    let server = MockServer::start().await;
    mock_empty_lists(&server).await;
    Mock::given(method("GET"))
        .and(path("/datasources/nocreds"))
        .respond_with(ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;

    let ws = workspace(&server.uri());
    write_resource(
        ws.path(),
        "data-sources",
        "nocreds",
        &json!({"name": "nocreds", "type": "azureblob",
                "credentials": {"connectionString": null},
                "container": {"name": "docs"}}),
    );

    rigg(ws.path())
        .args(["push", "demo", "--yes"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no credentials.connectionString"));

    let mutations = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .count();
    assert_eq!(mutations, 0, "must fail before any mutation");
}

#[tokio::test]
async fn push_replace_refuses_without_datasource_credentials_before_destroying() {
    let server = MockServer::start().await;
    mount_blob_ks(&server, "test-ks").await;
    mock_empty_lists(&server).await;

    let ws = workspace(&server.uri());
    write_migrated_files(ws.path(), "test-ks");
    // strip the credentials from the migrated data source
    write_resource(
        ws.path(),
        "data-sources",
        "test-ks-datasource",
        &json!({"name": "test-ks-datasource", "type": "azureblob",
                "credentials": {"connectionString": null},
                "container": {"name": "docs"}}),
    );

    rigg(ws.path())
        .args(["push", "demo", "--yes", "--allow-replace"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("no credentials.connectionString"));

    // Crucially: the old knowledge source and its pipeline are untouched.
    let mutations = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.method.as_str() != "GET")
        .count();
    assert_eq!(mutations, 0, "nothing may be unlinked or deleted");
}
