//! `rigg dev api-diff` / `api-fixture`: fetch OpenAPI documents from
//! Azure/azure-rest-api-specs and diff or extract them.

use anyhow::{Context, Result, bail};

use rigg_core::registry::{Provider, providers};

const RAW: &str = "https://raw.githubusercontent.com/Azure/azure-rest-api-specs/main";
const API: &str = "https://api.github.com/repos/Azure/azure-rest-api-specs/contents";

/// Definitions rigg cares about, per provider.
fn definitions(p: Provider) -> &'static [&'static str] {
    match p {
        Provider::SearchData => &[
            "SearchIndexerDataSource",
            "SearchIndex",
            "SearchIndexer",
            "SearchIndexerSkillset",
            "SynonymMap",
            "SearchAlias",
            "KnowledgeSource",
            "KnowledgeBase",
            "WebApiSkill",
            "AzureOpenAIVectorizerParameters",
            "AIServicesAccountIdentity",
            "SearchIndexerDataUserAssignedIdentity",
            "SearchIndexerDataSourceType",
            "KnowledgeSourceKind",
        ],
        Provider::CognitiveServicesArm => &[
            "Account",
            "Project",
            "Identity",
            "DeploymentProperties",
            "ConnectionPropertiesV2",
            "ConnectionAuthType",
            "ConnectionCategory",
            "RaiPolicyProperties",
        ],
        Provider::SearchArm => &[
            "SearchService",
            "SearchServiceProperties",
            "Identity",
            "NetworkRuleSet",
            "SharedPrivateLinkResourceProperties",
        ],
        Provider::StorageArm => &[
            "StorageAccount",
            "StorageAccountProperties",
            "NetworkRuleSet",
            "BlobServiceProperties",
        ],
        Provider::WebArm => &[
            "Site",
            "SiteProperties",
            "SiteConfig",
            "SiteAuthSettingsV2",
            "SiteAuthSettingsV2Properties",
        ],
        _ => &[],
    }
}

fn parse_provider(s: &str) -> Result<&'static rigg_core::registry::ProviderMeta> {
    providers()
        .iter()
        .find(|m| {
            m.label
                .to_ascii_lowercase()
                .replace(' ', "-")
                .contains(&s.to_ascii_lowercase())
                || format!("{:?}", m.provider).eq_ignore_ascii_case(s)
        })
        .with_context(|| {
            format!(
                "unknown provider '{s}' (one of: {})",
                providers()
                    .iter()
                    .map(|m| format!("{:?}", m.provider))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// The OpenAPI document of `version` under `spec_path`: the first `.json` in the version folder.
async fn fetch_openapi(
    http: &reqwest::Client,
    spec_path: &str,
    version: &str,
) -> Result<serde_json::Value> {
    let listing: Vec<serde_json::Value> = http
        .get(format!("{API}/{spec_path}/{version}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let file = listing
        .iter()
        .filter_map(|e| e["name"].as_str())
        .find(|n| n.ends_with(".json"))
        .with_context(|| format!("no .json in {spec_path}/{version}"))?;
    Ok(http
        .get(format!("{RAW}/{spec_path}/{version}/{file}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub async fn api_diff(provider_name: &str, from: Option<String>, to: Option<String>) -> Result<()> {
    let m = parse_provider(provider_name)?;
    let Some(spec_path) = m.spec_path else {
        bail!("{} is route-versioned; nothing to diff", m.label)
    };
    // A `-preview`-suffixed version lives under the provider's preview spec
    // folder, not its stable one — `from`/`to` can straddle both.
    let path_for = |version: &str| -> Result<&'static str> {
        if version.ends_with("-preview") {
            return m
                .preview_spec_path
                .with_context(|| format!("{} has no preview spec folder", m.label));
        }
        Ok(spec_path)
    };
    let http = reqwest::Client::builder()
        .user_agent("rigg-api-diff")
        .build()?;
    let from = from.unwrap_or_else(|| m.stable.to_string());
    let to = match to {
        Some(t) => t,
        None => {
            let l: Vec<serde_json::Value> = http
                .get(format!("{API}/{spec_path}"))
                .header("User-Agent", "rigg")
                .send()
                .await?
                .json()
                .await?;
            crate::commands::dev::latest_from_entries(&l).context("no versions upstream")?
        }
    };
    let (old, new) = (
        fetch_openapi(&http, path_for(&from)?, &from).await?,
        fetch_openapi(&http, path_for(&to)?, &to).await?,
    );
    println!("{}: {from} → {to}", m.label);
    for d in rigg_core::schema::diff_definitions(&old, &new, definitions(m.provider)) {
        if let Some(side) = d.missing_in {
            println!("  {}: missing in {side}", d.definition);
            continue;
        }
        if d.added.is_empty()
            && d.removed.is_empty()
            && d.enum_added.is_empty()
            && d.enum_removed.is_empty()
        {
            continue;
        }
        println!("  {}", d.definition);
        for f in &d.added {
            println!("    + {f}");
        }
        for f in &d.removed {
            println!("    - {f}");
        }
        for (f, v) in &d.enum_added {
            println!("    + {f}: {v}");
        }
        for (f, v) in &d.enum_removed {
            println!("    - {f}: {v}");
        }
    }
    Ok(())
}

pub async fn api_fixture(provider_name: &str) -> Result<()> {
    let m = parse_provider(provider_name)?;
    let Some(spec_path) = m.spec_path else {
        bail!("{} has no OpenAPI document", m.label)
    };
    let http = reqwest::Client::builder()
        .user_agent("rigg-api-fixture")
        .build()?;
    let out_dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rigg-core/fixtures/schema");
    let slug = match m.provider {
        Provider::SearchData => "search-data",
        Provider::CognitiveServicesArm => "cognitiveservices-arm",
        other => bail!("no fixture is kept for {other:?}"),
    };
    let mut versions = vec![(spec_path, m.stable)];
    if let (Some(p), Some(v)) = (m.preview_spec_path, m.preview) {
        versions.push((p, v));
    }
    for (path, version) in versions {
        let doc = fetch_openapi(&http, path, version).await?;
        let defs = rigg_core::schema::extract_fixture(&doc);
        let value =
            serde_json::json!({ "provider": slug, "version": version, "definitions": defs });
        let file = out_dir.join(format!("{slug}-{version}.json"));
        std::fs::write(&file, serde_json::to_string_pretty(&value)?)?;
        println!("wrote {}", file.display());
    }
    Ok(())
}
