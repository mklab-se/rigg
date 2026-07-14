//! Environment management commands.

use anyhow::{Result, anyhow, bail};
use colored::Colorize;
use serde_yaml::Value as Yaml;

use rigg_core::workspace::{Environment, WORKSPACE_FILE};

use crate::cli::EnvCommands;
use crate::commands::{CommandError, GlobalContext, discovery, interactive, load_workspace};

pub async fn run(ctx: &GlobalContext, cmd: EnvCommands) -> Result<()> {
    match cmd {
        EnvCommands::List => list(ctx),
        EnvCommands::Show { name } => show(ctx, name.as_deref()),
        EnvCommands::SetDefault { name } => set_default(&name),
        EnvCommands::Add {
            name,
            search_service,
            foundry_account,
            foundry_project,
        } => add(ctx, &name, search_service, foundry_account, foundry_project).await,
        EnvCommands::Remove { name } => remove(&name),
    }
}

fn list(ctx: &GlobalContext) -> Result<()> {
    let ws = load_workspace()?;
    if ctx.json() {
        let entries: Vec<serde_json::Value> = ws
            .config
            .environments
            .iter()
            .map(|(name, env)| {
                serde_json::json!({
                    "name": name,
                    "default": env.default,
                    "protected": env.policy.protected,
                    "search": env.search.as_slice().iter().map(|s| &s.service).collect::<Vec<_>>(),
                    "foundry": env.foundry.as_slice().iter().map(|f| format!("{}/{}", f.account, f.project)).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    for (name, env) in &ws.config.environments {
        let marker = if env.default { " (default)" } else { "" };
        println!("{}{}", name.bold(), marker.dimmed());
        print_env(env, "  ");
    }
    Ok(())
}

fn show(ctx: &GlobalContext, name: Option<&str>) -> Result<()> {
    let ws = load_workspace()?;
    let resolved = ws.resolve_env(name.or(ctx.env.as_deref()))?;
    println!("{}", resolved.name.bold());
    print_env(&resolved.env, "  ");
    Ok(())
}

fn print_env(env: &Environment, indent: &str) {
    println!("{indent}protected: {}", env.policy.protected);
    for s in env.search.as_slice() {
        let label = s.name.as_deref().unwrap_or("search");
        println!(
            "{indent}{label}: {} → {} (Azure AI Search)",
            s.service,
            s.url()
        );
    }
    for f in env.foundry.as_slice() {
        let label = f.name.as_deref().unwrap_or("foundry");
        println!(
            "{indent}{label}: {}/{} → {} (Microsoft Foundry)",
            f.account,
            f.project,
            f.url()
        );
    }
}

/// Edit rigg.yaml preserving comments is not possible with serde; env mutations
/// re-serialize the file. Comments in rigg.yaml are preserved only outside the
/// `environments:` block if the user runs these commands; editing the file
/// directly is always supported.
fn edit_workspace_yaml(edit: impl FnOnce(&mut Yaml) -> Result<()>) -> Result<()> {
    let ws = load_workspace()?;
    let path = ws.root.join(WORKSPACE_FILE);
    let text = std::fs::read_to_string(&path)?;
    let mut doc: Yaml = serde_yaml::from_str(&text)?;
    edit(&mut doc)?;
    std::fs::write(&path, serde_yaml::to_string(&doc)?)?;
    Ok(())
}

fn envs_mut(doc: &mut Yaml) -> Result<&mut serde_yaml::Mapping> {
    let map = doc
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("invalid rigg.yaml"))?;
    let envs = map
        .entry("environments".into())
        .or_insert_with(|| Yaml::Mapping(Default::default()));
    envs.as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("`environments` must be a mapping"))
}

fn set_default(name: &str) -> Result<()> {
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        if !envs.contains_key(name) {
            bail!("unknown environment '{name}'");
        }
        let keys: Vec<Yaml> = envs.keys().cloned().collect();
        for key in keys {
            let is_target = key.as_str() == Some(name);
            if let Some(env) = envs.get_mut(&key).and_then(|e| e.as_mapping_mut()) {
                if is_target {
                    env.insert("default".into(), Yaml::Bool(true));
                } else {
                    env.remove("default");
                }
            }
        }
        Ok(())
    })?;
    println!("Default environment set to '{name}'.");
    Ok(())
}

async fn add(
    ctx: &GlobalContext,
    name: &str,
    search_service: Option<String>,
    foundry_account: Option<String>,
    foundry_project: Option<String>,
) -> Result<()> {
    if foundry_account.is_some() != foundry_project.is_some() {
        bail!("--foundry-account and --foundry-project must be given together");
    }

    // Explicit flags skip the wizard entirely (non-interactive-friendly,
    // scriptable). With neither flag: a TTY runs the interactive wizard
    // (ARM discovery, same as `rigg init`); anything else is a usage error
    // that points at the wizard.
    let has_flags = search_service.is_some() || foundry_account.is_some();
    let (search, foundry, protected) = if has_flags {
        (search_service, foundry_account.zip(foundry_project), false)
    } else if !ctx.interactive() {
        return Err(anyhow!(CommandError::Usage(
            "in non-interactive mode pass --search-service and/or \
             --foundry-account/--foundry-project (or run `rigg env add <name>` on a terminal \
             for the interactive wizard)"
                .to_string()
        )));
    } else {
        let plain = ctx.no_color;
        let (search, foundry) = discovery::discover_interactive(plain).await?;
        let protected = interactive::confirm_default_no(
            "Protect this environment (require typed confirmation for cloud changes)?",
            plain,
        )?;
        (search, foundry, protected)
    };

    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        if envs.contains_key(name) {
            bail!("environment '{name}' already exists");
        }
        let mut env = serde_yaml::Mapping::new();
        if envs.is_empty() {
            env.insert("default".into(), Yaml::Bool(true));
        }
        if let Some(service) = &search {
            let mut s = serde_yaml::Mapping::new();
            s.insert("service".into(), Yaml::String(service.clone()));
            env.insert("search".into(), Yaml::Mapping(s));
        }
        if let Some((account, project)) = &foundry {
            let mut f = serde_yaml::Mapping::new();
            f.insert("account".into(), Yaml::String(account.clone()));
            f.insert("project".into(), Yaml::String(project.clone()));
            env.insert("foundry".into(), Yaml::Mapping(f));
        }
        if protected {
            let mut p = serde_yaml::Mapping::new();
            p.insert("protected".into(), Yaml::Bool(true));
            env.insert("policy".into(), Yaml::Mapping(p));
        }
        envs.insert(name.into(), Yaml::Mapping(env));
        Ok(())
    })?;

    println!("Environment '{name}' added.");
    if let Some(s) = &search {
        println!("  search:   {s}");
    }
    if let Some((a, p)) = &foundry {
        println!("  foundry:  {a}/{p}");
    }
    if protected {
        println!("  protected: true");
    }
    println!("Set as default with: rigg env set-default {name}");
    Ok(())
}

fn remove(name: &str) -> Result<()> {
    edit_workspace_yaml(|doc| {
        let envs = envs_mut(doc)?;
        if envs.remove(name).is_none() {
            bail!("unknown environment '{name}'");
        }
        Ok(())
    })?;
    println!("Environment '{name}' removed.");
    Ok(())
}
