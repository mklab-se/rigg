//! Workspace and project model.
//!
//! A workspace is a directory containing `rigg.yaml` (environments, each with
//! a single search/foundry target, tenant/subscription, policy and
//! dependency bindings), a `projects/` directory where each subdirectory
//! with a `project.yaml` is a project, and an `apis/` directory for shared
//! OpenAPI specifications. Resource definitions live inside project
//! directories; a resource belongs to exactly one project.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const WORKSPACE_FILE: &str = "rigg.yaml";
pub const PROJECT_FILE: &str = "project.yaml";
pub const PROJECTS_DIR: &str = "projects";
pub const APIS_DIR: &str = "apis";
pub const STATE_DIR: &str = ".rigg";
/// Subdirectory of a project holding one tree per environment:
/// `projects/<project>/envs/<env>/{search,foundry}/...`.
pub const ENVS_DIR: &str = "envs";

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("no {WORKSPACE_FILE} found in {0} or any parent directory")]
    NotFound(PathBuf),
    #[error("failed to read {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    #[error("unknown project '{0}' (available: {1})")]
    UnknownProject(String, String),
    #[error("unknown environment '{0}' (available: {1})")]
    UnknownEnvironment(String, String),
    #[error(
        "no default environment configured; pass --env or set `default: true` on one environment"
    )]
    NoDefaultEnvironment,
    #[error("environment '{env}' has an invalid dependency binding name '{name}': {reason}")]
    InvalidBindingName {
        path: PathBuf,
        env: String,
        name: String,
        reason: String,
    },
}

type Result<T> = std::result::Result<T, WorkspaceError>;

/// Top-level `rigg.yaml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Directory (relative to rigg.yaml) holding rigg's file trees —
    /// `projects/`, `apis/`, `.rigg/`. Default: alongside rigg.yaml.
    /// Set by `rigg init <folder>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default)]
    pub environments: BTreeMap<String, Environment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
    /// Azure AD tenant this environment's resources live in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Azure subscription this environment's resources live in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub search: Option<SearchConnection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foundry: Option<FoundryConnection>,
    #[serde(default, skip_serializing_if = "Policy::is_default")]
    pub policy: Policy,
    /// Named references to supporting resources outside rigg's own kinds
    /// (storage accounts, function apps, key vaults, ...).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, Binding>,
}

/// Per-environment policy gates. `protected: true` requires an explicit,
/// typed confirmation for every cloud-mutating operation against this
/// environment (`push` apply/`--prune`, `delete --remote`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub protected: bool,
    /// Require every `dependencies` binding to be resolvable before push.
    /// Defaults to `protected` when unset.
    #[serde(
        default,
        rename = "strict-bindings",
        skip_serializing_if = "Option::is_none"
    )]
    pub strict_bindings: Option<bool>,
}

impl Policy {
    fn is_default(&self) -> bool {
        self == &Policy::default()
    }

    pub fn strict_bindings(&self) -> bool {
        self.strict_bindings.unwrap_or(self.protected)
    }
}

/// `Binding`/`BindingType` moved to [`crate::binding`]; re-exported here so
/// existing `workspace::Binding` paths keep compiling.
pub use crate::binding::{Binding, BindingType, validate_binding_name};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchConnection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Azure AI Search service name (e.g. `mklabsrch`).
    pub service: String,
    /// Full endpoint override (sovereign clouds, testing). Default:
    /// `https://{service}.search.windows.net`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Override for the stable data-plane api-version.
    #[serde(
        default,
        rename = "api-version",
        skip_serializing_if = "Option::is_none"
    )]
    pub api_version: Option<String>,
    /// Override for the preview data-plane api-version.
    #[serde(
        default,
        rename = "preview-api-version",
        skip_serializing_if = "Option::is_none"
    )]
    pub preview_api_version: Option<String>,
}

impl SearchConnection {
    /// Base URL requests go to: the `endpoint` override, or the public-cloud
    /// default derived from the service name.
    pub fn url(&self) -> String {
        match &self.endpoint {
            Some(e) => e.trim_end_matches('/').to_string(),
            None => format!("https://{}.search.windows.net", self.service),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoundryConnection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Foundry account name (e.g. `mklabaifndr`).
    pub account: String,
    /// Full endpoint override (sovereign clouds, testing). Default:
    /// `https://{account}.services.ai.azure.com`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Foundry project name (e.g. `proj-default`).
    pub project: String,
    /// Override for the data-plane api-version (default `v1`).
    #[serde(
        default,
        rename = "api-version",
        skip_serializing_if = "Option::is_none"
    )]
    pub api_version: Option<String>,
}

