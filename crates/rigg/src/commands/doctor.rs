//! `rigg auth doctor [--fix]` — verify (and repair) the service-to-service
//! identity graph the workspace requires (spec §8.2).

use anyhow::{Context, Result, bail};
use colored::Colorize;

use rigg_client::arm::ArmClient;
use rigg_client::arm_resources::resolve_account_scope;
use rigg_core::identity::{EdgeKind, IdentityEdge, Principal, identity_edges};

use crate::commands::{GlobalContext, load_workspace, resolve_env};

const SEARCH_ARM_API: &str = "2023-11-01";
const COGNITIVE_ARM_API: &str = rigg_core::registry::ARM_COGNITIVE_API_VERSION;

pub async fn run(ctx: &GlobalContext, fix: bool) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let edges = identity_edges(&ws);
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
    let search_conn = env.env.search.as_slice().first();
    let foundry_conn = env.env.foundry.as_slice().first();

    let mut search_identity = None;
    let mut search_service_id = None;
    if let Some(conn) = search_conn {
        let id = arm.find_search_service_id(&conn.service).await?;
        search_identity = arm.get_resource_identity(&id, SEARCH_ARM_API).await?;
        search_service_id = Some(id);
    }
    let mut foundry_identity = None;
    let mut foundry_account_id = None;
    let mut foundry_project_id = None;
    if let Some(conn) = foundry_conn {
        let scope = resolve_account_scope(&arm, &conn.account).await?;
        let account_id = format!(
            "/subscriptions/{}/resourceGroups/{}/providers/Microsoft.CognitiveServices/accounts/{}",
            scope.subscription_id, scope.resource_group, scope.account
        );
        let project_id = format!("{account_id}/projects/{}", conn.project);
        foundry_identity = arm
            .get_resource_identity(&project_id, COGNITIVE_ARM_API)
            .await
            .ok()
            .flatten();
        foundry_account_id = Some(account_id);
        foundry_project_id = Some(project_id);
    }

    let mut failures: Vec<String> = Vec::new();
    let mut report = Vec::new();

    for edge in &edges {
        // Resolve principal + scope for this edge.
        let (identity, principal_desc, identity_resource, identity_api) = match edge.principal {
            Principal::SearchService => (
                search_identity.as_ref(),
                search_conn
                    .map(|c| format!("search service '{}'", c.service))
                    .unwrap_or_else(|| "search service (no connection configured)".into()),
                search_service_id.clone(),
                SEARCH_ARM_API,
            ),
            Principal::FoundryProject => (
                foundry_identity.as_ref(),
                foundry_conn
                    .map(|c| format!("foundry project '{}/{}'", c.account, c.project))
                    .unwrap_or_else(|| "foundry project (no connection configured)".into()),
                foundry_project_id.clone(),
                COGNITIVE_ARM_API,
            ),
        };
        let scope = edge.scope.clone().or_else(|| {
            // Default scopes: model access → foundry account; KB retrieval → search service.
            if edge.target.contains("Foundry account") {
                foundry_account_id.clone()
            } else if edge.target.contains("Search service") {
                search_service_id.clone()
            } else {
                None
            }
        });

        if edge.kind == EdgeKind::Informational {
            println!("  {} {} — {}", "ⓘ".blue(), edge.role_name, edge.reason);
            report.push((edge, "informational".to_string()));
            continue;
        }

        let Some(scope) = scope else {
            println!(
                "  {} {} → {} — cannot resolve target scope (set a real ResourceId= in the file)",
                "?".yellow().bold(),
                principal_desc,
                edge.target
            );
            failures.push(format!("unresolved scope for {}", edge.reason));
            continue;
        };

        // Ensure the principal has an identity.
        let principal_ids: Vec<String> = identity
            .map(|i| i.principal_ids().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default();
        if principal_ids.is_empty() {
            if fix {
                if let Some(resource) = &identity_resource {
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
            if roles.iter().any(|r| r.ends_with(&edge.role_id)) {
                assigned = true;
                break;
            }
        }

        if assigned {
            println!(
                "  {} {} → {} ({})",
                "✓".green().bold(),
                principal_desc,
                edge.target,
                edge.role_name
            );
            report.push((edge, "ok".to_string()));
        } else if fix {
            println!(
                "  {} assigning '{}' to {} at {}...",
                "fix".cyan().bold(),
                edge.role_name,
                principal_desc,
                scope
            );
            arm.create_role_assignment(&scope, &principal_ids[0], &edge.role_id)
                .await?;
            println!("    {} assigned", "✓".green());
            report.push((edge, "fixed".to_string()));
        } else {
            println!(
                "  {} {} lacks '{}' on {}\n      reason: {}\n      fix:    az role assignment create --assignee {} --role \"{}\" --scope \"{}\"",
                "✗".red().bold(),
                principal_desc,
                edge.role_name,
                edge.target,
                edge.reason,
                principal_ids[0],
                edge.role_name,
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

    if !failures.is_empty() && crate::commands::ai_assist::ai_on(ctx) {
        if let Ok(advice) = crate::commands::ai_assist::explain_doctor(&failures).await {
            println!();
            println!("AI advice (ailloy):");
            for line in advice.lines() {
                println!("  {line}");
            }
        }
    }

    if failures.is_empty() {
        println!();
        println!("{} identity wiring looks good", "✓".green().bold());
        Ok(())
    } else {
        println!();
        bail!(
            "{} identity problem(s) found — rerun with --fix (requires rights to assign roles) or run the printed az commands",
            failures.len()
        )
    }
}

#[allow(unused)]
fn _keep(_: &IdentityEdge) {}
