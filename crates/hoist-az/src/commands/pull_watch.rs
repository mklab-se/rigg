//! Poll the server for changes and pull updates automatically

use std::time::Duration;

use anyhow::Result;
use tokio::time::sleep;

use crate::cli::ResourceTypeFlags;
use crate::commands::common::resolve_resource_selection_from_flags;
use crate::commands::load_config_and_env;
use crate::commands::pull::execute_pull;

pub async fn run(
    flags: &ResourceTypeFlags,
    filter: Option<String>,
    force: bool,
    interval: u64,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, _config, env) = load_config_and_env(env_override)?;

    let selection = resolve_resource_selection_from_flags(flags, env.sync.include_preview, true);

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    println!("Watching for changes (env: {})...", env.name);
    println!("  Interval: {}s", interval);
    if force {
        println!("  Auto-update: enabled (--force)");
    }
    println!();
    println!("Press Ctrl+C to stop");
    println!();

    let interval_duration = Duration::from_secs(interval);

    loop {
        let timestamp = chrono::Local::now().format("%H:%M:%S");

        match execute_pull(
            &project_root,
            &env,
            &selection,
            filter.as_deref(),
            false, // not dry_run
            force,
        )
        .await
        {
            Ok(()) => {}
            Err(e) => {
                println!("[{}] Error: {}", timestamp, e);
            }
        }

        sleep(interval_duration).await;
    }
}