impl FoundryConnection {
    /// Base URL requests go to: the `endpoint` override, or the public-cloud
    /// default derived from the account name.
    pub fn url(&self) -> String {
        match &self.endpoint {
            Some(e) => e.trim_end_matches('/').to_string(),
            None => format!("https://{}.services.ai.azure.com", self.account),
        }
    }
}

/// `project.yaml` — metadata only; the directory contents are the membership.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub dir: PathBuf,
    pub manifest: ProjectManifest,
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub config: WorkspaceConfig,
    pub projects: Vec<Project>,
}

#[derive(Debug, Clone)]
pub struct ResolvedEnv {
    pub name: String,
    pub env: Environment,
}

impl Workspace {
    /// Walk up from `start` to the directory containing `rigg.yaml`, then load
    /// the workspace config and scan `projects/*/project.yaml`.
    pub fn discover(start: &Path) -> Result<Workspace> {
        let start = if start.as_os_str().is_empty() {
            Path::new(".")
        } else {
            start
        };
        let mut dir = start.canonicalize().map_err(|source| WorkspaceError::Io {
            path: start.to_path_buf(),
            source,
        })?;
        loop {
            if dir.join(WORKSPACE_FILE).is_file() {
                return Workspace::load(&dir);
            }
            if !dir.pop() {
                return Err(WorkspaceError::NotFound(start.to_path_buf()));
            }
        }
    }

    /// Load a workspace whose root is known to contain `rigg.yaml`.
    pub fn load(root: &Path) -> Result<Workspace> {
        let path = root.join(WORKSPACE_FILE);
        let text = std::fs::read_to_string(&path).map_err(|source| WorkspaceError::Io {
            path: path.clone(),
            source,
        })?;
        let config: WorkspaceConfig =
            serde_yaml::from_str(&text).map_err(|source| WorkspaceError::Parse {
                path: path.clone(),
                source,
            })?;

        for (env_name, env) in &config.environments {
            for name in env.dependencies.keys() {
                if let Err(reason) = validate_binding_name(name) {
                    return Err(WorkspaceError::InvalidBindingName {
                        path: path.clone(),
                        env: env_name.clone(),
                        name: name.clone(),
                        reason,
                    });
                }
            }
        }

        let files_root = match &config.root {
            Some(sub) => root.join(sub),
            None => root.to_path_buf(),
        };
        let mut projects = Vec::new();
        let projects_dir = files_root.join(PROJECTS_DIR);
        if projects_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&projects_dir)
                .map_err(|source| WorkspaceError::Io {
                    path: projects_dir.clone(),
                    source,
                })?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir() && p.join(PROJECT_FILE).is_file())
                .collect();
            entries.sort();
            for dir in entries {
                let manifest_path = dir.join(PROJECT_FILE);
                let text = std::fs::read_to_string(&manifest_path).map_err(|source| {
                    WorkspaceError::Io {
                        path: manifest_path.clone(),
                        source,
                    }
                })?;
                let manifest: ProjectManifest =
                    serde_yaml::from_str(&text).map_err(|source| WorkspaceError::Parse {
                        path: manifest_path,
                        source,
                    })?;
                let name = dir
                    .file_name()
                    .expect("project dir has a name")
                    .to_string_lossy()
                    .into_owned();
                projects.push(Project {
                    name,
                    dir,
                    manifest,
                });
            }
        }

        Ok(Workspace {
            root: root.to_path_buf(),
            config,
            projects,
        })
    }

    pub fn project(&self, name: &str) -> Result<&Project> {
        self.projects
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| {
                WorkspaceError::UnknownProject(
                    name.to_string(),
                    self.projects
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                )
            })
    }

    /// Resolve an environment: explicit selection > `RIGG_ENV` > `default: true`.
    pub fn resolve_env(&self, selected: Option<&str>) -> Result<ResolvedEnv> {
        let from_env = std::env::var("RIGG_ENV").ok();
        let name = selected
            .map(str::to_string)
            .or(from_env)
            .or_else(|| self.default_env_name().map(str::to_string))
            .ok_or(WorkspaceError::NoDefaultEnvironment)?;
        let env = self.config.environments.get(&name).ok_or_else(|| {
            WorkspaceError::UnknownEnvironment(
                name.clone(),
                self.config
                    .environments
                    .keys()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
            )
        })?;
        Ok(ResolvedEnv {
            name,
            env: env.clone(),
        })
    }

    pub fn default_env_name(&self) -> Option<&str> {
        self.config
            .environments
            .iter()
            .find(|(_, e)| e.default)
            .map(|(n, _)| n.as_str())
    }

    /// Directory holding rigg's file trees (`projects/`, `apis/`, `.rigg/`) —
    /// the workspace root unless `root:` in rigg.yaml relocates them.
    pub fn files_root(&self) -> PathBuf {
        match &self.config.root {
            Some(sub) => self.root.join(sub),
            None => self.root.clone(),
        }
    }

    pub fn apis_dir(&self) -> PathBuf {
        self.files_root().join(APIS_DIR)
    }

    pub fn state_dir(&self, env: &str, project: &str) -> PathBuf {
        self.files_root().join(STATE_DIR).join(env).join(project)
    }
}

