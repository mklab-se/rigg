//! Show project status

use anyhow::Result;
use serde_json::json;

use hoist_core::resources::ResourceKind;
use hoist_core::state::LocalState;

use crate::cli::OutputFormat;
use crate::commands::load_config;

fn count_resources(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
                .count()
        })
        .unwrap_or(0)
}

/// Count agent directories (each subdirectory is one agent)
fn count_agent_dirs(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .count()
        })
        .unwrap_or(0)
}

pub async fn run(output: OutputFormat) -> Result<()> {
    let (project_root, config) = load_config()?;

    // Load state
    let state = LocalState::load(&project_root)?;

    // Count resources by type (across all search services)
    let mut resource_counts = serde_json::Map::new();
    let mut total = 0;

    for search_svc in config.search_services() {
        let search_base = config.search_service_dir(&project_root, &search_svc.name);

        for kind in ResourceKind::stable() {
            let dir = search_base.join(kind.directory_name());
            let count = count_resources(&dir);
            let entry = resource_counts
                .entry(kind.display_name().to_string())
                .or_insert(json!(0));
            *entry = json!(entry.as_u64().unwrap_or(0) + count as u64);
            total += count;
        }

        if config.sync.include_preview {
            for kind in [ResourceKind::KnowledgeBase, ResourceKind::KnowledgeSource] {
                let dir = search_base.join(kind.directory_name());
                let count = count_resources(&dir);
                let entry = resource_counts
                    .entry(kind.display_name().to_string())
                    .or_insert(json!(0));
                *entry = json!(entry.as_u64().unwrap_or(0) + count as u64);
                total += count;
            }
        }
    }

    // Count Foundry agents
    if config.has_foundry() {
        let mut agent_total = 0;
        for foundry_config in config.foundry_services() {
            let agents_dir = config
                .foundry_service_dir(&project_root, &foundry_config.name, &foundry_config.project)
                .join("agents");
            agent_total += count_agent_dirs(&agents_dir);
        }
        resource_counts.insert("Agent".to_string(), json!(agent_total));
        total += agent_total;
    }

    // Get auth status
    let auth_status = match hoist_client::auth::get_auth_provider() {
        Ok(provider) => match provider.get_token() {
            Ok(_) => format!("OK ({})", provider.method_name()),
            Err(e) => format!("Failed - {}", e),
        },
        Err(e) => format!("Not configured - {}", e),
    };

    let last_sync = state
        .last_sync
        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string());

    let primary = config.primary_search_service();
    let service_name = primary
        .as_ref()
        .map(|s| s.name.as_str())
        .unwrap_or("(none)");

    match output {
        OutputFormat::Json => {
            let status = json!({
                "project_root": project_root.display().to_string(),
                "service_name": service_name,
                "service_url": config.service_url(),
                "api_version": config.api_version_for(false),
                "preview_api_version": config.api_version_for(true),
                "include_preview": config.sync.include_preview,
                "last_sync": last_sync,
                "resources": resource_counts,
                "total_resources": total,
                "authentication": auth_status,
            });
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        OutputFormat::Text => {
            println!("Project Status");
            println!("==============");
            println!();
            println!("Project root: {}", project_root.display());
            println!("Service: {}", service_name);
            println!("Service URL: {}", config.service_url());
            println!("API version: {}", config.api_version_for(false));
            if config.sync.include_preview {
                println!("Preview API: {} (enabled)", config.api_version_for(true));
            }
            println!();

            if let Some(ref sync_time) = last_sync {
                println!("Last sync: {}", sync_time);
            } else {
                println!("Last sync: never");
            }
            println!();

            println!("Local Resources:");
            println!("----------------");

            for search_svc in config.search_services() {
                let search_base = config.search_service_dir(&project_root, &search_svc.name);
                println!("  Search service: {}", search_svc.name);

                for kind in ResourceKind::stable() {
                    let dir = search_base.join(kind.directory_name());
                    if !dir.exists() {
                        println!("    {}: (not initialized)", kind.display_name());
                    } else {
                        let count = count_resources(&dir);
                        println!("    {}: {}", kind.display_name(), count);
                    }
                }

                if config.sync.include_preview {
                    println!();
                    println!("  Preview Resources:");
                    for kind in [ResourceKind::KnowledgeBase, ResourceKind::KnowledgeSource] {
                        let dir = search_base.join(kind.directory_name());
                        if !dir.exists() {
                            println!("    {}: (not initialized)", kind.display_name());
                        } else {
                            let count = count_resources(&dir);
                            println!("    {}: {}", kind.display_name(), count);
                        }
                    }
                }
            }

            if config.has_foundry() {
                println!();
                println!("Foundry Resources:");
                for foundry_config in config.foundry_services() {
                    println!(
                        "  Service: {}/{}",
                        foundry_config.name, foundry_config.project
                    );
                }
                let agent_count = resource_counts
                    .get("Agent")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                println!("  Agent: {}", agent_count);
            }

            println!();
            println!("Total: {} resource(s)", total);
            println!();
            println!("Authentication: {}", auth_status);
        }
    }

    Ok(())
}
