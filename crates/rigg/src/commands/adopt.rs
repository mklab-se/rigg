//! `rigg adopt` — bring selected unmanaged remote resources into a project.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};

use rigg_core::registry::{self, Domain};
use rigg_core::resources::{ResourceKind, ResourceRef, validate_resource_name};
use rigg_core::store::{ProjectState, Store, assert_exclusive_ownership};

use crate::cli::AdoptArgs;
use crate::commands::remote::{Remote, ensure_any_connection};
use crate::commands::{
    CommandError, GlobalContext, confirm, interactive, load_workspace, new, resolve_env,
};

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
    // Parse any given selectors first — cheap, and usage errors must not
    // require a workspace or network.
    let mut selectors = args
        .selectors
        .iter()
        .map(|s| Selector::parse(s).map_err(|e| anyhow!(CommandError::Usage(e.to_string()))))
        .collect::<Result<Vec<_>>>()?;

    let wizard =
        ctx.interactive() && !ctx.json() && (args.project.is_none() || selectors.is_empty());
    if !wizard {
        if args.project.is_none() {
            return Err(anyhow!(CommandError::Usage(
                "name a project (rigg adopt <project> <selector>...), or run on an interactive terminal for the wizard"
                    .to_string()
            )));
        }
        if selectors.is_empty() {
            return Err(anyhow!(CommandError::Usage(
                "name at least one selector: `all`, a kind (`indexes`), or `<kind>/<name>` (`agents/regulus`)"
                    .to_string()
            )));
        }
    }

    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    assert_exclusive_ownership(&ws, &env.name)?;
    let plain = ctx.no_color;

    // ---- Wizard step 1: project ----
    let project_name = match &args.project {
        Some(p) => p.clone(),
        None => match ws.projects.len() {
            0 => {
                println!("No projects yet — a project groups the resources you manage together.");
                if !interactive::confirm_default_yes("Create one now?", plain)? {
                    return Err(anyhow!("aborted"));
                }
                let name =
                    interactive::text("Project name (e.g. the agent or app it will own):", plain)?;
                new::create_project(&ws, &name)?;
                // Reload so ws.project() sees it.
                drop(ws);
                return Box::pin(run(
                    ctx,
                    AdoptArgs {
                        project: Some(name),
                        selectors: args.selectors.clone(),
                        dry_run: args.dry_run,
                        with_deps: args.with_deps,
                    },
                ))
                .await;
            }
            1 => {
                let name = ws.projects[0].name.clone();
                println!("Using project '{name}' (the only project in this workspace).");
                name
            }
            _ => interactive::select(
                "Adopt into which project?",
                ws.projects.iter().map(|p| p.name.clone()).collect(),
                plain,
            )?,
        },
    };
    let project = ws.project(&project_name)?;

    // Every resource key already owned by ANY project → its owner's name.
    let mut owned_by_any: BTreeMap<String, String> = BTreeMap::new();
    for p in &ws.projects {
        for (r, _) in Store::new(p, &env.name).list()? {
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
    let auto_created = registry::auto_created_by(&snapshot);

    // ---- Wizard step 2: resources ----
    let mut wizard_chosen: Vec<String> = Vec::new(); // keys, for the hint
    if selectors.is_empty() {
        let candidates = wizard_candidates(&snapshot, &owned_by_any, &auto_created, &project.name);
        if candidates.is_empty() {
            println!("Nothing to adopt — everything visible is already managed.");
            return Ok(());
        }
        // Service legend
        if remote.has_foundry() {
            println!("Foundry: unmanaged resources from the configured account/project");
        }
        if remote.has_search() {
            println!("Search:  unmanaged resources from the configured service");
        }
        let labels: Vec<String> = candidates.iter().map(|(_, l)| l.clone()).collect();
        let picked = interactive::multi_select(
            "Select resources to adopt (space toggles, type to filter):",
            labels,
            plain,
        )?;
        if picked.is_empty() {
            println!("Nothing selected.");
            return Ok(());
        }
        for i in picked {
            let (r, _) = &candidates[i];
            wizard_chosen.push(r.key());
            selectors.push(Selector::One(r.clone()));
        }
    }

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
    // Explicitly-named resources already owned by THIS project — not adopted
    // (they're already managed), but they seed dependency expansion below so
    // `--with-deps` can still pull in their unmanaged deps.
    let mut owned_seeds: Vec<(ResourceRef, Value)> = Vec::new();
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
                        owned_seeds.push((r.clone(), doc.clone()));
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
                None => {
                    if let Some(ks) = auto_created.get(key) {
                        if explicit.contains(key) {
                            skipped.push((
                                key.clone(),
                                format!(
                                    "auto-created by knowledge source '{ks}' — manage it via the knowledge source"
                                ),
                            ));
                        }
                        // swept in by a kind/all selector → silently skip
                    } else if registry::is_platform_managed(r.kind, doc) {
                        if explicit.contains(key) {
                            skipped.push((
                                key.clone(),
                                "platform-managed (provided by Microsoft) — reference it, don't adopt it"
                                    .to_string(),
                            ));
                        }
                        // swept in by a kind/all selector → silently skip
                    } else {
                        to_adopt.push((r.clone(), doc.clone()));
                    }
                }
            },
        }
    }

    // Optionally pull each candidate's upstream dependency graph.
    let mut dep_keys: BTreeSet<String> = BTreeSet::new();
    let mut with_deps = args.with_deps;
    if with_deps {
        let mut roots: Vec<(ResourceRef, Value)> = to_adopt.clone();
        roots.extend(owned_seeds.iter().cloned());
        let (adds, keys, _owned_refs) =
            expand_deps(&roots, &owned_by_any, &auto_created, &snap_map);
        to_adopt.extend(adds);
        dep_keys = keys;
    } else if wizard {
        let mut roots: Vec<(ResourceRef, Value)> = to_adopt.clone();
        roots.extend(owned_seeds.iter().cloned());
        let (adds, _keys, owned_refs) =
            expand_deps(&roots, &owned_by_any, &auto_created, &snap_map);
        let mine: Vec<&str> = owned_refs
            .iter()
            .filter(|(_, o)| o == &project.name)
            .map(|(k, _)| k.as_str())
            .collect();
        let theirs: Vec<String> = owned_refs
            .iter()
            .filter(|(_, o)| o != &project.name)
            .map(|(k, o)| format!("{k} (managed by '{o}')"))
            .collect();
        let managed_list: Vec<String> = mine
            .iter()
            .map(|k| k.to_string())
            .chain(theirs.iter().cloned())
            .collect();
        if adds.is_empty() {
            if !managed_list.is_empty() {
                println!(
                    "All dependencies of your selection are already managed: {}",
                    managed_list.join(", ")
                );
            }
        } else {
            if !managed_list.is_empty() {
                println!("Already managed: {}", managed_list.join(", "));
            }
            let labels: Vec<String> = adds.iter().map(|(r, _)| r.to_string()).collect();
            let picked = interactive::multi_select_checked(
                "Upstream dependencies found — adopt these too? (all selected; space to drop)",
                labels,
                true,
                plain,
            )?;
            if !picked.is_empty() {
                with_deps = true;
                for i in &picked {
                    let (r, doc) = &adds[*i];
                    dep_keys.insert(r.key());
                    to_adopt.push((r.clone(), doc.clone()));
                }
            }
        }
    }

    // Confirmation.
    let broad = selectors.iter().any(Selector::is_broad);
    if wizard && !to_adopt.is_empty() && !args.dry_run {
        println!(
            "Will adopt {} resource(s) into '{}':",
            to_adopt.len(),
            project.name
        );
        for (r, _) in &to_adopt {
            let tag = if dep_keys.contains(&r.key()) {
                " (dependency)"
            } else {
                ""
            };
            println!("  {r}{tag}");
        }
        if !interactive::confirm_default_yes("Proceed?", plain)? {
            println!("Aborted.");
            return Ok(());
        }
    } else if !to_adopt.is_empty() && broad && !ctx.yes && !args.dry_run {
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
        report(ctx, &to_adopt, &skipped, &dep_keys, true)?;
        return Ok(());
    }

    let store = Store::new(project, &env.name);
    let mut state = ProjectState::load(&ws, &env.name, &project.name);
    for (r, doc) in &to_adopt {
        store.write(r, doc)?;
        state.set_baseline(r, doc);
    }
    state.save(&ws, &env.name, &project.name)?;
    report(ctx, &to_adopt, &skipped, &dep_keys, false)?;

    // ---- teach the scriptable form ----
    if wizard && !wizard_chosen.is_empty() && !args.dry_run {
        println!();
        println!(
            "hint: next time: {}",
            equivalent_command(&project.name, &wizard_chosen, with_deps)
        );
    }
    Ok(())
}

