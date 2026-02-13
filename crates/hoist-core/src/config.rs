//! Configuration management for hoist

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Configuration errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Configuration file not found: {0}")]
    NotFound(PathBuf),
    #[error("Failed to read configuration: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse configuration: {0}")]
    ParseError(#[from] serde_yaml::Error),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

/// Main configuration file (hoist.yaml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub project: ProjectConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    pub environments: BTreeMap<String, EnvironmentConfig>,
}

/// Environment configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentConfig {
    #[serde(default)]
    pub default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search: Vec<SearchServiceConfig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foundry: Vec<FoundryServiceConfig>,
}

/// Search service configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchServiceConfig {
    /// Search service name
    pub name: String,
    /// Label for multi-service environments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Azure subscription ID (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
    /// Resource group (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
    /// API version to use
    #[serde(default = "default_api_version")]
    pub api_version: String,
    /// Preview API version
    #[serde(default = "default_preview_api_version")]
    pub preview_api_version: String,
}

/// Foundry service configuration
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FoundryServiceConfig {
    /// AI services host name (e.g., "my-ai-service")
    pub name: String,
    /// Foundry project name
    pub project: String,
    /// Label for multi-service environments
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// API version to use
    #[serde(default = "default_foundry_api_version")]
    pub api_version: String,
    /// Service endpoint URL (discovered from ARM; overrides name-based URL construction)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Azure subscription ID (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
    /// Resource group (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
}

fn default_api_version() -> String {
    "2024-07-01".to_string()
}

fn default_preview_api_version() -> String {
    "2025-11-01-preview".to_string()
}

fn default_foundry_api_version() -> String {
    "2025-05-15-preview".to_string()
}

/// Project-level settings
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectConfig {
    /// Project name (used in generated documentation)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Project description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Sync behavior settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    /// Include preview API resources (knowledge bases, knowledge sources)
    #[serde(default = "default_true")]
    pub include_preview: bool,
    /// Resource types to sync (empty = all)
    #[serde(default)]
    pub resources: Vec<String>,
}

fn default_true() -> bool {
    true
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            include_preview: true,
            resources: Vec::new(),
        }
    }
}

/// Resolved environment for command execution.
///
/// Central abstraction all commands work through. Created by `Config::resolve_env()`.
pub struct ResolvedEnvironment {
    pub name: String,
    pub search: Vec<SearchServiceConfig>,
    pub foundry: Vec<FoundryServiceConfig>,
    pub sync: SyncConfig,
}

impl ResolvedEnvironment {
    /// Base dir for a search service: search/ (single) or search/<label>/ (multi)
    pub fn search_service_dir(&self, root: &Path, service: &SearchServiceConfig) -> PathBuf {
        let base = root.join("search");
        if self.search.len() <= 1 {
            base
        } else {
            base.join(service.label.as_deref().unwrap_or(&service.name))
        }
    }

    /// Base dir for a foundry service: foundry/ (single) or foundry/<label>/ (multi)
    pub fn foundry_service_dir(&self, root: &Path, service: &FoundryServiceConfig) -> PathBuf {
        let base = root.join("foundry");
        if self.foundry.len() <= 1 {
            base
        } else {
            base.join(service.label.as_deref().unwrap_or(&service.name))
        }
    }

    pub fn primary_search_service(&self) -> Option<&SearchServiceConfig> {
        self.search.first()
    }

    pub fn has_foundry(&self) -> bool {
        !self.foundry.is_empty()
    }

    pub fn has_search(&self) -> bool {
        !self.search.is_empty()
    }
}

impl Config {
    /// Default configuration filename
    pub const FILENAME: &'static str = "hoist.yaml";

    /// Load configuration from a directory
    pub fn load(dir: &Path) -> Result<Self, ConfigError> {
        let path = dir.join(Self::FILENAME);
        Self::load_from(&path)
    }

