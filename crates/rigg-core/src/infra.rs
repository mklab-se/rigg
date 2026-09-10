//! Infrastructure references: recognizing, rewriting, and classifying the
//! values named by the registry's [`crate::registry::InfraRef`] table.
//!
//! [`parse`] turns a raw JSON value into a [`PhysicalRef`] (the physical
//! Azure resource it names); [`render`] does the inverse — rewrite a value
//! for a different physical resource, keeping everything about the original
//! that isn't the infrastructure part. [`extract`] walks a whole document
//! per its kind's `infra_refs` table and returns every reference found, with
//! concrete (indexed) paths. [`classify`] compares each found reference
//! against an environment's declared bindings.

use serde_json::{Map, Value};

use crate::binding::{BindingEntry, BindingKind, BindingType, EnvBindings, Wanted};
use crate::registry::{self, InfraForm, SEARCH_PREVIEW_API_VERSION};
use crate::resources::ResourceKind;

/// The kind of physical Azure resource an infrastructure reference names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Storage,
    Identity,
    ModelHost,
    AiServices,
    FunctionApp,
    Api,
    KeyVault,
    SearchService,
}

/// A recognized infrastructure reference: what it points at, and enough of
/// the original value to rewrite it later.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalRef {
    pub target: Target,
    /// Lowercase resource name (or host, for URL-shaped forms).
    pub physical: String,
    pub original: Value,
    /// The knowledge-base name, for [`InfraForm::SearchKbMcpUrl`] only.
    pub kb_name: Option<String>,
}

/// The physical resource `render` should rewrite a value to point at.
#[derive(Debug, Clone)]
pub struct RenderTarget {
    pub physical: String,
    pub arm_id: Option<String>,
    pub base_url: Option<String>,
    pub kb_name: Option<String>,
    /// The *source* binding's base URL, for [`InfraForm::ApiUri`]'s `Api`
    /// case — the prefix `render` should replace with `base_url`. When
    /// absent, only the scheme and host are swapped.
    pub source_base_url: Option<String>,
}

/// One infrastructure reference found in a document, at a concrete
/// (indexed) path.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundRef {
    /// Concrete path, e.g. `skills[2].uri`.
    pub path: String,
    pub form: InfraForm,
    pub physical: PhysicalRef,
}

/// How a [`FoundRef`] relates to `this_env`'s declared bindings.
#[derive(Debug, Clone, PartialEq)]
pub enum Class {
    /// Bound to a declared (or implicit) binding in this environment, named
    /// `String`, and not bound anywhere else.
    Bound(String),
    /// Bound in this environment (named `String`), and *also* bound to the
    /// same physical resource in the listed other environments.
    Shared(String, Vec<String>),
    /// Not bound in this environment, but bound (to `binding`) in the listed
    /// other environments — the reference "leaked" a value that belongs to
    /// another environment.
    Leak { binding: String, envs: Vec<String> },
    /// Not bound anywhere, and not an `Api` reference (so not presumed
    /// external either).
    Unbound,
    /// An `Api` reference bound nowhere — presumed to be a genuinely
    /// external, environment-independent endpoint.
    External,
}

/// One [`FoundRef`] together with its [`Class`].
#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub found: FoundRef,
    pub class: Class,
}

const OPENAI_HOST_SUFFIXES: &[&str] = &[
    "openai.azure.com",
    "cognitiveservices.azure.com",
    "services.ai.azure.com",
];

/// Parse a raw value per `form` into the [`PhysicalRef`] it names. `None`
/// when the value is absent, null, a placeholder, or not recognized as this
/// form's shape.
pub fn parse(form: InfraForm, value: &Value) -> Option<PhysicalRef> {
    match form {
        InfraForm::StorageResourceId => parse_storage(value),
        InfraForm::UserAssignedIdentity => parse_identity(value),
        InfraForm::OpenAiEndpoint => parse_host(value, Target::ModelHost),
        InfraForm::AiServicesSubdomain => parse_host(value, Target::AiServices),
        InfraForm::ApiUri => parse_api_uri(value),
        InfraForm::KeyVaultUri => parse_keyvault(value),
        InfraForm::SearchKbMcpUrl => parse_kb_mcp(value),
    }
}

