//! `rigg adopt` — bring selected unmanaged remote resources into a project.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use rigg_core::resources::{ResourceKind, ResourceRef, validate_resource_name};
use rigg_core::store::{ProjectState, Store, assert_exclusive_ownership};

use crate::cli::AdoptArgs;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{CommandError, GlobalContext, confirm, load_workspace, resolve_env};

/// What the user asked to adopt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// Every unmanaged resource across both services.
    All,
    /// Every unmanaged resource of one kind.
    Kind(ResourceKind),
    /// One specific resource.
    One(ResourceRef),
}

impl Selector {
    pub fn parse(s: &str) -> Result<Selector> {
        if s == "all" {
            return Ok(Selector::All);
        }
        if let Some((dir, name)) = s.split_once('/') {
            let kind = ResourceKind::from_directory_name(dir)
                .ok_or_else(|| anyhow!(unknown_kind_msg(dir)))?;
            validate_resource_name(name)
                .map_err(|e| anyhow!("invalid resource name '{name}': {e}"))?;
            return Ok(Selector::One(ResourceRef::new(kind, name.to_string())));
        }
        let kind =
            ResourceKind::from_directory_name(s).ok_or_else(|| anyhow!(unknown_kind_msg(s)))?;
        Ok(Selector::Kind(kind))
    }

    /// Broad selectors (all / whole-kind) require confirmation before writing.
    pub fn is_broad(&self) -> bool {
        matches!(self, Selector::All | Selector::Kind(_))
    }
}

pub async fn run(ctx: &GlobalContext, args: AdoptArgs) -> Result<()> {
    if args.selectors.is_empty() {
        return Err(anyhow!(CommandError::Usage(
            "name at least one selector: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`)"
                .to_string()
        )));
    }
    let selectors = args
        .selectors
        .iter()
        .map(|s| Selector::parse(s).map_err(|e| anyhow!(CommandError::Usage(e.to_string()))))
        .collect::<Result<Vec<_>>>()?;

    let ws = load_workspace()?;
    assert_exclusive_ownership(&ws)?;
    let env = resolve_env(&ws, ctx)?;
    let project = ws.project(&args.project)?;

    // Every resource key already owned by ANY project → its owner's name.
    let mut owned_by_any: BTreeMap<String, String> = BTreeMap::new();
    for p in &ws.projects {
        for (r, _) in Store::new(p).list()? {
            owned_by_any.insert(r.key(), p.name.clone());
        }
        let st = ProjectState::load(&ws, &env.name, &p.name);
        for k in st.baselines.keys() {
            owned_by_any
                .entry(k.clone())
                .or_insert_with(|| p.name.clone());
        }
    }

    let remote = Remote::for_project(&env, project);
    ensure_any_connection(&remote, project)?;
    let snapshot = remote.snapshot().await?;
    let snap_map: BTreeMap<String, (ResourceRef, Value)> = snapshot
        .iter()
        .map(|(r, v)| (r.key(), (r.clone(), v.clone())))
        .collect();
    let supported = remote.supported_kinds();

    // Resolve selectors → ordered, unique candidate keys; track explicitly-named ones.
    let mut selected: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut explicit: BTreeSet<String> = BTreeSet::new();
    let push = |key: String, selected: &mut Vec<String>, seen: &mut BTreeSet<String>| {
        if seen.insert(key.clone()) {
            selected.push(key);
        }
    };
    for sel in &selectors {
        match sel {
            Selector::All => {
                for (r, _) in &snapshot {
                    push(r.key(), &mut selected, &mut seen);
                }
            }
            Selector::Kind(k) => {
                if !supported.contains(k) {
                    return Err(anyhow!(CommandError::Usage(format!(
                        "no connection for kind '{}' in environment '{}'",
                        k.directory_name(),
                        env.name
                    ))));
                }
                for (r, _) in &snapshot {
                    if r.kind == *k {
                        push(r.key(), &mut selected, &mut seen);
                    }
                }
            }
            Selector::One(rf) => {
                if !supported.contains(&rf.kind) {
                    return Err(anyhow!(CommandError::Usage(format!(
                        "no connection for kind '{}' in environment '{}'",
                        rf.kind.directory_name(),
                        env.name
                    ))));
                }
                explicit.insert(rf.key());
                push(rf.key(), &mut selected, &mut seen);
            }
        }
    }

    // Classify each candidate.
    let mut to_adopt: Vec<(ResourceRef, Value)> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for key in &selected {
        match snap_map.get(key) {
            None => {
                // Only reachable for an explicit One selector (kind sweeps come from snapshot).
                skipped.push((
                    key.clone(),
                    "no matching unmanaged remote resource".to_string(),
                ));
            }
            Some((r, doc)) => match owned_by_any.get(key) {
                Some(owner) if owner == &project.name => {
                    if explicit.contains(key) {
                        skipped.push((key.clone(), "already managed by this project".to_string()));
                    }
                }
                Some(owner) => {
                    if explicit.contains(key) {
                        return Err(anyhow!(
                            "{r} is owned by project '{owner}' — a resource belongs to exactly one project"
                        ));
                    }
                    // swept in by a kind/all selector → silently skip another project's resource
                }
                None => to_adopt.push((r.clone(), doc.clone())),
            },
        }
    }

    // Confirmation for broad selections.
    let broad = selectors.iter().any(Selector::is_broad);
    if !to_adopt.is_empty() && broad && !ctx.yes && !args.dry_run {
        if ctx.interactive() && !ctx.json() {
            println!(
                "Would adopt {} resource(s) into '{}':",
                to_adopt.len(),
                project.name
            );
            for (r, _) in &to_adopt {
                println!("  {r}");
            }
            if !confirm::prompt_yes_no("Adopt these?")? {
                println!("Aborted.");
                return Ok(());
            }
        } else {
            return Err(anyhow!(CommandError::Usage(
                "broad selector in non-interactive mode: pass --yes to adopt, or --dry-run to preview"
                    .to_string()
            )));
        }
    }

    if args.dry_run {
        report(ctx, &to_adopt, &skipped, true)?;
        return Ok(());
    }

    let store = Store::new(project);
    let mut state = ProjectState::load(&ws, &env.name, &project.name);
    for (r, doc) in &to_adopt {
        store.write(r, doc)?;
        state.set_baseline(r, doc);
    }
    state.save(&ws, &env.name, &project.name)?;
    report(ctx, &to_adopt, &skipped, false)?;
    Ok(())
}

