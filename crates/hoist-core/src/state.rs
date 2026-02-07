//! Local state management for tracking synced resources

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::resources::ResourceKind;

/// State management errors
#[derive(Debug, Error)]
pub enum StateError {
    #[error("Failed to read state: {0}")]
    ReadError(#[from] std::io::Error),
    #[error("Failed to parse state: {0}")]
    ParseError(#[from] serde_json::Error),
}

/// Local state tracking (.hoist/state.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalState {
    /// Last sync timestamp
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<DateTime<Utc>>,
    /// Resources by kind and name
    #[serde(default)]
    pub resources: HashMap<String, ResourceState>,
}

/// State for a single resource
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceState {
    /// Resource kind
    pub kind: ResourceKind,
    /// Last known ETag from Azure
    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// Checksum of normalized JSON
    pub checksum: String,
    /// Last sync timestamp
    pub synced_at: DateTime<Utc>,
}

/// Checksums for change detection (.hoist/checksums.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Checksums {
    /// Checksum by resource key (kind/name)
    pub checksums: HashMap<String, String>,
}

impl LocalState {
    /// State directory name
    pub const DIR_NAME: &'static str = ".hoist";
    /// State file name
    pub const STATE_FILE: &'static str = "state.json";
    /// Checksums file name
    pub const CHECKSUMS_FILE: &'static str = "checksums.json";

    /// Get the state directory path
    pub fn state_dir(project_root: &Path) -> PathBuf {
        project_root.join(Self::DIR_NAME)
    }

    /// Get the state file path
    pub fn state_file(project_root: &Path) -> PathBuf {
        Self::state_dir(project_root).join(Self::STATE_FILE)
    }

    /// Get the checksums file path
    pub fn checksums_file(project_root: &Path) -> PathBuf {
        Self::state_dir(project_root).join(Self::CHECKSUMS_FILE)
    }

