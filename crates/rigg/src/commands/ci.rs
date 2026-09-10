//! `rigg ci init <provider>` — scaffold CI/CD workflows (spec §8.3).
//!
//! The role list printed at the end is not a canned paragraph: it is
//! [`rigg_core::identity::operator_edges`] over this environment's own tree,
//! scoped through the bindings cache, so the CI identity is told about the
//! exact roles and ARM scopes the workflows will actually need — including
//! the grants it must be able to make on a bound storage account. Everything
//! here is offline; a scope the cache cannot resolve is printed as a
//! placeholder rather than guessed at.

use anyhow::{Result, anyhow, bail};
use colored::Colorize;

use rigg_core::binding::{BindingCache, EnvBindings};
use rigg_core::identity::{Edge, EdgeKind, Scope, graph_for_docs, operator_edges};
use rigg_core::resources::ResourceKind;
use rigg_core::service::ServiceDomain;
use rigg_core::workspace::{ResolvedEnv, Workspace};

use crate::cli::CiCommands;
use crate::commands::auth_engine;
use crate::commands::{CommandError, GlobalContext, load_workspace, resolve_env};
use crate::say;

const VALIDATE_YML: &str = include_str!("ci_templates/rigg-validate.yml");
const DEPLOY_YML: &str = include_str!("ci_templates/rigg-deploy.yml");
const DRIFT_YML: &str = include_str!("ci_templates/rigg-drift.yml");

pub async fn run(ctx: &GlobalContext, cmd: CiCommands) -> Result<()> {
    match cmd {
        CiCommands::Init { provider, force } => init(ctx, &provider, force).await,
    }
}

/// One line of the printed role list.
pub struct RoleLine {
    pub role: String,
    /// The role definition GUID — what `az role assignment create --role`
    /// must be given. A role *name* resolves against the tenant's role
    /// definitions, and Microsoft is renaming the Foundry roles, so the id
    /// is what the printed list leads with.
    pub role_id: String,
    pub scope: String,
    /// True for a role the CI identity must be able to *grant* to a service
    /// identity, rather than hold itself.
    pub grant: bool,
}

impl RoleLine {
    /// `<guid>  # <display name>` — the form a caller can paste into
    /// `--role`.
    pub fn display(&self) -> String {
        format!("{}  # {}", self.role_id, self.role)
    }
}

/// How a scope reads in the printed list: the ARM id when the bindings cache
/// resolved it, otherwise a pointer at the command that would resolve it.
fn render_scope(scope: &Scope, env: &str) -> String {
    match scope {
        Scope::Resolved(id) => id.clone(),
        Scope::Unresolved { .. } => {
            format!(
                "{} — resolve with `rigg env bind {env} --learn`",
                scope.describe()
            )
        }
    }
}

/// The kinds the workflows will push. The environment's own tree when it has
/// one; otherwise every kind the environment has a target for, because the
/// deploy workflow pushes whatever lands in the repository later.
fn kinds_for(
    env: &ResolvedEnv,
    docs: &[(ResourceKind, String, serde_json::Value)],
) -> Vec<ResourceKind> {
    if !docs.is_empty() {
        let mut kinds: Vec<ResourceKind> = docs.iter().map(|(k, ..)| *k).collect();
        kinds.sort_by_key(|k| k.directory_name());
        kinds.dedup();
        return kinds;
    }
    ResourceKind::all()
        .iter()
        .copied()
        .filter(|k| match k.domain() {
            ServiceDomain::Search => env.env.search.is_some(),
            ServiceDomain::Foundry => env.env.foundry.is_some(),
        })
        .collect()
}

/// The roles a CI identity needs for this environment, derived from the
/// identity graph rather than hard-coded.
pub fn role_lines(ws: &Workspace, env: &ResolvedEnv) -> Vec<RoleLine> {
    let cache = BindingCache::load(ws, &env.name);
    let bindings = EnvBindings::of_env(&env.name, &env.env, Some(&cache));
    let docs = auth_engine::env_documents(ws, &env.name);
    let kinds = kinds_for(env, &docs);
    let graph = graph_for_docs(&bindings, &docs);

    // Offline sibling of `auth_engine::foundry_project_id`: Foundry User is
    // scoped at `<account>/projects/<project>`, and the cache carries the
    // account id.
    let project_id = env.env.foundry.as_ref().and_then(|f| {
        bindings
            .get("foundry")
            .and_then(|e| e.resolved.as_ref().and_then(|r| r.arm_id.clone()))
            .map(|account| format!("{account}/projects/{}", f.project))
    });

    let grants: Vec<&Edge> = graph
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Rbac)
        .collect();
    // `verify: false` — the scaffolded workflows run validate / push / diff,
    // none of which read the data plane. Add Search Index Data Reader if you
    // add `--verify` to the deploy job.
    let (operator, _) = operator_edges(&bindings, &kinds, false, &grants, project_id.as_deref());

    let mut lines: Vec<RoleLine> = operator
        .iter()
        .map(|e| RoleLine {
            role: e.role.name.to_string(),
            role_id: e.role.id.to_string(),
            scope: render_scope(&e.scope, &env.name),
            grant: false,
        })
        .collect();
    for edge in grants {
        let line = RoleLine {
            role: edge.role.name.to_string(),
            role_id: edge.role.id.to_string(),
            scope: render_scope(&edge.scope, &env.name),
            grant: true,
        };
        if !lines
            .iter()
            .any(|l| l.role == line.role && l.scope == line.scope && l.grant)
        {
            lines.push(line);
        }
    }
    lines
}