/// Unmanaged candidates for the wizard's multi-select, sorted Foundry-first
/// then Search, by (kind directory, name) within domain. Label is
/// `"[Foundry] agents/x"` / `"[Search] indexes/y"`, with a `" (managed)"`
/// suffix for entries already owned by `target_project` (re-adoption case:
/// picking one seeds its missing dependencies without re-adopting itself).
/// Entries owned by any OTHER project, or platform-managed, are excluded.
fn wizard_candidates(
    snapshot: &[(ResourceRef, Value)],
    owned_by_any: &BTreeMap<String, String>,
    auto_created: &BTreeMap<String, String>,
    target_project: &str,
) -> Vec<(ResourceRef, String)> {
    fn domain_rank(kind: ResourceKind) -> u8 {
        match registry::meta(kind).domain {
            Domain::FoundryData | Domain::FoundryArm => 0,
            Domain::Search => 1,
        }
    }
    fn prefix(kind: ResourceKind) -> &'static str {
        match registry::meta(kind).domain {
            Domain::FoundryData | Domain::FoundryArm => "[Foundry]",
            Domain::Search => "[Search]",
        }
    }

    let mut items: Vec<(ResourceRef, String)> = snapshot
        .iter()
        .filter(|(r, doc)| {
            let owner = owned_by_any.get(&r.key());
            let owned_by_other = matches!(owner, Some(o) if o != target_project);
            !owned_by_other
                && !registry::is_platform_managed(r.kind, doc)
                && !auto_created.contains_key(&r.key())
        })
        .map(|(r, _)| {
            let managed = matches!(owned_by_any.get(&r.key()), Some(o) if o == target_project);
            let suffix = if managed { " (managed)" } else { "" };
            let label = format!(
                "{} {}/{}{}",
                prefix(r.kind),
                r.kind.directory_name(),
                r.name,
                suffix
            );
            (r.clone(), label)
        })
        .collect();
    items.sort_by(|(a, _), (b, _)| {
        (
            domain_rank(a.kind),
            a.kind.directory_name(),
            a.name.as_str(),
        )
            .cmp(&(
                domain_rank(b.kind),
                b.kind.directory_name(),
                b.name.as_str(),
            ))
    });
    items
}