    /// Load configuration from a specific file path
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        if !path.exists() {
            return Err(ConfigError::NotFound(path.to_path_buf()));
        }
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_yaml::from_str(&content)?;
        config.validate()?;
        Ok(config)
    }

    /// Save configuration to a directory
    pub fn save(&self, dir: &Path) -> Result<(), ConfigError> {
        let path = dir.join(Self::FILENAME);
        self.save_to(&path)
    }

    /// Save configuration to a specific file path
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.environments.is_empty() {
            return Err(ConfigError::Invalid(
                "At least one environment must be configured".to_string(),
            ));
        }

        for (name, env) in &self.environments {
            if env.search.is_empty() && env.foundry.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "Environment '{}' has no services configured",
                    name
                )));
            }
            // If multi-service, require labels
            if env.search.len() > 1 && env.search.iter().any(|s| s.label.is_none()) {
                return Err(ConfigError::Invalid(format!(
                    "Environment '{}' has multiple search services — all must have a 'label'",
                    name
                )));
            }
            if env.foundry.len() > 1 && env.foundry.iter().any(|s| s.label.is_none()) {
                return Err(ConfigError::Invalid(format!(
                    "Environment '{}' has multiple foundry services — all must have a 'label'",
                    name
                )));
            }
            // Validate service names
            for (i, svc) in env.search.iter().enumerate() {
                if svc.name.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "environments.{}.search[{}].name is required",
                        name, i
                    )));
                }
            }
            for (i, svc) in env.foundry.iter().enumerate() {
                if svc.name.is_empty() || svc.project.is_empty() {
                    return Err(ConfigError::Invalid(format!(
                        "environments.{}.foundry[{}] requires name and project",
                        name, i
                    )));
                }
            }
        }

        // At most one default
        let defaults = self.environments.values().filter(|e| e.default).count();
        if defaults > 1 {
            return Err(ConfigError::Invalid(
                "Only one environment can be set as default".to_string(),
            ));
        }

        Ok(())
    }

    /// Resolve an environment by name (or default)
    pub fn resolve_env(&self, name: Option<&str>) -> Result<ResolvedEnvironment, ConfigError> {
        let env_name = name.or_else(|| self.default_env_name()).ok_or_else(|| {
            ConfigError::Invalid("No environment specified and no default set".to_string())
        })?;
        let env_config = self
            .environments
            .get(env_name)
            .ok_or_else(|| ConfigError::Invalid(format!("Environment '{}' not found", env_name)))?;
        Ok(ResolvedEnvironment {
            name: env_name.to_string(),
            search: env_config.search.clone(),
            foundry: env_config.foundry.clone(),
            sync: self.sync.clone(),
        })
    }

    /// Find the default environment name
    pub fn default_env_name(&self) -> Option<&str> {
        // 1. Env with default: true
        self.environments
            .iter()
            .find(|(_, e)| e.default)
            .map(|(n, _)| n.as_str())
            // 2. Only one env → use it
            .or_else(|| {
                if self.environments.len() == 1 {
                    self.environments.keys().next().map(|s| s.as_str())
                } else {
                    None
                }
            })
    }

    /// Get all environment names
    pub fn environment_names(&self) -> Vec<&str> {
        self.environments.keys().map(|s| s.as_str()).collect()
    }
}

impl SearchServiceConfig {
    /// Get the base URL for this search service
    pub fn service_url(&self) -> String {
        format!("https://{}.search.windows.net", self.name)
    }
}

impl FoundryServiceConfig {
    /// Get the base URL for this Foundry service.
    ///
    /// Prefers the `endpoint` field (discovered from ARM) over a URL
    /// constructed from `name`, since the ARM custom subdomain may differ
    /// from the resource name.
    pub fn service_url(&self) -> String {
        if let Some(ref ep) = self.endpoint {
            return ep.trim_end_matches('/').to_string();
        }
        format!("https://{}.services.ai.azure.com", self.name)
    }
}

