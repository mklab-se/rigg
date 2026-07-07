//! `rigg mcp` — delegates to the MCP server module.

use anyhow::Result;

use crate::cli::McpArgs;
use crate::commands::GlobalContext;

pub async fn run(_ctx: &GlobalContext, args: McpArgs) -> Result<()> {
    crate::mcp::run(args).await
}
