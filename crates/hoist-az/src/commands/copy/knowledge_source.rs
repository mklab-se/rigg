//! Copy a knowledge source and all its managed sub-resources under new names.

use anyhow::{Result, bail};
use colored::Colorize;

use hoist_core::copy::{NameMap, rewrite_references};
use hoist_core::normalize::format_json;
use hoist_core::resources::ResourceKind;
use hoist_core::resources::managed;

use super::rewrite::{rewrite_created_resources, rewrite_index_projections};

/// Copy a knowledge source and all its managed sub-resources under new names.
pub(super) fn copy_knowledge_source(
    service_dir: &std::path::Path,
    source: &str,
    target: &str,
) -> Result<()> {
    let ks_base = service_dir.join("agentic-retrieval/knowledge-sources");
    let source_dir = ks_base.join(source);
    let target_dir = ks_base.join(target);

    if !source_dir.exists() {
        bail!(
            "Knowledge source '{}' not found at {}",
            source,
            source_dir.display()
        );
    }
    if target_dir.exists() {
        bail!(
            "Target '{}' already exists at {}",
            target,
            target_dir.display()
        );
    }

    // Read source KS definition
    let ks_file = source_dir.join(format!("{}.json", source));
    if !ks_file.exists() {
        bail!("KS definition not found at {}", ks_file.display());
    }
    let ks_content = std::fs::read_to_string(&ks_file)?;
    let ks_def: serde_json::Value = serde_json::from_str(&ks_content)?;

    // Extract managed resources to build name map
    let managed = managed::extract_managed_resources(source, &ks_def);

    let mut name_map = NameMap::new();
    name_map.insert(ResourceKind::KnowledgeSource, source, target);

    if let Some(ref old_name) = managed.index {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::Index, old_name, &new_name);
    }
    if let Some(ref old_name) = managed.indexer {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::Indexer, old_name, &new_name);
    }
    if let Some(ref old_name) = managed.datasource {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::DataSource, old_name, &new_name);
    }
    if let Some(ref old_name) = managed.skillset {
        let new_name = old_name.replacen(source, target, 1);
        name_map.insert(ResourceKind::Skillset, old_name, &new_name);
    }

    // Create target directory
    std::fs::create_dir_all(&target_dir)?;

    println!("Copying knowledge source '{}' -> '{}':", source, target);

    // Copy and rewrite KS definition
    let mut new_ks_def = ks_def.clone();
    if let Some(obj) = new_ks_def.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(target.to_string()),
        );
    }
    let warnings = rewrite_references(ResourceKind::KnowledgeSource, &mut new_ks_def, &name_map);
    for w in &warnings {
        println!("  {} {}", "warning:".yellow(), w);
    }
    rewrite_created_resources(&mut new_ks_def, source, target);

    std::fs::write(
        target_dir.join(format!("{}.json", target)),
        format_json(&new_ks_def),
    )?;
    println!("  {} Knowledge Source '{}'", "+".green(), target);

    // Copy and rewrite managed sub-resources
    let suffixes = [
        ("index", ResourceKind::Index),
        ("indexer", ResourceKind::Indexer),
        ("datasource", ResourceKind::DataSource),
        ("skillset", ResourceKind::Skillset),
    ];

    for (suffix, kind) in &suffixes {
        let source_file = source_dir.join(format!("{}-{}.json", source, suffix));
        if !source_file.exists() {
            continue;
        }

        let content = std::fs::read_to_string(&source_file)?;
        let mut def: serde_json::Value = serde_json::from_str(&content)?;

        let old_name = def
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("")
            .to_string();
        let new_name = name_map
            .get(*kind, &old_name)
            .map(|s| s.to_string())
            .unwrap_or_else(|| old_name.replacen(source, target, 1));

        // Update name field
        if let Some(obj) = def.as_object_mut() {
            obj.insert(
                "name".to_string(),
                serde_json::Value::String(new_name.clone()),
            );
        }

        // Rewrite cross-references (e.g., indexer's dataSourceName, targetIndexName)
        let warnings = rewrite_references(*kind, &mut def, &name_map);
        for w in &warnings {
            println!("  {} {}", "warning:".yellow(), w);
        }

        // For skillsets, also rewrite indexProjections.selectors[].targetIndexName
        if *kind == ResourceKind::Skillset {
            rewrite_index_projections(&mut def, source, target);
        }

        let target_filename = format!("{}-{}.json", target, suffix);
        std::fs::write(target_dir.join(&target_filename), format_json(&def))?;
        println!("  {} {} '{}'", "+".green(), kind.display_name(), new_name);
    }

    println!();
    println!("Copy complete. Run 'hoist push --knowledgesources' to push to Azure.");

    Ok(())
}
