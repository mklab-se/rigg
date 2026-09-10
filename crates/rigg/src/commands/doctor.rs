//! `rigg auth doctor [--fix]` — verify (and repair) the service-to-service
//! identity graph the workspace requires (spec §8.2).

use anyhow::{Context, Result, bail};
use colored::Colorize;

use rigg_client::arm::ArmClient;
use rigg_client::arm_resources::resolve_account_scope;
use rigg_core::binding::{BindingCache, BindingType, EnvBindings};
use rigg_core::identity::{Edge, EdgeKind, Principal, Scope, graph_for_docs};
use rigg_core::registry::Provider;
use rigg_core::resources::ResourceKind;
use rigg_core::store::Store;
use rigg_core::workspace::Workspace;
use serde_json::Value;

use crate::commands::{GlobalContext, load_workspace, resolve_env};

/// Every document in the environment's tree, as `graph_for_docs` wants them.
fn env_documents(ws: &Workspace, env: &str) -> Vec<(ResourceKind, String, Value)> {
    let mut docs = Vec::new();
    for project in &ws.projects {
        let store = Store::new(project, env);
        let Ok(files) = store.list() else { continue };
        for (r, _) in files {
            if let Ok(value) = store.read(&r) {
                docs.push((r.kind, r.name.clone(), value));
            }
        }
    }
    docs
}

