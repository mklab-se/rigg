//! `rigg diff` — semantic comparison of local project files vs live Azure
//! (or one environment vs another with --compare-env).

use anyhow::{Result, anyhow};
use serde_json::Value;

use rigg_core::normalize::normalize_for_push;
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::Store;
use rigg_core::workspace::{Project, Workspace};
use rigg_diff::output::SideLabels;
use rigg_diff::semantic::DiffResult;

use crate::cli::{DiffArgs, DiffFormat};
use crate::commands::remote::Remote;
use crate::commands::{CommandError, GlobalContext, load_workspace, select_projects};

pub async fn run(ctx: &GlobalContext, args: DiffArgs) -> Result<()> {
    let ws = load_workspace()?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;
    let only = args.only.as_deref().map(parse_only).transpose()?;

    let mut diffs: Vec<(String, DiffResult)> = Vec::new();
    let mut replace_notes: Vec<String> = Vec::new();
    for project in projects {
        diffs.extend(
            diff_project(ctx, &ws, project, &args, only.as_ref(), &mut replace_notes).await?,
        );
    }

    let labels = side_labels(&ws, ctx, &args)?;
    let output = match args.format {
        DiffFormat::Text => {
            rigg_diff::output::format_report(&diffs, rigg_diff::output::OutputFormat::Text, &labels)
        }
        DiffFormat::Json => {
            rigg_diff::output::format_report(&diffs, rigg_diff::output::OutputFormat::Json, &labels)
        }
        DiffFormat::Markdown => rigg_diff::output::format_markdown(&diffs, &labels),
    };
    print!("{output}");
    if args.format == DiffFormat::Text {
        for note in &replace_notes {
            println!("{note}");
        }
    }

    let has_drift = diffs.iter().any(|(_, d)| !d.is_equal);
    if has_drift && args.format == DiffFormat::Text && crate::commands::ai_assist::ai_on(ctx) {
        match crate::commands::ai_assist::explain_diff(&output).await {
            Ok(summary) => {
                println!();
                println!("AI summary (ailloy):");
                for line in summary.lines() {
                    println!("  {line}");
                }
            }
            Err(e) => eprintln!("note: AI summary unavailable ({e})"),
        }
    }
    if has_drift && args.format == DiffFormat::Text && args.compare_env.is_none() {
        let p = drifted_project_hint(&diffs);
        println!();
        println!("hint: rigg pull {p} — update local files to match Azure");
        println!("      rigg push {p} — make Azure match your local files");
    }
    if args.exit_code && has_drift {
        return Err(anyhow!(CommandError::DriftOrConflict(
            "differences found".to_string()
        )));
    }
    Ok(())
}

/// Build the labels for the two sides of the diff: local-vs-Azure by
/// default, or the two named environments in `--compare-env` mode. Mirrors
/// the env resolution in `diff_project` — new=local/env_a, old=Azure/env_b
/// (see the diff-orientation comment there).
fn side_labels(ws: &Workspace, ctx: &GlobalContext, args: &DiffArgs) -> Result<SideLabels> {
    let env_b = ws.resolve_env(args.compare_env.as_deref().or(ctx.env.as_deref()))?;
    if args.compare_env.is_some() {
        let env_a = ws.resolve_env(ctx.env.as_deref())?;
        Ok(SideLabels {
            new_side: env_a.name.clone(),
            old_side: env_b.name.clone(),
        })
    } else {
        Ok(SideLabels {
            new_side: "local".to_string(),
            old_side: format!("Azure ({})", env_b.name),
        })
    }
}

/// Derive the `<project>` placeholder for the post-diff hint: the single
/// drifted project's name if every drifted key names the same one, else the
/// literal `<project>` placeholder. Keys look like `"{project}/{kind}/{name}"`.
fn drifted_project_hint(diffs: &[(String, DiffResult)]) -> String {
    let mut projects: Vec<&str> = diffs
        .iter()
        .filter(|(_, d)| !d.is_equal)
        .filter_map(|(key, _)| key.split_once('/').map(|(project, _)| project))
        .collect();
    projects.sort_unstable();
    projects.dedup();
    match projects.as_slice() {
        [only] => only.to_string(),
        _ => "<project>".to_string(),
    }
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
    notes: &mut Vec<String>,
) -> Result<Vec<(String, DiffResult)>> {
    // Baseline side: local files (or env A remote with --compare-env).
    // Comparison side: the resolved environment's remote.
    let env_b = ws.resolve_env(args.compare_env.as_deref().or(ctx.env.as_deref()))?;
    let store = Store::new(project, &env_b.name);
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
        // Immutable-field drift (local vs Azure only): pushing this is not an
        // update — surface the replace consequence alongside the diff.
        if args.compare_env.is_none() {
            if let (Some(local), Some(remote)) = (&left, &right) {
                for (path, remote_val, local_val) in
                    rigg_core::registry::immutable_diff(r.kind, local, remote)
                {
                    notes.push(format!(
                        "note: {}/{r} — '{path}' is immutable ({remote_val} → {local_val}): \
                         push will REPLACE this resource (delete + recreate; a knowledge \
                         source's index is rebuilt from source data)",
                        project.name
                    ));
                }
            }
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
