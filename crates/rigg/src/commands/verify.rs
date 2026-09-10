//! `rigg verify` and `rigg push --verify` (spec §4.2.4) — prove the pushed
//! stack actually works instead of trusting a 200 from the control plane.
//!
//! For every resource in the project's environment tree that can be
//! exercised: run each indexer to completion, retrieve from each knowledge
//! base, ask each agent one turn. A failure whose message looks like an
//! authorization problem is attributed to the identity edge that would
//! explain it — the same graph `rigg auth doctor` verifies, built from the
//! cached binding table so verification never depends on ARM access.

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::{Value, json};

use rigg_core::identity::{Scope, graph_for_docs};
use rigg_core::resources::ResourceKind;
use rigg_core::store::Store;
use rigg_core::workspace::{Project, ResolvedEnv, Workspace};

use crate::cli::VerifyArgs;
use crate::commands::auth_engine;
use crate::commands::az::indexer::run_and_watch;
use crate::commands::remote::Remote;
use crate::commands::{
    GlobalContext, confirm_protected_env, load_workspace, resolve_env, select_projects,
};
use crate::say;

/// The prompt every smoke test sends: short, cheap, and obviously synthetic
/// so it is recognizable in a service's own logs.
const PING: &str = "ping";
const AGENT_PROMPT: &str = "Reply with OK";

pub async fn run(ctx: &GlobalContext, args: VerifyArgs) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let projects = select_projects(&ws, args.project.as_deref(), args.all)?;
    // Verification triggers real indexer runs (ingestion, skill and embedding
    // costs), so a protected environment asks first — the same gate
    // `rigg az indexer run` applies. `push --verify` does not come through
    // here: the push's own gate already covered this environment.
    if !confirm_protected_env(
        ctx,
        &env,
        None,
        "verify (runs every indexer)",
        "verify",
        json!({"env": env.name}),
    )? {
        say!(ctx, "Aborted.");
        return Ok(());
    }
    run_for(ctx, &ws, &env, &projects).await
}

/// Verify `projects` in `env`. Shared with `rigg push --verify`, which calls
/// it once the whole push has landed.
pub async fn run_for(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    projects: &[&Project],
) -> Result<()> {
    let mut failures: Vec<String> = Vec::new();
    let mut checked = 0usize;

    for project in projects {
        let store = Store::new(project, &env.name);
        let remote = Remote::for_project(env, project);

        let mut docs: Vec<(ResourceKind, String, Value)> = Vec::new();
        let (mut indexers, mut kbs, mut agents) = (Vec::new(), Vec::new(), Vec::new());
        for (r, _) in store.list()? {
            let body = store.read(&r)?;
            match r.kind {
                ResourceKind::Indexer => indexers.push(r.name.clone()),
                ResourceKind::KnowledgeBase => kbs.push(r.name.clone()),
                ResourceKind::Agent => agents.push(r.name.clone()),
                _ => {}
            }
            docs.push((r.kind, r.name.clone(), body));
        }
        if indexers.is_empty() && kbs.is_empty() && agents.is_empty() {
            continue;
        }

        say!(
            ctx,
            "{} project '{}' (env: {})",
            "Verify".bold(),
            project.name.bold(),
            env.name
        );
        for line in remote.target_lines() {
            say!(ctx, "{line}");
        }
        let attribution = Attribution::build(ws, env, &docs).await;

        for name in &indexers {
            checked += 1;
            let outcome = match run_and_watch(&remote, name, ctx).await {
                Ok(o) => o,
                Err(e) => {
                    report_failure(
                        ctx,
                        &attribution,
                        &mut failures,
                        "indexer",
                        name,
                        &e_str(&e),
                    );
                    continue;
                }
            };
            if outcome.succeeded() {
                say!(
                    ctx,
                    "  {} indexer '{name}' — {} processed, {} failed",
                    "✓".green(),
                    outcome.items,
                    outcome.items_failed
                );
            } else {
                let detail = outcome
                    .error
                    .unwrap_or_else(|| format!("run ended in {}", outcome.status));
                report_failure(ctx, &attribution, &mut failures, "indexer", name, &detail);
            }
        }

        for name in &kbs {
            checked += 1;
            // The same body `rigg az knowledge-base ask` sends, with a
            // throwaway prompt: a retrieve that answers at all proves the
            // knowledge base can reach its sources.
            let body = json!({
                "intents": [{ "type": "semantic", "search": PING }],
                "includeActivity": true
            });
            match remote.kb_retrieve(name, &body).await {
                Ok(result) => match kb_source_error(&result) {
                    Some(detail) => {
                        report_failure(
                            ctx,
                            &attribution,
                            &mut failures,
                            "knowledge base",
                            name,
                            &detail,
                        );
                    }
                    None => say!(ctx, "  {} knowledge base '{name}' retrieved", "✓".green()),
                },
                Err(e) => report_failure(
                    ctx,
                    &attribution,
                    &mut failures,
                    "knowledge base",
                    name,
                    &e_str(&e),
                ),
            }
        }

        for name in &agents {
            checked += 1;
            match remote.agent_ask(name, AGENT_PROMPT).await {
                Ok(_) => say!(ctx, "  {} agent '{name}' replied", "✓".green()),
                Err(e) => {
                    report_failure(ctx, &attribution, &mut failures, "agent", name, &e_str(&e))
                }
            }
        }
    }

    if checked == 0 {
        say!(
            ctx,
            "nothing to verify (no indexers, knowledge bases or agents in this environment)"
        );
        return Ok(());
    }
    if failures.is_empty() {
        say!(ctx, "{} {checked} check(s) passed", "✓".green().bold());
        return Ok(());
    }
    Err(anyhow!(
        "{} of {checked} verification(s) failed: {}",
        failures.len(),
        failures.join("; ")
    ))
}

