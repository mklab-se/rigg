//! AI feature configuration commands (hoist ai init/status/remove)

use std::io::{self, BufRead, Write};

use anyhow::Result;

use hoist_client::arm::AiServicesAccount;
use hoist_core::config::{AiConfig, AiProvider};

use crate::cli::AiCommands;
use crate::commands::init::{DiscoveryContext, try_authenticate};

pub async fn run(cmd: AiCommands) -> Result<()> {
    match cmd {
        AiCommands::Init {
            account,
            deployment,
            provider,
            model,
        } => run_init(account, deployment, provider, model).await,
        AiCommands::Status => run_status(),
        AiCommands::Remove => run_remove(),
    }
}

async fn run_init(
    account_flag: Option<String>,
    deployment_flag: Option<String>,
    provider_flag: Option<String>,
    model_flag: Option<String>,
) -> Result<()> {
    let (project_root, mut config) = crate::commands::load_config()?;

    println!("Configuring AI features for hoist...");
    println!();

    // If --provider flag is specified, parse it directly
    let selected_provider = if let Some(ref name) = provider_flag {
        parse_provider_name(name)?
    } else {
        // Detect available providers and let user choose
        let available = hoist_client::local_agent::detect_available_providers();

        if available.is_empty() {
            anyhow::bail!(
                "No AI providers detected. Install one of the following:\n\
                 \x20 - claude  (Anthropic Claude CLI)\n\
                 \x20 - codex   (OpenAI Codex CLI)\n\
                 \x20 - copilot (GitHub Copilot CLI)\n\
                 \x20 - ollama  (Ollama for local LLMs)\n\
                 \x20 - az      (Azure CLI for Azure OpenAI API)"
            );
        }

        println!("Available AI providers:");
        for (i, p) in available.iter().enumerate() {
            println!(
                "  {}. {:<14} — {}",
                i + 1,
                p.display_name(),
                p.description()
            );
        }

        let default_idx = 0;
        print!("\nWhich provider should hoist use? [{}]: ", default_idx + 1);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();

        let idx = if input.is_empty() {
            default_idx
        } else {
            let n: usize = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid selection: {}", input))?;
            if n < 1 || n > available.len() {
                anyhow::bail!("Selection out of range: {}", n);
            }
            n - 1
        };

        available[idx].clone()
    };

    // Provider-specific setup
    let ai_config = match selected_provider {
        AiProvider::AzureOpenai => {
            setup_azure_openai(account_flag, deployment_flag, model_flag).await?
        }
        AiProvider::Claude | AiProvider::Codex | AiProvider::Copilot => {
            setup_local_agent(&selected_provider, model_flag)?
        }
        AiProvider::Ollama => setup_ollama(model_flag).await?,
    };

    config.ai = Some(ai_config.clone());
    config.save(&project_root)?;

    println!();
    println!(
        "AI features configured: {} {}",
        ai_config.provider.display_name(),
        ai_config
            .effective_model()
            .map(|m| format!("(model: {m})"))
            .unwrap_or_default()
    );
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

/// Set up Azure OpenAI with ARM discovery
async fn setup_azure_openai(
    account_flag: Option<String>,
    deployment_flag: Option<String>,
    model_flag: Option<String>,
) -> Result<AiConfig> {
    let ctx = try_authenticate().await?;

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

    let selected_account = match &account_flag {
        Some(name) => accounts
            .iter()
            .find(|a| a.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow::anyhow!("AI Services account '{}' not found", name))?,
        None => {
            for a in &accounts {
                println!("  Found: {}", a);
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

    Ok(AiConfig {
        provider: AiProvider::AzureOpenai,
        model: model_flag,
        account: Some(selected_account.name.clone()),
        deployment: Some(deployment_name),
        endpoint: None,
        subscription: Some(ctx.subscription_id.clone()),
        resource_group: None,
        api_version: "2024-12-01-preview".to_string(),
        ollama_url: None,
    })
}

/// Set up a local CLI agent (claude, codex, copilot)
fn setup_local_agent(provider: &AiProvider, model_flag: Option<String>) -> Result<AiConfig> {
    let model = if let Some(m) = model_flag {
        m
    } else {
        let default = provider.default_model().unwrap_or("default");
        println!();
        println!(
            "  {} uses '{}' as the recommended model.",
            provider.display_name(),
            default
        );

        print!("  Model [{}]: ", default);
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();

        if input.is_empty() {
            default.to_string()
        } else {
            input.to_string()
        }
    };

    Ok(AiConfig {
        provider: provider.clone(),
        model: Some(model),
        account: None,
        deployment: None,
        endpoint: None,
        subscription: None,
        resource_group: None,
        api_version: "2024-12-01-preview".to_string(),
        ollama_url: None,
    })
}

/// Set up Ollama with model selection from installed models
async fn setup_ollama(model_flag: Option<String>) -> Result<AiConfig> {
    println!();
    println!("  Connecting to Ollama...");

    let client = hoist_client::ollama::OllamaClient::new(None);
    let models = client
        .list_models()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to Ollama. Is it running? {e}"))?;

    if models.is_empty() {
        anyhow::bail!(
            "No models installed in Ollama. Install one first:\n\
             \x20 ollama pull gemma3:4b\n\
             \x20 ollama pull llama3:8b"
        );
    }

    let model_name = if let Some(m) = model_flag {
        m
    } else {
        for (i, m) in models.iter().enumerate() {
            println!(
                "  {}. {:<24} ({})",
                i + 1,
                m.name,
                hoist_client::ollama::format_model_size(m.size)
            );
        }

        print!("\n  Which model? [1]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        let input = input.trim();

        let idx = if input.is_empty() {
            0
        } else {
            let n: usize = input
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid selection: {}", input))?;
            if n < 1 || n > models.len() {
                anyhow::bail!("Selection out of range: {}", n);
            }
            n - 1
        };

        models[idx].name.clone()
    };

    // Ask for custom URL
    print!("  Ollama URL [http://localhost:11434]: ");
    io::stdout().flush()?;

    let mut url_input = String::new();
    io::stdin().lock().read_line(&mut url_input)?;
    let url_input = url_input.trim();

    let ollama_url = if url_input.is_empty() || url_input == "http://localhost:11434" {
        None
    } else {
        Some(url_input.to_string())
    };

    Ok(AiConfig {
        provider: AiProvider::Ollama,
        model: Some(model_name),
        account: None,
        deployment: None,
        endpoint: None,
        subscription: None,
        resource_group: None,
        api_version: "2024-12-01-preview".to_string(),
        ollama_url,
    })
}

fn run_status() -> Result<()> {
    let (_project_root, config) = crate::commands::load_config()?;

    match &config.ai {
        Some(ai) => {
            println!("AI features: configured");
            println!("  Provider:    {}", ai.provider.display_name());

            if let Some(model) = ai.effective_model() {
                println!("  Model:       {}", model);
            }

            match ai.provider {
                AiProvider::AzureOpenai => {
                    if let Some(ref account) = ai.account {
                        println!("  Account:     {}", account);
                    }
                    if let Some(ref deployment) = ai.deployment {
                        println!("  Deployment:  {}", deployment);
                    }
                    if let Some(ref endpoint) = ai.openai_endpoint() {
                        println!("  Endpoint:    {}", endpoint);
                    }
                    println!("  API version: {}", ai.api_version);

                    // Test authentication
                    match hoist_client::auth::get_cognitive_services_auth() {
                        Ok(provider) => match provider.get_token() {
                            Ok(_) => println!("  Auth:        OK ({})", provider.method_name()),
                            Err(e) => println!("  Auth:        Failed - {}", e),
                        },
                        Err(e) => println!("  Auth:        Not configured - {}", e),
                    }
                }
                AiProvider::Claude | AiProvider::Codex | AiProvider::Copilot => {
                    let binary = ai.provider.binary_name().unwrap_or("unknown");
                    let available = hoist_client::local_agent::is_available(&ai.provider);
                    println!(
                        "  Status:      {} ({})",
                        if available { "Available" } else { "Not found" },
                        binary
                    );
                }
                AiProvider::Ollama => {
                    let url = ai.ollama_base_url();
                    println!("  Ollama URL:  {}", url);
                    // Quick connectivity check
                    let reachable = std::net::TcpStream::connect_timeout(
                        &std::net::SocketAddr::from(([127, 0, 0, 1], 11434)),
                        std::time::Duration::from_millis(500),
                    )
                    .is_ok();
                    println!(
                        "  Status:      {}",
                        if reachable {
                            "Connected"
                        } else {
                            "Not reachable"
                        }
                    );
                }
            }
        }
        None => {
            println!("AI features: not configured");
            println!();
            println!("Run 'hoist ai init' to set up an AI provider.");
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

/// Parse a provider name string into an AiProvider
fn parse_provider_name(name: &str) -> Result<AiProvider> {
    match name.to_lowercase().as_str() {
        "azure-openai" | "azure_openai" | "azureopenai" => Ok(AiProvider::AzureOpenai),
        "claude" => Ok(AiProvider::Claude),
        "codex" => Ok(AiProvider::Codex),
        "copilot" => Ok(AiProvider::Copilot),
        "ollama" => Ok(AiProvider::Ollama),
        _ => {
            let valid: Vec<&str> = AiProvider::all()
                .iter()
                .filter_map(|p| {
                    // Use binary_name for local agents, or a fixed string for Azure
                    p.binary_name().or(if matches!(p, AiProvider::AzureOpenai) {
                        Some("azure-openai")
                    } else {
                        None
                    })
                })
                .collect();
            anyhow::bail!(
                "Unknown provider '{}'. Valid options: {}",
                name,
                valid.join(", ")
            )
        }
    }
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
