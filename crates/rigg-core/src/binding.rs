//! Binding values, implicit `search`/`foundry` bindings, and the
//! per-environment resolution cache.
//!
//! [`Binding`] (a `dependencies` entry from `rigg.yaml`) and [`BindingType`]
//! live here; `workspace` re-exports them so existing `workspace::Binding`
//! paths keep compiling. [`EnvBindings`] is the read-only view combining an
//! environment's declared `dependencies` with its implicit `search`/
//! `foundry` targets, optionally enriched with cached resolution results.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::workspace::{Environment, STATE_DIR, Workspace};

/// A named reference to a supporting Azure resource outside rigg's own
/// kinds, e.g. `docs-storage: { storage: mklabstorageacc }`. Serialized as a
/// one-key map `{ "<type>": "<value>" }`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub kind: BindingType,
    pub value: String,
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let map = BTreeMap::<String, String>::deserialize(d)?;
        if map.len() != 1 {
            return Err(serde::de::Error::custom(
                "a binding is exactly one `<type>: <value>` pair",
            ));
        }
        let (k, value) = map.into_iter().next().expect("one entry");
        let kind = k.parse::<BindingType>().map_err(serde::de::Error::custom)?;
        if value.trim().is_empty() {
            return Err(serde::de::Error::custom(format!(
                "binding `{k}` has an empty value"
            )));
        }
        Ok(Binding { kind, value })
    }
}

impl Serialize for Binding {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1))?;
        map.serialize_entry(&self.kind.to_string(), &self.value)?;
        map.end()
    }
}

/// The kind of supporting resource a [`Binding`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BindingType {
    Storage,
    AiServices,
    FunctionApp,
    Identity,
    KeyVault,
    Api,
}

impl BindingType {
    const ALL: [(&'static str, BindingType); 6] = [
        ("storage", BindingType::Storage),
        ("ai-services", BindingType::AiServices),
        ("function-app", BindingType::FunctionApp),
        ("identity", BindingType::Identity),
        ("key-vault", BindingType::KeyVault),
        ("api", BindingType::Api),
    ];
}

impl std::fmt::Display for BindingType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = Self::ALL
            .iter()
            .find(|(_, t)| *t == *self)
            .map(|(name, _)| *name)
            .expect("all variants covered");
        write!(f, "{name}")
    }
}

impl std::str::FromStr for BindingType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|(name, _)| *name == s)
            .map(|(_, t)| *t)
            .ok_or_else(|| {
                format!(
                    "unknown binding type '{s}' (expected one of: storage, ai-services, \
                     function-app, identity, key-vault, api)"
                )
            })
    }
}

/// Serialized as a plain string (its kebab name) for use as an ordinary JSON
/// field, e.g. in [`ResolvedBinding::kind`] — distinct from [`Binding`]'s
/// one-key-map encoding.
impl Serialize for BindingType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for BindingType {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Reserved dependency binding names — these name the `search`/`foundry`
/// targets, not `dependencies` entries.
const RESERVED_BINDING_NAMES: [&str; 2] = ["search", "foundry"];

/// Validate a `dependencies` binding name: lowercase kebab-case, not
/// reserved, not empty.
pub fn validate_binding_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() {
        return Err("binding name must not be empty".to_string());
    }
    if RESERVED_BINDING_NAMES.contains(&name) {
        return Err(format!(
            "'{name}' is a reserved name (used for the search/foundry target) and cannot be a \
             dependency binding"
        ));
    }
    let valid_chars = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid_chars || name.starts_with('-') || name.ends_with('-') || name.contains("--") {
        return Err(format!(
            "binding name '{name}' must be lowercase kebab-case (letters, digits, single \
             hyphens; no leading/trailing hyphen)"
        ));
    }
    Ok(())
}

/// The parsed shape of a [`Binding`]'s value string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingValue {
    /// A bare resource name (e.g. `mklabstorageacc`).
    Name(String),
    /// A full ARM resource id (`/subscriptions/...`).
    ArmId(String),
    /// A URL (`http://` or `https://`).
    Url(String),
}

