//! `rigg auth easy-auth <function-app binding>` — enable Microsoft Entra
//! authentication on a bound function app so Web API skills can be keyless
//! (spec `2026-09-09-identity-and-auth-design.md` §5).
//!
//! The positional argument is a **binding name** from the environment's
//! `dependencies`, not a site name: every scope rigg acts on comes from a
//! resolved binding, so the site's ARM id, its default hostname and the
//! search identity that must be admitted are all derived rather than typed.
//!
//! Nothing is mutated before the merged `authsettingsV2` document has been
//! shown as a diff and confirmed (`auth.easyauth.<site>`); non-interactively
//! that confirmation must arrive as `--yes` or `--answer`, else exit 6.

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::{Value, json};

use rigg_client::arm::ArmClient;
use rigg_client::graph::GraphClient;
use rigg_core::binding::{BindingType, EnvBindings};
use rigg_core::registry::{self, X_RIGG_AUTH};
use rigg_core::resources::ResourceKind;
use rigg_core::store::Store;
use rigg_core::workspace::{ResolvedEnv, Workspace};

use crate::commands::ask::Question;
use crate::commands::{
    CommandError, GlobalContext, auth_engine, credentials, load_workspace, resolve_env,
};
use crate::say;

/// What the wiring did, for the caller's summary.
pub struct Wiring {
    /// The function app's ARM resource name.
    pub site: String,
    /// The identifier URI a Web API skill now asks tokens for.
    pub audience: String,
    /// Workspace-relative paths of the skillset files that were rewritten.
    pub touched: Vec<String>,
    /// False when the user declined the confirmation — nothing was changed.
    pub applied: bool,
}

/// `rigg auth easy-auth <binding> [--client-id <id>]`.
pub async fn run(ctx: &GlobalContext, binding: String, client_id: Option<String>) -> Result<()> {
    let ws = load_workspace()?;
    let env = resolve_env(&ws, ctx)?;
    let wiring = wire(ctx, &ws, &env, &binding, client_id.as_deref()).await?;
    if !wiring.applied {
        say!(ctx, "no changes made");
        return Ok(());
    }
    say!(ctx);
    say!(
        ctx,
        "{} Entra authentication is on for '{}' — audience {}",
        "✓".green().bold(),
        wiring.site,
        wiring.audience
    );
    if wiring.touched.is_empty() {
        say!(
            ctx,
            "  no skillset in '{}' calls this app yet; set \"authResourceId\": \"{}\" on the \
             WebApiSkill when you add one",
            env.name,
            wiring.audience
        );
    } else {
        for file in &wiring.touched {
            say!(ctx, "  updated {file}");
        }
        say!(
            ctx,
            "  run `rigg push` to send the keyless skillset(s) to Azure"
        );
    }
    Ok(())
}