pub async fn run(ctx: &GlobalContext, fix: bool) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let cache = BindingCache::load(&ws, &env.name);
    let bindings = EnvBindings::of_env(&env.name, &env.env, Some(&cache));
    let edges = graph_for_docs(&bindings, &env_documents(&ws, &env.name)).edges;
    if edges.is_empty() {
        println!(
            "{} no service-to-service identity requirements found in this workspace",
            "✓".green().bold()
        );
        return Ok(());
    }

    println!(
        "{} {} identity edge(s) derived from workspace files (env: {})",
        "Doctor".bold(),
        edges.len(),
        env.name
    );

    let arm = ArmClient::new().context("auth doctor needs ARM access (az login)")?;

    // Resolve principal identities once.
    let search_conn = env.search();
    let foundry_conn = env.foundry();

    let mut search_identity = None;
    let mut search_service_id = None;
    if let Some(conn) = search_conn {
        let id = arm.find_search_service_id(&conn.service).await?;
        search_identity = arm.get_resource_identity(&id, Provider::SearchArm).await?;
        search_service_id = Some(id);
    }
    let mut foundry_identity = None;
    let mut foundry_project_id = None;
    if let Some(conn) = foundry_conn {
        let scope = resolve_account_scope(&arm, &conn.account).await?;
        let account_id = format!(
            "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}",
            scope.subscription_id, scope.resource_group, scope.account
        );
        let project_id = format!("{account_id}/projects/{}", conn.project);
        foundry_identity = arm
            .get_resource_identity(&project_id, Provider::CognitiveServicesArm)
            .await
            .ok()
            .flatten();
        foundry_project_id = Some(project_id);
    }

    let mut failures: Vec<String> = Vec::new();
    let mut report = Vec::new();

    for edge in &edges {
        // Resolve principal + scope for this edge.
        let (identity, principal_desc, identity_resource, identity_api) = match &edge.principal {
            Principal::SearchSystem | Principal::SearchUser { .. } => (
                search_identity.as_ref(),
                search_conn
                    .map(|c| format!("search service '{}'", c.service))
                    .unwrap_or_else(|| "search service (no connection configured)".into()),
                search_service_id.clone(),
                Provider::SearchArm,
            ),
            Principal::FoundryProject => (
                foundry_identity.as_ref(),
                foundry_conn
                    .map(|c| format!("foundry project '{}/{}'", c.account, c.project))
                    .unwrap_or_else(|| "foundry project (no connection configured)".into()),
                foundry_project_id.clone(),
                Provider::CognitiveServicesArm,
            ),
            other => {
                println!("  {} {other} — {}", "ⓘ".blue(), edge.reason);
                report.push((edge, "informational".to_string()));
                continue;
            }
        };
        let target = edge.scope.describe();
        // Unbound Cognitive Services accounts (skillset enrichment) are still
        // resolvable by name through ARM — any Cognitive Services kind, not
        // just Foundry's AIServices accounts.
        let scope = match &edge.scope {
            Scope::Resolved(id) => Some(id.clone()),
            Scope::Unresolved {
                kind: Some(BindingType::AiServices),
                physical,
                ..
            } => match arm.find_cognitive_account_id(physical).await {
                Ok(id) => Some(id),
                Err(e) => {
                    println!(
                        "  {} could not resolve AI services account '{physical}' via ARM: {e}",
                        "?".yellow()
                    );
                    None
                }
            },
            Scope::Unresolved { .. } => None,
        };

        if edge.kind != EdgeKind::Rbac {
            println!("  {} {} — {}", "ⓘ".blue(), edge.role.name, edge.reason);
            report.push((edge, "informational".to_string()));
            continue;
        }

        let Some(scope) = scope else {
            println!(
                "  {} {} → {} — cannot resolve target scope (bind it in rigg.yaml, then \
                 `rigg env resolve`)",
                "?".yellow().bold(),
                principal_desc,
                target
            );
            failures.push(format!("unresolved scope for {}", edge.reason));
            continue;
        };

        // Ensure the principal has an identity.
        let principal_ids: Vec<String> = identity
            .map(|i| i.principal_ids().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        if principal_ids.is_empty() {
            if fix && let Some(resource) = &identity_resource {
                println!(
                    "  {} enabling system-assigned identity on {principal_desc}...",
                    "fix".cyan().bold()
                );
                arm.enable_system_identity(resource, identity_api).await?;
                println!(
                    "    identity enabled — rerun `rigg auth doctor` to verify role assignments"
                );
                failures.push(format!(
                    "identity newly enabled for {principal_desc}; rerun doctor"
                ));
                continue;
            }
            println!(
                "  {} {principal_desc} has no managed identity — run with --fix or:\n      az search service update ... --identity-type SystemAssigned",
                "✗".red().bold()
            );
            failures.push(format!("{principal_desc} has no managed identity"));
            continue;
        }

        // Check role assignments.
        let mut assigned = false;
        for pid in &principal_ids {
            let roles = arm.list_role_assignments(&scope, pid).await?;
            if roles.iter().any(|r| r.ends_with(edge.role.id)) {
                assigned = true;
                break;
            }
        }

        if assigned {
            println!(
                "  {} {} → {} ({})",
                "✓".green().bold(),
                principal_desc,
                target,
                edge.role.name
            );
            report.push((edge, "ok".to_string()));
        } else if fix {
            println!(
                "  {} assigning '{}' to {} at {}...",
                "fix".cyan().bold(),
                edge.role.name,
                principal_desc,
                scope
            );
            arm.create_role_assignment(&scope, &principal_ids[0], edge.role.id)
                .await?;
            println!("    {} assigned", "✓".green());
            report.push((edge, "fixed".to_string()));
        } else {
            println!(
                "  {} {} lacks '{}' on {}\n      reason: {}\n      fix:    az role assignment create --assignee {} --role \"{}\" --scope \"{}\"",
                "✗".red().bold(),
                principal_desc,
                edge.role.name,
                target,
                edge.reason,
                principal_ids[0],
                edge.role.name,
                scope
            );
            failures.push(edge.reason.clone());
            report.push((edge, "missing".to_string()));
        }
    }

    if ctx.json() {
        let value: Vec<_> = report
            .iter()
            .map(|(e, status)| serde_json::json!({"edge": e, "status": status}))
            .collect();
        println!("{}", serde_json::to_string_pretty(&value)?);
    }

    if !failures.is_empty()
        && crate::commands::ai_assist::ai_on(ctx)
        && let Ok(advice) = crate::commands::ai_assist::explain_doctor(&failures).await
    {
        println!();
        println!("AI advice (ailloy):");
        for line in advice.lines() {
            println!("  {line}");
        }
    }

    if failures.is_empty() {
        println!();
        println!("{} identity wiring looks good", "✓".green().bold());
        Ok(())
    } else if fix {
        println!();
        bail!(
            "{} identity problem(s) rigg could not fix automatically (see the lines above for what each one needs — unresolvable targets must be corrected in the files; role assignments need Owner/User Access Administrator rights)",
            failures.len()
        )
    } else {
        println!();
        bail!(
            "{} identity problem(s) found — rerun with --fix (requires rights to assign roles) or run the printed az commands",
            failures.len()
        )
    }
}

#[allow(unused)]
fn _keep(_: &Edge) {}
