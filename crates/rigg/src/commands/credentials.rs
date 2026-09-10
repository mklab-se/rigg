//! Identity-based data-source connection helpers.
//!
//! Rigg never stores keys in files; data sources use keyless
//! `ResourceId=<storage account ARM id>;` references and the search
//! service's managed identity. Azure's GET responses never return
//! credentials, so copied/migrated definitions arrive without one — these
//! helpers detect that and, since the user is already logged in via Azure
//! CLI, DISCOVER the right storage account through ARM (by the container
//! the data source reads) instead of asking the user to hand-type an id.

use anyhow::Result;
use colored::Colorize;
use serde_json::Value;

use rigg_client::arm::ArmClient;
use rigg_core::binding::{BindingCache, BindingType};
use rigg_core::workspace::{ResolvedEnv, Workspace};

use crate::commands::interactive;

/// A data source with no usable connection (missing/null/empty
/// `credentials.connectionString`).
pub fn missing_credentials(doc: &Value) -> bool {
    doc.pointer("/credentials/connectionString")
        .and_then(Value::as_str)
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
}

/// The blob container a data source reads from, when declared.
pub fn container_name(doc: &Value) -> Option<&str> {
    doc.pointer("/container/name").and_then(Value::as_str)
}

/// Set `credentials.connectionString` on a data-source document.
pub fn set_connection(doc: &mut Value, connection: &str) {
    doc["credentials"] = serde_json::json!({ "connectionString": connection });
}

/// After an identity-based connection is chosen the search service's
/// managed identity still needs data-plane RBAC — point at the doctor.
pub fn print_rbac_hint(account: &str) {
    println!(
        "  hint: the search service's managed identity needs 'Storage Blob Data Reader' on {account} — run `rigg auth doctor --fix` to verify/grant"
    );
}

/// Interactively resolve an identity-based connection for a data source:
/// discover which storage account(s) hold its container via ARM (Azure CLI
/// login), confirm/select, with a manual entry fallback. Returns the chosen
/// `ResourceId=...;` string, or None when the user skips.
pub async fn discover_connection_interactive(
    ds_display: &str,
    container: Option<&str>,
    plain: bool,
) -> Result<Option<String>> {
    let Some(container) = container else {
        println!(
            "  {} {ds_display} declares no container — cannot auto-discover its storage account",
            "!".yellow()
        );
        return manual_entry(plain);
    };
    println!(
        "  looking up which storage account holds container '{container}' (ARM, via your az login)..."
    );
    let arm = match ArmClient::new() {
        Ok(arm) => arm,
        Err(e) => {
            println!(
                "  {} ARM access unavailable ({e}) — enter the connection manually",
                "!".yellow()
            );
            return manual_entry(plain);
        }
    };
    let matches = match arm.find_storage_accounts_with_container(container).await {
        Ok(m) => m,
        Err(e) => {
            println!("  {} discovery failed ({e})", "!".yellow());
            return manual_entry(plain);
        }
    };
    match matches.as_slice() {
        [] => {
            println!(
                "  {} no storage account with a container '{container}' is visible to your login",
                "!".yellow()
            );
            manual_entry(plain)
        }
        [account] => {
            println!("  found {} — {}", account.name.bold(), account.id);
            if interactive::confirm_default_yes(
                &format!(
                    "Use identity-based access to '{}' for {ds_display}?",
                    account.name
                ),
                plain,
            )? {
                Ok(Some(format!("ResourceId={};", account.id)))
            } else {
                manual_entry(plain)
            }
        }
        many => {
            const MANUAL: &str = "enter manually";
            let mut options: Vec<String> = many
                .iter()
                .map(|a| format!("{} — {}", a.name, a.id))
                .collect();
            options.push(MANUAL.to_string());
            let choice = interactive::select(
                &format!(
                    "Several storage accounts hold a container '{container}' — which one does {ds_display} read?"
                ),
                options,
                plain,
            )?;
            if choice == MANUAL {
                return manual_entry(plain);
            }
            let account = many
                .iter()
                .find(|a| choice.starts_with(&a.name))
                .expect("choice derived from list");
            Ok(Some(format!("ResourceId={};", account.id)))
        }
    }
}

fn manual_entry(plain: bool) -> Result<Option<String>> {
    let entered = interactive::text_with_default(
        "Storage connection (ResourceId=/subscriptions/.../storageAccounts/<name>;) — empty to skip:",
        "",
        plain,
    )?;
    let entered = entered.trim().to_string();
    Ok((!entered.is_empty()).then_some(entered))
}

