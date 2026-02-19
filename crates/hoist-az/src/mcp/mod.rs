//! MCP (Model Context Protocol) server for AI agent integration

pub mod tools;

use anyhow::Result;
use rmcp::ServiceExt;

use crate::cli::McpCommands;

/// Run MCP subcommands
pub async fn run(cmd: McpCommands) -> Result<()> {
    match cmd {
        McpCommands::Serve => serve().await,
        McpCommands::Install { target } => install(target),
    }
}

/// Start the MCP server on stdio transport
async fn serve() -> Result<()> {
    // Disable colored output for MCP (stdout is JSON-RPC)
    colored::control::set_override(false);

    let server = tools::HoistMcpServer::new();

    let service = server
        .serve(rmcp::transport::io::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("Failed to start MCP server: {}", e))?;

    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP server error: {}", e))?;

    Ok(())
}

/// Install hoist as an MCP server with AI tools
fn install(target: crate::cli::McpInstallTarget) -> Result<()> {
    match target {
        crate::cli::McpInstallTarget::ClaudeCode => install_claude_code(),
        crate::cli::McpInstallTarget::VsCode => install_vscode(),
    }
}

fn install_claude_code() -> Result<()> {
    // Run: claude mcp add hoist -- hoist mcp serve
    let status = std::process::Command::new("claude")
        .args([
            "mcp",
            "add",
            "hoist",
            "--transport",
            "stdio",
            "--",
            "hoist",
            "mcp",
            "serve",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("Registered hoist MCP server with Claude Code.");
            println!("The hoist tools are now available in all Claude Code sessions.");
            Ok(())
        }
        Ok(s) => {
            anyhow::bail!("claude mcp add failed with exit code: {}", s);
        }
        Err(e) => {
            eprintln!("Could not run 'claude' CLI: {}", e);
            eprintln!();
            eprintln!("To register manually, add to your Claude Code MCP config:");
            eprintln!(r#"  claude mcp add hoist --transport stdio -- hoist mcp serve"#);
            anyhow::bail!("Claude CLI not found");
        }
    }
}

fn install_vscode() -> Result<()> {
    let config_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    let mcp_config_path = config_dir.join(".vscode").join("mcp.json");

    let mcp_entry = serde_json::json!({
        "servers": {
            "hoist": {
                "command": "hoist",
                "args": ["mcp", "serve"],
                "type": "stdio"
            }
        }
    });

    // If file exists, try to merge; otherwise create
    if mcp_config_path.exists() {
        let content = std::fs::read_to_string(&mcp_config_path)?;
        let mut config: serde_json::Value =
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

        if let Some(servers) = config.get_mut("servers").and_then(|s| s.as_object_mut()) {
            servers.insert("hoist".to_string(), mcp_entry["servers"]["hoist"].clone());
        } else {
            config["servers"] = mcp_entry["servers"].clone();
        }

        std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&config)?)?;
    } else {
        std::fs::create_dir_all(mcp_config_path.parent().unwrap())?;
        std::fs::write(&mcp_config_path, serde_json::to_string_pretty(&mcp_entry)?)?;
    }

    println!("Registered hoist MCP server with VS Code.");
    println!("Config: {}", mcp_config_path.display());
    Ok(())
}