impl Binding {
    /// Classify this binding's value string.
    pub fn value(&self) -> BindingValue {
        if self.value.starts_with("/subscriptions/") {
            BindingValue::ArmId(self.value.clone())
        } else if self.value.starts_with("http://") || self.value.starts_with("https://") {
            BindingValue::Url(self.value.clone())
        } else {
            BindingValue::Name(self.value.clone())
        }
    }

    /// The physical resource name this binding resolves to, lowercased:
    /// the bare name as-is, an ARM id's last path segment, or a URL's host
    /// (no port, no path).
    pub fn physical_name(&self) -> String {
        match self.value() {
            BindingValue::Name(n) => n.to_lowercase(),
            BindingValue::ArmId(id) => arm_resource_name(&id).unwrap_or(id.as_str()).to_lowercase(),
            BindingValue::Url(u) => url_host(&u).to_lowercase(),
        }
    }

    /// The raw ARM resource id, when this binding's value is one.
    pub fn arm_id(&self) -> Option<&str> {
        match self.value() {
            BindingValue::ArmId(_) => Some(self.value.as_str()),
            _ => None,
        }
    }
}

/// The host portion of a URL: strips scheme, userinfo, port and path.
fn url_host(u: &str) -> String {
    let after_scheme = u.splitn(2, "://").last().unwrap_or(u);
    let host_and_rest = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host_port = host_and_rest.rsplit('@').next().unwrap_or(host_and_rest);
    let host = host_port.split(':').next().unwrap_or(host_port);
    host.to_string()
}

/// The last path segment of an ARM resource id — the resource's own name.
pub fn arm_resource_name(id: &str) -> Option<&str> {
    let trimmed = id.trim_end_matches('/');
    trimmed.rsplit('/').next().filter(|s| !s.is_empty())
}

/// The `{subscription}` segment of `/subscriptions/{subscription}/...`.
pub fn arm_subscription(id: &str) -> Option<&str> {
    arm_segment(id, "subscriptions")
}

/// The `{resourceGroup}` segment of `.../resourceGroups/{resourceGroup}/...`.
pub fn arm_resource_group(id: &str) -> Option<&str> {
    arm_segment(id, "resourceGroups")
}

fn arm_segment<'a>(id: &'a str, key: &str) -> Option<&'a str> {
    let parts: Vec<&str> = id.split('/').collect();
    parts
        .iter()
        .position(|p| p.eq_ignore_ascii_case(key))
        .and_then(|i| parts.get(i + 1).copied())
        .filter(|s| !s.is_empty())
}

/// A binding resolved against Azure, cached so later runs don't have to
/// re-discover it. Written by `rigg env resolve` (a later task); read by
/// anything that needs a physical name, ARM id, or endpoint for a binding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedBinding {
    pub name: String,
    pub kind: BindingType,
    pub physical_name: String,
    pub arm_id: Option<String>,
    pub subscription: Option<String>,
    pub resource_group: Option<String>,
    pub location: Option<String>,
    pub endpoint: Option<String>,
    /// Principal id of the resource's system-assigned managed identity, when
    /// applicable (filled in for identity bindings by a later task).
    #[serde(default)]
    pub principal_id: Option<String>,
    /// RFC3339 timestamp of when this resolution was captured.
    pub resolved_at: String,
}

/// Per-environment cache of [`ResolvedBinding`]s, persisted at
/// `.rigg/<env>/bindings.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BindingCache {
    #[serde(default)]
    pub bindings: BTreeMap<String, ResolvedBinding>,
}

impl BindingCache {
    pub fn path(ws: &Workspace, env: &str) -> PathBuf {
        ws.files_root()
            .join(STATE_DIR)
            .join(env)
            .join("bindings.json")
    }

    pub fn load(ws: &Workspace, env: &str) -> BindingCache {
        let path = Self::path(ws, env);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, ws: &Workspace, env: &str) -> io::Result<()> {
        let path = Self::path(ws, env);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).unwrap_or_default();
        std::fs::write(&path, json)
    }

    pub fn get(&self, name: &str) -> Option<&ResolvedBinding> {
        self.bindings.get(name)
    }
}

