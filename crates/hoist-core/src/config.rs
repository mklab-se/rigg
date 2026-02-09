//! Configuration management for hoist

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::service::ServiceDomain;

/// Configuration errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Configuration file not found: {0}")]
    NotFound(PathBuf),
    #[error("Failed to read configuration: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse configuration: {0}")]
    ParseError(#[from] toml::de::Error),
    #[error("Failed to serialize configuration: {0}")]
    SerializeError(#[from] toml::ser::Error),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

/// Main configuration file (hoist.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Legacy Azure Search service configuration (auto-migrated to services.search)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<ServiceConfig>,
    /// Multi-service configuration (v0.2.0+)
    #[serde(default)]
    pub services: ServicesConfig,
    /// Project settings
    #[serde(default)]
    pub project: ProjectConfig,
    /// Pull/push settings
    #[serde(default)]
    pub sync: SyncConfig,
}

/// Multi-service configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ServicesConfig {
    /// Azure AI Search services
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub search: Vec<SearchServiceConfig>,
    /// Microsoft Foundry services
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foundry: Vec<FoundryServiceConfig>,
}

/// Azure Search service connection settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    /// Search service name (e.g., "my-search-service")
    pub name: String,
    /// Azure subscription ID (optional, can use default)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
    /// Resource group (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_group: Option<String>,
    /// API version to use
    #[serde(default = "default_api_version")]
    pub api_version: String,
    /// Preview API version for agentic search
    #[serde(default = "default_preview_api_version")]
    pub preview_api_version: String,
}

/// Search service configuration (v0.2.0+ format)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchServiceConfig {
    /// Search service name
    pub name: String,
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
    /// Subdirectory for resource files (relative to project root)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
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

impl Config {
    /// Default configuration filename
    pub const FILENAME: &'static str = "hoist.toml";

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
        let config: Config = toml::from_str(&content)?;
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
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Validate configuration
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Must have at least one search or foundry service
        let has_legacy = self.service.as_ref().is_some_and(|s| !s.name.is_empty());
        let has_search = !self.services.search.is_empty();
        let has_foundry = !self.services.foundry.is_empty();

        if !has_legacy && !has_search && !has_foundry {
            return Err(ConfigError::Invalid(
                "At least one service must be configured (service, services.search, or services.foundry)".to_string(),
            ));
        }

        // Validate legacy service name
        if let Some(ref svc) = self.service {
            if svc.name.is_empty() && !has_search && !has_foundry {
                return Err(ConfigError::Invalid("service.name is required".to_string()));
            }
        }