/// A skillset whose `cognitiveServices` connection is key-based but carries
/// no usable key (Azure never returns keys on GET, so copied definitions
/// arrive with a null or `<redacted>` placeholder). Returns the subdomain
/// URL when one is declared — the ingredient needed for the identity-based
/// rewrite.
pub fn skillset_missing_ai_services_key(doc: &Value) -> Option<Option<String>> {
    let cs = doc.get("cognitiveServices")?.as_object()?;
    let odata = cs.get("@odata.type").and_then(Value::as_str).unwrap_or("");
    if !odata.ends_with("ByKey") {
        return None;
    }
    let key = cs.get("key").and_then(Value::as_str).unwrap_or("");
    if !key.trim().is_empty() && key != "<redacted>" {
        return None; // a real key — validate rejects it elsewhere
    }
    Some(
        cs.get("subdomainUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// Rewrite a skillset's AI services connection to the keyless
/// identity-based form (the search service's system-assigned managed
/// identity authenticates; nothing secret on disk).
pub fn set_ai_services_identity(doc: &mut Value, subdomain_url: &str) {
    doc["cognitiveServices"] = serde_json::json!({
        "@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity",
        "subdomainUrl": subdomain_url.trim_end_matches('/'),
        "identity": null
    });
}

/// Resolve WHERE a skillset's keyless billing should point. Keyless
/// (`AIServicesByIdentity`) billing requires a Foundry resource (ARM kind
/// `AIServices`); legacy `CognitiveServices`-kind accounts only support
/// key-based billing, and an identity connection to one fails with
/// "Unable to connect to AI Services using managed identity". So: keep the
/// current subdomain when its account is Foundry-kind, otherwise discover
/// the Foundry accounts the login can see and offer those. Falls back to
/// the current subdomain (with a warning) when ARM is unavailable.
pub async fn resolve_ai_services_billing_target(
    current_subdomain: &str,
    plain: bool,
) -> Result<Option<String>> {
    let normalized = current_subdomain.trim_end_matches('/').to_string();
    let Some(account_name) = ai_services_account_name(&normalized).map(str::to_string) else {
        return Ok(Some(normalized));
    };
    let Ok(arm) = ArmClient::new() else {
        println!(
            "  {} cannot verify account kind (no ARM access) — keeping '{account_name}'",
            "!".yellow()
        );
        return Ok(Some(normalized));
    };
    match arm.find_cognitive_account(&account_name).await {
        Ok(acct) if acct.kind.eq_ignore_ascii_case("AIServices") => Ok(Some(normalized)),
        Ok(acct) => {
            println!(
                "  {} '{}' is a legacy Cognitive Services account (kind: {}) — keyless \
                 identity-based billing requires a Foundry (AI Services) resource",
                "!".yellow(),
                acct.name,
                acct.kind
            );
            offer_foundry_accounts(&arm, plain).await
        }
        Err(_) => {
            println!(
                "  {} account '{account_name}' not found via ARM — keeping it (verify manually)",
                "!".yellow()
            );
            Ok(Some(normalized))
        }
    }
}

async fn offer_foundry_accounts(arm: &ArmClient, plain: bool) -> Result<Option<String>> {
    let endpoint_of = |a: &rigg_client::arm::AiServicesAccount| -> String {
        a.properties
            .endpoint
            .as_deref()
            .map(|e| e.trim_end_matches('/').to_string())
            .unwrap_or_else(|| format!("https://{}.cognitiveservices.azure.com", a.name))
    };
    let accounts = arm.all_foundry_accounts().await.unwrap_or_default();
    match accounts.as_slice() {
        [] => {
            println!(
                "  {} no Foundry (AI Services) resource visible to your login — create one, or keep key-based billing",
                "!".yellow()
            );
            Ok(None)
        }
        [only] => {
            let endpoint = endpoint_of(only);
            if interactive::confirm_default_yes(
                &format!(
                    "Bill skillset enrichment through '{}' ({endpoint})?",
                    only.name
                ),
                plain,
            )? {
                Ok(Some(endpoint))
            } else {
                Ok(None)
            }
        }
        many => {
            const SKIP: &str = "skip (keep the file as is)";
            let mut options: Vec<String> = many
                .iter()
                .map(|a| format!("{} — {}", a.name, endpoint_of(a)))
                .collect();
            options.push(SKIP.to_string());
            let choice = interactive::select(
                "Which Foundry resource should billing go through?",
                options,
                plain,
            )?;
            if choice == SKIP {
                return Ok(None);
            }
            let acct = many
                .iter()
                .find(|a| choice.starts_with(&a.name))
                .expect("choice derived from list");
            Ok(Some(endpoint_of(acct)))
        }
    }
}

/// The account name in an AI services subdomain URL
/// (`https://<name>.cognitiveservices.azure.com/` → `<name>`).
pub fn ai_services_account_name(subdomain_url: &str) -> Option<&str> {
    subdomain_url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('.').next())
        .filter(|s| !s.is_empty())
}

/// RBAC pointer for the identity-based AI services connection.
pub fn print_ai_services_rbac_hint(account: &str) {
    println!(
        "  hint: the search service's managed identity needs 'Cognitive Services User' on AI services account '{account}' — run `rigg auth doctor --fix` to verify/grant"
    );
}

// ---------------------------------------------------------------------------
// Custom Web API skill authorization
// ---------------------------------------------------------------------------

/// Rigg-local annotation marking a Web API skill whose function key is
/// resolved through ARM at push time (never stored on disk). Kept in the
/// file, stripped before any PUT like every `x-rigg-*` key. Defined once,
/// in the registry — the table every `x-rigg-*` key belongs to.
pub use rigg_core::registry::{
    X_RIGG_AUTH, X_RIGG_AUTH_FUNCTION_KEY, X_RIGG_AUTH_KEY_VAULT_PREFIX, is_known_auth_annotation,
    parse_key_vault_auth,
};

/// Indices of Web API skills whose auth was lost to redaction: the URI
/// carries Azure's `code=<redacted>` placeholder and the skill has neither
/// AAD auth (`authResourceId`) nor the push-time key annotation.
/// The Azure Functions key header. Azure redacts its value on GET exactly
/// like the `code` uri parameter; header names are matched case-insensitively.
pub const FUNCTIONS_KEY_HEADER: &str = "x-functions-key";

/// The skill's function-key header entry, if any: (exact key as written, value).
pub fn function_key_header(skill: &Value) -> Option<(&String, &Value)> {
    skill
        .get("httpHeaders")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(FUNCTIONS_KEY_HEADER))
}

/// A key value Azure Search cannot authenticate with: redacted, empty, null.
fn unusable_key_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty() || s.contains("<redacted>"),
        _ => false,
    }
}