/// How a [`BindingEntry`] came to exist in an [`EnvBindings`] table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKind {
    /// A `dependencies` entry declared in `rigg.yaml`.
    Declared(BindingType),
    /// The environment's `search` target, exposed under the reserved name
    /// `search`.
    ImplicitSearch,
    /// The environment's `foundry` target, exposed under the reserved name
    /// `foundry`.
    ImplicitFoundry,
}

/// One entry in an [`EnvBindings`] table.
#[derive(Debug, Clone)]
pub struct BindingEntry {
    pub name: String,
    pub kind: BindingKind,
    pub physical_name: String,
    /// The declared `dependencies` binding, when this entry came from one.
    pub declared: Option<Binding>,
    /// The cached resolution for this entry, when one is available.
    pub resolved: Option<ResolvedBinding>,
}

/// What [`EnvBindings::find_physical`] is looking for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wanted {
    /// A declared binding of this exact type.
    Type(BindingType),
    /// The environment's implicit `search` target.
    SearchService,
    /// Anything that can host a model deployment: a declared `ai-services`
    /// binding, or the environment's implicit `foundry` target.
    ModelHost,
}

impl Wanted {
    fn matches(&self, kind: BindingKind) -> bool {
        match (self, kind) {
            (Wanted::Type(t), BindingKind::Declared(k)) => *t == k,
            (Wanted::SearchService, BindingKind::ImplicitSearch) => true,
            (Wanted::ModelHost, BindingKind::Declared(BindingType::AiServices)) => true,
            (Wanted::ModelHost, BindingKind::ImplicitFoundry) => true,
            _ => false,
        }
    }
}

/// One environment's binding table: declared `dependencies` plus implicit
/// `search`/`foundry` entries, optionally enriched from a [`BindingCache`].
#[derive(Debug, Clone)]
pub struct EnvBindings {
    pub env: String,
    entries: BTreeMap<String, BindingEntry>,
}

impl EnvBindings {
    /// Build the binding table for one environment. Workspace-free — takes
    /// the environment directly, so callers that already have it (and
    /// tests) don't need a full [`Workspace`].
    pub fn of_env(name: &str, env: &Environment, cache: Option<&BindingCache>) -> EnvBindings {
        let mut entries = BTreeMap::new();

        if let Some(search) = &env.search {
            let physical_name = search.service.to_lowercase();
            entries.insert(
                "search".to_string(),
                BindingEntry {
                    name: "search".to_string(),
                    kind: BindingKind::ImplicitSearch,
                    physical_name,
                    declared: None,
                    resolved: cache.and_then(|c| c.get("search")).cloned(),
                },
            );
        }
        if let Some(foundry) = &env.foundry {
            let physical_name = foundry.account.to_lowercase();
            entries.insert(
                "foundry".to_string(),
                BindingEntry {
                    name: "foundry".to_string(),
                    kind: BindingKind::ImplicitFoundry,
                    physical_name,
                    declared: None,
                    resolved: cache.and_then(|c| c.get("foundry")).cloned(),
                },
            );
        }
        for (dep_name, binding) in &env.dependencies {
            entries.insert(
                dep_name.clone(),
                BindingEntry {
                    name: dep_name.clone(),
                    kind: BindingKind::Declared(binding.kind),
                    physical_name: binding.physical_name(),
                    declared: Some(binding.clone()),
                    resolved: cache.and_then(|c| c.get(dep_name)).cloned(),
                },
            );
        }

        EnvBindings {
            env: name.to_string(),
            entries,
        }
    }

    /// Like [`EnvBindings::of_env`], for callers that have a [`Workspace`]
    /// in hand. Loads nothing itself — pass an already-loaded
    /// [`BindingCache`] as `cache`.
    pub fn of(
        _ws: &Workspace,
        env_name: &str,
        env: &Environment,
        cache: Option<&BindingCache>,
    ) -> EnvBindings {
        Self::of_env(env_name, env, cache)
    }

