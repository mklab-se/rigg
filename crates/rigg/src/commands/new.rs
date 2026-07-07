//! `rigg new` — scaffold projects, resources, pipelines, and API specs.

use anyhow::{Result, anyhow, bail};
use colored::Colorize;

use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::scaffold;
use rigg_core::store::Store;
use rigg_core::workspace::{PROJECT_FILE, PROJECTS_DIR, Workspace};

use crate::cli::NewArgs;
use crate::commands::{CommandError, GlobalContext, load_workspace};

pub async fn run(ctx: &GlobalContext, args: NewArgs) -> Result<()> {
    match args.kind.as_str() {
        "project" => new_project(&args.name),
        "api" => new_api(&args.name),
        "pipeline" => new_pipeline(ctx, &args),
        kind_str => {
            let kind = ResourceKind::from_cli_name(kind_str).ok_or_else(|| {
                anyhow!(CommandError::Usage(format!(
                    "unknown kind '{kind_str}'. Valid: project, pipeline, api, {}",
                    ResourceKind::all()
                        .iter()
                        .map(|k| k.cli_name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )))
            })?;
            new_resource(ctx, kind, &args).await
        }
    }
}

fn new_project(name: &str) -> Result<()> {
    let ws = load_workspace()?;
    let dir = ws.root.join(PROJECTS_DIR).join(name);
    if dir.exists() {
        bail!("project '{name}' already exists");
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join(PROJECT_FILE),
        format!(
            "# Rigg project: {name}\n# The files in this directory ARE the project membership.\ndescription: \"\"\n"
        ),
    )?;
    println!("Created project '{}' at {}", name.bold(), dir.display());
    println!("Add resources with: rigg new <kind> <name> -p {name}");
    Ok(())
}

fn new_api(name: &str) -> Result<()> {
    let ws = load_workspace()?;
    let dir = ws.apis_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.json"));
    if path.exists() {
        bail!("API spec '{name}' already exists at {}", path.display());
    }
    let spec = scaffold::scaffold_api_spec(name);
    std::fs::write(&path, rigg_core::normalize::format_json(&spec))?;
    println!("Created OpenAPI spec {}", path.display());
    println!("Link it from a skillset WebApiSkill with: \"x-rigg-api\": \"{name}\"");
    Ok(())
}

fn resolve_project<'w>(
    ws: &'w Workspace,
    args: &NewArgs,
) -> Result<&'w rigg_core::workspace::Project> {
    match (&args.project, ws.projects.len()) {
        (Some(p), _) => Ok(ws.project(p)?),
        (None, 1) => Ok(&ws.projects[0]),
        (None, 0) => Err(anyhow!(CommandError::Usage(
            "no projects in workspace; run `rigg new project <name>` first".to_string()
        ))),
        (None, _) => Err(anyhow!(CommandError::Usage(
            "multiple projects in workspace; pass -p <project>".to_string()
        ))),
    }
}

fn new_pipeline(_ctx: &GlobalContext, args: &NewArgs) -> Result<()> {
    let ws = load_workspace()?;
    let project = resolve_project(&ws, args)?;
    let store = Store::new(project);
    let ds_type = args.ds_type.as_deref().unwrap_or("azureblob");
    if let Some(warning) = scaffold::check_datasource_type(ds_type)
        .map_err(|e| anyhow!(CommandError::Validation(e)))?
    {
        eprintln!("{} {warning}", "warning:".yellow().bold());
    }
    let parts = scaffold::scaffold_pipeline(&args.name, ds_type, true)
        .map_err(|e| anyhow!(CommandError::Validation(e)))?;
    for (kind, name, value) in &parts {
        let r = ResourceRef::new(*kind, name.clone());
        if store.path_for(&r).exists() {
            bail!("{r} already exists in project '{}'", project.name);
        }
        store.write(&r, value)?;
        println!("  created {}", store.path_for(&r).display());
    }
    println!();
    println!(
        "Pipeline '{}' scaffolded in project '{}':",
        args.name.bold(),
        project.name
    );
    println!("  1. Edit the data source (connection ResourceId, container)");
    println!("  2. Shape the index fields for your data");
    println!("  3. Adjust or remove the skillset, wire the indexer");
    println!("  4. Push step by step: rigg push {}", project.name);
    Ok(())
}

async fn new_resource(ctx: &GlobalContext, kind: ResourceKind, args: &NewArgs) -> Result<()> {
    let ws = load_workspace()?;
    let project = resolve_project(&ws, args)?;
    let store = Store::new(project);
    let r = ResourceRef::new(kind, args.name.clone());
    if store.path_for(&r).exists() {
        bail!("{r} already exists in project '{}'", project.name);
    }
    if kind == ResourceKind::DataSource {
        if let Some(warning) =
            scaffold::check_datasource_type(args.ds_type.as_deref().unwrap_or("azureblob"))
                .map_err(|e| anyhow!(CommandError::Validation(e)))?
        {
            eprintln!("{} {warning}", "warning:".yellow().bold());
        }
    }

    let mut value = scaffold::scaffold(kind, &args.name, args.ds_type.as_deref())
        .map_err(|e| anyhow!(CommandError::Validation(e)))?;

    // NL scaffolding via ailloy (when configured & enabled).
    if let Some(description) = &args.describe {
        match crate::commands::new::ai_draft(ctx, kind, &args.name, description, &value).await {
            Ok(Some(draft)) => value = draft,
            Ok(None) => eprintln!(
                "{} AI is not enabled (run `rigg ai enable`); wrote the standard template instead",
                "note:".dimmed()
            ),
            Err(e) => eprintln!(
                "{} AI drafting failed ({e}); wrote the standard template instead",
                "warning:".yellow().bold()
            ),
        }
    }

    store.write(&r, &value)?;
    println!("Created {}", store.path_for(&r).display());
    Ok(())
}

/// Draft a resource definition from a natural-language description via ailloy.
/// Returns Ok(None) when AI is not active for rigg.
pub async fn ai_draft(
    _ctx: &GlobalContext,
    kind: ResourceKind,
    name: &str,
    description: &str,
    template: &serde_json::Value,
) -> Result<Option<serde_json::Value>> {
    if !ailloy::config_tui::is_ai_active("rigg") {
        return Ok(None);
    }
    let system = format!(
        "You generate Azure {} definitions as raw JSON for the Azure AI Search / Microsoft Foundry REST APIs (api-version {}). \
         NEVER include API keys, connection strings with AccountKey, or any secret — use managed-identity patterns (ResourceId=...) only. \
         Respond with ONLY the JSON document, no prose, no code fences.",
        kind.display_name(),
        rigg_core::registry::SEARCH_STABLE_API_VERSION,
    );
    let user = format!(
        "Resource name: '{name}'.\nWhat I want:\n{description}\n\nStart from this template and produce a complete, valid definition:\n{}",
        serde_json::to_string_pretty(template)?
    );
    let response = rigg_client::ai::generate_text(&system, &user).await?;
    let text = response.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .unwrap_or(text)
        .trim_end_matches("```")
        .trim();
    let mut draft: serde_json::Value =
        serde_json::from_str(text).map_err(|e| anyhow!("AI returned invalid JSON: {e}"))?;
    if let Some(obj) = draft.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(name.to_string()),
        );
    }
    Ok(Some(draft))
}
