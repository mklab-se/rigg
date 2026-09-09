//! Developer utilities (`rigg dev ...`).

use anyhow::Result;
use colored::Colorize;

use crate::cli::DevCommands;
use crate::commands::GlobalContext;

pub async fn run(ctx: &GlobalContext, cmd: DevCommands) -> Result<()> {
    match cmd {
        DevCommands::ApiCheck => api_check(ctx).await,
        DevCommands::ApiDiff { provider, from, to } => {
            crate::commands::dev_spec::api_diff(&provider, from, to).await
        }
        DevCommands::ApiFixture { provider } => {
            crate::commands::dev_spec::api_fixture(&provider).await
        }
    }
}

const SPECS_REPO: &str = "https://api.github.com/repos/Azure/azure-rest-api-specs/contents";

pub(crate) struct Check {
    label: String,
    channel: &'static str,
    spec_path: &'static str,
    supported: &'static str,
}

/// One check per spec-backed api-version in the registry provider table:
/// a stable-channel row for every provider with a `spec_path`, plus a
/// preview-channel row for every provider with a `preview_spec_path`.
pub(crate) fn checks() -> Vec<Check> {
    let mut out = Vec::new();
    for p in rigg_core::registry::providers() {
        if let Some(path) = p.spec_path {
            out.push(Check {
                label: format!("{} (stable)", p.label),
                channel: "stable",
                spec_path: path,
                supported: p.stable,
            });
        }
        if let (Some(path), Some(preview)) = (p.preview_spec_path, p.preview) {
            out.push(Check {
                label: format!("{} (preview)", p.label),
                channel: "preview",
                spec_path: path,
                supported: preview,
            });
        }
    }
    out
}

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
    let checks = checks();

    for check in &checks {
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

    // Route-versioned providers (Foundry data plane, Graph) have no dated
    // spec folder to compare against; report them informationally only.
    let route_versioned: Vec<_> = rigg_core::registry::providers()
        .iter()
        .filter(|p| p.route_versioned)
        .collect();

    if ctx.json() {
        let mut value: Vec<_> = rows
            .iter()
            .map(|(c, latest, status)| {
                serde_json::json!({
                    "api": c.label,
                    "channel": c.channel,
                    "supported": c.supported,
                    "latest_upstream": latest,
                    "status": status,
                })
            })
            .collect();
        value.extend(route_versioned.iter().map(|p| {
            serde_json::json!({
                "api": p.label,
                "channel": "stable",
                "supported": p.stable,
                "latest_upstream": serde_json::Value::Null,
                "status": "route-versioned",
            })
        }));
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
        for p in &route_versioned {
            println!(
                "  {} {:<45} supported {:<20} (route-versioned)",
                "ⓘ".blue(),
                p.label,
                p.stable
            );
        }
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
    Ok(latest_from_entries(&entries))
}

/// Pick the newest api-version-shaped `name` out of a GitHub contents-API
/// listing. Pure and network-free so it can be unit tested directly.
pub(crate) fn latest_from_entries(entries: &[serde_json::Value]) -> Option<String> {
    let mut versions: Vec<String> = entries
        .iter()
        .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
        .filter(|n| n.len() >= 10 && n.as_bytes()[4] == b'-')
        .map(str::to_string)
        .collect();
    versions.sort();
    versions.pop()
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

    #[test]
    fn latest_from_entries_picks_newest_version_folder() {
        let entries: Vec<serde_json::Value> =
            ["2023-11-01", "2025-05-01", "README.md", "2022-09-01"]
                .iter()
                .map(|n| serde_json::json!({"name": n}))
                .collect();
        assert_eq!(latest_from_entries(&entries).as_deref(), Some("2025-05-01"));
        assert_eq!(latest_from_entries(&[]), None);
    }

    #[test]
    fn every_spec_backed_provider_is_checked() {
        let checks = checks();
        let backed = rigg_core::registry::providers()
            .iter()
            .filter(|p| p.spec_path.is_some())
            .count();
        let previews = rigg_core::registry::providers()
            .iter()
            .filter(|p| p.preview_spec_path.is_some())
            .count();
        assert_eq!(checks.len(), backed + previews);
    }
}