        // Validate search services
        for (i, svc) in self.services.search.iter().enumerate() {
            if svc.name.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "services.search[{}].name is required",
                    i
                )));
            }
        }

        // Validate foundry services
        for (i, svc) in self.services.foundry.iter().enumerate() {
            if svc.name.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "services.foundry[{}].name is required",
                    i
                )));
            }
            if svc.project.is_empty() {
                return Err(ConfigError::Invalid(format!(
                    "services.foundry[{}].project is required",
                    i
                )));
            }
        }

        Ok(())
    }

    /// Get all search service configs (including legacy auto-migrated)
    pub fn search_services(&self) -> Vec<SearchServiceConfig> {
        let mut result = self.services.search.clone();

        // Auto-migrate legacy [service] to search config
        if let Some(ref legacy) = self.service {
            if !legacy.name.is_empty() {
                // Only auto-migrate if no services.search already has this name
                let already_present = result.iter().any(|s| s.name == legacy.name);
                if !already_present {
                    result.insert(
                        0,
                        SearchServiceConfig {
                            name: legacy.name.clone(),
                            subscription: legacy.subscription.clone(),
                            resource_group: legacy.resource_group.clone(),
                            api_version: legacy.api_version.clone(),
                            preview_api_version: legacy.preview_api_version.clone(),
                        },
                    );
                }
            }
        }

        result
    }

    /// Get all foundry service configs
    pub fn foundry_services(&self) -> &[FoundryServiceConfig] {
        &self.services.foundry
    }

    /// Quick check if any foundry services are configured
    pub fn has_foundry(&self) -> bool {
        !self.services.foundry.is_empty()
    }

    /// Get the primary search service config (first one, for backward compat)
    pub fn primary_search_service(&self) -> Option<SearchServiceConfig> {
        self.search_services().into_iter().next()
    }

    /// Get the base URL for the Azure Search service (backward compat helper)
    pub fn service_url(&self) -> String {
        if let Some(ref svc) = self.service {
            return format!("https://{}.search.windows.net", svc.name);
        }
        if let Some(svc) = self.services.search.first() {
            return format!("https://{}.search.windows.net", svc.name);
        }
        String::new()
    }

    /// Get the base directory for resource files (project_root or project_root/path)
    pub fn resource_dir(&self, project_root: &Path) -> PathBuf {
        match &self.project.path {
            Some(path) => project_root.join(path),
            None => project_root.to_path_buf(),
        }
    }

    /// Base directory for a specific search service's resources
    /// Returns: resource_dir / "search-resources" / service_name
    pub fn search_service_dir(&self, project_root: &Path, service_name: &str) -> PathBuf {
        self.resource_dir(project_root)
            .join(ServiceDomain::Search.directory_prefix())
            .join(service_name)
    }

    /// Base directory for a specific foundry service/project's resources
    /// Returns: resource_dir / "foundry-resources" / service_name / project_name
    pub fn foundry_service_dir(
        &self,
        project_root: &Path,
        service_name: &str,
        project: &str,
    ) -> PathBuf {
        self.resource_dir(project_root)
            .join(ServiceDomain::Foundry.directory_prefix())
            .join(service_name)
            .join(project)
    }

    /// Get the API version to use for a resource (backward compat helper)
    pub fn api_version_for(&self, preview: bool) -> &str {
        if let Some(ref svc) = self.service {
            if preview {
                return &svc.preview_api_version;
            } else {
                return &svc.api_version;
            }
        }
        if let Some(svc) = self.services.search.first() {
            if preview {
                return &svc.preview_api_version;
            } else {
                return &svc.api_version;
            }
        }
        if preview {
            "2025-11-01-preview"
        } else {
            "2024-07-01"
        }
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

/// Create a Config with legacy [service] format (backward compat helper)
pub fn make_legacy_config(name: &str) -> Config {
    Config {
        service: Some(ServiceConfig {
            name: name.to_string(),
            subscription: None,
            resource_group: None,
            api_version: default_api_version(),
            preview_api_version: default_preview_api_version(),
        }),
        services: ServicesConfig::default(),
        project: ProjectConfig::default(),
        sync: SyncConfig::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_config(name: &str) -> Config {
        make_legacy_config(name)
    }

    #[test]
    fn test_validate_empty_name() {
        let config = make_config("");
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_valid_name() {
        let config = make_config("my-search");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_service_url() {
        let config = make_config("my-search");
        assert_eq!(config.service_url(), "https://my-search.search.windows.net");
    }

    #[test]
    fn test_api_version_for_stable() {
        let config = make_config("my-search");
        assert_eq!(config.api_version_for(false), "2024-07-01");
    }

    #[test]
    fn test_api_version_for_preview() {
        let config = make_config("my-search");
        assert_eq!(config.api_version_for(true), "2025-11-01-preview");
    }

    #[test]
    fn test_resource_dir_without_path() {
        let config = make_config("my-search");
        let root = Path::new("/projects/search");
        assert_eq!(config.resource_dir(root), PathBuf::from("/projects/search"));
    }

    #[test]
    fn test_resource_dir_with_path() {
        let mut config = make_config("my-search");
        config.project.path = Some("search".to_string());
        let root = Path::new("/projects/myapp");
        assert_eq!(
            config.resource_dir(root),
            PathBuf::from("/projects/myapp/search")
        );
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config("test-service");
        config.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path()).unwrap();
        assert_eq!(loaded.service.as_ref().unwrap().name, "test-service");
        assert_eq!(loaded.service.as_ref().unwrap().api_version, "2024-07-01");
    }

    #[test]
    fn test_load_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = Config::load(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_from_toml_string_legacy() {
        let toml = r#"
[service]
name = "my-svc"

[sync]
include_preview = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.service.as_ref().unwrap().name, "my-svc");
        assert!(!config.sync.include_preview);
        assert_eq!(config.service.as_ref().unwrap().api_version, "2024-07-01");
    }

    #[test]
    fn test_sync_config_defaults() {
        let sync = SyncConfig::default();
        assert!(sync.include_preview);
        assert!(sync.resources.is_empty());
    }

    #[test]
    fn test_find_project_root_found() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            dir.path().join(Config::FILENAME),
            "[service]\nname = \"x\"\n",
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
    fn test_path_serialized_in_toml() {
        let mut config = make_config("svc");
        config.project.path = Some("search".to_string());
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("path = \"search\""));
    }

    #[test]
    fn test_path_not_serialized_when_none() {
        let config = make_config("svc");
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(!toml_str.contains("path"));
    }

    // === New multi-service config tests ===

    #[test]
    fn test_new_format_search_only() {
        let toml = r#"
[[services.search]]
name = "my-search-service"
api_version = "2024-07-01"

[project]
name = "My Project"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.validate().is_ok());
        assert!(config.service.is_none());
        assert_eq!(config.services.search.len(), 1);
        assert_eq!(config.services.search[0].name, "my-search-service");
    }

    #[test]
    fn test_new_format_foundry_only() {
        let toml = r#"
[[services.foundry]]
name = "my-ai-service"
project = "my-project"

[project]
name = "My Project"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.validate().is_ok());
        assert!(config.has_foundry());
        assert_eq!(config.services.foundry[0].name, "my-ai-service");
        assert_eq!(config.services.foundry[0].project, "my-project");
        assert_eq!(config.services.foundry[0].api_version, "2025-05-15-preview");
    }

    #[test]
    fn test_new_format_both_services() {
        let toml = r#"
[[services.search]]
name = "my-search"

[[services.foundry]]
name = "my-ai"
project = "proj-1"

[project]
name = "RAG System"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.validate().is_ok());
        assert_eq!(config.search_services().len(), 1);
        assert!(config.has_foundry());
    }

    #[test]
    fn test_legacy_auto_migration() {
        let toml = r#"
[service]
name = "legacy-svc"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let search = config.search_services();
        assert_eq!(search.len(), 1);
        assert_eq!(search[0].name, "legacy-svc");
    }

    #[test]
    fn test_legacy_not_duplicated_in_search_services() {
        let toml = r#"
[service]
name = "my-svc"

[[services.search]]
name = "my-svc"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        let search = config.search_services();
        // Should not duplicate — same name in both legacy and services.search
        assert_eq!(search.len(), 1);
    }

    #[test]
    fn test_foundry_validation_requires_project() {
        let toml = r#"
[[services.foundry]]
name = "my-ai"
project = ""
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_foundry_service_url() {
        let svc = FoundryServiceConfig {
            name: "my-ai-service".to_string(),
            project: "proj-1".to_string(),
            api_version: "2025-05-15-preview".to_string(),
            endpoint: None,
            subscription: None,
            resource_group: None,
        };
        assert_eq!(
            svc.service_url(),
            "https://my-ai-service.services.ai.azure.com"
        );
    }

    #[test]
    fn test_foundry_service_url_with_endpoint() {
        let svc = FoundryServiceConfig {
            name: "my-ai-service".to_string(),
            project: "proj-1".to_string(),
            api_version: "2025-05-15-preview".to_string(),
            endpoint: Some("https://custom-subdomain.services.ai.azure.com".to_string()),
            subscription: None,
            resource_group: None,
        };
        assert_eq!(
            svc.service_url(),
            "https://custom-subdomain.services.ai.azure.com"
        );
    }

    #[test]
    fn test_foundry_service_url_strips_trailing_slash() {
        let svc = FoundryServiceConfig {
            name: "my-ai-service".to_string(),
            project: "proj-1".to_string(),
            api_version: "2025-05-15-preview".to_string(),
            endpoint: Some("https://custom-subdomain.services.ai.azure.com/".to_string()),
            subscription: None,
            resource_group: None,
        };
        assert_eq!(
            svc.service_url(),
            "https://custom-subdomain.services.ai.azure.com"
        );
    }

    #[test]
    fn test_search_service_url() {
        let svc = SearchServiceConfig {
            name: "my-search".to_string(),
            subscription: None,
            resource_group: None,
            api_version: "2024-07-01".to_string(),
            preview_api_version: "2025-11-01-preview".to_string(),
        };
        assert_eq!(svc.service_url(), "https://my-search.search.windows.net");
    }

    #[test]
    fn test_primary_search_service() {
        let config = make_config("primary-svc");
        let primary = config.primary_search_service().unwrap();
        assert_eq!(primary.name, "primary-svc");
    }

    #[test]
    fn test_has_foundry_false_when_empty() {
        let config = make_config("svc");
        assert!(!config.has_foundry());
    }

    #[test]
    fn test_no_services_validation_fails() {
        let config = Config {
            service: None,
            services: ServicesConfig::default(),
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_search_service_dir_without_path() {
        let config = make_config("my-search");
        let root = Path::new("/projects/search");
        assert_eq!(
            config.search_service_dir(root, "my-search"),
            PathBuf::from("/projects/search/search-resources/my-search")
        );
    }

    #[test]
    fn test_search_service_dir_with_path() {
        let mut config = make_config("my-search");
        config.project.path = Some("resources".to_string());
        let root = Path::new("/projects/myapp");
        assert_eq!(
            config.search_service_dir(root, "my-search"),
            PathBuf::from("/projects/myapp/resources/search-resources/my-search")
        );
    }

    #[test]
    fn test_foundry_service_dir_without_path() {
        let config = make_config("svc");
        let root = Path::new("/projects/ai");
        assert_eq!(
            config.foundry_service_dir(root, "my-ai-service", "my-project"),
            PathBuf::from("/projects/ai/foundry-resources/my-ai-service/my-project")
        );
    }

    #[test]
    fn test_foundry_service_dir_with_path() {
        let mut config = make_config("svc");
        config.project.path = Some("resources".to_string());
        let root = Path::new("/projects/ai");
        assert_eq!(
            config.foundry_service_dir(root, "my-ai-service", "my-project"),
            PathBuf::from("/projects/ai/resources/foundry-resources/my-ai-service/my-project")
        );
    }

    #[test]
    fn test_foundry_default_api_version() {
        let toml = r#"
[[services.foundry]]
name = "my-ai"
project = "proj-1"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.services.foundry[0].api_version, "2025-05-15-preview");
    }

    #[test]
    fn test_save_load_roundtrip_new_format() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            service: None,
            services: ServicesConfig {
                search: vec![SearchServiceConfig {
                    name: "test-search".to_string(),
                    subscription: None,
                    resource_group: None,
                    api_version: "2024-07-01".to_string(),
                    preview_api_version: "2025-11-01-preview".to_string(),
                }],
                foundry: vec![FoundryServiceConfig {
                    name: "test-ai".to_string(),
                    project: "test-proj".to_string(),
                    api_version: "2025-05-15-preview".to_string(),
                    endpoint: None,
                    subscription: None,
                    resource_group: None,
                }],
            },
            project: ProjectConfig {
                name: Some("Test".to_string()),
                description: None,
                path: None,
            },
            sync: SyncConfig::default(),
        };
        config.save(dir.path()).unwrap();

        let loaded = Config::load(dir.path()).unwrap();
        assert_eq!(loaded.services.search[0].name, "test-search");
        assert_eq!(loaded.services.foundry[0].name, "test-ai");
        assert_eq!(loaded.services.foundry[0].project, "test-proj");
    }
}

/// Find the project root by looking for hoist.toml
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
