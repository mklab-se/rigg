//! Dynamic tab-completion candidates, resolved from LOCAL workspace files.
//!
//! Registered per shell with one line (e.g. `source <(COMPLETE=zsh rigg)`);
//! the shell then invokes the rigg binary for candidates as the user types.
//! Everything here is offline: the workspace mirrors the cloud, so resource
//! names complete instantly from disk. Outside a workspace, completion
//! yields nothing, silently.

use clap_complete::engine::CompletionCandidate;
use rigg_core::registry;
use rigg_core::resources::ResourceKind;
use rigg_core::store::Store;
use rigg_core::workspace::Workspace;

fn workspace() -> Option<Workspace> {
    Workspace::discover(std::path::Path::new(".")).ok()
}

/// The environment completion should resolve against: `RIGG_ENV` when set,
/// else the workspace default.
fn env_name(ws: &Workspace) -> Option<String> {
    if let Ok(env) = std::env::var("RIGG_ENV") {
        if !env.is_empty() {
            return Some(env);
        }
    }
    ws.resolve_env(None).ok().map(|e| e.name)
}

fn candidates<I: IntoIterator<Item = String>>(values: I) -> Vec<CompletionCandidate> {
    let mut values: Vec<String> = values.into_iter().collect();
    values.sort();
    values.dedup();
    values.into_iter().map(CompletionCandidate::new).collect()
}

/// Project names in the workspace.
pub fn projects() -> Vec<CompletionCandidate> {
    let Some(ws) = workspace() else {
        return Vec::new();
    };
    candidates(ws.projects.iter().map(|p| p.name.clone()))
}

/// Environment names from rigg.yaml.
pub fn envs() -> Vec<CompletionCandidate> {
    let Some(ws) = workspace() else {
        return Vec::new();
    };
    candidates(ws.config.environments.keys().cloned())
}

/// Physical resource names of one kind across every project (resolved env).
pub fn resource_names(kind: ResourceKind) -> Vec<CompletionCandidate> {
    let Some(ws) = workspace() else {
        return Vec::new();
    };
    let Some(env) = env_name(&ws) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for project in &ws.projects {
        if let Ok(list) = Store::new(project, &env).list() {
            names.extend(
                list.into_iter()
                    .filter(|(r, _)| r.kind == kind)
                    .map(|(r, _)| r.name),
            );
        }
    }
    candidates(names)
}

/// `<kind-dir>/<name>` selectors across every kind and project.
pub fn selectors() -> Vec<CompletionCandidate> {
    let Some(ws) = workspace() else {
        return Vec::new();
    };
    let Some(env) = env_name(&ws) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for project in &ws.projects {
        if let Ok(list) = Store::new(project, &env).list() {
            keys.extend(list.into_iter().map(|(r, _)| r.key()));
        }
    }
    candidates(keys)
}

/// Kind-specific wrappers (clap's `add = ArgValueCandidates::new(f)` wants
/// plain functions).
pub fn indexers() -> Vec<CompletionCandidate> {
    resource_names(ResourceKind::Indexer)
}
pub fn indexes() -> Vec<CompletionCandidate> {
    resource_names(ResourceKind::Index)
}
pub fn knowledge_bases() -> Vec<CompletionCandidate> {
    resource_names(ResourceKind::KnowledgeBase)
}
pub fn knowledge_sources() -> Vec<CompletionCandidate> {
    resource_names(ResourceKind::KnowledgeSource)
}
pub fn agents() -> Vec<CompletionCandidate> {
    resource_names(ResourceKind::Agent)
}

/// Kinds accepted by `rigg new` (resource kinds + the special scaffolds).
pub fn new_kinds() -> Vec<CompletionCandidate> {
    let mut kinds: Vec<String> = registry::all_kinds()
        .iter()
        .map(|k| k.cli_name().to_string())
        .collect();
    kinds.extend([
        "project".to_string(),
        "pipeline".to_string(),
        "api".to_string(),
    ]);
    candidates(kinds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_workspace() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("rigg.yaml"),
            "environments:\n  dev:\n    default: true\n    search: { service: s }\n  prod:\n    search: { service: s }\n",
        )
        .unwrap();
        for project in ["alpha", "beta"] {
            let dir = tmp
                .path()
                .join("projects")
                .join(project)
                .join("envs/dev/search/indexers");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                tmp.path()
                    .join("projects")
                    .join(project)
                    .join("project.yaml"),
                "{}\n",
            )
            .unwrap();
            std::fs::write(
                dir.join(format!("{project}-indexer.json")),
                serde_json::to_string_pretty(&json!({"name": format!("{project}-indexer")}))
                    .unwrap(),
            )
            .unwrap();
        }
        tmp
    }

    fn in_dir<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
        // Serialize cwd changes across tests.
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = LOCK.lock().unwrap();
        let orig = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir).unwrap();
        let out = f();
        std::env::set_current_dir(orig).unwrap();
        out
    }

    fn values(cands: Vec<CompletionCandidate>) -> Vec<String> {
        cands
            .into_iter()
            .map(|c| c.get_value().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn completes_projects_envs_resources_selectors() {
        let ws = temp_workspace();
        in_dir(ws.path(), || {
            assert_eq!(values(projects()), vec!["alpha", "beta"]);
            assert_eq!(values(envs()), vec!["dev", "prod"]);
            let indexers = values(resource_names(ResourceKind::Indexer));
            assert_eq!(indexers, vec!["alpha-indexer", "beta-indexer"]);
            let sel = values(selectors());
            assert!(
                sel.contains(&"indexers/alpha-indexer".to_string()),
                "{sel:?}"
            );
            assert!(values(resource_names(ResourceKind::Index)).is_empty());
        });
    }

    #[test]
    fn outside_workspace_completes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        in_dir(tmp.path(), || {
            assert!(projects().is_empty());
            assert!(envs().is_empty());
            assert!(resource_names(ResourceKind::Indexer).is_empty());
            assert!(selectors().is_empty());
        });
    }

    #[test]
    fn new_kinds_include_specials_and_resources() {
        let kinds = values(new_kinds());
        assert!(kinds.contains(&"project".to_string()));
        assert!(kinds.contains(&"pipeline".to_string()));
        assert!(kinds.contains(&"knowledge-source".to_string()));
    }
}
