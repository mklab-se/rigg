//! ARM-based service discovery for Azure AI Search and Microsoft Foundry

use anyhow::Result;

use rigg_client::arm::{AiServicesAccount, ArmClient, SearchService};
use rigg_client::auth::AzCliAuth;
use rigg_core::config::{FoundryServiceConfig, SearchServiceConfig};

use super::prompts::{prompt_multi_selection, prompt_selection};

/// Authenticated ARM context for discovery
pub(crate) struct DiscoveryContext {
    pub(crate) arm: ArmClient,
    pub(crate) subscription_id: String,
}

/// Try to authenticate and select a subscription for ARM discovery
pub(crate) async fn try_authenticate() -> Result<DiscoveryContext> {
    let status = AzCliAuth::check_status().map_err(|e| {
        println!(
            "Not logged in to Azure CLI. Run 'az login' for auto-discovery, or enter manually."
        );
        anyhow::anyhow!("{}", e)
    })?;

    if let Some(user) = &status.user {
        println!("Checking Azure authentication... logged in as {}", user);
    }
    println!();

    let arm = ArmClient::new()?;

    println!("Fetching subscriptions...");
    let subscriptions = arm.list_subscriptions().await?;

    if subscriptions.is_empty() {
        anyhow::bail!("No Azure subscriptions found. Check your Azure access permissions.");
    }

    let default_idx = status
        .subscription_id
        .as_ref()
        .and_then(|id| subscriptions.iter().position(|s| &s.subscription_id == id))
        .unwrap_or(0);

    let selected_sub = prompt_selection("Select subscription", &subscriptions, default_idx)?;
    println!();

    Ok(DiscoveryContext {
        arm,
        subscription_id: selected_sub.subscription_id.clone(),
    })
}

/// Discover a Foundry service and project via ARM APIs
pub(super) async fn discover_foundry_service(
    ctx: &DiscoveryContext,
) -> Result<Option<FoundryServiceConfig>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        return Ok(None);
    }

    println!("Fetching AI Services accounts...");
    let accounts = ctx
        .arm
        .list_ai_services_accounts(&ctx.subscription_id)
        .await?;

    if accounts.is_empty() {
        println!("  No AI Services accounts found in this subscription.");
        return Ok(None);
    }

    let selected_account = auto_select_or_prompt("Select AI Services account", &accounts, 0)?;

    println!("Fetching Microsoft Foundry projects...");
    let projects = ctx
        .arm
        .list_foundry_projects(selected_account, &ctx.subscription_id)
        .await?;

    if projects.is_empty() {
        println!("  No Foundry projects found for this account.");
        return Ok(None);
    }

    let selected_project = auto_select_or_prompt("Select Foundry project", &projects, 0)?;

    Ok(Some(FoundryServiceConfig {
        name: selected_account.name.clone(),
        project: selected_project.display_name().to_string(),
        label: None,
        api_version: "2025-05-15-preview".to_string(),
        endpoint: Some(selected_account.agents_endpoint()),
        subscription: Some(ctx.subscription_id.clone()),
        resource_group: None,
    }))
}

/// Discover search services for fresh init (multi-select)
pub(super) async fn discover_search_services_fresh(
    ctx: &DiscoveryContext,
) -> Result<Vec<SearchServiceConfig>> {
    if !crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        return Ok(vec![]);
    }

    println!("Fetching Azure AI Search services...");
    let services = ctx.arm.list_search_services(&ctx.subscription_id).await?;

    if services.is_empty() {
        println!("  No search services found in this subscription.");
        return Ok(vec![]);
    }

    let selected = prompt_multi_selection("Add services", &services)?;
    Ok(selected
        .into_iter()
        .map(|s| SearchServiceConfig {
            name: s.name.clone(),
            label: None,
            subscription: Some(ctx.subscription_id.clone()),
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        })
        .collect())
}

/// Discover search services not yet configured (additive mode)
pub(crate) async fn discover_new_search_services(
    ctx: &DiscoveryContext,
    existing: &[SearchServiceConfig],
) -> Result<Vec<SearchServiceConfig>> {
    println!("Fetching Azure AI Search services...");
    let all_services = ctx.arm.list_search_services(&ctx.subscription_id).await?;

    // Show already configured
    for svc in existing {
        println!("  [x] {} (already configured)", svc.name);
    }

    // Filter to not-yet-configured
    let existing_names: Vec<&str> = existing.iter().map(|s| s.name.as_str()).collect();
    let new_services: Vec<&SearchService> = all_services
        .iter()
        .filter(|s| !existing_names.contains(&s.name.as_str()))
        .collect();

    if new_services.is_empty() {
        if existing.is_empty() {
            println!("  No search services found.");
        } else {
            println!("  No additional search services found.");
        }
        return Ok(vec![]);
    }

    let selected = prompt_multi_selection("Add services", &new_services)?;
    Ok(selected
        .into_iter()
        .map(|s| SearchServiceConfig {
            name: s.name.clone(),
            label: None,
            subscription: Some(ctx.subscription_id.clone()),
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        })
        .collect())
}

/// Discover foundry services/projects not yet configured (additive mode)
pub(crate) async fn discover_new_foundry_services(
    ctx: &DiscoveryContext,
    existing: &[FoundryServiceConfig],
    accounts: &[AiServicesAccount],
) -> Result<Vec<FoundryServiceConfig>> {
    // Show already configured
    for svc in existing {
        println!("  [x] {} / {} (already configured)", svc.name, svc.project);
    }

    if accounts.is_empty() {
        println!("  No AI Services accounts found.");
        return Ok(vec![]);
    }

    let selected_accounts = prompt_multi_selection("Add accounts", accounts)?;

    let mut new_configs = Vec::new();
    for account in selected_accounts {
        println!("Fetching projects for {}...", account.name);
        let projects = ctx
            .arm
            .list_foundry_projects(account, &ctx.subscription_id)
            .await?;

        // Filter out already-configured project/account pairs
        let new_projects: Vec<_> = projects
            .iter()
            .filter(|p| {
                !existing
                    .iter()
                    .any(|e| e.name == account.name && e.project == p.display_name())
            })
            .collect();

        if new_projects.is_empty() {
            println!("  No new projects found.");
            continue;
        }

        let selected_projects = prompt_multi_selection("Add projects", &new_projects)?;
        for project in selected_projects {
            new_configs.push(FoundryServiceConfig {
                name: account.name.clone(),
                project: project.display_name().to_string(),
                label: None,
                api_version: "2025-05-15-preview".to_string(),
                endpoint: Some(account.agents_endpoint()),
                subscription: Some(ctx.subscription_id.clone()),
                resource_group: None,
            });
        }
    }
    Ok(new_configs)
}

/// Auto-select if only one item, otherwise prompt for selection
fn auto_select_or_prompt<'a, T: std::fmt::Display>(
    label: &str,
    items: &'a [T],
    default: usize,
) -> Result<&'a T> {
    if items.len() == 1 {
        println!("  Found: {}", items[0]);
        return Ok(&items[0]);
    }
    prompt_selection(label, items, default)
}
