//! Environment management commands

use anyhow::Result;

use hoist_core::config::EnvironmentConfig;
use hoist_core::resources::ResourceKind;
use hoist_core::service::ServiceDomain;

use crate::cli::EnvCommands;
use crate::commands::load_config;

pub async fn run(cmd: EnvCommands) -> Result<()> {
    match cmd {
        EnvCommands::List => list().await,
        EnvCommands::Show { name } => show(name).await,
        EnvCommands::SetDefault { name } => set_default(&name).await,
        EnvCommands::Add { name } => add(&name).await,
        EnvCommands::Remove { name } => remove(&name).await,
    }
}

async fn list() -> Result<()> {
    let (_project_root, config) = load_config()?;

    let default_name = config.default_env_name().unwrap_or("");

    for name in config.environment_names() {
        let env_config = &config.environments[name];
        let search_count = env_config.search.len();
        let foundry_count = env_config.foundry.len();
        let is_default = name == default_name;

        let mut parts = Vec::new();
        if search_count > 0 {
            parts.push(format!("search: {}", search_count));
        }
        if foundry_count > 0 {
            parts.push(format!("foundry: {}", foundry_count));
        }

        let suffix = if is_default { " (default)" } else { "" };
        println!("  {}  {}{}", name, parts.join(", "), suffix);
    }

    Ok(())
}

async fn show(name: Option<String>) -> Result<()> {
    let (_project_root, config) = load_config()?;

    let env_name = match name {
        Some(ref n) => n.as_str(),
        None => config.default_env_name().ok_or_else(|| {
            anyhow::anyhow!("No default environment set. Specify one with: hoist env show <name>")
        })?,
    };

    let env_config = config.environments.get(env_name).ok_or_else(|| {
        anyhow::anyhow!(
            "Environment '{}' not found. Available: {}",
            env_name,
            config.environment_names().join(", ")
        )
    })?;

    let is_default = config.default_env_name() == Some(env_name);
    let default_tag = if is_default { " (default)" } else { "" };
    println!("Environment: {}{}", env_name, default_tag);

    if let Some(ref desc) = env_config.description {
        println!("Description: {}", desc);
    }

    if !env_config.search.is_empty() {
        println!();
        println!("  Search services:");
        for svc in &env_config.search {
            let mut detail = String::new();
            if let Some(ref label) = svc.label {
                detail.push_str(&format!(" [{}]", label));
            }
            if let Some(ref sub) = svc.subscription {
                detail.push_str(&format!(" (subscription: {})", sub));
            }
            println!("    {}{}", svc.name, detail);
        }
    }

    if !env_config.foundry.is_empty() {
        println!();
        println!("  Foundry services:");
        for svc in &env_config.foundry {
            let mut detail = String::new();
            if let Some(ref label) = svc.label {
                detail.push_str(&format!(" [{}]", label));
            }
            println!("    {} / {}{}", svc.name, svc.project, detail);
        }
    }

    Ok(())
}

async fn set_default(name: &str) -> Result<()> {
    let (project_root, mut config) = load_config()?;

    if !config.environments.contains_key(name) {
        anyhow::bail!(
            "Environment '{}' not found. Available: {}",
            name,
            config.environment_names().join(", ")
        );
    }

    // Clear all defaults
    for env in config.environments.values_mut() {
        env.default = false;
    }

    // Set the named env as default
    config.environments.get_mut(name).unwrap().default = true;
    config.save(&project_root)?;

    println!("Default environment set to '{}'", name);

    Ok(())
}

async fn add(name: &str) -> Result<()> {
    let (project_root, mut config) = load_config()?;

    if config.environments.contains_key(name) {
        anyhow::bail!(
            "Environment '{}' already exists. Use 'hoist init' to update it.",
            name
        );
    }

    println!("Adding environment '{}'", name);
    println!();

    let ctx = super::init::try_authenticate().await?;

    let mut env_config = EnvironmentConfig {
        default: false,
        description: None,
        search: vec![],
        foundry: vec![],
    };

    // Discover search services
    if crate::commands::confirm::prompt_yes_default("Manage Azure AI Search resources?")? {
        let search_services =
            super::init::discover_new_search_services(&ctx, &env_config.search).await?;
        env_config.search = search_services;
    }

    // Discover foundry services
    if crate::commands::confirm::prompt_yes_default("Manage Microsoft Foundry agents?")? {
        let accounts = ctx
            .arm
            .list_ai_services_accounts(&ctx.subscription_id)
            .await?;
        let foundry_services =
            super::init::discover_new_foundry_services(&ctx, &env_config.foundry, &accounts)
                .await?;
        env_config.foundry = foundry_services;
    }

    if env_config.search.is_empty() && env_config.foundry.is_empty() {
        anyhow::bail!("No services configured. Environment not added.");
    }

    // Insert the new environment
    config.environments.insert(name.to_string(), env_config);

    // If this is the only env and none are default, make it default
    let has_default = config.environments.values().any(|e| e.default);
    if !has_default && config.environments.len() == 1 {
        config.environments.get_mut(name).unwrap().default = true;
    }

    // Resolve to create directories
    let env = config
        .resolve_env(Some(name))
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let files_root = config.files_root(&project_root);

    // Create search directories (under files_root)
    for search_svc in &env.search {
        let search_base = env.search_service_dir(&files_root, search_svc);
        for kind in ResourceKind::search_kinds() {
            if kind.domain() == ServiceDomain::Search {
                let dir = search_base.join(kind.directory_name());
                std::fs::create_dir_all(&dir)?;
            }
        }
    }

    // Create foundry directories (under files_root)
    for foundry_svc in &env.foundry {
        let foundry_base = env.foundry_service_dir(&files_root, foundry_svc);
        std::fs::create_dir_all(foundry_base.join("agents"))?;
    }

    // Create per-env state directory (always in project_root)
    let state_dir = project_root.join(".hoist").join(name);
    std::fs::create_dir_all(&state_dir)?;

    // Save config
    config.save(&project_root)?;

    println!();
    println!("Environment '{}' added.", name);

    let search_count = env.search.len();
    let foundry_count = env.foundry.len();
    if search_count > 0 {
        println!("  Search services: {}", search_count);
    }
    if foundry_count > 0 {
        println!("  Foundry services: {}", foundry_count);
    }
    println!();
    println!("Pull resources with: hoist pull --all --env {}", name);

    Ok(())
}

async fn remove(name: &str) -> Result<()> {
    let (project_root, mut config) = load_config()?;

    if !config.environments.contains_key(name) {
        anyhow::bail!(
            "Environment '{}' not found. Available: {}",
            name,
            config.environment_names().join(", ")
        );
    }

    if config.environments.len() == 1 {
        anyhow::bail!("Cannot remove the only environment. At least one environment must exist.");
    }

    let was_default = config.environments[name].default;

    // Confirm removal
    if !crate::commands::confirm::prompt_yes_no(&format!(
        "Remove environment '{}'? This will delete its state files.",
        name
    ))? {
        println!("Cancelled.");
        return Ok(());
    }

    // Remove environment
    config.environments.remove(name);

    // If removed env was default, set first remaining as default
    if was_default {
        if let Some(first_name) = config.environments.keys().next().cloned() {
            config.environments.get_mut(&first_name).unwrap().default = true;
            println!("Default environment changed to '{}'", first_name);
        }
    }

    // Delete state directory
    let state_dir = project_root.join(".hoist").join(name);
    if state_dir.exists() {
        std::fs::remove_dir_all(&state_dir)?;
    }

    config.save(&project_root)?;

    println!("Environment '{}' removed.", name);

    Ok(())
}
