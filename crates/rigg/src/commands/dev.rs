//! Developer utilities (`rigg dev ...`).

use anyhow::Result;
use colored::Colorize;

use crate::cli::DevCommands;
use crate::commands::GlobalContext;

pub async fn run(ctx: &GlobalContext, cmd: DevCommands) -> Result<()> {
    match cmd {
        DevCommands::ApiCheck => api_check(ctx).await,
    }
}

const SPECS_REPO: &str = "https://api.github.com/repos/Azure/azure-rest-api-specs/contents";

struct Check {
    label: &'static str,
    spec_path: &'static str,
    supported: &'static str,
}

const CHECKS: &[Check] = &[
    Check {
        label: "Azure AI Search data plane (stable)",
        spec_path: "specification/search/data-plane/Search/stable",
        supported: rigg_core::registry::SEARCH_STABLE_API_VERSION,
    },
    Check {
        label: "Azure AI Search data plane (preview)",
        spec_path: "specification/search/data-plane/Search/preview",
        supported: rigg_core::registry::SEARCH_PREVIEW_API_VERSION,
    },
    Check {
        label: "Microsoft.CognitiveServices ARM (stable)",
        spec_path: "specification/cognitiveservices/resource-manager/Microsoft.CognitiveServices/stable",
        supported: rigg_core::registry::ARM_COGNITIVE_API_VERSION,
    },
];

/// Compare rigg's supported Azure API versions against the newest published
/// in Azure/azure-rest-api-specs. Exit 1 when upstream is ahead; network
/// failures never fail the command (sessions must work offline).
async fn api_check(ctx: &GlobalContext) -> Result<()> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .user_agent("rigg-api-check")
        .build()?;

    let mut behind = false;
    let mut rows = Vec::new();

    for check in CHECKS {
        let url = format!("{SPECS_REPO}/{}", check.spec_path);
        let latest = match fetch_latest_version(&http, &url).await {
            Ok(Some(latest)) => latest,
            Ok(None) => {
                rows.push((
                    check,
                    "?".to_string(),
                    "no versions found upstream".to_string(),
                ));
                continue;
            }
            Err(e) => {
                rows.push((check, "?".to_string(), format!("lookup failed: {e}")));
                continue;
            }
        };
        let status = if version_newer(&latest, check.supported) {
            behind = true;
            "BEHIND".to_string()
        } else {
            "current".to_string()
        };
        rows.push((check, latest, status));
    }

    // Foundry v1 data plane is unversioned-by-date; informational only.
    if ctx.json() {
        let value: Vec<_> = rows
            .iter()
            .map(|(c, latest, status)| {
                serde_json::json!({
                    "api": c.label,
                    "supported": c.supported,
                    "latest_upstream": latest,
                    "status": status,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        for (c, latest, status) in &rows {
            let marker = match status.as_str() {
                "current" => "✓".green().bold().to_string(),
                "BEHIND" => "✗".red().bold().to_string(),
                _ => "?".yellow().bold().to_string(),
            };
            println!(
                "  {marker} {:<45} supported {:<20} upstream {:<20} {status}",
                c.label, c.supported, latest
            );
        }
        println!(
            "  {} Microsoft Foundry data plane                 supported {:<20} (route-versioned: v1)",
            "ⓘ".blue(),
            rigg_core::registry::FOUNDRY_API_VERSION
        );
    }

    if behind {
        println!();
        println!(
            "{} Azure has newer API versions than rigg supports.",
            "action needed:".red().bold()
        );
        println!("  1. Read the changelog for the new version(s) on learn.microsoft.com");
        println!("  2. Update the constants + capability data in crates/rigg-core/src/registry.rs");
        println!("  3. Re-verify against the design spec (docs/superpowers/specs/…) section 2");
        anyhow::bail!("rigg is behind the latest Azure API versions");
    }
    Ok(())
}

/// List a specs directory and return the newest api-version-shaped entry.
async fn fetch_latest_version(http: &reqwest::Client, url: &str) -> Result<Option<String>> {
    let response = http.get(url).send().await?;
    if !response.status().is_success() {
        anyhow::bail!("GitHub API returned {}", response.status());
    }
    let entries: Vec<serde_json::Value> = response.json().await?;
    let mut versions: Vec<String> = entries
        .iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .filter(|n| n.len() >= 10 && n.as_bytes()[4] == b'-')
        .map(str::to_string)
        .collect();
    versions.sort();
    Ok(versions.pop())
}

/// Is `a` a newer api-version than `b`? Date-prefix comparison; a dated
/// version with a `-preview` suffix compares by date first.
fn version_newer(a: &str, b: &str) -> bool {
    let date = |s: &str| s.get(..10).unwrap_or(s).to_string();
    date(a) > date(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert!(version_newer("2026-05-01", "2026-04-01"));
        assert!(version_newer("2026-06-01-preview", "2026-05-01-preview"));
        assert!(!version_newer("2026-04-01", "2026-04-01"));
        assert!(!version_newer("2025-11-01-preview", "2026-05-01-preview"));
    }
}
