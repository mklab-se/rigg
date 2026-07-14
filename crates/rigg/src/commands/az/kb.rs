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
    let result = remote
        .kb_retrieve(name, &body)
        .await
        .map_err(|e| super::hint_user_role(e, "Search Index Data Reader"))?;
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

    // Grounding content. The stable API returns the grounding as a
    // JSON-encoded array of {ref_id, content} inside the message text —
    // unpack it for humans (the raw payload is available via --output json).
    let mut printed = false;
    if let Some(messages) = result.get("response").and_then(Value::as_array) {
        for message in messages {
            let Some(contents) = message.get("content").and_then(Value::as_array) else {
                continue;
            };
            for content in contents {
                if let Some(text) = content.get("text").and_then(Value::as_str) {
                    render_grounding(text);
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
                .filter(|t| !t.is_empty())
                .or_else(|| r.get("url").and_then(Value::as_str))
                .or_else(|| r.get("blobUrl").and_then(Value::as_str))
                .or_else(|| r.get("docKey").and_then(Value::as_str))
                .or_else(|| r.get("id").and_then(Value::as_str))
                .unwrap_or("?");
            let title: String = if title.chars().count() > 100 {
                format!("{}…", title.chars().take(100).collect::<String>())
            } else {
                title.to_string()
            };
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

/// Grounding text arrives as a JSON-encoded `[{ref_id, content}, ...]`
/// array. Render each chunk with its reference tag, truncated for reading;
/// fall back to printing the text verbatim when it isn't that shape.
fn render_grounding(text: &str) {
    let Ok(Value::Array(chunks)) = serde_json::from_str::<Value>(text) else {
        println!("{text}");
        return;
    };
    for chunk in &chunks {
        let ref_id = chunk
            .get("ref_id")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".to_string());
        let content = chunk.get("content").and_then(Value::as_str).unwrap_or("");
        let excerpt: String = content.chars().take(600).collect();
        let ellipsis = if content.chars().count() > 600 {
            "…"
        } else {
            ""
        };
        println!();
        println!("{} {excerpt}{ellipsis}", format!("[ref {ref_id}]").bold());
    }
    if !chunks.is_empty() {
        println!();
        println!("(chunks truncated for reading — full text via --output json)");
    }
}
