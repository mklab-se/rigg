//! Show project status

use anyhow::Result;
use serde_json::json;

use hoist_core::resources::ResourceKind;
use hoist_core::state::LocalState;

use crate::cli::OutputFormat;
use crate::commands::load_config_and_env;

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

/// Count knowledge source directories (each subdirectory is one KS with its managed resources)
fn count_ks_dirs(dir: &std::path::Path) -> usize {
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

/// Count agent YAML files
fn count_agent_files(dir: &std::path::Path) -> usize {
    if !dir.exists() {
        return 0;
    }
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("yaml"))
                .count()
        })
        .unwrap_or(0)
}

pub async fn run(output: OutputFormat, env_override: Option<&str>) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    // Load state
    let state = LocalState::load_env(&project_root, &env.name)?;

    // Count resources by type (across all search services)
    let mut resource_counts = serde_json::Map::new();
    let mut total = 0;

    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_root, search_svc);

        for kind in ResourceKind::stable() {
            let dir = search_base.join(kind.directory_name());
            let count = count_resources(&dir);
            let entry = resource_counts
                .entry(kind.display_name().to_string())
                .or_insert(json!(0));
            *entry = json!(entry.as_u64().unwrap_or(0) + count as u64);
            total += count;
        }

        if env.sync.include_preview {
            // Knowledge bases are flat JSON files
            let kb_dir = search_base.join(ResourceKind::KnowledgeBase.directory_name());
            let kb_count = count_resources(&kb_dir);
            let kb_entry = resource_counts
                .entry(ResourceKind::KnowledgeBase.display_name().to_string())
                .or_insert(json!(0));
            *kb_entry = json!(kb_entry.as_u64().unwrap_or(0) + kb_count as u64);
            total += kb_count;

            // Knowledge sources are directories (each subdir = one KS with managed resources)
            let ks_dir = search_base.join(ResourceKind::KnowledgeSource.directory_name());
            let ks_count = count_ks_dirs(&ks_dir);
            let ks_entry = resource_counts
                .entry(ResourceKind::KnowledgeSource.display_name().to_string())
                .or_insert(json!(0));
            *ks_entry = json!(ks_entry.as_u64().unwrap_or(0) + ks_count as u64);
            total += ks_count;
        }
    }

    // Count Foundry agents
    if env.has_foundry() {
        let mut agent_total = 0;
        for foundry_config in &env.foundry {
            let agents_dir = env
                .foundry_service_dir(&files_root, foundry_config)
                .join("agents");
            agent_total += count_agent_files(&agents_dir);
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

    let primary = env.primary_search_service();
    let service_name = primary.map(|s| s.name.as_str()).unwrap_or("(none)");

    match output {
        OutputFormat::Json => {
            let mut status = json!({
                "project_root": project_root.display().to_string(),
                "environment": env.name,
                "service_name": service_name,
                "include_preview": env.sync.include_preview,
                "last_sync": last_sync,
                "resources": resource_counts,
                "total_resources": total,
                "authentication": auth_status,
            });
            if let Some(svc) = primary {
                status["service_url"] = json!(svc.service_url());
                status["api_version"] = json!(&svc.api_version);
                status["preview_api_version"] = json!(&svc.preview_api_version);
            }
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        OutputFormat::Text => {
            println!("Project Status");
            println!("==============");
            println!();
            println!("Project root: {}", project_root.display());
            println!("Environment: {}", env.name);
            if let Some(svc) = primary {
                println!("Service: {}", svc.name);
                println!("Service URL: {}", svc.service_url());
                println!("API version: {}", svc.api_version);
                if env.sync.include_preview {
                    println!("Preview API: {} (enabled)", svc.preview_api_version);
                }
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

            for search_svc in &env.search {
                let search_base = env.search_service_dir(&files_root, search_svc);
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

                if env.sync.include_preview {
                    println!();
                    println!("  Preview Resources:");

                    // Knowledge bases are flat JSON files
                    let kb_dir = search_base.join(ResourceKind::KnowledgeBase.directory_name());
                    if !kb_dir.exists() {
                        println!(
                            "    {}: (not initialized)",
                            ResourceKind::KnowledgeBase.display_name()
                        );
                    } else {
                        let count = count_resources(&kb_dir);
                        println!(
                            "    {}: {}",
                            ResourceKind::KnowledgeBase.display_name(),
                            count
                        );
                    }

                    // Knowledge sources are directories
                    let ks_dir = search_base.join(ResourceKind::KnowledgeSource.directory_name());
                    if !ks_dir.exists() {
                        println!(
                            "    {}: (not initialized)",
                            ResourceKind::KnowledgeSource.display_name()
                        );
                    } else {
                        let count = count_ks_dirs(&ks_dir);
                        println!(
                            "    {}: {}",
                            ResourceKind::KnowledgeSource.display_name(),
                            count
                        );
                    }
                }
            }

            if env.has_foundry() {
                println!();
                println!("Foundry Resources:");
                for foundry_config in &env.foundry {
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
