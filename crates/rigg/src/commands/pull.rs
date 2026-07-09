//! `rigg pull` — bring remote resource definitions into project files.
//!
//! Ownership rules (spec §5.3):
//! - a project pulls only resources it owns (file or baseline exists)
//! - remote resources owned by no project are reported as *unmanaged*;
//!   `rigg adopt <project> <selector>` claims them into that project
//! - remote-deleted resources ask (interactive) or exit 5 (non-interactive)

use std::collections::BTreeSet;

use anyhow::{Result, anyhow};
use colored::Colorize;

use rigg_core::resources::ResourceRef;
use rigg_core::store::{ProjectState, Store, SyncClass, assert_exclusive_ownership};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};

use crate::cli::PullArgs;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{
    CommandError, GlobalContext, confirm, load_workspace, resolve_env, select_projects,
};

pub async fn run(ctx: &GlobalContext, args: PullArgs) -> Result<()> {
    if args.watch {
        loop {
            pull_once(ctx, &args).await?;
            tokio::time::sleep(std::time::Duration::from_secs(args.interval)).await;
        }
    }
    pull_once(ctx, &args).await
}

async fn pull_once(ctx: &GlobalContext, args: &PullArgs) -> Result<()> {
    let ws = load_workspace()?;
    assert_exclusive_ownership(&ws)?;
    let env = resolve_env(&ws, ctx)?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;

    // Resources owned by ANY project (for unmanaged detection).
    let mut owned_by_any: BTreeSet<String> = BTreeSet::new();
    for project in &ws.projects {
        for (r, _) in Store::new(project).list()? {
            owned_by_any.insert(r.key());
        }
        let state = ProjectState::load(&ws, &env.name, &project.name);
        owned_by_any.extend(state.baselines.keys().cloned());
    }

    let mut any_conflict = false;
    for project in projects {
        any_conflict |= pull_project(ctx, &ws, &env, project, &owned_by_any).await?;
    }
    if any_conflict {
        return Err(anyhow!(CommandError::DriftOrConflict(
            "conflicts detected during pull; resolve interactively or with --yes".to_string()
        )));
    }
    Ok(())
}

async fn pull_project(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    project: &Project,
    owned_by_any: &BTreeSet<String>,
) -> Result<bool> {
    let store = Store::new(project);
    let remote = Remote::for_project(env, project);
    ensure_any_connection(&remote, project)?;
    let mut state = ProjectState::load(ws, &env.name, &project.name);

    println!(
        "{} project '{}' (env: {})",
        "Pull".bold(),
        project.name.bold(),
        env.name
    );

    let snapshot = remote.snapshot().await?;
    let remote_keys: BTreeSet<String> = snapshot.iter().map(|(r, _)| r.key()).collect();

    // Which of the remote resources does THIS project own?
    let local_files = store.list()?;
    let mut owned_here: BTreeSet<String> = local_files.iter().map(|(r, _)| r.key()).collect();
    owned_here.extend(state.baselines.keys().cloned());

    let mut any_conflict = false;
    let mut unmanaged = 0usize;
    let mut written = 0usize;

    for (r, doc) in &snapshot {
        let key = r.key();
        let owned_by_this = owned_here.contains(&key);
        let owned_elsewhere = !owned_by_this && owned_by_any.contains(&key);
        if owned_elsewhere {
            continue; // another project's resource
        }
        if !owned_by_this {
            unmanaged += 1;
            continue;
        }

        let local = store.read(r).ok();
        match state.classify(r, local.as_ref(), Some(doc)) {
            SyncClass::InSync => {
                // Content equal — refresh the baseline unconditionally so a
                // stale baseline (e.g. after both sides converged) self-heals.
                state.set_baseline(r, doc);
            }
            SyncClass::RemoteAhead | SyncClass::RemoteOnly => {
                if store.write(r, doc)? {
                    println!("  {} updated {}", "~".cyan(), r);
                    written += 1;
                }
                state.set_baseline(r, doc);
            }
            SyncClass::LocalAhead => {
                println!("  {} {} (local ahead — push pending)", "≠".yellow(), r);
            }
            SyncClass::Untracked | SyncClass::Conflict => {
                if ctx.yes
                    || (ctx.interactive() && {
                        println!(
                            "  {} {} differs locally and remotely",
                            "conflict".red().bold(),
                            r
                        );
                        confirm::prompt_yes_no("  overwrite local file with the remote version?")?
                    })
                {
                    store.write(r, doc)?;
                    state.set_baseline(r, doc);
                    println!("  {} overwrote {}", "~".cyan(), r);
                    written += 1;
                } else if !ctx.interactive() {
                    println!(
                        "  {} {} (local and remote differ; pass --yes to overwrite)",
                        "conflict".red().bold(),
                        r
                    );
                    any_conflict = true;
                } else {
                    println!("  kept local {r}");
                }
            }
            SyncClass::LocalOnly => unreachable!("remote doc was provided"),
        }
    }

    // Remote-deleted: owned here, baseline exists, gone remotely.
    let deleted: Vec<ResourceRef> = local_files
        .iter()
        .map(|(r, _)| r.clone())
        .filter(|r| {
            state.baseline(r).is_some()
                && remote.supported_kinds().contains(&r.kind)
                && !remote_keys.contains(&r.key())
        })
        .collect();
    for r in deleted {
        if ctx.interactive() {
            println!("  {} {} was deleted remotely", "!".red().bold(), r);
            if confirm::prompt_yes_no("  delete the local file too? (no = keep for re-push)")? {
                store.delete(&r)?;
                state.clear_baseline(&r);
                println!("  {} removed local {}", "✓".green(), r);
            } else {
                state.clear_baseline(&r);
                println!("  kept {r} (push will re-create it in Azure)");
            }
        } else {
            println!(
                "  {} {} deleted remotely (kept locally; delete the file or push to re-create)",
                "!".red().bold(),
                r
            );
            state.clear_baseline(&r);
            any_conflict = true;
        }
    }

    state.save(ws, &env.name, &project.name)?;
    if unmanaged > 0 {
        println!(
            "  {} {unmanaged} unmanaged remote resource(s) — adopt with `rigg adopt {} <selector>` (e.g. `all`, `indexes`, `agents/name`)",
            "i".blue(),
            project.name,
        );
    }
    if written == 0 {
        println!("  {} up to date", "✓".green());
    }
    Ok(any_conflict)
}
