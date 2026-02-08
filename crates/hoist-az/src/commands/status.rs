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

pub async fn run(output: OutputFormat) -> Result<()> {
    let (project_root, config) = load_config()?;

    // Load state
    let state = LocalState::load(&project_root)?;

    // Count resources by type
    let mut resource_counts = serde_json::Map::new();
    let mut total = 0;

    for kind in ResourceKind::stable() {
        let dir = config
            .resource_dir(&project_root)
            .join(kind.directory_name());
        let count = count_resources(&dir);
        resource_counts.insert(kind.display_name().to_string(), json!(count));
        total += count;
    }

    if config.sync.include_preview {
        for kind in [ResourceKind::KnowledgeBase, ResourceKind::KnowledgeSource] {
            let dir = config
                .resource_dir(&project_root)
                .join(kind.directory_name());
            let count = count_resources(&dir);
            resource_counts.insert(kind.display_name().to_string(), json!(count));
            total += count;
        }
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

    match output {
        OutputFormat::Json => {
            let status = json!({
                "project_root": project_root.display().to_string(),
                "service_name": config.service.name,
                "service_url": config.service_url(),
                "api_version": config.service.api_version,
                "preview_api_version": config.service.preview_api_version,
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
            println!("Service: {}", config.service.name);
            println!("Service URL: {}", config.service_url());
            println!("API version: {}", config.service.api_version);
            if config.sync.include_preview {
                println!(
                    "Preview API: {} (enabled)",
                    config.service.preview_api_version
                );
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

            for kind in ResourceKind::stable() {
                let dir = config
                    .resource_dir(&project_root)
                    .join(kind.directory_name());
                if !dir.exists() {
                    println!("  {}: (not initialized)", kind.display_name());
                } else {
                    let count = resource_counts
                        .get(kind.display_name())
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    println!("  {}: {}", kind.display_name(), count);
                }
            }

            if config.sync.include_preview {
                println!();
                println!("Preview Resources:");
                for kind in [ResourceKind::KnowledgeBase, ResourceKind::KnowledgeSource] {
                    let dir = config
                        .resource_dir(&project_root)
                        .join(kind.directory_name());
                    if !dir.exists() {
                        println!("  {}: (not initialized)", kind.display_name());
                    } else {
                        let count = resource_counts
                            .get(kind.display_name())
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        println!("  {}: {}", kind.display_name(), count);
                    }
                }
            }

            println!();
            println!("Total: {} resource(s)", total);
            println!();
            println!("Authentication: {}", auth_status);
        }
    }

    Ok(())
}
