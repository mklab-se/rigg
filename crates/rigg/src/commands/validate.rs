//! `rigg validate` — structural, referential, ownership, and no-secrets checks.
//!
//! Exit code 3 when any check fails.

use anyhow::{Result, anyhow};
use colored::Colorize;
use serde_json::Value;

use rigg_core::registry::{self, X_RIGG_API, X_RIGG_REF};
use rigg_core::resources::{ResourceKind, ResourceRef};
use rigg_core::store::{Store, assert_exclusive_ownership};
use rigg_core::workspace::{Project, Workspace};

use crate::cli::ValidateArgs;
use crate::commands::{CommandError, GlobalContext, load_workspace};

pub fn run(ctx: &GlobalContext, args: ValidateArgs) -> Result<()> {
    let ws = load_workspace()?;
    let projects = select_projects_lenient(&ws, args.project.as_deref())?;

    let mut problems: Vec<String> = Vec::new();

    // Validate every environment any project participates in. A project
    // with no env dirs yet reports nothing (empty project).
    let mut all_envs: Vec<String> = Vec::new();
    for project in &ws.projects {
        for e in Store::envs_of(project) {
            if !all_envs.contains(&e) {
                all_envs.push(e);
            }
        }
    }
    all_envs.sort();

    for env in &all_envs {
        // Per-env: exclusive ownership.
        if let Err(e) = assert_exclusive_ownership(&ws, env) {
            problems.push(format!("[env {env}] {e}"));
        }

        // All resources in this env across the workspace (for reference
        // resolution — references resolve within the SAME env).
        let mut workspace_refs: Vec<ResourceRef> = Vec::new();
        for project in &ws.projects {
            if let Ok(list) = Store::new(project, env).list() {
                workspace_refs.extend(list.into_iter().map(|(r, _)| r));
            }
        }

        for project in &projects {
            if !Store::envs_of(project).contains(env) {
                continue; // this project doesn't participate in this env
            }
            let proj_problems = validate_project(&ws, project, env, &workspace_refs, args.strict);
            problems.extend(
                proj_problems
                    .into_iter()
                    .map(|p| format!("[env {env}] {p}")),
            );
        }
    }

    if ctx.json() {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "valid": problems.is_empty(),
                "problems": problems,
            }))?
        );
    } else if problems.is_empty() {
        println!("{} all checks passed", "✓".green().bold());
    } else {
        for p in &problems {
            println!("{} {p}", "✗".red().bold());
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(CommandError::Validation(format!(
            "{} validation problem(s) found",
            problems.len()
        ))))
    }
}

fn select_projects_lenient<'w>(
    ws: &'w Workspace,
    project: Option<&str>,
) -> Result<Vec<&'w Project>> {
    match project {
        Some(name) => Ok(vec![ws.project(name)?]),
        None => Ok(ws.projects.iter().collect()),
    }
}