/// Do the wiring: Graph application + service principal, the merged
/// `authsettingsV2` PUT, the optional app-role assignment, and the local
/// skillset rewrite. Shared with push's Web API auth resolution
/// ("identity-based — set it up now").
pub async fn wire(
    ctx: &GlobalContext,
    ws: &Workspace,
    env: &ResolvedEnv,
    binding: &str,
    client_id: Option<&str>,
) -> Result<Wiring> {
    // 1. The binding must be a declared function-app dependency: rigg acts on
    //    resolved scopes, never on a name typed at the command line.
    let declared = env.env.dependencies.get(binding).ok_or_else(|| {
        anyhow!(CommandError::Validation(format!(
            "environment '{}' has no dependency named '{binding}' — declare the function app \
             first: `rigg env bind {} {binding} function-app:<app-name>`",
            env.name, env.name
        )))
    })?;
    if declared.kind != BindingType::FunctionApp {
        return Err(anyhow!(CommandError::Validation(format!(
            "'{binding}' is a {} binding, not a function-app binding — easy-auth wires Entra \
             authentication onto a function app",
            declared.kind
        ))));
    }

    let tenant = env.env.tenant.clone().ok_or_else(|| {
        anyhow!(CommandError::Validation(format!(
            "environment '{}' declares no `tenant:` — Easy Auth's OpenID issuer \
             (https://login.microsoftonline.com/<tenant>/v2.0) needs it; add it to rigg.yaml",
            env.name
        )))
    })?;

    let arm = ArmClient::for_tenant(Some(&tenant))
        .map_err(|e| anyhow!(CommandError::AuthDenied(format!("{e}"))))?;
    let bindings = auth_engine::bindings_for(ws, env, Some(&arm)).await;
    let site_id = resolved_arm_id(&bindings, binding).ok_or_else(|| {
        anyhow!(CommandError::AuthDenied(format!(
            "binding '{binding}' does not resolve to a function app in Azure — check the value \
             in rigg.yaml, then `rigg env show {} --refresh` (or `rigg env bind {} --learn`)",
            env.name, env.name
        )))
    })?;
    let site = rigg_core::binding::arm_resource_name(&site_id)
        .unwrap_or(binding)
        .to_string();
    let hostname = match arm.site_default_hostname(&site_id).await {
        Ok(host) => host,
        Err(e) => {
            say!(ctx, "  (could not read the app's default hostname: {e})");
            format!("{site}.azurewebsites.net")
        }
    };

    // 2. Which local skillsets call this app, and through which identity.
    let mut targets = webapi_targets(ws, &env.name, &hostname)?;

    // 3. The caller Easy Auth must admit: the skillset's user-assigned
    //    identity when it declares one (spec §7), else the search service's
    //    system-assigned identity.
    let caller = caller_identity(&arm, &tenant, &bindings, &targets).await?;

    // 4. Current settings, planned merge, diff, confirmation.
    let current = arm.site_auth_settings(&site_id).await?;
    let planned_client_id = client_id.unwrap_or(NEW_APP_PLACEHOLDER);
    let planned = merge_auth_settings(
        &current,
        planned_client_id,
        &tenant,
        &format!("api://{planned_client_id}"),
        &caller.client_id,
    );
    say!(ctx);
    say!(
        ctx,
        "Easy Auth on '{site}' ({}) — planned authsettingsV2:",
        env.name
    );
    let diff = rigg_diff::semantic::diff(&planned, &current, "name");
    for line in rigg_diff::output::format_text(
        &diff,
        "authsettingsV2",
        &rigg_diff::output::SideLabels {
            new_side: "planned".to_string(),
            old_side: "current".to_string(),
        },
    )
    .lines()
    {
        say!(ctx, "  {line}");
    }
    say!(
        ctx,
        "  the search identity admitted is {} ({})",
        caller.client_id,
        caller.label
    );
    if !targets.is_empty() {
        say!(ctx, "  skillset files that become keyless:");
        for t in &targets {
            say!(ctx, "    {}", t.display);
        }
    }
    if !confirm(ctx, env, &site)? {
        return Ok(Wiring {
            site,
            audience: format!("api://{planned_client_id}"),
            touched: Vec::new(),
            applied: false,
        });
    }

    // 5. Graph: the application (reused when --client-id names one), its
    //    identifier URI and app role, and the enterprise application.
    let graph = GraphClient::for_tenant(Some(&tenant))
        .map_err(|e| anyhow!(CommandError::AuthDenied(format!("{e}"))))?;
    let app = match client_id {
        Some(id) => graph.application_by_app_id(id).await?.ok_or_else(|| {
            anyhow!(CommandError::Validation(format!(
                "no application registration with client id '{id}' in tenant '{tenant}'"
            )))
        })?,
        None => graph.create_application(&format!("rigg-{site}")).await?,
    };
    let audience = format!("api://{}", app.app_id);
    let role_id = graph
        .set_identifier_uri_and_role(&app.id, &audience)
        .await?;
    let sp = graph.ensure_service_principal(&app.app_id).await?;

    // 6. The function app itself — merged over the document read above, so
    //    every other identity provider and unrelated setting survives.
    let settings =
        merge_auth_settings(&current, &app.app_id, &tenant, &audience, &caller.client_id);
    arm.put_site_auth_settings(&site_id, &settings).await?;

    // 7. A gated enterprise application issues no token without an
    //    assignment, however correct the audience is.
    if sp.app_role_assignment_required {
        graph
            .assign_app_role(&sp.id, &caller.object_id, &role_id)
            .await?;
        say!(
            ctx,
            "  granted the '{}' identity the app role (assignment is required on this app)",
            caller.label
        );
    }

    // 8. The skill files: keyless from here, and the key carriers go with
    //    the key. Pushing them is the user's call, never a side effect.
    let mut touched = Vec::new();
    for target in &mut targets {
        for idx in &target.skills {
            let skill = &mut target.doc["skills"][*idx];
            skill["authResourceId"] = Value::String(audience.clone());
            skill["uri"] = Value::String(credentials::strip_code_param(
                skill.get("uri").and_then(Value::as_str).unwrap_or_default(),
            ));
            credentials::remove_function_key_header(skill);
            if let Some(map) = skill.as_object_mut()
                && map
                    .get(X_RIGG_AUTH)
                    .and_then(Value::as_str)
                    .is_some_and(registry::is_known_auth_annotation)
            {
                map.remove(X_RIGG_AUTH);
            }
        }
        let project = ws.project(&target.project)?;
        Store::new(project, &env.name).write_exact(&target.r, &target.doc)?;
        touched.push(target.display.clone());
    }

    Ok(Wiring {
        site,
        audience,
        touched,
        applied: true,
    })
}

