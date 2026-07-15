//! `rigg status` — per-project sync classification + unmanaged remote
//! resources, reported across all environments by default.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::json;

use rigg_core::registry;
use rigg_core::resources::ResourceRef;
use rigg_core::store::{ProjectState, Store, SyncClass};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};

use crate::cli::StatusArgs;
use crate::commands::remote::Remote;
use crate::commands::{CommandError, GlobalContext, is_auth_error, load_workspace, resolve_env};

type ProjectRows = (String, Vec<(ResourceRef, SyncClass)>, Vec<ResourceRef>);

struct EnvReport {
    name: String,
    is_default: bool,
    outcome: Result<Vec<ProjectRows>>,
}

pub async fn run(ctx: &GlobalContext, args: StatusArgs) -> Result<()> {
    let ws = load_workspace()?;
    let projects: Vec<_> = match args.project.as_deref() {
        Some(name) => vec![ws.project(name)?],
        None => ws.projects.iter().collect(),
    };

    if ws.projects.is_empty() && args.project.is_none() && !ctx.json() {
        crate::commands::print_no_projects_hint();
        return Ok(());
    }

    let envs = selected_envs(&ws, ctx)?;
    let default_name = ws.default_env_name().map(str::to_string);

    // One report per environment, fetched concurrently; failures are
    // captured per env so one broken environment never hides the others.
    let reports = futures::future::join_all(envs.iter().map(|env| async {
        EnvReport {
            name: env.name.clone(),
            is_default: Some(env.name.as_str()) == default_name.as_deref(),
            outcome: env_report(&ws, env, &projects).await,
        }
    }))
    .await;

    if !reports.is_empty() && reports.iter().all(|r| r.outcome.is_err()) {
        let reasons = reports
            .iter()
            .filter_map(|r| {
                r.outcome
                    .as_ref()
                    .err()
                    .map(|e| format!("{}: {e:#}", r.name))
            })
            .collect::<Vec<_>>()
            .join("\n");
        if reports
            .iter()
            .any(|r| matches!(&r.outcome, Err(e) if is_auth(e)))
        {
            return Err(anyhow!(CommandError::AuthDenied(format!(
                "no environment reachable:\n{reasons}"
            ))));
        }
        return Err(anyhow!("no environment reachable:\n{reasons}"));
    }

    if ctx.json() {
        render_json(&reports)?;
    } else {
        render_text(&reports);
    }
    Ok(())
}

/// Environments to report on: an explicit selection (`--env` flag or
/// `RIGG_ENV`) narrows to that one env; otherwise all environments, the
/// default one first, the rest alphabetical.
fn selected_envs(ws: &Workspace, ctx: &GlobalContext) -> Result<Vec<ResolvedEnv>> {
    if ctx.env.is_some() || std::env::var("RIGG_ENV").is_ok() {
        return Ok(vec![resolve_env(ws, ctx)?]);
    }
    let mut names: Vec<String> = ws.config.environments.keys().cloned().collect();
    if let Some(default) = ws.default_env_name()
        && let Some(pos) = names.iter().position(|n| n == default)
    {
        let d = names.remove(pos);
        names.insert(0, d);
    }
    names.iter().map(|n| Ok(ws.resolve_env(Some(n))?)).collect()
}

async fn env_report(
    ws: &Workspace,
    env: &ResolvedEnv,
    projects: &[&Project],
) -> Result<Vec<ProjectRows>> {
    // All owned keys across the workspace, for unmanaged detection.
    let mut owned_by_any: BTreeSet<String> = BTreeSet::new();
    for project in &ws.projects {
        for (r, _) in Store::new(project, &env.name).list()? {
            owned_by_any.insert(r.key());
        }
        let state = ProjectState::load(ws, &env.name, &project.name);
        owned_by_any.extend(state.baselines.keys().cloned());
    }

    let mut report = Vec::new();
    for project in projects {
        let store = Store::new(project, &env.name);
        let remote = Remote::for_project(env, project);
        let state = ProjectState::load(ws, &env.name, &project.name);

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
    Ok(report)
}

/// Does any cause in the chain look like an authentication/authorization
/// failure?
fn is_auth(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<rigg_client::error::ClientError>()
            .is_some_and(is_auth_error)
    })
}

fn render_json(reports: &[EnvReport]) -> Result<()> {
    let value = json!(
        reports
            .iter()
            .map(|rep| match &rep.outcome {
                Ok(projects) => json!({
                    "env": rep.name,
                    "default": rep.is_default,
                    "error": serde_json::Value::Null,
                    "projects": projects.iter().map(|(name, rows, unmanaged)| json!({
                        "project": name,
                        "resources": rows.iter().map(|(r, c)| json!({
                            "resource": r.key(),
                            "state": c,
                        })).collect::<Vec<_>>(),
                        "unmanaged": unmanaged.iter().map(|r| r.key()).collect::<Vec<_>>(),
                    })).collect::<Vec<_>>(),
                }),
                Err(e) => json!({
                    "env": rep.name,
                    "default": rep.is_default,
                    "error": format!("{e:#}"),
                    "projects": [],
                }),
            })
            .collect::<Vec<_>>()
    );
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn render_text(reports: &[EnvReport]) {
    for rep in reports {
        if rep.is_default {
            println!(
                "{} {}",
                format!("env: {}", rep.name).bold(),
                "(default)".dimmed()
            );
        } else {
            println!("{}", format!("env: {}", rep.name).bold());
        }
        match &rep.outcome {
            Err(e) => {
                let hint = if is_auth(e) {
                    " — run `rigg auth doctor`"
                } else {
                    ""
                };
                println!("  {}: {e:#}{hint}", "unreachable".red());
            }
            Ok(projects) => {
                for (name, rows, unmanaged) in projects {
                    println!("  {}", name.bold());
                    if rows.is_empty() {
                        println!("    (no resources)");
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
                        println!("    {:<50} {label}", r.to_string());
                    }
                    if !unmanaged.is_empty() {
                        println!(
                            "    {} unmanaged remote resource(s): {}",
                            unmanaged.len(),
                            unmanaged
                                .iter()
                                .map(|r| r.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                        println!(
                            "      adopt with: rigg adopt <project> <selector>  (e.g. all, indexes, agents/name)"
                        );
                    }
                }
            }
        }
        println!();
    }
}
