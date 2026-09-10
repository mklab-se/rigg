//! Documentation guards: the generated reference material matches the
//! binary, and the prose never names a command, flag or file that is not
//! there.
//!
//! Three artefacts are generated from the code and only checked here:
//! `docs/reference/cli.md` (whole file), the infrastructure-reference table
//! in `docs/reference/resource-files.md`, and the tool table in `MCP.md`.
//! The command-line and link walk lives in the binary (`rigg dev
//! docs-check`) because the `rigg` crate has no lib target to import `Cli`
//! from — these tests only drive it and assert the verdict.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// The workspace root — `crates/rigg/` two levels up.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("workspace root resolves")
}

fn rigg() -> Command {
    let mut cmd = Command::cargo_bin("rigg").expect("binary builds");
    cmd.env("RIGG_NO_UPDATE_CHECK", "1");
    cmd.env_remove("RIGG_ENV");
    cmd
}

/// Run a generator and return its stdout.
fn generate(args: &[&str]) -> String {
    let out = rigg().args(args).assert().success();
    String::from_utf8(out.get_output().stdout.clone()).expect("generator prints UTF-8")
}

/// Compare ignoring trailing whitespace per line and around the whole text.
fn normalized(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// The text between `<!-- generated:<name>:start -->` and its `:end` marker.
fn generated_region(file: &Path, name: &str) -> String {
    let text = std::fs::read_to_string(file)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", file.display()));
    let start = format!("<!-- generated:{name}:start -->");
    let end = format!("<!-- generated:{name}:end -->");
    let open = text
        .find(&start)
        .unwrap_or_else(|| panic!("{} has no `{start}` marker", file.display()));
    let body = open + start.len();
    let close = text[body..]
        .find(&end)
        .unwrap_or_else(|| panic!("{} has no `{end}` marker", file.display()))
        + body;
    text[body..close].to_string()
}

fn regen_hint(name: &str) -> &'static str {
    match name {
        "infra-table" => {
            "run: cargo run -q --bin rigg -- dev infra-table \
             and replace the text between the markers"
        }
        "mcp-tools" => {
            "run: cargo run -q --bin rigg -- mcp tools --markdown \
             and replace the text between the markers"
        }
        other => panic!("no regeneration hint for region `{other}`"),
    }
}

/// Assert the marked region of `file` is exactly what the generator prints.
fn assert_region_current(file: &Path, name: &str, generated: &str) {
    let on_disk = normalized(&generated_region(file, name));
    let fresh = normalized(generated);
    assert_eq!(
        on_disk,
        fresh,
        "\n{} region `{name}` is out of date — {}\n",
        file.display(),
        regen_hint(name)
    );
}

#[test]
fn cli_reference_is_current() {
    let file = repo_root().join("docs").join("reference").join("cli.md");
    let on_disk = std::fs::read_to_string(&file)
        .unwrap_or_else(|e| panic!("{} is readable: {e}", file.display()));
    assert_eq!(
        normalized(&on_disk),
        normalized(&generate(&["dev", "cli-reference"])),
        "\n{} is out of date — run: cargo run -q --bin rigg -- dev cli-reference > docs/reference/cli.md\n",
        file.display()
    );
}

#[test]
fn infra_table_is_current() {
    let file = repo_root()
        .join("docs")
        .join("reference")
        .join("resource-files.md");
    assert_region_current(&file, "infra-table", &generate(&["dev", "infra-table"]));
}

#[test]
fn mcp_tool_table_is_current() {
    let file = repo_root().join("MCP.md");
    assert_region_current(
        &file,
        "mcp-tools",
        &generate(&["mcp", "tools", "--markdown"]),
    );
}

/// Every `rigg …` line in a fenced block in the docs parses through clap:
/// no page may name a subcommand or flag that does not exist.
#[test]
fn docs_commands_parse() {
    let root = repo_root();
    let out = rigg()
        .args(["dev", "docs-check", "--root"])
        .arg(&root)
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("UTF-8");
    assert!(
        stdout.trim_end().ends_with("docs-check: ok"),
        "docs-check did not report ok:\n{stdout}"
    );
}

/// Every relative Markdown link (and heading anchor) in the docs resolves.
#[test]
fn relative_links_resolve() {
    let root = repo_root();
    let out = rigg()
        .args(["dev", "docs-check", "--root"])
        .arg(&root)
        .assert()
        .success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).expect("UTF-8");
    assert!(
        stdout.trim_end().ends_with("docs-check: ok"),
        "docs-check did not report ok:\n{stdout}"
    );
}
