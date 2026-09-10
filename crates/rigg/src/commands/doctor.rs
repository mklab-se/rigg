//! `rigg auth doctor` (spec §4.1) — verify, explain and repair the identity
//! graph an environment needs. Thin over [`super::auth_engine`]: the
//! verification, the rendering and the fixes are shared with `rigg push`'s
//! auth preflight and `rigg status --auth`.

use anyhow::{Result, anyhow};
use colored::Colorize;
use rigg_client::arm::ArmClient;

use crate::commands::ask::Question;
use crate::commands::auth_engine::{self, Status, VerifyOpts, VerifyScope};
use crate::commands::{
    CommandError, GlobalContext, confirm_protected_env, load_workspace, resolve_env,
};
use crate::say;

/// `rigg auth doctor [--fix] [--principal <id>] [--plan] [--live]
/// [--confirm-env <env>]`.
pub async fn run(
    ctx: &GlobalContext,
    fix: bool,
    principal: Option<String>,
    plan: bool,
    live: bool,
    confirm_env: Option<&str>,
) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;

    let scope = if plan {
        VerifyScope::Plan(auth_engine::plan_documents(&ws, &env).await?)
    } else {
        VerifyScope::EnvTree
    };
    let report = auth_engine::verify(
        ctx,
        &ws,
        &env,
        scope,
        VerifyOpts {
            principal,
            verify_roles: false,
            live,
        },
    )
    .await?;

    let fixes = if fix { report.fixes() } else { Vec::new() };

    // The confirmation is one question for the whole batch (spec §4.2's "one
    // confirmation per fix class, not per edge", taken to its conclusion —
    // the list is right there in the report).
    //
    // In text mode the report is printed first, so a human sees what they
    // are agreeing to. In `--output json` it must come *after*: an
    // unanswered question emits the `needs-input` document on stdout, and
    // stdout may carry only one JSON document per run.
    let confirm = |ctx: &GlobalContext| -> Result<bool> {
        if fixes.is_empty() {
            return Ok(true);
        }
        // Printed whether or not an answer is needed: `--yes` should still
        // say what it is about to change.
        say!(ctx);
        say!(ctx, "{} rigg can fix:", fixes.len());
        for f in &fixes {
            say!(ctx, "  - {}", f.describe());
        }
        // Protected-environment gate, before the batch confirmation and
        // therefore before any write: a role assignment, a search service's
        // auth options or a storage firewall rule is a change to the
        // environment, so `--fix` sits behind the same typed confirmation as
        // `push` — and `--yes` deliberately does not satisfy it. Asked here
        // rather than after the report so `--output json` still emits at
        // most one document (the `needs-input` one).
        if !confirm_protected_env(
            ctx,
            &env,
            confirm_env,
            "auth doctor --fix",
            "auth doctor --fix",
            serde_json::json!({"env": env.name, "fixes": fixes.len()}),
        )? {
            return Ok(false);
        }
        if ctx.yes {
            return Ok(true);
        }
        let mut asker = ctx.asker(
            "auth doctor --fix",
            serde_json::json!({"env": env.name, "fixes": fixes.len()}),
        );
        Ok(asker
            .ask(&Question::confirm(
                "auth.fix.all",
                format!("Apply {} fix(es)?", fixes.len()),
                true,
            ))?
            .as_bool()
            == Some(true))
    };

    let approved = if ctx.json() {
        let approved = confirm(ctx)?;
        println!(
            "{}",
            serde_json::to_string_pretty(&auth_engine::to_json(&report))?
        );
        approved
    } else {
        auth_engine::render_text(&report, fix);
        confirm(ctx)?
    };

    if !fix || fixes.is_empty() {
        return verdict(ctx, &report, &env.name).await;
    }
    if !approved {
        say!(ctx, "no changes made");
        return verdict(ctx, &report, &env.name).await;
    }

    let arm = ArmClient::for_tenant(env.env.tenant.as_deref())
        .map_err(|e| anyhow!(CommandError::AuthDenied(format!("{e}"))))?;
    let results = auth_engine::apply(ctx, &arm, &fixes).await?;
    let failed: Vec<String> = results
        .iter()
        .filter_map(|(f, r)| r.as_ref().err().map(|e| format!("{}: {e}", f.describe())))
        .collect();

    say!(ctx);
    let applied = results.len() - failed.len();
    say!(ctx, "{applied} fix(es) applied, {} failed", failed.len());
    let unfixable = report.unfixable();
    let unresolved = report.unresolved();
    for item in unfixable.iter().chain(unresolved.iter()) {
        say!(ctx, "  {} {} — {}", "✗".red(), item.headline(), item.detail);
    }
    if failed.is_empty() && unfixable.is_empty() && unresolved.is_empty() {
        say!(
            ctx,
            "{} re-run `rigg auth doctor` to confirm (role assignments take a moment to \
             propagate)",
            "✓".green().bold()
        );
        return Ok(());
    }
    // Anything rigg could not fix — or could not judge in the first place,
    // which a fix does not turn into a verdict — keeps the exit code at 4.
    Err(anyhow!(CommandError::AuthDenied(format!(
        "{} problem(s) remain after --fix (re-run `rigg auth doctor -e {}` to re-verify): {}",
        failed.len() + unfixable.len() + unresolved.len(),
        env.name,
        failed
            .iter()
            .cloned()
            .chain(
                unfixable
                    .iter()
                    .chain(unresolved.iter())
                    .map(|i| i.headline())
            )
            .collect::<Vec<_>>()
            .join("; ")
    ))))
}

/// Exit 0 when everything is in place, 4 otherwise (spec §4.1.7).
async fn verdict(ctx: &GlobalContext, report: &auth_engine::Report, env: &str) -> Result<()> {
    if report.summary.clean() {
        say!(ctx, "{} identity wiring is complete", "✓".green().bold());
        return Ok(());
    }
    // Optional ailloy commentary on what the failures have in common.
    let failures: Vec<String> = report
        .items
        .iter()
        .chain(report.operator.iter())
        .filter(|i| matches!(i.status, Status::Missing | Status::Unresolved))
        .map(|i| format!("{} — {}", i.headline(), i.detail))
        .collect();
    if !failures.is_empty()
        && crate::commands::ai_assist::ai_on(ctx)
        && let Ok(advice) = crate::commands::ai_assist::explain_doctor(&failures).await
    {
        say!(ctx);
        say!(ctx, "AI advice (ailloy):");
        for line in advice.lines() {
            say!(ctx, "  {line}");
        }
    }
    let hint = if report.fixes().is_empty() {
        format!("run the printed az commands (env: {env})")
    } else {
        format!("re-run with --fix, or run the printed az commands (env: {env})")
    };
    Err(anyhow!(CommandError::AuthDenied(format!(
        "{} missing, {} unresolved{} — {hint}",
        report.summary.missing,
        report.summary.unresolved,
        match report.summary.live {
            0 => String::new(),
            n => format!(", {n} live finding(s)"),
        }
    ))))
}