/// Put a real function key into the carrier the skill actually uses: the
/// `x-functions-key` header when present (name case preserved, uri left
/// alone), otherwise the uri's `code` parameter.
pub fn place_function_key(skill: &mut Value, key: &str) {
    let header_name = function_key_header(skill).map(|(k, _)| k.clone());
    if let Some(name) = header_name {
        skill["httpHeaders"][name] = Value::String(key.to_string());
        return;
    }
    let uri = skill.get("uri").and_then(Value::as_str).unwrap_or_default();
    let injected = set_code_param(uri, key);
    skill["uri"] = Value::String(injected);
}

/// Remove the function-key header (any casing) — the Entra ID counterpart of
/// `strip_code_param`.
pub fn remove_function_key_header(skill: &mut Value) {
    if let Some(headers) = skill.get_mut("httpHeaders").and_then(Value::as_object_mut) {
        headers.retain(|k, _| !k.eq_ignore_ascii_case(FUNCTIONS_KEY_HEADER));
    }
}

pub fn webapi_skills_missing_auth(doc: &Value) -> Vec<usize> {
    let Some(skills) = doc.get("skills").and_then(Value::as_array) else {
        return Vec::new();
    };
    skills
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            let unusable_uri_key = s
                .get("uri")
                .and_then(Value::as_str)
                .is_some_and(|u| u.contains("code=<redacted>"));
            let unusable_header_key =
                function_key_header(s).is_some_and(|(_, v)| unusable_key_value(v));
            s.get("@odata.type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.ends_with("WebApiSkill"))
                && (unusable_uri_key || unusable_header_key)
                && s.get("authResourceId")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                && !s
                    .get(X_RIGG_AUTH)
                    .and_then(Value::as_str)
                    .is_some_and(is_known_auth_annotation)
        })
        .map(|(i, _)| i)
        .collect()
}

/// `https://<site>.azurewebsites.net/api/<function>?...` → (site, function).
pub fn parse_function_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let site = host.strip_suffix(".azurewebsites.net")?;
    let path = path.split('?').next().unwrap_or(path);
    let function = path.rsplit('/').next().filter(|s| !s.is_empty())?;
    Some((site.to_string(), function.to_string()))
}

/// Replace (or append) the `code` query parameter of a URI.
/// Put a real function key into the uri's `code` parameter. Defined in
/// core, next to the promote-side stripping that is its inverse.
pub use rigg_core::promote::set_code_param;

/// Remove the `code` query parameter from a URI.
pub fn strip_code_param(uri: &str) -> String {
    let (base, query) = match uri.split_once('?') {
        Some((b, q)) => (b, q),
        None => return uri.to_string(),
    };
    let params: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("code="))
        .collect();
    if params.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", params.join("&"))
    }
}