/// `(additions, dep_keys, owned_refs)` — see [`expand_deps`].
type ExpandDepsResult = (
    Vec<(ResourceRef, Value)>,
    BTreeSet<String>,
    Vec<(String, String)>,
);

/// Expand `to_adopt`'s upstream dependency graph. Returns the additions (not
/// yet appended to `to_adopt`), their keys, and deduped `(key, owner)` pairs
/// for references that were encountered but skipped because they're already
/// owned by some project — so callers (the wizard) can surface "already
/// managed" instead of silently dropping them. Auto-created/platform-managed
/// refs are never recorded here (they aren't adoptable dependencies at all).
/// Same traversal as the `--with-deps` flag path; callers append the result
/// themselves so both the flag path and the wizard's ask-first path share one
/// algorithm.
fn expand_deps(
    to_adopt: &[(ResourceRef, Value)],
    owned_by_any: &BTreeMap<String, String>,
    auto_created: &BTreeMap<String, String>,
    snap_map: &BTreeMap<String, (ResourceRef, Value)>,
) -> ExpandDepsResult {
    let mut in_set: BTreeSet<String> = to_adopt.iter().map(|(r, _)| r.key()).collect();
    let mut queue: Vec<(ResourceRef, Value)> = to_adopt.to_vec();
    let mut additions: Vec<(ResourceRef, Value)> = Vec::new();
    let mut dep_keys: BTreeSet<String> = BTreeSet::new();
    let mut owned_refs: Vec<(String, String)> = Vec::new();
    let mut owned_refs_seen: BTreeSet<String> = BTreeSet::new();
    while let Some((r, doc)) = queue.pop() {
        for (dk, dn) in registry::extract_references(r.kind, &doc) {
            let dref = ResourceRef::new(dk, dn);
            let key = dref.key();
            if in_set.contains(&key) {
                continue; // already selected → not an unmanaged dep
            }
            if auto_created.contains_key(&key) {
                continue; // auto-created → never adopted, not even as a dep, and never reported
            }
            if let Some(owner) = owned_by_any.get(&key) {
                if owned_refs_seen.insert(key.clone()) {
                    owned_refs.push((key.clone(), owner.clone()));
                }
                continue; // owned by someone → not an unmanaged dep, but worth surfacing
            }
            if let Some((rr, dv)) = snap_map.get(&key) {
                if registry::is_platform_managed(rr.kind, dv) {
                    continue; // platform-managed → never adopted, not even as a dep
                }
                in_set.insert(key.clone());
                dep_keys.insert(key.clone());
                additions.push((rr.clone(), dv.clone()));
                queue.push((rr.clone(), dv.clone()));
            }
        }
    }
    (additions, dep_keys, owned_refs)
}

