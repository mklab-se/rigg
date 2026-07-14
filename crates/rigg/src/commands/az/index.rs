//! `rigg az index` — query and inspect live indexes.

use anyhow::Result;
use colored::Colorize;
use serde_json::{Value, json};

use crate::cli::{AzIndexCommands, AzIndexQueryArgs};
use crate::commands::GlobalContext;

pub async fn run(ctx: &GlobalContext, command: AzIndexCommands) -> Result<()> {
    match command {
        AzIndexCommands::Query(args) => query(ctx, args).await,
        AzIndexCommands::Stats { name } => stats(ctx, &name).await,
    }
}

async fn query(ctx: &GlobalContext, args: AzIndexQueryArgs) -> Result<()> {
    let (_ws, _env, remote) = super::connect(ctx)?;
    let mut body = json!({
        "search": args.search,
        "top": args.top,
        "count": true
    });
    if let Some(filter) = &args.filter {
        body["filter"] = json!(filter);
    }
    if let Some(select) = &args.select {
        body["select"] = json!(select);
    }
    let result = remote.search_docs(&args.name, &body).await?;
    if ctx.json() {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }
    let total = result
        .get("@odata.count")
        .and_then(Value::as_u64)
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".to_string());
    let hits = result
        .get("value")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    println!(
        "{} match(es) in '{}' (showing {})",
        total.bold(),
        args.name,
        hits.len()
    );
    for (i, hit) in hits.iter().enumerate() {
        let score = hit
            .get("@search.score")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
        println!();
        println!("{} score {score:.3}", format!("[{}]", i + 1).bold());
        if let Some(obj) = hit.as_object() {
            for (k, v) in obj {
                if k.starts_with("@search") {
                    continue;
                }
                let text = match v {
                    Value::String(s) => {
                        let mut t: String = s.chars().take(200).collect();
                        if s.chars().count() > 200 {
                            t.push('…');
                        }
                        t
                    }
                    other => {
                        let s = other.to_string();
                        if s.len() > 200 {
                            format!("{}…", &s[..200])
                        } else {
                            s
                        }
                    }
                };
                println!("  {k}: {text}");
            }
        }
    }
    Ok(())
}

async fn stats(ctx: &GlobalContext, name: &str) -> Result<()> {
    let (_ws, _env, remote) = super::connect(ctx)?;
    let stats = remote.index_stats(name).await?;
    if ctx.json() {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }
    let docs = stats
        .get("documentCount")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let bytes = stats
        .get("storageSize")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    println!("{} '{name}'", "Index".bold());
    println!("  documents: {docs}");
    println!("  storage:   {}", human_bytes(bytes));
    if let Some(vector) = stats.get("vectorIndexSize").and_then(Value::as_u64) {
        println!("  vectors:   {}", human_bytes(vector));
    }
    Ok(())
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