fn report(
    ctx: &GlobalContext,
    to_adopt: &[(ResourceRef, Value)],
    skipped: &[(String, String)],
    dry_run: bool,
) -> Result<()> {
    if ctx.json() {
        let key = if dry_run { "would_adopt" } else { "adopted" };
        let value = json!({
            key: to_adopt.iter().map(|(r, _)| r.key()).collect::<Vec<_>>(),
            "skipped": skipped
                .iter()
                .map(|(k, why)| json!({ "resource": k, "reason": why }))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    if to_adopt.is_empty() {
        println!("Nothing to adopt (no unmanaged resources matched).");
    }
    for (r, _) in to_adopt {
        if dry_run {
            println!("  would adopt {r}");
        } else {
            println!("  + adopted {r}");
        }
    }
    for (k, why) in skipped {
        println!("  - skipped {k} ({why})");
    }
    Ok(())
}

fn unknown_kind_msg(dir: &str) -> String {
    let kinds = ResourceKind::all()
        .iter()
        .map(|k| k.directory_name())
        .collect::<Vec<_>>()
        .join(", ");
    format!("unknown resource kind '{dir}'. Valid kinds: {kinds}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all() {
        assert_eq!(Selector::parse("all").unwrap(), Selector::All);
    }

    #[test]
    fn parses_bare_kind() {
        assert_eq!(
            Selector::parse("indexes").unwrap(),
            Selector::Kind(ResourceKind::Index)
        );
    }

    #[test]
    fn parses_kind_slash_name() {
        assert_eq!(
            Selector::parse("indexes/hotels").unwrap(),
            Selector::One(ResourceRef::new(ResourceKind::Index, "hotels".to_string()))
        );
    }

    #[test]
    fn unknown_kind_is_error_listing_kinds() {
        let err = Selector::parse("widgets").unwrap_err().to_string();
        assert!(err.contains("unknown resource kind 'widgets'"), "{err}");
        assert!(err.contains("indexes"), "lists valid kinds: {err}");
    }

    #[test]
    fn is_broad_classifies_correctly() {
        assert!(Selector::parse("all").unwrap().is_broad());
        assert!(Selector::parse("indexes").unwrap().is_broad());
        assert!(!Selector::parse("indexes/hotels").unwrap().is_broad());
    }
}
