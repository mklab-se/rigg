//! `rigg describe` — workspace overview: projects, resources, dependency
//! graph, and the APIs a user (or agent) must implement.

use anyhow::Result;
use colored::Colorize;
use serde_json::{Value, json};

use rigg_core::registry::{self, X_RIGG_API};
use rigg_core::resources::ResourceRef;
use rigg_core::store::Store;
use rigg_core::workspace::Workspace;

use crate::cli::DescribeArgs;
use crate::commands::{GlobalContext, load_workspace};

pub fn run(ctx: &GlobalContext, args: DescribeArgs) -> Result<()> {
    let ws = load_workspace()?;
    let projects: Vec<_> = match args.project.as_deref() {
        Some(name) => vec![ws.project(name)?],
        None => ws.projects.iter().collect(),
    };

    if ws.projects.is_empty() && args.project.is_none() && !ctx.json() {
        crate::commands::print_no_projects_hint();
        return Ok(());
    }

    let mut out_projects = Vec::new();
    for project in &projects {
        let store = Store::new(project);
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
            }))
            .collect::<Vec<_>>());
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }

    for (project, resources, edges, apis) in &out_projects {
        println!("{}", project.name.bold());
        if let Some(desc) = &project.manifest.description {
            if !desc.is_empty() {
                println!("  {}", desc.dimmed());
            }
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
        println!();
    }
    Ok(())
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

#[allow(unused)]
fn _t(_: &Workspace) {}
