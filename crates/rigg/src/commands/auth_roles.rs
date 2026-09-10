//! `rigg auth roles list|remove` (spec §4.4) — the role assignments rigg
//! itself created, found by the description prefix it stamps on them
//! (`rigg:<workspace>:<env>`) at every scope the environment's identity
//! graph knows about.
//!
//! `rigg env remove --clean-roles` calls [`remove`] before dropping the
//! environment, so tearing an environment down does not leave orphaned
//! assignments behind on shared infrastructure.

use std::collections::BTreeSet;

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::json;

use rigg_client::arm::ArmClient;
use rigg_client::arm_reads::RoleAssignmentInfo;
use rigg_core::identity::{Scope, graph_for_docs, operator_edges};
use rigg_core::resources::ResourceKind;
use rigg_core::workspace::{ResolvedEnv, Workspace};

use crate::commands::ask::Question;
use crate::commands::auth_engine;
use crate::commands::{
    CommandError, GlobalContext, confirm_protected_env, load_workspace, resolve_env,
};
use crate::say;

/// One rigg-created assignment, with where it lives.
struct Found {
    scope: String,
    assignment: RoleAssignmentInfo,
}

/// Every distinct resolved scope the environment's graph mentions — the
/// places rigg could ever have created an assignment. The Foundry project
/// id is resolved exactly as `auth doctor` resolves it, so the assignment
/// doctor creates at `<account>/projects/<project>` is reachable here.
async fn scopes_of(
    ws: &Workspace,
    env: &ResolvedEnv,
    bindings: &rigg_core::binding::EnvBindings,
    arm: &ArmClient,
) -> Vec<String> {
    let docs = auth_engine::env_documents(ws, &env.name);
    let graph = graph_for_docs(bindings, &docs);
    let kinds: Vec<ResourceKind> = docs.iter().map(|(k, ..)| *k).collect();
    let project = auth_engine::foundry_project_id(env, bindings, Some(arm)).await;
    let (operator, _) = operator_edges(bindings, &kinds, true, &[], project.as_deref());

    let mut scopes: BTreeSet<String> = BTreeSet::new();
    let from_scope = |s: &Scope| s.arm_id().map(str::to_string);
    for edge in graph.edges.iter().chain(operator.iter()) {
        if let Some(id) = from_scope(&edge.scope) {
            scopes.insert(id);
        }
    }
    for check in &graph.checks {
        if let Some(id) = auth_engine::check_scope(check).and_then(from_scope) {
            scopes.insert(id);
        }
    }
    scopes.into_iter().collect()
}

async fn find(
    ctx: &GlobalContext,
    remove: bool,
) -> Result<(Vec<Found>, ArmClient, String, ResolvedEnv)> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let arm = ArmClient::for_tenant(env.env.tenant.as_deref())
        .map_err(|e| anyhow!(CommandError::AuthDenied(format!("{e}"))))?;
    let bindings = auth_engine::bindings_for(&ws, &env, Some(&arm)).await;
    let prefix = auth_engine::description_prefix(&ws, &env.name);
    // The trailing colon is load-bearing: `rigg:acme:dev` is a prefix of
    // `rigg:acme:dev2:…`, and a neighbouring environment's grants are not
    // this environment's to list — let alone to delete.
    let filter = auth_engine::role_description(&prefix, "");

    let scopes = scopes_of(&ws, &env, &bindings, &arm).await;
    if !remove {
        say!(
            ctx,
            "{} assignments described '{prefix}:…' across {} scope(s) (env: {})",
            "auth roles".bold(),
            scopes.len(),
            env.name
        );
    }
    let mut found = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for scope in scopes {
        match arm.list_rigg_role_assignments(&scope, &filter).await {
            Ok(list) => {
                for assignment in list {
                    // One assignment can answer at more than one of the
                    // scopes the graph knows (nested scopes, or the same
                    // scope reached two ways): one row, one DELETE.
                    if !seen.insert(assignment.id.to_ascii_lowercase()) {
                        continue;
                    }
                    found.push(Found {
                        scope: scope.clone(),
                        assignment,
                    });
                }
            }
            Err(e) => say!(ctx, "  {} {scope}: {e}", "?".yellow()),
        }
    }
    Ok((found, arm, prefix, env))
}