/// The `registration.clientId` shown in the plan when rigg has not created
/// the application yet — the one field the diff cannot know in advance.
const NEW_APP_PLACEHOLDER: &str = "<app registration rigg will create>";

fn confirm(ctx: &GlobalContext, env: &ResolvedEnv, site: &str) -> Result<bool> {
    if ctx.yes {
        return Ok(true);
    }
    let mut asker = ctx.asker("auth easy-auth", json!({"env": env.name, "site": site}));
    Ok(asker
        .ask(&Question::confirm(
            format!("auth.easyauth.{site}"),
            format!("Enable Microsoft Entra authentication on '{site}'?"),
            true,
        ))?
        .as_bool()
        == Some(true))
}

fn resolved_arm_id(bindings: &EnvBindings, name: &str) -> Option<String> {
    bindings
        .get(name)
        .and_then(|e| e.resolved.as_ref())
        .and_then(|r| r.arm_id.clone())
}

/// The managed identity Easy Auth must admit.
struct Caller {
    /// Directory object id — what an app-role assignment is made for.
    object_id: String,
    /// Application (client) id — what `allowedApplications` names.
    client_id: String,
    label: String,
}

async fn caller_identity(
    arm: &ArmClient,
    tenant: &str,
    bindings: &EnvBindings,
    targets: &[SkillsetTarget],
) -> Result<Caller> {
    // A skillset that declares a user-assigned identity calls through THAT
    // identity; admitting the system one would authorize nothing (spec §7).
    if let Some(uami) = targets.iter().find_map(|t| t.uami.clone()) {
        let (object_id, client_id) = arm.managed_identity_ids(&uami).await?;
        return Ok(Caller {
            object_id,
            client_id,
            label: rigg_core::binding::arm_resource_name(&uami)
                .unwrap_or("user-assigned identity")
                .to_string(),
        });
    }
    let search_id = resolved_arm_id(bindings, "search").ok_or_else(|| {
        anyhow!(CommandError::AuthDenied(
            "the environment's search service does not resolve in Azure — `rigg env show \
             --refresh`"
                .to_string()
        ))
    })?;
    let info = arm.get_search_service(&search_id).await?;
    let object_id = info.identity.principal_id.clone().ok_or_else(|| {
        anyhow!(CommandError::Validation(format!(
            "search service '{}' has no system-assigned identity — enable it first \
             (`rigg auth doctor --fix`)",
            info.name
        )))
    })?;
    // ARM reports a system-assigned identity's principal (object) id; only
    // the directory knows the client id Easy Auth wants.
    let graph = GraphClient::for_tenant(Some(tenant))
        .map_err(|e| anyhow!(CommandError::AuthDenied(format!("{e}"))))?;
    let client_id = graph.service_principal_app_id(&object_id).await?;
    Ok(Caller {
        object_id,
        client_id,
        label: format!("{} (system-assigned)", info.name),
    })
}

/// One skillset file with Web API skills pointed at the site being wired.
struct SkillsetTarget {
    project: String,
    r: rigg_core::resources::ResourceRef,
    display: String,
    doc: Value,
    /// Indices into `skills` of the WebApiSkills calling this host.
    skills: Vec<usize>,
    /// The user-assigned identity one of those skills authenticates with.
    uami: Option<String>,
}

