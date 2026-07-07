//! `rigg init` — create a new workspace.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use colored::Colorize;

use rigg_client::arm::ArmClient;
use rigg_core::workspace::{APIS_DIR, PROJECTS_DIR, STATE_DIR, WORKSPACE_FILE};

use crate::cli::InitArgs;
use crate::commands::{CommandError, GlobalContext};

pub async fn run(ctx: &GlobalContext, args: InitArgs) -> Result<()> {
    let root = Path::new(&args.path);
    std::fs::create_dir_all(root)?;
    let ws_file = root.join(WORKSPACE_FILE);
    if ws_file.exists() {
        bail!(
            "{} already exists — this is already a rigg workspace",
            ws_file.display()
        );
    }

    // Determine services: explicit flags > ARM discovery > manual entry.
    let (search, foundry) = if args.search_service.is_some() || args.foundry_account.is_some() {
        if args.foundry_account.is_some() != args.foundry_project.is_some() {
            return Err(anyhow!(CommandError::Usage(
                "--foundry-account and --foundry-project must be given together".to_string()
            )));
        }
        (
            args.search_service.clone(),
            args.foundry_account
                .clone()
                .zip(args.foundry_project.clone()),
        )
    } else if args.no_discovery || !ctx.interactive() {
        return Err(anyhow!(CommandError::Usage(
            "in non-interactive mode pass --search-service and/or --foundry-account/--foundry-project".to_string()
        )));
    } else {
        discover_interactive().await?
    };

    if search.is_none() && foundry.is_none() {
        return Err(anyhow!(CommandError::Usage(
            "nothing to manage: no search service or foundry project selected".to_string()
        )));
    }

    // Identity guidance (spec §8.2): informational in 0.18, doctor lands in 0.19.
    println!();
    println!("{}", "Identity guidance".bold());
    println!(
        "  For stacks spanning services (search + storage + foundry), a USER-ASSIGNED managed\n\
         \x20 identity is recommended: one identity for the whole pipeline, role assignments\n\
         \x20 survive service re-creation, and it works across environments. Use system-assigned\n\
         \x20 for simple single-service setups. `rigg auth doctor` (0.19) will verify the wiring."
    );

    // Write rigg.yaml
    let mut yaml = String::new();
    yaml.push_str("# Rigg workspace configuration.\n");
    yaml.push_str("# Resource definitions live in projects/<name>/ — see `rigg new project`.\n");
    yaml.push_str("environments:\n");
    yaml.push_str(&format!("  {}:\n", args.env_name));
    yaml.push_str("    default: true\n");
    if let Some(service) = &search {
        yaml.push_str(&format!("    search: {{ service: {service} }}\n"));
    }
    if let Some((account, project)) = &foundry {
        yaml.push_str(&format!(
            "    foundry: {{ account: {account}, project: {project} }}\n"
        ));
    }
    std::fs::write(&ws_file, yaml)?;

    // Directory skeleton + .gitignore
    std::fs::create_dir_all(root.join(PROJECTS_DIR))?;
    std::fs::create_dir_all(root.join(APIS_DIR))?;
    let gitignore = root.join(".gitignore");
    let mut gi = if gitignore.exists() {
        std::fs::read_to_string(&gitignore)?
    } else {
        String::new()
    };
    if !gi.lines().any(|l| l.trim() == format!("{STATE_DIR}/")) {
        if !gi.is_empty() && !gi.ends_with('\n') {
            gi.push('\n');
        }
        gi.push_str(&format!("{STATE_DIR}/\n"));
        std::fs::write(&gitignore, gi)?;
    }

    println!();
    println!("{} rigg workspace initialized", "✓".green().bold());
    println!("  config:   {}", ws_file.display());
    if let Some(s) = &search {
        println!("  search:   {s}");
    }
    if let Some((a, p)) = &foundry {
        println!("  foundry:  {a}/{p}");
    }
    println!();
    println!("Next steps:");
    println!("  rigg new project <name>           # create your first project");
    println!("  rigg new pipeline <name> -p <p>   # scaffold an explicit RAG pipeline");
    println!("  rigg pull <project> --adopt <p>   # or adopt existing Azure resources");
    Ok(())
}

type Discovered = (Option<String>, Option<(String, String)>);

/// Discover services via ARM and let the user pick interactively.
async fn discover_interactive() -> Result<Discovered> {
    println!("Discovering Azure services (via Azure CLI credentials)...");
    let arm = match ArmClient::new() {
        Ok(arm) => arm,
        Err(e) => {
            println!("  ARM discovery unavailable ({e}); falling back to manual entry.");
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