/// Validate one project's tree in one environment. Returns problems found
/// (without the `[env <name>]` prefix — the caller adds that uniformly).
fn validate_project(
    ws: &Workspace,
    project: &Project,
    env: &str,
    workspace_refs: &[ResourceRef],
    strict: bool,
) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    let store = Store::new(project, env);
    let list = match store.list() {
        Ok(list) => list,
        Err(e) => {
            problems.push(format!("[{}] {e}", project.name));
            return problems;
        }
    };

    for (r, path) in &list {
        let display = path
            .strip_prefix(&ws.root)
            .unwrap_or(path)
            .display()
            .to_string();
        let value = match store.read(r) {
            Ok(v) => v,
            Err(e) => {
                problems.push(format!("[{display}] {e}"));
                continue;
            }
        };

        // "name" identifies the physical resource; it no longer needs to
        // match the file stem (the stem is the logical/cross-env id — they
        // may legitimately diverge when a resource is renamed in one env).
        // A file with no "name" field at all still needs one to push.
        if value.get("name").and_then(Value::as_str).is_none() {
            problems.push(format!("[{display}] missing \"name\" field"));
        }

        // no secrets (registry secret_fields carrying key material)
        check_secrets(r.kind, &value, &display, &mut problems);

        // registry references resolve within the workspace, same environment
        for (kind, name) in registry::extract_references(r.kind, &value) {
            if name.starts_with('<') {
                problems.push(format!(
                    "[{display}] placeholder reference '{name}' — replace the scaffold placeholder"
                ));
                continue;
            }
            let target = ResourceRef::new(kind, name.clone());
            if !workspace_refs.contains(&target) {
                // The target may legitimately live outside rigg (pre-existing
                // Azure resource). Warn by default; --strict makes it an error.
                if strict {
                    problems.push(format!(
                        "[{display}] references {target} which does not exist in this workspace"
                    ));
                } else {
                    eprintln!(
                        "{} [{display}] references {target} — not in this workspace (must already exist in Azure)",
                        "warning:".yellow()
                    );
                }
            }
        }

        // x-rigg-api links resolve to specs in apis/
        check_api_links(ws, &value, &display, &mut problems);

        // datasource type validity
        if r.kind == ResourceKind::DataSource {
            if let Some(ds_type) = value.get("type").and_then(Value::as_str) {
                match rigg_core::scaffold::check_datasource_type(ds_type) {
                    Ok(Some(warning)) => eprintln!("{} [{display}] {warning}", "warning:".yellow()),
                    Ok(None) => {}
                    Err(e) => problems.push(format!("[{display}] {e}")),
                }
                warn_missing_deletion_tracking(ds_type, &value, &display);
            } else {
                problems.push(format!("[{display}] data source missing \"type\""));
            }
            warn_missing_credentials(&value, &display);
        }

        // custom Web API skills with a redacted function key and no auth
        if r.kind == ResourceKind::Skillset
            && !crate::commands::credentials::webapi_skills_missing_auth(&value).is_empty()
        {
            eprintln!(
                "{} [{display}] a custom Web API skill's key was redacted — enrichment will fail; run `rigg push` interactively to choose Entra ID auth or a push-time function key",
                "warning:".yellow()
            );
        }

        // skillset with a key-based AI services connection but no usable key
        if r.kind == ResourceKind::Skillset {
            if let Some(subdomain) =
                crate::commands::credentials::skillset_missing_ai_services_key(&value)
            {
                let hint = match subdomain {
                    Some(s) => format!("switch to AIServicesByIdentity (subdomainUrl: {s})"),
                    None => "add an identity-based cognitiveServices connection \
                             (AIServicesByIdentity + subdomainUrl)"
                        .to_string(),
                };
                eprintln!(
                    "{} [{display}] key-based cognitiveServices connection without a usable key — push will fail; {hint}",
                    "warning:".yellow()
                );
            }
        }
    }
    problems
}

/// Warn when a data source has no usable connection at all — typical after
/// copying an Azure-generated definition (GET never returns credentials).
/// Pushing it would create a data source that cannot reach its source.
fn warn_missing_credentials(value: &Value, display: &str) {
    let conn = value
        .pointer("/credentials/connectionString")
        .and_then(Value::as_str)
        .unwrap_or("");
    if conn.trim().is_empty() {
        eprintln!(
            "{} [{display}] no credentials.connectionString — the indexer cannot reach the source; \
             use identity-based access (ResourceId=/subscriptions/.../storageAccounts/<name>;)",
            "warning:".yellow()
        );
    }
}

/// Warn when a data source cannot detect deletions: removed source documents
/// would stay in the index forever. SQL integrated change tracking covers
/// deletes by itself; everything else needs an explicit deletion policy.
fn warn_missing_deletion_tracking(ds_type: &str, value: &Value, display: &str) {
    let has_deletion_policy = value
        .get("dataDeletionDetectionPolicy")
        .is_some_and(|p| !p.is_null());
    let integrated_sql = value
        .get("dataChangeDetectionPolicy")
        .and_then(|p| p.get("@odata.type"))
        .and_then(Value::as_str)
        .is_some_and(|t| t.contains("SqlIntegratedChangeTracking"));
    if !has_deletion_policy && !integrated_sql {
        let hint = match ds_type {
            "azureblob" | "adlsgen2" | "azurefile" | "azurefiles" => {
                "add NativeBlobSoftDeleteDeletionDetectionPolicy (and enable blob soft delete on the storage account)"
            }
            "cosmosdb" => "add SoftDeleteColumnDeletionDetectionPolicy on a soft-delete column",
            "azuresql" => {
                "use SqlIntegratedChangeTrackingPolicy (covers deletes) or a soft-delete column policy"
            }
            _ => "add a dataDeletionDetectionPolicy suited to the source",
        };
        eprintln!(
            "{} [{display}] no deletion tracking — documents removed from the source will remain in the index; {hint}",
            "warning:".yellow()
        );
    }
}

