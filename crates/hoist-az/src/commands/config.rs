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
        project_root.join(hoist_core::Config::FILENAME).display()
    );

    println!();
    println!("project:");
    if let Some(ref name) = config.project.name {
        println!("  name: \"{}\"", name);
    }
    if let Some(ref desc) = config.project.description {
        println!("  description: \"{}\"", desc);
    }

    println!();
    println!("sync:");
    println!("  include_preview: {}", config.sync.include_preview);

    println!();
    println!("environments:");
    for (env_name, env_config) in &config.environments {
        let default_marker = if env_config.default { " (default)" } else { "" };
        println!("  {}:{}", env_name, default_marker);
        for svc in &env_config.search {
            println!("    search: {}", svc.name);
            println!("      api_version: {}", svc.api_version);
            if let Some(ref sub) = svc.subscription {
                println!("      subscription: {}", sub);
            }
        }
        for svc in &env_config.foundry {
            println!("    foundry: {}/{}", svc.name, svc.project);
            println!("      api_version: {}", svc.api_version);
        }
    }

    Ok(())
}

async fn set(key: &str, value: &str) -> Result<()> {
    let (project_root, mut config) = load_config()?;

    match key {
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
    println!("Use 'hoist init' to create a new project or 'hoist env add' to add environments.");
    Ok(())
}
