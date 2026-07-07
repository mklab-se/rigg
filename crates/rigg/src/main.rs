//! rigg - Configuration-as-code for Azure AI Search and Microsoft Foundry

use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

mod banner;
mod cli;
mod commands;
mod mcp;
mod update;

use cli::Cli;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Parse CLI arguments
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose > 0 {
        match cli.verbose {
            1 => "rigg=debug",
            _ => "rigg=trace,rigg_client=debug",
        }
    } else if cli.quiet {
        "error"
    } else {
        "rigg=info"
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).without_time())
        .with(EnvFilter::new(filter))
        .init();

    // Handle --no-color
    if cli.no_color {
        colored::control::set_override(false);
    }

    // Skip update check in MCP mode (stdout is JSON-RPC)
    let is_mcp = matches!(cli.command, cli::Commands::Mcp(_));

    // Spawn background update check (skipped in quiet mode, MCP mode, or when opted out)
    let check_update = if cli.quiet || is_mcp || std::env::var_os("RIGG_NO_UPDATE_CHECK").is_some()
    {
        None
    } else {
        Some(tokio::spawn(update::check_for_update()))
    };

    // Run the command
    let code = cli.run().await;

    // Print update notification (if any) after the command completes
    if let Some(handle) = check_update {
        if let Ok(Some(message)) = handle.await {
            eprintln!();
            eprintln!("{message}");
        }
    }

    code.into()
}