/// Reconstruct the non-interactive command line equivalent to a wizard run,
/// e.g. `rigg adopt regulus agents/regulus --with-deps`.
fn equivalent_command(project: &str, chosen: &[String], with_deps: bool) -> String {
    let mut cmd = format!("rigg adopt {project} {}", chosen.join(" "));
    if with_deps {
        cmd.push_str(" --with-deps");
    }
    cmd
}

fn report(
    ctx: &GlobalContext,
    to_adopt: &[(ResourceRef, Value)],
    skipped: &[(String, String)],
    dep_keys: &BTreeSet<String>,
    dry_run: bool,
) -> Result<()> {
    if ctx.json() {
        let key = if dry_run { "would_adopt" } else { "adopted" };
        let value = json!({
            key: to_adopt.iter().map(|(r, _)| r.key()).collect::<Vec<_>>(),
            "dependencies": dep_keys.iter().cloned().collect::<Vec<_>>(),
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
        let tag = if dep_keys.contains(&r.key()) {
            " (dependency)"
        } else {
            ""
        };
        if dry_run {
            println!("  would adopt {r}{tag}");
        } else {
            println!("  + adopted {r}{tag}");
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
    use serde_json::json;
    use std::collections::BTreeMap;

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

    fn snap() -> Vec<(ResourceRef, serde_json::Value)> {
        vec![
            (
                ResourceRef::new(ResourceKind::Index, "docs".to_string()),
                json!({"name": "docs"}),
            ),
            (
                ResourceRef::new(ResourceKind::Agent, "regulus".to_string()),
                json!({"name": "regulus"}),
            ),
            (
                ResourceRef::new(ResourceKind::Agent, "helper".to_string()),
                json!({"name": "helper"}),
            ),
        ]
    }

    #[test]
    fn wizard_candidates_filters_owned_and_groups_foundry_first() {
        let mut owned = BTreeMap::new();
        owned.insert("agents/helper".to_string(), "other".to_string());
        let items = wizard_candidates(&snap(), &owned, &BTreeMap::new(), "demo");
        let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            vec!["[Foundry] agents/regulus", "[Search] indexes/docs"]
        );
    }

    #[test]
    fn wizard_candidates_excludes_platform_managed_guardrail() {
        let owned = BTreeMap::new();
        let snap = vec![
            (
                ResourceRef::new(ResourceKind::Index, "docs".to_string()),
                json!({"name": "docs"}),
            ),
            (
                ResourceRef::new(ResourceKind::Guardrail, "Microsoft.DefaultV2".to_string()),
                json!({"name": "Microsoft.DefaultV2", "properties": {"type": "SystemManaged"}}),
            ),
            (
                ResourceRef::new(ResourceKind::Guardrail, "my-policy".to_string()),
                json!({"name": "my-policy", "properties": {"type": "UserManaged"}}),
            ),
        ];
        let items = wizard_candidates(&snap, &owned, &BTreeMap::new(), "demo");
        let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
        assert_eq!(
            labels,
            vec!["[Foundry] guardrails/my-policy", "[Search] indexes/docs"]
        );
    }

    #[test]
    fn wizard_candidates_marks_target_project_owned_as_managed() {
        let mut owned = BTreeMap::new();
        owned.insert("agents/regulus".to_string(), "regulus".to_string()); // target project
        owned.insert("agents/helper".to_string(), "other".to_string()); // other project
        let snap = vec![
            (
                ResourceRef::new(ResourceKind::Agent, "regulus".to_string()),
                serde_json::json!({"name": "regulus"}),
            ),
            (
                ResourceRef::new(ResourceKind::Agent, "helper".to_string()),
                serde_json::json!({"name": "helper"}),
            ),
            (
                ResourceRef::new(ResourceKind::Agent, "newbie".to_string()),
                serde_json::json!({"name": "newbie"}),
            ),
        ];
        let items = wizard_candidates(&snap, &owned, &BTreeMap::new(), "regulus");
        let labels: Vec<&str> = items.iter().map(|(_, l)| l.as_str()).collect();
        assert!(
            labels.contains(&"[Foundry] agents/regulus (managed)"),
            "{labels:?}"
        );
        assert!(labels.contains(&"[Foundry] agents/newbie"), "{labels:?}");
        assert!(
            !labels.iter().any(|l| l.contains("helper")),
            "other-project resources hidden: {labels:?}"
        );
    }

    #[test]
    fn expand_deps_skips_platform_managed_guardrail_dependency() {
        let deployment = json!({
            "name": "gpt-5-mini",
            "properties": {"raiPolicyName": "Microsoft.DefaultV2"}
        });
        let to_adopt = vec![(
            ResourceRef::new(ResourceKind::Deployment, "gpt-5-mini".to_string()),
            deployment,
        )];
        let owned_by_any: BTreeMap<String, String> = BTreeMap::new();
        let mut snap_map: BTreeMap<String, (ResourceRef, Value)> = BTreeMap::new();
        snap_map.insert(
            "guardrails/Microsoft.DefaultV2".to_string(),
            (
                ResourceRef::new(ResourceKind::Guardrail, "Microsoft.DefaultV2".to_string()),
                json!({"name": "Microsoft.DefaultV2", "properties": {"type": "SystemManaged"}}),
            ),
        );
        let (adds, dep_keys, owned_refs) =
            expand_deps(&to_adopt, &owned_by_any, &BTreeMap::new(), &snap_map);
        assert!(adds.is_empty(), "expected no deps adopted, got {adds:?}");
        assert!(dep_keys.is_empty());
        assert!(owned_refs.is_empty());
    }

    #[test]
    fn expand_deps_reports_owned_references() {
        let indexer = json!({
            "name": "ix", "dataSourceName": "ds", "targetIndexName": "idx"
        });
        let ds = json!({"name": "ds", "type": "azureblob"});
        let idx = json!({"name": "idx"});
        let roots = vec![(
            ResourceRef::new(ResourceKind::Indexer, "ix".to_string()),
            indexer,
        )];
        let mut owned = BTreeMap::new();
        owned.insert("indexes/idx".to_string(), "demo".to_string());
        let snap_map: BTreeMap<String, (ResourceRef, serde_json::Value)> = [
            (
                ResourceRef::new(ResourceKind::DataSource, "ds".to_string()),
                ds,
            ),
            (
                ResourceRef::new(ResourceKind::Index, "idx".to_string()),
                idx,
            ),
        ]
        .into_iter()
        .map(|(r, v)| (r.key(), (r, v)))
        .collect();
        let (adds, _keys, owned_refs) = expand_deps(&roots, &owned, &BTreeMap::new(), &snap_map);
        assert_eq!(adds.len(), 1, "only the unmanaged data source is added");
        assert!(
            owned_refs.contains(&("indexes/idx".to_string(), "demo".to_string())),
            "{owned_refs:?}"
        );
    }

    #[test]
    fn equivalent_command_reconstructs_invocation() {
        assert_eq!(
            equivalent_command("regulus", &["agents/regulus".into()], true),
            "rigg adopt regulus agents/regulus --with-deps"
        );
        assert_eq!(
            equivalent_command("p", &["indexes/a".into(), "indexes/b".into()], false),
            "rigg adopt p indexes/a indexes/b"
        );
    }
}
