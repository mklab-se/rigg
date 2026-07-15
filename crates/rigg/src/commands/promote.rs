//! `rigg promote` — copy one environment's project tree into another,
//! preserving pinned (environment-specific) fields. Purely local: it reads
//! and writes project files only, and never talks to Azure. The subsequent
//! `rigg diff`/`rigg push` (against the target env) are what actually sync
//! with the cloud.
//!
//! Correlation across environments is by LOGICAL id — the resource's file
//! stem within its kind directory — not by physical (Azure) name, since the
//! two may diverge once a resource is renamed in one environment (see
//! `rigg-core::store` module docs).

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::{Value, json};

use rigg_core::registry::{self, X_RIGG_PIN};
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::Store;
use rigg_diff::output::SideLabels;
use rigg_diff::semantic::DiffResult;

use rigg_client::arm::ArmClient;

use crate::cli::PromoteArgs;
use crate::commands::{
    CommandError, GlobalContext, credentials, interactive, load_workspace, select_one_project,
};

/// One logical resource's promotion plan: what it looks like in the source
/// env, what (if anything) it looks like in the target env today, and what
/// it would become after pinned fields are re-applied.
struct Item {
    kind: ResourceKind,
    stem: String,
    target: Option<Value>,
    merged: Value,
    /// Pinned paths used to build `merged` (empty when nothing was kept,
    /// i.e. a brand-new file where there is nothing to pin from yet).
    pinned: Vec<String>,
    diff: DiffResult,
}

impl Item {
    fn label(&self) -> String {
        format!("{}/{}", self.kind.directory_name(), self.stem)
    }
}

struct Plan {
    changed: Vec<Item>,
    new: Vec<Item>,
    unchanged: Vec<Item>,
    kept_only_in_to: Vec<(ResourceKind, String)>,
}

pub async fn run(ctx: &GlobalContext, args: PromoteArgs) -> Result<()> {
    let ws = load_workspace()?;
    if !ws.config.environments.contains_key(&args.from) {
        return Err(anyhow!(CommandError::Usage(format!(
            "unknown environment '{}' (see `rigg env list`)",
            args.from
        ))));
    }
    if !ws.config.environments.contains_key(&args.to) {
        return Err(anyhow!(CommandError::Usage(format!(
            "unknown environment '{}' (see `rigg env list`)",
            args.to
        ))));
    }
    if args.from == args.to {
        return Err(anyhow!(CommandError::Usage(
            "--from and --to must name different environments".to_string()
        )));
    }

    let project = select_one_project(&ws, args.project.as_deref())?;
    let store_from = Store::new(project, &args.from);
    let store_to = Store::new(project, &args.to);

    let mut plan = build_plan(&store_from, &store_to)?;

    if !ctx.json() {
        print_preview(&args, &project.name, &plan);
    }

    let nothing_to_do = plan.changed.is_empty() && plan.new.is_empty();

    if args.dry_run {
        if !ctx.json() {
            println!();
            println!("(dry run — nothing written)");
        } else {
            print_json(&plan, true);
        }
        return Ok(());
    }

    if nothing_to_do {
        if ctx.json() {
            print_json(&plan, false);
        } else {
            println!();
            println!(
                "nothing to promote — '{}' already matches '{}'",
                args.to, args.from
            );
        }
        return Ok(());
    }

    if ctx.interactive() && !ctx.json() {
        if !interactive::confirm_default_yes("Proceed?", ctx.no_color)? {
            println!("aborted");
            return Ok(());
        }
    } else if !ctx.yes {
        return Err(anyhow!(CommandError::Usage(
            "non-interactive promote requires --yes".to_string()
        )));
    }

    resolve_new_webapi_uris(ctx, &args.from, &args.to, &mut plan.new).await?;

    for item in &plan.changed {
        let name = item
            .merged
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&item.stem)
            .to_string();
        store_to.write(&ResourceRef::new(item.kind, name), &item.merged)?;
    }
    for item in &plan.new {
        store_to.write_at(&item.stem, item.kind, &item.merged)?;
    }

    if ctx.json() {
        print_json(&plan, false);
    } else {
        println!();
        println!("hint: rigg diff {} -e {}", project.name, args.to);
        println!("      rigg push {} -e {}", project.name, args.to);
        print_new_file_hints(&args, &plan);
    }
    Ok(())
}

