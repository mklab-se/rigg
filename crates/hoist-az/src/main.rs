//! hoist - Configuration-as-code for Azure AI Search and Microsoft Foundry

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod cli;
mod commands;
mod update;

use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments
    let cli = Cli::parse();

    // Initialize logging
    let filter = if cli.verbose > 0 {
        match cli.verbose {
            1 => "hoist=debug",
            _ => "hoist=trace,hoist_client=debug",
        }
    } else if cli.quiet {
        "error"
    } else {
        "hoist=info"
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false).without_time())
        .with(EnvFilter::new(filter))
        .init();

    // Handle --no-color
    if cli.no_color {
        colored::control::set_override(false);
    }

    // Spawn background update check (skipped in quiet mode or when opted out)
    let check_update = if cli.quiet || std::env::var_os("HOIST_NO_UPDATE_CHECK").is_some() {
        None
    } else {
        Some(tokio::spawn(update::check_for_update()))
    };

    // Run the command
    let result = cli.run().await;

    // Print update notification (if any) after the command completes
    if let Some(handle) = check_update {
        if let Ok(Some(message)) = handle.await {
            eprintln!();
            eprintln!("{message}");
        }
    }

    result
}