/// `rigg auth roles list`.
pub async fn list(ctx: &GlobalContext) -> Result<()> {
    let (found, ..) = find(ctx, false).await?;
    if ctx.json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!(
                found
                    .iter()
                    .map(|f| json!({
                        "scope": f.scope,
                        "id": f.assignment.id,
                        "role": f.assignment.role_guid(),
                        "description": f.assignment.description,
                    }))
                    .collect::<Vec<_>>()
            ))?
        );
        return Ok(());
    }
    if found.is_empty() {
        say!(ctx, "  (rigg has not created any role assignments here)");
        return Ok(());
    }
    for f in &found {
        say!(ctx, "  {} {}", "•".dimmed(), f.assignment.description);
        say!(ctx, "      role:  {}", f.assignment.role_guid());
        say!(ctx, "      scope: {}", f.scope);
    }
    say!(ctx);
    say!(
        ctx,
        "{} assignment(s) — remove them with `rigg auth roles remove`",
        found.len()
    );
    Ok(())
}

/// `rigg auth roles remove [--confirm-env <env>]`.
pub async fn remove(ctx: &GlobalContext, confirm_env: Option<&str>) -> Result<()> {
    remove_inner(ctx, confirm_env, true).await
}

/// `gate` is false only for `rigg env remove --clean-roles`, which has its
/// own ruling: the flag names the removal, and the environment is going away
/// anyway.
async fn remove_inner(ctx: &GlobalContext, confirm_env: Option<&str>, gate: bool) -> Result<()> {
    let (found, arm, prefix, resolved) = find(ctx, true).await?;
    let env = resolved.name.clone();
    if found.is_empty() {
        say!(ctx, "no rigg-created role assignments in '{env}'");
        if ctx.json() {
            println!("{}", serde_json::to_string_pretty(&json!({"removed": []}))?);
        }
        return Ok(());
    }
    say!(
        ctx,
        "{} {} assignment(s) described '{prefix}:…' (env: {env}):",
        "auth roles remove".bold(),
        found.len()
    );
    for f in &found {
        say!(ctx, "  {} @ {}", f.assignment.role_guid(), f.scope);
    }
    // Protected-environment gate before the first DELETE: removing a role
    // assignment is a change to the environment, so it sits behind the same
    // typed confirmation as `push` — `--yes` does not satisfy it.
    if gate
        && !confirm_protected_env(
            ctx,
            &resolved,
            confirm_env,
            "auth roles remove",
            "auth roles remove",
            json!({"env": env, "assignments": found.len()}),
        )?
    {
        say!(ctx, "Aborted.");
        return Ok(());
    }
    if !ctx.yes {
        let mut asker = ctx.asker(
            "auth roles remove",
            json!({"env": env, "assignments": found.len()}),
        );
        let answer = asker.ask(&Question::confirm(
            "auth.roles.remove",
            format!("Remove {} role assignment(s)?", found.len()),
            false,
        ))?;
        if answer.as_bool() != Some(true) {
            say!(ctx, "no changes made");
            return Ok(());
        }
    }
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for f in &found {
        match arm.delete_role_assignment(&f.assignment.id).await {
            Ok(()) => removed.push(f.assignment.id.clone()),
            Err(e) => failed.push(format!("{}: {e}", f.assignment.id)),
        }
    }
    if ctx.json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"removed": removed, "failed": failed}))?
        );
    } else {
        say!(ctx, "{} removed, {} failed", removed.len(), failed.len());
        for e in &failed {
            say!(ctx, "  {} {e}", "✗".red());
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(CommandError::AuthDenied(format!(
            "{} role assignment(s) could not be removed",
            failed.len()
        ))))
    }
}

/// `rigg env remove --clean-roles`: remove `env`'s rigg-created assignments
/// before the environment itself goes away. Reported, never fatal — the
/// environment must still be removable when the caller has no rights on the
/// scopes any more.
pub async fn clean_for_env(ctx: &GlobalContext, env: &str) -> Result<()> {
    let mut scoped = ctx.clone();
    scoped.env = Some(env.to_string());
    // `--clean-roles` is itself the consent: the flag names the removal, and
    // asking again from inside `env remove` would only strand the caller on
    // exit 6 in a script that already said what it wanted.
    scoped.yes = true;
    match remove_inner(&scoped, None, false).await {
        Ok(()) => Ok(()),
        Err(e) => {
            say!(ctx, "  (could not clean role assignments: {e:#})");
            Ok(())
        }
    }
}
