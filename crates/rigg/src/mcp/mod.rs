//! MCP (Model Context Protocol) server for AI agent integration

pub mod tools;

use anyhow::Result;
use rmcp::ServiceExt;

use crate::cli::{McpArgs, McpCommands};

/// Run MCP subcommands
pub async fn run(args: McpArgs) -> Result<()> {
    match args.command {
        McpCommands::Serve => serve().await,
        McpCommands::Tools { markdown } => {
            if markdown {
                print!("{}", tools_markdown());
            } else {
                for tool in tools::RiggMcpServer::new().tool_list() {
                    println!("{}\t{}", tool.name, purpose(tool.description.as_deref()));
                }
            }
            Ok(())
        }
        McpCommands::Install { target, scope } => install(target, scope),
    }
}

/// The `| Tool | Purpose | Parameters |` table for the generated region of
/// `MCP.md`, built from the tool router's own list — the exact names,
/// descriptions and JSON schemas an MCP client sees.
///
/// `list_all()` sorts by tool name, and parameters are ordered required
/// first, then alphabetically within each group, so the output is stable.
pub fn tools_markdown() -> String {
    let mut out = String::from("| Tool | Purpose | Parameters |\n|---|---|---|\n");
    for tool in tools::RiggMcpServer::new().tool_list() {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            tool.name,
            cell(&purpose(tool.description.as_deref())),
            parameters(&tool.input_schema),
        ));
    }
    out
}

/// The first sentence of a tool description: a `.` followed by whitespace
/// and a capital letter. That keeps `e.g. dev → staging` inside the
/// sentence it belongs to.
fn purpose(description: Option<&str>) -> String {
    let Some(text) = description.map(str::trim).filter(|t| !t.is_empty()) else {
        return String::new();
    };
    let chars: Vec<char> = text.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if *c != '.' {
            continue;
        }
        let Some(next) = chars.get(i + 1) else {
            return text.to_string();
        };
        if !next.is_whitespace() {
            continue;
        }
        if chars
            .get(i + 2)
            .is_some_and(|c| c.is_uppercase() || c.is_ascii_digit())
        {
            return chars[..=i].iter().collect();
        }
    }
    text.to_string()
}

/// `name: type` per property, `*` marking the required ones.
fn parameters(schema: &serde_json::Map<String, serde_json::Value>) -> String {
    let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) else {
        return "none".to_string();
    };
    if properties.is_empty() {
        return "none".to_string();
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut names: Vec<&String> = properties.keys().collect();
    names.sort();
    let (mut req, opt): (Vec<&String>, Vec<&String>) = names
        .into_iter()
        .partition(|n| required.contains(&n.as_str()));
    req.extend(opt);

    req.iter()
        .map(|name| {
            let ty = type_label(&properties[*name]);
            let star = if required.contains(&name.as_str()) {
                "*"
            } else {
                ""
            };
            format!("`{name}`: {ty}{star}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The JSON-schema type of one property, with the `null` half of an
/// `Option<T>` dropped.
fn type_label(schema: &serde_json::Value) -> String {
    if let Some(t) = schema.get("type") {
        if let Some(name) = t.as_str() {
            return name.to_string();
        }
        if let Some(list) = t.as_array() {
            let names: Vec<String> = list
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|v| *v != "null")
                .map(String::from)
                .collect();
            if !names.is_empty() {
                return names.join(" \\| ");
            }
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(list) = schema.get(key).and_then(|v| v.as_array()) {
            let names: Vec<String> = list
                .iter()
                .map(type_label)
                .filter(|n| n != "null" && n != "any")
                .collect();
            if !names.is_empty() {
                return names.join(" \\| ");
            }
        }
    }
    "any".to_string()
}

fn cell(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('|', "\\|")
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
fn install(target: crate::cli::McpTarget, scope: crate::cli::McpScope) -> Result<()> {
    use crate::cli::{McpScope, McpTarget};
    match (target, scope) {
        (McpTarget::ClaudeCode, McpScope::Workspace) => install_claude_code("project"),
        (McpTarget::ClaudeCode, McpScope::Global) => install_claude_code("user"),
        (McpTarget::VsCode, McpScope::Workspace) => install_vscode_workspace(),
        (McpTarget::VsCode, McpScope::Global) => install_vscode_global(),
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
