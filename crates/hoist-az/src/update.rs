//! Background update checker for hoist.
//!
//! Queries crates.io for the latest published version of `hoist-az` and prints
//! a notification to stderr when a newer version is available. Results are
//! cached for 24 hours to avoid redundant network calls.

use chrono::{DateTime, Utc};
use colored::Colorize;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const CRATE_NAME: &str = "hoist-az";
const CACHE_MAX_AGE_SECS: i64 = 24 * 60 * 60;
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Cached result of the last update check.
#[derive(Serialize, Deserialize)]
struct UpdateCache {
    last_check: DateTime<Utc>,
    latest_version: String,
}

/// Returns the path to the cache file: `<cache_dir>/hoist/update-check.json`.
fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("hoist").join("update-check.json"))
}

/// Loads and deserializes the cache file. Returns `None` on any error.
fn load_cache() -> Option<UpdateCache> {
    let path = cache_path()?;
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Persists the cache file, creating parent directories as needed.
fn save_cache(cache: &UpdateCache) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Ok(data) = serde_json::to_string(cache) else {
        return;
    };
    let _ = std::fs::write(path, data);
}

/// Returns `true` if the cache is missing or older than 24 hours.
fn should_check(cache: Option<&UpdateCache>) -> bool {
    match cache {
        None => true,
        Some(c) => {
            let age = Utc::now().signed_duration_since(c.last_check);
            age.num_seconds() > CACHE_MAX_AGE_SECS
        }
    }
}

/// Crates.io API response (subset).
#[derive(Deserialize)]
struct CratesIoResponse {
    #[serde(rename = "crate")]
    krate: CrateInfo,
}

#[derive(Deserialize)]
struct CrateInfo {
    max_stable_version: String,
}

/// Fetches the latest stable version of `hoist-az` from crates.io.
async fn fetch_latest_version() -> Option<String> {
    let url = format!("https://crates.io/api/v1/crates/{CRATE_NAME}");
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .ok()?;
    let resp = client
        .get(&url)
        .header(
            "User-Agent",
            format!("hoist/{} update-check", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .await
        .ok()?;
    let body: CratesIoResponse = resp.json().await.ok()?;
    Some(body.krate.max_stable_version)
}

/// Determines the install command based on the executable path.
fn detect_install_method() -> String {
    let exe_path = std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::canonicalize(p).ok());

    if let Some(ref path) = exe_path {
        let path_str = path.to_string_lossy();
        if path_str.contains("Cellar") || path_str.contains("homebrew") {
            return "brew upgrade hoist".to_string();
        }
        if let Some(home) = dirs::home_dir() {
            let cargo_bin = home.join(".cargo").join("bin");
            if path.starts_with(&cargo_bin) {
                return "cargo install hoist-az".to_string();
            }
        }
    }

    "Visit https://github.com/mklab-se/hoist/releases".to_string()
}

/// Public entry point: checks for a newer version and returns a formatted
/// notification message, or `None` if the current version is up to date
/// (or on any error).
pub async fn check_for_update() -> Option<String> {
    let cache = load_cache();

    let latest_str = if should_check(cache.as_ref()) {
        let version = fetch_latest_version().await?;
        save_cache(&UpdateCache {
            last_check: Utc::now(),
            latest_version: version.clone(),
        });
        version
    } else {
        cache?.latest_version
    };

    let current = Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    let latest = Version::parse(&latest_str).ok()?;

    if latest <= current {
        return None;
    }

    let install_cmd = detect_install_method();
    Some(format!(
        "{} {} {} {}. Run: {}",
        "Update available:".yellow().bold(),
        current.to_string().dimmed(),
        "\u{2192}".dimmed(),
        latest.to_string().green().bold(),
        install_cmd.cyan(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_check_returns_true_when_no_cache() {
        assert!(should_check(None));
    }

    #[test]
    fn should_check_returns_true_when_cache_is_stale() {
        let old = UpdateCache {
            last_check: Utc::now() - chrono::Duration::hours(25),
            latest_version: "0.1.0".to_string(),
        };
        assert!(should_check(Some(&old)));
    }

    #[test]
    fn should_check_returns_false_when_cache_is_fresh() {
        let recent = UpdateCache {
            last_check: Utc::now() - chrono::Duration::hours(1),
            latest_version: "0.1.0".to_string(),
        };
        assert!(!should_check(Some(&recent)));
    }

    #[test]
    fn cache_serialization_roundtrip() {
        let cache = UpdateCache {
            last_check: Utc::now(),
            latest_version: "1.2.3".to_string(),
        };
        let json = serde_json::to_string(&cache).unwrap();
        let deserialized: UpdateCache = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.latest_version, "1.2.3");
        assert_eq!(
            deserialized.last_check.timestamp(),
            cache.last_check.timestamp()
        );
    }

    #[test]
    fn version_comparison_newer_available() {
        let current = Version::parse("0.1.3").unwrap();
        let latest = Version::parse("0.1.4").unwrap();
        assert!(latest > current);
    }

    #[test]
    fn version_comparison_same() {
        let current = Version::parse("0.1.3").unwrap();
        let latest = Version::parse("0.1.3").unwrap();
        assert!(latest <= current);
    }

    #[test]
    fn version_comparison_older_or_dev() {
        let current = Version::parse("0.2.0").unwrap();
        let latest = Version::parse("0.1.4").unwrap();
        assert!(latest <= current);
    }

    #[test]
    fn detect_install_method_returns_something() {
        let method = detect_install_method();
        assert!(!method.is_empty());
    }
}