/// Rewrite `original` per `form`, pointing at `target` instead — keeping
/// everything about `original` that isn't the infrastructure part (e.g. a
/// connection string's `Database=` tail, a URI's path and query).
pub fn render(form: InfraForm, original: &Value, target: &RenderTarget) -> Result<Value, String> {
    match form {
        InfraForm::StorageResourceId => render_storage(original, target),
        InfraForm::UserAssignedIdentity => render_identity(original, target),
        InfraForm::OpenAiEndpoint => render_host_swap(original, target, OPENAI_HOST_SUFFIXES),
        InfraForm::AiServicesSubdomain => render_host_swap(original, target, OPENAI_HOST_SUFFIXES),
        InfraForm::ApiUri => render_api_uri(original, target),
        InfraForm::KeyVaultUri => render_keyvault(original, target),
        InfraForm::SearchKbMcpUrl => render_kb_mcp(target),
    }
}

/// Every infrastructure reference in `doc`, per `kind`'s `infra_refs` table,
/// at concrete (indexed) paths.
pub fn extract(kind: ResourceKind, doc: &Value) -> Vec<FoundRef> {
    let mut out = Vec::new();
    for ir in registry::infra_refs(kind) {
        let segments: Vec<&str> = ir.path.split('.').collect();
        let mut found: Vec<(String, &Value)> = Vec::new();
        walk_infra(
            doc,
            &segments,
            String::new(),
            ir.only_odata_type,
            &mut found,
        );
        for (path, value) in found {
            if let Some(physical) = parse(ir.form, value) {
                out.push(FoundRef {
                    path,
                    form: ir.form,
                    physical,
                });
            }
        }
    }
    out
}

/// Classify every ref in `refs` against `this_env`'s bindings, checking
/// `other_envs` for sharing/leaking.
pub fn classify(
    this_env: &EnvBindings,
    other_envs: &[EnvBindings],
    refs: Vec<FoundRef>,
) -> Vec<Classified> {
    refs.into_iter()
        .map(|found| {
            let target = found.physical.target;
            let wanted = wanted_for(target);
            let ref_url = found.physical.original.as_str().unwrap_or("").to_string();
            let physical = found.physical.physical.clone();

            let this_hit =
                find_wanted(this_env, wanted, target, &ref_url, &physical).map(|e| e.name.clone());

            let class = match this_hit {
                Some(name) => {
                    let shared_envs: Vec<String> = other_envs
                        .iter()
                        .filter(|e| find_wanted(e, wanted, target, &ref_url, &physical).is_some())
                        .map(|e| e.env.clone())
                        .collect();
                    if shared_envs.is_empty() {
                        Class::Bound(name)
                    } else {
                        Class::Shared(name, shared_envs)
                    }
                }
                None => {
                    let mut leak_binding: Option<String> = None;
                    let mut envs = Vec::new();
                    for e in other_envs {
                        if let Some(entry) = find_wanted(e, wanted, target, &ref_url, &physical) {
                            if leak_binding.is_none() {
                                leak_binding = Some(entry.name.clone());
                            }
                            envs.push(e.env.clone());
                        }
                    }
                    match leak_binding {
                        Some(binding) => Class::Leak { binding, envs },
                        None if target == Target::Api => Class::External,
                        None => Class::Unbound,
                    }
                }
            };
            Classified { found, class }
        })
        .collect()
}

fn wanted_for(target: Target) -> Wanted {
    match target {
        Target::Storage => Wanted::Type(BindingType::Storage),
        Target::Identity => Wanted::Type(BindingType::Identity),
        Target::ModelHost => Wanted::ModelHost,
        Target::AiServices => Wanted::ModelHost,
        Target::FunctionApp => Wanted::Type(BindingType::FunctionApp),
        Target::Api => Wanted::Type(BindingType::Api),
        Target::KeyVault => Wanted::Type(BindingType::KeyVault),
        Target::SearchService => Wanted::SearchService,
    }
}

/// Look up a binding matching `wanted` in `env`. `Api` references are
/// matched by URL prefix against declared `api` bindings rather than by
/// exact physical-name equality (an `api` binding's value may be a base URL
/// with a path, e.g. `https://api.partner.example/v1`).
fn find_wanted<'a>(
    env: &'a EnvBindings,
    wanted: Wanted,
    target: Target,
    ref_url: &str,
    physical: &str,
) -> Option<&'a BindingEntry> {
    if target == Target::Api {
        find_api_binding(env, ref_url)
    } else {
        env.find_physical(wanted, physical)
    }
}

