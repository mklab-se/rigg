//! Validate local configuration

use anyhow::Result;
use std::collections::{HashMap, HashSet};

use hoist_core::resources::ResourceKind;

use crate::commands::load_config;

pub async fn run(strict: bool, check_references: bool) -> Result<()> {
    let (project_root, config) = load_config()?;

    println!("Validating project at {}", project_root.display());
    println!();

    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Collect all resources
    let mut resources: HashMap<ResourceKind, Vec<(String, serde_json::Value)>> = HashMap::new();

    for kind in ResourceKind::stable() {
        let resource_dir = config
            .resource_dir(&project_root)
            .join(kind.directory_name());
        if !resource_dir.exists() {
            continue;
        }

        let mut kind_resources = Vec::new();

        for entry in std::fs::read_dir(&resource_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }

            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid file name"))?
                .to_string();

            // Parse JSON
            let content = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(value) => {
                    // Validate JSON name matches filename
                    if let Some(json_name) = value.get("name").and_then(|n| n.as_str()) {
                        if json_name != name {
                            errors.push(format!(
                                "{}/{}.json: name field '{}' doesn't match filename",
                                kind.directory_name(),
                                name,
                                json_name
                            ));
                        }
                    } else {
                        errors.push(format!(
                            "{}/{}.json: missing required 'name' field",
                            kind.directory_name(),
                            name
                        ));
                    }

                    kind_resources.push((name, value));
                }
                Err(e) => {
                    errors.push(format!(
                        "{}/{}.json: invalid JSON - {}",
                        kind.directory_name(),
                        name,
                        e
                    ));
                }
            }
        }

        resources.insert(*kind, kind_resources);
    }

    // Check references if requested
    if check_references {
        validate_references(&resources, &mut errors, &mut warnings);
    }

    // Strict mode: treat warnings as errors
    if strict {
        errors.append(&mut warnings);
    }

    // Report results
    let total_resources: usize = resources.values().map(|v| v.len()).sum();

    println!("Scanned {} resources", total_resources);

    if !warnings.is_empty() {
        println!();
        println!("Warnings ({}):", warnings.len());
        for warning in &warnings {
            println!("  ! {}", warning);
        }
    }

    if !errors.is_empty() {
        println!();
        println!("Errors ({}):", errors.len());
        for error in &errors {
            println!("  x {}", error);
        }
        println!();
        anyhow::bail!("Validation failed with {} error(s)", errors.len());
    }

    println!();
    println!("Validation passed!");

    if !config.sync.include_preview {
        println!();
        println!("Note: Preview resources (knowledge bases, knowledge sources) not validated.");
        println!("      Set sync.include_preview = true to include them.");
    }

    Ok(())
}

fn validate_references(
    resources: &HashMap<ResourceKind, Vec<(String, serde_json::Value)>>,
    errors: &mut Vec<String>,
    _warnings: &mut Vec<String>,
) {
    // Build lookup sets
    let indexes: HashSet<_> = resources
        .get(&ResourceKind::Index)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    let datasources: HashSet<_> = resources
        .get(&ResourceKind::DataSource)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    let skillsets: HashSet<_> = resources
        .get(&ResourceKind::Skillset)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    let synonym_maps: HashSet<_> = resources
        .get(&ResourceKind::SynonymMap)
        .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();

    // Validate indexer references
    if let Some(indexers) = resources.get(&ResourceKind::Indexer) {
        for (name, value) in indexers {
            // Check data source reference
            if let Some(ds_name) = value.get("dataSourceName").and_then(|n| n.as_str()) {
                if !datasources.contains(ds_name) {
                    errors.push(format!(
                        "indexers/{}.json: references missing data source '{}'",
                        name, ds_name
                    ));
                }
            }

            // Check target index reference
            if let Some(idx_name) = value.get("targetIndexName").and_then(|n| n.as_str()) {
                if !indexes.contains(idx_name) {
                    errors.push(format!(
                        "indexers/{}.json: references missing index '{}'",
                        name, idx_name
                    ));
                }
            }

            // Check skillset reference (optional)
            if let Some(ss_name) = value.get("skillsetName").and_then(|n| n.as_str()) {
                if !skillsets.contains(ss_name) {
                    errors.push(format!(
                        "indexers/{}.json: references missing skillset '{}'",
                        name, ss_name
                    ));
                }
            }
        }
    }

    // Validate index synonym map references
    if let Some(indexes_list) = resources.get(&ResourceKind::Index) {
        for (name, value) in indexes_list {
            if let Some(fields) = value.get("fields").and_then(|f| f.as_array()) {
                for field in fields {
                    if let Some(syn_maps) = field.get("synonymMaps").and_then(|s| s.as_array()) {
                        for syn_map in syn_maps {
                            if let Some(syn_name) = syn_map.as_str() {
                                if !synonym_maps.contains(syn_name) {
                                    let field_name = field
                                        .get("name")
                                        .and_then(|n| n.as_str())
                                        .unwrap_or("unknown");
                                    errors.push(format!(
                                        "indexes/{}.json: field '{}' references missing synonym map '{}'",
                                        name, field_name, syn_name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