    pub fn get(&self, name: &str) -> Option<&BindingEntry> {
        self.entries.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &BindingEntry> {
        self.entries.values()
    }

    /// The entry whose kind accepts `wanted` and whose physical name equals
    /// `physical` (case-insensitive).
    pub fn find_physical(&self, wanted: Wanted, physical: &str) -> Option<&BindingEntry> {
        let physical = physical.to_lowercase();
        self.entries
            .values()
            .find(|e| e.physical_name == physical && wanted.matches(e.kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{FoundryConnection, SearchConnection, WORKSPACE_FILE};

    #[test]
    fn binding_value_forms_and_physical_names() {
        let name = Binding {
            kind: BindingType::Storage,
            value: "MKLabStorage".into(),
        };
        assert!(matches!(name.value(), BindingValue::Name(_)));
        assert_eq!(name.physical_name(), "mklabstorage");

        let id = Binding {
            kind: BindingType::Storage,
            value: "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct".into(),
        };
        assert_eq!(id.physical_name(), "acct");
        assert_eq!(arm_subscription(id.arm_id().unwrap()), Some("s"));
        assert_eq!(arm_resource_group(id.arm_id().unwrap()), Some("rg"));

        let api = Binding {
            kind: BindingType::Api,
            value: "https://Api.Partner.example/v1/".into(),
        };
        assert!(matches!(api.value(), BindingValue::Url(_)));
        assert_eq!(api.physical_name(), "api.partner.example");
    }

    #[test]
    fn env_bindings_include_implicit_search_and_foundry_and_match_model_hosts() {
        let env = Environment {
            search: Some(SearchConnection {
                service: "mklabsrch".into(),
                ..Default::default()
            }),
            foundry: Some(FoundryConnection {
                account: "mklabaifndr".into(),
                project: "p".into(),
                ..Default::default()
            }),
            dependencies: [(
                "enrichment".to_string(),
                Binding {
                    kind: BindingType::AiServices,
                    value: "mklabaisrvc".into(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let b = EnvBindings::of_env("dev", &env, None);
        assert_eq!(b.get("search").unwrap().physical_name, "mklabsrch");
        assert!(matches!(
            b.get("foundry").unwrap().kind,
            BindingKind::ImplicitFoundry
        ));
        assert_eq!(
            b.find_physical(Wanted::ModelHost, "MKLABAIFNDR")
                .unwrap()
                .name,
            "foundry"
        );
        assert_eq!(
            b.find_physical(Wanted::ModelHost, "mklabaisrvc")
                .unwrap()
                .name,
            "enrichment"
        );
        assert!(
            b.find_physical(Wanted::Type(BindingType::Storage), "x")
                .is_none()
        );
    }

    #[test]
    fn cache_round_trips_under_state_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(WORKSPACE_FILE),
            "environments:\n  dev:\n    default: true\n    search: { service: s }\n",
        )
        .unwrap();
        let ws = Workspace::load(tmp.path()).unwrap();
        let mut c = BindingCache::default();
        c.bindings.insert(
            "docs".into(),
            ResolvedBinding {
                name: "docs".into(),
                kind: BindingType::Storage,
                physical_name: "acct".into(),
                arm_id: Some(
                    "/subscriptions/s/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/acct"
                        .into(),
                ),
                subscription: Some("s".into()),
                resource_group: Some("rg".into()),
                location: Some("swedencentral".into()),
                endpoint: None,
                principal_id: None,
                resolved_at: "2026-09-10T00:00:00Z".into(),
            },
        );
        c.save(&ws, "dev").unwrap();
        assert!(BindingCache::path(&ws, "dev").ends_with(".rigg/dev/bindings.json"));
        assert_eq!(
            BindingCache::load(&ws, "dev")
                .get("docs")
                .unwrap()
                .resource_group
                .as_deref(),
            Some("rg")
        );
    }
}
