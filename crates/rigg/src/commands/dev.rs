//! Developer utilities (`rigg dev ...`).

use anyhow::Result;

use crate::cli::DevCommands;
use crate::commands::GlobalContext;

pub async fn run(_ctx: &GlobalContext, cmd: DevCommands) -> Result<()> {
    match cmd {
        DevCommands::ApiCheck => api_check().await,
    }
}

/// Compare rigg's supported Azure API versions against the newest available.
///
/// Full implementation lands in 0.19; the command exists so CI and the
/// api-watchdog skill have a stable entry point.
async fn api_check() -> Result<()> {
    println!(
        "supported search stable:   {}",
        rigg_core::registry::SEARCH_STABLE_API_VERSION
    );
    println!(
        "supported search preview:  {}",
        rigg_core::registry::SEARCH_PREVIEW_API_VERSION
    );
    println!(
        "supported foundry:         {}",
        rigg_core::registry::FOUNDRY_API_VERSION
    );
    println!(
        "supported foundry ARM:     {}",
        rigg_core::registry::ARM_COGNITIVE_API_VERSION
    );
    println!();
    println!("note: automatic upstream comparison ships in rigg 0.19");
    Ok(())
}
