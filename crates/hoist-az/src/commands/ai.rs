//! AI feature management
//!
//! `hoist ai`         — show status
//! `hoist ai test`    — test AI connection
//! `hoist ai enable`  — enable AI for hoist
//! `hoist ai disable` — disable AI for hoist
//! `hoist ai config`  — open config in editor

use anyhow::Result;
use colored::Colorize;

use crate::cli::AiCommands;

const APP_NAME: &str = "hoist";

pub async fn run(cmd: Option<AiCommands>) -> Result<()> {
    match cmd {
        None => status(),
        Some(AiCommands::Test { message }) => test(message).await,
        Some(AiCommands::Enable) => enable(),
        Some(AiCommands::Disable) => disable(),
        Some(AiCommands::Config) => open_config(),
    }
}

/// Check if AI features are active (configured via ailloy + enabled for this tool).
pub fn is_ai_active() -> bool {
    !is_disabled()
        && ailloy::config::Config::load()
            .ok()
            .and_then(|c| c.default_chat_node().ok().map(|_| ()))
            .is_some()
}

fn status() -> Result<()> {
    let configured = ailloy::config::Config::load()
        .ok()
        .and_then(|c| c.default_chat_node().ok().map(|_| true))
        .unwrap_or(false);

    let enabled = !is_disabled();

    if configured {
        let config = ailloy::config::Config::load()?;
        let (id, node) = config.default_chat_node()?;
        if enabled {
            println!("{} AI is configured and enabled\n", "✓".green().bold());
        } else {
            println!("{} AI is configured but disabled\n", "!".yellow().bold());
        }
        print_node_info(id, node);
        if !enabled {
            println!(
                "\n  Run {} to re-enable.",
                format!("{APP_NAME} ai enable").cyan()
            );
        }
    } else {
        println!("{} AI is not configured\n", "✗".red().bold());
        println!(
            "  Edit the config file:  {}",
            format!("{APP_NAME} ai config").cyan()
        );
        println!(
            "\n  For advanced setup:    {}",
            "https://github.com/mklab-se/ailloy".dimmed()
        );
    }

    Ok(())
}

async fn test(message: Option<String>) -> Result<()> {
    let message = message.unwrap_or_else(|| "Say hello in one sentence.".to_string());

    println!("Testing AI connection...\n");

    let result: Result<ailloy::ChatResponse> = async {
        let client = ailloy::Client::from_config()?;
        client.chat(&[ailloy::Message::user(&message)]).await
    }
    .await;

    match result {
        Ok(response) => {
            println!("{}\n", "✓ PASS".green().bold());
            println!("  {}", response.content);
            Ok(())
        }
        Err(e) => {
            println!("{}\n", "✗ FAIL".red().bold());
            println!("  Error: {e}");
            println!(
                "\n  Run {} to check your configuration.",
                format!("{APP_NAME} ai config").cyan()
            );
            Err(e)
        }
    }
}

fn enable() -> Result<()> {
    let marker = disabled_marker_path();
    if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    println!("{} AI features enabled for {APP_NAME}.", "✓".green().bold());
    Ok(())
}

fn disable() -> Result<()> {
    let marker = disabled_marker_path();
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&marker, "")?;
    println!(
        "{} AI features disabled for {APP_NAME}.",
        "!".yellow().bold()
    );
    Ok(())
}

fn open_config() -> Result<()> {
    let path = ailloy::config::Config::config_path()?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if !path.exists() {
        std::fs::write(
            &path,
            "# ailloy AI configuration — https://github.com/mklab-se/ailloy\n\n\
             nodes:\n  default:\n    provider: openai\n    model: gpt-4o\n    # api_key: sk-...\n",
        )?;
    }

    let editor = resolve_editor();
    println!("Opening {} in {editor}...", path.display());
    let status = std::process::Command::new(&editor).arg(&path).status()?;

    if !status.success() {
        anyhow::bail!("Editor exited with non-zero status");
    }

    Ok(())
}

/// Resolve the best available editor: $VISUAL → $EDITOR → code → vi
fn resolve_editor() -> String {
    if let Ok(v) = std::env::var("VISUAL") {
        if !v.is_empty() {
            return v;
        }
    }
    if let Ok(v) = std::env::var("EDITOR") {
        if !v.is_empty() {
            return v;
        }
    }
    // Detect VS Code on PATH
    if which("code") {
        return "code".to_string();
    }
    "vi".to_string()
}

fn which(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn disabled_marker_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(APP_NAME)
        .join("ai_disabled")
}

fn is_disabled() -> bool {
    disabled_marker_path().exists()
}

fn print_node_info(id: &str, node: &ailloy::config::AiNode) {
    println!("  {} {}", "Node:".bold(), id.cyan());
    println!("  {} {:?}", "Provider:".bold(), node.provider);
    if let Some(ref model) = node.model {
        println!("  {} {}", "Model:".bold(), model);
    }
    if let Some(ref alias) = node.alias {
        println!("  {} {}", "Alias:".bold(), alias);
    }
}
