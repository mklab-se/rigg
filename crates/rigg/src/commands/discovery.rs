//! Shared Azure service discovery for interactive wizards (`rigg init`,
//! `rigg env add`).
//!
//! Both wizards need the same ARM discovery + numbered pick-list flow, so it
//! lives here once rather than being duplicated per command.

use std::io::{BufRead, Write};

use anyhow::{Result, anyhow};

use rigg_client::arm::ArmClient;

pub(crate) type Discovered = (Option<String>, Option<(String, String)>);

/// Discover services via ARM and let the user pick interactively.
pub(crate) async fn discover_interactive() -> Result<Discovered> {
    println!("Discovering Azure services (via Azure CLI credentials)...");
    let arm = match ArmClient::new() {
        Ok(arm) => arm,
        Err(e) => {
            // Render via anyhow's alternate Display so the full source chain
            // (e.g. reqwest detail under ClientError::Request) is shown, not
            // just the top-level message.
            let e = anyhow::Error::from(e);
            println!("  ARM discovery unavailable ({e:#}); falling back to manual entry.");
            return manual_entry();
        }
    };
    let subs = match arm.list_subscriptions().await {
        Ok(subs) if !subs.is_empty() => subs,
        _ => {
            println!("  No subscriptions visible; falling back to manual entry.");
            return manual_entry();
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

    let search = pick("Azure AI Search service", &search_services)?;
    let foundry_labels: Vec<String> = foundry_projects
        .iter()
        .map(|(a, p)| format!("{a}/{p}"))
        .collect();
    let foundry = pick("Microsoft Foundry project", &foundry_labels)?.and_then(|label| {
        foundry_projects
            .iter()
            .find(|(a, p)| format!("{a}/{p}") == label)
            .cloned()
    });
    Ok((search, foundry))
}

fn manual_entry() -> Result<Discovered> {
    let search = ask("Azure AI Search service name (empty to skip): ")?;
    let account = ask("Foundry account name (empty to skip): ")?;
    let foundry = match account {
        Some(account) => {
            let project = ask("Foundry project name: ")?
                .ok_or_else(|| anyhow!("a Foundry project name is required with an account"))?;
            Some((account, project))
        }
        None => None,
    };
    Ok((search, foundry))
}

/// Numbered pick from a list; empty list or "0" skips.
fn pick(label: &str, options: &[String]) -> Result<Option<String>> {
    if options.is_empty() {
        println!("  No {label} found — skipping.");
        return Ok(None);
    }
    println!();
    println!("Select {label} (0 to skip):");
    for (i, opt) in options.iter().enumerate() {
        println!("  {}. {}", i + 1, opt);
    }
    loop {
        let answer = ask("> ")?.unwrap_or_default();
        if answer.is_empty() || answer == "0" {
            return Ok(None);
        }
        if let Ok(n) = answer.parse::<usize>() {
            if n >= 1 && n <= options.len() {
                return Ok(Some(options[n - 1].clone()));
            }
        }
        println!("  Enter a number between 0 and {}.", options.len());
    }
}

fn ask(prompt: &str) -> Result<Option<String>> {
    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut input = String::new();
    std::io::stdin().lock().read_line(&mut input)?;
    let trimmed = input.trim().to_string();
    Ok((!trimmed.is_empty()).then_some(trimmed))
}
