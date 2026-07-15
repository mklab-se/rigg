//! `rigg delete <project> --remote` — remove a project's resources from Azure.
//!
//! Local files are never touched by this command (delete files + `rigg push
//! --prune` for single resources; `rm -r` for local project removal).

use anyhow::{Result, anyhow};
use colored::Colorize;

use rigg_core::graph;
use rigg_core::store::{ProjectState, Store};

use crate::cli::DeleteArgs;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{
    CommandError, GlobalContext, confirm_protected_env, interactive, load_workspace, resolve_env,
};

pub async fn run(ctx: &GlobalContext, args: DeleteArgs) -> Result<()> {
    if !args.remote {
        return Err(anyhow!(CommandError::Usage(
            "rigg delete removes remote resources and requires --remote; \
             to remove local files just delete them (then `rigg push --prune`)"
                .to_string()
        )));
    }
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let project = ws.project(&args.project)?;
    let store = Store::new(project, &env.name);
    let remote = Remote::for_project(&env, project);
    ensure_any_connection(&remote, project)?;
    let mut state = ProjectState::load(&ws, &env.name, &project.name);

    // Everything the project owns that exists remotely.
    let mut items = Vec::new();
    for (r, _) in store.list()? {
        if remote.supported_kinds().contains(&r.kind) && remote.get(&r).await?.is_some() {
            let body = store.read(&r)?;
            items.push((r, body));
        }
    }
    if items.is_empty() {
        println!(
            "Nothing to delete: no remote resources found for project '{}' in environment '{}'.",
            project.name, env.name
        );
        return Ok(());
    }

    println!(
        "{} the following resources of project '{}' from {} (env: {}{}):",
        "DELETING".red().bold(),
        project.name.bold(),
        "Azure".bold(),
        env.name,
        if env.protected() {
            format!(", {}", "protected".yellow())
        } else {
            String::new()
        }
    );
    remote.print_targets();
    let order = graph::delete_order(&items)?;
    for r in &order {
        println!("  {} {}", "delete".red(), r);
    }

    // Protected-env gate: separate from, and comes before, the typed
    // project-name confirmation below (which guards against deleting the
    // wrong project, not against mutating a protected environment).
    if !confirm_protected_env(ctx, &env, args.confirm_env.as_deref(), "delete")? {
        println!("Aborted.");
        return Ok(());
    }

    if ctx.interactive() {
        println!();
        let answer = interactive::text("Type the project name to confirm:", ctx.no_color)?;
        if answer.trim() != project.name {
            println!("aborted");
            return Ok(());
        }
    } else if !ctx.yes {
        return Err(anyhow!(CommandError::Usage(
            "non-interactive delete requires --yes".to_string()
        )));
    }

    for r in &order {
        remote.delete(r).await?;
        state.clear_baseline(r);
        state.save(&ws, &env.name, &project.name)?;
        println!("  {} deleted {}", "✓".green(), r);
    }
    println!(
        "{} local files kept; push the project to re-create everything",
        "i".blue()
    );
    Ok(())
}
