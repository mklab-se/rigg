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
        &[("storageAccounts", "devacct", "rg", "swedencentral")],
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
    assert!(ws.path().join(".rigg/dev/bindings.json").exists());

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
        .stdout(predicate::str::contains("ghost").and(predicate::str::contains("?")));
}
