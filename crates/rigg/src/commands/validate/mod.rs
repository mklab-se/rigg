//! Validate local configuration

mod field_types;
mod lint;
mod references;

use anyhow::Result;
use serde_json::json;
use std::collections::HashMap;

use rigg_core::resources::ResourceKind;

use crate::cli::OutputFormat;
use crate::commands::load_config_and_env;

use field_types::validate_field_types;
use lint::lint_resources;
use references::validate_references;

pub async fn run(
    strict: bool,
    check_references: bool,
    output: OutputFormat,
    env_override: Option<&str>,
) -> Result<()> {
    let (project_root, config, env) = load_config_and_env(env_override)?;
    let files_root = config.files_root(&project_root);

    if matches!(output, OutputFormat::Text) {
        println!("Validating project at {}", project_root.display());
        println!();
    }

    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Collect all resources
    let mut resources: HashMap<ResourceKind, Vec<(String, serde_json::Value)>> = HashMap::new();

    let kinds = if env.sync.include_preview {
        ResourceKind::all()
    } else {
        ResourceKind::stable()
    };

    let primary_search = env.primary_search_service();

    for kind in kinds {
        if kind.domain() == rigg_core::service::ServiceDomain::Foundry {
            continue; // Agent validation is handled below
        }

        let resource_dir = match primary_search {
            Some(svc) => env
                .search_service_dir(&files_root, svc)
                .join(kind.directory_name()),
            None => continue,
        };
        if !resource_dir.exists() {
            continue;
        }

        let mut kind_resources = Vec::new();

        if *kind == ResourceKind::KnowledgeSource {
            // KS are stored as subdirectories: <ks-name>/<ks-name>.json
            for entry in std::fs::read_dir(&resource_dir)? {
                let entry = entry?;
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };
                let ks_file = path.join(format!("{}.json", name));
                if !ks_file.exists() {
                    errors.push(format!(
                        "{}/{}/{}.json: missing KS definition file",
                        kind.directory_name(),
                        name,
                        name
                    ));
                    continue;
                }
                let content = std::fs::read_to_string(&ks_file)?;
                match serde_json::from_str::<serde_json::Value>(&content) {
                    Ok(value) => {
                        if let Some(json_name) = value.get("name").and_then(|n| n.as_str()) {
                            if json_name != name {
                                errors.push(format!(
                                    "{}/{}/{}.json: name field '{}' doesn't match directory",
                                    kind.directory_name(),
                                    name,
                                    name,
                                    json_name
                                ));
                            }
                        } else {
                            errors.push(format!(
                                "{}/{}/{}.json: missing required 'name' field",
                                kind.directory_name(),
                                name,
                                name
                            ));
                        }
                        kind_resources.push((name, value));
                    }
                    Err(e) => {
                        errors.push(format!(
                            "{}/{}/{}.json: invalid JSON - {}",
                            kind.directory_name(),
                            name,
                            name,
                            e
                        ));
                    }
                }
            }
        } else {
            // Standard flat JSON files
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
        }

        resources.insert(*kind, kind_resources);
    }

    // Validate Foundry agents
    if env.has_foundry() {
        let mut agent_resources = Vec::new();
        for foundry_config in &env.foundry {
            let agents_dir = env
                .foundry_service_dir(&files_root, foundry_config)
                .join("agents");
            if !agents_dir.exists() {
                continue;
            }
            for entry in std::fs::read_dir(&agents_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                    continue;
                }
                let name = match path.file_stem().and_then(|n| n.to_str()) {
                    Some(n) => n.to_string(),
                    None => continue,
                };

                if let Some(resource) =
                    validate_agent_yaml(&path, &name, &mut errors, &mut warnings)
                {
                    agent_resources.push(resource);
                }
            }
        }
        if !agent_resources.is_empty() {
            resources.insert(ResourceKind::Agent, agent_resources);
        }
    }

    // Run lint checks
    lint_resources(&resources, &mut warnings);

    // Validate index field types
    if let Some(indexes) = resources.get(&ResourceKind::Index) {
        for (name, value) in indexes {
            if let Some(fields) = value.get("fields").and_then(|f| f.as_array()) {
                validate_field_types(name, fields, "", &mut errors);
            }
        }
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
    let passed = errors.is_empty();

    match output {
        OutputFormat::Json => {
            let result = json!({
                "total_resources": total_resources,
                "errors": errors,
                "error_count": errors.len(),
                "warnings": warnings,
                "warning_count": warnings.len(),
                "passed": passed,
                "include_preview": env.sync.include_preview,
            });
            println!("{}", serde_json::to_string_pretty(&result)?);
            if !passed {
                anyhow::bail!("Validation failed with {} error(s)", errors.len());
            }
        }
        OutputFormat::Text => {
            println!("Scanned {} resources", total_resources);

            if !warnings.is_empty() {
                println!();
                println!("Warnings ({}):", warnings.len());
                for warning in &warnings {
                    println!("  ! {}", warning);
                }
            }

            if !passed {
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

            if env.sync.include_preview {
                println!("Note: Includes preview resources (knowledge bases, knowledge sources).");
            } else {
                println!();
                println!(
                    "Note: Preview resources (knowledge bases, knowledge sources) not validated."
                );
                println!("      Set sync.include_preview = true to include them.");
            }

            if env.has_foundry() {
                let agent_count = resources
                    .get(&ResourceKind::Agent)
                    .map(|v| v.len())
                    .unwrap_or(0);
                if agent_count > 0 {
                    println!("      Validated {} Foundry agent(s).", agent_count);
                }
            }
        }
    }

    Ok(())
}

/// Validate a single agent YAML file. Returns the parsed value on success,
/// pushing any issues into the errors/warnings vecs.
fn validate_agent_yaml(
    yaml_path: &std::path::Path,
    name: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Option<(String, serde_json::Value)> {
    let content = match std::fs::read_to_string(yaml_path) {
        Ok(c) => c,
        Err(e) => {
            errors.push(format!("agents/{}.yaml: read error - {}", name, e));
            return None;
        }
    };

    match serde_yaml::from_str::<serde_json::Value>(&content) {
        Ok(value) => {
            // Agent name is derived from filename — no name field to validate

            // Validate model field exists
            if value.get("model").and_then(|m| m.as_str()).is_none() {
                warnings.push(format!("agents/{}.yaml: missing 'model' field", name));
            }

            // Validate instructions field exists (warning, not error)
            let has_instructions = value
                .get("instructions")
                .and_then(|i| i.as_str())
                .is_some_and(|s| !s.is_empty());
            if !has_instructions {
                warnings.push(format!(
                    "agents/{}.yaml: missing or empty 'instructions'",
                    name
                ));
            }

            Some((name.to_string(), value))
        }
        Err(e) => {
            errors.push(format!("agents/{}.yaml: invalid YAML - {}", name, e));
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_agent_yaml_full() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("my-agent.yaml");

        std::fs::write(
            &yaml_path,
            "kind: prompt\nmodel: gpt-4o\ninstructions: You are a helpful assistant.\n",
        )
        .unwrap();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let result = validate_agent_yaml(&yaml_path, "my-agent", &mut errors, &mut warnings);

        assert!(result.is_some());
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
        assert_eq!(result.unwrap().0, "my-agent");
    }

    #[test]
    fn test_validate_agent_yaml_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("bad.yaml");
        std::fs::write(&yaml_path, "{{invalid yaml").unwrap();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let result = validate_agent_yaml(&yaml_path, "bad", &mut errors, &mut warnings);

        assert!(result.is_none());
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("invalid YAML"));
    }

    #[test]
    fn test_validate_agent_yaml_missing_model_warning() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("my-agent.yaml");

        std::fs::write(&yaml_path, "kind: prompt\ninstructions: Be helpful.\n").unwrap();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_agent_yaml(&yaml_path, "my-agent", &mut errors, &mut warnings);

        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing 'model' field"));
    }

    #[test]
    fn test_validate_agent_yaml_missing_instructions_warning() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("my-agent.yaml");

        std::fs::write(&yaml_path, "kind: prompt\nmodel: gpt-4o\n").unwrap();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_agent_yaml(&yaml_path, "my-agent", &mut errors, &mut warnings);

        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("missing or empty 'instructions'"));
    }

    #[test]
    fn test_validate_agent_yaml_multiple_issues() {
        let dir = tempfile::tempdir().unwrap();
        let yaml_path = dir.path().join("my-agent.yaml");

        // No model + no instructions
        std::fs::write(&yaml_path, "kind: prompt\n").unwrap();

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_agent_yaml(&yaml_path, "my-agent", &mut errors, &mut warnings);

        assert!(errors.is_empty());
        assert_eq!(warnings.len(), 2); // missing model + missing instructions
    }
}
