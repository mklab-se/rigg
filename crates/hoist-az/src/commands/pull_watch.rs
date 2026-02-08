//! Poll the server for changes and pull updates automatically

use std::time::Duration;

use anyhow::Result;
use tokio::time::sleep;

use crate::commands::common::{resolve_resource_selection, SingularFlags};
use crate::commands::load_config;
use crate::commands::pull::execute_pull;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    all: bool,
    indexes: bool,
    indexers: bool,
    datasources: bool,
    skillsets: bool,
    synonymmaps: bool,
    aliases: bool,
    knowledgebases: bool,
    knowledgesources: bool,
    singular: &SingularFlags,
    filter: Option<String>,
    force: bool,
    source: Option<String>,
    interval: u64,
) -> Result<()> {
    let (project_root, config) = load_config()?;

    let selection = resolve_resource_selection(
        all,
        indexes,
        indexers,
        datasources,
        skillsets,
        synonymmaps,
        aliases,
        knowledgebases,
        knowledgesources,
        singular,
        config.sync.include_preview,
        true,
    );

    if selection.is_empty() {
        println!("No resource types specified. Use --all or specify types (e.g., --indexes)");
        return Ok(());
    }

    let server_name = source.as_deref().unwrap_or(&config.service.name);

    println!("Watching for changes on {}...", server_name);
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
            &config,
            &selection,
            filter.as_deref(),
            false, // not dry_run
            force,
            source.as_deref(),
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
