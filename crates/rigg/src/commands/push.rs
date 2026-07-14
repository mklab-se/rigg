//! `rigg push` — apply local project files to Azure, in dependency order.
//!
//! Semantics (spec §5.3):
//! - only semantically-changed resources are pushed
//! - creates/updates run in reference-graph order; prunes in reverse order
//! - after every successful write the server document is fetched back,
//!   normalized and written to disk + baseline (canonicalization)
//! - orphans (baseline exists, file deleted) require --prune or confirmation
//! - conflicts (local and remote both changed) fail non-interactively (exit 5)

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, anyhow};
use colored::Colorize;
use serde_json::{Value, json};

use rigg_core::normalize::normalize_for_push;
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::{ProjectState, Store, SyncClass, assert_exclusive_ownership};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};
use rigg_core::{graph, migrate, registry};

use crate::cli::PushArgs;
use crate::commands::credentials;
use crate::commands::remote::{Remote, ensure_any_connection, resolve_cross_service_refs};
use crate::commands::{
    CommandError, GlobalContext, confirm_protected_env, interactive, load_workspace, resolve_env,
    select_projects,
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

/// One replace operation: a resource whose immutable field(s) changed, so it
/// must be deleted and re-created — for a knowledge source, together with the
/// generated pipeline Azure cascades away on delete.
struct ReplaceBundle {
    ks: ResourceRef,
    /// Desired local document (the new shape).
    new_body: Value,
    /// Current remote document (the old shape, incl. createdResources).
    remote_ks: Value,
    /// Local files re-created after the cascade delete, regardless of their
    /// own sync class (an in-sync copy would otherwise be skipped and lost).
    sub: Vec<(ResourceRef, Value)>,
    /// (path, remote value, local value) for plan display.
    diff: Vec<(&'static str, String, String)>,
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
        "{} project '{}' (env: {}{})",
        "Push".bold(),
        project.name.bold(),
        env.name,
        if env.protected() {
            format!(", {}", "protected".yellow())
        } else {
            String::new()
        }
    );
    remote.print_targets();

    // Collect local resources.
    let local_files = store.list()?;
    let mut items: Vec<(ResourceRef, Value)> = Vec::new();
    for (r, _) in &local_files {
        items.push((r.clone(), store.read(r)?));
    }

    // Leftover relink obligations from an interrupted replace (see
    // execute_replace): ks name → original knowledge-base docs.
    let mut pending_relinks = load_pending_relinks(ws, &env.name, &project.name)?;

    // Classify each against remote + baseline.
    let mut to_push: Vec<PlanItem> = Vec::new();
    let mut replaces: Vec<ReplaceBundle> = Vec::new();
    let mut conflicts: Vec<ResourceRef> = Vec::new();
    let mut skipped_remote_ahead: Vec<ResourceRef> = Vec::new();
    for (r, body) in &items {
        let remote_doc = remote.get(r).await?;
        // Immutable-field change → in-place PUT cannot reconcile: replace.
        if let Some(remote_body) = &remote_doc {
            let diff = registry::immutable_diff(r.kind, body, remote_body);
            if !diff.is_empty() {
                replaces.push(ReplaceBundle {
                    ks: r.clone(),
                    new_body: body.clone(),
                    remote_ks: remote_body.clone(),
                    sub: Vec::new(),
                    diff,
                });
                continue;
            }
        }
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

    // Attach each replace bundle's sub-resources: local files named by the
    // remote knowledge source's createdResources are cascade-deleted with it
    // and must be re-created inside the bundle — pull them out of the normal
    // plan whatever their own sync class.
    for bundle in &mut replaces {
        let created: BTreeSet<String> = migrate::created_resources(&bundle.remote_ks)
            .iter()
            .map(|(kind, name)| ResourceRef::new(*kind, name.clone()).key())
            .collect();
        for (r, body) in &items {
            if created.contains(&r.key()) {
                bundle.sub.push((r.clone(), body.clone()));
            }
        }
        to_push.retain(|p| !created.contains(&p.r.key()));
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
    if to_push.is_empty() && orphans.is_empty() && conflicts.is_empty() && replaces.is_empty() {
        if !pending_relinks.is_empty() && !args.dry_run {
            finish_pending_relinks(
                &remote,
                ws,
                env,
                project,
                &store,
                &mut state,
                &mut pending_relinks,
            )
            .await?;
        }
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
    for bundle in &replaces {
        let (path, remote_val, local_val) = &bundle.diff[0];
        println!(
            "  {} {}   {}: {} → {}",
            "replace".magenta().bold(),
            bundle.ks,
            path,
            remote_val,
            local_val
        );
        println!(
            "      {} deletes the knowledge source AND its generated pipeline, then",
            "⚠".yellow()
        );
        println!("        recreates it explicitly. The index is REBUILT from source data:");
        println!("        this takes time, costs ingestion/embeddings, and the source is");
        println!("        unavailable to knowledge bases until repopulated.");
        if !bundle.sub.is_empty() {
            let names: Vec<String> = bundle.sub.iter().map(|(r, _)| r.to_string()).collect();
            println!("      recreates: {}", names.join(", "));
        }
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

    // Credential preflight: a data source about to be CREATED (plain create
    // or replace re-create) without a usable connection fails at PUT time —
    // for a replace, AFTER the old pipeline was already destroyed. Detect it
    // here, before anything mutates. Updates that omit credentials are legal
    // (the service keeps the existing secret), so only creations are gated.
    let mut fixed_credentials = false;
    let mut cred_missing: Vec<ResourceRef> = Vec::new();
    for item in &to_push {
        if item.r.kind == ResourceKind::DataSource
            && !item.exists_remotely
            && credentials::missing_credentials(&item.body)
        {
            cred_missing.push(item.r.clone());
        }
    }
    for bundle in &replaces {
        for (r, body) in &bundle.sub {
            if r.kind == ResourceKind::DataSource && credentials::missing_credentials(body) {
                cred_missing.push(r.clone());
            }
        }
    }
    for r in &cred_missing {
        println!(
            "  {} {} has no credentials.connectionString — a created data source needs a connection (identity-based ResourceId=...)",
            "!".yellow(),
            r
        );
    }

    // Same for skillsets: a key-based AI services connection whose key was
    // redacted (Azure never returns keys) fails at PUT time. The identity-
    // based rewrite needs only the subdomain already in the file.
    let mut key_missing: Vec<(ResourceRef, Option<String>)> = Vec::new();
    for item in &to_push {
        if item.r.kind == ResourceKind::Skillset && !item.exists_remotely {
            if let Some(subdomain) = credentials::skillset_missing_ai_services_key(&item.body) {
                key_missing.push((item.r.clone(), subdomain));
            }
        }
    }
    for bundle in &replaces {
        for (r, body) in &bundle.sub {
            if r.kind == ResourceKind::Skillset {
                if let Some(subdomain) = credentials::skillset_missing_ai_services_key(body) {
                    key_missing.push((r.clone(), subdomain));
                }
            }
        }
    }
    for (r, _) in &key_missing {
        println!(
            "  {} {} has a key-based cognitiveServices connection without a usable key — switch to identity-based (AIServicesByIdentity)",
            "!".yellow(),
            r
        );
    }

    if args.dry_run {
        println!("  (dry run — nothing pushed)");
        return Ok(!conflicts.is_empty());
    }

    // Resolve missing connections before any gate: interactively, discover
    // the storage account by container via ARM (the user is logged in with
    // Azure CLI — rigg figures it out instead of asking for an id); anything
    // still unresolved refuses the push before a single remote call.
    if !cred_missing.is_empty() {
        if ctx.interactive() {
            let pending: Vec<ResourceRef> = cred_missing.clone();
            for r in pending {
                let mut doc = store.read(&r)?;
                let container = credentials::container_name(&doc).map(str::to_string);
                let found = credentials::discover_connection_interactive(
                    &r.to_string(),
                    container.as_deref(),
                    ctx.no_color,
                )
                .await?;
                if let Some(conn) = found {
                    credentials::set_connection(&mut doc, &conn);
                    store.write(&r, &doc)?;
                    if let Some(item) = to_push.iter_mut().find(|p| p.r == r) {
                        credentials::set_connection(&mut item.body, &conn);
                    }
                    for bundle in &mut replaces {
                        if let Some((_, body)) = bundle.sub.iter_mut().find(|(sr, _)| *sr == r) {
                            credentials::set_connection(body, &conn);
                        }
                    }
                    println!(
                        "  {} {} connection set (identity-based, no key on disk)",
                        "✓".green(),
                        r
                    );
                    fixed_credentials = true;
                    if let Some(account) = conn.strip_prefix("ResourceId=") {
                        credentials::print_rbac_hint(account.trim_end_matches(';'));
                    }
                    cred_missing.retain(|x| x != &r);
                }
            }
        }
        if !cred_missing.is_empty() {
            let names: Vec<String> = cred_missing.iter().map(|r| r.to_string()).collect();
            return Err(anyhow!(CommandError::Validation(format!(
                "{} has no credentials.connectionString — set an identity-based connection \
                 (`ResourceId=/subscriptions/.../storageAccounts/<name>;`) in the file, or run \
                 `rigg push` interactively to auto-discover the storage account",
                names.join(", ")
            ))));
        }
    }
    if !key_missing.is_empty() {
        if ctx.interactive() {
            let pending = key_missing.clone();
            for (r, subdomain) in pending {
                let Some(subdomain) = subdomain else {
                    continue; // no subdomain in the file — cannot rewrite automatically
                };
                if interactive::confirm_default_yes(
                    &format!(
                        "Switch {r} to identity-based AI services access ('{subdomain}', no key on disk)?"
                    ),
                    ctx.no_color,
                )? {
                    let mut doc = store.read(&r)?;
                    credentials::set_ai_services_identity(&mut doc, &subdomain);
                    store.write(&r, &doc)?;
                    if let Some(item) = to_push.iter_mut().find(|p| p.r == r) {
                        credentials::set_ai_services_identity(&mut item.body, &subdomain);
                    }
                    for bundle in &mut replaces {
                        if let Some((_, body)) = bundle.sub.iter_mut().find(|(sr, _)| *sr == r) {
                            credentials::set_ai_services_identity(body, &subdomain);
                        }
                    }
                    println!("  {} {} switched to AIServicesByIdentity", "✓".green(), r);
                    fixed_credentials = true;
                    if let Some(account) = credentials::ai_services_account_name(&subdomain) {
                        credentials::print_ai_services_rbac_hint(account);
                    }
                    key_missing.retain(|(x, _)| x != &r);
                }
            }
        }
        if !key_missing.is_empty() {
            let names: Vec<String> = key_missing.iter().map(|(r, _)| r.to_string()).collect();
            return Err(anyhow!(CommandError::Validation(format!(
                "{} has a key-based cognitiveServices connection without a usable key — \
                 rigg never stores keys; use the identity-based form \
                 (`{{\"@odata.type\": \"#Microsoft.Azure.Search.AIServicesByIdentity\", \
                 \"subdomainUrl\": \"https://<account>.cognitiveservices.azure.com/\"}}`), \
                 or run `rigg push` interactively to rewrite it automatically",
                names.join(", ")
            ))));
        }
    }

    // The connections just fixed need data-plane roles the identities may
    // not have yet — offer to verify/grant them right here instead of
    // hinting and letting the push run into a predictable 400.
    if fixed_credentials && ctx.interactive() {
        println!();
        if interactive::confirm_default_yes(
            "Verify and grant the roles these connections need now (runs auth doctor --fix)?",
            ctx.no_color,
        )? {
            if let Err(e) = crate::commands::doctor::run(ctx, true).await {
                println!(
                    "  {} auth doctor could not fix everything ({e:#}) — continuing; the push may fail until the roles exist",
                    "!".yellow()
                );
            }
        }
    }

    // Protected-env gate: fires before any mutating call (creates/updates
    // below, and the --prune deletion path), and before the routine apply
    // confirmation so a rejected/missing typed confirmation short-circuits
    // everything that follows.
    if !confirm_protected_env(ctx, env, args.confirm_env.as_deref(), "push")? {
        println!("Aborted.");
        return Ok(false);
    }

    if !conflicts.is_empty() && !ctx.interactive() {
        return Ok(true); // caller reports exit 5
    }

    // Replace gate: --yes deliberately does NOT satisfy it (same philosophy
    // as --confirm-env) — a replace destroys and rebuilds a live index, and
    // scripts pipe -y reflexively. Interactive: explicit default-No confirm.
    if !replaces.is_empty() && !args.allow_replace {
        if ctx.interactive() {
            let prompt = format!(
                "Proceed with {} replace(s)? The index rebuild takes time and money.",
                replaces.len()
            );
            if !interactive::confirm_default_no(&prompt, ctx.no_color)? {
                println!("  aborted");
                return Ok(false);
            }
        } else {
            return Err(anyhow!(CommandError::Usage(
                "push plan contains replace(s); pass --allow-replace (in addition to --yes) to proceed"
                    .to_string()
            )));
        }
    }

    // Confirm.
    if ctx.interactive() {
        let total = order.len() + replaces.len() + if args.prune { orphans.len() } else { 0 };
        if total > 0
            && !interactive::confirm_default_no(&format!("Apply {total} change(s)?"), ctx.no_color)?
        {
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
        const PUSH_LOCAL: &str = "push local";
        const KEEP_REMOTE: &str = "keep remote (overwrites local file)";
        const AI_MERGE: &str = "AI merge proposal";
        const SKIP: &str = "skip";
        let mut options = vec![PUSH_LOCAL.to_string(), KEEP_REMOTE.to_string()];
        if ai {
            options.push(AI_MERGE.to_string());
        }
        options.push(SKIP.to_string());
        // Esc/Ctrl-C counts as skip for this resource.
        let choice = match interactive::select("Resolve:", options, ctx.no_color) {
            Ok(c) => c,
            Err(_) => SKIP.to_string(),
        };
        match choice.as_str() {
            PUSH_LOCAL => to_push.push(PlanItem {
                r: r.clone(),
                body: local,
                exists_remotely: true,
            }),
            KEEP_REMOTE => {
                store.write(r, &remote_doc)?;
                state.set_baseline(r, &remote_doc);
                println!("  kept remote version for {r}");
            }
            AI_MERGE => {
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
                        if interactive::confirm_default_no(
                            "Accept the proposal (writes the local file and pushes it)?",
                            ctx.no_color,
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

        match put_with_rbac_help(&remote, r, &body, fixed_credentials).await {
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

    // Execute replace bundles (delete + recreate with knowledge-base
    // unlink/relink), then finish any leftover relink obligations from a
    // previously interrupted run.
    for bundle in &replaces {
        let prior = pending_relinks.remove(&bundle.ks.name).unwrap_or_default();
        execute_replace(
            env,
            ws,
            project,
            &store,
            &mut state,
            &remote,
            bundle,
            prior,
            fixed_credentials,
        )
        .await?;
    }
    if !pending_relinks.is_empty() {
        finish_pending_relinks(
            &remote,
            ws,
            env,
            project,
            &store,
            &mut state,
            &mut pending_relinks,
        )
        .await?;
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

/// Errors that smell like missing data-plane RBAC for a managed identity.
fn is_rbac_error(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}").to_lowercase();
    msg.contains("managed identity")
        || msg.contains("cognitive services user")
        || msg.contains("storage blob data reader")
        || (msg.contains("identity") && msg.contains("permission"))
}

/// PUT with two kinds of RBAC help: when `patience` is set (a role may have
/// been granted moments ago by the inline doctor fix), RBAC-shaped rejections
/// are retried twice at 20s intervals — Azure role assignments take a moment
/// to propagate. Either way, an RBAC-shaped failure points at the doctor
/// instead of leaving a bare API error.
async fn put_with_rbac_help(
    remote: &Remote,
    r: &ResourceRef,
    body: &Value,
    patience: bool,
) -> Result<Value> {
    let mut attempt = 0u8;
    loop {
        match remote.put(r, body).await {
            Ok(v) => return Ok(v),
            Err(e) if patience && attempt < 2 && is_rbac_error(&e) => {
                attempt += 1;
                println!(
                    "  {} {} rejected for a missing role — waiting 20s for RBAC propagation (retry {attempt}/2)",
                    "…".yellow(),
                    r
                );
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
            }
            Err(e) if is_rbac_error(&e) => {
                return Err(e.context(
                    "this looks like a missing role assignment — run `rigg auth doctor --fix` \
                     (and allow a minute for a fresh grant to propagate)",
                ));
            }
            Err(e) => return Err(e),
        }
    }
}

// ---------------------------------------------------------------------------
// Replace orchestration (knowledge-source kind change)
// ---------------------------------------------------------------------------

/// Recovery file for one replace: written before the knowledge bases are
/// unlinked, removed after they are restored. Its presence after a crash is
/// what lets the next `rigg push` finish the relink — essential for
/// knowledge bases outside this project, whose original docs exist nowhere
/// else.
fn recovery_path(ws: &Workspace, env: &str, project: &str, ks_name: &str) -> std::path::PathBuf {
    ws.state_dir(env, project)
        .join(format!("replace-{ks_name}.json"))
}

/// Load leftover relink obligations (`replace-*.json`) from interrupted runs:
/// ks name → original knowledge-base docs.
fn load_pending_relinks(
    ws: &Workspace,
    env: &str,
    project: &str,
) -> Result<BTreeMap<String, Vec<Value>>> {
    let mut out = BTreeMap::new();
    let dir = ws.state_dir(env, project);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(out);
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let Some(ks) = name
            .strip_prefix("replace-")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        let text = std::fs::read_to_string(entry.path())
            .with_context(|| format!("reading recovery file {}", entry.path().display()))?;
        let doc: Value = serde_json::from_str(&text)
            .with_context(|| format!("parsing recovery file {}", entry.path().display()))?;
        let kbs = doc
            .get("knowledge_bases")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        println!(
            "  {} found interrupted replace of knowledge-sources/{ks} — will restore {} knowledge base link(s)",
            "↻".cyan(),
            kbs.len()
        );
        out.insert(ks.to_string(), kbs);
    }
    Ok(out)
}

/// Restore knowledge-base links recorded by interrupted replaces, where the
/// replaced knowledge source exists again. Removes each finished file.
#[allow(clippy::too_many_arguments)]
async fn finish_pending_relinks(
    remote: &Remote,
    ws: &Workspace,
    env: &ResolvedEnv,
    project: &Project,
    store: &Store<'_>,
    state: &mut ProjectState,
    pending: &mut BTreeMap<String, Vec<Value>>,
) -> Result<()> {
    let names: Vec<String> = pending.keys().cloned().collect();
    for ks_name in names {
        let ks_ref = ResourceRef::new(ResourceKind::KnowledgeSource, ks_name.clone());
        if remote.get(&ks_ref).await?.is_none() {
            println!(
                "  {} knowledge-sources/{ks_name} still missing remotely — push its file, then run push again to restore knowledge base links",
                "!".yellow()
            );
            continue;
        }
        let kbs = pending.remove(&ks_name).unwrap_or_default();
        relink_knowledge_bases(remote, store, state, &kbs).await?;
        state.save(ws, &env.name, &project.name)?;
        std::fs::remove_file(recovery_path(ws, &env.name, &project.name, &ks_name)).ok();
        println!(
            "  {} restored {} knowledge base link(s) for knowledge-sources/{ks_name}",
            "✓".green(),
            kbs.len()
        );
    }
    Ok(())
}

/// PUT each original knowledge-base doc back (re-creating any that were
/// deleted during unlink); canonicalize the ones this project owns.
async fn relink_knowledge_bases(
    remote: &Remote,
    store: &Store<'_>,
    state: &mut ProjectState,
    kbs: &[Value],
) -> Result<()> {
    for original in kbs {
        let Some(name) = original.get("name").and_then(Value::as_str) else {
            continue;
        };
        let kb_ref = ResourceRef::new(ResourceKind::KnowledgeBase, name.to_string());
        let body = normalize_for_push(ResourceKind::KnowledgeBase, original);
        let server_doc = remote
            .put(&kb_ref, &body)
            .await
            .with_context(|| format!("failed to restore {kb_ref}"))?;
        if store.locate(&kb_ref)?.is_some() {
            store.write(&kb_ref, &server_doc)?;
            state.set_baseline(&kb_ref, &server_doc);
        }
    }
    Ok(())
}

/// Execute one replace bundle:
/// 1. snapshot every remote knowledge base referencing the knowledge source
///    (plus `prior` obligations from an interrupted run),
/// 2. write the recovery file,
/// 3. unlink (PUT without the reference; DELETE when the service rejects an
///    empty knowledgeSources list),
/// 4. delete the old knowledge source (Azure cascades the generated pipeline),
/// 5. re-create the local sub-resources in dependency order,
/// 6. create the new knowledge source,
/// 7. restore the knowledge bases and remove the recovery file.
///
/// Any failure leaves the recovery file in place; re-running `rigg push`
/// resumes (the kind change is re-detected, or the missing knowledge source
/// becomes a plain create, and leftover relinks are finished at the end).
#[allow(clippy::too_many_arguments)]
async fn execute_replace(
    env: &ResolvedEnv,
    ws: &Workspace,
    project: &Project,
    store: &Store<'_>,
    state: &mut ProjectState,
    remote: &Remote,
    bundle: &ReplaceBundle,
    prior: Vec<Value>,
    rbac_patience: bool,
) -> Result<()> {
    let ks = &bundle.ks;
    println!("  {} {}", "replace".magenta().bold(), ks);

    // 1. Snapshot referencing knowledge bases — ALL of them, this project's
    // or not: the delete fails while any reference exists. Foreign ones are
    // restored byte-for-byte afterwards.
    let mut referencing: Vec<Value> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for kb in remote.list(ResourceKind::KnowledgeBase).await? {
        let mut refs = Vec::new();
        registry::collect_path(&kb, "knowledgeSources[].name", &mut |v| {
            if let Some(s) = v.as_str() {
                refs.push(s.to_string());
            }
        });
        if refs.iter().any(|r| r == &ks.name) {
            if let Some(name) = kb.get("name").and_then(Value::as_str) {
                seen.insert(name.to_string());
                let kb_ref = ResourceRef::new(ResourceKind::KnowledgeBase, name.to_string());
                if store.locate(&kb_ref)?.is_none() && !state.has_baseline(&kb_ref) {
                    println!(
                        "      {} temporarily unlinking foreign knowledge base '{name}' (not managed by this project) — restored afterwards",
                        "!".yellow()
                    );
                }
            }
            referencing.push(kb);
        }
    }
    // Merge obligations from an interrupted earlier run (those knowledge
    // bases are already unlinked, so the listing above missed them).
    for kb in prior {
        let name = kb.get("name").and_then(Value::as_str).unwrap_or_default();
        if !name.is_empty() && !seen.contains(name) {
            referencing.push(kb);
        }
    }

    // 2. Recovery file BEFORE any mutation.
    let recovery = recovery_path(ws, &env.name, &project.name, &ks.name);
    if let Some(parent) = recovery.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &recovery,
        serde_json::to_string_pretty(&json!({
            "ks": ks.name,
            "knowledge_bases": referencing,
        }))?,
    )
    .with_context(|| format!("writing recovery file {}", recovery.display()))?;

    let step = |msg: &str| {
        format!(
            "replace of '{}' interrupted {msg}; re-run `rigg push` to resume (recovery file kept)",
            ks.name
        )
    };

    // 3. Unlink.
    for kb in &referencing {
        let Some(name) = kb.get("name").and_then(Value::as_str) else {
            continue;
        };
        let kb_ref = ResourceRef::new(ResourceKind::KnowledgeBase, name.to_string());
        let mut unlinked = normalize_for_push(ResourceKind::KnowledgeBase, kb);
        let mut now_empty = false;
        if let Some(list) = unlinked
            .get_mut("knowledgeSources")
            .and_then(Value::as_array_mut)
        {
            list.retain(|entry| {
                entry.get("name").and_then(Value::as_str) != Some(ks.name.as_str())
            });
            now_empty = list.is_empty();
        }
        let result = remote.put(&kb_ref, &unlinked).await;
        match result {
            Ok(_) => println!("      unlinked {kb_ref}"),
            Err(e) if now_empty => {
                // The service may reject an empty knowledgeSources list —
                // fall back to deleting the knowledge base (restored later
                // from the recovery snapshot).
                tracing::debug!("unlink PUT rejected ({e:#}); deleting {kb_ref} instead");
                remote
                    .delete(&kb_ref)
                    .await
                    .with_context(|| step("while unlinking knowledge bases"))?;
                println!("      deleted {kb_ref} (empty after unlink; restored afterwards)");
            }
            Err(e) => {
                return Err(e.context(step("while unlinking knowledge bases")));
            }
        }
    }

    // 4. Delete the old knowledge source; Azure cascades the generated
    // pipeline away, so those baselines are gone too.
    remote
        .delete(ks)
        .await
        .with_context(|| step("after unlinking knowledge bases"))?;
    state.clear_baseline(ks);
    for (kind, name) in migrate::created_resources(&bundle.remote_ks) {
        state.clear_baseline(&ResourceRef::new(kind, name));
    }
    state.save(ws, &env.name, &project.name)?;
    println!("      deleted old {ks} (generated pipeline cascaded)");

    // 5. Re-create the explicit pipeline in dependency order.
    let order = graph::push_order(&bundle.sub)?;
    for r in &order {
        let (_, body) = bundle
            .sub
            .iter()
            .find(|(sr, _)| sr == r)
            .expect("ordered item");
        let mut with_refs = body.clone();
        resolve_cross_service_refs(env.search_for(project).ok(), &mut with_refs)?;
        let push_body = normalize_for_push(r.kind, &with_refs);
        let server_doc = put_with_rbac_help(remote, r, &push_body, rbac_patience)
            .await
            .with_context(|| step(&format!("while re-creating {r}")))?;
        store.write(r, &server_doc)?;
        state.set_baseline(r, &server_doc);
        state.save(ws, &env.name, &project.name)?;
        println!("      {} {}", "✓".green(), r);
    }

    // 6. Create the new knowledge source.
    let mut with_refs = bundle.new_body.clone();
    resolve_cross_service_refs(env.search_for(project).ok(), &mut with_refs)?;
    let push_body = normalize_for_push(ks.kind, &with_refs);
    let server_doc = put_with_rbac_help(remote, ks, &push_body, rbac_patience)
        .await
        .with_context(|| step("while re-creating the knowledge source"))?;
    store.write(ks, &server_doc)?;
    state.set_baseline(ks, &server_doc);
    state.save(ws, &env.name, &project.name)?;
    println!("      {} {} (kind: searchIndex)", "✓".green(), ks);

    // 7. Restore the knowledge bases exactly as snapshotted.
    relink_knowledge_bases(remote, store, state, &referencing)
        .await
        .with_context(|| step("while restoring knowledge base links"))?;
    state.save(ws, &env.name, &project.name)?;
    std::fs::remove_file(&recovery).ok();
    if !referencing.is_empty() {
        println!(
            "      {} restored {} knowledge base link(s)",
            "✓".green(),
            referencing.len()
        );
    }
    println!(
        "      {} index is repopulating — knowledge bases may return thin results until the indexer finishes",
        "ℹ".cyan()
    );
    Ok(())
}
