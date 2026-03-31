//! MCP (Model Context Protocol) server for AI agent integration

pub mod tools;

use anyhow::Result;
use rmcp::ServiceExt;

use crate::cli::McpCommands;

/// Run MCP subcommands
pub async fn run(cmd: McpCommands) -> Result<()> {
    match cmd {
        McpCommands::Serve => serve().await,
        McpCommands::Install { target, scope } => install(target, scope),
    }
}

/// Start the MCP server on stdio transport
async fn serve() -> Result<()> {
    // Disable colored output for MCP (stdout is JSON-RPC)
    colored::control::set_override(false);

    let server = tools::RiggMcpServer::new();

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

/// Install rigg as an MCP server with AI tools
fn install(target: crate::cli::McpInstallTarget, scope: crate::cli::McpInstallScope) -> Result<()> {
    use crate::cli::{McpInstallScope, McpInstallTarget};
    match (target, scope) {
        (McpInstallTarget::ClaudeCode, McpInstallScope::Workspace) => {
            install_claude_code("project")
        }
        (McpInstallTarget::ClaudeCode, McpInstallScope::Global) => install_claude_code("user"),
        (McpInstallTarget::VsCode, McpInstallScope::Workspace) => install_vscode_workspace(),
        (McpInstallTarget::VsCode, McpInstallScope::Global) => install_vscode_global(),
    }
}

fn install_claude_code(scope: &str) -> Result<()> {
    let status = std::process::Command::new("claude")
        .args([
            "mcp",
            "add",
            "rigg",
            "--scope",
            scope,
            "--transport",
            "stdio",
            "--",
            "rigg",
            "mcp",
            "serve",
        ])
        .status();

    let scope_desc = if scope == "project" {
        "workspace"
    } else {
        "global"
    };

    match status {
        Ok(s) if s.success() => {
            println!(
                "Registered rigg MCP server with Claude Code ({}).",
                scope_desc
            );
            if scope == "project" {
                println!("Available when Claude Code opens this project.");
            } else {
                println!("Available in all Claude Code sessions.");
            }
            Ok(())
        }
        Ok(s) => {
            anyhow::bail!("claude mcp add failed with exit code: {}", s);
        }
        Err(e) => {
            eprintln!("Could not run 'claude' CLI: {}", e);
            eprintln!();
            eprintln!("To register manually:");
            eprintln!(
                r#"  claude mcp add rigg --scope {} --transport stdio -- rigg mcp serve"#,
                scope
            );
            anyhow::bail!("Claude CLI not found");
        }
    }
}

fn install_vscode_workspace() -> Result<()> {
    let project_root = find_project_root()?;
    let mcp_config_path = project_root.join(".vscode").join("mcp.json");

    write_vscode_mcp_config(&mcp_config_path)?;

    println!("Registered rigg MCP server with VS Code (workspace).");
    println!("Config: {}", mcp_config_path.display());
    println!("Available when VS Code opens this project.");
    Ok(())
}

fn install_vscode_global() -> Result<()> {
    let home_dir =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    let mcp_config_path = home_dir.join(".vscode").join("mcp.json");

    write_vscode_mcp_config(&mcp_config_path)?;

    println!("Registered rigg MCP server with VS Code (global).");
    println!("Config: {}", mcp_config_path.display());
    println!("Available in all VS Code sessions.");
    Ok(())
}

fn write_vscode_mcp_config(mcp_config_path: &std::path::Path) -> Result<()> {
    let mcp_entry = serde_json::json!({
        "servers": {
            "rigg": {
                "command": "rigg",
                "args": ["mcp", "serve"],
                "type": "stdio"
            }
        }
    });

    if mcp_config_path.exists() {
        let content = std::fs::read_to_string(mcp_config_path)?;
        let mut config: serde_json::Value =
            serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}));

        if let Some(servers) = config.get_mut("servers").and_then(|s| s.as_object_mut()) {
            servers.insert("rigg".to_string(), mcp_entry["servers"]["rigg"].clone());
        } else {
            config["servers"] = mcp_entry["servers"].clone();
        }

        std::fs::write(mcp_config_path, serde_json::to_string_pretty(&config)?)?;
    } else {
        std::fs::create_dir_all(mcp_config_path.parent().unwrap())?;
        std::fs::write(mcp_config_path, serde_json::to_string_pretty(&mcp_entry)?)?;
    }

    Ok(())
}

/// Find the project root by walking up from the current directory looking for rigg.yaml.
/// Falls back to the current working directory if not found.
fn find_project_root() -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir()?;
    let mut dir = cwd.as_path();
    loop {
        if dir.join("rigg.yaml").exists() {
            return Ok(dir.to_path_buf());
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => break,
        }
    }
    // Fallback to cwd
    Ok(cwd)
}