/// Reject key material in registry-declared secret fields, and obvious
/// key patterns anywhere (AccountKey=, AccountName=...;AccountKey).
fn check_secrets(kind: ResourceKind, value: &Value, display: &str, problems: &mut Vec<String>) {
    for spec in registry::meta(kind).secret_fields {
        registry::collect_path(value, spec, &mut |v| {
            if let Some(s) = v.as_str() {
                if !s.is_empty() && !s.starts_with("ResourceId=") && !s.starts_with('<') {
                    problems.push(format!(
                        "[{display}] field '{spec}' contains a credential — rigg never stores secrets locally. \
                         Use a managed identity (connection string 'ResourceId=/subscriptions/...') and grant the \
                         identity RBAC access instead; secrets belong in Azure Key Vault, never in files"
                    ));
                }
            }
        });
    }
    // Blanket scan for storage account keys.
    let text = value.to_string();
    if text.contains("AccountKey=") {
        problems.push(format!(
            "[{display}] contains an 'AccountKey=' connection string — replace it with an identity-based \
             'ResourceId=...' connection and delete/rotate the leaked key"
        ));
    }
    // Azure Functions keys in Web API skill headers. The header name is
    // matched case-insensitively, which the registry's path table cannot
    // express — hence checked here instead of via `secret_fields`.
    if kind == ResourceKind::Skillset {
        if let Some(skills) = value.get("skills").and_then(Value::as_array) {
            for skill in skills {
                if let Some((name, v)) = crate::commands::credentials::function_key_header(skill) {
                    let real = v
                        .as_str()
                        .is_some_and(|s| !s.is_empty() && !s.starts_with('<'));
                    if real {
                        problems.push(format!(
                            "[{display}] header '{name}' contains a function key — rigg never stores secrets locally. \
                             Keep the '<redacted>' placeholder and run `rigg push` interactively to choose Entra ID \
                             auth (authResourceId) or push-time key resolution (x-rigg-auth: function-key)"
                        ));
                    }
                }
            }
        }
    }
}

fn check_api_links(ws: &Workspace, value: &Value, display: &str, problems: &mut Vec<String>) {
    // Walk skills so the contract check sees the whole skill object.
    if let Some(skills) = value.get("skills").and_then(Value::as_array) {
        for skill in skills {
            let Some(api) = skill.get(X_RIGG_API).and_then(Value::as_str) else {
                continue;
            };
            let path = ws.apis_dir().join(format!("{api}.json"));
            if !path.is_file() {
                problems.push(format!(
                    "[{display}] x-rigg-api '{api}' has no spec at apis/{api}.json (create with `rigg new api {api}`)"
                ));
                continue;
            }
            let spec = match rigg_core::openapi::load(&path) {
                Ok(spec) => spec,
                Err(e) => {
                    problems.push(format!("[{display}] apis/{api}.json: {e}"));
                    continue;
                }
            };
            check_skill_contract(skill, api, &spec, display, problems);
        }
    }
    // Non-skillset x-rigg-api references still need the spec to exist.
    collect_key(value, X_RIGG_API, &mut |v| {
        if let Some(api) = v.as_str() {
            let path = ws.apis_dir().join(format!("{api}.json"));
            if !path.is_file() && value.get("skills").is_none() {
                problems.push(format!(
                    "[{display}] x-rigg-api '{api}' has no spec at apis/{api}.json (create with `rigg new api {api}`)"
                ));
            }
        }
    });
    collect_key(value, X_RIGG_REF, &mut |v| {
        if let Some(s) = v.as_str() {
            let valid = s
                .split_once('/')
                .and_then(|(dir, _)| ResourceKind::from_directory_name(dir))
                .is_some();
            if !valid {
                problems.push(format!(
                    "[{display}] x-rigg-ref '{s}' is not of the form <kind-dir>/<name>"
                ));
            }
        }
    });
}

/// Verify a WebApiSkill against its OpenAPI contract (spec §9).
fn check_skill_contract(
    skill: &Value,
    api: &str,
    spec: &rigg_core::openapi::ApiSpec,
    display: &str,
    problems: &mut Vec<String>,
) {
    // URI path must match one of the spec's paths (compared by path suffix).
    if let Some(uri) = skill.get("uri").and_then(Value::as_str) {
        if let Ok(parsed) = reqwest::Url::parse(uri) {
            let uri_path = parsed.path();
            if !uri_path.is_empty()
                && uri_path != "/"
                && !spec.paths.iter().any(|p| uri_path.ends_with(p.as_str()))
            {
                problems.push(format!(
                    "[{display}] WebApiSkill uri path '{uri_path}' does not match any path in apis/{api}.json ({})",
                    spec.paths.join(", ")
                ));
            }
        }
    }
    if !spec.open_props {
        for (field, props) in [
            ("inputs", &spec.request_data_props),
            ("outputs", &spec.response_data_props),
        ] {
            if let Some(entries) = skill.get(field).and_then(Value::as_array) {
                for entry in entries {
                    let Some(name) = entry.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    if !props.contains(&name.to_string()) {
                        problems.push(format!(
                            "[{display}] skill {field} '{name}' is not in apis/{api}.json's data schema ({})",
                            props.join(", ")
                        ));
                    }
                }
            }
        }
    }
}

fn collect_key(value: &Value, key: &str, f: &mut dyn FnMut(&Value)) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == key {
                    f(v);
                } else {
                    collect_key(v, key, f);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_key(item, key, f);
            }
        }
        _ => {}
    }
}