/// Every skillset in `env` whose WebApiSkill `uri` host is `hostname`.
fn webapi_targets(ws: &Workspace, env: &str, hostname: &str) -> Result<Vec<SkillsetTarget>> {
    let mut out = Vec::new();
    for project in &ws.projects {
        if !Store::envs_of(project).contains(&env.to_string()) {
            continue;
        }
        let store = Store::new(project, env);
        for (r, path) in store.list()? {
            if r.kind != ResourceKind::Skillset {
                continue;
            }
            let doc = store.read(&r)?;
            let Some(skills) = doc.get("skills").and_then(Value::as_array) else {
                continue;
            };
            let mut indices = Vec::new();
            let mut uami = None;
            for (i, skill) in skills.iter().enumerate() {
                let is_webapi = skill
                    .get("@odata.type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.ends_with("WebApiSkill"));
                if !is_webapi {
                    continue;
                }
                let uri = skill.get("uri").and_then(Value::as_str).unwrap_or_default();
                if uri_host(uri).is_none_or(|h| !h.eq_ignore_ascii_case(hostname)) {
                    continue;
                }
                indices.push(i);
                uami = uami.or_else(|| {
                    skill
                        .pointer("/authIdentity/userAssignedIdentity")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                });
            }
            if indices.is_empty() {
                continue;
            }
            out.push(SkillsetTarget {
                project: project.name.clone(),
                r,
                display: path
                    .strip_prefix(&ws.root)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
                doc,
                skills: indices,
                uami,
            });
        }
    }
    Ok(out)
}

/// The host of an `http(s)://host/…` URI.
fn uri_host(uri: &str) -> Option<&str> {
    let rest = uri
        .strip_prefix("https://")
        .or_else(|| uri.strip_prefix("http://"))?;
    let host = rest.split(['/', '?']).next().filter(|h| !h.is_empty())?;
    Some(host)
}

/// Merge rigg's Entra settings into a site's current `authsettingsV2`
/// document (spec §5.3).
///
/// The PUT replaces the document, so everything the app already has is
/// carried over verbatim: other identity providers, login settings, HTTP
/// settings. Only the fields below are set, and the two list fields are
/// UNIONED — an app that already accepts another audience or admits another
/// caller keeps doing so.
pub fn merge_auth_settings(
    current: &Value,
    client_id: &str,
    tenant: &str,
    audience: &str,
    allowed_application: &str,
) -> Value {
    let mut merged = current.clone();
    if !merged.is_object() {
        merged = json!({});
    }
    ensure_object(&mut merged, "properties");
    let props = &mut merged["properties"];

    ensure_object(props, "platform");
    props["platform"]["enabled"] = json!(true);

    ensure_object(props, "globalValidation");
    props["globalValidation"]["requireAuthentication"] = json!(true);
    props["globalValidation"]["unauthenticatedClientAction"] = json!("Return401");

    ensure_object(props, "identityProviders");
    ensure_object(&mut props["identityProviders"], "azureActiveDirectory");
    let aad = &mut props["identityProviders"]["azureActiveDirectory"];
    aad["enabled"] = json!(true);

    ensure_object(aad, "registration");
    aad["registration"]["clientId"] = json!(client_id);
    aad["registration"]["openIdIssuer"] =
        json!(format!("https://login.microsoftonline.com/{tenant}/v2.0"));

    ensure_object(aad, "validation");
    let audiences = union_with(aad.pointer("/validation/allowedAudiences"), audience);
    aad["validation"]["allowedAudiences"] = audiences;
    ensure_object(&mut aad["validation"], "defaultAuthorizationPolicy");
    let allowed = union_with(
        aad.pointer("/validation/defaultAuthorizationPolicy/allowedApplications"),
        allowed_application,
    );
    aad["validation"]["defaultAuthorizationPolicy"]["allowedApplications"] = allowed;

    merged
}

/// Make `value[key]` an object, keeping it when it already is one.
fn ensure_object(value: &mut Value, key: &str) {
    if !value.get(key).is_some_and(Value::is_object) {
        value[key] = json!({});
    }
}

/// `existing ∪ {entry}`, order-preserving — never a replacement.
fn union_with(existing: Option<&Value>, entry: &str) -> Value {
    let mut items: Vec<String> = existing
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    if !items.iter().any(|i| i == entry) {
        items.push(entry.to_string());
    }
    json!(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> &'static str {
        "11111111-2222-3333-4444-555555555555"
    }

    #[test]
    fn merge_sets_every_field_the_spec_requires_on_an_empty_document() {
        let merged = merge_auth_settings(&json!({}), "app-1", tenant(), "api://app-1", "mi-1");
        let props = &merged["properties"];
        assert_eq!(props["platform"]["enabled"], json!(true));
        assert_eq!(
            props["globalValidation"]["requireAuthentication"],
            json!(true)
        );
        assert_eq!(
            props["globalValidation"]["unauthenticatedClientAction"],
            json!("Return401")
        );
        let aad = &props["identityProviders"]["azureActiveDirectory"];
        assert_eq!(aad["enabled"], json!(true));
        assert_eq!(aad["registration"]["clientId"], json!("app-1"));
        assert_eq!(
            aad["registration"]["openIdIssuer"],
            json!(format!(
                "https://login.microsoftonline.com/{}/v2.0",
                tenant()
            ))
        );
        assert_eq!(
            aad["validation"]["allowedAudiences"],
            json!(["api://app-1"])
        );
        assert_eq!(
            aad["validation"]["defaultAuthorizationPolicy"]["allowedApplications"],
            json!(["mi-1"])
        );
    }

    #[test]
    fn merge_keeps_other_providers_and_unrelated_fields_verbatim() {
        let current = json!({
            "properties": {
                "platform": {"enabled": false, "runtimeVersion": "~1"},
                "identityProviders": {
                    "google": {"enabled": true, "registration": {"clientId": "g"}},
                    "azureActiveDirectory": {"isAutoProvisioned": true}
                },
                "login": {"tokenStore": {"enabled": true}},
                "httpSettings": {"requireHttps": true}
            }
        });
        let merged = merge_auth_settings(&current, "app-1", tenant(), "api://app-1", "mi-1");
        let props = &merged["properties"];
        assert_eq!(props["platform"]["runtimeVersion"], json!("~1"));
        assert_eq!(props["platform"]["enabled"], json!(true), "switched on");
        assert_eq!(
            props["identityProviders"]["google"]["registration"]["clientId"],
            json!("g")
        );
        assert_eq!(
            props["identityProviders"]["azureActiveDirectory"]["isAutoProvisioned"],
            json!(true)
        );
        assert_eq!(props["login"]["tokenStore"]["enabled"], json!(true));
        assert_eq!(props["httpSettings"]["requireHttps"], json!(true));
    }

    #[test]
    fn merge_unions_audiences_and_allowed_applications_never_replaces_them() {
        let current = json!({"properties": {"identityProviders": {"azureActiveDirectory": {
            "validation": {
                "allowedAudiences": ["api://legacy"],
                "defaultAuthorizationPolicy": {
                    "allowedApplications": ["other-mi"],
                    "allowedPrincipals": {"identities": ["keep-me"]}
                }
            }
        }}}});
        let merged = merge_auth_settings(&current, "app-1", tenant(), "api://app-1", "mi-1");
        let validation =
            &merged["properties"]["identityProviders"]["azureActiveDirectory"]["validation"];
        assert_eq!(
            validation["allowedAudiences"],
            json!(["api://legacy", "api://app-1"])
        );
        assert_eq!(
            validation["defaultAuthorizationPolicy"]["allowedApplications"],
            json!(["other-mi", "mi-1"])
        );
        assert_eq!(
            validation["defaultAuthorizationPolicy"]["allowedPrincipals"]["identities"],
            json!(["keep-me"])
        );
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_auth_settings(&json!({}), "app-1", tenant(), "api://app-1", "mi-1");
        let twice = merge_auth_settings(&once, "app-1", tenant(), "api://app-1", "mi-1");
        assert_eq!(once, twice);
    }

    #[test]
    fn uri_host_reads_the_host_only() {
        assert_eq!(
            uri_host("https://fn.azurewebsites.net/api/enrich?code=x"),
            Some("fn.azurewebsites.net")
        );
        assert_eq!(
            uri_host("https://fn.azurewebsites.net"),
            Some("fn.azurewebsites.net")
        );
        assert_eq!(uri_host("not a uri"), None);
    }
}
