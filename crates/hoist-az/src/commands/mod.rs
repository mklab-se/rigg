//! CLI command implementations

pub mod auth;
pub mod common;
pub mod completion;
pub mod config;
pub mod confirm;
pub mod describe;
pub mod describe_project;
pub mod diff;
pub mod init;
pub mod pull;
pub mod pull_watch;
pub mod push;
pub mod status;
pub mod validate;

use std::path::PathBuf;

use hoist_core::config::find_project_root;
use hoist_core::Config;

/// Find project root and load configuration
pub fn load_config() -> anyhow::Result<(PathBuf, Config)> {
    let current_dir = std::env::current_dir()?;
    let project_root = find_project_root(&current_dir).ok_or_else(|| {
        anyhow::anyhow!(
            "Not in an hoist project. Run 'hoist init' to create one, or change to a directory containing hoist.toml"
        )
    })?;

    let config = Config::load(&project_root)?;
    Ok((project_root, config))
}
