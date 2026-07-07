//! Project-scoped resource file store and sync-state classification.
//!
//! Layout inside a project directory:
//!
//! ```text
//! projects/<name>/
//!   project.yaml
//!   search/<kind-dir>/<resource-name>.json
//!   foundry/<kind-dir>/<resource-name>.json
//! ```
//!
//! Files are written via [`crate::normalize::normalize_for_disk`] and long
//! text fields are extracted to Markdown sidecars ([`crate::sidecar`]).
//! Baselines (`.rigg/<env>/<project>/state.json`) hold the checksum of each
//! resource at last sync, enabling local/remote/conflict classification.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::normalize::{format_json, normalize_for_disk, normalize_for_push};
use crate::resources::traits::{ResourceKind, ResourceRef, validate_resource_name};
use crate::service::ServiceDomain;
use crate::sidecar::{self, SidecarError};
use crate::workspace::{Project, Workspace};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error(transparent)]
    Sidecar(#[from] SidecarError),
    #[error("invalid resource name in {path}: {message}")]
    BadName { path: PathBuf, message: String },
    #[error(
        "resource {reference} is defined in both project '{first}' and project '{second}' — a resource must belong to exactly one project"
    )]
    DuplicateOwnership {
        reference: String,
        first: String,
        second: String,
    },
}

type Result<T> = std::result::Result<T, StoreError>;

/// File store for one project.
pub struct Store<'w> {
    project: &'w Project,
}

impl<'w> Store<'w> {
    pub fn new(project: &'w Project) -> Self {
        Store { project }
    }

    pub fn project(&self) -> &Project {
        self.project
    }

