//! `rigg az knowledge-base` (alias `kb`) — agentic retrieval against a
//! live knowledge base. The stable API is extractive: the response carries
//! grounding content and references, not a synthesized answer.

use anyhow::Result;
use colored::Colorize;
use serde_json::{Value, json};

use crate::cli::AzKbCommands;
use crate::commands::GlobalContext;

pub async fn run(ctx: &GlobalContext, command: AzKbCommands) -> Result<()> {
    match command {
        AzKbCommands::Ask { name, prompt } => ask(ctx, &name, &prompt).await,
    }
}

async fn ask(ctx: &GlobalContext, name: &str, prompt: &str) -> Result<()> {
    let (_ws, _env, remote) = super::connect(ctx)?;
    let body = json!({
        "intents": [{ "type": "semantic", "search": prompt }],
        "includeActivity": true
    });
    let result = remote.kb_retrieve(name, &body).await?;
    if ctx.json() {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    // Any knowledge source that errored shows up in the activity records.
    let source_errors: Vec<&Value> = result
        .get("activity")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|r| r.get("error").is_some()).collect())
        .unwrap_or_default();
    for err in &source_errors {
        println!(
            "{} knowledge source '{}' reported an error: {}",
            "!".yellow(),
            err.get("knowledgeSourceName")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            err.pointer("/error/message")
                .and_then(Value::as_str)
                .unwrap_or("?")
        );
    }

    // Grounding content.
    let mut printed = false;
    if let Some(messages) = result.get("response").and_then(Value::as_array) {
        for message in messages {
            let Some(contents) = message.get("content").and_then(Value::as_array) else {
                continue;
            };
            for content in contents {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    println!("{text}");
                    printed = true;
                }
            }
        }
    }
    if !printed {
        println!("(no grounding content returned)");
    }

    // References.
    let refs = result
        .get("references")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !refs.is_empty() {
        println!();
        println!("{}", "References:".bold());
        for (i, r) in refs.iter().enumerate() {
            let title = r
                .pointer("/sourceData/title")
                .and_then(Value::as_str)
                .or_else(|| r.get("docKey").and_then(Value::as_str))
                .or_else(|| r.get("url").and_then(Value::as_str))
                .or_else(|| r.get("blobUrl").and_then(Value::as_str))
                .unwrap_or("?");
            let score = r
                .get("rerankerScore")
                .and_then(Value::as_f64)
                .map(|s| format!(" (score {s:.2})"))
                .unwrap_or_default();
            println!("  [{}] {title}{score}", i + 1);
        }
    }
    Ok(())
}
