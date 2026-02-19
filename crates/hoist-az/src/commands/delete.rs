//! Delete resources from Azure

use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;

use hoist_client::AzureSearchClient;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::managed::{self, ManagedMap};
use hoist_core::service::ServiceDomain;
use hoist_core::state::{Checksums, LocalState};

use crate::commands::confirm::prompt_yes_no;
use crate::commands::load_config_and_env;

pub async fn run(
    kind: ResourceKind,
    name: &str,
    force: bool,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    println!(
        "Deleting {} '{}' from environment '{}'",
        kind.display_name(),
        name,
        env.name
    );

    // Warn about managed sub-resources for knowledge sources
    if kind == ResourceKind::KnowledgeSource {
        println!();
        println!(
            "{} Deleting a knowledge source will also delete its managed sub-resources",
            "WARNING:".yellow().bold()
        );
        println!("  (index, indexer, data source, skillset provisioned by Azure).");
    }

    println!();

    if !force && !prompt_yes_no(&format!("Delete {} '{}'?", kind.display_name(), name))? {
        println!("Aborted.");
        return Ok(());
    }

    // Delete from Azure
    if kind.domain() == ServiceDomain::Search {
        let search_svc = env
            .primary_search_service()
            .ok_or_else(|| anyhow::anyhow!("No search service in environment '{}'", env.name))?;
        let client = AzureSearchClient::from_service_config(search_svc)?;

        print!("Deleting {} '{}' from Azure... ", kind.display_name(), name);
        io::stdout().flush()?;

        client.delete(kind, name).await?;
        println!("done");
    } else if kind.domain() == ServiceDomain::Foundry {
        let foundry_config = env
            .foundry
            .first()
            .ok_or_else(|| anyhow::anyhow!("No Foundry service in environment '{}'", env.name))?;
        let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;

        print!("Deleting {} '{}' from Azure... ", kind.display_name(), name);
        io::stdout().flush()?;

        foundry_client.delete_agent(name).await?;
        println!("done");
    }

    // Remove local files
    let service_dir = if kind.domain() == ServiceDomain::Search {
        let search_svc = env.primary_search_service().unwrap();
        env.search_service_dir(&files_root, search_svc)
    } else {
        let foundry_config = env.foundry.first().unwrap();
        env.foundry_service_dir(&files_root, foundry_config)
    };

    if kind == ResourceKind::Agent {
        let yaml_path = service_dir.join("agents").join(format!("{}.yaml", name));
        if yaml_path.exists() {
            std::fs::remove_file(&yaml_path)?;
            println!("Removed local file {}", yaml_path.display());
        }
    } else if kind == ResourceKind::KnowledgeSource {
        // Remove the KS directory and all managed sub-resources
        let ks_dir = service_dir
            .join("agentic-retrieval/knowledge-sources")
            .join(name);
        if ks_dir.exists() {
            std::fs::remove_dir_all(&ks_dir)?;
            println!("Removed local directory {}", ks_dir.display());
        }
    } else {
        let file_path = service_dir
            .join(kind.directory_name())
            .join(format!("{}.json", name));
        if file_path.exists() {
            std::fs::remove_file(&file_path)?;
            println!("Removed local file {}", file_path.display());
        }
    }

    // Update state and checksums
    let managed_map = ManagedMap::new();
    let mut state = LocalState::load_env(&project_root, &env.name)?;
    let mut checksums = Checksums::load_env(&project_root, &env.name)?;

    state.remove_managed(kind, name, &managed_map);
    checksums.remove_managed(kind, name, &managed_map);

    // For knowledge sources, also remove managed sub-resource state
    if kind == ResourceKind::KnowledgeSource {
        let ks_dir = service_dir
            .join("agentic-retrieval/knowledge-sources")
            .join(name);
        // Build managed map from the local KS file if it still exists
        let ks_file = ks_dir.join(format!("{}.json", name));
        if let Ok(content) = std::fs::read_to_string(&ks_file) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) {
                let ks_pairs = vec![(name.to_string(), value)];
                let local_managed = managed::build_managed_map(&ks_pairs);
                for (sub_kind, sub_name) in local_managed.keys() {
                    state.remove_managed(*sub_kind, sub_name, &local_managed);
                    checksums.remove_managed(*sub_kind, sub_name, &local_managed);
                }
            }
        }
    }

    state.save_env(&project_root, &env.name)?;
    checksums.save_env(&project_root, &env.name)?;

    println!();
    println!("Deleted {} '{}' successfully.", kind.display_name(), name);

    Ok(())
}
