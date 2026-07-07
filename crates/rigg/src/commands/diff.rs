//! `rigg diff` — semantic comparison of local project files vs live Azure
//! (or one environment vs another with --compare-env).

use anyhow::{Result, anyhow};
use serde_json::Value;

use rigg_core::normalize::normalize_for_push;
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::Store;
use rigg_core::workspace::{Project, Workspace};
use rigg_diff::semantic::DiffResult;

use crate::cli::{DiffArgs, DiffFormat};
use crate::commands::remote::Remote;
use crate::commands::{CommandError, GlobalContext, load_workspace, select_projects};

pub async fn run(ctx: &GlobalContext, args: DiffArgs) -> Result<()> {
    let ws = load_workspace()?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;
    let only = args.only.as_deref().map(parse_only).transpose()?;

    let mut diffs: Vec<(String, DiffResult)> = Vec::new();
    for project in projects {
        diffs.extend(diff_project(ctx, &ws, project, &args, only.as_ref()).await?);
    }

    let output = match args.format {
        DiffFormat::Text => {
            rigg_diff::output::format_report(&diffs, rigg_diff::output::OutputFormat::Text)
        }
        DiffFormat::Json => {
            rigg_diff::output::format_report(&diffs, rigg_diff::output::OutputFormat::Json)
        }
        DiffFormat::Markdown => rigg_diff::output::format_markdown(&diffs),
    };
    print!("{output}");

    let has_drift = diffs.iter().any(|(_, d)| !d.is_equal);
    if args.exit_code && has_drift {
        return Err(anyhow!(CommandError::DriftOrConflict(
            "differences found".to_string()
        )));
    }
    Ok(())
}

fn parse_only(s: &str) -> Result<ResourceRef> {
    let (dir, name) = s.split_once('/').ok_or_else(|| {
        anyhow!(CommandError::Usage(
            "--only takes <kind-dir>/<name> (e.g. indexes/my-index)".to_string()
        ))
    })?;
    let kind = ResourceKind::from_directory_name(dir).ok_or_else(|| {
        anyhow!(CommandError::Usage(format!(
            "unknown resource kind directory '{dir}'"
        )))
    })?;
    Ok(ResourceRef::new(kind, name.to_string()))
}

async fn diff_project(
    ctx: &GlobalContext,
    ws: &Workspace,
    project: &Project,
    args: &DiffArgs,
    only: Option<&ResourceRef>,
) -> Result<Vec<(String, DiffResult)>> {
    let store = Store::new(project);

    // Baseline side: local files (or env A remote with --compare-env).
    // Comparison side: the resolved environment's remote.
    let env_b = ws.resolve_env(args.compare_env.as_deref().or(ctx.env.as_deref()))?;
    let remote_b = Remote::for_project(&env_b, project);

    let mut pairs: Vec<(ResourceRef, Option<Value>, Option<Value>)> = Vec::new();

    if let Some(compare_env) = &args.compare_env {
        // env A (selected/default) vs env B (--compare-env)
        let env_a = ws.resolve_env(ctx.env.as_deref())?;
        if env_a.name == *compare_env {
            return Err(anyhow!(CommandError::Usage(
                "--compare-env must name a different environment".to_string()
            )));
        }
        let remote_a = Remote::for_project(&env_a, project);
        let snap_a = remote_a.snapshot().await?;
        let snap_b = remote_b.snapshot().await?;
        let keys: std::collections::BTreeSet<ResourceRef> = snap_a
            .iter()
            .chain(snap_b.iter())
            .map(|(r, _)| r.clone())
            .collect();
        for r in keys {
            let a = snap_a.iter().find(|(x, _)| *x == r).map(|(_, v)| v.clone());
            let b = snap_b.iter().find(|(x, _)| *x == r).map(|(_, v)| v.clone());
            pairs.push((r, a, b));
        }
    } else {
        // local vs remote — union of local files and remote resources owned here
        let local_files = store.list()?;
        let mut seen: std::collections::BTreeSet<ResourceRef> = Default::default();
        for (r, _) in &local_files {
            let local = store.read(r).ok();
            let remote_doc = remote_b.get(r).await?;
            seen.insert(r.clone());
            pairs.push((r.clone(), local, remote_doc));
        }
        // remote resources with a baseline here but no local file (deleted locally)
        let state = rigg_core::store::ProjectState::load(ws, &env_b.name, &project.name);
        for key in state.baselines.keys() {
            if let Some((dir, name)) = key.split_once('/') {
                if let Some(kind) = ResourceKind::from_directory_name(dir) {
                    let r = ResourceRef::new(kind, name.to_string());
                    if !seen.contains(&r) && remote_b.supported_kinds().contains(&kind) {
                        let remote_doc = remote_b.get(&r).await?;
                        if remote_doc.is_some() {
                            pairs.push((r, None, remote_doc));
                        }
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    for (r, left, right) in pairs {
        if let Some(only) = only {
            if &r != only {
                continue;
            }
        }
        if !remote_b.supported_kinds().contains(&r.kind) {
            continue;
        }
        let left_n = left
            .map(|v| normalize_for_push(r.kind, &v))
            .unwrap_or(Value::Null);
        let right_n = right
            .map(|v| normalize_for_push(r.kind, &v))
            .unwrap_or(Value::Null);
        // diff(old=remote/right, new=local/left): report what pushing would change
        let result = rigg_diff::semantic::diff(&right_n, &left_n, "name");
        out.push((format!("{}/{}", project.name, r), result));
    }
    Ok(out)
}
