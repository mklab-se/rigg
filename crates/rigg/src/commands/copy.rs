//! `rigg copy` — copy a resource file locally under a new name,
//! within or across projects. No network access.

use anyhow::{Context, Result, bail};

use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::Store;
use rigg_core::workspace::Workspace;

use crate::cli::CopyArgs;
use crate::commands::{GlobalContext, load_workspace};

pub fn run(_ctx: &GlobalContext, args: CopyArgs) -> Result<()> {
    let ws = load_workspace()?;

    let (src_project, src_kind, src_name) = parse_source(&ws, &args.source)?;
    let (dst_project_name, dst_name) = match args.target.split_once(':') {
        Some((p, n)) => (Some(p.to_string()), n.to_string()),
        None => (None, args.target.clone()),
    };
    let dst_project = match &dst_project_name {
        Some(p) => ws.project(p)?,
        None => ws.project(&src_project)?,
    };

    let src_ref = ResourceRef::new(src_kind, src_name.clone());
    let dst_ref = ResourceRef::new(src_kind, dst_name.clone());

    let src_store = Store::new(ws.project(&src_project)?);
    let dst_store = Store::new(dst_project);

    if dst_store.path_for(&dst_ref).exists() {
        bail!(
            "target {} already exists in project '{}'",
            dst_ref,
            dst_project.name
        );
    }

    let mut value = src_store
        .read(&src_ref)
        .with_context(|| format!("source {} not found in project '{}'", src_ref, src_project))?;

    // Rename the resource itself.
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(dst_name.clone()),
        );
    }

    dst_store.write(&dst_ref, &value)?;
    println!(
        "Copied {}:{} -> {}:{}",
        src_project, src_ref, dst_project.name, dst_ref
    );
    println!("note: references inside the copy still point at the original's dependencies;");
    println!("      edit the new file if the copy should use different ones.");
    Ok(())
}

fn parse_source(ws: &Workspace, source: &str) -> Result<(String, ResourceKind, String)> {
    let (project_hint, rest) = match source.split_once(':') {
        Some((p, r)) => (Some(p.to_string()), r.to_string()),
        None => (None, source.to_string()),
    };
    let Some((dir, name)) = rest.split_once('/') else {
        bail!("source must be <kind-dir>/<name> (e.g. indexes/my-index)");
    };
    let kind = ResourceKind::from_directory_name(dir)
        .ok_or_else(|| anyhow::anyhow!("unknown resource kind directory '{dir}'"))?;

    if let Some(p) = project_hint {
        return Ok((p, kind, name.to_string()));
    }
    // Find the owning project.
    let reference = ResourceRef::new(kind, name);
    for project in &ws.projects {
        if Store::new(project).path_for(&reference).exists() {
            return Ok((project.name.clone(), kind, reference.name));
        }
    }
    bail!("{reference} not found in any project");
}