impl ResolvedEnv {
    pub fn search(&self) -> Option<&SearchConnection> {
        self.env.search.as_ref()
    }

    pub fn foundry(&self) -> Option<&FoundryConnection> {
        self.env.foundry.as_ref()
    }

    /// Whether this environment's policy gates cloud-mutating operations.
    pub fn protected(&self) -> bool {
        self.env.policy.protected
    }

    /// Whether every `dependencies` binding must resolve before push.
    pub fn strict_bindings(&self) -> bool {
        self.env.policy.strict_bindings()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_yaml_single() -> &'static str {
        r#"
name: demo
environments:
  dev:
    default: true
    search: { service: mklabsrch }
    foundry: { account: mklabaifndr, project: proj-default }
  prod:
    search: { service: mklabsrch-prod, api-version: 2026-04-01 }
"#
    }

    fn ws_yaml_bindings() -> &'static str {
        r#"
environments:
  dev:
    default: true
    tenant: 45943588-b4fb-4765-ae17-76638c45bb5c
    subscription: fa354123-c4ee-4b2e-a700-bf01decf803a
    search: { service: mklabsrch }
    foundry: { account: mklabaifndr, project: proj-default }
    dependencies:
      docs-storage: { storage: mklabstorageacc }
      enrich-fn: { function-app: mklab }
      partner: { api: https://api.partner.example/v1 }
  prod:
    policy: { protected: true }
    search: { service: mklabsrch-prod }
    dependencies:
      docs-storage: { storage: /subscriptions/0b1d/resourceGroups/rg/providers/Microsoft.Storage/storageAccounts/mklabstorageprod }
"#
    }

    fn make_ws(dir: &Path, yaml: &str, projects: &[(&str, &str)]) -> Workspace {
        std::fs::write(dir.join(WORKSPACE_FILE), yaml).unwrap();
        for (name, manifest) in projects {
            let pdir = dir.join(PROJECTS_DIR).join(name);
            std::fs::create_dir_all(&pdir).unwrap();
            std::fs::write(pdir.join(PROJECT_FILE), manifest).unwrap();
        }
        Workspace::load(dir).unwrap()
    }

