//! Guard: every Azure api-version lives in the registry provider table.
use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "enabled in Task 4 once client.rs and sync.rs are migrated"]
fn no_api_version_literals_outside_registry() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("crates");
    let mut files = Vec::new();
    walk(&root, &mut files);
    let re = regex_lite::Regex::new(
        r#"api-version=20\d\d-\d\d-\d\d|"20\d\d-\d\d-\d\d(-preview)?"\s*(,|\)|;|$)"#,
    )
    .unwrap();
    let mut offenders = Vec::new();
    for f in files {
        if f.ends_with("registry.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if re.is_match(line) {
                offenders.push(format!("{}:{}: {}", f.display(), i + 1, line.trim()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "api-version literals outside registry.rs:\n{}",
        offenders.join("\n")
    );
}
