//! `rigg az indexer` — run, reset, and inspect live indexers.

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::Value;

use crate::cli::{AzIndexerCommands, AzIndexerResetArgs, AzIndexerRunArgs};
use crate::commands::{CommandError, GlobalContext, confirm_protected_env, interactive};

pub async fn run(ctx: &GlobalContext, command: AzIndexerCommands) -> Result<()> {
    match command {
        AzIndexerCommands::Run(args) => run_indexer(ctx, args).await,
        AzIndexerCommands::Reset(args) => reset_indexer(ctx, args).await,
        AzIndexerCommands::Status { name } => status(ctx, &name).await,
    }
}

/// Poll interval for --watch (env-tunable for tests).
fn watch_interval() -> u64 {
    std::env::var("RIGG_WATCH_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5)
}

/// Confirm the full-reprocess cost of a reset.
fn confirm_reset(ctx: &GlobalContext, name: &str) -> Result<bool> {
    if ctx.yes {
        return Ok(true);
    }
    if !ctx.interactive() {
        return Err(anyhow!(CommandError::Usage(format!(
            "resetting '{name}' makes the next run reprocess EVERY document \
             (ingestion, skill and embedding costs) — pass --yes to confirm non-interactively"
        ))));
    }
    interactive::confirm_default_no(
        &format!(
            "Reset '{name}'? The next run reprocesses EVERY document (ingestion, skill and embedding costs)."
        ),
        ctx.no_color,
    )
}

async fn run_indexer(ctx: &GlobalContext, args: AzIndexerRunArgs) -> Result<()> {
    let (_ws, env, remote) = super::connect(ctx)?;
    if !confirm_protected_env(ctx, &env, args.confirm_env.as_deref(), "indexer run")? {
        println!("Aborted.");
        return Ok(());
    }
    if args.reset {
        if !confirm_reset(ctx, &args.name)? {
            println!("Aborted.");
            return Ok(());
        }
        remote.indexer_reset(&args.name).await?;
        println!("  {} reset {}", "✓".green(), args.name);
    }
    remote.indexer_run(&args.name).await?;
    println!("  {} triggered a run of '{}'", "✓".green(), args.name);
    if !args.watch {
        println!("  follow it with: rigg az indexer status {}", args.name);
        return Ok(());
    }

    // Watch: poll until the run reaches a terminal state.
    let interval = watch_interval();
    let mut last_state = String::new();
    for _ in 0..720 {
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
        let status = remote.indexer_status(&args.name).await?;
        let run = status.get("lastResult").cloned().unwrap_or(Value::Null);
        let state = run
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_string();
        if state != last_state {
            println!("  … {state}");
            last_state = state.clone();
        }
        match state.as_str() {
            "success" => {
                println!(
                    "  {} run completed: {} processed, {} failed",
                    "✓".green(),
                    run.get("itemsProcessed")
                        .and_then(Value::as_u64)
                        .unwrap_or(0),
                    run.get("itemsFailed").and_then(Value::as_u64).unwrap_or(0)
                );
                return Ok(());
            }
            "error" | "transientFailure" => {
                render_run_problems(&run);
                return Err(anyhow!("indexer run for '{}' ended in {state}", args.name));
            }
            _ => {} // inProgress / reset / pending — keep polling
        }
    }
    Err(anyhow!(
        "gave up watching '{}' after an hour — check `rigg az indexer status {}`",
        args.name,
        args.name
    ))
}

async fn reset_indexer(ctx: &GlobalContext, args: AzIndexerResetArgs) -> Result<()> {
    let (_ws, env, remote) = super::connect(ctx)?;
    if !confirm_protected_env(ctx, &env, args.confirm_env.as_deref(), "indexer reset")? {
        println!("Aborted.");
        return Ok(());
    }
    if !confirm_reset(ctx, &args.name)? {
        println!("Aborted.");
        return Ok(());
    }
    remote.indexer_reset(&args.name).await?;
    println!(
        "  {} reset {} — run it with: rigg az indexer run {}",
        "✓".green(),
        args.name,
        args.name
    );
    Ok(())
}

async fn status(ctx: &GlobalContext, name: &str) -> Result<()> {
    let (_ws, _env, remote) = super::connect(ctx)?;
    let status = remote.indexer_status(name).await?;
    if ctx.json() {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    println!(
        "{} '{name}': {}",
        "Indexer".bold(),
        status.get("status").and_then(Value::as_str).unwrap_or("?")
    );
    let Some(run) = status.get("lastResult").filter(|r| !r.is_null()) else {
        println!("  no runs recorded yet");
        return Ok(());
    };
    println!(
        "  last run: {} ({} → {})",
        run.get("status").and_then(Value::as_str).unwrap_or("?"),
        run.get("startTime").and_then(Value::as_str).unwrap_or("?"),
        run.get("endTime").and_then(Value::as_str).unwrap_or("…")
    );
    println!(
        "  items: {} processed, {} failed",
        run.get("itemsProcessed")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        run.get("itemsFailed").and_then(Value::as_u64).unwrap_or(0)
    );
    render_run_problems(run);
    Ok(())
}

/// Print a run's errors and warnings (message + document key), capped at 20
/// each with an overflow count.
fn render_run_problems(run: &Value) {
    for (label, key, color) in [
        ("error", "errors", "red"),
        ("warning", "warnings", "yellow"),
    ] {
        let Some(items) = run.get(key).and_then(Value::as_array) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        println!("  {} {}(s):", items.len(), label);
        for item in items.iter().take(20) {
            let msg = item
                .get("errorMessage")
                .or_else(|| item.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            let doc = item.get("key").and_then(Value::as_str).unwrap_or("");
            let line = if doc.is_empty() {
                format!("    - {msg}")
            } else {
                format!("    - [{doc}] {msg}")
            };
            if color == "red" {
                println!("{}", line.red());
            } else {
                println!("{}", line.yellow());
            }
        }
        if items.len() > 20 {
            println!("    … and {} more", items.len() - 20);
        }
    }
}
