//! `rigg validate` — structural, referential, ownership, and no-secrets checks.
//!
//! Exit code 3 when any check fails.

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::Value;

use rigg_core::registry::{self, X_RIGG_API, X_RIGG_REF};
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::{Store, assert_exclusive_ownership};
use rigg_core::workspace::{Project, Workspace};

use crate::cli::ValidateArgs;
use crate::commands::{CommandError, GlobalContext, load_workspace, select_projects};

pub fn run(ctx: &GlobalContext, args: ValidateArgs) -> Result<()> {
    let ws = load_workspace()?;
    let projects = select_projects_lenient(&ws, args.project.as_deref())?;

    let mut problems: Vec<String> = Vec::new();

    // Workspace-wide: exclusive ownership.
    if let Err(e) = assert_exclusive_ownership(&ws) {
        problems.push(e.to_string());
    }

    // All resources across the workspace (for reference resolution).
    let mut workspace_refs: Vec<ResourceRef> = Vec::new();
    for project in &ws.projects {
        if let Ok(list) = Store::new(project).list() {
            workspace_refs.extend(list.into_iter().map(|(r, _)| r));
        }
    }

    for project in &projects {
        validate_project(&ws, project, &workspace_refs, args.strict, &mut problems);
    }

    if ctx.json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "valid": problems.is_empty(),
                "problems": problems,
            }))?
        );
    } else if problems.is_empty() {
        println!("{} all checks passed", "✓".green().bold());
    } else {
        for p in &problems {
            println!("{} {p}", "✗".red().bold());
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(CommandError::Validation(format!(
            "{} validation problem(s) found",
            problems.len()
        ))))
    }
}

fn select_projects_lenient<'w>(
    ws: &'w Workspace,
    project: Option<&str>,
) -> Result<Vec<&'w Project>> {
    match project {
        Some(name) => Ok(vec![ws.project(name)?]),
        None => Ok(ws.projects.iter().collect()),
    }
}

fn validate_project(
    ws: &Workspace,
    project: &Project,
    workspace_refs: &[ResourceRef],
    strict: bool,
    problems: &mut Vec<String>,
) {
    let store = Store::new(project);
    let list = match store.list() {
        Ok(list) => list,
        Err(e) => {
            problems.push(format!("[{}] {e}", project.name));
            return;
        }
    };

    for (r, path) in &list {
        let display = path
            .strip_prefix(&ws.root)
            .unwrap_or(path)
            .display()
            .to_string();
        let value = match store.read(r) {
            Ok(v) => v,
            Err(e) => {
                problems.push(format!("[{display}] {e}"));
                continue;
            }
        };

        // name matches filename
        match value.get("name").and_then(Value::as_str) {
            Some(name) if name == r.name => {}
            Some(name) => problems.push(format!(
                "[{display}] \"name\" is '{name}' but the filename says '{}' — they must match",
                r.name
            )),
            None => problems.push(format!("[{display}] missing \"name\" field")),
        }

        // no secrets (registry secret_fields carrying key material)
        check_secrets(r.kind, &value, &display, problems);

        // registry references resolve within the workspace
        for (kind, name) in registry::extract_references(r.kind, &value) {
            if name.starts_with('<') {
                problems.push(format!(
                    "[{display}] placeholder reference '{name}' — replace the scaffold placeholder"
                ));
                continue;
            }
            let target = ResourceRef::new(kind, name.clone());
            let in_workspace = workspace_refs.contains(&target);
            if !in_workspace {
                let cross_domain = kind.domain() != r.kind.domain();
                if strict || !cross_domain {
                    problems.push(format!(
                        "[{display}] references {target} which does not exist in this workspace"
                    ));
                }
            }
        }

        // x-rigg-api links resolve to specs in apis/
        check_api_links(ws, &value, &display, problems);

        // datasource type validity
        if r.kind == ResourceKind::DataSource {
            if let Some(ds_type) = value.get("type").and_then(Value::as_str) {
                match rigg_core::scaffold::check_datasource_type(ds_type) {
                    Ok(Some(warning)) => eprintln!("{} [{display}] {warning}", "warning:".yellow()),
                    Ok(None) => {}
                    Err(e) => problems.push(format!("[{display}] {e}")),
                }
            } else {
                problems.push(format!("[{display}] data source missing \"type\""));
            }
        }
    }
}

/// Reject key material in registry-declared secret fields, and obvious
/// key patterns anywhere (AccountKey=, AccountName=...;AccountKey).
fn check_secrets(kind: ResourceKind, value: &Value, display: &str, problems: &mut Vec<String>) {
    for spec in registry::meta(kind).secret_fields {
        registry::collect_path(value, spec, &mut |v| {
            if let Some(s) = v.as_str() {
                if !s.is_empty() && !s.starts_with("ResourceId=") && !s.starts_with('<') {
                    problems.push(format!(
                        "[{display}] field '{spec}' contains a credential — rigg never stores secrets locally. \
                         Use a managed identity (connection string 'ResourceId=/subscriptions/...') and grant the \
                         identity RBAC access instead; secrets belong in Azure Key Vault, never in files"
                    ));
                }
            }
        });
    }
    // Blanket scan for storage account keys.
    let text = value.to_string();
    if text.contains("AccountKey=") {
        problems.push(format!(
            "[{display}] contains an 'AccountKey=' connection string — replace it with an identity-based \
             'ResourceId=...' connection and delete/rotate the leaked key"
        ));
    }
}

fn check_api_links(ws: &Workspace, value: &Value, display: &str, problems: &mut Vec<String>) {
    collect_key(value, X_RIGG_API, &mut |v| {
        if let Some(api) = v.as_str() {
            let path = ws.apis_dir().join(format!("{api}.json"));
            if !path.is_file() {
                problems.push(format!(
                    "[{display}] x-rigg-api '{api}' has no spec at apis/{api}.json (create with `rigg new api {api}`)"
                ));
            }
        }
    });
    collect_key(value, X_RIGG_REF, &mut |v| {
        if let Some(s) = v.as_str() {
            let valid = s
                .split_once('/')
                .and_then(|(dir, _)| ResourceKind::from_directory_name(dir))
                .is_some();
            if !valid {
                problems.push(format!(
                    "[{display}] x-rigg-ref '{s}' is not of the form <kind-dir>/<name>"
                ));
            }
        }
    });
}

fn collect_key(value: &Value, key: &str, f: &mut dyn FnMut(&Value)) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == key {
                    f(v);
                } else {
                    collect_key(v, key, f);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_key(item, key, f);
            }
        }
        _ => {}
    }
}
