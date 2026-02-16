//! Validate local configuration

use anyhow::Result;
use serde_json::json;
use std::collections::{HashMap, HashSet};

use hoist_core::resources::ResourceKind;

use crate::cli::OutputFormat;
use crate::commands::load_config_and_env;

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
        if kind.domain() == hoist_core::service::ServiceDomain::Foundry {
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

/// Field count threshold for the "large index" lint warning.
const LARGE_FIELD_COUNT_THRESHOLD: usize = 50;

fn lint_resources(
    resources: &HashMap<ResourceKind, Vec<(String, serde_json::Value)>>,
    warnings: &mut Vec<String>,
) {
    // Lint indexes
    if let Some(indexes) = resources.get(&ResourceKind::Index) {
        for (name, value) in indexes {
            lint_index(name, value, warnings);
        }
    }

    // Lint indexers
    if let Some(indexers) = resources.get(&ResourceKind::Indexer) {
        for (name, value) in indexers {
            lint_indexer(name, value, warnings);
        }
    }

    // Lint data sources
    if let Some(datasources) = resources.get(&ResourceKind::DataSource) {
        for (name, value) in datasources {
            lint_datasource(name, value, warnings);
        }
    }
}

fn lint_index(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    if let Some(fields) = value.get("fields").and_then(|f| f.as_array()) {
        // Check for missing key field
        let has_key = fields
            .iter()
            .any(|f| f.get("key").and_then(|k| k.as_bool()).unwrap_or(false));
        if !has_key {
            warnings.push(format!(
                "indexes/{}.json: no field has \"key\": true — index has no key field",
                name
            ));
        }

        // Check for large field count
        let field_count = fields.len();
        if field_count > LARGE_FIELD_COUNT_THRESHOLD {
            warnings.push(format!(
                "indexes/{}.json: index has {} fields (threshold: {}), which may impact performance",
                name, field_count, LARGE_FIELD_COUNT_THRESHOLD
            ));
        }
    }
}

fn lint_indexer(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    // Check for missing or null schedule
    let has_schedule = value
        .get("schedule")
        .is_some_and(|s| !s.is_null() && s.get("interval").is_some());
    if !has_schedule {
        warnings.push(format!(
            "indexers/{}.json: no schedule defined — indexer will only run when triggered manually",
            name
        ));
    }
}

fn lint_datasource(name: &str, value: &serde_json::Value, warnings: &mut Vec<String>) {
    // Check for empty or missing container name
    let container_name = value
        .get("container")
        .and_then(|c| c.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("");

    if container_name.is_empty() {
        warnings.push(format!(
            "data-sources/{}.json: container name is empty or missing",
            name
        ));
    }
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

    // Validate knowledge source references
    if let Some(knowledge_sources) = resources.get(&ResourceKind::KnowledgeSource) {
        let knowledge_bases: HashSet<_> = resources
            .get(&ResourceKind::KnowledgeBase)
            .map(|r| r.iter().map(|(n, _)| n.as_str()).collect())
            .unwrap_or_default();

        for (name, value) in knowledge_sources {
            // Check index reference
            if let Some(idx_name) = value.get("indexName").and_then(|n| n.as_str()) {
                if !indexes.contains(idx_name) {
                    errors.push(format!(
                        "knowledge-sources/{}.json: references missing index '{}'",
                        name, idx_name
                    ));
                }
            }

            // Check knowledge base reference (optional)
            if let Some(kb_name) = value.get("knowledgeBaseName").and_then(|n| n.as_str()) {
                if !knowledge_bases.contains(kb_name) {
                    errors.push(format!(
                        "knowledge-sources/{}.json: references missing knowledge base '{}'",
                        name, kb_name
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_resources(
        entries: Vec<(ResourceKind, Vec<(&str, serde_json::Value)>)>,
    ) -> HashMap<ResourceKind, Vec<(String, serde_json::Value)>> {
        entries
            .into_iter()
            .map(|(kind, items)| {
                (
                    kind,
                    items
                        .into_iter()
                        .map(|(name, val)| (name.to_string(), val))
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn test_valid_references_pass() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("my-index", json!({"name": "my-index", "fields": []}))],
            ),
            (
                ResourceKind::DataSource,
                vec![(
                    "my-ds",
                    json!({"name": "my-ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "my-ds",
                        "targetIndexName": "my-index"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty(), "Expected no errors, got: {:?}", errors);
    }

    #[test]
    fn test_missing_datasource_reference() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "missing-ds",
                        "targetIndexName": "idx"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing data source 'missing-ds'"));
    }

    #[test]
    fn test_missing_index_reference() {
        let resources = make_resources(vec![
            (
                ResourceKind::DataSource,
                vec![(
                    "ds",
                    json!({"name": "ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "ds",
                        "targetIndexName": "missing-index"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing index 'missing-index'"));
    }

    #[test]
    fn test_missing_skillset_reference() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::DataSource,
                vec![(
                    "ds",
                    json!({"name": "ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "ds",
                        "targetIndexName": "idx",
                        "skillsetName": "missing-skillset"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing skillset 'missing-skillset'"));
    }

    #[test]
    fn test_missing_synonym_map_reference() {
        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "my-index",
                json!({
                    "name": "my-index",
                    "fields": [
                        {"name": "title", "type": "Edm.String", "synonymMaps": ["missing-syn"]}
                    ]
                }),
            )],
        )]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing synonym map 'missing-syn'"));
        assert!(errors[0].contains("field 'title'"));
    }

    #[test]
    fn test_indexer_without_skillset_passes() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::DataSource,
                vec![(
                    "ds",
                    json!({"name": "ds", "type": "azureblob", "credentials": {}, "container": {"name": "c"}}),
                )],
            ),
            (
                ResourceKind::Indexer,
                vec![(
                    "my-indexer",
                    json!({
                        "name": "my-indexer",
                        "dataSourceName": "ds",
                        "targetIndexName": "idx"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_empty_resources_passes() {
        let resources: HashMap<ResourceKind, Vec<(String, serde_json::Value)>> = HashMap::new();
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_knowledge_source_missing_index() {
        let resources = make_resources(vec![(
            ResourceKind::KnowledgeSource,
            vec![(
                "ks1",
                json!({
                    "name": "ks1",
                    "indexName": "missing-index"
                }),
            )],
        )]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing index 'missing-index'"));
    }

    #[test]
    fn test_knowledge_source_missing_knowledge_base() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::KnowledgeSource,
                vec![(
                    "ks1",
                    json!({
                        "name": "ks1",
                        "indexName": "idx",
                        "knowledgeBaseName": "missing-kb"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("missing knowledge base 'missing-kb'"));
    }

    #[test]
    fn test_knowledge_source_valid_references() {
        let resources = make_resources(vec![
            (
                ResourceKind::Index,
                vec![("idx", json!({"name": "idx", "fields": []}))],
            ),
            (
                ResourceKind::KnowledgeBase,
                vec![("kb1", json!({"name": "kb1"}))],
            ),
            (
                ResourceKind::KnowledgeSource,
                vec![(
                    "ks1",
                    json!({
                        "name": "ks1",
                        "indexName": "idx",
                        "knowledgeBaseName": "kb1"
                    }),
                )],
            ),
        ]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_multiple_errors_accumulated() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "missing-ds",
                    "targetIndexName": "missing-idx",
                    "skillsetName": "missing-ss"
                }),
            )],
        )]);

        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        validate_references(&resources, &mut errors, &mut warnings);
        assert_eq!(errors.len(), 3);
    }

    // ---- Lint tests ----

    #[test]
    fn test_lint_index_no_key_field() {
        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "my-index",
                json!({
                    "name": "my-index",
                    "fields": [
                        {"name": "title", "type": "Edm.String", "key": false},
                        {"name": "content", "type": "Edm.String"}
                    ]
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no key field"));
        assert!(warnings[0].contains("my-index"));
    }

    #[test]
    fn test_lint_index_with_key_field_no_warning() {
        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "my-index",
                json!({
                    "name": "my-index",
                    "fields": [
                        {"name": "id", "type": "Edm.String", "key": true},
                        {"name": "title", "type": "Edm.String"}
                    ]
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_indexer_no_schedule() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx"
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no schedule defined"));
        assert!(warnings[0].contains("my-indexer"));
        assert!(warnings[0].contains("manually"));
    }

    #[test]
    fn test_lint_indexer_null_schedule() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx",
                    "schedule": null
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no schedule defined"));
    }

    #[test]
    fn test_lint_indexer_with_schedule_no_warning() {
        let resources = make_resources(vec![(
            ResourceKind::Indexer,
            vec![(
                "my-indexer",
                json!({
                    "name": "my-indexer",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx",
                    "schedule": {"interval": "PT5M"}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_index_large_field_count() {
        let mut fields = Vec::new();
        for i in 0..55 {
            fields.push(json!({"name": format!("field_{}", i), "type": "Edm.String"}));
        }

        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "big-index",
                json!({
                    "name": "big-index",
                    "fields": fields
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        // Should have 2 warnings: no key field + large field count
        assert_eq!(warnings.len(), 2);
        let large_warning = warnings.iter().find(|w| w.contains("55 fields"));
        assert!(
            large_warning.is_some(),
            "Expected large field count warning"
        );
        assert!(large_warning.unwrap().contains("big-index"));
    }

    #[test]
    fn test_lint_index_at_threshold_no_warning() {
        let mut fields = Vec::new();
        for i in 0..49 {
            fields.push(json!({"name": format!("field_{}", i), "type": "Edm.String"}));
        }
        fields.push(json!({"name": "id", "type": "Edm.String", "key": true}));
        // 50 fields total — at threshold, not above

        let resources = make_resources(vec![(
            ResourceKind::Index,
            vec![(
                "normal-index",
                json!({
                    "name": "normal-index",
                    "fields": fields
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings at threshold, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_datasource_empty_container_name() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {},
                    "container": {"name": ""}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("container name is empty or missing"));
        assert!(warnings[0].contains("my-ds"));
    }

    #[test]
    fn test_lint_datasource_missing_container() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("container name is empty or missing"));
    }

    #[test]
    fn test_lint_datasource_missing_container_name_field() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {},
                    "container": {}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("container name is empty or missing"));
    }

    #[test]
    fn test_lint_datasource_valid_container_no_warning() {
        let resources = make_resources(vec![(
            ResourceKind::DataSource,
            vec![(
                "my-ds",
                json!({
                    "name": "my-ds",
                    "type": "azureblob",
                    "credentials": {},
                    "container": {"name": "my-container"}
                }),
            )],
        )]);

        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    #[test]
    fn test_lint_no_resources_no_warnings() {
        let resources: HashMap<ResourceKind, Vec<(String, serde_json::Value)>> = HashMap::new();
        let mut warnings = Vec::new();
        lint_resources(&resources, &mut warnings);
        assert!(warnings.is_empty());
    }

    // === Foundry agent validation tests ===

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
