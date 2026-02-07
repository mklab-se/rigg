//! hoist - Azure AI Search configuration management CLI

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

mod cli;
mod commands;

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

    // Run the command
    cli.run().await
}
