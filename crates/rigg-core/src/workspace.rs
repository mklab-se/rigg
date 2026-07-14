//! Workspace and project model.
//!
//! A workspace is a directory containing `rigg.yaml` (environments, service
//! connections, defaults), a `projects/` directory where each subdirectory
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
    #[error(
        "environment '{env}' has {count} {kind} connections; set `{kind}-connection` in project.yaml for project '{project}'"
    )]
    AmbiguousConnection {
        env: String,
        kind: &'static str,
        count: usize,
        project: String,
    },
    #[error("environment '{env}' has no {kind} connection (required by project '{project}')")]
    MissingConnection {
        env: String,
        kind: &'static str,
        project: String,
    },
    #[error(
        "project '{project}' pins {kind} connection '{name}' which does not exist in environment '{env}'"
    )]
    UnknownConnection {
        project: String,
        kind: &'static str,
        name: String,
        env: String,
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
    #[serde(default, skip_serializing_if = "Defaults::is_empty")]
    pub defaults: Defaults,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    /// Preferred managed-identity style for scaffolds: "user-assigned" | "system-assigned".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
}

impl Defaults {
    fn is_empty(&self) -> bool {
        self.identity.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub default: bool,
    #[serde(default, skip_serializing_if = "ConnectionList::is_empty")]
    pub search: ConnectionList<SearchConnection>,
    #[serde(default, skip_serializing_if = "ConnectionList::is_empty")]
    pub foundry: ConnectionList<FoundryConnection>,
    #[serde(default, skip_serializing_if = "Policy::is_default")]
    pub policy: Policy,
}

/// Per-environment policy gates. `protected: true` requires an explicit,
/// typed confirmation for every cloud-mutating operation against this
/// environment (`push` apply/`--prune`, `delete --remote`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    #[serde(default)]
    pub protected: bool,
}

impl Policy {
    fn is_default(&self) -> bool {
        self == &Policy::default()
    }
}

/// Accepts either a single mapping or a list of mappings in YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConnectionList<T> {
    One(T),
    Many(Vec<T>),
}

impl<T> Default for ConnectionList<T> {
    fn default() -> Self {
        ConnectionList::Many(Vec::new())
    }
}

impl<T> ConnectionList<T> {
    pub fn as_slice(&self) -> &[T] {
        match self {
            ConnectionList::One(one) => std::slice::from_ref(one),
            ConnectionList::Many(many) => many.as_slice(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Pin to a named search connection when the environment defines several.
    #[serde(
        default,
        rename = "search-connection",
        skip_serializing_if = "Option::is_none"
    )]
    pub search_connection: Option<String>,
    #[serde(
        default,
        rename = "foundry-connection",
        skip_serializing_if = "Option::is_none"
    )]
    pub foundry_connection: Option<String>,
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
            serde_yaml::from_str(&text).map_err(|source| WorkspaceError::Parse { path, source })?;

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
    pub fn search_for(&self, project: &Project) -> Result<&SearchConnection> {
        pick_connection(
            &self.name,
            "search",
            self.env.search.as_slice(),
            project,
            project.manifest.search_connection.as_deref(),
            |c| c.name.as_deref(),
        )
    }

    pub fn foundry_for(&self, project: &Project) -> Result<&FoundryConnection> {
        pick_connection(
            &self.name,
            "foundry",
            self.env.foundry.as_slice(),
            project,
            project.manifest.foundry_connection.as_deref(),
            |c| c.name.as_deref(),
        )
    }

    pub fn has_search(&self) -> bool {
        !self.env.search.is_empty()
    }

    pub fn has_foundry(&self) -> bool {
        !self.env.foundry.is_empty()
    }

    /// Whether this environment's policy gates cloud-mutating operations.
    pub fn protected(&self) -> bool {
        self.env.policy.protected
    }
}

fn pick_connection<'a, T>(
    env_name: &str,
    kind: &'static str,
    conns: &'a [T],
    project: &Project,
    pinned: Option<&str>,
    name_of: impl Fn(&T) -> Option<&str>,
) -> Result<&'a T> {
    match (pinned, conns.len()) {
        (_, 0) => Err(WorkspaceError::MissingConnection {
            env: env_name.to_string(),
            kind,
            project: project.name.clone(),
        }),
        (Some(pin), _) => conns
            .iter()
            .find(|c| name_of(c) == Some(pin))
            .ok_or_else(|| WorkspaceError::UnknownConnection {
                project: project.name.clone(),
                kind,
                name: pin.to_string(),
                env: env_name.to_string(),
            }),
        (None, 1) => Ok(&conns[0]),
        (None, n) => Err(WorkspaceError::AmbiguousConnection {
            env: env_name.to_string(),
            kind,
            count: n,
            project: project.name.clone(),
        }),
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

    fn ws_yaml_multi() -> &'static str {
        r#"
environments:
  dev:
    default: true
    search:
      - name: primary
        service: srch-a
      - name: secondary
        service: srch-b
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
        let p1 = ws.project("p1").unwrap();
        assert_eq!(dev.search_for(p1).unwrap().service, "mklabsrch");
        let f = dev.foundry_for(p1).unwrap();
        assert_eq!(
            (f.account.as_str(), f.project.as_str()),
            ("mklabaifndr", "proj-default")
        );
    }

    #[test]
    fn parses_multi_connection_env_and_requires_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(
            tmp.path(),
            ws_yaml_multi(),
            &[
                ("pinned", "search-connection: secondary\n"),
                ("unpinned", "{}\n"),
            ],
        );
        let dev = ws.resolve_env(None).unwrap();
        let pinned = ws.project("pinned").unwrap();
        assert_eq!(dev.search_for(pinned).unwrap().service, "srch-b");
        let unpinned = ws.project("unpinned").unwrap();
        assert!(matches!(
            dev.search_for(unpinned),
            Err(WorkspaceError::AmbiguousConnection { count: 2, .. })
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

    #[test]
    fn missing_connection_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = make_ws(
            tmp.path(),
            "environments:\n  dev:\n    default: true\n    search: { service: s }\n",
            &[("p", "{}\n")],
        );
        let dev = ws.resolve_env(None).unwrap();
        let p = ws.project("p").unwrap();
        assert!(matches!(
            dev.foundry_for(p),
            Err(WorkspaceError::MissingConnection {
                kind: "foundry",
                ..
            })
        ));
    }
}
