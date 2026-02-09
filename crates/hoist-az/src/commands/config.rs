//! Configuration management commands

use anyhow::Result;

use crate::cli::ConfigCommands;
use crate::commands::load_config;

pub async fn run(cmd: ConfigCommands) -> Result<()> {
    match cmd {
        ConfigCommands::Show => show().await,
        ConfigCommands::Set { key, value } => set(&key, &value).await,
        ConfigCommands::Init => init().await,
    }
}

async fn show() -> Result<()> {
    let (project_root, config) = load_config()?;

    println!(
        "Configuration: {}",
        project_root.join("hoist.toml").display()
    );

    if let Some(ref svc) = config.service {
        println!();
        println!("[service]");
        println!("  name = \"{}\"", svc.name);
        if let Some(ref sub) = svc.subscription {
            println!("  subscription = \"{}\"", sub);
        }
        if let Some(ref rg) = svc.resource_group {
            println!("  resource_group = \"{}\"", rg);
        }
        println!("  api_version = \"{}\"", svc.api_version);
        println!("  preview_api_version = \"{}\"", svc.preview_api_version);
    }

    for (i, svc) in config.services.search.iter().enumerate() {
        println!();
        println!("[[services.search]] #{}", i + 1);
        println!("  name = \"{}\"", svc.name);
        if let Some(ref sub) = svc.subscription {
            println!("  subscription = \"{}\"", sub);
        }
        println!("  api_version = \"{}\"", svc.api_version);
    }

    for (i, svc) in config.services.foundry.iter().enumerate() {
        println!();
        println!("[[services.foundry]] #{}", i + 1);
        println!("  name = \"{}\"", svc.name);
        println!("  project = \"{}\"", svc.project);
        println!("  api_version = \"{}\"", svc.api_version);
    }

    println!();
    println!("[project]");
    if let Some(ref name) = config.project.name {
        println!("  name = \"{}\"", name);
    }
    if let Some(ref desc) = config.project.description {
        println!("  description = \"{}\"", desc);
    }

    println!();
    println!("[sync]");
    println!("  include_preview = {}", config.sync.include_preview);
    if !config.sync.resources.is_empty() {
        println!("  resources = {:?}", config.sync.resources);
    }

    println!();
    println!("Service URL: {}", config.service_url());

    Ok(())
}

async fn set(key: &str, value: &str) -> Result<()> {
    let (project_root, mut config) = load_config()?;

    // For service.* keys, ensure the legacy service section exists
    if key.starts_with("service.") && config.service.is_none() {
        config.service = Some(hoist_core::config::ServiceConfig {
            name: String::new(),
            subscription: None,
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        });
    }

    match key {
        "service.name" => config.service.as_mut().unwrap().name = value.to_string(),
        "service.subscription" => {
            config.service.as_mut().unwrap().subscription = Some(value.to_string())
        }
        "service.resource_group" => {
            config.service.as_mut().unwrap().resource_group = Some(value.to_string())
        }
        "service.api_version" => config.service.as_mut().unwrap().api_version = value.to_string(),
        "service.preview_api_version" => {
            config.service.as_mut().unwrap().preview_api_version = value.to_string()
        }
        "project.name" => config.project.name = Some(value.to_string()),
        "project.description" => config.project.description = Some(value.to_string()),
        "sync.include_preview" => {
            config.sync.include_preview = value
                .parse()
                .map_err(|_| anyhow::anyhow!("Invalid boolean value: {}", value))?;
        }
        _ => anyhow::bail!("Unknown configuration key: {}", key),
    }

    config.save(&project_root)?;
    println!("Set {} = \"{}\"", key, value);

    Ok(())
}

async fn init() -> Result<()> {
    use std::io::{self, BufRead, Write};

    println!("Interactive configuration setup");
    println!("================================");
    println!();

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    // Service name
    print!("Azure Search service name: ");
    stdout.flush()?;
    let mut service_name = String::new();
    stdin.lock().read_line(&mut service_name)?;
    let service_name = service_name.trim().to_string();

    if service_name.is_empty() {
        anyhow::bail!("Service name is required");
    }

    // Subscription (optional)
    print!("Subscription ID (optional, press Enter to skip): ");
    stdout.flush()?;
    let mut subscription = String::new();
    stdin.lock().read_line(&mut subscription)?;
    let subscription = subscription.trim();
    let subscription = if subscription.is_empty() {
        None
    } else {
        Some(subscription.to_string())
    };

    // Create config
    let config = hoist_core::Config {
        service: Some(hoist_core::config::ServiceConfig {
            name: service_name,
            subscription,
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        }),
        services: Default::default(),
        project: hoist_core::config::ProjectConfig::default(),
        sync: hoist_core::config::SyncConfig::default(),
    };

    let current_dir = std::env::current_dir()?;
    config.save(&current_dir)?;

    println!();
    println!("Configuration saved to hoist.toml");

    Ok(())
}
