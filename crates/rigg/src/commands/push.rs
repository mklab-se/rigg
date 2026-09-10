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

use rigg_core::binding::{BindingCache, EnvBindings};
use rigg_core::infra;
use rigg_core::normalize::{normalize_for_compare, normalize_for_push};
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::{ProjectState, Store, SyncClass, assert_exclusive_ownership, baseline_doc};
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};
use rigg_core::{graph, migrate, registry};

use crate::cli::PushArgs;
use crate::commands::ask::Question;
use crate::commands::auth_engine::{self, Fix, ReportItem, Status, VerifyOpts, VerifyScope};
use crate::commands::credentials;
use crate::commands::infra_report::{self, Level};
use crate::commands::remote::{Remote, ensure_any_connection, resolve_cross_service_refs};
use crate::commands::{
    CommandError, GlobalContext, confirm_protected_env, interactive, load_workspace, resolve_env,
    select_projects, verify,
};
use crate::say;

pub async fn run(ctx: &GlobalContext, args: PushArgs) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    assert_exclusive_ownership(&ws, &env.name)?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;

    let mut any_conflict = false;
    for project in &projects {
        any_conflict |= push_project(ctx, &ws, &env, project, &args).await?;
    }
    // `--verify` proves the stack works, so it runs after every project has
    // landed — and only then: verifying half a plan proves nothing, and a
    // dry run wrote nothing to verify.
    if args.verify && !any_conflict && !args.dry_run {
        say!(ctx);
        verify::run_for(ctx, &ws, &env, &projects).await?;
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

    say!(
        ctx,
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
    for line in remote.target_lines() {
        say!(ctx, "{line}");
    }

    // Collect local resources.
    let local_files = store.list()?;
    let mut items: Vec<(ResourceRef, Value)> = Vec::new();
    for (r, _) in &local_files {
        items.push((r.clone(), store.read(r)?));
    }

    // Leftover relink obligations from an interrupted replace (see
    // execute_replace): ks name → original knowledge-base docs.
    let mut pending_relinks = load_pending_relinks(ctx, ws, &env.name, &project.name)?;

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
            SyncClass::InSync => {
                // Azure redacts stored secrets on every GET, so "in sync"
                // cannot certify the remote key of a Web API skill — but it
                // cannot condemn it either (issue #5). Ordinary pushes leave
                // in-sync resources alone; `--refresh-credentials` makes the
                // re-authorization explicit: annotated skills re-PUT with a
                // freshly fetched key, unresolved redacted ones enter the
                // auth gate like any planned mutation.
                if args.refresh_credentials
                    && r.kind == ResourceKind::Skillset
                    && (body.to_string().contains("\"x-rigg-auth\"")
                        || !credentials::webapi_skills_missing_auth(body).is_empty())
                {
                    to_push.push(PlanItem {
                        r: r.clone(),
                        body: body.clone(),
                        exists_remotely: remote_doc.is_some(),
                    });
                }
            }
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
        if let Some(r) = parse_key(key)
            && remote.supported_kinds().contains(&r.kind)
            && remote.get(&r).await?.is_some()
        {
            orphans.push(r);
        }
    }

    // Web API skills with unusable (redacted) auth are gated ONLY when this
    // push would actually PUT them — like the other credential preflights.
    // For everything else (typically in-sync resources) Azure's redaction is
    // routine secret hygiene, not evidence of a broken key (issue #5): those
    // are surfaced as a non-blocking note pointing at --refresh-credentials.
    let mut webapi_missing: Vec<ResourceRef> = Vec::new();
    for item in &to_push {
        if item.r.kind == ResourceKind::Skillset
            && !credentials::webapi_skills_missing_auth(&item.body).is_empty()
        {
            webapi_missing.push(item.r.clone());
        }
    }
    for bundle in &replaces {
        for (r, body) in &bundle.sub {
            if r.kind == ResourceKind::Skillset
                && !credentials::webapi_skills_missing_auth(body).is_empty()
            {
                webapi_missing.push(r.clone());
            }
        }
    }
    let mut webapi_unknown: Vec<ResourceRef> = Vec::new();
    for (r, body) in &items {
        if r.kind == ResourceKind::Skillset
            && !webapi_missing.contains(r)
            && !credentials::webapi_skills_missing_auth(body).is_empty()
        {
            webapi_unknown.push(r.clone());
        }
    }
    let print_webapi_unknown_notes = |unknown: &[ResourceRef]| {
        for r in unknown {
            say!(
                ctx,
                "  note: {r} has a server-redacted Web API key (normal on Azure GETs, says nothing about the stored key) — if enrichment is failing, run `rigg push --refresh-credentials` to re-authorize"
            );
        }
    };

    // Report the plan. webapi_missing is empty whenever the plan is empty
    // (it only flags planned mutations), so an ordinary no-op push stays a
    // no-op — in-sync redacted skills get the note, nothing more.
    if to_push.is_empty() && orphans.is_empty() && conflicts.is_empty() && replaces.is_empty() {
        print_webapi_unknown_notes(&webapi_unknown);
        if !pending_relinks.is_empty() && !args.dry_run {
            // Same refusal semantics as the main plan's preflight: relink
            // bodies (restored knowledge-base docs) can carry infrastructure
            // references too, and they are about to be PUT.
            let relink_bodies: Vec<(ResourceRef, Value)> = pending_relinks
                .values()
                .flatten()
                .filter_map(|kb| {
                    kb.get("name").and_then(Value::as_str).map(|name| {
                        (
                            ResourceRef::new(ResourceKind::KnowledgeBase, name.to_string()),
                            kb.clone(),
                        )
                    })
                })
                .collect();
            binding_preflight(
                ctx,
                ws,
                env,
                relink_bodies.iter().map(|(r, b)| (r, b)),
                false,
            )?;
            finish_pending_relinks(
                ctx,
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
        say!(ctx, "  {} everything in sync", "✓".green());
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
        say!(ctx, "  {} {}", verb.cyan(), r);
    }
    for bundle in &replaces {
        let (path, remote_val, local_val) = &bundle.diff[0];
        say!(
            ctx,
            "  {} {}   {}: {} → {}",
            "replace".magenta().bold(),
            bundle.ks,
            path,
            remote_val,
            local_val
        );
        say!(
            ctx,
            "      {} deletes the knowledge source AND its generated pipeline, then",
            "⚠".yellow()
        );
        say!(
            ctx,
            "        recreates it explicitly. The index is REBUILT from source data:"
        );
        say!(
            ctx,
            "        this takes time, costs ingestion/embeddings, and the source is"
        );
        say!(
            ctx,
            "        unavailable to knowledge bases until repopulated."
        );
        if !bundle.sub.is_empty() {
            let names: Vec<String> = bundle.sub.iter().map(|(r, _)| r.to_string()).collect();
            say!(ctx, "      recreates: {}", names.join(", "));
        }
    }
    for r in &skipped_remote_ahead {
        say!(
            ctx,
            "  {} {} (remote changed since last sync — pull first)",
            "skip".yellow(),
            r
        );
    }
    for r in &conflicts {
        say!(
            ctx,
            "  {} {} (both local and remote changed)",
            "conflict".red().bold(),
            r
        );
    }
    for r in &orphans {
        if args.prune {
            say!(ctx, "  {} {}", "delete".red(), r);
        } else {
            say!(
                ctx,
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
        say!(
            ctx,
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
        if item.r.kind == ResourceKind::Skillset
            && !item.exists_remotely
            && let Some(subdomain) = credentials::skillset_missing_ai_services_key(&item.body)
        {
            key_missing.push((item.r.clone(), subdomain));
        }
    }
    for bundle in &replaces {
        for (r, body) in &bundle.sub {
            if r.kind == ResourceKind::Skillset
                && let Some(subdomain) = credentials::skillset_missing_ai_services_key(body)
            {
                key_missing.push((r.clone(), subdomain));
            }
        }
    }
    for (r, _) in &key_missing {
        say!(
            ctx,
            "  {} {} has a key-based cognitiveServices connection without a usable key — switch to identity-based (AIServicesByIdentity)",
            "!".yellow(),
            r
        );
    }

    // Custom Web API skills whose outgoing body would carry an unusable
    // (redacted) key: PUTting the literal placeholder breaks enrichment for
    // every document — resolve now (Entra ID or push-time key), not then.
    for r in &webapi_missing {
        say!(
            ctx,
            "  {} {} would push a custom Web API skill with an unusable (redacted) key — authorize it first (Entra ID or push-time function key)",
            "!".yellow(),
            r
        );
    }
    print_webapi_unknown_notes(&webapi_unknown);

    // Binding preflight: classify every infrastructure reference in every
    // body this push would write, exactly as `rigg validate` does, and
    // refuse before a single mutation when one of them belongs to another
    // environment (a leak) — or, in a strict-bindings environment, is bound
    // nowhere at all. Runs before the protected gate so the refusal is the
    // first thing a wrong-environment push hits, and before the dry-run
    // early return so a preview reports the same findings (without
    // refusing) instead of showing a clean plan for a push that would fail
    // one command later.
    let mut preflight_bodies: Vec<(&ResourceRef, &Value)> =
        to_push.iter().map(|p| (&p.r, &p.body)).collect();
    for bundle in &replaces {
        preflight_bodies.push((&bundle.ks, &bundle.new_body));
        preflight_bodies.extend(bundle.sub.iter().map(|(r, body)| (r, body)));
    }
    binding_preflight(ctx, ws, env, preflight_bodies, args.dry_run)?;

    // Auth preflight: verify the identity graph THIS plan implies before a
    // single resource is written (spec §4.2). Placed after the binding
    // preflight (a wrong-environment push must be caught first — there is no
    // point granting roles for a plan that will be refused) and before the
    // credential preflights and the protected gate, so a refusal is the
    // first thing a push that cannot work hits. The *repair* it proposes is
    // applied later, past every gate that can still abort — see
    // `apply_auth_repair` below.
    let mut plan_docs: Vec<(ResourceKind, String, Value)> = to_push
        .iter()
        .map(|p| (p.r.kind, p.r.name.clone(), p.body.clone()))
        .collect();
    for bundle in &replaces {
        plan_docs.push((
            bundle.ks.kind,
            bundle.ks.name.clone(),
            bundle.new_body.clone(),
        ));
        plan_docs.extend(
            bundle
                .sub
                .iter()
                .map(|(r, body)| (r.kind, r.name.clone(), body.clone())),
        );
    }
    let auth_repair = auth_preflight(ctx, ws, env, plan_docs, args).await?;

    if args.dry_run {
        say!(ctx, "  (dry run — nothing pushed)");
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
                    say!(
                        ctx,
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
                // Keyless billing only works against Foundry-kind resources;
                // verify (and if needed re-target) before offering the switch.
                let Some(subdomain) =
                    credentials::resolve_ai_services_billing_target(&subdomain, ctx.no_color)
                        .await?
                else {
                    continue;
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
                    say!(
                        ctx,
                        "  {} {} switched to AIServicesByIdentity",
                        "✓".green(),
                        r
                    );
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

    if !webapi_missing.is_empty() {
        if ctx.interactive() {
            let pending = webapi_missing.clone();
            for r in pending {
                let mut doc = store.read(&r)?;
                let mut resolved_any = false;
                let mut all_resolved = true;
                for idx in credentials::webapi_skills_missing_auth(&doc) {
                    match credentials::resolve_webapi_auth(ctx, &mut doc, idx, &r.to_string())
                        .await?
                    {
                        credentials::WebApiAuthOutcome::Skipped => all_resolved = false,
                        _ => resolved_any = true,
                    }
                }
                if resolved_any {
                    store.write(&r, &doc)?;
                    let mut placed = false;
                    if let Some(item) = to_push.iter_mut().find(|p| p.r == r) {
                        item.body = doc.clone();
                        placed = true;
                    }
                    for bundle in &mut replaces {
                        if let Some((_, body)) = bundle.sub.iter_mut().find(|(sr, _)| *sr == r) {
                            *body = doc.clone();
                            placed = true;
                        }
                    }
                    if !placed {
                        // In-sync-but-broken: force the fixed document out.
                        to_push.push(PlanItem {
                            r: r.clone(),
                            body: doc.clone(),
                            exists_remotely: true,
                        });
                    }
                    fixed_credentials = true;
                }
                if all_resolved {
                    webapi_missing.retain(|x| x != &r);
                }
            }
            for r in &webapi_missing {
                say!(
                    ctx,
                    "  {} {} left WITHOUT Web API authorization — its enrichment will fail until fixed",
                    "!".yellow(),
                    r
                );
            }
        } else {
            // webapi_missing only ever holds planned mutations, so blocking
            // here can never trip on an untouched in-sync resource.
            let blocking: Vec<String> = webapi_missing.iter().map(|r| r.to_string()).collect();
            return Err(anyhow!(CommandError::Validation(format!(
                "{} would push a custom Web API skill with an unusable (redacted) key — run \
                 `rigg push` interactively to choose Entra ID auth (authResourceId) or a \
                 push-time-resolved function key",
                blocking.join(", ")
            ))));
        }
    }

    // The connections just fixed need data-plane roles the identities may
    // not have yet — offer to verify/grant them right here instead of
    // hinting and letting the push run into a predictable 400.
    if fixed_credentials && ctx.interactive() {
        say!(ctx);
        if interactive::confirm_default_yes(
            "Verify and grant the roles these connections need now (runs auth doctor --fix)?",
            ctx.no_color,
        )? && let Err(e) =
            crate::commands::doctor::run(ctx, true, None, false, false, args.confirm_env.as_deref())
                .await
        {
            say!(
                ctx,
                "  {} auth doctor could not fix everything ({e:#}) — continuing; the push may fail until the roles exist",
                "!".yellow()
            );
        }
    }

    // Protected-env gate: fires after the plan is built and displayed (the
    // "explain, then act" rule — show what would happen, then ask), and
    // before any mutating call to the environment's resources (creates/
    // updates below, and the --prune deletion path), and before the routine
    // apply confirmation so a
    // rejected/missing typed confirmation short-circuits everything that
    // follows. Dry runs never reach here — they return above, before this
    // point, so previewing a protected env's plan without confirming is
    // still the whole point of `--dry-run`.
    if !confirm_protected_env(
        ctx,
        env,
        args.confirm_env.as_deref(),
        "push",
        "push",
        serde_json::json!({"project": project.name, "env": env.name}),
    )? {
        say!(ctx, "Aborted.");
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
                say!(ctx, "  aborted");
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
            say!(ctx, "  aborted");
            return Ok(false);
        }
    } else if !ctx.yes {
        return Err(anyhow!(CommandError::Usage(
            "non-interactive push requires --yes".to_string()
        )));
    }

    // Apply what the auth preflight found — with consent, and then waited
    // out. Deliberately *after* every gate that can still abort (the
    // protected-environment gate, the replace gate, the apply confirmation):
    // the preflight's refusal has to come before them so a doomed push is
    // stopped early, but its *writes* must not. A role assignment or a
    // storage network rule is a change to the environment, and a push the
    // operator never confirmed has to leave it exactly as it found it. Still
    // before the first `put_with_rbac_help`, so every write below sees the
    // access the plan needs.
    if let Some(repair) = auth_repair {
        apply_auth_repair(ctx, env, repair).await?;
    }

    // Interactive conflict handling: choose local/remote/skip per conflict.
    for r in &conflicts {
        let local = store.read(r)?;
        let remote_doc = remote.get(r).await?.unwrap_or(Value::Null);
        say!(ctx);
        say!(ctx, "{} {}", "Conflict:".red().bold(), r);
        // `normalize_for_compare`, matching what classified this as a
        // conflict: it also strips the write-only fields Azure redacts, so
        // the diff does not open with a phantom `connectionString: null → …`.
        let diff = rigg_diff::semantic::diff(
            &normalize_for_compare(r.kind, &remote_doc),
            &normalize_for_compare(r.kind, &local),
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
                state.set_baseline(r, &baseline_doc(r.kind, &remote_doc, Some(&local)));
                say!(ctx, "  kept remote version for {r}");
            }
            AI_MERGE => {
                say!(ctx, "  asking ailloy for a merge proposal...");
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
                        say!(ctx, "  proposal vs LOCAL:");
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
                        say!(ctx, "  proposal vs REMOTE:");
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
                            say!(ctx, "  discarded proposal; skipped {r}");
                        }
                    }
                    Err(e) => say!(ctx, "  AI merge failed ({e}); skipped {r}"),
                }
            }
            _ => say!(ctx, "  skipped {r}"),
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
        resolve_cross_service_refs(env.search(), &mut with_refs)?;
        let carriers = credentials::inject_function_keys(&mut with_refs, ws, env).await?;
        let body = normalize_for_push(r.kind, &with_refs);

        match put_with_rbac_help(&remote, r, &body, ctx, ws, env, args.verify).await {
            Ok(mut server_doc) => {
                // The echo may carry the injected key back; the local
                // placeholders go back in before anything is persisted.
                credentials::restore_key_carriers(&mut server_doc, &carriers);
                store.write(r, &server_doc)?;
                state.set_baseline(r, &baseline_doc(r.kind, &server_doc, Some(&item.body)));
                state.save(ws, &env.name, &project.name)?;
                say!(ctx, "  {} {}", "✓".green(), r);
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
            ctx,
            env,
            ws,
            project,
            &store,
            &mut state,
            &remote,
            bundle,
            prior,
            args.verify,
        )
        .await?;
    }
    if !pending_relinks.is_empty() {
        finish_pending_relinks(
            ctx,
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
            say!(ctx, "  {} deleted {}", "✓".green(), r);
        }
    }

    state.save(ws, &env.name, &project.name)?;
    Ok(false)
}

/// Classify every infrastructure reference in the bodies this push would
/// write against `env`'s bindings (and every other environment's, for
/// leak/share detection) — the same machinery `rigg validate` runs, with the
/// same message texts ([`infra_report`]).
///
/// A leak is always an error; unbound/external references are errors in a
/// strict-bindings environment (the default for protected ones) and warnings
/// otherwise. Called before any mutating call, so an error means nothing was
/// written to Azure — except on `dry_run`, where a preview never refuses:
/// every finding (error or warning) is printed instead, so a `--dry-run`
/// never reports a clean plan for a push that would fail one command later.
fn binding_preflight<'a>(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    bodies: impl IntoIterator<Item = (&'a ResourceRef, &'a Value)>,
    dry_run: bool,
) -> Result<()> {
    let mut env_bindings: BTreeMap<String, EnvBindings> = BTreeMap::new();
    for (name, e) in &ws.config.environments {
        let cache = BindingCache::load(ws, name);
        env_bindings.insert(name.clone(), EnvBindings::of_env(name, e, Some(&cache)));
    }
    let Some(this) = env_bindings.get(&env.name) else {
        return Ok(());
    };
    let others: Vec<EnvBindings> = env_bindings
        .iter()
        .filter(|(name, _)| name.as_str() != env.name.as_str())
        .map(|(_, eb)| eb.clone())
        .collect();
    let strict = env.strict_bindings();

    let mut problems: Vec<String> = Vec::new();
    for (r, body) in bodies {
        let display = r.to_string();
        let refs = infra::extract(r.kind, body);
        if refs.is_empty() {
            continue;
        }
        for c in infra::classify(this, &others, refs) {
            let Some((level, message)) =
                infra_report::classified_finding(&c, &env.name, &display, strict)
            else {
                continue;
            };
            match level {
                Level::Error => problems.push(message),
                Level::Warning => say!(ctx, "  {} {message}", "!".yellow()),
            }
        }
    }

    if problems.is_empty() {
        return Ok(());
    }
    if dry_run {
        for p in &problems {
            say!(ctx, "  {} {p}", "✗".red());
        }
        return Ok(());
    }
    Err(anyhow!(CommandError::Validation(format!(
        "{} infrastructure binding problem(s) in the push plan for '{}'; nothing was pushed:\n  {}",
        problems.len(),
        env.name,
        problems.join("\n  ")
    ))))
}

/// A repair the auth preflight found and the push must perform before it
/// writes anything: the fixes to apply, and the missing requirements to name
/// if consent for them is refused.
struct AuthRepair {
    fixes: Vec<Fix>,
    missing: Vec<ReportItem>,
}

/// Plan-scoped auth preflight (spec §4.2): verify the identity graph the
/// bodies this push would write imply — service-identity role assignments,
/// the settings and network rules they depend on, and the operator's own
/// rights — before a single resource is written.
///
/// This is the **check** half only. Anything rigg may not repair — the
/// operator's own rights, which rigg never grants itself — refuses here,
/// with exit 4 and the `az` line to run, so the refusal lands before the
/// protected-environment gate and before anything else is asked. What rigg
/// *can* repair is reported and handed back as an [`AuthRepair`] for
/// [`apply_auth_repair`] to consent to and apply once every gate has been
/// cleared: a preflight must never change an environment the operator has
/// not yet confirmed the push to.
///
/// `--dry-run` reports every finding — what rigg would fix and what only a
/// human can — and refuses nothing (the binding preflight's rule: a preview
/// must not show a clean plan for a push that would fail, but must not fail
/// either). `--skip-auth-preflight` skips the whole thing, for a caller who
/// cannot read ARM but knows the wiring holds.
///
/// Unresolved items never refuse: "rigg could not check this" is not
/// "this is wrong", and an environment whose ARM is unreachable must still
/// be pushable.
async fn auth_preflight(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    docs: Vec<(ResourceKind, String, Value)>,
    args: &PushArgs,
) -> Result<Option<AuthRepair>> {
    if args.skip_auth_preflight || docs.is_empty() {
        return Ok(None);
    }
    let report = auth_engine::verify(
        ctx,
        ws,
        env,
        VerifyScope::Plan(docs),
        VerifyOpts {
            principal: None,
            // `--verify` exercises the data plane afterwards, so the roles
            // that reads need become part of what the preflight requires.
            verify_roles: args.verify,
            live: false,
        },
    )
    .await?;

    let missing: Vec<&ReportItem> = report
        .items
        .iter()
        .chain(report.operator.iter())
        .filter(|i| i.status == Status::Missing)
        .collect();
    if missing.is_empty() {
        return Ok(None);
    }

    say!(ctx);
    say!(
        ctx,
        "  {} auth preflight: {} requirement(s) missing for this plan",
        "!".yellow(),
        missing.len()
    );
    for item in &missing {
        say!(
            ctx,
            "    {} {} — {}",
            "✗".red(),
            item.headline(),
            item.detail
        );
    }

    let fixes = report.fixes();
    let unfixable = report.unfixable();

    // The `az` lines for what only a human may grant, then what rigg would
    // repair itself. Both are printed on every path, `--dry-run` included,
    // so a preview shows the whole remediation up front instead of half of
    // it.
    for item in &unfixable {
        if let Some(fix) = &item.fix {
            say!(ctx, "      {}", fix.command());
        }
    }
    if !fixes.is_empty() {
        say!(ctx);
        say!(ctx, "  rigg can fix:");
        for fix in &fixes {
            say!(ctx, "    - {}", fix.describe());
        }
    }

    if args.dry_run {
        say!(ctx, "  (dry run — nothing granted)");
        return Ok(None);
    }

    // Operator rights are never granted by rigg (that would let anyone who
    // can run a push escalate their own access), so they always refuse.
    if !unfixable.is_empty() {
        return Err(refusal(env, &unfixable));
    }
    if fixes.is_empty() {
        return Err(refusal(env, &missing));
    }
    Ok(Some(AuthRepair {
        fixes,
        missing: missing.into_iter().cloned().collect(),
    }))
}

/// Consent to, apply and wait out the repairs [`auth_preflight`] found.
///
/// Called once every gate that can still abort the push has been cleared and
/// immediately before the first write, so an environment is only ever
/// changed for a push that is actually going ahead.
///
/// A fresh role assignment is not visible to the data plane the instant ARM
/// accepts it, so each granted role is polled at its scope until it shows up
/// ([`put_with_rbac_help`] remains the safety net for the data plane's own
/// propagation lag).
async fn apply_auth_repair(
    ctx: &GlobalContext,
    env: &ResolvedEnv,
    repair: AuthRepair,
) -> Result<()> {
    let AuthRepair { fixes, missing } = repair;
    let missing: Vec<&ReportItem> = missing.iter().collect();
    // `--yes` is consent for the whole push, grants included. Otherwise ask
    // `auth.fix.all` — the same question `auth doctor --fix` asks, so a
    // scripted caller can pre-answer it. Asked through `ask_all` (not `ask`)
    // so a malformed `--answer auth.fix.all=maybe` is the usage error
    // (exit 2) every other flow produces. An unanswerable question here is
    // NOT `needs-input` (exit 6): a push that cannot be made to work is an
    // auth refusal (exit 4), with the list and the escape hatch.
    let approved = ctx.yes
        || match ctx
            .asker(
                "push (auth preflight)",
                json!({"env": env.name, "fixes": fixes.len()}),
            )
            .ask_all(&[Question::confirm(
                "auth.fix.all",
                format!("Grant/apply {} fix(es) now?", fixes.len()),
                true,
            )]) {
            Ok(answers) => answers.first().and_then(|a| a.as_bool()) == Some(true),
            Err(e)
                if e.downcast_ref::<crate::commands::ask::NeedsInput>()
                    .is_some() =>
            {
                false
            }
            Err(e) => return Err(e),
        };
    if !approved {
        return Err(refusal(env, &missing));
    }

    let arm = rigg_client::arm::ArmClient::for_tenant(env.env.tenant.as_deref())
        .map_err(|e| anyhow!(CommandError::AuthDenied(format!("{e}"))))?;
    let results = auth_engine::apply(ctx, &arm, &fixes).await?;
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(f, r)| r.as_ref().err().map(|e| format!("{}: {e}", f.describe())))
        .collect();
    if !failed.is_empty() {
        return Err(anyhow!(CommandError::AuthDenied(format!(
            "{} auth fix(es) failed; nothing was pushed to '{}': {}",
            failed.len(),
            env.name,
            failed.join("; ")
        ))));
    }
    wait_for_grants(ctx, &arm, &results).await;
    Ok(())
}

/// The exit-4 refusal a preflight ends in, naming what is missing and the
/// escape hatch.
fn refusal(env: &ResolvedEnv, items: &[&ReportItem]) -> anyhow::Error {
    anyhow!(CommandError::AuthDenied(format!(
        "{} auth requirement(s) missing for this plan; nothing was pushed to '{}': {} — run `rigg auth doctor -e {} --fix`, or push with --skip-auth-preflight to try anyway",
        items.len(),
        env.name,
        items
            .iter()
            .map(|i| i.headline())
            .collect::<Vec<_>>()
            .join("; "),
        env.name
    )))
}

/// Propagation wait after a grant, env-overridable (tests set both to 0).
///
/// Shorter and more patient than [`rbac_retry_tuning`]: this polls ARM's own
/// listing (cheap, and usually consistent within a few seconds), whereas the
/// PUT-retry loop re-attempts a real write against the data plane.
fn rbac_wait_tuning() -> (u64, u32) {
    let secs = std::env::var("RIGG_RBAC_RETRY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let attempts = std::env::var("RIGG_RBAC_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(18);
    (secs, attempts)
}

/// Poll until every role assignment rigg just created is visible through
/// `atScope()` for its principal (spec §4.2 step 2). A role that never
/// appears is a warning, not a refusal — the push proceeds and
/// [`put_with_rbac_help`] catches it if it really has not landed.
async fn wait_for_grants(
    ctx: &GlobalContext,
    arm: &rigg_client::arm::ArmClient,
    results: &[(Fix, std::result::Result<(), String>)],
) {
    let granted: Vec<&Fix> = results
        .iter()
        .filter(|(_, r)| r.is_ok())
        .map(|(f, _)| f)
        .filter(|f| matches!(f, Fix::RoleAssignment { .. }))
        .collect();
    if granted.is_empty() {
        return;
    }
    let (delay, attempts) = rbac_wait_tuning();
    // The budget is per role and the polls run one role after another, so
    // the worst case is N × (delay × attempts), not one role's worth.
    let budget = delay * attempts as u64 * granted.len() as u64;
    say!(
        ctx,
        "  waiting for {} role assignment(s) to become visible (up to ~{} min)",
        granted.len(),
        budget.div_ceil(60).max(1)
    );
    for fix in granted {
        let Fix::RoleAssignment {
            scope,
            principal_id,
            role,
            ..
        } = fix
        else {
            continue;
        };
        let mut visible = false;
        for attempt in 0..=attempts {
            match arm.role_assignments_for(scope, principal_id).await {
                Ok(list) => {
                    if list
                        .iter()
                        .any(|a| a.role_guid().eq_ignore_ascii_case(role.id))
                    {
                        visible = true;
                        break;
                    }
                }
                Err(e) => say!(
                    ctx,
                    "      {} could not re-read {scope} ({e})",
                    "!".yellow()
                ),
            }
            if attempt == attempts {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
        if visible {
            say!(ctx, "      {} '{}' is visible", "✓".green(), role.name);
        } else {
            say!(
                ctx,
                "      {} '{}' is not visible yet — pushing anyway (rigg retries the write while \
                 it propagates)",
                "!".yellow(),
                role.name
            );
        }
    }
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

/// RBAC propagation tuning, env-overridable (tests set both low).
fn rbac_retry_tuning() -> (u64, u32) {
    let secs = std::env::var("RIGG_RBAC_RETRY_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);
    let attempts = std::env::var("RIGG_RBAC_MAX_RETRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    (secs, attempts)
}

/// Which roles `body` requires that the service identities do not hold —
/// the very verification `rigg auth doctor` performs, narrowed to this one
/// document. Going through the auth engine (rather than the 1.x ad-hoc ARM
/// walk) means the diagnosis sees the environment's binding table, so an
/// unresolved scope resolves the same way here as it does everywhere else.
///
/// `None` when the document implies nothing rigg can check or repair.
async fn diagnose_rbac(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    r: &ResourceRef,
    body: &Value,
    verify_roles: bool,
) -> Result<Option<auth_engine::Report>> {
    let report = auth_engine::verify(
        ctx,
        ws,
        env,
        VerifyScope::Plan(vec![(r.kind, r.name.clone(), body.clone())]),
        VerifyOpts {
            verify_roles,
            ..VerifyOpts::default()
        },
    )
    .await?;
    let anything = report
        .items
        .iter()
        .any(|i| matches!(i.status, Status::Ok | Status::Missing));
    Ok(anything.then_some(report))
}

/// PUT that treats RBAC-shaped rejections as a solvable problem instead of
/// a dead end: diagnose the required roles through ARM; grant missing ones
/// (with consent, interactively); then wait out Azure's role-assignment
/// propagation with periodic retries. Every path either succeeds or ends in
/// an error that names the exact role, scope, and next command.
#[allow(clippy::too_many_arguments)]
async fn put_with_rbac_help(
    remote: &Remote,
    r: &ResourceRef,
    body: &Value,
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    // `push --verify`'s data-plane smoke run needs the read roles too — a
    // 403 diagnosed without them names every edge but the one that explains
    // the failure.
    verify_roles: bool,
) -> Result<Value> {
    let first = match remote.put(r, body).await {
        Ok(v) => return Ok(v),
        Err(e) if is_rbac_error(&e) => e,
        Err(e) => return Err(e),
    };
    say!(
        ctx,
        "  {} {} rejected for a role/permission — diagnosing via ARM...",
        "!".yellow(),
        r
    );
    let diagnosis = match diagnose_rbac(ctx, ws, env, r, body, verify_roles).await {
        Ok(d) => d,
        Err(e) => {
            say!(ctx, "  {} diagnosis unavailable ({e:#})", "!".yellow());
            None
        }
    };
    match diagnosis {
        Some(report) if !report.fixes().is_empty() => {
            for item in report
                .items
                .iter()
                .filter(|i| i.status == Status::Missing && !i.is_operator_edge())
            {
                say!(ctx, "  {} missing: {}", "✗".red(), item.headline());
                say!(ctx, "      {}", item.detail);
            }
            let fixes = report.fixes();
            if !ctx.interactive() {
                return Err(anyhow!(CommandError::Validation(format!(
                    "{r} requires access the service identity does not hold: {} — run `rigg auth doctor --fix`",
                    fixes
                        .iter()
                        .map(Fix::describe)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))));
            }
            if !interactive::confirm_default_yes(
                "Grant the missing role(s) now (requires rights to assign roles)?",
                ctx.no_color,
            )? {
                return Err(first.context(
                    "missing role(s) left ungranted — run `rigg auth doctor --fix`, then push again",
                ));
            }
            let arm = rigg_client::arm::ArmClient::for_tenant(env.env.tenant.as_deref())?;
            let results = auth_engine::apply(ctx, &arm, &fixes).await?;
            for (fix, outcome) in &results {
                if let Err(e) = outcome {
                    return Err(anyhow!("failed to {}: {e}", fix.describe()));
                }
            }
            wait_for_grants(ctx, &arm, &results).await;
        }
        // Missing, but nothing rigg may repair: the operator's own rights,
        // or a requirement with no fix at all. Waiting cannot help here, so
        // say so and stop instead of burning five minutes in the retry loop
        // below over a problem that will still be there afterwards.
        Some(report) if !report.unfixable().is_empty() => {
            let unfixable = report.unfixable();
            for item in &unfixable {
                say!(ctx, "  {} missing: {}", "✗".red(), item.headline());
                say!(ctx, "      {}", item.detail);
                if let Some(fix) = &item.fix {
                    say!(ctx, "      {}", fix.command());
                }
            }
            return Err(first.context(format!(
                "{r} needs {} access requirement(s) rigg may not grant itself — waiting will not help; run the command(s) above (or `rigg auth doctor -e {} --fix`), then push again",
                unfixable.len(),
                env.name
            )));
        }
        Some(_) => say!(
            ctx,
            "  {} the role assignments exist — Azure is still propagating them",
            "ℹ".cyan()
        ),
        None => say!(
            ctx,
            "  {} could not verify role assignments — retrying in case a fresh grant is propagating",
            "!".yellow()
        ),
    }
    let (delay, attempts) = rbac_retry_tuning();
    say!(
        ctx,
        "  waiting for RBAC propagation — retrying every {delay}s for up to ~{} min (Ctrl-C is safe; re-running push resumes)",
        (delay * attempts as u64).div_ceil(60)
    );
    let mut last = first;
    for attempt in 1..=attempts {
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        match remote.put(r, body).await {
            Ok(v) => {
                say!(
                    ctx,
                    "  {} access propagated (attempt {attempt})",
                    "✓".green()
                );
                return Ok(v);
            }
            Err(e) if is_rbac_error(&e) => {
                say!(ctx, "  … not yet ({attempt}/{attempts})");
                last = e;
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.context(format!(
        "access had not propagated after {attempts} attempts — wait a few minutes and re-run `rigg push` (it resumes where it left off)"
    )))
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
    ctx: &GlobalContext,
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
        say!(
            ctx,
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
    ctx: &GlobalContext,
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
            say!(
                ctx,
                "  {} knowledge-sources/{ks_name} still missing remotely — push its file, then run push again to restore knowledge base links",
                "!".yellow()
            );
            continue;
        }
        let kbs = pending.remove(&ks_name).unwrap_or_default();
        relink_knowledge_bases(remote, store, state, &kbs).await?;
        state.save(ws, &env.name, &project.name)?;
        std::fs::remove_file(recovery_path(ws, &env.name, &project.name, &ks_name)).ok();
        say!(
            ctx,
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
    ctx: &GlobalContext,
    env: &ResolvedEnv,
    ws: &Workspace,
    project: &Project,
    store: &Store<'_>,
    state: &mut ProjectState,
    remote: &Remote,
    bundle: &ReplaceBundle,
    prior: Vec<Value>,
    verify_roles: bool,
) -> Result<()> {
    let ks = &bundle.ks;
    say!(ctx, "  {} {}", "replace".magenta().bold(), ks);

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
                    say!(
                        ctx,
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
            Ok(_) => say!(ctx, "      unlinked {kb_ref}"),
            Err(e) if now_empty => {
                // The service may reject an empty knowledgeSources list —
                // fall back to deleting the knowledge base (restored later
                // from the recovery snapshot).
                tracing::debug!("unlink PUT rejected ({e:#}); deleting {kb_ref} instead");
                remote
                    .delete(&kb_ref)
                    .await
                    .with_context(|| step("while unlinking knowledge bases"))?;
                say!(
                    ctx,
                    "      deleted {kb_ref} (empty after unlink; restored afterwards)"
                );
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
    say!(ctx, "      deleted old {ks} (generated pipeline cascaded)");

    // 5. Re-create the explicit pipeline in dependency order.
    let order = graph::push_order(&bundle.sub)?;
    for r in &order {
        let (_, body) = bundle
            .sub
            .iter()
            .find(|(sr, _)| sr == r)
            .expect("ordered item");
        let mut with_refs = body.clone();
        resolve_cross_service_refs(env.search(), &mut with_refs)?;
        let carriers = credentials::inject_function_keys(&mut with_refs, ws, env).await?;
        let push_body = normalize_for_push(r.kind, &with_refs);
        let mut server_doc = put_with_rbac_help(remote, r, &push_body, ctx, ws, env, verify_roles)
            .await
            .with_context(|| step(&format!("while re-creating {r}")))?;
        credentials::restore_key_carriers(&mut server_doc, &carriers);
        store.write(r, &server_doc)?;
        state.set_baseline(r, &baseline_doc(r.kind, &server_doc, Some(body)));
        state.save(ws, &env.name, &project.name)?;
        say!(ctx, "      {} {}", "✓".green(), r);
    }

    // 6. Create the new knowledge source.
    let mut with_refs = bundle.new_body.clone();
    resolve_cross_service_refs(env.search(), &mut with_refs)?;
    let push_body = normalize_for_push(ks.kind, &with_refs);
    let server_doc = put_with_rbac_help(remote, ks, &push_body, ctx, ws, env, verify_roles)
        .await
        .with_context(|| step("while re-creating the knowledge source"))?;
    store.write(ks, &server_doc)?;
    state.set_baseline(
        ks,
        &baseline_doc(ks.kind, &server_doc, Some(&bundle.new_body)),
    );
    state.save(ws, &env.name, &project.name)?;
    say!(ctx, "      {} {} (kind: searchIndex)", "✓".green(), ks);

    // 7. Restore the knowledge bases exactly as snapshotted.
    relink_knowledge_bases(remote, store, state, &referencing)
        .await
        .with_context(|| step("while restoring knowledge base links"))?;
    state.save(ws, &env.name, &project.name)?;
    std::fs::remove_file(&recovery).ok();
    if !referencing.is_empty() {
        say!(
            ctx,
            "      {} restored {} knowledge base link(s)",
            "✓".green(),
            referencing.len()
        );
    }
    say!(
        ctx,
        "      {} index is repopulating — knowledge bases may return thin results until the indexer finishes",
        "ℹ".cyan()
    );
    Ok(())
}
