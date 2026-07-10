//! `rigg push` — apply local project files to Azure, in dependency order.
//!
//! Semantics (spec §5.3):
//! - only semantically-changed resources are pushed
//! - creates/updates run in reference-graph order; prunes in reverse order
//! - after every successful write the server document is fetched back,
//!   normalized and written to disk + baseline (canonicalization)
//! - orphans (baseline exists, file deleted) require --prune or confirmation
//! - conflicts (local and remote both changed) fail non-interactively (exit 5)

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::Value;

use rigg_core::graph;
use rigg_core::normalize::normalize_for_push;
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::{ProjectState, Store, SyncClass, assert_exclusive_ownership};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};

use crate::cli::PushArgs;
use crate::commands::remote::{Remote, ensure_any_connection, resolve_cross_service_refs};
use crate::commands::{
    CommandError, GlobalContext, confirm, load_workspace, resolve_env, select_projects,
};

pub async fn run(ctx: &GlobalContext, args: PushArgs) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    assert_exclusive_ownership(&ws, &env.name)?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;

    let mut any_conflict = false;
    for project in projects {
        any_conflict |= push_project(ctx, &ws, &env, project, &args).await?;
    }
    if any_conflict {
        return Err(anyhow!(CommandError::DriftOrConflict(
            "conflicts detected; resolve them (pull, merge, or push after review) and retry"
                .to_string()
        )));
    }
    Ok(())
}

struct PlanItem {
    r: ResourceRef,
    body: Value,
    exists_remotely: bool,
}