fn e_str(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

/// Print one `✗` line, attributing it to an identity edge when the message
/// looks like an authorization failure, and record it for the exit code.
fn report_failure(
    ctx: &GlobalContext,
    attribution: &Attribution,
    failures: &mut Vec<String>,
    kind: &str,
    name: &str,
    detail: &str,
) {
    let likely = attribution.likely(detail);
    say!(ctx, "  {} {kind} '{name}' — {detail}", "✗".red());
    if let Some(edge) = &likely {
        say!(ctx, "      → likely {edge}");
    }
    failures.push(match likely {
        Some(edge) => format!("{kind} '{name}' ({edge})"),
        None => format!("{kind} '{name}'"),
    });
}

/// Did any knowledge source report an error in the retrieve activity? A
/// retrieve returns 200 even when a source underneath it was denied, so the
/// activity records are the only place that failure shows up.
fn kb_source_error(result: &Value) -> Option<String> {
    let records = result.get("activity")?.as_array()?;
    let mut problems = Vec::new();
    for record in records {
        let Some(message) = record.pointer("/error/message").and_then(Value::as_str) else {
            continue;
        };
        let source = record
            .get("knowledgeSourceName")
            .and_then(Value::as_str)
            .unwrap_or("?");
        problems.push(format!("knowledge source '{source}': {message}"));
    }
    (!problems.is_empty()).then(|| problems.join("; "))
}

/// The identity edges this project's documents imply, reduced to the pair a
/// failure message can be matched against: the edge id, and the resource
/// name its scope is about.
struct Attribution {
    edges: Vec<(String, String)>,
}

impl Attribution {
    /// Built from the cached binding table alone (`arm: None`): attribution
    /// is a convenience on top of a failure that already happened, and must
    /// never itself need ARM access or add a round trip.
    async fn build(
        ws: &Workspace,
        env: &ResolvedEnv,
        docs: &[(ResourceKind, String, Value)],
    ) -> Self {
        let bindings = auth_engine::bindings_for(ws, env, None).await;
        let graph = graph_for_docs(&bindings, docs);
        let edges = graph
            .edges
            .iter()
            .filter_map(|e| Some((e.id.clone(), scope_resource(&e.scope)?)))
            .collect();
        Attribution { edges }
    }

    /// The first edge whose scope names a resource the message mentions —
    /// but only for messages that look like an authorization failure, so a
    /// document-parsing error is never blamed on a role.
    fn likely(&self, message: &str) -> Option<String> {
        if !auth_engine::looks_like_auth(message) {
            return None;
        }
        let haystack = message.to_lowercase();
        self.edges
            .iter()
            .find(|(_, resource)| haystack.contains(&resource.to_lowercase()))
            .map(|(id, _)| id.clone())
    }
}

/// The resource name a scope is about: the last segment of its ARM id, or
/// the physical name when the binding never resolved.
fn scope_resource(scope: &Scope) -> Option<String> {
    let name = match scope {
        Scope::Resolved(id) => id.rsplit('/').next().unwrap_or(id).to_string(),
        Scope::Unresolved { physical, .. } => physical.clone(),
    };
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_only_fires_for_auth_shaped_messages() {
        let a = Attribution {
            edges: vec![("search-system|abc|acct".to_string(), "acct".to_string())],
        };
        assert_eq!(
            a.likely("This request is not authorized. Storage account 'acct'"),
            Some("search-system|abc|acct".to_string())
        );
        // Names the resource, but is not an auth failure.
        assert_eq!(a.likely("could not parse document in 'acct'"), None);
        // Auth-shaped, but names no scope in the graph.
        assert_eq!(a.likely("403 Forbidden"), None);
    }

    #[test]
    fn kb_source_errors_are_surfaced_from_the_activity_records() {
        let result = json!({"activity": [
            {"knowledgeSourceName": "docs-ks", "error": {"message": "Forbidden (403)"}},
            {"knowledgeSourceName": "other"}
        ]});
        assert_eq!(
            kb_source_error(&result),
            Some("knowledge source 'docs-ks': Forbidden (403)".to_string())
        );
        assert_eq!(kb_source_error(&json!({"activity": []})), None);
        assert_eq!(kb_source_error(&json!({})), None);
    }

    #[test]
    fn scope_resource_reads_the_last_arm_segment() {
        assert_eq!(
            scope_resource(&Scope::Resolved("/subscriptions/s/rg/x/acct".into())),
            Some("acct".to_string())
        );
        assert_eq!(
            scope_resource(&Scope::Unresolved {
                binding: "docs".into(),
                kind: None,
                physical: "acct".into(),
            }),
            Some("acct".to_string())
        );
    }
}
