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
use serde_json::Value;

use rigg_core::normalize::normalize_for_push;
use rigg_core::registry;
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::{ProjectState, Store, SyncClass, assert_exclusive_ownership};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};

use crate::cli::PullArgs;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{
    CommandError, GlobalContext, interactive, load_workspace, resolve_env, select_projects,
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
    let env = resolve_env(&ws, ctx)?;
    assert_exclusive_ownership(&ws, &env.name)?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;

    // Resources owned by ANY project (for unmanaged detection).
    let mut owned_by_any: BTreeSet<String> = BTreeSet::new();
    for project in &ws.projects {
        for (r, _) in Store::new(project, &env.name).list()? {
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
    let store = Store::new(project, &env.name);
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
    let auto_created = registry::auto_created_by(&snapshot);
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
            if !registry::is_platform_managed(r.kind, doc) && !auto_created.contains_key(&key) {
                unmanaged += 1;
            }
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
                let summary = local
                    .as_ref()
                    .map(|l| conflict_summary(r.kind, l, doc))
                    .unwrap_or_else(|| "differs locally and remotely".to_string());
                if ctx.yes {
                    store.write(r, doc)?;
                    state.set_baseline(r, doc);
                    println!("  {} overwrote {}", "~".cyan(), r);
                    written += 1;
                } else if ctx.interactive() {
                    println!("  {} {} — {}", "conflict".red().bold(), r, summary);
                    const OVERWRITE: &str = "overwrite local with remote";
                    const KEEP: &str = "keep local";
                    const DIFF: &str = "show diff";
                    const ABORT: &str = "abort pull";
                    let mut show_diff_option = true;
                    loop {
                        let mut opts = vec![OVERWRITE.to_string(), KEEP.to_string()];
                        if show_diff_option {
                            opts.push(DIFF.to_string());
                        }
                        opts.push(ABORT.to_string());
                        // Esc/Ctrl-C counts as abort: save baselines gathered
                        // so far, exactly like choosing "abort pull".
                        let choice = match interactive::select("Resolve:", opts, ctx.no_color) {
                            Ok(c) => c,
                            Err(_) => ABORT.to_string(),
                        };
                        match choice.as_str() {
                            OVERWRITE => {
                                store.write(r, doc)?;
                                state.set_baseline(r, doc);
                                println!("  {} overwrote {}", "~".cyan(), r);
                                written += 1;
                                break;
                            }
                            KEEP => {
                                println!("  kept local {r}");
                                break;
                            }
                            DIFF => {
                                if let Some(l) = &local {
                                    let result = rigg_diff::semantic::diff(
                                        &normalize_for_push(r.kind, doc),
                                        &normalize_for_push(r.kind, l),
                                        "name",
                                    );
                                    let labels = rigg_diff::output::SideLabels {
                                        new_side: "local".to_string(),
                                        old_side: format!("Azure ({})", env.name),
                                    };
                                    println!();
                                    print!(
                                        "{}",
                                        rigg_diff::output::format_text(
                                            &result,
                                            &r.to_string(),
                                            &labels
                                        )
                                    );
                                    println!();
                                }
                                show_diff_option = false;
                            }
                            _ => {
                                state.save(ws, &env.name, &project.name)?;
                                return Err(anyhow!("aborted"));
                            }
                        }
                    }
                } else {
                    println!(
                        "  {} {} — {} (run `rigg diff {}` to inspect; pass --yes to overwrite)",
                        "conflict".red().bold(),
                        r,
                        summary,
                        project.name
                    );
                    any_conflict = true;
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
            state.has_baseline(r)
                && remote.supported_kinds().contains(&r.kind)
                && !remote_keys.contains(&r.key())
        })
        .collect();
    for r in deleted {
        if ctx.interactive() {
            println!("  {} {} was deleted remotely", "!".red().bold(), r);
            if interactive::confirm_default_no(
                "Delete the local file too? (no = keep for re-push)",
                ctx.no_color,
            )? {
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

/// One-line conflict summary: change count + first few differing fields.
/// Orientation matches `rigg diff`: old=remote, new=local.
fn conflict_summary(kind: ResourceKind, local: &Value, remote: &Value) -> String {
    let result = rigg_diff::semantic::diff(
        &normalize_for_push(kind, remote),
        &normalize_for_push(kind, local),
        "name",
    );
    let n = result.changes.len();
    // The diff engine walks a HashSet internally, so change order is not
    // stable across runs — sort for deterministic, readable output.
    let mut paths: Vec<&str> = result.changes.iter().map(|c| c.path.as_str()).collect();
    paths.sort_unstable();
    let mut fields: Vec<&str> = paths.into_iter().take(3).collect();
    if n > 3 {
        fields.push("…");
    }
    format!("{n} field(s) differ ({})", fields.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_summary_counts_and_names_fields() {
        let local = serde_json::json!({"name": "a", "model": "x", "p": 1, "q": 2, "r": 3});
        let remote = serde_json::json!({"name": "a", "model": "y", "p": 9, "q": 8, "r": 7});
        let s = conflict_summary(ResourceKind::Agent, &local, &remote);
        assert!(s.starts_with("4 field(s) differ ("), "{s}");
        assert!(s.contains("model"), "{s}");
        assert!(
            s.ends_with(", …)") || s.matches(',').count() >= 2,
            "at most 3 named: {s}"
        );
    }
}
