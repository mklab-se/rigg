//! `rigg az` — operate the LIVE Azure resources (indexers, indexes,
//! knowledge bases, agents), in contrast to the config-plane commands that
//! act on local files. Resources are addressed by physical name; project
//! ownership is not required.

mod agent;
mod index;
mod indexer;
mod kb;

use anyhow::{Result, bail};
use rigg_core::workspace::{ResolvedEnv, Workspace};

use crate::cli::AzCommands;
use crate::commands::remote::Remote;
use crate::commands::{GlobalContext, load_workspace, resolve_env};

pub async fn run(ctx: &GlobalContext, command: AzCommands) -> Result<()> {
    match command {
        AzCommands::Indexer { command } => indexer::run(ctx, command).await,
        AzCommands::Index { command } => index::run(ctx, command).await,
        AzCommands::KnowledgeBase { command } => kb::run(ctx, command).await,
        AzCommands::Agent { command } => agent::run(ctx, command).await,
    }
}

/// Data-plane reads (documents, retrieval, agent invocation) need roles the
/// config-plane commands never exercise — decorate a 403 with the exact role
/// the signed-in identity is missing instead of a bare "access denied".
pub(crate) fn hint_user_role(e: anyhow::Error, role: &str) -> anyhow::Error {
    let msg = format!("{e:#}").to_lowercase();
    if msg.contains("403") || msg.contains("access denied") || msg.contains("forbidden") {
        e.context(format!(
            "your signed-in identity needs the '{role}' role for this operation — grant it \
             (e.g. az role assignment create --assignee <your-upn> --role \"{role}\" \
             --scope <resource-id>) and allow a few minutes to propagate"
        ))
    } else {
        e
    }
}

/// Resolve the environment and build a connected [`Remote`], printing the
/// target banner. Operations are service-level, so any project's connection
/// configuration for this environment will do — the first project with a
/// usable connection wins.
pub(crate) fn connect(ctx: &GlobalContext) -> Result<(Workspace, ResolvedEnv, Remote)> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let remote = ws
        .projects
        .iter()
        .map(|p| Remote::for_project(&env, p))
        .find(|r| r.has_search() || r.has_foundry());
    let Some(remote) = remote else {
        bail!(
            "no reachable services in environment '{}' (configure `search:` or `foundry:` in rigg.yaml; the workspace also needs at least one project)",
            env.name
        );
    };
    if !ctx.json() {
        remote.print_targets();
    }
    Ok((ws, env, remote))
}
