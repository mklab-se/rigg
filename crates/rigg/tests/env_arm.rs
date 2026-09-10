//! `rigg env` against the ARM fake: `env show --refresh` resolution and the
//! binding cache it writes.

#[path = "arm_fake.rs"]
mod arm_fake;

use arm_fake::mount_arm_fake;
use assert_cmd::Command;
use predicates::prelude::*;
use wiremock::MockServer;

fn rigg(dir: &std::path::Path, arm: &str) -> Command {
    let mut cmd = Command::cargo_bin("rigg").expect("binary builds");
    cmd.env("RIGG_NO_UPDATE_CHECK", "1");
    cmd.env_remove("RIGG_ENV");
    cmd.env("RIGG_ARM_ENDPOINT", arm);
    cmd.env("RIGG_ACCESS_TOKEN", "t");
    cmd.current_dir(dir);
    cmd
}

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

#[tokio::test(flavor = "multi_thread")]
async fn env_show_refresh_resolves_bindings_against_arm() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &["sub-a"],
        &[
            ("storageAccounts", "devacct", "rg", "swedencentral"),
            ("searchServices", "unit-test-svc", "rg", "swedencentral"),
        ],
    )
    .await;
    let ws = workspace();
    rigg(ws.path(), &server.uri())
        .args(["env", "bind", "dev", "docs", "storage:devacct"])
        .assert()
        .success();
    rigg(ws.path(), &server.uri())
        .args(["env", "show", "dev", "--refresh"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "/subscriptions/sub-a/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/devacct",
        ));
    // The implicit `search` target is resolved and cached too — it is a
    // binding like any other, and every classification leans on it.
    let cache: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(ws.path().join(".rigg/dev/bindings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(cache["bindings"]["search"]["kind"], "search");
    assert_eq!(
        cache["bindings"]["search"]["arm_id"],
        "/subscriptions/sub-a/resourceGroups/rg/providers/Microsoft.Search/searchServices/unit-test-svc"
    );
    assert_eq!(cache["bindings"]["docs"]["name"], "docs");

    // the cached id is shown on later runs without --refresh
    rigg(ws.path(), &server.uri())
        .args(["env", "show", "dev"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "/subscriptions/sub-a/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/devacct",
        ));
}

#[tokio::test(flavor = "multi_thread")]
async fn env_show_refresh_marks_an_unresolvable_binding_and_still_succeeds() {
    let server = MockServer::start().await;
    mount_arm_fake(&server, &["sub-a"], &[]).await;
    let ws = workspace();
    rigg(ws.path(), &server.uri())
        .args(["env", "bind", "dev", "ghost", "storage:missingacct"])
        .assert()
        .success();
    rigg(ws.path(), &server.uri())
        .args(["env", "show", "dev", "--refresh"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("ghost").and(predicate::str::contains(
                "Resource not found: storage 'missingacct'",
            )),
        );
}

// ---------------------------------------------------------------------
// `rigg promote` — the online phase (Web API auth re-derivation, deployment
// availability and quota) against the ARM fake.
// ---------------------------------------------------------------------

/// Two environments, each with its own search service and Foundry account,
/// and a `demo` project.
fn promote_workspace() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("rigg.yaml"),
        "environments:\n  \
         dev:\n    default: true\n    search: { service: s-dev }\n    foundry: { account: fndr-dev, project: p }\n  \
         prod:\n    search: { service: s-prod }\n    foundry: { account: fndr-prod, project: p }\n",
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

/// A dev skillset whose Web API skill carries a redacted function key in its
/// URI — the carrier promote must strip and re-derive against the target.
fn write_keyed_skillset(ws: &std::path::Path, uri: &str) {
    write_json(
        &ws.join("projects/demo/envs/dev/search/skillsets/ss.json"),
        &serde_json::json!({
            "name": "ss",
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

/// Bind the same logical function app to a different site per environment.
fn bind_function_apps(ws: &std::path::Path, arm: &str) {
    for (env, site) in [("dev", "mklab-dev"), ("prod", "mklab-prod")] {
        rigg(ws, arm)
            .args(["env", "bind", env, "fn", &format!("function-app:{site}")])
            .assert()
            .success();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn promote_sets_entra_auth_when_target_function_app_has_easy_auth() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &["sub-a"],
        &[
            ("sites", "mklab-dev", "rg", "swedencentral"),
            ("sites", "mklab-prod", "rg", "swedencentral"),
        ],
    )
    .await;
    arm_fake::mount_easy_auth(&server, "mklab-prod", true, "client-1").await;

    let ws = promote_workspace();
    bind_function_apps(ws.path(), &server.uri());
    write_keyed_skillset(
        ws.path(),
        "https://mklab-dev.azurewebsites.net/api/enrich?code=<redacted>",
    );

    rigg(ws.path(), &server.uri())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Entra"));

    let prod = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/skillsets/ss.json"),
    );
    let skill = &prod["skills"][0];
    assert_eq!(
        skill["authResourceId"], "api://client-1",
        "Easy Auth on the target app is re-derived as authResourceId: {prod}"
    );
    let uri = skill["uri"].as_str().unwrap();
    assert!(
        uri.starts_with("https://mklab-prod.azurewebsites.net/") && !uri.contains("code="),
        "the uri is translated and keyless: {uri}"
    );
    assert!(
        skill.get("x-rigg-auth").is_none(),
        "no push-time key annotation when Entra auth is available: {prod}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn promote_keeps_function_key_annotation_when_easy_auth_is_off_and_source_used_a_key() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &["sub-a"],
        &[
            ("sites", "mklab-dev", "rg", "swedencentral"),
            ("sites", "mklab-prod", "rg", "swedencentral"),
        ],
    )
    .await;
    arm_fake::mount_easy_auth(&server, "mklab-prod", false, "").await;

    let ws = promote_workspace();
    bind_function_apps(ws.path(), &server.uri());
    write_keyed_skillset(
        ws.path(),
        "https://mklab-dev.azurewebsites.net/api/enrich?code=<redacted>",
    );

    rigg(ws.path(), &server.uri())
        .args(["promote", "demo", "--from", "dev", "--to", "prod", "--yes"])
        .assert()
        .success()
        .stdout(predicate::str::contains("function key"));

    let prod = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/skillsets/ss.json"),
    );
    let skill = &prod["skills"][0];
    assert_eq!(
        skill["x-rigg-auth"], "function-key",
        "no Entra auth on the target app, but dev used a key: {prod}"
    );
    assert!(
        skill["uri"]
            .as_str()
            .unwrap()
            .contains("mklab-prod.azurewebsites.net/api/enrich?code=<redacted>"),
        "the key carrier is the target app's, still redacted: {prod}"
    );
    assert!(skill.get("authResourceId").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn promote_asks_when_a_deployment_model_is_unavailable_in_the_target_region() {
    let server = MockServer::start().await;
    mount_arm_fake(
        &server,
        &["sub-a"],
        &[("accounts", "fndr-prod", "rg", "swedencentral")],
    )
    .await;
    arm_fake::mount_models(
        &server,
        "sub-a",
        "swedencentral",
        vec![serde_json::json!({
            "kind": "OpenAI",
            "model": {
                "format": "OpenAI",
                "name": "text-embedding-3-large",
                "version": "1",
                "skus": [{
                    "name": "GlobalStandard",
                    "usageName": "OpenAI.GlobalStandard.text-embedding-3-large"
                }]
            }
        })],
        vec![serde_json::json!({
            "name": {"value": "OpenAI.GlobalStandard.text-embedding-3-large"},
            "currentValue": 10.0,
            "limit": 100.0
        })],
    )
    .await;

    let ws = promote_workspace();
    let dev = ws.path().join("projects/demo/envs/dev/foundry/deployments");
    write_json(
        &dev.join("gpt5.json"),
        &serde_json::json!({
            "name": "gpt-5-mini",
            "sku": {"name": "GlobalStandard", "capacity": 50},
            "properties": {"model": {"format": "OpenAI", "name": "gpt-5-mini", "version": "2026-01-01"}}
        }),
    );
    write_json(
        &dev.join("emb.json"),
        &serde_json::json!({
            "name": "text-embedding-3-large",
            "sku": {"name": "GlobalStandard", "capacity": 10},
            "properties": {"model": {"format": "OpenAI", "name": "text-embedding-3-large", "version": "1"}}
        }),
    );

    // Unanswered, the unavailable model is a question — and nothing is written.
    let out = rigg(ws.path(), &server.uri())
        .args([
            "promote", "demo", "--from", "dev", "--to", "prod", "--yes", "--output", "json",
        ])
        .assert()
        .code(6);
    let doc: serde_json::Value = serde_json::from_slice(&out.get_output().stdout).unwrap();
    assert_eq!(
        doc["questions"][0]["id"], "promote.deployment.gpt5",
        "the unavailable model is asked about: {doc}"
    );
    let cands: Vec<&str> = doc["questions"][0]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["value"].as_str().unwrap())
        .collect();
    assert!(
        cands.contains(&"continue") && cands.contains(&"skip"),
        "candidates: {cands:?}"
    );
    assert!(
        !ws.path()
            .join("projects/demo/envs/prod/foundry/deployments/emb.json")
            .exists(),
        "an unanswered question writes nothing at all"
    );

    // Answered `skip`, the deployment is left out; the available one is written.
    rigg(ws.path(), &server.uri())
        .args([
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--answer",
            "promote.deployment.gpt5=skip",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("quota"));
    assert!(
        !ws.path()
            .join("projects/demo/envs/prod/foundry/deployments/gpt5.json")
            .exists(),
        "a skipped deployment is not written"
    );
    assert!(
        ws.path()
            .join("projects/demo/envs/prod/foundry/deployments/emb.json")
            .is_file(),
        "the available deployment is promoted"
    );
}

#[test]
fn promote_offline_reports_unresolved_carriers() {
    // No ARM at all: `--offline` must not reach for it, and must say the
    // auth carrier is unresolved rather than inventing one.
    let ws = promote_workspace();
    let unreachable = "http://127.0.0.1:9";
    bind_function_apps(ws.path(), unreachable);
    write_keyed_skillset(
        ws.path(),
        "https://mklab-dev.azurewebsites.net/api/enrich?code=<redacted>",
    );
    rigg(ws.path(), unreachable)
        .args([
            "promote",
            "demo",
            "--from",
            "dev",
            "--to",
            "prod",
            "--yes",
            "--offline",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("unresolved"));

    let prod = read_json(
        &ws.path()
            .join("projects/demo/envs/prod/search/skillsets/ss.json"),
    );
    let skill = &prod["skills"][0];
    assert!(
        skill.get("authResourceId").is_none() && skill.get("x-rigg-auth").is_none(),
        "offline promote derives no carrier: {prod}"
    );
    assert!(
        !skill["uri"].as_str().unwrap().contains("code="),
        "the source's key never crosses: {prod}"
    );
}