    fn domain_dir(domain: ServiceDomain) -> &'static str {
        match domain {
            ServiceDomain::Search => "search",
            ServiceDomain::Foundry => "foundry",
        }
    }

    /// Absolute path for a resource file.
    pub fn path_for(&self, r: &ResourceRef) -> PathBuf {
        self.project
            .dir
            .join(Self::domain_dir(r.kind.domain()))
            .join(r.kind.directory_name())
            .join(format!("{}.json", r.name))
    }

    /// Scan the project directory for resource files.
    pub fn list(&self) -> Result<Vec<(ResourceRef, PathBuf)>> {
        let mut out = Vec::new();
        for kind in ResourceKind::all() {
            let dir = self
                .project
                .dir
                .join(Self::domain_dir(kind.domain()))
                .join(kind.directory_name());
            if !dir.is_dir() {
                continue;
            }
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
                .map_err(|source| StoreError::Io {
                    path: dir.clone(),
                    source,
                })?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            entries.sort();
            for path in entries {
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                validate_resource_name(&name).map_err(|e| StoreError::BadName {
                    path: path.clone(),
                    message: e.to_string(),
                })?;
                out.push((ResourceRef::new(*kind, name), path));
            }
        }
        Ok(out)
    }

    /// Read a resource file with sidecars inlined.
    pub fn read(&self, r: &ResourceRef) -> Result<Value> {
        let path = self.path_for(r);
        self.read_path(&path)
    }

    /// Read any resource file (must belong to this project) with sidecars inlined.
    pub fn read_path(&self, path: &Path) -> Result<Value> {
        let text = std::fs::read_to_string(path).map_err(|source| StoreError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let mut value: Value = serde_json::from_str(&text).map_err(|source| StoreError::Parse {
            path: path.to_path_buf(),
            source,
        })?;
        sidecar::inline_sidecars(path, &mut value)?;
        Ok(value)
    }

    /// Write a resource: normalize for disk, extract sidecars, write only if
    /// the semantic content changed. Returns true if the file was (re)written.
    pub fn write(&self, r: &ResourceRef, value: &Value) -> Result<bool> {
        let path = self.path_for(r);
        let mut normalized = normalize_for_disk(r.kind, value);

        // Preserve any x-rigg-* annotations the user added locally: they are
        // Rigg-local and never come back from Azure.
        if path.is_file() {
            if let Ok(existing) = self.read_path(&path) {
                carry_over_x_rigg(&existing, &mut normalized);
                if crate::normalize::semantic_eq(r.kind, &existing, &normalized) {
                    return Ok(false);
                }
            }
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| StoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        sidecar::extract_sidecars(r.kind, &path, &mut normalized)?;
        std::fs::write(&path, format_json(&normalized)).map_err(|source| StoreError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(true)
    }

    /// Delete a resource file (and its default sidecars).
    pub fn delete(&self, r: &ResourceRef) -> Result<()> {
        let path = self.path_for(r);
        if path.is_file() {
            std::fs::remove_file(&path).map_err(|source| StoreError::Io {
                path: path.clone(),
                source,
            })?;
        }
        // Remove default sidecars (e.g. `<name>.instructions.md`).
        if let Some(dir) = path.parent() {
            for field in crate::registry::meta(r.kind).sidecar_fields {
                let sidecar = dir.join(format!("{}.{}.md", r.name, field));
                if sidecar.is_file() {
                    let _ = std::fs::remove_file(sidecar);
                }
            }
        }
        Ok(())
    }
}

/// Copy `x-rigg-*` keys from `from` into `to` at the same paths (top-level and
/// one structural match deep for arrays keyed by `name`/`type`).
fn carry_over_x_rigg(from: &Value, to: &mut Value) {
    match (from, to) {
        (Value::Object(src), Value::Object(dst)) => {
            for (k, v) in src {
                if k.starts_with("x-rigg-") {
                    dst.entry(k.clone()).or_insert_with(|| v.clone());
                } else if let Some(dv) = dst.get_mut(k) {
                    carry_over_x_rigg(v, dv);
                }
            }
        }
        (Value::Array(src), Value::Array(dst)) => {
            for sv in src {
                let key = sv.get("name").or_else(|| sv.get("type"));
                if let Some(key) = key {
                    if let Some(dv) = dst
                        .iter_mut()
                        .find(|d| d.get("name").or_else(|| d.get("type")) == Some(key))
                    {
                        carry_over_x_rigg(sv, dv);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Enforce exclusive ownership: a (kind, name) may appear in only one project.
pub fn assert_exclusive_ownership(ws: &Workspace) -> Result<()> {
    let mut seen: BTreeMap<ResourceRef, &str> = BTreeMap::new();
    for project in &ws.projects {
        let store = Store::new(project);
        for (r, _) in store.list()? {
            if let Some(first) = seen.get(&r) {
                return Err(StoreError::DuplicateOwnership {
                    reference: r.to_string(),
                    first: first.to_string(),
                    second: project.name.clone(),
                });
            }
            seen.insert(r, &project.name);
        }
    }
    Ok(())
}

/// Sync classification of one resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SyncClass {
    /// Local, remote and baseline all agree.
    InSync,
    /// Local changed since last sync; remote unchanged.
    LocalAhead,
    /// Remote changed since last sync; local unchanged.
    RemoteAhead,
    /// Both changed since last sync.
    Conflict,
    /// Exists locally, not remotely (new resource or remote-deleted).
    LocalOnly,
    /// Exists remotely, not locally (unmanaged or locally-deleted).
    RemoteOnly,
    /// No baseline; local and remote both exist but differ (never synced).
    Untracked,
}

/// Per-project, per-environment sync baselines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectState {
    /// `kind-dir/name` → checksum of the push-normalized JSON at last sync.
    #[serde(default)]
    pub baselines: BTreeMap<String, String>,
}

impl ProjectState {
    pub fn path(ws: &Workspace, env: &str, project: &str) -> PathBuf {
        ws.state_dir(env, project).join("state.json")
    }

    pub fn load(ws: &Workspace, env: &str, project: &str) -> ProjectState {
        let path = Self::path(ws, env, project);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, ws: &Workspace, env: &str, project: &str) -> std::io::Result<()> {
        let path = Self::path(ws, env, project);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, format_json(&serde_json::to_value(self).unwrap()))
    }

    /// Checksum of the push-normalized form of a document.
    ///
    /// The form is canonicalized (object keys sorted recursively, arrays of
    /// named objects sorted by name) so that server-side reordering between
    /// GET and PUT responses never reads as a change — matching the semantics
    /// of the order-insensitive diff.
    pub fn checksum(kind: ResourceKind, value: &Value) -> String {
        let normalized = canonical_form(&normalize_for_push(kind, value));
        let canonical = serde_json::to_string(&normalized).unwrap_or_default();
        format!("{:x}", md5_like(&canonical))
    }

    pub fn baseline(&self, r: &ResourceRef) -> Option<&str> {
        self.baselines.get(&r.key()).map(String::as_str)
    }

    pub fn set_baseline(&mut self, r: &ResourceRef, kind_value: &Value) {
        self.baselines
            .insert(r.key(), Self::checksum(r.kind, kind_value));
    }

    pub fn clear_baseline(&mut self, r: &ResourceRef) {
        self.baselines.remove(&r.key());
    }

    /// Classify a resource given its (optional) local and remote documents.
    pub fn classify(
        &self,
        r: &ResourceRef,
        local: Option<&Value>,
        remote: Option<&Value>,
    ) -> SyncClass {
        let baseline = self.baseline(r);
        match (local, remote) {
            (None, None) => SyncClass::InSync, // nothing anywhere (only baseline leftover)
            (Some(_), None) => SyncClass::LocalOnly,
            (None, Some(_)) => SyncClass::RemoteOnly,
            (Some(l), Some(rm)) => {
                let lsum = Self::checksum(r.kind, l);
                let rsum = Self::checksum(r.kind, rm);
                match baseline {
                    None => {
                        if lsum == rsum {
                            SyncClass::InSync
                        } else {
                            SyncClass::Untracked
                        }
                    }
                    Some(base) => {
                        let local_changed = lsum != base;
                        let remote_changed = rsum != base;
                        match (local_changed, remote_changed) {
                            (false, false) => SyncClass::InSync,
                            (true, false) => SyncClass::LocalAhead,
                            (false, true) => SyncClass::RemoteAhead,
                            (true, true) => {
                                if lsum == rsum {
                                    // Both moved to the same content.
                                    SyncClass::InSync
                                } else {
                                    SyncClass::Conflict
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Order-canonical JSON: object keys sorted recursively; arrays whose items
/// all carry a string `name` are sorted by it (identity-keyed arrays).
fn canonical_form(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            // null-valued keys are dropped: Azure oscillates between omitting
            // a field and returning it as null depending on the endpoint.
            let mut sorted: Vec<(String, Value)> = map
                .iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), canonical_form(v)))
                .collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(arr) => {
            let mut items: Vec<Value> = arr.iter().map(canonical_form).collect();
            if !items.is_empty()
                && items
                    .iter()
                    .all(|i| i.get("name").and_then(Value::as_str).is_some())
            {
                items.sort_by(|a, b| {
                    a["name"]
                        .as_str()
                        .unwrap_or_default()
                        .cmp(b["name"].as_str().unwrap_or_default())
                });
            }
            Value::Array(items)
        }
        other => other.clone(),
    }
}

/// Small non-cryptographic checksum (FNV-1a 128-ish via two 64-bit lanes).
/// Collision resistance is ample for change detection.
fn md5_like(s: &str) -> u128 {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x9e3779b97f4a7c15;
    for b in s.as_bytes() {
        h1 ^= u64::from(*b);
        h1 = h1.wrapping_mul(0x100000001b3);
        h2 = h2.rotate_left(5) ^ u64::from(*b);
        h2 = h2.wrapping_mul(0x2545f4914f6cdd1d);
    }
    (u128::from(h1) << 64) | u128::from(h2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{PROJECT_FILE, PROJECTS_DIR, WORKSPACE_FILE};
    use serde_json::json;

    fn ws_with_projects(dir: &Path, names: &[&str]) -> Workspace {
        std::fs::write(
            dir.join(WORKSPACE_FILE),
            "environments:\n  dev:\n    default: true\n    search: { service: s }\n",
        )
        .unwrap();
        for name in names {
            let pdir = dir.join(PROJECTS_DIR).join(name);
            std::fs::create_dir_all(&pdir).unwrap();
            std::fs::write(pdir.join(PROJECT_FILE), "{}\n").unwrap();
        }
        Workspace::load(dir).unwrap()
    }

    #[test]
    fn write_list_read_round_trip_with_sidecars() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = ws_with_projects(tmp.path(), &["p"]);
        let store = Store::new(ws.project("p").unwrap());

        let agent_ref = ResourceRef::new(ResourceKind::Agent, "helper");
        let agent = json!({"name": "helper", "model": "gpt-5-mini", "instructions": "Be nice."});
        assert!(store.write(&agent_ref, &agent).unwrap());

        // sidecar extracted
        let sidecar = store
            .path_for(&agent_ref)
            .parent()
            .unwrap()
            .join("helper.instructions.md");
        assert!(sidecar.is_file());

        // read inlines it back
        let read = store.read(&agent_ref).unwrap();
        assert_eq!(read["instructions"], json!("Be nice."));

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].0, agent_ref);
    }

    #[test]
    fn write_returns_false_when_semantically_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = ws_with_projects(tmp.path(), &["p"]);
        let store = Store::new(ws.project("p").unwrap());
        let r = ResourceRef::new(ResourceKind::Index, "idx");
        assert!(
            store
                .write(&r, &json!({"name": "idx", "fields": []}))
                .unwrap()
        );
        // same content + volatile noise → no rewrite
        let noisy = json!({"@odata.etag": "0x1", "name": "idx", "fields": []});
        assert!(!store.write(&r, &noisy).unwrap());
        // real change → rewrite
        let changed = json!({"name": "idx", "fields": [{"name": "f"}]});
        assert!(store.write(&r, &changed).unwrap());
    }

    #[test]
    fn write_preserves_local_x_rigg_annotations() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = ws_with_projects(tmp.path(), &["p"]);
        let store = Store::new(ws.project("p").unwrap());
        let r = ResourceRef::new(ResourceKind::Skillset, "sk");
        let local = json!({
            "name": "sk",
            "skills": [{"name": "web", "uri": "https://f", "x-rigg-api": "enrich"}]
        });
        store.write(&r, &local).unwrap();
        // Azure returns the same thing without the annotation
        let remote = json!({
            "name": "sk",
            "skills": [{"name": "web", "uri": "https://f"}]
        });
        let rewritten = store.write(&r, &remote).unwrap();
        let read = store.read(&r).unwrap();
        assert_eq!(read["skills"][0]["x-rigg-api"], json!("enrich"));
        assert!(!rewritten, "annotation-only delta is not a semantic change");
    }

    #[test]
    fn exclusive_ownership_violation_names_both_projects() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = ws_with_projects(tmp.path(), &["alpha", "beta"]);
        for p in ["alpha", "beta"] {
            let store = Store::new(ws.project(p).unwrap());
            store
                .write(
                    &ResourceRef::new(ResourceKind::Index, "shared"),
                    &json!({"name": "shared"}),
                )
                .unwrap();
        }
        let err = assert_exclusive_ownership(&ws).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("alpha") && msg.contains("beta") && msg.contains("indexes/shared"));
    }

    #[test]
    fn classify_truth_table() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = ws_with_projects(tmp.path(), &["p"]);
        let r = ResourceRef::new(ResourceKind::Index, "idx");
        let a = json!({"name": "idx", "fields": [{"name": "f1"}]});
        let b = json!({"name": "idx", "fields": [{"name": "f2"}]});
        let c = json!({"name": "idx", "fields": [{"name": "f3"}]});

        let mut state = ProjectState::default();
        // no baseline
        assert_eq!(state.classify(&r, Some(&a), Some(&a)), SyncClass::InSync);
        assert_eq!(state.classify(&r, Some(&a), Some(&b)), SyncClass::Untracked);
        assert_eq!(state.classify(&r, Some(&a), None), SyncClass::LocalOnly);
        assert_eq!(state.classify(&r, None, Some(&a)), SyncClass::RemoteOnly);
        assert_eq!(state.classify(&r, None, None), SyncClass::InSync);

        // with baseline = a
        state.set_baseline(&r, &a);
        assert_eq!(state.classify(&r, Some(&a), Some(&a)), SyncClass::InSync);
        assert_eq!(
            state.classify(&r, Some(&b), Some(&a)),
            SyncClass::LocalAhead
        );
        assert_eq!(
            state.classify(&r, Some(&a), Some(&b)),
            SyncClass::RemoteAhead
        );
        assert_eq!(state.classify(&r, Some(&b), Some(&c)), SyncClass::Conflict);
        assert_eq!(state.classify(&r, Some(&b), Some(&b)), SyncClass::InSync);

        // save/load round trip
        state.save(&ws, "dev", "p").unwrap();
        let loaded = ProjectState::load(&ws, "dev", "p");
        assert_eq!(loaded.baseline(&r), state.baseline(&r));
    }

    #[test]
    fn checksum_is_order_canonical() {
        // same content, different key order and array order
        let a = serde_json::from_str::<Value>(
            r#"{"name": "i", "fields": [{"name": "b"}, {"name": "a"}], "x": 1}"#,
        )
        .unwrap();
        let b = serde_json::from_str::<Value>(
            r#"{"x": 1, "name": "i", "fields": [{"name": "a"}, {"name": "b"}]}"#,
        )
        .unwrap();
        assert_eq!(
            ProjectState::checksum(ResourceKind::Index, &a),
            ProjectState::checksum(ResourceKind::Index, &b)
        );
    }

    #[test]
    fn checksum_ignores_volatile_and_annotations() {
        let a = json!({"name": "i", "@odata.etag": "1", "x-rigg-note": "hi"});
        let b = json!({"name": "i"});
        assert_eq!(
            ProjectState::checksum(ResourceKind::Index, &a),
            ProjectState::checksum(ResourceKind::Index, &b)
        );
    }
}
