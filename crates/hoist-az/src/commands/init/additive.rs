//! Additive update: discover and add new services to an existing project

use std::path::Path;

use anyhow::Result;

use hoist_client::arm::AiServicesAccount;
use hoist_core::config::{Config, FoundryServiceConfig};
use hoist_core::resources::ResourceKind;

use super::discovery::{
    discover_new_foundry_services, discover_new_search_services, try_authenticate,
};

/// Additive update: discover and add new services to an existing project
pub(super) async fn run_additive(project_dir: &Path) -> Result<()> {
    let mut config = Config::load(project_dir)?;
    crate::banner::print_banner();
    println!();
    println!("Updating hoist project in {}", project_dir.display());
    println!();

    let ctx = try_authenticate().await?;

    // Find the default environment to update
    let env_name = config
        .default_env_name()
        .ok_or_else(|| anyhow::anyhow!("No default environment set"))?
        .to_string();
    let files_dir = config.files_root(project_dir);
    let env_config = config
        .environments
        .get_mut(&env_name)
        .ok_or_else(|| anyhow::anyhow!("Environment '{}' not found", env_name))?;

    // Discover and add new search services
    if crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        let new_search = discover_new_search_services(&ctx, &env_config.search).await?;
        let resolved = config
            .resolve_env(Some(&env_name))
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for svc in &new_search {
            let search_base = resolved.search_service_dir(&files_dir, svc);
            for kind in ResourceKind::search_kinds() {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }
        let env_config = config.environments.get_mut(&env_name).unwrap();
        env_config.search.extend(new_search);
    }

    // Discover and add new foundry services
    if crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        let env_config = config.environments.get_mut(&env_name).unwrap();
        let accounts = ctx
            .arm
            .list_ai_services_accounts(&ctx.subscription_id)
            .await?;
        refresh_foundry_endpoints(&mut env_config.foundry, &accounts);

        let new_foundry =
            discover_new_foundry_services(&ctx, &env_config.foundry, &accounts).await?;
        let resolved = config
            .resolve_env(Some(&env_name))
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        for svc in &new_foundry {
            let foundry_base = resolved.foundry_service_dir(&files_dir, svc);
            std::fs::create_dir_all(foundry_base.join("agents"))?;
        }
        let env_config = config.environments.get_mut(&env_name).unwrap();
        env_config.foundry.extend(new_foundry);
    }

    // Save updated config
    config.save(project_dir)?;
    println!();
    println!("Configuration updated.");

    Ok(())
}

/// Refresh endpoint URLs for existing Foundry configs using ARM data
pub(crate) fn refresh_foundry_endpoints(
    existing: &mut [FoundryServiceConfig],
    accounts: &[AiServicesAccount],
) {
    for config in existing.iter_mut() {
        if let Some(account) = accounts.iter().find(|a| a.name == config.name) {
            config.endpoint = Some(account.agents_endpoint());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoist_client::arm::AiServicesAccountProperties;

    #[test]
    fn test_refresh_foundry_endpoints() {
        let mut configs = vec![FoundryServiceConfig {
            name: "my-ai-svc".to_string(),
            project: "proj-1".to_string(),
            label: None,
            api_version: "2025-05-15-preview".to_string(),
            endpoint: None,
            subscription: None,
            resource_group: None,
        }];

        let accounts = vec![AiServicesAccount {
            name: "my-ai-svc".to_string(),
            location: "eastus".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties {
                endpoint: Some("https://custom-sub.cognitiveservices.azure.com/".to_string()),
            },
        }];

        refresh_foundry_endpoints(&mut configs, &accounts);

        assert_eq!(
            configs[0].endpoint.as_deref(),
            Some("https://custom-sub.services.ai.azure.com")
        );
    }

    #[test]
    fn test_refresh_foundry_endpoints_no_match() {
        let mut configs = vec![FoundryServiceConfig {
            name: "my-ai-svc".to_string(),
            project: "proj-1".to_string(),
            label: None,
            api_version: "2025-05-15-preview".to_string(),
            endpoint: Some("https://old-endpoint.services.ai.azure.com".to_string()),
            subscription: None,
            resource_group: None,
        }];

        let accounts = vec![AiServicesAccount {
            name: "different-svc".to_string(),
            location: "eastus".to_string(),
            kind: "AIServices".to_string(),
            id: String::new(),
            properties: AiServicesAccountProperties::default(),
        }];

        refresh_foundry_endpoints(&mut configs, &accounts);

        // Should not change -- no matching account
        assert_eq!(
            configs[0].endpoint.as_deref(),
            Some("https://old-endpoint.services.ai.azure.com")
        );
    }
}