fn find_api_binding<'a>(env: &'a EnvBindings, ref_url: &str) -> Option<&'a BindingEntry> {
    let ref_lower = ref_url.to_ascii_lowercase();
    env.iter().find(|e| {
        matches!(e.kind, BindingKind::Declared(BindingType::Api))
            && e.declared.as_ref().is_some_and(|b| {
                let origin = b.value.trim_end_matches('/').to_ascii_lowercase();
                !origin.is_empty() && ref_lower.starts_with(&origin)
            })
    })
}

// ---------------------------------------------------------------------
// parse
// ---------------------------------------------------------------------

fn parse_storage(value: &Value) -> Option<PhysicalRef> {
    let s = value.as_str()?;
    let body = s.strip_prefix("ResourceId=")?;
    let arm_id = body.split(';').next().unwrap_or(body);
    if arm_id.contains('<') {
        return None;
    }
    let trimmed = arm_id.trim_end_matches('/');
    let name = crate::binding::arm_resource_name(trimmed)?;
    Some(PhysicalRef {
        target: Target::Storage,
        physical: name.to_ascii_lowercase(),
        original: value.clone(),
        kb_name: None,
    })
}

fn parse_identity(value: &Value) -> Option<PhysicalRef> {
    let obj = value.as_object()?;
    let uid = obj.get("userAssignedIdentity")?.as_str()?;
    if uid.is_empty() || uid.contains('<') {
        return None;
    }
    let name = crate::binding::arm_resource_name(uid.trim_end_matches('/'))?;
    Some(PhysicalRef {
        target: Target::Identity,
        physical: name.to_ascii_lowercase(),
        original: value.clone(),
        kb_name: None,
    })
}

fn parse_host(value: &Value, target: Target) -> Option<PhysicalRef> {
    let url = as_url(value)?;
    let (host, _tail) = host_and_tail(url);
    let (name, _suffix) = parse_host_suffix(&host, OPENAI_HOST_SUFFIXES)?;
    Some(PhysicalRef {
        target,
        physical: name,
        original: value.clone(),
        kb_name: None,
    })
}

fn parse_api_uri(value: &Value) -> Option<PhysicalRef> {
    let url = as_url(value)?;
    let (host, _tail) = host_and_tail(url);
    let host_lower = host.to_ascii_lowercase();
    if let Some(name) = host_lower.strip_suffix(".azurewebsites.net") {
        return Some(PhysicalRef {
            target: Target::FunctionApp,
            physical: name.to_string(),
            original: value.clone(),
            kb_name: None,
        });
    }
    Some(PhysicalRef {
        target: Target::Api,
        physical: host_lower,
        original: value.clone(),
        kb_name: None,
    })
}

fn parse_keyvault(value: &Value) -> Option<PhysicalRef> {
    let url = as_url(value)?;
    let (host, _tail) = host_and_tail(url);
    let name = host
        .to_ascii_lowercase()
        .strip_suffix(".vault.azure.net")?
        .to_string();
    Some(PhysicalRef {
        target: Target::KeyVault,
        physical: name,
        original: value.clone(),
        kb_name: None,
    })
}

fn parse_kb_mcp(value: &Value) -> Option<PhysicalRef> {
    let url = as_url(value)?;
    let (host, tail) = host_and_tail(url);
    let svc = host
        .to_ascii_lowercase()
        .strip_suffix(".search.windows.net")?
        .to_string();
    let path_only = tail.split('?').next().unwrap_or(tail);
    let mut segs = path_only.split('/').filter(|s| !s.is_empty());
    let a = segs.next()?;
    let kb = segs.next()?;
    let c = segs.next()?;
    if !a.eq_ignore_ascii_case("knowledgebases") || !c.eq_ignore_ascii_case("mcp") {
        return None;
    }
    Some(PhysicalRef {
        target: Target::SearchService,
        physical: svc,
        original: value.clone(),
        kb_name: Some(kb.to_string()),
    })
}