/// Find the project root by looking for hoist.yaml
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(Config::FILENAME).exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_search_service(name: &str) -> SearchServiceConfig {
        SearchServiceConfig {
            name: name.to_string(),
            label: None,
            subscription: None,
            resource_group: None,
            api_version: default_api_version(),
            preview_api_version: default_preview_api_version(),
        }
    }

    fn make_foundry_service(name: &str, project: &str) -> FoundryServiceConfig {
        FoundryServiceConfig {
            name: name.to_string(),
            project: project.to_string(),
            label: None,
            api_version: default_foundry_api_version(),
            endpoint: None,
            subscription: None,
            resource_group: None,
        }
    }

    fn make_config_search(name: &str) -> Config {
        Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: true,
                    description: None,
                    search: vec![make_search_service(name)],
                    foundry: vec![],
                },
            )]),
        }
    }

    fn make_config_both() -> Config {
        Config {
            project: ProjectConfig {
                name: Some("Test".to_string()),
                description: None,
            },
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: true,
                    description: None,
                    search: vec![make_search_service("test-search")],
                    foundry: vec![make_foundry_service("test-ai", "test-proj")],
                },
            )]),
        }
    }

    #[test]
    fn test_validate_no_environments() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::new(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_empty_env() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: false,
                    description: None,
                    search: vec![],
                    foundry: vec![],
                },
            )]),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_valid_search_only() {
        let config = make_config_search("my-search");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_empty_search_name() {
        let mut config = make_config_search("");
        // Empty name in search service should fail
        let result = config.validate();
        assert!(result.is_err());
        // Fix it to verify the error was about the name
        config.environments.get_mut("prod").unwrap().search[0].name = "valid".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_foundry_requires_project() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: false,
                    description: None,
                    search: vec![],
                    foundry: vec![FoundryServiceConfig {
                        name: "my-ai".to_string(),
                        project: "".to_string(),
                        label: None,
                        api_version: default_foundry_api_version(),
                        endpoint: None,
                        subscription: None,
                        resource_group: None,
                    }],
                },
            )]),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_multi_search_requires_labels() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: false,
                    description: None,
                    search: vec![make_search_service("svc-1"), make_search_service("svc-2")],
                    foundry: vec![],
                },
            )]),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_multi_search_with_labels() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "prod".to_string(),
                EnvironmentConfig {
                    default: false,
                    description: None,
                    search: vec![
                        SearchServiceConfig {
                            label: Some("primary".to_string()),
                            ..make_search_service("svc-1")
                        },
                        SearchServiceConfig {
                            label: Some("analytics".to_string()),
                            ..make_search_service("svc-2")
                        },
                    ],
                    foundry: vec![],
                },
            )]),
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_multiple_defaults() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([
                (
                    "prod".to_string(),
                    EnvironmentConfig {
                        default: true,
                        description: None,
                        search: vec![make_search_service("svc-1")],
                        foundry: vec![],
                    },
                ),
                (
                    "test".to_string(),
                    EnvironmentConfig {
                        default: true,
                        description: None,
                        search: vec![make_search_service("svc-2")],
                        foundry: vec![],
                    },
                ),
            ]),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_search_service_url() {
        let svc = make_search_service("my-search");
        assert_eq!(svc.service_url(), "https://my-search.search.windows.net");
    }

    #[test]
    fn test_foundry_service_url() {
        let svc = make_foundry_service("my-ai-service", "proj-1");
        assert_eq!(
            svc.service_url(),
            "https://my-ai-service.services.ai.azure.com"
        );
    }

    #[test]
    fn test_foundry_service_url_with_endpoint() {
        let mut svc = make_foundry_service("my-ai-service", "proj-1");
        svc.endpoint = Some("https://custom-subdomain.services.ai.azure.com".to_string());
        assert_eq!(
            svc.service_url(),
            "https://custom-subdomain.services.ai.azure.com"
        );
    }

    #[test]
    fn test_foundry_service_url_strips_trailing_slash() {
        let mut svc = make_foundry_service("my-ai-service", "proj-1");
        svc.endpoint = Some("https://custom-subdomain.services.ai.azure.com/".to_string());
        assert_eq!(
            svc.service_url(),
            "https://custom-subdomain.services.ai.azure.com"
        );
    }

    #[test]
    fn test_sync_config_defaults() {
        let sync = SyncConfig::default();
        assert!(sync.include_preview);
        assert!(sync.resources.is_empty());
    }

    #[test]
    fn test_resolve_env_default() {
        let config = make_config_search("my-search");
        let env = config.resolve_env(None).unwrap();
        assert_eq!(env.name, "prod");
        assert_eq!(env.search.len(), 1);
        assert_eq!(env.search[0].name, "my-search");
    }

    #[test]
    fn test_resolve_env_by_name() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([
                (
                    "prod".to_string(),
                    EnvironmentConfig {
                        default: true,
                        description: None,
                        search: vec![make_search_service("search-prod")],
                        foundry: vec![],
                    },
                ),
                (
                    "test".to_string(),
                    EnvironmentConfig {
                        default: false,
                        description: None,
                        search: vec![make_search_service("search-test")],
                        foundry: vec![],
                    },
                ),
            ]),
        };
        let env = config.resolve_env(Some("test")).unwrap();
        assert_eq!(env.name, "test");
        assert_eq!(env.search[0].name, "search-test");
    }

    #[test]
    fn test_resolve_env_not_found() {
        let config = make_config_search("svc");
        assert!(config.resolve_env(Some("missing")).is_err());
    }

    #[test]
    fn test_resolve_env_single_env_is_default() {
        // Single env without default: true should still resolve
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([(
                "staging".to_string(),
                EnvironmentConfig {
                    default: false,
                    description: None,
                    search: vec![make_search_service("svc")],
                    foundry: vec![],
                },
            )]),
        };
        let env = config.resolve_env(None).unwrap();
        assert_eq!(env.name, "staging");
    }

    #[test]
    fn test_resolve_env_multiple_envs_no_default() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([
                (
                    "prod".to_string(),
                    EnvironmentConfig {
                        default: false,
                        description: None,
                        search: vec![make_search_service("svc-1")],
                        foundry: vec![],
                    },
                ),
                (
                    "test".to_string(),
                    EnvironmentConfig {
                        default: false,
                        description: None,
                        search: vec![make_search_service("svc-2")],
                        foundry: vec![],
                    },
                ),
            ]),
        };
        assert!(config.resolve_env(None).is_err());
    }

    #[test]
    fn test_default_env_name_with_default() {
        let config = make_config_search("svc");
        assert_eq!(config.default_env_name(), Some("prod"));
    }

    #[test]
    fn test_environment_names() {
        let config = Config {
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
            environments: BTreeMap::from([
                (
                    "prod".to_string(),
                    EnvironmentConfig {
                        default: true,
                        description: None,
                        search: vec![make_search_service("svc-1")],
                        foundry: vec![],
                    },
                ),
                (
                    "test".to_string(),
                    EnvironmentConfig {
                        default: false,
                        description: None,
                        search: vec![make_search_service("svc-2")],
                        foundry: vec![],
                    },
                ),
            ]),
        };
        let names = config.environment_names();
        assert_eq!(names, vec!["prod", "test"]);
    }

    #[test]
    fn test_resolved_env_search_service_dir_single() {
        let env = ResolvedEnvironment {
            name: "prod".to_string(),
            search: vec![make_search_service("my-search")],
            foundry: vec![],
            sync: SyncConfig::default(),
        };
        let root = Path::new("/projects/myapp");
        assert_eq!(
            env.search_service_dir(root, &env.search[0]),
            PathBuf::from("/projects/myapp/search")
        );
    }

    #[test]
    fn test_resolved_env_search_service_dir_multi_with_labels() {
        let env = ResolvedEnvironment {
            name: "prod".to_string(),
            search: vec![
                SearchServiceConfig {
                    label: Some("primary".to_string()),
                    ..make_search_service("svc-1")
                },
                SearchServiceConfig {
                    label: Some("analytics".to_string()),
                    ..make_search_service("svc-2")
                },
            ],
            foundry: vec![],
            sync: SyncConfig::default(),
        };
        let root = Path::new("/projects/myapp");
        assert_eq!(
            env.search_service_dir(root, &env.search[0]),
            PathBuf::from("/projects/myapp/search/primary")
        );
        assert_eq!(
            env.search_service_dir(root, &env.search[1]),
            PathBuf::from("/projects/myapp/search/analytics")
        );
    }

    #[test]
    fn test_resolved_env_foundry_service_dir_single() {
        let env = ResolvedEnvironment {
            name: "prod".to_string(),
            search: vec![],
            foundry: vec![make_foundry_service("my-ai", "proj-1")],
            sync: SyncConfig::default(),
        };
        let root = Path::new("/projects/myapp");
        assert_eq!(
            env.foundry_service_dir(root, &env.foundry[0]),
            PathBuf::from("/projects/myapp/foundry")
        );
    }

    #[test]
    fn test_resolved_env_foundry_service_dir_multi() {
        let env = ResolvedEnvironment {
            name: "prod".to_string(),
            search: vec![],
            foundry: vec![
                FoundryServiceConfig {
                    label: Some("rag".to_string()),
                    ..make_foundry_service("ai-svc", "rag-proj")
                },
                FoundryServiceConfig {
                    label: Some("chat".to_string()),
                    ..make_foundry_service("ai-svc", "chat-proj")
                },
            ],
            sync: SyncConfig::default(),
        };
        let root = Path::new("/projects/myapp");
        assert_eq!(
            env.foundry_service_dir(root, &env.foundry[0]),
            PathBuf::from("/projects/myapp/foundry/rag")
        );
        assert_eq!(
            env.foundry_service_dir(root, &env.foundry[1]),
            PathBuf::from("/projects/myapp/foundry/chat")
        );
    }

    #[test]
    fn test_resolved_env_has_foundry() {
        let env = ResolvedEnvironment {
            name: "prod".to_string(),
            search: vec![],
            foundry: vec![make_foundry_service("ai", "proj")],
            sync: SyncConfig::default(),
        };
        assert!(env.has_foundry());
        assert!(!env.has_search());
    }

    #[test]
    fn test_resolved_env_primary_search_service() {
        let env = ResolvedEnvironment {
            name: "prod".to_string(),
            search: vec![make_search_service("svc")],
            foundry: vec![],
            sync: SyncConfig::default(),
        };
        assert_eq!(env.primary_search_service().unwrap().name, "svc");
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_both();
        config.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path()).unwrap();
        let env = loaded.resolve_env(None).unwrap();
        assert_eq!(env.search[0].name, "test-search");
        assert_eq!(env.foundry[0].name, "test-ai");
        assert_eq!(env.foundry[0].project, "test-proj");
    }

    #[test]
    fn test_load_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = Config::load(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_from_yaml_string() {
        let yaml = r#"
environments:
  prod:
    default: true
    search:
      - name: my-search-service
        api_version: "2024-07-01"

project:
  name: My Project

sync:
  include_preview: false
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.environments.len(), 1);
        let env = config.resolve_env(None).unwrap();
        assert_eq!(env.search[0].name, "my-search-service");
        assert!(!env.sync.include_preview);
    }

    #[test]
    fn test_load_foundry_only() {
        let yaml = r#"
environments:
  prod:
    foundry:
      - name: my-ai-service
        project: my-project
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.validate().is_ok());
        let env = config.resolve_env(None).unwrap();
        assert!(env.has_foundry());
        assert_eq!(env.foundry[0].name, "my-ai-service");
        assert_eq!(env.foundry[0].project, "my-project");
        assert_eq!(env.foundry[0].api_version, "2025-05-15-preview");
    }

    #[test]
    fn test_load_both_services() {
        let yaml = r#"
environments:
  prod:
    default: true
    search:
      - name: my-search
    foundry:
      - name: my-ai
        project: proj-1

project:
  name: RAG System
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.validate().is_ok());
        let env = config.resolve_env(None).unwrap();
        assert_eq!(env.search.len(), 1);
        assert!(env.has_foundry());
    }

    #[test]
    fn test_load_multi_environment() {
        let yaml = r#"
environments:
  prod:
    default: true
    search:
      - name: search-prod
  test:
    search:
      - name: search-test
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.environment_names(), vec!["prod", "test"]);

        let prod = config.resolve_env(Some("prod")).unwrap();
        assert_eq!(prod.search[0].name, "search-prod");

        let test = config.resolve_env(Some("test")).unwrap();
        assert_eq!(test.search[0].name, "search-test");
    }

    #[test]
    fn test_find_project_root_found() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            dir.path().join(Config::FILENAME),
            "environments:\n  prod:\n    search:\n      - name: x\n",
        )
        .unwrap();

        let found = find_project_root(&sub);
        assert_eq!(found, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn test_find_project_root_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let found = find_project_root(dir.path());
        assert!(found.is_none());
    }

    #[test]
    fn test_foundry_default_api_version() {
        let yaml = r#"
environments:
  prod:
    foundry:
      - name: my-ai
        project: proj-1
"#;
        let config: Config = serde_yaml::from_str(yaml).unwrap();
        let env = config.resolve_env(None).unwrap();
        assert_eq!(env.foundry[0].api_version, "2025-05-15-preview");
    }

    #[test]
    fn test_config_filename_is_yaml() {
        assert_eq!(Config::FILENAME, "hoist.yaml");
    }
}