    /// Load state from disk
    pub fn load(project_root: &Path) -> Result<Self, StateError> {
        let path = Self::state_file(project_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let state: Self = serde_json::from_str(&content)?;
        Ok(state)
    }

    /// Save state to disk
    pub fn save(&self, project_root: &Path) -> Result<(), StateError> {
        let dir = Self::state_dir(project_root);
        std::fs::create_dir_all(&dir)?;

        let path = Self::state_file(project_root);
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Get resource key
    pub fn resource_key(kind: ResourceKind, name: &str) -> String {
        format!("{}/{}", kind.directory_name(), name)
    }

    /// Get resource state
    pub fn get(&self, kind: ResourceKind, name: &str) -> Option<&ResourceState> {
        let key = Self::resource_key(kind, name);
        self.resources.get(&key)
    }

    /// Set resource state
    pub fn set(&mut self, kind: ResourceKind, name: &str, state: ResourceState) {
        let key = Self::resource_key(kind, name);
        self.resources.insert(key, state);
    }

    /// Remove resource state
    pub fn remove(&mut self, kind: ResourceKind, name: &str) {
        let key = Self::resource_key(kind, name);
        self.resources.remove(&key);
    }
}

impl Checksums {
    /// Load checksums from disk
    pub fn load(project_root: &Path) -> Result<Self, StateError> {
        let path = LocalState::checksums_file(project_root);
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let checksums: Self = serde_json::from_str(&content)?;
        Ok(checksums)
    }

    /// Save checksums to disk
    pub fn save(&self, project_root: &Path) -> Result<(), StateError> {
        let dir = LocalState::state_dir(project_root);
        std::fs::create_dir_all(&dir)?;

        let path = LocalState::checksums_file(project_root);
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Calculate checksum for content
    pub fn calculate(content: &str) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Get checksum for a resource
    pub fn get(&self, kind: ResourceKind, name: &str) -> Option<&String> {
        let key = LocalState::resource_key(kind, name);
        self.checksums.get(&key)
    }

    /// Set checksum for a resource
    pub fn set(&mut self, kind: ResourceKind, name: &str, checksum: String) {
        let key = LocalState::resource_key(kind, name);
        self.checksums.insert(key, checksum);
    }

    /// Remove checksum for a resource
    pub fn remove(&mut self, kind: ResourceKind, name: &str) {
        let key = LocalState::resource_key(kind, name);
        self.checksums.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_key_format() {
        let key = LocalState::resource_key(ResourceKind::Index, "my-index");
        assert_eq!(key, "search-management/indexes/my-index");
    }

    #[test]
    fn test_resource_key_datasource() {
        let key = LocalState::resource_key(ResourceKind::DataSource, "ds1");
        assert_eq!(key, "search-management/data-sources/ds1");
    }

    #[test]
    fn test_state_get_set() {
        let mut state = LocalState::default();
        assert!(state.get(ResourceKind::Index, "idx").is_none());

        state.set(
            ResourceKind::Index,
            "idx",
            ResourceState {
                kind: ResourceKind::Index,
                etag: Some("etag1".to_string()),
                checksum: "abc".to_string(),
                synced_at: chrono::Utc::now(),
            },
        );

        let got = state.get(ResourceKind::Index, "idx").unwrap();
        assert_eq!(got.checksum, "abc");
        assert_eq!(got.etag.as_deref(), Some("etag1"));
    }

    #[test]
    fn test_state_remove() {
        let mut state = LocalState::default();
        state.set(
            ResourceKind::Index,
            "idx",
            ResourceState {
                kind: ResourceKind::Index,
                etag: None,
                checksum: "abc".to_string(),
                synced_at: chrono::Utc::now(),
            },
        );

        state.remove(ResourceKind::Index, "idx");
        assert!(state.get(ResourceKind::Index, "idx").is_none());
    }

    #[test]
    fn test_state_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = LocalState::default();
        state.last_sync = Some(chrono::Utc::now());
        state.set(
            ResourceKind::Indexer,
            "my-indexer",
            ResourceState {
                kind: ResourceKind::Indexer,
                etag: None,
                checksum: "hash123".to_string(),
                synced_at: chrono::Utc::now(),
            },
        );

        state.save(dir.path()).unwrap();
        let loaded = LocalState::load(dir.path()).unwrap();

        assert!(loaded.last_sync.is_some());
        let got = loaded.get(ResourceKind::Indexer, "my-indexer").unwrap();
        assert_eq!(got.checksum, "hash123");
    }

    #[test]
    fn test_state_load_missing_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let state = LocalState::load(dir.path()).unwrap();
        assert!(state.last_sync.is_none());
        assert!(state.resources.is_empty());
    }

    #[test]
    fn test_checksums_calculate_deterministic() {
        let c1 = Checksums::calculate("hello world");
        let c2 = Checksums::calculate("hello world");
        assert_eq!(c1, c2);
    }

    #[test]
    fn test_checksums_calculate_different_input() {
        let c1 = Checksums::calculate("hello");
        let c2 = Checksums::calculate("world");
        assert_ne!(c1, c2);
    }

    #[test]
    fn test_checksums_get_set() {
        let mut checksums = Checksums::default();
        assert!(checksums.get(ResourceKind::Index, "idx").is_none());

        checksums.set(ResourceKind::Index, "idx", "abc123".to_string());
        assert_eq!(
            checksums.get(ResourceKind::Index, "idx"),
            Some(&"abc123".to_string())
        );
    }

    #[test]
    fn test_checksums_remove() {
        let mut checksums = Checksums::default();
        checksums.set(ResourceKind::Index, "idx", "abc123".to_string());
        assert!(checksums.get(ResourceKind::Index, "idx").is_some());

        checksums.remove(ResourceKind::Index, "idx");
        assert!(checksums.get(ResourceKind::Index, "idx").is_none());
    }

    #[test]
    fn test_checksums_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let mut checksums = Checksums::default();
        checksums.set(ResourceKind::Skillset, "sk1", "hash1".to_string());

        checksums.save(dir.path()).unwrap();
        let loaded = Checksums::load(dir.path()).unwrap();

        assert_eq!(
            loaded.get(ResourceKind::Skillset, "sk1"),
            Some(&"hash1".to_string())
        );
    }

    #[test]
    fn test_state_dir_path() {
        let root = Path::new("/my/project");
        assert_eq!(
            LocalState::state_dir(root),
            PathBuf::from("/my/project/.hoist")
        );
    }

    #[test]
    fn test_state_file_path() {
        let root = Path::new("/my/project");
        assert_eq!(
            LocalState::state_file(root),
            PathBuf::from("/my/project/.hoist/state.json")
        );
    }

    #[test]
    fn test_checksums_file_path() {
        let root = Path::new("/my/project");
        assert_eq!(
            LocalState::checksums_file(root),
            PathBuf::from("/my/project/.hoist/checksums.json")
        );
    }
}
