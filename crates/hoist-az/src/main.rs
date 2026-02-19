//! hoist - Configuration-as-code for Azure AI Search and Microsoft Foundry

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

mod banner;
mod cli;
mod commands;
mod mcp;
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

    // Skip update check in MCP mode (stdout is JSON-RPC)
    let is_mcp = matches!(cli.command, cli::Commands::Mcp(_));

    // Spawn background update check (skipped in quiet mode, MCP mode, or when opted out)
    let check_update = if cli.quiet || is_mcp || std::env::var_os("HOIST_NO_UPDATE_CHECK").is_some()
    {
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

    // Handle errors with user-friendly output
    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            // Check if the root cause is a ClientError with rich context
            if let Some(client_err) = err.downcast_ref::<hoist_client::ClientError>() {
                eprintln!();
                eprintln!("Error: {}", client_err);
                eprintln!();
                for line in client_err.suggestion().lines() {
                    eprintln!("  {}", line);
                }

                // Write detailed error log
                write_error_log(client_err);

                std::process::exit(1);
            }

            // Fall through for other errors
            Err(err)
        }
    }
}

/// Write detailed error information to `hoist-error.log` for diagnostics.
fn write_error_log(err: &hoist_client::ClientError) {
    use std::io::Write;

    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let mut lines = Vec::new();

    lines.push(format!("[{}] Error: {}", timestamp, err));

    // Walk the error source chain for full diagnostics
    {
        use std::error::Error;
        let mut source = err.source();
        while let Some(cause) = source {
            lines.push(format!("  Caused by: {}", cause));
            source = cause.source();
        }
    }

    if let Some(body) = err.raw_body() {
        if !body.is_empty() {
            lines.push(format!("Response body: {}", body));
        }
    }

    lines.push(format!("Suggestion: {}", err.suggestion()));
    lines.push(String::new());

    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("hoist-error.log")
    {
        Ok(mut file) => {
            for line in &lines {
                let _ = writeln!(file, "{}", line);
            }
            eprintln!("  Error details written to hoist-error.log");
        }
        Err(_) => {
            // If we can't write the log file, show the raw body inline
            if let Some(body) = err.raw_body() {
                if !body.is_empty() {
                    eprintln!("  Response: {}", body);
                }
            }
        }
    }
}
