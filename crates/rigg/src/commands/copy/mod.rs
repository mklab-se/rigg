//! Copy resources locally under new names.
//!
//! A local-only operation: reads files, rewrites names and cross-references,
//! and writes new files. No network calls -- push separately after copying.

mod knowledge_source;
mod rewrite;
mod standalone;

use anyhow::{Result, bail};

use rigg_core::resources::ResourceKind;

use crate::commands::load_config_and_env;

use knowledge_source::copy_knowledge_source;
use standalone::copy_standalone_resource;

#[allow(clippy::too_many_arguments)]
pub fn run(
    source: &str,
    target: &str,
    knowledgesource: bool,
    knowledgebase: bool,
    index: bool,
    indexer: bool,
    datasource: bool,
    skillset: bool,
    synonymmap: bool,
    alias: bool,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    let search_svc = env
        .primary_search_service()
        .ok_or_else(|| anyhow::anyhow!("No search service configured"))?;
    let service_dir = env.search_service_dir(&files_root, search_svc);

    if knowledgesource {
        copy_knowledge_source(&service_dir, source, target)
    } else {
        let kind = if knowledgebase {
            ResourceKind::KnowledgeBase
        } else if index {
            ResourceKind::Index
        } else if indexer {
            ResourceKind::Indexer
        } else if datasource {
            ResourceKind::DataSource
        } else if skillset {
            ResourceKind::Skillset
        } else if synonymmap {
            ResourceKind::SynonymMap
        } else if alias {
            ResourceKind::Alias
        } else {
            bail!("Specify a resource type (e.g., --knowledgesource, --index)");
        };
        copy_standalone_resource(&service_dir, kind, source, target)
    }
}

#[cfg(test)]
mod tests;
