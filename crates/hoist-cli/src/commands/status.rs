//! Show project status

use anyhow::Result;

use hoist_core::resources::ResourceKind;
use hoist_core::state::LocalState;

use crate::commands::load_config;

pub async fn run() -> Result<()> {
    let (project_root, config) = load_config()?;

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

    // Load state
    let state = LocalState::load(&project_root)?;

    if let Some(last_sync) = state.last_sync {
        println!("Last sync: {}", last_sync.format("%Y-%m-%d %H:%M:%S UTC"));
    } else {
        println!("Last sync: never");
    }
    println!();

    // Count resources by type
    println!("Local Resources:");
    println!("----------------");

    let mut total = 0;

    for kind in ResourceKind::stable() {
        let dir = config
            .resource_dir(&project_root)
            .join(kind.directory_name());
        if !dir.exists() {
            println!("  {}: (not initialized)", kind.display_name());
            continue;
        }

        let count = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .count();

        println!("  {}: {}", kind.display_name(), count);
        total += count;
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
                continue;
            }

            let count = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
                .count();

            println!("  {}: {}", kind.display_name(), count);
            total += count;
        }
    }

    println!();
    println!("Total: {} resource(s)", total);
    println!();

    // Authentication status (brief)
    print!("Authentication: ");
    match hoist_client::auth::get_auth_provider() {
        Ok(provider) => match provider.get_token() {
            Ok(_) => println!("OK ({})", provider.method_name()),
            Err(e) => println!("Failed - {}", e),
        },
        Err(e) => println!("Not configured - {}", e),
    }

    Ok(())
}
