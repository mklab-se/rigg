//! Shared Azure service discovery for interactive wizards (`rigg init`,
//! `rigg env add`).
//!
//! Both wizards need the same ARM discovery + pick-list flow, so it lives
//! here once rather than being duplicated per command.

use anyhow::{Result, anyhow};

use rigg_client::arm::ArmClient;

use crate::commands::interactive;

pub(crate) type Discovered = (Option<String>, Option<(String, String)>);

/// Discover services via ARM and let the user pick interactively.
pub(crate) async fn discover_interactive(plain: bool) -> Result<Discovered> {
    println!("Discovering Azure services (via Azure CLI credentials)...");
    let arm = match ArmClient::new() {
        Ok(arm) => arm,
        Err(e) => {
            // Render via anyhow's alternate Display so the full source chain
            // (e.g. reqwest detail under ClientError::Request) is shown, not
            // just the top-level message.
            let e = anyhow::Error::from(e);
            println!("  ARM discovery unavailable ({e:#}); falling back to manual entry.");
            return manual_entry(plain);
        }
    };
    let subs = match arm.list_subscriptions().await {
        Ok(subs) if !subs.is_empty() => subs,
        _ => {
            println!("  No subscriptions visible; falling back to manual entry.");
            return manual_entry(plain);
        }
    };

    let mut search_services: Vec<String> = Vec::new();
    let mut foundry_projects: Vec<(String, String)> = Vec::new();
    for sub in &subs {
        if let Ok(services) = arm.list_search_services(&sub.subscription_id).await {
            search_services.extend(services.into_iter().map(|s| s.name));
        }
        if let Ok(accounts) = arm.list_ai_services_accounts(&sub.subscription_id).await {
            for account in accounts {
                let rg = account
                    .id
                    .split('/')
                    .skip_while(|s| !s.eq_ignore_ascii_case("resourceGroups"))
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                if rg.is_empty() {
                    continue;
                }
                if let Ok(projects) = arm
                    .list_foundry_projects(&account, &sub.subscription_id)
                    .await
                {
                    for project in projects {
                        foundry_projects
                            .push((account.name.clone(), project.display_name().to_string()));
                    }
                }
            }
        }
    }

    let search = pick("Azure AI Search service:", &search_services, plain)?;
    let foundry_labels: Vec<String> = foundry_projects
        .iter()
        .map(|(a, p)| format!("{a}/{p}"))
        .collect();
    let foundry = pick("Microsoft Foundry project:", &foundry_labels, plain)?.and_then(|label| {
        foundry_projects
            .iter()
            .find(|(a, p)| format!("{a}/{p}") == label)
            .cloned()
    });
    Ok((search, foundry))
}

fn manual_entry(plain: bool) -> Result<Discovered> {
    let search = optional(interactive::text(
        "Azure AI Search service name (empty to skip):",
        plain,
    )?);
    let account = optional(interactive::text(
        "Foundry account name (empty to skip):",
        plain,
    )?);
    let foundry = match account {
        Some(account) => {
            let project = optional(interactive::text("Foundry project name:", plain)?)
                .ok_or_else(|| anyhow!("a Foundry project name is required with an account"))?;
            Some((account, project))
        }
        None => None,
    };
    Ok((search, foundry))
}

const SKIP: &str = "(skip — none)";

/// Arrow-key pick from a list, with an explicit skip row; empty list skips.
fn pick(label: &str, options: &[String], plain: bool) -> Result<Option<String>> {
    if options.is_empty() {
        println!("  No {} found — skipping.", label.trim_end_matches(':'));
        return Ok(None);
    }
    let mut rows = options.to_vec();
    rows.push(SKIP.to_string());
    let choice = interactive::select(label, rows, plain)?;
    Ok((choice != SKIP).then_some(choice))
}

fn optional(answer: String) -> Option<String> {
    let trimmed = answer.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}