// ---------------------------------------------------------------------
// render
// ---------------------------------------------------------------------

fn render_storage(original: &Value, target: &RenderTarget) -> Result<Value, String> {
    let arm_id = target
        .arm_id
        .as_ref()
        .ok_or("StorageResourceId render requires arm_id")?;
    let s = original
        .as_str()
        .ok_or("StorageResourceId render requires a string value")?;
    let body = s
        .strip_prefix("ResourceId=")
        .ok_or("expected a `ResourceId=` value")?;
    let tail = body.split_once(';').map(|(_, t)| t);
    let out = match tail {
        Some(t) => format!("ResourceId={arm_id};{t}"),
        None => format!("ResourceId={arm_id}"),
    };
    Ok(Value::String(out))
}

fn render_identity(original: &Value, target: &RenderTarget) -> Result<Value, String> {
    let arm_id = target
        .arm_id
        .as_ref()
        .ok_or("UserAssignedIdentity render requires arm_id")?;
    let mut obj: Map<String, Value> = original
        .as_object()
        .cloned()
        .ok_or("UserAssignedIdentity render requires an object value")?;
    obj.insert(
        "userAssignedIdentity".to_string(),
        Value::String(arm_id.clone()),
    );
    Ok(Value::Object(obj))
}

fn render_host_swap(
    original: &Value,
    target: &RenderTarget,
    suffixes: &'static [&'static str],
) -> Result<Value, String> {
    let url = original
        .as_str()
        .ok_or("host-swap render requires a string value")?;
    let (host, tail) = host_and_tail(url);
    let (_, suffix) = parse_host_suffix(&host, suffixes)
        .ok_or_else(|| format!("`{url}` is not a recognized infrastructure host"))?;
    let scheme = url_scheme(url);
    Ok(Value::String(format!(
        "{scheme}://{}.{suffix}{tail}",
        target.physical
    )))
}

fn render_api_uri(original: &Value, target: &RenderTarget) -> Result<Value, String> {
    let url = original
        .as_str()
        .ok_or("ApiUri render requires a string value")?;
    let (host, tail) = host_and_tail(url);
    if host.to_ascii_lowercase().ends_with(".azurewebsites.net") {
        let scheme = url_scheme(url);
        return Ok(Value::String(format!(
            "{scheme}://{}.azurewebsites.net{tail}",
            target.physical
        )));
    }

    let base_url = target
        .base_url
        .as_ref()
        .ok_or("Api render requires base_url")?;
    if let Some(src) = &target.source_base_url {
        let src_norm = src.trim_end_matches('/').to_ascii_lowercase();
        let url_lower = url.to_ascii_lowercase();
        if url_lower.starts_with(&src_norm) {
            let remainder = &url[src_norm.len()..];
            return Ok(Value::String(format!(
                "{}{remainder}",
                base_url.trim_end_matches('/')
            )));
        }
        return Err(format!(
            "source_base_url `{src}` does not prefix reference `{url}`"
        ));
    }
    Ok(Value::String(format!(
        "{}{tail}",
        scheme_and_host(base_url)
    )))
}

fn render_keyvault(original: &Value, target: &RenderTarget) -> Result<Value, String> {
    let url = original
        .as_str()
        .ok_or("KeyVaultUri render requires a string value")?;
    let (_, tail) = host_and_tail(url);
    let scheme = url_scheme(url);
    Ok(Value::String(format!(
        "{scheme}://{}.vault.azure.net{tail}",
        target.physical
    )))
}

fn render_kb_mcp(target: &RenderTarget) -> Result<Value, String> {
    let kb = target
        .kb_name
        .as_deref()
        .ok_or("SearchKbMcpUrl render requires kb_name")?;
    Ok(Value::String(format!(
        "https://{}.search.windows.net/knowledgebases/{kb}/mcp?api-version={SEARCH_PREVIEW_API_VERSION}",
        target.physical
    )))
}

// ---------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------

fn as_url(value: &Value) -> Option<&str> {
    let s = value.as_str()?;
    (s.starts_with("http://") || s.starts_with("https://")).then_some(s)
}

