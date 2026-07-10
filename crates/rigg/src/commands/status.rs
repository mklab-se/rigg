//! `rigg status` — per-project sync classification + unmanaged remote resources.

use std::collections::BTreeSet;

use anyhow::Result;
use colored::Colorize;
use serde_json::json;

use rigg_core::registry;
use rigg_core::store::{ProjectState, Store, SyncClass};
use rigg_core::workspace::Workspace;

use crate::cli::StatusArgs;
use crate::commands::remote::Remote;
use crate::commands::{GlobalContext, load_workspace, resolve_env};

pub async fn run(ctx: &GlobalContext, args: StatusArgs) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let projects: Vec<_> = match args.project.as_deref() {
        Some(name) => vec![ws.project(name)?],
        None => ws.projects.iter().collect(),
    };

    if ws.projects.is_empty() && args.project.is_none() && !ctx.json() {
        crate::commands::print_no_projects_hint();
        return Ok(());
    }

    // All owned keys across the workspace, for unmanaged detection.
    let mut owned_by_any: BTreeSet<String> = BTreeSet::new();
    for project in &ws.projects {
        for (r, _) in Store::new(project, &env.name).list()? {
            owned_by_any.insert(r.key());
        }
        let state = ProjectState::load(&ws, &env.name, &project.name);
        owned_by_any.extend(state.baselines.keys().cloned());
    }

    let mut report = Vec::new();
    for project in &projects {
        let store = Store::new(project, &env.name);
        let remote = Remote::for_project(&env, project);
        let state = ProjectState::load(&ws, &env.name, &project.name);

        let mut rows = Vec::new();
        let mut unmanaged = Vec::new();

        if remote.has_search() || remote.has_foundry() {
            let snapshot = remote.snapshot().await?;
            let auto_created = registry::auto_created_by(&snapshot);
            let remote_map: std::collections::BTreeMap<String, &serde_json::Value> =
                snapshot.iter().map(|(r, v)| (r.key(), v)).collect();

            for (r, _) in store.list()? {
                let local = store.read(&r).ok();
                let remote_doc = remote_map.get(&r.key()).copied().cloned();
                let class = state.classify(&r, local.as_ref(), remote_doc.as_ref());
                rows.push((r, class));
            }
            for (r, doc) in &snapshot {
                if !owned_by_any.contains(&r.key())
                    && !registry::is_platform_managed(r.kind, doc)
                    && !auto_created.contains_key(&r.key())
                {
                    unmanaged.push(r.clone());
                }
            }
        } else {
            for (r, _) in store.list()? {
                rows.push((r, SyncClass::LocalOnly));
            }
        }

        report.push((project.name.clone(), rows, unmanaged));
    }

    if ctx.json() {
        let value = json!(
            report
                .iter()
                .map(|(name, rows, unmanaged)| json!({
                    "project": name,
                    "resources": rows.iter().map(|(r, c)| json!({
                        "resource": r.key(),
                        "state": c,
                    })).collect::<Vec<_>>(),
                    "unmanaged": unmanaged.iter().map(|r| r.key()).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>()
        );
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    for (name, rows, unmanaged) in &report {
        println!("{} (env: {})", name.bold(), env.name);
        if rows.is_empty() {
            println!("  (no resources)");
        }
        for (r, class) in rows {
            let label = match class {
                SyncClass::InSync => "in sync".green(),
                SyncClass::LocalAhead => "local ahead (push pending)".yellow(),
                SyncClass::RemoteAhead => "remote ahead (pull pending)".yellow(),
                SyncClass::Conflict => "CONFLICT".red().bold(),
                SyncClass::LocalOnly => "local only (push to create)".cyan(),
                SyncClass::RemoteOnly => "remote only".cyan(),
                SyncClass::Untracked => "untracked (never synced)".dimmed(),
            };
            println!("  {:<50} {label}", r.to_string());
        }
        if !unmanaged.is_empty() {
            println!(
                "  {} unmanaged remote resource(s): {}",
                unmanaged.len(),
                unmanaged
                    .iter()
                    .map(|r| r.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            println!(
                "    adopt with: rigg adopt <project> <selector>  (e.g. all, indexes, agents/name)"
            );
        }
        println!();
    }
    Ok(())
}

// keep Workspace import used in signature evolution
#[allow(unused)]
fn _t(_: &Workspace) {}
