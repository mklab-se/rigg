//! Reference-graph ordering for push and delete.
//!
//! Order is computed from actual references extracted via the registry
//! (`registry::extract_references`), never declared. References to resources
//! outside the given set are ignored — they may already exist in Azure or in
//! another project.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use thiserror::Error;

use crate::registry;
use crate::resources::traits::ResourceRef;

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("dependency cycle involving: {}", .0.join(" -> "))]
    Cycle(Vec<String>),
}

/// Topological order for pushing: dependencies before dependents.
/// Independent resources are ordered deterministically (kind, then name).
pub fn push_order(items: &[(ResourceRef, Value)]) -> Result<Vec<ResourceRef>, GraphError> {
    let in_set: BTreeSet<&ResourceRef> = items.iter().map(|(r, _)| r).collect();

    // edges: node -> set of dependencies (within the set)
    let mut deps: BTreeMap<&ResourceRef, BTreeSet<&ResourceRef>> = BTreeMap::new();
    for (r, body) in items {
        let entry = deps.entry(r).or_default();
        for (kind, name) in registry::extract_references(r.kind, body) {
            let dep = ResourceRef { kind, name };
            if let Some(&found) = in_set.get(&dep) {
                if found != r {
                    entry.insert(found);
                }
            }
        }
    }

    // Kahn's algorithm over the BTreeMap (deterministic iteration order).
    let mut order = Vec::with_capacity(items.len());
    let mut remaining: BTreeMap<&ResourceRef, BTreeSet<&ResourceRef>> = deps;
    while !remaining.is_empty() {
        let ready: Vec<&ResourceRef> = remaining
            .iter()
            .filter(|(_, d)| d.is_empty())
            .map(|(r, _)| *r)
            .collect();
        if ready.is_empty() {
            // Cycle: report the smallest strongly-connected remainder.
            let cycle: Vec<String> = remaining.keys().map(|r| r.to_string()).collect();
            return Err(GraphError::Cycle(cycle));
        }
        for r in &ready {
            remaining.remove(*r);
        }
        for d in remaining.values_mut() {
            for r in &ready {
                d.remove(*r);
            }
        }
        order.extend(ready.into_iter().cloned());
    }
    Ok(order)
}

/// Topological order for deleting: dependents before dependencies.
pub fn delete_order(items: &[(ResourceRef, Value)]) -> Result<Vec<ResourceRef>, GraphError> {
    let mut order = push_order(items)?;
    order.reverse();
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::traits::ResourceKind;
    use serde_json::json;

    fn r(kind: ResourceKind, name: &str) -> ResourceRef {
        ResourceRef::new(kind, name)
    }

    fn full_chain() -> Vec<(ResourceRef, Value)> {
        vec![
            (
                r(ResourceKind::KnowledgeBase, "kb"),
                json!({"name": "kb", "knowledgeSources": [{"name": "ks"}]}),
            ),
            (
                r(ResourceKind::Indexer, "idxr"),
                json!({
                    "name": "idxr",
                    "dataSourceName": "ds",
                    "targetIndexName": "idx",
                    "skillsetName": "skills"
                }),
            ),
            (
                r(ResourceKind::KnowledgeSource, "ks"),
                json!({
                    "name": "ks",
                    "kind": "searchIndex",
                    "searchIndexParameters": {"searchIndexName": "idx"}
                }),
            ),
            (r(ResourceKind::Index, "idx"), json!({"name": "idx"})),
            (
                r(ResourceKind::Skillset, "skills"),
                json!({"name": "skills"}),
            ),
            (r(ResourceKind::DataSource, "ds"), json!({"name": "ds"})),
            (
                r(ResourceKind::Agent, "agent"),
                json!({
                    "name": "agent",
                    "model": "gpt-5-mini",
                    "tools": [{"type": "mcp", "x-rigg-ref": "knowledge-bases/kb"}]
                }),
            ),
            (
                r(ResourceKind::Deployment, "gpt-5-mini"),
                json!({"name": "gpt-5-mini", "properties": {"raiPolicyName": "rai"}}),
            ),
            (r(ResourceKind::Guardrail, "rai"), json!({"name": "rai"})),
        ]
    }

    fn pos(order: &[ResourceRef], kind: ResourceKind, name: &str) -> usize {
        order
            .iter()
            .position(|x| x.kind == kind && x.name == name)
            .unwrap_or_else(|| panic!("{name} not in order"))
    }

    #[test]
    fn push_order_respects_all_edges() {
        let items = full_chain();
        let order = push_order(&items).unwrap();
        assert_eq!(order.len(), items.len());
        let p = |k, n| pos(&order, k, n);
        assert!(p(ResourceKind::DataSource, "ds") < p(ResourceKind::Indexer, "idxr"));
        assert!(p(ResourceKind::Index, "idx") < p(ResourceKind::Indexer, "idxr"));
        assert!(p(ResourceKind::Skillset, "skills") < p(ResourceKind::Indexer, "idxr"));
        assert!(p(ResourceKind::Index, "idx") < p(ResourceKind::KnowledgeSource, "ks"));
        assert!(p(ResourceKind::KnowledgeSource, "ks") < p(ResourceKind::KnowledgeBase, "kb"));
        assert!(p(ResourceKind::KnowledgeBase, "kb") < p(ResourceKind::Agent, "agent"));
        assert!(p(ResourceKind::Guardrail, "rai") < p(ResourceKind::Deployment, "gpt-5-mini"));
        assert!(p(ResourceKind::Deployment, "gpt-5-mini") < p(ResourceKind::Agent, "agent"));
    }

    #[test]
    fn delete_order_is_reversed() {
        let items = full_chain();
        let push = push_order(&items).unwrap();
        let mut del = delete_order(&items).unwrap();
        del.reverse();
        assert_eq!(push, del);
    }

    #[test]
    fn external_references_ignored() {
        let items = vec![(
            r(ResourceKind::Indexer, "idxr"),
            json!({"name": "idxr", "dataSourceName": "not-in-set", "targetIndexName": "idx"}),
        )];
        let order = push_order(&items).unwrap();
        assert_eq!(order.len(), 1);
    }

    #[test]
    fn deterministic_order_for_independent_nodes() {
        let items = vec![
            (r(ResourceKind::Index, "zebra"), json!({"name": "zebra"})),
            (r(ResourceKind::Index, "apple"), json!({"name": "apple"})),
            (
                r(ResourceKind::DataSource, "mango"),
                json!({"name": "mango"}),
            ),
        ];
        let order = push_order(&items).unwrap();
        // BTree ordering: kind declaration order (DataSource < Index), then name.
        assert_eq!(order[0].name, "mango");
        assert_eq!(order[1].name, "apple");
        assert_eq!(order[2].name, "zebra");
    }

    #[test]
    fn cycle_detected_with_names() {
        // Two aliases pointing at each other is impossible in Azure, but the
        // graph must not hang; simulate a cycle with x-rigg-ref.
        let items = vec![
            (
                r(ResourceKind::Agent, "a"),
                json!({"name": "a", "x-rigg-ref": "agents/b"}),
            ),
            (
                r(ResourceKind::Agent, "b"),
                json!({"name": "b", "x-rigg-ref": "agents/a"}),
            ),
        ];
        let err = push_order(&items).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("agents/a") && msg.contains("agents/b"));
    }
}
