//! `rigg init` — create a new workspace.

use std::path::Path;

use anyhow::{Result, anyhow, bail};
use colored::Colorize;

use rigg_core::workspace::{APIS_DIR, PROJECTS_DIR, STATE_DIR, WORKSPACE_FILE};

use crate::cli::InitArgs;
use crate::commands::discovery;
use crate::commands::{CommandError, GlobalContext};

pub async fn run(ctx: &GlobalContext, args: InitArgs) -> Result<()> {
    // The workspace root is always the current directory — that is where
    // rigg.yaml lands and where rigg commands work from. The optional path
    // argument names the folder that stores rigg's file trees
    // (projects/, apis/, .rigg/), recorded as `root:` in rigg.yaml.
    let root = Path::new(".");
    let files_sub = args
        .path
        .trim_end_matches('/')
        .trim_start_matches("./")
        .to_string();
    let files_sub = (!files_sub.is_empty() && files_sub != ".").then_some(files_sub);
    let files_root = match &files_sub {
        Some(sub) => root.join(sub),
        None => root.to_path_buf(),
    };
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
        discovery::discover_interactive(ctx.no_color).await?
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
    if let Some(sub) = &files_sub {
        yaml.push_str(&format!(
            "# Resource definitions live in {sub}/projects/<name>/ — see `rigg new project`.\n"
        ));
        yaml.push_str(&format!("root: {sub}\n"));
    } else {
        yaml.push_str(
            "# Resource definitions live in projects/<name>/ — see `rigg new project`.\n",
        );
    }
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
    std::fs::create_dir_all(files_root.join(PROJECTS_DIR))?;
    std::fs::create_dir_all(files_root.join(APIS_DIR))?;
    let state_ignore = match &files_sub {
        Some(sub) => format!("{sub}/{STATE_DIR}/"),
        None => format!("{STATE_DIR}/"),
    };
    let gitignore = root.join(".gitignore");
    let mut gi = if gitignore.exists() {
        std::fs::read_to_string(&gitignore)?
    } else {
        String::new()
    };
    if !gi.lines().any(|l| l.trim() == state_ignore) {
        if !gi.is_empty() && !gi.ends_with('\n') {
            gi.push('\n');
        }
        gi.push_str(&state_ignore);
        gi.push('\n');
        std::fs::write(&gitignore, gi)?;
    }

    println!();
    println!("{} rigg workspace initialized", "✓".green().bold());
    println!("  config:   {}", ws_file.display());
    if let Some(sub) = &files_sub {
        println!("  files:    {sub}/ (projects, apis, state)");
    }
    if let Some(s) = &search {
        println!("  search:   {s}");
    }
    if let Some((a, p)) = &foundry {
        println!("  foundry:  {a}/{p}");
    }
    println!(
        "  environment: {} (default) — rigg commands target it unless -e/RIGG_ENV say otherwise; \
         add more with `rigg env add`",
        args.env_name
    );
    println!();
    println!("Next steps:");
    println!("  rigg new project <name>           # create your first project");
    println!("  rigg new pipeline <name> -p <p>   # scaffold an explicit RAG pipeline");
    println!("  rigg adopt <project> <selector>   # or adopt existing Azure resources");
    println!();
    println!("Tip: enable tab completion (names included) with one line in your shell rc:");
    println!("  zsh:  source <(COMPLETE=zsh rigg)     bash: source <(COMPLETE=bash rigg)");
    Ok(())
}