    #[test]
    fn parses_single_connection_env() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(
            tmp.path(),
            ws_yaml_single(),
            &[("p1", "description: test\n")],
        );
        let dev = ws.resolve_env(Some("dev")).unwrap();
        assert_eq!(dev.search().unwrap().service, "mklabsrch");
        let f = dev.foundry().unwrap();
        assert_eq!(
            (f.account.as_str(), f.project.as_str()),
            ("mklabaifndr", "proj-default")
        );
    }

    #[test]
    fn parses_targets_tenant_subscription_and_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(tmp.path(), ws_yaml_bindings(), &[("p", "{}\n")]);
        let dev = ws.resolve_env(Some("dev")).unwrap();
        assert_eq!(dev.search().unwrap().service, "mklabsrch");
        assert_eq!(dev.foundry().unwrap().project, "proj-default");
        assert_eq!(
            dev.env.subscription.as_deref(),
            Some("fa354123-c4ee-4b2e-a700-bf01decf803a")
        );
        let b = &dev.env.dependencies["docs-storage"];
        assert_eq!(b.kind, BindingType::Storage);
        assert_eq!(b.value, "mklabstorageacc");
        assert_eq!(dev.env.dependencies["partner"].kind, BindingType::Api);
        let prod = ws.resolve_env(Some("prod")).unwrap();
        assert!(prod.foundry().is_none());
        assert!(
            prod.protected() && prod.strict_bindings(),
            "strict-bindings defaults to protected"
        );
        assert!(!dev.strict_bindings());
    }

    #[test]
    fn binding_round_trips_as_a_one_key_map() {
        let b: Binding = serde_yaml::from_str("key-vault: mklabkv").unwrap();
        assert_eq!(b.kind, BindingType::KeyVault);
        assert_eq!(
            serde_yaml::to_string(&b).unwrap().trim(),
            "key-vault: mklabkv"
        );
        assert!(
            serde_yaml::from_str::<Binding>("cosmos: x").is_err(),
            "unknown type rejected"
        );
        assert!(
            serde_yaml::from_str::<Binding>("storage: a\nidentity: b").is_err(),
            "exactly one key"
        );
    }

    #[test]
    fn binding_names_are_validated_and_reserved() {
        assert!(validate_binding_name("docs-storage").is_ok());
        assert!(validate_binding_name("Docs").is_err());
        assert!(validate_binding_name("search").is_err());
        assert!(validate_binding_name("foundry").is_err());
    }

    #[test]
    fn list_form_targets_are_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join(WORKSPACE_FILE),
            "environments:\n  dev:\n    search:\n      - service: a\n",
        )
        .unwrap();
        assert!(matches!(
            Workspace::load(tmp.path()),
            Err(WorkspaceError::Parse { .. })
        ));
    }

    #[test]
    fn env_resolution_precedence() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(tmp.path(), ws_yaml_single(), &[]);
        // explicit wins
        assert_eq!(ws.resolve_env(Some("prod")).unwrap().name, "prod");
        // default used when nothing selected (RIGG_ENV not set in tests)
        assert_eq!(ws.resolve_env(None).unwrap().name, "dev");
        // unknown errors
        assert!(matches!(
            ws.resolve_env(Some("staging")),
            Err(WorkspaceError::UnknownEnvironment(..))
        ));
    }

    #[test]
    fn no_default_env_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(
            tmp.path(),
            "environments:\n  a:\n    search: { service: s }\n",
            &[],
        );
        assert!(matches!(
            ws.resolve_env(None),
            Err(WorkspaceError::NoDefaultEnvironment)
        ));
    }

    #[test]
    fn root_setting_relocates_file_trees() {
        let tmp = tempfile::tempdir().unwrap();
        let yaml =
            "root: rag\nenvironments:\n  dev:\n    default: true\n    search: { service: s }\n";
        std::fs::write(tmp.path().join(WORKSPACE_FILE), yaml).unwrap();
        let pdir = tmp.path().join("rag").join(PROJECTS_DIR).join("alpha");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join(PROJECT_FILE), "{}\n").unwrap();
        let ws = Workspace::load(tmp.path()).unwrap();
        assert_eq!(ws.root, tmp.path());
        assert_eq!(ws.files_root(), tmp.path().join("rag"));
        assert_eq!(ws.project("alpha").unwrap().dir, pdir);
        assert_eq!(ws.apis_dir(), tmp.path().join("rag").join(APIS_DIR));
        assert_eq!(
            ws.state_dir("dev", "alpha"),
            tmp.path()
                .join("rag")
                .join(STATE_DIR)
                .join("dev")
                .join("alpha")
        );
    }

    #[test]
    fn discover_walks_up_and_finds_projects() {
        let tmp = tempfile::tempdir().unwrap();
        make_ws(
            tmp.path(),
            ws_yaml_single(),
            &[("alpha", "{}\n"), ("beta", "description: b\n")],
        );
        let nested = tmp.path().join(PROJECTS_DIR).join("alpha").join("search");
        std::fs::create_dir_all(&nested).unwrap();
        let ws = Workspace::discover(&nested).unwrap();
        assert_eq!(
            ws.projects
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert!(matches!(
            ws.project("gamma"),
            Err(WorkspaceError::UnknownProject(..))
        ));
    }

    #[test]
    fn policy_protected_parses_and_defaults_unprotected() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(
            tmp.path(),
            "environments:\n  dev:\n    default: true\n    search: { service: s }\n  prod:\n    policy: { protected: true }\n    search: { service: p }\n",
            &[],
        );
        assert!(!ws.resolve_env(Some("dev")).unwrap().protected());
        assert!(ws.resolve_env(Some("prod")).unwrap().protected());
    }
}