async fn push_project(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    project: &Project,
    args: &PushArgs,
) -> Result<bool> {
    let store = Store::new(project, &env.name);
    let remote = Remote::for_project(env, project);
    ensure_any_connection(&remote, project)?;
    let mut state = ProjectState::load(ws, &env.name, &project.name);

    println!(
        "{} project '{}' (env: {})",
        "Push".bold(),
        project.name.bold(),
        env.name
    );

    // Collect local resources.
    let local_files = store.list()?;
    let mut items: Vec<(ResourceRef, Value)> = Vec::new();
    for (r, _) in &local_files {
        items.push((r.clone(), store.read(r)?));
    }

    // Classify each against remote + baseline.
    let mut to_push: Vec<PlanItem> = Vec::new();
    let mut conflicts: Vec<ResourceRef> = Vec::new();
    let mut skipped_remote_ahead: Vec<ResourceRef> = Vec::new();
    for (r, body) in &items {
        let remote_doc = remote.get(r).await?;
        match state.classify(r, Some(body), remote_doc.as_ref()) {
            SyncClass::InSync => {}
            SyncClass::LocalAhead | SyncClass::LocalOnly | SyncClass::Untracked => {
                to_push.push(PlanItem {
                    r: r.clone(),
                    body: body.clone(),
                    exists_remotely: remote_doc.is_some(),
                });
            }
            SyncClass::RemoteAhead => skipped_remote_ahead.push(r.clone()),
            SyncClass::Conflict => conflicts.push(r.clone()),
            SyncClass::RemoteOnly => unreachable!("local body was provided"),
        }
    }

    // Orphans: baseline exists but local file is gone.
    let local_set: std::collections::BTreeSet<String> =
        items.iter().map(|(r, _)| r.key()).collect();
    let mut orphans: Vec<ResourceRef> = Vec::new();
    for key in state.baselines.keys() {
        if local_set.contains(key) {
            continue;
        }
        if let Some(r) = parse_key(key) {
            if remote.supported_kinds().contains(&r.kind) && remote.get(&r).await?.is_some() {
                orphans.push(r);
            }
        }
    }

    // Report the plan.
    if to_push.is_empty() && orphans.is_empty() && conflicts.is_empty() {
        println!("  {} everything in sync", "✓".green());
        return Ok(false);
    }
    let order = graph::push_order(
        &to_push
            .iter()
            .map(|p| (p.r.clone(), p.body.clone()))
            .collect::<Vec<_>>(),
    )?;
    for r in &order {
        let item = to_push.iter().find(|p| &p.r == r).expect("ordered item");
        let verb = if item.exists_remotely {
            "update"
        } else {
            "create"
        };
        println!("  {} {}", verb.cyan(), r);
    }
    for r in &skipped_remote_ahead {
        println!(
            "  {} {} (remote changed since last sync — pull first)",
            "skip".yellow(),
            r
        );
    }
    for r in &conflicts {
        println!(
            "  {} {} (both local and remote changed)",
            "conflict".red().bold(),
            r
        );
    }
    for r in &orphans {
        if args.prune {
            println!("  {} {}", "delete".red(), r);
        } else {
            println!(
                "  {} {} (file deleted locally; pass --prune to delete remotely)",
                "orphan".yellow(),
                r
            );
        }
    }

    if args.dry_run {
        println!("  (dry run — nothing pushed)");
        return Ok(!conflicts.is_empty());
    }
    if !conflicts.is_empty() && !ctx.interactive() {
        return Ok(true); // caller reports exit 5
    }

    // Confirm.
    if ctx.interactive() {
        let total = order.len() + if args.prune { orphans.len() } else { 0 };
        if total > 0 && !confirm::prompt_yes_no(&format!("Apply {total} change(s)?"))? {
            println!("  aborted");
            return Ok(false);
        }
    } else if !ctx.yes {
        return Err(anyhow!(CommandError::Usage(
            "non-interactive push requires --yes".to_string()
        )));
    }

    // Interactive conflict handling: choose local/remote/skip per conflict.
    for r in &conflicts {
        let local = store.read(r)?;
        let remote_doc = remote.get(r).await?.unwrap_or(Value::Null);
        println!();
        println!("{} {}", "Conflict:".red().bold(), r);
        let diff = rigg_diff::semantic::diff(
            &normalize_for_push(r.kind, &remote_doc),
            &normalize_for_push(r.kind, &local),
            "name",
        );
        let conflict_labels = rigg_diff::output::SideLabels {
            new_side: "local".to_string(),
            old_side: format!("Azure ({})", env.name),
        };
        print!(
            "{}",
            rigg_diff::output::format_text(&diff, &r.to_string(), &conflict_labels)
        );
        let ai = crate::commands::ai_assist::ai_on(ctx);
        if ai {
            println!(
                "  [l] push local  [r] keep remote (overwrites local file)  [a] AI merge proposal  [s] skip"
            );
        } else {
            println!("  [l] push local  [r] keep remote (overwrites local file)  [s] skip");
        }
        let options: &[char] = if ai {
            &['l', 'r', 'a', 's']
        } else {
            &['l', 'r', 's']
        };
        match confirm::prompt_choice("resolve", options)? {
            'l' => to_push.push(PlanItem {
                r: r.clone(),
                body: local,
                exists_remotely: true,
            }),
            'r' => {
                store.write(r, &remote_doc)?;
                state.set_baseline(r, &remote_doc);
                println!("  kept remote version for {r}");
            }
            'a' => {
                println!("  asking ailloy for a merge proposal...");
                match crate::commands::ai_assist::propose_merge(&r.to_string(), &local, &remote_doc)
                    .await
                {
                    Ok(proposal) => {
                        let vs_local = rigg_diff::semantic::diff(
                            &normalize_for_push(r.kind, &local),
                            &normalize_for_push(r.kind, &proposal),
                            "name",
                        );
                        let vs_remote = rigg_diff::semantic::diff(
                            &normalize_for_push(r.kind, &remote_doc),
                            &normalize_for_push(r.kind, &proposal),
                            "name",
                        );
                        println!("  proposal vs LOCAL:");
                        let vs_local_labels = rigg_diff::output::SideLabels {
                            new_side: "AI proposal".to_string(),
                            old_side: "local".to_string(),
                        };
                        print!(
                            "{}",
                            rigg_diff::output::format_text(
                                &vs_local,
                                &r.to_string(),
                                &vs_local_labels
                            )
                        );
                        println!("  proposal vs REMOTE:");
                        let vs_remote_labels = rigg_diff::output::SideLabels {
                            new_side: "AI proposal".to_string(),
                            old_side: format!("Azure ({})", env.name),
                        };
                        print!(
                            "{}",
                            rigg_diff::output::format_text(
                                &vs_remote,
                                &r.to_string(),
                                &vs_remote_labels
                            )
                        );
                        if confirm::prompt_yes_no(
                            "  accept the proposal (writes the local file and pushes it)?",
                        )? {
                            store.write(r, &proposal)?;
                            to_push.push(PlanItem {
                                r: r.clone(),
                                body: proposal,
                                exists_remotely: true,
                            });
                        } else {
                            println!("  discarded proposal; skipped {r}");
                        }
                    }
                    Err(e) => println!("  AI merge failed ({e}); skipped {r}"),
                }
            }
            _ => println!("  skipped {r}"),
        }
    }

    // Execute in order (conflicts resolved to local were appended — reorder).
    let order = graph::push_order(
        &to_push
            .iter()
            .map(|p| (p.r.clone(), p.body.clone()))
            .collect::<Vec<_>>(),
    )?;
    for r in &order {
        let item = to_push.iter().find(|p| &p.r == r).expect("ordered item");
        // Resolve cross-service refs BEFORE stripping the x-rigg-* annotations
        // that drive the resolution.
        let mut with_refs = item.body.clone();
        resolve_cross_service_refs(env.search_for(project).ok(), &mut with_refs)?;
        let body = normalize_for_push(r.kind, &with_refs);

        match remote.put(r, &body).await {
            Ok(server_doc) => {
                store.write(r, &server_doc)?;
                state.set_baseline(r, &server_doc);
                state.save(ws, &env.name, &project.name)?;
                println!("  {} {}", "✓".green(), r);
            }
            Err(e) => {
                state.save(ws, &env.name, &project.name)?;
                return Err(e.context(format!("failed to push {r}")));
            }
        }
    }

    // Prune orphans in reverse dependency order (best effort ordering: use
    // registry declaration order reversed — orphan bodies are gone).
    if args.prune {
        let mut ordered = orphans.clone();
        ordered.sort();
        ordered.reverse();
        for r in &ordered {
            remote.delete(r).await?;
            state.clear_baseline(r);
            state.save(ws, &env.name, &project.name)?;
            println!("  {} deleted {}", "✓".green(), r);
        }
    }

    state.save(ws, &env.name, &project.name)?;
    Ok(false)
}

fn parse_key(key: &str) -> Option<ResourceRef> {
    let (dir, name) = key.split_once('/')?;
    let kind = ResourceKind::from_directory_name(dir)?;
    Some(ResourceRef::new(kind, name.to_string()))
}