/// A skillset that is NEW in the target env carries its Web API skill URLs
/// verbatim from the source env — usually the WRONG function for the target
/// (the pinned paths protect existing files, but a new file has nothing to
/// pin from). Resolve each URL: automatically when Azure shows the source's
/// function app as the only one visible (both envs share it), interactively
/// otherwise; non-interactively the copy is kept but flagged loudly.
async fn resolve_new_webapi_uris(
    ctx: &GlobalContext,
    from: &str,
    to: &str,
    new_items: &mut [Item],
) -> Result<()> {
    let mut sites: Option<Vec<String>> = None; // ARM site list, fetched once on demand
    for item in new_items
        .iter_mut()
        .filter(|i| i.kind == ResourceKind::Skillset)
    {
        let Some(skills) = item.merged.get_mut("skills").and_then(Value::as_array_mut) else {
            continue;
        };
        for skill in skills {
            let is_webapi = skill
                .get("@odata.type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.ends_with("WebApiSkill"));
            if !is_webapi {
                continue;
            }
            let uri = skill
                .get("uri")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let Some((site, _)) = credentials::parse_function_uri(&uri) else {
                continue;
            };
            let skill_name = skill
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("<unnamed>")
                .to_string();

            if !ctx.interactive() || ctx.json() {
                if !ctx.json() {
                    println!(
                        "  {} skill '{skill_name}' calls {uri} — copied from env '{from}'; \
                         verify this is the right function for '{to}' (edit the file, then `rigg push -e {to}`)",
                        "!".yellow()
                    );
                }
                continue;
            }

            if sites.is_none() {
                sites = Some(match ArmClient::new() {
                    Ok(arm) => arm.list_web_sites().await.unwrap_or_default(),
                    Err(_) => Vec::new(),
                });
            }
            let known = sites.as_ref().expect("filled above");
            let others: Vec<&String> = known
                .iter()
                .filter(|s| !s.eq_ignore_ascii_case(&site))
                .collect();
            if others.is_empty() && known.iter().any(|s| s.eq_ignore_ascii_case(&site)) {
                println!(
                    "  {} skill '{skill_name}': '{site}' is the only function app your login can see — both environments share it, keeping {uri}",
                    "✓".green()
                );
                continue;
            }

            let keep = format!("keep {uri} (same function app as '{from}')");
            const MANUAL: &str = "enter a URL manually";
            let mut options = vec![keep.clone()];
            for other in &others {
                options.push(swap_function_site(&uri, &site, other));
            }
            options.push(MANUAL.to_string());
            let choice = interactive::select(
                &format!(
                    "Skill '{skill_name}' calls a function in env '{from}' — which URL should '{to}' use?"
                ),
                options,
                ctx.no_color,
            )?;
            let new_uri = if choice == keep {
                uri.clone()
            } else if choice == MANUAL {
                interactive::text_with_default(
                    &format!("Function URL for env '{to}':"),
                    &uri,
                    ctx.no_color,
                )?
            } else {
                choice
            };
            if new_uri != uri {
                skill["uri"] = Value::String(new_uri);
            }
        }
    }
    Ok(())
}

/// `https://<site>.azurewebsites.net/<path>` with the site swapped.
fn swap_function_site(uri: &str, old_site: &str, new_site: &str) -> String {
    uri.replacen(
        &format!("https://{old_site}.azurewebsites.net"),
        &format!("https://{new_site}.azurewebsites.net"),
        1,
    )
}

/// Build the promotion plan: correlate FROM/TO by (kind, stem), merge each
/// FROM resource into its TO counterpart (pinning fields per
/// `pinned_paths`), and classify the result.
fn build_plan(store_from: &Store, store_to: &Store) -> Result<Plan> {
    let from_map = list_by_stem(store_from)?;
    let to_map = list_by_stem(store_to)?;

    let mut changed = Vec::new();
    let mut new = Vec::new();
    let mut unchanged = Vec::new();

    for ((kind, stem), from_path) in &from_map {
        let source = store_from.read_path(from_path)?;
        let target = match to_map.get(&(*kind, stem.clone())) {
            Some(to_path) => Some(store_to.read_path(to_path)?),
            None => None,
        };
        let pinned = pinned_paths(*kind, target.as_ref());
        let merged = merge_promote(*kind, &source, target.as_ref(), &pinned);
        let diff =
            rigg_diff::semantic::diff(target.as_ref().unwrap_or(&Value::Null), &merged, "name");
        let item = Item {
            kind: *kind,
            stem: stem.clone(),
            target: target.clone(),
            merged,
            pinned,
            diff,
        };
        match &item.target {
            None => new.push(item),
            Some(_) if item.diff.is_equal => unchanged.push(item),
            Some(_) => changed.push(item),
        }
    }

    let mut kept_only_in_to: Vec<(ResourceKind, String)> = to_map
        .keys()
        .filter(|k| !from_map.contains_key(*k))
        .cloned()
        .collect();
    kept_only_in_to.sort();

    Ok(Plan {
        changed,
        new,
        unchanged,
        kept_only_in_to,
    })
}

fn list_by_stem(store: &Store) -> Result<BTreeMap<(ResourceKind, String), PathBuf>> {
    let mut out = BTreeMap::new();
    for (r, path) in store.list()? {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        out.insert((r.kind, stem), path);
    }
    Ok(out)
}

/// The full set of paths kept pinned to the target's current value for this
/// kind/resource: `"name"` (always — the target keeps its physical name) ∪
/// the kind's registry defaults ∪ any extra paths the target's own
/// `x-rigg-pin` annotation names ∪ the annotation key itself (so it survives
/// the promote and keeps applying on the next one). When there is no target
/// yet, this is still the set that WOULD apply — used to hint which fields
/// are worth reviewing on the new copy.
fn pinned_paths(kind: ResourceKind, target: Option<&Value>) -> Vec<String> {
    let mut pinned: Vec<String> = vec!["name".to_string()];
    pinned.extend(registry::env_pinned(kind).into_iter().map(String::from));
    if let Some(target) = target {
        if let Some(extra) = target.get(X_RIGG_PIN).and_then(Value::as_array) {
            for p in extra {
                if let Some(s) = p.as_str() {
                    pinned.push(s.to_string());
                }
            }
        }
        pinned.push(X_RIGG_PIN.to_string());
    }
    pinned.sort();
    pinned.dedup();
    pinned
}

/// The source's document becomes the target's, except at `pinned` paths,
/// which keep the target's current value (when a target exists at all — a
/// brand-new resource has nothing to pin from, and is created verbatim).
/// Target-only array elements along pinned array paths survive wholesale
/// (see `registry::restore_path`).
fn merge_promote(
    kind: ResourceKind,
    source: &Value,
    target: Option<&Value>,
    pinned: &[String],
) -> Value {
    let mut merged = source.clone();
    // The x-rigg-pin annotation belongs to the TARGET env's file only — a
    // source-side copy must not leak across; the target's own annotation (if
    // any) is restored below via the pinned X_RIGG_PIN path.
    if let Some(map) = merged.as_object_mut() {
        map.remove(X_RIGG_PIN);
    }
    // Same for a skill's x-rigg-auth: it authorizes THE SOURCE env's
    // function URL, and unlike other pinned paths it must not survive when
    // the target has no counterpart — an annotation pointing at the wrong
    // env's function would silence push's Web API auth gate. Strip it here;
    // the target's own annotation (if any) is restored via the pinned path.
    if kind == ResourceKind::Skillset {
        if let Some(skills) = merged.get_mut("skills").and_then(Value::as_array_mut) {
            for skill in skills {
                if let Some(map) = skill.as_object_mut() {
                    map.remove(credentials::X_RIGG_AUTH);
                }
            }
        }
    }
    if let Some(target) = target {
        for path in pinned {
            registry::restore_path(&mut merged, target, path);
        }
    }
    merged
}

fn print_preview(args: &PromoteArgs, project_name: &str, plan: &Plan) {
    println!(
        "{} project '{}': {} {} {}",
        "Promote".bold(),
        project_name,
        args.from,
        "→".dimmed(),
        args.to
    );
    println!(
        "  {} changed, {} new, {} unchanged, {} kept (only in '{}')",
        plan.changed.len(),
        plan.new.len(),
        plan.unchanged.len(),
        plan.kept_only_in_to.len(),
        args.to
    );

    if !plan.changed.is_empty() {
        println!();
        let labels = SideLabels {
            new_side: format!("{} (incoming)", args.from),
            old_side: args.to.clone(),
        };
        for item in &plan.changed {
            print!(
                "{}",
                rigg_diff::output::format_text(&item.diff, &item.label(), &labels)
            );
        }
    }

    if !plan.new.is_empty() {
        println!();
        println!("new (will be created in '{}'):", args.to);
        for item in &plan.new {
            println!("  {}", item.label());
        }
    }

    if !plan.kept_only_in_to.is_empty() {
        println!();
        println!("kept (only in '{}' — never touched by promote):", args.to);
        for (kind, stem) in &plan.kept_only_in_to {
            println!("  {}/{}", kind.directory_name(), stem);
        }
    }
}

fn print_new_file_hints(args: &PromoteArgs, plan: &Plan) {
    let with_pins: Vec<&Item> = plan
        .new
        .iter()
        .filter(|i| !registry::env_pinned(i.kind).is_empty())
        .collect();
    if with_pins.is_empty() {
        return;
    }
    println!();
    println!(
        "New files were created verbatim from '{}'. Fields worth reviewing (env-pinned by default):",
        args.from
    );
    for item in with_pins {
        println!(
            "  {}: {}",
            item.label(),
            registry::env_pinned(item.kind).join(", ")
        );
    }
}

fn print_json(plan: &Plan, dry_run: bool) {
    let mut pinned_kept: serde_json::Map<String, Value> = serde_json::Map::new();
    for item in &plan.changed {
        pinned_kept.insert(item.label(), json!(item.pinned));
    }
    let value = json!({
        "dry_run": dry_run,
        "promoted": plan.changed.iter().map(Item::label).collect::<Vec<_>>(),
        "created": plan.new.iter().map(Item::label).collect::<Vec<_>>(),
        "kept_only_in_to": plan
            .kept_only_in_to
            .iter()
            .map(|(k, s)| format!("{}/{}", k.directory_name(), s))
            .collect::<Vec<_>>(),
        "pinned_kept": pinned_kept,
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&value).unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn merge_promote_keeps_name_from_target() {
        let source = json!({"name": "a-name", "model": "m"});
        let target = json!({"name": "b-name", "model": "old"});
        let merged = merge_promote(
            ResourceKind::Agent,
            &source,
            Some(&target),
            &["name".to_string()],
        );
        assert_eq!(merged["name"], json!("b-name"));
        assert_eq!(
            merged["model"],
            json!("m"),
            "non-pinned field comes from source"
        );
    }

    #[test]
    fn merge_promote_keeps_registry_pinned_path() {
        // Real Agent shape: tools[].server_url differs per env (points at a
        // different Search service) and must stay pinned to the target's.
        let source = json!({
            "name": "agent",
            "model": "gpt-5-mini",
            "tools": [{"type": "mcp", "server_url": "https://dev.search.windows.net/x"}]
        });
        let target = json!({
            "name": "agent",
            "model": "gpt-5-mini",
            "tools": [{"type": "mcp", "server_url": "https://prod.search.windows.net/x"}]
        });
        let pinned = pinned_paths(ResourceKind::Agent, Some(&target));
        assert!(pinned.iter().any(|p| p == "tools[].server_url"));
        let merged = merge_promote(ResourceKind::Agent, &source, Some(&target), &pinned);
        assert_eq!(
            merged["tools"][0]["server_url"],
            json!("https://prod.search.windows.net/x"),
            "target's server_url kept, not source's"
        );
    }

    #[test]
    fn merge_promote_preserves_target_only_tools() {
        // CRITICAL regression: prod (target) has customizations dev doesn't —
        // an extra file_search tool and a second MCP tool. Promote must not
        // silently delete them: they survive the merge wholesale.
        let source = json!({
            "name": "agent",
            "model": "gpt-5-mini",
            "tools": [{"type": "mcp", "server_url": "https://dev.search.windows.net/x"}]
        });
        let target = json!({
            "name": "agent",
            "model": "gpt-4o-old",
            "tools": [
                {"type": "mcp", "server_url": "https://prod.search.windows.net/x"},
                {"type": "file_search", "vector_store_ids": ["vs-prod"]},
                {"type": "mcp", "server_url": "https://prod.search.windows.net/y",
                 "project_connection_id": "conn-prod-2"}
            ]
        });
        let pinned = pinned_paths(ResourceKind::Agent, Some(&target));
        let merged = merge_promote(ResourceKind::Agent, &source, Some(&target), &pinned);
        let tools = merged["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 3, "target-only tools survive: {tools:?}");
        assert_eq!(
            tools[0]["server_url"],
            json!("https://prod.search.windows.net/x"),
            "paired tool keeps target's pinned field"
        );
        assert_eq!(
            tools[1],
            json!({"type": "file_search", "vector_store_ids": ["vs-prod"]}),
            "target-only tool survives wholesale"
        );
        assert_eq!(
            tools[2]["project_connection_id"],
            json!("conn-prod-2"),
            "second target-only tool survives with all fields"
        );
        assert_eq!(merged["model"], json!("gpt-5-mini"), "non-pinned promoted");
    }

    #[test]
    fn merge_promote_source_only_tools_stay() {
        // Reverse direction: source has MORE tools than the target. The
        // extras come from the source (that's the promotion) and keep the
        // source's own values — nothing on the target side to pin from.
        let source = json!({
            "name": "agent",
            "tools": [
                {"type": "mcp", "server_url": "https://dev/x"},
                {"type": "code_interpreter"}
            ]
        });
        let target = json!({
            "name": "agent",
            "tools": [{"type": "mcp", "server_url": "https://prod/x"}]
        });
        let pinned = pinned_paths(ResourceKind::Agent, Some(&target));
        let merged = merge_promote(ResourceKind::Agent, &source, Some(&target), &pinned);
        let tools = merged["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2, "source's extra tool is promoted: {tools:?}");
        assert_eq!(tools[0]["server_url"], json!("https://prod/x"), "paired");
        assert_eq!(tools[1], json!({"type": "code_interpreter"}));
    }

    #[test]
    fn merge_promote_keeps_x_rigg_pin_listed_extra_path() {
        let source = json!({"name": "conn", "properties": {"description": "new description"}});
        let target = json!({
            "name": "conn",
            "properties": {"description": "prod description"},
            "x-rigg-pin": ["properties.description"]
        });
        let pinned = pinned_paths(ResourceKind::Connection, Some(&target));
        let merged = merge_promote(ResourceKind::Agent, &source, Some(&target), &pinned);
        assert_eq!(
            merged["properties"]["description"],
            json!("prod description"),
            "x-rigg-pin-listed path kept from target"
        );
    }

    #[test]
    fn merge_promote_target_none_is_source_verbatim() {
        let source = json!({"name": "a", "model": "m", "tools": []});
        let merged = merge_promote(
            ResourceKind::Agent,
            &source,
            None,
            &["name".to_string(), "model".to_string()],
        );
        assert_eq!(merged, source);
    }

    #[test]
    fn merge_promote_x_rigg_pin_annotation_itself_survives() {
        let source = json!({"name": "conn", "properties": {"target": "https://dev"}});
        let target = json!({
            "name": "conn",
            "properties": {"target": "https://prod"},
            "x-rigg-pin": ["properties.description"]
        });
        let pinned = pinned_paths(ResourceKind::Connection, Some(&target));
        let merged = merge_promote(ResourceKind::Agent, &source, Some(&target), &pinned);
        assert_eq!(
            merged["x-rigg-pin"],
            json!(["properties.description"]),
            "the annotation itself travels with the target, unmerged from source"
        );
        assert_eq!(
            merged["properties"]["target"],
            json!("https://prod"),
            "properties.target is env_pinned_extra for Connection — kept from target too"
        );
    }

    #[test]
    fn merge_promote_strips_source_side_x_rigg_pin() {
        // The annotation lives in the TARGET env's file. A source-side copy
        // (e.g. promoted A→B earlier, now promoting B→A's sibling) must not
        // leak into the merged output when the target has none of its own.
        let source = json!({
            "name": "conn",
            "properties": {"target": "https://dev"},
            "x-rigg-pin": ["properties.description"]
        });
        let target = json!({"name": "conn", "properties": {"target": "https://prod"}});
        let pinned = pinned_paths(ResourceKind::Connection, Some(&target));
        let merged = merge_promote(ResourceKind::Agent, &source, Some(&target), &pinned);
        assert!(
            merged.get(X_RIGG_PIN).is_none(),
            "source's annotation must not leak: {merged:?}"
        );
        // and target None (new file) also drops it — the fresh copy starts clean
        let created = merge_promote(ResourceKind::Agent, &source, None, &pinned);
        assert!(created.get(X_RIGG_PIN).is_none());
    }

    #[test]
    fn pinned_paths_has_no_target_annotation_key_when_target_is_none() {
        // A brand-new file: nothing to keep pinned yet, so pinned_paths is
        // just the structural defaults (used only for the review hint).
        let pinned = pinned_paths(ResourceKind::Agent, None);
        assert!(!pinned.iter().any(|p| p == X_RIGG_PIN));
        assert!(pinned.iter().any(|p| p == "tools[].server_url"));
    }
}