/// The Entra ID audience a function app's Easy Auth grants, read from its
/// `authsettingsV2` document: the first allowed audience, else
/// `api://<clientId>`. `None` when the platform switch or the Microsoft
/// identity provider is off — i.e. when the app has no Entra auth at all.
pub fn easy_auth_audience_of(settings: &Value) -> Option<String> {
    let enabled = settings
        .pointer("/properties/platform/enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        && settings
            .pointer("/properties/identityProviders/azureActiveDirectory/enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    if !enabled {
        return None;
    }
    settings
        .pointer("/properties/identityProviders/azureActiveDirectory/validation/allowedAudiences/0")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            settings
                .pointer("/properties/identityProviders/azureActiveDirectory/registration/clientId")
                .and_then(Value::as_str)
                .filter(|c| !c.is_empty())
                .map(|c| format!("api://{c}"))
        })
}

/// [`easy_auth_audience_of`] for the site at `site_id` (a Microsoft.Web
/// site's ARM resource id, from [`ArmClient::find_web_site_id`]).
///
/// An ARM failure collapses into `None` here, which is only safe because the
/// one caller — the interactive [`resolve_webapi_auth`] — offers the user a
/// choice either way and never records a decision on the strength of a
/// `None`. A caller that would WRITE something on "no Entra auth" must call
/// [`ArmClient::site_auth_settings`] itself and keep the `Err` apart from an
/// `Ok` document that says the app is anonymous — promote's online phase
/// does exactly that.
pub async fn easy_auth_audience(arm: &ArmClient, site_id: &str) -> Option<String> {
    easy_auth_audience_of(&arm.site_auth_settings(site_id).await.ok()?)
}

/// How one Web API skill's authorization got resolved.
pub enum WebApiAuthOutcome {
    /// `authResourceId` written — durable, keyless (Entra ID).
    EntraId,
    /// `x-rigg-auth: function-key` annotated — key injected at push time.
    FunctionKey,
    /// User skipped; the skill will fail at enrichment time until fixed.
    Skipped,
}

