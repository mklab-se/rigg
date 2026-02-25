//! AI feature configuration commands (hoist ai init/status/remove)

use std::io::{self, BufRead, Write};

use anyhow::Result;

use hoist_client::arm::AiServicesAccount;
use hoist_core::config::AiConfig;

use crate::cli::AiCommands;
use crate::commands::init::{DiscoveryContext, try_authenticate};

pub async fn run(cmd: AiCommands) -> Result<()> {
    match cmd {
        AiCommands::Init {
            account,
            deployment,
        } => run_init(account, deployment).await,
        AiCommands::Status => run_status(),
        AiCommands::Remove => run_remove(),
    }
}

async fn run_init(account_flag: Option<String>, deployment_flag: Option<String>) -> Result<()> {
    let (project_root, mut config) = crate::commands::load_config()?;

    println!("Configuring AI features for hoist...");
    println!();

    let ctx = try_authenticate().await?;

    // Discover AI Services accounts
    println!("Checking Azure AI Services accounts...");
    let accounts = ctx
        .arm
        .list_ai_services_accounts(&ctx.subscription_id)
        .await?;

    if accounts.is_empty() {
        anyhow::bail!(
            "No AI Services accounts found in this subscription. \
             Create one in the Azure portal first."
        );
    }

    // Select or auto-pick account
    let selected_account = match &account_flag {
        Some(name) => accounts
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow::anyhow!("AI Services account '{}' not found", name))?,
        None => {
            // Note shared accounts (also used by Foundry)
            for a in &accounts {
                let shared = config
                    .environments
                    .values()
                    .any(|e| e.foundry.iter().any(|f| f.name == a.name));
                let note = if shared {
                    " — also used by Foundry"
                } else {
                    ""
                };
                println!("  Found: {}{}", a, note);
            }

            if accounts.len() == 1 {
                &accounts[0]
            } else {
                prompt_selection(
                    "Which account should hoist use for AI features?",
                    &accounts,
                    0,
                )?
            }
        }
    };

    println!();
    println!("Using account: {}", selected_account.name);

    // List model deployments
    println!("Checking model deployments on {}...", selected_account.name);
    let deployments = ctx
        .arm
        .list_model_deployments(selected_account, &ctx.subscription_id)
        .await?;

    let deployment_name = match &deployment_flag {
        Some(name) => {
            if !deployments.iter().any(|d| d.name == *name) {
                anyhow::bail!(
                    "Deployment '{}' not found on {}",
                    name,
                    selected_account.name
                );
            }
            name.clone()
        }
        None => {
            if deployments.is_empty() {
                println!("  No model deployments found.");
                println!();
                if !crate::commands::confirm::prompt_yes_default(
                    "Create a gpt-4o-mini deployment? (recommended for fast, low-cost AI features)",
                )? {
                    anyhow::bail!(
                        "AI features require a model deployment. Run 'hoist ai init' again after creating one."
                    );
                }
                create_deployment(&ctx, selected_account).await?
            } else {
                for d in &deployments {
                    println!("  Found: {}", d);
                }

                // Prefer gpt-4o-mini if available
                let default_idx = deployments
                    .iter()
                    .position(|d| d.properties.model.name.contains("4o-mini"))
                    .unwrap_or(0);

                if deployments.len() == 1 {
                    deployments[0].name.clone()
                } else {
                    let tip = if default_idx > 0 {
                        format!(
                            "  Tip: {} is recommended (fast, low cost)",
                            deployments[default_idx].name
                        )
                    } else {
                        String::new()
                    };
                    if !tip.is_empty() {
                        println!("{}", tip);
                    }
                    let selected = prompt_selection(
                        "Which deployment should hoist use?",
                        &deployments,
                        default_idx,
                    )?;
                    selected.name.clone()
                }
            }
        }
    };

    // Build AiConfig
    let ai_config = AiConfig {
        account: selected_account.name.clone(),
        deployment: deployment_name.clone(),
        endpoint: None,
        subscription: Some(ctx.subscription_id.clone()),
        resource_group: None,
        api_version: "2024-12-01-preview".to_string(),
    };

    config.ai = Some(ai_config);
    config.save(&project_root)?;

    println!();
    println!("AI features configured in hoist.yaml.");
    println!();
    println!("AI explanations are now enabled by default for:");
    println!("  hoist diff          AI-enhanced diff summaries");
    println!("  hoist pull          AI summary of pulled changes");
    println!("  hoist push          AI summary of pushed changes");
    println!();
    println!("To suppress AI explanations for a single command:");
    println!("  hoist diff --no-explain");
    println!();
    println!("Other commands:");
    println!("  hoist ai status     Check AI configuration");
    println!("  hoist ai remove     Remove AI configuration");

    Ok(())
}

fn run_status() -> Result<()> {
    let (_project_root, config) = crate::commands::load_config()?;

    match &config.ai {
        Some(ai) => {
            println!("AI features: configured");
            println!("  Account:    {}", ai.account);
            println!("  Deployment: {}", ai.deployment);
            println!("  Endpoint:   {}", ai.openai_endpoint());
            println!("  API version: {}", ai.api_version);

            // Test authentication
            match hoist_client::auth::get_cognitive_services_auth() {
                Ok(provider) => match provider.get_token() {
                    Ok(_) => println!("  Auth:       OK ({})", provider.method_name()),
                    Err(e) => println!("  Auth:       Failed - {}", e),
                },
                Err(e) => println!("  Auth:       Not configured - {}", e),
            }
        }
        None => {
            println!("AI features: not configured");
            println!();
            println!("Run 'hoist ai init' to set up Azure OpenAI integration.");
        }
    }

    Ok(())
}

fn run_remove() -> Result<()> {
    let (project_root, mut config) = crate::commands::load_config()?;

    if config.ai.is_none() {
        println!("AI features are not configured.");
        return Ok(());
    }

    config.ai = None;
    config.save(&project_root)?;
    println!("AI configuration removed from hoist.yaml.");

    Ok(())
}

async fn create_deployment(ctx: &DiscoveryContext, account: &AiServicesAccount) -> Result<String> {
    let name = "gpt-4o-mini".to_string();
    println!("Creating deployment '{}'...", name);

    ctx.arm
        .create_model_deployment(
            account,
            &ctx.subscription_id,
            &name,
            "gpt-4o-mini",
            "2024-07-18",
        )
        .await?;

    println!("  Deployment '{}' created.", name);
    Ok(name)
}

/// Prompt user to select from a numbered list
fn prompt_selection<'a, T: std::fmt::Display>(
    prompt: &str,
    items: &'a [T],
    default: usize,
) -> Result<&'a T> {
    println!();
    for (i, item) in items.iter().enumerate() {
        let marker = if i == default { " (Recommended)" } else { "" };
        println!("  {}. {}{}", i + 1, item, marker);
    }

    print!("{} [{}]: ", prompt, default + 1);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().lock().read_line(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(&items[default]);
    }

    let index: usize = input
        .parse::<usize>()
        .map_err(|_| anyhow::anyhow!("Invalid selection: {}", input))?;

    if index < 1 || index > items.len() {
        anyhow::bail!("Selection out of range: {}", index);
    }

    Ok(&items[index - 1])
}
