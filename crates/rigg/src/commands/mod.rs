//! CLI command implementations

pub mod ai;
pub mod auth;
pub mod common;
pub mod completion;
pub mod config;
pub mod confirm;
pub mod copy;
pub mod delete;
pub mod describe;
pub mod describe_project;
pub mod diff;
pub mod env;
pub mod explain;
pub mod init;
pub mod pull;
pub mod pull_watch;
pub mod push;
pub mod scaffold;
pub mod skill;
pub mod status;
pub mod validate;

use std::path::PathBuf;

use rigg_core::Config;
use rigg_core::config::{ResolvedEnvironment, find_project_root};

/// Find project root and load configuration
pub fn load_config() -> anyhow::Result<(PathBuf, Config)> {
    let current_dir = std::env::current_dir()?;
    let project_root = find_project_root(&current_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "Not in an rigg project. Run 'rigg init' to create one, or change to a directory containing rigg.yaml"
        )
    })?;

    let config = Config::load(&project_root)?;
    Ok((project_root, config))
}

/// Find project root, load configuration, and resolve the environment
pub fn load_config_and_env(
    env_override: Option<&str>,
) -> anyhow::Result<(PathBuf, Config, ResolvedEnvironment)> {
    let (project_root, config) = load_config()?;
    let env = config.resolve_env(env_override)?;
    Ok((project_root, config, env))
}