/// Interactively resolve authorization for the Web API skill at `idx`:
/// Entra ID (`authResourceId`, recommended — offered ready-made when the
/// function app already has Easy Auth, otherwise with concrete enablement
/// guidance) or a push-time-resolved function key (never stored on disk).
pub async fn resolve_webapi_auth(
    ctx: &crate::commands::GlobalContext,
    doc: &mut Value,
    idx: usize,
    ds_display: &str,
) -> Result<WebApiAuthOutcome> {
    let plain = ctx.no_color;
    let uri = doc["skills"][idx]["uri"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let parsed = parse_function_uri(&uri);
    println!();
    println!("{ds_display} calls a custom Web API whose key was redacted by Azure:\n    {uri}");

    // Diagnose the function app's Entra (Easy Auth) state via ARM.
    let arm = ArmClient::new().ok();
    let mut site_id = None;
    let mut entra_audience: Option<String> = None;
    if let (Some(arm), Some((site, _))) = (&arm, &parsed)
        && let Ok(id) = arm.find_web_site_id(site).await
    {
        entra_audience = easy_auth_audience(arm, &id).await;
        site_id = Some(id);
    }

    const ENTRA_READY: &str =
        "identity-based (Entra ID) — recommended: keyless, verifiable by auth doctor";
    const ENTRA_SETUP: &str =
        "identity-based (Entra ID) — set it up now (registers the app and enables Easy Auth)";
    const ENTRA_GUIDE: &str = "identity-based (Entra ID) — recommended, but the function app has no Entra auth yet (show what's needed)";
    const KEY: &str = "function key, resolved at push time — key stays in Azure, never on disk";
    const SKIP: &str = "skip for now (enrichment will fail until authorized)";
    // `rigg auth easy-auth` wires the app end to end, but only when the app
    // is a declared function-app binding — that is where its scope comes from.
    let wiring = easy_auth_binding_for(ctx, &parsed);
    let entra_option = match (&entra_audience, &wiring) {
        (Some(_), _) => ENTRA_READY,
        (None, Some(_)) => ENTRA_SETUP,
        (None, None) => ENTRA_GUIDE,
    };
    let choice = interactive::select(
        "How should the search service authenticate to this function?",
        vec![entra_option.to_string(), KEY.to_string(), SKIP.to_string()],
        plain,
    )?;

    if choice == ENTRA_SETUP {
        let (ws, env, binding) = wiring.expect("offered only when a binding was found");
        // `wire` also rewrites the skillset FILE (every skill calling that
        // app). The same edit is applied to the caller's in-memory document
        // below, because the caller writes it back after this returns — so
        // the two must agree rather than one silently undoing the other.
        let wired = crate::commands::easy_auth::wire(ctx, &ws, &env, &binding, None).await?;
        if !wired.applied {
            return Ok(WebApiAuthOutcome::Skipped);
        }
        doc["skills"][idx]["authResourceId"] = Value::String(wired.audience.clone());
        doc["skills"][idx]["uri"] = Value::String(strip_code_param(&uri));
        remove_function_key_header(&mut doc["skills"][idx]);
        if let Some(map) = doc["skills"][idx].as_object_mut() {
            map.remove(X_RIGG_AUTH);
        }
        println!(
            "  {} Easy Auth enabled on '{}'; authResourceId set to '{}'",
            "✓".green(),
            wired.site,
            wired.audience
        );
        return Ok(WebApiAuthOutcome::EntraId);
    }

    if choice == ENTRA_READY {
        let audience = entra_audience.expect("ready implies audience");
        doc["skills"][idx]["authResourceId"] = Value::String(audience.clone());
        doc["skills"][idx]["uri"] = Value::String(strip_code_param(&uri));
        remove_function_key_header(&mut doc["skills"][idx]);
        println!(
            "  {} authResourceId set to '{audience}' (uri key parameter and x-functions-key header removed)",
            "✓".green()
        );
        println!(
            "  note: if the app's Entra config requires assignment, permit the search identity on it"
        );
        return Ok(WebApiAuthOutcome::EntraId);
    }
    if choice == ENTRA_GUIDE {
        let site = parsed
            .as_ref()
            .map(|(s, _)| s.clone())
            .unwrap_or_else(|| "<app>".to_string());
        println!("  to make this keyless, enable Entra auth on the function app:");
        println!("    az webapp auth microsoft update -g <rg> -n {site} \\");
        println!("        --client-id <app-registration-id> --yes");
        println!("    (portal: Function App → Authentication → Add identity provider → Microsoft)");
        println!(
            "  then run `rigg push` again — rigg will detect it and offer the ready-made option."
        );
        // Fall through to offering the key so the user is not stuck.
        if !interactive::confirm_default_yes("Use the push-time function key until then?", plain)? {
            return Ok(WebApiAuthOutcome::Skipped);
        }
    } else if choice == SKIP {
        return Ok(WebApiAuthOutcome::Skipped);
    }

    // Function-key mode: verify the key is retrievable NOW so push-time
    // injection cannot surprise-fail later.
    let (Some(arm), Some((_, function))) = (&arm, &parsed) else {
        println!(
            "  {} cannot reach ARM or parse the function URI — fix the uri or use Entra auth",
            "!".yellow()
        );
        return Ok(WebApiAuthOutcome::Skipped);
    };
    let Some(site_id) = &site_id else {
        println!(
            "  {} function app not found via ARM — is it in a subscription this login can see?",
            "!".yellow()
        );
        return Ok(WebApiAuthOutcome::Skipped);
    };
    match arm.function_key(site_id, function).await {
        Ok(_) => {
            doc["skills"][idx][X_RIGG_AUTH] = Value::String(X_RIGG_AUTH_FUNCTION_KEY.to_string());
            println!(
                "  {} key verified retrievable; the skill is annotated `{}: {}` — rigg fetches and injects it whenever the skillset is pushed (or on `rigg push --refresh-credentials`), the file keeps the placeholder",
                "✓".green(),
                X_RIGG_AUTH,
                X_RIGG_AUTH_FUNCTION_KEY
            );
            Ok(WebApiAuthOutcome::FunctionKey)
        }
        Err(e) => {
            println!(
                "  {} could not retrieve the function key ({e})",
                "!".yellow()
            );
            Ok(WebApiAuthOutcome::Skipped)
        }
    }
}

/// The workspace, environment and `function-app` binding name that
/// `rigg auth easy-auth` would need for the site in `parsed` — `None` when
/// there is no workspace, no environment, or no binding pointing at it.
fn easy_auth_binding_for(
    ctx: &crate::commands::GlobalContext,
    parsed: &Option<(String, String)>,
) -> Option<(Workspace, ResolvedEnv, String)> {
    let (site, _) = parsed.as_ref()?;
    let ws = crate::commands::load_workspace().ok()?;
    let env = crate::commands::resolve_env(&ws, ctx).ok()?;
    let binding = env
        .env
        .dependencies
        .iter()
        .find(|(_, b)| {
            b.kind == BindingType::FunctionApp && b.physical_name().eq_ignore_ascii_case(site)
        })
        .map(|(name, _)| name.clone())?;
    Some((ws, env, binding))
}

/// Push-time key injection: for every Web API skill annotated with a key
/// source, fetch the key and place it in the PUSHED body —
/// `function-key` through ARM `listkeys`, `key-vault:<secret>@<binding>`
/// through the vault's data plane (spec §6). The annotation itself is
/// stripped with the other `x-rigg-*` keys before the PUT; the file never
/// changes.
///
/// The fetched value exists only in `body` from here on: it is never
/// written to disk, printed, or traced — not even in an error, which names
/// the secret and the vault but never the value.
pub async fn inject_function_keys(
    body: &mut Value,
    ws: &Workspace,
    env: &ResolvedEnv,
) -> Result<()> {
    let Some(skills) = body.get_mut("skills").and_then(Value::as_array_mut) else {
        return Ok(());
    };
    let mut arm: Option<ArmClient> = None;
    for skill in skills {
        let Some(annotation) = skill.get(X_RIGG_AUTH).and_then(Value::as_str) else {
            continue;
        };
        if let Some((secret, binding)) = parse_key_vault_auth(annotation) {
            let vault_uri = key_vault_uri(ws, env, binding).await?;
            let key =
                rigg_client::keyvault::get_secret(env.env.tenant.as_deref(), &vault_uri, secret)
                    .await
                    .map_err(|e| {
                        anyhow::anyhow!("could not read secret '{secret}' from '{vault_uri}': {e}")
                    })?;
            place_function_key(skill, &key);
            continue;
        }
        if annotation != X_RIGG_AUTH_FUNCTION_KEY {
            anyhow::bail!(
                "unknown `{X_RIGG_AUTH}` value '{annotation}' — expected '{X_RIGG_AUTH_FUNCTION_KEY}' \
                 or '{X_RIGG_AUTH_KEY_VAULT_PREFIX}<secret>@<key-vault binding>'"
            );
        }
        let uri = skill.get("uri").and_then(Value::as_str).unwrap_or_default();
        let Some((site, function)) = parse_function_uri(uri) else {
            anyhow::bail!("cannot parse function app URI '{uri}' for push-time key injection");
        };
        if arm.is_none() {
            arm = Some(
                ArmClient::for_tenant(env.env.tenant.as_deref()).map_err(|e| {
                    anyhow::anyhow!("push-time key injection needs ARM access (az login): {e}")
                })?,
            );
        }
        let arm = arm.as_ref().expect("just initialized");
        let site_id = arm.find_web_site_id(&site).await?;
        let key = arm.function_key(&site_id, &function).await?;
        place_function_key(skill, &key);
    }
    Ok(())
}

/// The data-plane URI of the `key-vault` binding named by a
/// `key-vault:<secret>@<binding>` annotation.
///
/// The binding must be a declared `key-vault` dependency — `rigg validate`
/// says so too, but push must not depend on validate having run. The URI
/// comes from the resolution cache when it has one, else from a fresh ARM
/// lookup.
async fn key_vault_uri(ws: &Workspace, env: &ResolvedEnv, binding: &str) -> Result<String> {
    let declared = env.env.dependencies.get(binding).ok_or_else(|| {
        anyhow::anyhow!(
            "`{X_RIGG_AUTH}: {X_RIGG_AUTH_KEY_VAULT_PREFIX}…@{binding}` names no dependency in \
             environment '{}' — declare it: `rigg env bind {} {binding} key-vault:<vault-name>`",
            env.name,
            env.name
        )
    })?;
    if declared.kind != BindingType::KeyVault {
        anyhow::bail!(
            "'{binding}' is a {} binding, not a key-vault binding — a `{X_RIGG_AUTH}` key source \
             must name a key vault",
            declared.kind
        );
    }
    let cache = BindingCache::load(ws, &env.name);
    if let Some(uri) = cache.get(binding).and_then(|r| r.endpoint.clone()) {
        return Ok(uri);
    }
    let arm = ArmClient::for_tenant(env.env.tenant.as_deref())
        .map_err(|e| anyhow::anyhow!("resolving key vault '{binding}' needs ARM access: {e}"))?;
    let resolved = arm
        .resolve_binding(
            BindingType::KeyVault,
            &declared.value,
            env.env.subscription.as_deref(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("could not resolve key-vault binding '{binding}': {e}"))?;
    resolved
        .endpoint
        .clone()
        .ok_or_else(|| anyhow::anyhow!("key vault '{binding}' reports no vaultUri"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn auth_settings(platform: bool, aad: bool, audiences: Value, client_id: &str) -> Value {
        json!({"properties": {
            "platform": {"enabled": platform},
            "identityProviders": {"azureActiveDirectory": {
                "enabled": aad,
                "registration": {"clientId": client_id},
                "validation": {"allowedAudiences": audiences}
            }}
        }})
    }

    #[test]
    fn easy_auth_audience_prefers_the_allowed_audience_over_the_client_id() {
        assert_eq!(
            easy_auth_audience_of(&auth_settings(
                true,
                true,
                json!(["api://audience-1"]),
                "client-1"
            ))
            .as_deref(),
            Some("api://audience-1")
        );
        // No audience listed: the app registration's own id is the audience.
        assert_eq!(
            easy_auth_audience_of(&auth_settings(true, true, json!([]), "client-1")).as_deref(),
            Some("api://client-1")
        );
        // Either switch off means the app has no Entra auth to use.
        assert!(
            easy_auth_audience_of(&auth_settings(false, true, json!(["api://a"]), "c")).is_none()
        );
        assert!(
            easy_auth_audience_of(&auth_settings(true, false, json!(["api://a"]), "c")).is_none()
        );
        // Enabled but unconfigured: nothing to authenticate with.
        assert!(easy_auth_audience_of(&auth_settings(true, true, json!([]), "")).is_none());
        assert!(easy_auth_audience_of(&json!({})).is_none());
    }

    fn webapi_skillset(uri: &str, extra: Value) -> Value {
        let mut skill = json!({
            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
            "uri": uri
        });
        if let (Some(s), Some(e)) = (skill.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                s.insert(k.clone(), v.clone());
            }
        }
        json!({"name": "ss", "skills": [skill]})
    }

    #[test]
    fn detects_webapi_skill_with_redacted_code() {
        let doc = webapi_skillset(
            "https://fn.azurewebsites.net/api/enrich?code=<redacted>",
            json!({}),
        );
        assert_eq!(webapi_skills_missing_auth(&doc), vec![0]);
        // authResourceId set → not missing
        let doc = webapi_skillset(
            "https://fn.azurewebsites.net/api/enrich?code=<redacted>",
            json!({"authResourceId": "api://x"}),
        );
        assert!(webapi_skills_missing_auth(&doc).is_empty());
        // annotated for push-time key → not missing
        let doc = webapi_skillset(
            "https://fn.azurewebsites.net/api/enrich?code=<redacted>",
            json!({"x-rigg-auth": "function-key"}),
        );
        assert!(webapi_skills_missing_auth(&doc).is_empty());
        // real key (hand-authored) → not missing
        let doc = webapi_skillset(
            "https://fn.azurewebsites.net/api/enrich?code=abc",
            json!({}),
        );
        assert!(webapi_skills_missing_auth(&doc).is_empty());
    }

    #[test]
    fn parses_function_uris() {
        assert_eq!(
            parse_function_uri(
                "https://mklab.azurewebsites.net/api/enrichRegulatoryMetadata?code=<redacted>"
            ),
            Some(("mklab".to_string(), "enrichRegulatoryMetadata".to_string()))
        );
        assert_eq!(parse_function_uri("https://example.com/api/x"), None);
    }

    #[test]
    fn code_param_roundtrip() {
        let uri = "https://f.azurewebsites.net/api/x?code=<redacted>&v=1";
        assert_eq!(
            set_code_param(uri, "KEY"),
            "https://f.azurewebsites.net/api/x?v=1&code=KEY"
        );
        assert_eq!(
            strip_code_param(uri),
            "https://f.azurewebsites.net/api/x?v=1"
        );
        assert_eq!(
            strip_code_param("https://f.azurewebsites.net/api/x?code=only"),
            "https://f.azurewebsites.net/api/x"
        );
        assert_eq!(
            set_code_param("https://f.azurewebsites.net/api/x", "KEY"),
            "https://f.azurewebsites.net/api/x?code=KEY"
        );
    }

    #[test]
    fn detects_key_based_ai_services_with_placeholder() {
        let doc = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByKey",
            "key": "<redacted>",
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        assert_eq!(
            skillset_missing_ai_services_key(&doc),
            Some(Some("https://acc.cognitiveservices.azure.com/".to_string()))
        );
    }

    #[test]
    fn ignores_identity_based_and_real_keys() {
        let identity = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity",
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        assert_eq!(skillset_missing_ai_services_key(&identity), None);
        let real = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByKey",
            "key": "abc123",
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        assert_eq!(skillset_missing_ai_services_key(&real), None);
        let none = json!({"name": "s", "skills": []});
        assert_eq!(skillset_missing_ai_services_key(&none), None);
    }

    #[test]
    fn rewrite_sets_identity_form() {
        let mut doc = json!({"name": "s", "cognitiveServices": {
            "@odata.type": "#Microsoft.Azure.Search.AIServicesByKey",
            "key": null,
            "subdomainUrl": "https://acc.cognitiveservices.azure.com/"
        }});
        set_ai_services_identity(&mut doc, "https://acc.cognitiveservices.azure.com/");
        assert_eq!(
            doc["cognitiveServices"]["@odata.type"],
            "#Microsoft.Azure.Search.AIServicesByIdentity"
        );
        assert!(doc["cognitiveServices"].get("key").is_none());
    }

    #[test]
    fn account_name_from_subdomain() {
        assert_eq!(
            ai_services_account_name("https://mklabaisrvc.cognitiveservices.azure.com/"),
            Some("mklabaisrvc")
        );
        assert_eq!(ai_services_account_name("nonsense"), None);
    }

    #[test]
    fn missing_credentials_detection() {
        assert!(missing_credentials(
            &json!({"credentials": {"connectionString": null}})
        ));
        assert!(missing_credentials(&json!({"name": "x"})));
        assert!(!missing_credentials(
            &json!({"credentials": {"connectionString": "ResourceId=/x;"}})
        ));
    }
}

#[cfg(test)]
mod function_key_header_tests {
    use super::*;
    use serde_json::json;

    fn webapi_skillset(uri: &str, extra: Value) -> Value {
        let mut skill = json!({
            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
            "uri": uri
        });
        if let (Some(s), Some(e)) = (skill.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                s.insert(k.clone(), v.clone());
            }
        }
        json!({"name": "ss", "skills": [skill]})
    }

    const URI: &str = "https://fn.azurewebsites.net/api/enrich";

    #[test]
    fn detects_redacted_functions_key_header() {
        let doc = webapi_skillset(
            URI,
            json!({"httpHeaders": {"x-functions-key": "<redacted>"}}),
        );
        assert_eq!(webapi_skills_missing_auth(&doc), vec![0]);
    }

    #[test]
    fn header_name_matching_is_case_insensitive() {
        let doc = webapi_skillset(
            URI,
            json!({"httpHeaders": {"X-Functions-Key": "<redacted>"}}),
        );
        assert_eq!(webapi_skills_missing_auth(&doc), vec![0]);
    }

    #[test]
    fn empty_or_null_header_values_are_unusable() {
        let doc = webapi_skillset(URI, json!({"httpHeaders": {"x-functions-key": ""}}));
        assert_eq!(webapi_skills_missing_auth(&doc), vec![0]);
        let doc = webapi_skillset(URI, json!({"httpHeaders": {"x-functions-key": null}}));
        assert_eq!(webapi_skills_missing_auth(&doc), vec![0]);
    }

    #[test]
    fn absent_header_is_not_flagged() {
        // Anonymous functions are legitimate — no header, no code param, no flag.
        let doc = webapi_skillset(
            URI,
            json!({"httpHeaders": {"content-type": "application/json"}}),
        );
        assert!(webapi_skills_missing_auth(&doc).is_empty());
        let doc = webapi_skillset(URI, json!({}));
        assert!(webapi_skills_missing_auth(&doc).is_empty());
    }

    #[test]
    fn auth_resource_id_or_annotation_suppresses_header_detection() {
        let doc = webapi_skillset(
            URI,
            json!({"httpHeaders": {"x-functions-key": "<redacted>"}, "authResourceId": "api://x"}),
        );
        assert!(webapi_skills_missing_auth(&doc).is_empty());
        let doc = webapi_skillset(
            URI,
            json!({"httpHeaders": {"x-functions-key": "<redacted>"}, "x-rigg-auth": "function-key"}),
        );
        assert!(webapi_skills_missing_auth(&doc).is_empty());
    }

    #[test]
    fn real_header_value_is_not_flagged_as_missing() {
        // A real key is not "missing auth" — validate rejects it separately
        // under the no-secrets policy.
        let doc = webapi_skillset(URI, json!({"httpHeaders": {"x-functions-key": "real-key"}}));
        assert!(webapi_skills_missing_auth(&doc).is_empty());
    }

    #[test]
    fn place_function_key_prefers_header_carrier_preserving_name_case() {
        let mut skill = json!({
            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
            "uri": URI,
            "httpHeaders": {"X-Functions-Key": "<redacted>"}
        });
        place_function_key(&mut skill, "real-key");
        assert_eq!(skill["httpHeaders"]["X-Functions-Key"], "real-key");
        assert_eq!(
            skill["uri"], URI,
            "uri untouched when header is the carrier"
        );
    }

    #[test]
    fn place_function_key_falls_back_to_code_param() {
        let mut skill = json!({
            "@odata.type": "#Microsoft.Skills.Custom.WebApiSkill",
            "uri": format!("{URI}?code=<redacted>")
        });
        place_function_key(&mut skill, "real-key");
        assert_eq!(skill["uri"], format!("{URI}?code=real-key"));
    }

    #[test]
    fn remove_function_key_header_strips_any_casing() {
        let mut skill = json!({
            "httpHeaders": {"X-FUNCTIONS-KEY": "<redacted>", "content-type": "application/json"}
        });
        remove_function_key_header(&mut skill);
        assert_eq!(
            skill["httpHeaders"],
            json!({"content-type": "application/json"})
        );
    }
}
