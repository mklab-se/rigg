//! `rigg describe` — workspace overview: projects, resources, dependency
//! graph, and the APIs a user (or agent) must implement.

use anyhow::Result;
use colored::Colorize;
use serde_json::{Value, json};

use rigg_core::binding::{Binding, EnvBindings, Wanted};
use rigg_core::registry::{self, X_RIGG_API};
use rigg_core::resources::ResourceRef;
use rigg_core::store::Store;
use rigg_core::workspace::Workspace;

use crate::cli::DescribeArgs;
use crate::commands::{GlobalContext, load_workspace, resolve_env};

pub fn run(ctx: &GlobalContext, args: DescribeArgs) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let projects: Vec<_> = match args.project.as_deref() {
        Some(name) => vec![ws.project(name)?],
        None => ws.projects.iter().collect(),
    };

    if ws.projects.is_empty() && args.project.is_none() && !ctx.json() {
        crate::commands::print_no_projects_hint();
        return Ok(());
    }

    let infrastructure = infrastructure_rows(&ws, &env.name);

    let mut out_projects = Vec::new();
    for project in &projects {
        let store = Store::new(project, &env.name);
        let mut resources = Vec::new();
        let mut edges = Vec::new();
        let mut apis: Vec<(String, String)> = Vec::new(); // (api, consumer)

        for (r, path) in store.list()? {
            let value = store.read(&r)?;
            for (kind, name) in registry::extract_references(r.kind, &value) {
                edges.push((r.key(), ResourceRef::new(kind, name).key()));
            }
            collect_api_links(&value, &r, &mut apis);
            resources.push((r, path, value));
        }
        out_projects.push((project, resources, edges, apis));
    }

    if ctx.json() {
        let value = json!(out_projects
            .iter()
            .map(|(project, resources, edges, apis)| json!({
                "project": project.name,
                "env": env.name,
                "description": project.manifest.description,
                "resources": resources.iter().map(|(r, path, value)| json!({
                    "resource": r.key(),
                    "kind": r.kind.cli_name(),
                    "name": r.name,
                    "file_path": path.display().to_string(),
                    "definition": value,
                })).collect::<Vec<_>>(),
                "dependencies": edges.iter().map(|(from, to)| json!({
                    "from": from, "to": to
                })).collect::<Vec<_>>(),
                "apis_to_implement": apis.iter().map(|(api, consumer)| json!({
                    "api": api,
                    "spec_path": ws.apis_dir().join(format!("{api}.json")).display().to_string(),
                    "consumed_by": consumer,
                })).collect::<Vec<_>>(),
                // Environment-level, repeated per project entry so a caller
                // reading one entry sees the infrastructure its resources
                // resolve against.
                "infrastructure": infrastructure.iter().map(|(name, binding, shared)| json!({
                    "name": name,
                    "type": binding.kind.to_string(),
                    "value": binding.value,
                    "physical_name": binding.physical_name(),
                    "shared_with": shared,
                })).collect::<Vec<_>>(),
            }))
            .collect::<Vec<_>>());
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    for (project, resources, edges, apis) in &out_projects {
        println!("{} (env: {})", project.name.bold(), env.name);
        if let Some(desc) = &project.manifest.description
            && !desc.is_empty()
        {
            println!("  {}", desc.dimmed());
        }
        if resources.is_empty() {
            println!("  (no resources)");
        }
        for (r, _, _) in resources {
            let deps: Vec<&str> = edges
                .iter()
                .filter(|(from, _)| *from == r.key())
                .map(|(_, to)| to.as_str())
                .collect();
            if deps.is_empty() {
                println!("  {}", r);
            } else {
                println!("  {} {} {}", r, "->".dimmed(), deps.join(", ").dimmed());
            }
        }
        if !apis.is_empty() {
            println!();
            println!("  {}", "APIs to implement (specs in apis/):".bold());
            for (api, consumer) in apis {
                println!("    {} (used by {})", api.cyan(), consumer);
            }
        }
        if !infrastructure.is_empty() {
            println!();
            println!("  {}", "Infrastructure:".bold());
            for (name, binding, shared) in &infrastructure {
                let shared = if shared.is_empty() {
                    String::new()
                } else {
                    format!(" (shared with: {})", shared.join(", "))
                };
                println!(
                    "    {name}  {}  {}{}",
                    binding.kind,
                    binding.value,
                    shared.dimmed()
                );
            }
        }
        println!();
    }
    Ok(())
}

/// `env`'s declared bindings, each with the other environments that bind the
/// same physical resource under the same type.
fn infrastructure_rows(ws: &Workspace, env_name: &str) -> Vec<(String, Binding, Vec<String>)> {
    let Some(env) = ws.config.environments.get(env_name) else {
        return Vec::new();
    };
    let others: Vec<EnvBindings> = ws
        .config
        .environments
        .iter()
        .filter(|(name, _)| name.as_str() != env_name)
        .map(|(name, e)| EnvBindings::of_env(name, e, None))
        .collect();
    env.dependencies
        .iter()
        .map(|(name, binding)| {
            let shared = others
                .iter()
                .filter(|o| {
                    o.find_physical(Wanted::Type(binding.kind), &binding.physical_name())
                        .is_some()
                })
                .map(|o| o.env.clone())
                .collect();
            (name.clone(), binding.clone(), shared)
        })
        .collect()
}

fn collect_api_links(value: &Value, r: &ResourceRef, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let Some(api) = map.get(X_RIGG_API).and_then(Value::as_str) {
                out.push((api.to_string(), r.key()));
            }
            for (_, v) in map {
                collect_api_links(v, r, out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_api_links(item, r, out);
            }
        }
        _ => {}
    }
}