/// Split a URL into its host and the tail (path + query, including the
/// leading `/`, or empty when there is none).
fn host_and_tail(url: &str) -> (String, &str) {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match after_scheme.find('/') {
        Some(i) => (after_scheme[..i].to_string(), &after_scheme[i..]),
        None => (after_scheme.to_string(), ""),
    }
}

/// The scheme + host of `url` (drops any path/query).
fn scheme_and_host(url: &str) -> String {
    match url.find("://") {
        Some(i) => {
            let after = &url[i + 3..];
            match after.find('/') {
                Some(j) => url[..i + 3 + j].to_string(),
                None => url.to_string(),
            }
        }
        None => url.to_string(),
    }
}

fn url_scheme(url: &str) -> &'static str {
    if url.starts_with("https://") {
        "https"
    } else {
        "http"
    }
}

/// Strip one of `suffixes` (each matched as `.{suffix}`, case-insensitively)
/// from `host`, returning the remaining name and the matched suffix.
fn parse_host_suffix(
    host: &str,
    suffixes: &'static [&'static str],
) -> Option<(String, &'static str)> {
    let lower = host.to_ascii_lowercase();
    for &suffix in suffixes {
        if let Some(name) = lower.strip_suffix(&format!(".{suffix}"))
            && !name.is_empty()
        {
            return Some((name.to_string(), suffix));
        }
    }
    None
}

/// `constraint`'s last dot-segment (e.g. `"AzureOpenAIEmbeddingSkill"` from
/// `"#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill"`) must be a suffix of
/// `actual` — so any namespace ending in that skill type name matches.
fn odata_type_matches(constraint: &str, actual: &str) -> bool {
    let tail = constraint.rsplit('.').next().unwrap_or(constraint);
    actual.ends_with(tail)
}

