//! Configuration management for hoist

use serde::{Deserialize, Serialize};
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
    ParseError(#[from] toml::de::Error),
    #[error("Failed to serialize configuration: {0}")]
    SerializeError(#[from] toml::ser::Error),
    #[error("Invalid configuration: {0}")]
    Invalid(String),
}

/// Main configuration file (hoist.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Azure Search service configuration
    pub service: ServiceConfig,
    /// Project settings
    #[serde(default)]
    pub project: ProjectConfig,
    /// Pull/push settings
    #[serde(default)]
    pub sync: SyncConfig,
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

fn default_api_version() -> String {
    "2024-07-01".to_string()
}

fn default_preview_api_version() -> String {
    "2025-11-01-preview".to_string()
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
    /// Generate README files
    #[serde(default = "default_true")]
    pub generate_docs: bool,
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
            generate_docs: true,
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
        if self.service.name.is_empty() {
            return Err(ConfigError::Invalid("service.name is required".to_string()));
        }
        Ok(())
    }

    /// Get the base URL for the Azure Search service
    pub fn service_url(&self) -> String {
        format!("https://{}.search.windows.net", self.service.name)
    }

    /// Get the base directory for resource files (project_root or project_root/path)
    pub fn resource_dir(&self, project_root: &Path) -> PathBuf {
        match &self.project.path {
            Some(path) => project_root.join(path),
            None => project_root.to_path_buf(),
        }
    }

    /// Get the API version to use for a resource
    pub fn api_version_for(&self, preview: bool) -> &str {
        if preview {
            &self.service.preview_api_version
        } else {
            &self.service.api_version
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_config(name: &str) -> Config {
        Config {
            service: ServiceConfig {
                name: name.to_string(),
                subscription: None,
                resource_group: None,
                api_version: "2024-07-01".to_string(),
                preview_api_version: "2025-11-01-preview".to_string(),
            },
            project: ProjectConfig::default(),
            sync: SyncConfig::default(),
        }
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
        assert_eq!(loaded.service.name, "test-service");
        assert_eq!(loaded.service.api_version, "2024-07-01");
    }

    #[test]
    fn test_load_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let result = Config::load(dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_from_toml_string() {
        let toml = r#"
[service]
name = "my-svc"

[sync]
include_preview = false
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.service.name, "my-svc");
        assert!(!config.sync.include_preview);
        // Defaults should apply
        assert_eq!(config.service.api_version, "2024-07-01");
        assert!(config.sync.generate_docs);
    }

    #[test]
    fn test_sync_config_defaults() {
        let sync = SyncConfig::default();
        assert!(sync.include_preview);
        assert!(sync.generate_docs);
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
