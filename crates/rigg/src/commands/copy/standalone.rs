//! Copy a standalone (non-managed) resource under a new name.

use anyhow::{Result, bail};
use colored::Colorize;

use rigg_core::normalize::format_json;
use rigg_core::resources::ResourceKind;

/// Copy a standalone (non-managed) resource under a new name.
pub(super) fn copy_standalone_resource(
    service_dir: &std::path::Path,
    kind: ResourceKind,
    source: &str,
    target: &str,
) -> Result<()> {
    let resource_dir = service_dir.join(kind.directory_name());
    let source_file = resource_dir.join(format!("{}.json", source));
    let target_file = resource_dir.join(format!("{}.json", target));

    if !source_file.exists() {
        bail!(
            "{} '{}' not found at {}",
            kind.display_name(),
            source,
            source_file.display()
        );
    }
    if target_file.exists() {
        bail!(
            "Target '{}' already exists at {}",
            target,
            target_file.display()
        );
    }

    let content = std::fs::read_to_string(&source_file)?;
    let mut def: serde_json::Value = serde_json::from_str(&content)?;

    // Update name field
    if let Some(obj) = def.as_object_mut() {
        obj.insert(
            "name".to_string(),
            serde_json::Value::String(target.to_string()),
        );
    }

    std::fs::write(&target_file, format_json(&def))?;
    println!(
        "  {} {} '{}' (copied from '{}')",
        "+".green(),
        kind.display_name(),
        target,
        source
    );

    Ok(())
}
