//! Dependency validation for Azure AI Search resources

use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::resources::ResourceKind;

/// Dependency violation error
#[derive(Debug, Error)]
#[error("{kind} '{name}' cannot be deleted: it is referenced by {dependents}. {suggestion}")]
pub struct DependencyViolation {
    pub kind: ResourceKind,
    pub name: String,
    pub dependents: String,
    pub suggestion: String,
}

/// A resource identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceId {
    pub kind: ResourceKind,
    pub name: String,
}

impl ResourceId {
    pub fn new(kind: ResourceKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
        }
    }
}

/// Check for dependency violations when deleting resources
pub fn check_dependencies(
    resources_to_delete: &[ResourceId],
    all_dependencies: &HashMap<ResourceId, Vec<ResourceId>>,
) -> Vec<DependencyViolation> {
    let mut violations = Vec::new();
    let delete_set: HashSet<_> = resources_to_delete.iter().collect();

    // Build reverse dependency map (what depends on what)
    let mut dependents_map: HashMap<&ResourceId, Vec<&ResourceId>> = HashMap::new();
    for (resource, deps) in all_dependencies {
        for dep in deps {
            dependents_map.entry(dep).or_default().push(resource);
        }
    }

    // Check each resource being deleted
    for resource in resources_to_delete {
        if let Some(dependents) = dependents_map.get(resource) {
            // Filter out dependents that are also being deleted
            let remaining_dependents: Vec<_> = dependents
                .iter()
                .filter(|d| !delete_set.contains(*d))
                .collect();

            if !remaining_dependents.is_empty() {
                let dependent_names: Vec<_> = remaining_dependents
                    .iter()
                    .map(|d| format!("{} '{}'", d.kind, d.name))
                    .collect();

                violations.push(DependencyViolation {
                    kind: resource.kind,
                    name: resource.name.clone(),
                    dependents: dependent_names.join(", "),
                    suggestion: "Delete or modify the dependent resources first, or use --force to override.".to_string(),
                });
            }
        }
    }

    violations
}

/// Build a dependency map from a set of resources
pub fn build_dependency_map<F>(
    resources: &[(ResourceKind, String)],
    get_dependencies: F,
) -> HashMap<ResourceId, Vec<ResourceId>>
where
    F: Fn(&ResourceKind, &str) -> Vec<(ResourceKind, String)>,
{
    resources
        .iter()
        .map(|(kind, name)| {
            let id = ResourceId::new(*kind, name.clone());
            let deps = get_dependencies(kind, name)
                .into_iter()
                .map(|(k, n)| ResourceId::new(k, n))
                .collect();
            (id, deps)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_violation_when_no_dependents() {
        let to_delete = vec![ResourceId::new(ResourceKind::Index, "test-index")];
        let dependencies = HashMap::new();

        let violations = check_dependencies(&to_delete, &dependencies);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_violation_when_indexer_depends_on_datasource() {
        let ds = ResourceId::new(ResourceKind::DataSource, "my-datasource");
        let indexer = ResourceId::new(ResourceKind::Indexer, "my-indexer");

        let mut dependencies = HashMap::new();
        dependencies.insert(indexer.clone(), vec![ds.clone()]);

        let to_delete = vec![ds.clone()];
        let violations = check_dependencies(&to_delete, &dependencies);

        assert_eq!(violations.len(), 1);
        assert!(violations[0].dependents.contains("my-indexer"));
    }

    #[test]
    fn test_no_violation_when_deleting_both() {
        let ds = ResourceId::new(ResourceKind::DataSource, "my-datasource");
        let indexer = ResourceId::new(ResourceKind::Indexer, "my-indexer");

        let mut dependencies = HashMap::new();
        dependencies.insert(indexer.clone(), vec![ds.clone()]);

        // Both are being deleted
        let to_delete = vec![ds.clone(), indexer.clone()];
        let violations = check_dependencies(&to_delete, &dependencies);

        assert!(violations.is_empty());
    }
}
