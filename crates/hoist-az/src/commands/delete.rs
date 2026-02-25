//! Delete resources from Azure or remove local files

use std::io::{self, Write};

use anyhow::Result;
use colored::Colorize;

use hoist_client::AzureSearchClient;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::managed::{self, ManagedMap};
use hoist_core::service::ServiceDomain;
use hoist_core::state::{Checksums, LocalState};

use crate::cli::DeleteTarget;
use crate::commands::confirm::prompt_yes_no;
use crate::commands::load_config_and_env;

pub async fn run(
    kind: ResourceKind,
    name: &str,
    target: DeleteTarget,
    force: bool,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    match target {
        DeleteTarget::Remote => {
            println!(
                "Deleting {} '{}' from {} on Azure (environment '{}')",
                kind.display_name(),
                name,
                env.primary_search_service()
                    .map(|s| s.name.as_str())
                    .unwrap_or("(unknown)"),
                env.name
            );
            println!("  Local files will NOT be affected.");
        }
        DeleteTarget::Local => {
            println!(
                "Removing local files for {} '{}'",
                kind.display_name(),
                name,
            );
            println!("  Local files are shared across all environments.");
            println!("  Azure resources are NOT affected in any environment.");
        }
    }

    // Warn about managed sub-resources for knowledge sources
    if kind == ResourceKind::KnowledgeSource {
        println!();
        match target {
            DeleteTarget::Remote => {
                println!(
                    "{} Deleting a knowledge source from Azure will also delete its managed",
                    "WARNING:".yellow().bold()
                );
                println!("  sub-resources (index, indexer, data source, skillset) on Azure.");
                println!("  The search index and all its data will be lost.");
            }
            DeleteTarget::Local => {
                println!(
                    "{} This will remove the local KS directory including all managed",
                    "NOTE:".yellow().bold()
                );
                println!("  sub-resource files (index, indexer, data source, skillset).");
                println!("  The Azure resources are not affected. Run 'hoist pull' to restore.");
            }
        }
    }

    println!();

    let action = match target {
        DeleteTarget::Remote => format!("Delete {} '{}' from Azure?", kind.display_name(), name),
        DeleteTarget::Local => {
            format!("Remove local files for {} '{}'?", kind.display_name(), name)
        }
    };
    if !force && !prompt_yes_no(&action)? {
        println!("Aborted.");
        return Ok(());
    }

    match target {
        DeleteTarget::Remote => {
            delete_from_azure(kind, name, &env).await?;
        }
        DeleteTarget::Local => {
            remove_local_files(kind, name, &env, &files_root)?;
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
        let service_dir = if kind.domain() == ServiceDomain::Search {
            let search_svc = env.primary_search_service().unwrap();
            env.search_service_dir(&files_root, search_svc)
        } else {
            let foundry_config = env.foundry.first().unwrap();
            env.foundry_service_dir(&files_root, foundry_config)
        };
        let ks_dir = service_dir
            .join("agentic-retrieval/knowledge-sources")
            .join(name);
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

    let target_desc = match target {
        DeleteTarget::Remote => "from Azure",
        DeleteTarget::Local => "locally",
    };
    println!();
    println!(
        "Deleted {} '{}' {} successfully.",
        kind.display_name(),
        name,
        target_desc,
    );

    // Suggest next step
    match target {
        DeleteTarget::Remote => {
            println!(
                "  To also remove local files: hoist delete --{} {} --target local",
                kind.cli_flag_name(),
                name
            );
        }
        DeleteTarget::Local => {
            println!(
                "  To restore from Azure: hoist pull --{}",
                kind.cli_flag_name_plural()
            );
        }
    }

    Ok(())
}

/// Delete a resource from the remote Azure service.
async fn delete_from_azure(
    kind: ResourceKind,
    name: &str,
    env: &hoist_core::config::ResolvedEnvironment,
) -> Result<()> {
    if kind.domain() == ServiceDomain::Search {
        let search_svc = env
            .primary_search_service()
            .ok_or_else(|| anyhow::anyhow!("No search service in environment '{}'", env.name))?;
        let client = AzureSearchClient::from_service_config(search_svc)?;

        print!(
            "Deleting {} '{}' from {}... ",
            kind.display_name(),
            name,
            search_svc.name
        );
        io::stdout().flush()?;

        client.delete(kind, name).await?;
        println!("done");
    } else if kind.domain() == ServiceDomain::Foundry {
        let foundry_config = env
            .foundry
            .first()
            .ok_or_else(|| anyhow::anyhow!("No Foundry service in environment '{}'", env.name))?;
        let foundry_client = hoist_client::FoundryClient::new(foundry_config)?;

        print!(
            "Deleting {} '{}' from Foundry... ",
            kind.display_name(),
            name
        );
        io::stdout().flush()?;

        foundry_client.delete_agent(name).await?;
        println!("done");
    }

    Ok(())
}

/// Remove local files for a resource.
fn remove_local_files(
    kind: ResourceKind,
    name: &str,
    env: &hoist_core::config::ResolvedEnvironment,
    files_root: &std::path::Path,
) -> Result<()> {
    let service_dir = if kind.domain() == ServiceDomain::Search {
        let search_svc = env.primary_search_service().unwrap();
        env.search_service_dir(files_root, search_svc)
    } else {
        let foundry_config = env.foundry.first().unwrap();
        env.foundry_service_dir(files_root, foundry_config)
    };

    if kind == ResourceKind::Agent {
        let yaml_path = service_dir.join("agents").join(format!("{}.yaml", name));
        if yaml_path.exists() {
            std::fs::remove_file(&yaml_path)?;
            println!("Removed {}", yaml_path.display());
        } else {
            println!("No local file found for Agent '{}'", name);
        }
    } else if kind == ResourceKind::KnowledgeSource {
        let ks_dir = service_dir
            .join("agentic-retrieval/knowledge-sources")
            .join(name);
        if ks_dir.exists() {
            std::fs::remove_dir_all(&ks_dir)?;
            println!("Removed {}", ks_dir.display());
        } else {
            println!("No local directory found for Knowledge Source '{}'", name);
        }
    } else {
        let file_path = service_dir
            .join(kind.directory_name())
            .join(format!("{}.json", name));
        if file_path.exists() {
            std::fs::remove_file(&file_path)?;
            println!("Removed {}", file_path.display());
        } else {
            println!("No local file found for {} '{}'", kind.display_name(), name);
        }
    }

    Ok(())
}