/// Walk `segments` (registry path syntax) from `v`, appending matched
/// terminal `(concrete_path, &Value)` pairs to `out`. `[]` segments expand
/// to concrete indices (`skills[2]`); when `only_odata_type` is set, an
/// array element is only descended into when its `@odata.type` matches (see
/// [`odata_type_matches`]).
fn walk_infra<'a>(
    v: &'a Value,
    segments: &[&str],
    prefix: String,
    only_odata_type: Option<&str>,
    out: &mut Vec<(String, &'a Value)>,
) {
    let Some((head, rest)) = segments.split_first() else {
        out.push((prefix, v));
        return;
    };
    if let Some(key) = head.strip_suffix("[]") {
        let target = if key.is_empty() { Some(v) } else { v.get(key) };
        if let Some(Value::Array(arr)) = target {
            for (i, item) in arr.iter().enumerate() {
                if let Some(constraint) = only_odata_type {
                    let actual = item
                        .get("@odata.type")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !odata_type_matches(constraint, actual) {
                        continue;
                    }
                }
                let new_prefix = format!("{prefix}{key}[{i}]");
                walk_infra(item, rest, new_prefix, only_odata_type, out);
            }
        }
    } else if let Some(next) = v.get(*head) {
        let new_prefix = if prefix.is_empty() {
            (*head).to_string()
        } else {
            format!("{prefix}.{head}")
        };
        walk_infra(next, rest, new_prefix, only_odata_type, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::Binding;
    use crate::workspace::{Environment, FoundryConnection, SearchConnection};
    use serde_json::json;

    fn env_with(
        deps: &[(&str, BindingType, &str)],
        search_svc: &str,
        foundry_acct: &str,
    ) -> Environment {
        Environment {
            search: Some(SearchConnection {
                service: search_svc.to_string(),
                ..Default::default()
            }),
            foundry: Some(FoundryConnection {
                account: foundry_acct.to_string(),
                project: "p".to_string(),
                ..Default::default()
            }),
            dependencies: deps
                .iter()
                .map(|(name, kind, value)| {
                    (
                        name.to_string(),
                        Binding {
                            kind: *kind,
                            value: value.to_string(),
                        },
                    )
                })
                .collect(),
            ..Default::default()
        }
    }

    fn found(form: InfraForm, path: &str, target: Target, physical: &str) -> FoundRef {
        FoundRef {
            path: path.to_string(),
            form,
            physical: PhysicalRef {
                target,
                physical: physical.to_string(),
                original: json!(format!("https://{physical}/probe")),
                kb_name: None,
            },
        }
    }

    #[test]
    fn parse_and_render_storage_resource_id() {
        let v = json!(
            "ResourceId=/subscriptions/S/resourceGroups/RG/providers/Microsoft.Storage/storageAccounts/MKLabAcct/;Database=x"
        );
        let p = parse(InfraForm::StorageResourceId, &v).unwrap();
        assert_eq!(
            (p.target, p.physical.as_str()),
            (Target::Storage, "mklabacct")
        );
        let out = render(
            InfraForm::StorageResourceId,
            &v,
            &RenderTarget {
                physical: "prodacct".into(),
                arm_id: Some(
                    "/subscriptions/P/resourceGroups/PRG/providers/Microsoft.Storage/storageAccounts/prodacct"
                        .into(),
                ),
                base_url: None,
                kb_name: None,
                source_base_url: None,
            },
        )
        .unwrap();
        assert_eq!(
            out,
            json!(
                "ResourceId=/subscriptions/P/resourceGroups/PRG/providers/Microsoft.Storage/storageAccounts/prodacct;Database=x"
            )
        );
        assert!(
            parse(
                InfraForm::StorageResourceId,
                &json!("ResourceId=/subscriptions/<sub>/…")
            )
            .is_none()
        );
    }

    #[test]
    fn parse_user_assigned_identity() {
        let id = json!({
            "@odata.type": "#Microsoft.Azure.Search.DataUserAssignedIdentity",
            "userAssignedIdentity": "/subscriptions/S/resourcegroups/RG/providers/Microsoft.ManagedIdentity/userAssignedIdentities/Rigg-Dev"
        });
        assert_eq!(
            parse(InfraForm::UserAssignedIdentity, &id)
                .unwrap()
                .physical,
            "rigg-dev"
        );
        assert!(parse(InfraForm::UserAssignedIdentity, &json!(null)).is_none());
    }

    #[test]
    fn parse_and_render_openai_endpoint_keeps_path() {
        let e = json!("https://MKLabAIFNDR.openai.azure.com/");
        let p = parse(InfraForm::OpenAiEndpoint, &e).unwrap();
        assert_eq!(
            (p.target, p.physical.as_str()),
            (Target::ModelHost, "mklabaifndr")
        );
        assert_eq!(
            render(
                InfraForm::OpenAiEndpoint,
                &e,
                &RenderTarget {
                    physical: "prodaifndr".into(),
                    arm_id: None,
                    base_url: None,
                    kb_name: None,
                    source_base_url: None,
                },
            )
            .unwrap(),
            json!("https://prodaifndr.openai.azure.com/")
        );
    }

    #[test]
    fn parse_and_render_function_app_vs_external_api() {
        let f = json!("https://mklab.azurewebsites.net/api/enrich?code=<redacted>");
        let p = parse(InfraForm::ApiUri, &f).unwrap();
        assert_eq!(
            (p.target, p.physical.as_str()),
            (Target::FunctionApp, "mklab")
        );
        assert_eq!(
            render(
                InfraForm::ApiUri,
                &f,
                &RenderTarget {
                    physical: "mklab-prod".into(),
                    arm_id: None,
                    base_url: None,
                    kb_name: None,
                    source_base_url: None,
                },
            )
            .unwrap(),
            json!("https://mklab-prod.azurewebsites.net/api/enrich?code=<redacted>")
        );

        let x = json!("https://api.partner.example/v1/enrich");
        assert_eq!(parse(InfraForm::ApiUri, &x).unwrap().target, Target::Api);
        assert_eq!(
            render(
                InfraForm::ApiUri,
                &x,
                &RenderTarget {
                    physical: "api.partner-prod.example".into(),
                    arm_id: None,
                    base_url: Some("https://api.partner-prod.example/v2".into()),
                    kb_name: None,
                    source_base_url: Some("https://api.partner.example/v1".into()),
                },
            )
            .unwrap_or(json!(null)),
            json!("https://api.partner-prod.example/v2/enrich")
        );
    }

    #[test]
    fn parse_key_vault_uri() {
        assert_eq!(
            parse(
                InfraForm::KeyVaultUri,
                &json!("https://mklabkv.vault.azure.net/keys/k/1")
            )
            .unwrap()
            .physical,
            "mklabkv"
        );
    }

    #[test]
    fn parse_and_render_search_kb_mcp_url() {
        let m = json!(
            "https://mklabsrch.search.windows.net/knowledgeBases/regulatory-kb/mcp?api-version=old"
        );
        let p = parse(InfraForm::SearchKbMcpUrl, &m).unwrap();
        assert_eq!(
            (p.target, p.physical.as_str(), p.kb_name.as_deref()),
            (Target::SearchService, "mklabsrch", Some("regulatory-kb"))
        );
        let r = render(
            InfraForm::SearchKbMcpUrl,
            &m,
            &RenderTarget {
                physical: "mklabsrch-prod".into(),
                arm_id: None,
                base_url: None,
                kb_name: Some("regulatory-kb".into()),
                source_base_url: None,
            },
        )
        .unwrap();
        assert_eq!(
            r,
            json!(format!(
                "https://mklabsrch-prod.search.windows.net/knowledgebases/regulatory-kb/mcp?api-version={}",
                crate::registry::SEARCH_PREVIEW_API_VERSION
            ))
        );
    }

    #[test]
    fn extract_walks_arrays_with_concrete_paths() {
        let skillset = json!({"name": "ss", "skills": [
            {"@odata.type": "#Microsoft.Skills.Text.SplitSkill"},
            {"@odata.type": "#Microsoft.Skills.Text.AzureOpenAIEmbeddingSkill", "resourceUri": "https://mklabaifndr.openai.azure.com"},
            {"@odata.type": "#Microsoft.Skills.Custom.WebApiSkill", "uri": "https://mklab.azurewebsites.net/api/x"}
        ], "cognitiveServices": {"@odata.type": "#Microsoft.Azure.Search.AIServicesByIdentity", "subdomainUrl": "https://mklabaisrvc.cognitiveservices.azure.com/"}});
        let refs = extract(ResourceKind::Skillset, &skillset);
        let paths: Vec<&str> = refs.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"skills[1].resourceUri"), "{paths:?}");
        assert!(paths.contains(&"skills[2].uri"), "{paths:?}");
        assert!(
            paths.contains(&"cognitiveServices.subdomainUrl"),
            "{paths:?}"
        );
        assert_eq!(refs.len(), 3);
    }

    #[test]
    fn classify_bound_shared_leak_unbound_external() {
        let dev = EnvBindings::of_env(
            "dev",
            &env_with(
                &[
                    ("docs", BindingType::Storage, "acct"),
                    ("fn", BindingType::FunctionApp, "mklab"),
                ],
                "mklabsrch",
                "mklabaifndr",
            ),
            None,
        );
        let prod = EnvBindings::of_env(
            "prod",
            &env_with(
                &[
                    ("docs", BindingType::Storage, "acct"),
                    ("fn", BindingType::FunctionApp, "mklab-prod"),
                ],
                "mklabsrch-prod",
                "mklabaifndr-prod",
            ),
            None,
        );
        let refs = vec![
            found(
                InfraForm::StorageResourceId,
                "credentials.connectionString",
                Target::Storage,
                "acct",
            ),
            found(
                InfraForm::ApiUri,
                "skills[0].uri",
                Target::FunctionApp,
                "mklab-prod",
            ),
            found(
                InfraForm::OpenAiEndpoint,
                "skills[1].resourceUri",
                Target::ModelHost,
                "mklabaifndr",
            ),
            found(
                InfraForm::KeyVaultUri,
                "encryptionKey.keyVaultUri",
                Target::KeyVault,
                "kv",
            ),
            found(
                InfraForm::ApiUri,
                "skills[2].uri",
                Target::Api,
                "api.partner.example",
            ),
        ];
        let out = classify(&dev, &[prod], refs);
        assert!(
            matches!(&out[0].class, Class::Shared(b, envs) if b == "docs" && envs == &vec!["prod".to_string()])
        );
        assert!(
            matches!(&out[1].class, Class::Leak { binding, envs } if binding == "fn" && envs == &vec!["prod".to_string()])
        );
        assert!(matches!(&out[2].class, Class::Bound(b) if b == "foundry"));
        assert!(matches!(out[3].class, Class::Unbound));
        assert!(matches!(out[4].class, Class::External));
    }
}