async fn init(ctx: &GlobalContext, provider: &str, force: bool) -> Result<()> {
    if provider != "github" {
        return Err(anyhow!(CommandError::Usage(format!(
            "unsupported CI provider '{provider}' (supported: github; Azure DevOps templates are documented in docs/)"
        ))));
    }
    let ws = load_workspace()?;
    let resolved = resolve_env(&ws, ctx).ok();
    let env = resolved
        .as_ref()
        .map(|e| e.name.clone())
        .or_else(|| ws.default_env_name().map(str::to_string))
        .unwrap_or_else(|| "dev".to_string());
    let dir = ws.root.join(".github").join("workflows");
    std::fs::create_dir_all(&dir)?;

    let files = [
        ("rigg-validate.yml", VALIDATE_YML),
        ("rigg-deploy.yml", DEPLOY_YML),
        ("rigg-drift.yml", DRIFT_YML),
    ];
    for (name, template) in files {
        let path = dir.join(name);
        if path.exists() && !force {
            bail!(
                "{} already exists — pass --force to overwrite",
                path.display()
            );
        }
        std::fs::write(&path, template.replace("{{RIGG_ENV}}", &env))?;
        say!(ctx, "  created {}", path.display());
    }

    say!(ctx);
    say!(
        ctx,
        "{} GitHub workflows created for environment '{env}'. To finish setup:",
        "✓".green().bold()
    );
    say!(
        ctx,
        "  1. Create an Entra app registration with federated credentials for this repo"
    );
    say!(
        ctx,
        "     (workload identity federation — no client secrets):"
    );
    say!(ctx, "       az ad app create --display-name rigg-deploy");
    say!(
        ctx,
        "       az ad app federated-credential create ... (subject: repo:<owner>/<repo>:ref:refs/heads/main)"
    );

    match resolved.as_ref() {
        Some(env_ref) => {
            let lines = role_lines(&ws, env_ref);
            let (held, granted): (Vec<&RoleLine>, Vec<&RoleLine>) =
                lines.iter().partition(|l| !l.grant);
            say!(
                ctx,
                "  2. Grant it what '{env}' actually requires — from this workspace's files:"
            );
            if held.is_empty() {
                say!(
                    ctx,
                    "       (no resources yet — re-run after your first push)"
                );
            }
            for line in &held {
                say!(ctx, "       {}", line.display().bold());
                say!(ctx, "         {}", line.scope.dimmed());
            }
            if !granted.is_empty() {
                say!(
                    ctx,
                    "     …and, so a push can grant the service identities their own roles,"
                );
                say!(
                    ctx,
                    "     Microsoft.Authorization/roleAssignments/write at each scope below"
                );
                say!(ctx, "     (with the role rigg would grant there):");
                for line in &granted {
                    say!(ctx, "       {}", line.display().bold());
                    say!(ctx, "         {}", line.scope.dimmed());
                }
                say!(
                    ctx,
                    "     (or pre-grant them yourself once: rigg auth doctor -e {env} --fix,"
                );
                say!(
                    ctx,
                    "      and add --skip-auth-preflight to the deploy job's push)"
                );
            }
            say!(
                ctx,
                "     az role assignment create --assignee <AZURE_CLIENT_ID> --role <role-guid> --scope \"<scope>\""
            );
            say!(
                ctx,
                "     Verify: rigg auth doctor -e {env} --principal <the CI identity's object id>"
            );
        }
        None => {
            say!(
                ctx,
                "  2. Grant it the roles this workspace requires — run `rigg auth doctor -e {env}`"
            );
            say!(
                ctx,
                "     once the environment exists to see them with their exact scopes."
            );
        }
    }
    say!(
        ctx,
        "  3. Add repository variables: AZURE_CLIENT_ID, AZURE_TENANT_ID, AZURE_SUBSCRIPTION_ID."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_are_valid_yaml_with_placeholder() {
        for (name, t) in [
            ("validate", VALIDATE_YML),
            ("deploy", DEPLOY_YML),
            ("drift", DRIFT_YML),
        ] {
            assert!(t.contains("{{RIGG_ENV}}"), "{name} missing env placeholder");
            let replaced = t.replace("{{RIGG_ENV}}", "prod");
            let parsed: std::result::Result<serde_yaml::Value, _> = serde_yaml::from_str(&replaced);
            assert!(parsed.is_ok(), "{name} is not valid YAML: {parsed:?}");
        }
    }
}
