//! Declarative per-kind metadata registry.
//!
//! This is the single place that encodes what Rigg knows *about* each resource
//! kind: where it lives in the API, which api-version channel it needs, which
//! fields are volatile / read-only / secret, how it references other
//! resources, and which values are valid per channel. Resources themselves
//! remain schema-light `serde_json::Value` passthrough documents.
//!
//! When Azure ships a new API version, updating Rigg should mostly mean
//! editing this file (`rigg dev api-check` watches for that).

use serde_json::Value;

use crate::resources::traits::{ResourceKind, ResourceRef};

/// Default data-plane api-versions. Overridable per connection in `rigg.yaml`.
pub const SEARCH_STABLE_API_VERSION: &str = "2026-04-01";
pub const SEARCH_PREVIEW_API_VERSION: &str = "2026-05-01-preview";
pub const FOUNDRY_API_VERSION: &str = "v1";
/// ARM api-version for Microsoft.CognitiveServices (deployments, connections, RAI policies).
pub const ARM_COGNITIVE_API_VERSION: &str = "2026-05-01";

/// Which service/plane a kind is managed through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// Azure AI Search data plane.
    Search,
    /// Microsoft Foundry project data plane (`api-version=v1`).
    FoundryData,
    /// ARM control plane under Microsoft.CognitiveServices.
    FoundryArm,
}

/// API version channel a kind (or capability) requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Preview,
}

/// A reference-bearing field: `path` addresses a string (or array of strings)
/// naming resources of kind `to`.
///
/// Path syntax: dot-separated keys; `[]` after a key descends into each array
/// element. Examples: `"dataSourceName"`, `"knowledgeSources[].name"`,
/// `"indexes[].name"`.
#[derive(Debug, Clone, Copy)]
pub struct RefField {
    pub path: &'static str,
    pub to: ResourceKind,
}

/// Declarative metadata for one resource kind.
#[derive(Debug, Clone, Copy)]
pub struct KindMeta {
    pub kind: ResourceKind,
    pub domain: Domain,
    /// API collection path (exact casing as the REST API expects).
    pub collection_path: &'static str,
    /// Directory name on disk, relative to the project's `search/` or `foundry/` dir.
    pub dir_name: &'static str,
    /// Minimum channel required for the kind itself.
    pub channel: Channel,
    /// Stripped on pull and ignored in diff (dot paths, applied at any depth
    /// for `@odata.*`; top-level otherwise).
    pub volatile_fields: &'static [&'static str],
    /// Returned by GET but rejected by PUT — never written to files.
    pub read_only_fields: &'static [&'static str],
    /// Paths that may carry key material — validation rejects files where
    /// these contain anything but identity-based placeholders.
    pub secret_fields: &'static [&'static str],
    /// Fields the server accepts on PUT but never returns on GET (redacted).
    /// Kept in local files, sent on push, excluded from comparisons.
    pub write_only_fields: &'static [&'static str],
    /// String fields extracted to Markdown sidecars on pull by default.
    pub sidecar_fields: &'static [&'static str],
    /// How this kind references other resources.
    pub reference_fields: &'static [RefField],
}

const COMMON_VOLATILE: &[&str] = &["@odata.etag", "@odata.context", "e_tag", "etag"];

static KINDS: &[KindMeta] = &[
    KindMeta {
        kind: ResourceKind::DataSource,
        domain: Domain::Search,
        collection_path: "datasources",
        dir_name: "data-sources",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &[],
        secret_fields: &["credentials.connectionString"],
        write_only_fields: &["credentials.connectionString"],
        sidecar_fields: &[],
        reference_fields: &[],
    },
    KindMeta {
        kind: ResourceKind::Index,
        domain: Domain::Search,
        collection_path: "indexes",
        dir_name: "indexes",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &[],
        secret_fields: &[
            "encryptionKey.accessCredentials.applicationSecret",
            "vectorSearch.vectorizers[].azureOpenAIParameters.apiKey",
        ],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[],
    },
    KindMeta {
        kind: ResourceKind::Skillset,
        domain: Domain::Search,
        collection_path: "skillsets",
        dir_name: "skillsets",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &[],
        secret_fields: &[
            "cognitiveServices.key",
            "skills[].apiKey",
            "encryptionKey.accessCredentials.applicationSecret",
        ],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[
            // SearchIndexKnowledgeStore / index projections target the index by name.
            RefField {
                path: "knowledgeStore.projections[].objects[].storageContainer",
                to: ResourceKind::Index,
            },
        ],
    },
    KindMeta {
        kind: ResourceKind::Indexer,
        domain: Domain::Search,
        collection_path: "indexers",
        dir_name: "indexers",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &["status", "lastResult", "executionHistory", "limits"],
        secret_fields: &[],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[
            RefField {
                path: "dataSourceName",
                to: ResourceKind::DataSource,
            },
            RefField {
                path: "targetIndexName",
                to: ResourceKind::Index,
            },
            RefField {
                path: "skillsetName",
                to: ResourceKind::Skillset,
            },
        ],
    },
    KindMeta {
        kind: ResourceKind::SynonymMap,
        domain: Domain::Search,
        collection_path: "synonymmaps",
        dir_name: "synonym-maps",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &[],
        secret_fields: &["encryptionKey.accessCredentials.applicationSecret"],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[],
    },
    KindMeta {
        kind: ResourceKind::Alias,
        domain: Domain::Search,
        collection_path: "aliases",
        dir_name: "aliases",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &[],
        secret_fields: &[],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[RefField {
            path: "indexes[]",
            to: ResourceKind::Index,
        }],
    },
    KindMeta {
        kind: ResourceKind::KnowledgeSource,
        domain: Domain::Search,
        collection_path: "knowledgeSources",
        dir_name: "knowledge-sources",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        // Explicit-only model: Rigg never manages Azure-created sub-resources.
        read_only_fields: &["createdResources", "ingestionPermissionOptions"],
        secret_fields: &["searchIndexParameters.apiKey"],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[RefField {
            path: "searchIndexParameters.searchIndexName",
            to: ResourceKind::Index,
        }],
    },
    KindMeta {
        kind: ResourceKind::KnowledgeBase,
        domain: Domain::Search,
        collection_path: "knowledgeBases",
        dir_name: "knowledge-bases",
        channel: Channel::Stable,
        volatile_fields: COMMON_VOLATILE,
        read_only_fields: &[],
        secret_fields: &["models[].apiKey", "models[].azureOpenAIParameters.apiKey"],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[RefField {
            path: "knowledgeSources[].name",
            to: ResourceKind::KnowledgeSource,
        }],
    },
    KindMeta {
        kind: ResourceKind::Agent,
        domain: Domain::FoundryData,
        collection_path: "agents",
        dir_name: "agents",
        channel: Channel::Stable,
        volatile_fields: &[
            "@odata.etag",
            "@odata.context",
            "id",
            "object",
            "created_at",
            "updated_at",
            "version",
            "metadata.modified_at",
        ],
        read_only_fields: &[],
        secret_fields: &[],
        write_only_fields: &[],
        sidecar_fields: &["instructions"],
        reference_fields: &[
            RefField {
                path: "model",
                to: ResourceKind::Deployment,
            },
            RefField {
                path: "tools[].project_connection_id",
                to: ResourceKind::Connection,
            },
        ],
    },
    KindMeta {
        kind: ResourceKind::Deployment,
        domain: Domain::FoundryArm,
        collection_path: "deployments",
        dir_name: "deployments",
        channel: Channel::Stable,
        volatile_fields: &[
            "id",
            "type",
            "systemData",
            "etag",
            "properties.provisioningState",
            "properties.capabilities",
            "properties.rateLimits",
            "properties.model.callRateLimit",
            "properties.currentCapacity",
            "properties.deploymentState",
        ],
        read_only_fields: &[],
        secret_fields: &[],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[RefField {
            path: "properties.raiPolicyName",
            to: ResourceKind::Guardrail,
        }],
    },
    KindMeta {
        kind: ResourceKind::Connection,
        domain: Domain::FoundryArm,
        collection_path: "connections",
        dir_name: "connections",
        channel: Channel::Stable,
        volatile_fields: &[
            "id",
            "type",
            "systemData",
            "etag",
            "properties.provisioningState",
        ],
        read_only_fields: &[],
        // Identity-based auth only — any credential payload is rejected.
        secret_fields: &[
            "properties.credentials.key",
            "properties.credentials.keys",
            "properties.credentials.secret",
            "properties.credentials.clientSecret",
            "properties.credentials.pat",
            "properties.credentials.sas",
        ],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[],
    },
    KindMeta {
        kind: ResourceKind::Guardrail,
        domain: Domain::FoundryArm,
        collection_path: "raiPolicies",
        dir_name: "guardrails",
        channel: Channel::Stable,
        volatile_fields: &["id", "type", "systemData", "etag"],
        read_only_fields: &[],
        secret_fields: &[],
        write_only_fields: &[],
        sidecar_fields: &[],
        reference_fields: &[],
    },
];

/// All kinds, in push-friendly declaration order.
pub fn all_kinds() -> &'static [ResourceKind] {
    static ORDER: &[ResourceKind] = &[
        ResourceKind::DataSource,
        ResourceKind::Index,
        ResourceKind::Skillset,
        ResourceKind::Indexer,
        ResourceKind::SynonymMap,
        ResourceKind::Alias,
        ResourceKind::KnowledgeSource,
        ResourceKind::KnowledgeBase,
        ResourceKind::Agent,
        ResourceKind::Deployment,
        ResourceKind::Connection,
        ResourceKind::Guardrail,
    ];
    ORDER
}

/// Metadata for a kind. Total over all kinds.
pub fn meta(kind: ResourceKind) -> &'static KindMeta {
    KINDS
        .iter()
        .find(|m| m.kind == kind)
        .expect("registry entry exists for every ResourceKind")
}

/// Valid `type` strings for Azure AI Search data sources per channel.
///
/// Note Azure's own inconsistency: the stable reference spells Azure Files
/// `azurefile`, the preview reference `azurefiles`. Both are accepted (and
/// validation warns to double-check against the pinned api-version).
pub fn valid_datasource_types(channel: Channel) -> &'static [&'static str] {
    const GA: &[&str] = &[
        "azureblob",
        "adlsgen2",
        "azuretable",
        "azuresql",
        "cosmosdb",
        "onelake",
    ];
    const PREVIEW: &[&str] = &[
        "azureblob",
        "adlsgen2",
        "azuretable",
        "azuresql",
        "cosmosdb",
        "onelake",
        "mysql",
        "sharepoint",
        "azurefile",
        "azurefiles",
    ];
    match channel {
        Channel::Stable => GA,
        Channel::Preview => PREVIEW,
    }
}

/// Data source types that are preview-only (or preview-spelled).
pub fn preview_only_datasource_types() -> &'static [&'static str] {
    &["mysql", "sharepoint", "azurefile", "azurefiles"]
}

/// The key used by Rigg-local cross-service references
/// (e.g. an agent tool pointing at a knowledge base by name).
pub const X_RIGG_REF: &str = "x-rigg-ref";
/// The key linking a WebApiSkill to an OpenAPI spec in `apis/`.
pub const X_RIGG_API: &str = "x-rigg-api";

/// Extract all references from `body` per the kind's `reference_fields`,
/// plus any `x-rigg-ref` values (`"<dir-name>/<name>"`) found at any depth.
pub fn extract_references(kind: ResourceKind, body: &Value) -> Vec<(ResourceKind, String)> {
    let mut out = Vec::new();
    for rf in meta(kind).reference_fields {
        collect_path(body, rf.path, &mut |v| {
            if let Some(s) = v.as_str() {
                if !s.is_empty() {
                    out.push((rf.to, s.to_string()));
                }
            }
        });
    }
    collect_x_rigg_refs(body, &mut out);
    if kind == ResourceKind::Agent {
        collect_portal_agent_refs(body, &mut out);
    }
    out.sort();
    out.dedup();
    out
}

/// Portal-authored agents reference Search knowledge bases by raw MCP URL
/// (`https://<svc>.search.windows.net/knowledgebases/<name>/mcp?...`) rather
/// than an `x-rigg-ref` annotation. Recognize the shape so dependency
/// expansion can cross the service boundary.
fn collect_portal_agent_refs(v: &Value, out: &mut Vec<(ResourceKind, String)>) {
    match v {
        Value::Object(map) => {
            if let Some(url) = map.get("server_url").and_then(Value::as_str) {
                if let Some(kb) = parse_kb_mcp_url(url) {
                    out.push((ResourceKind::KnowledgeBase, kb));
                }
            }
            for val in map.values() {
                collect_portal_agent_refs(val, out);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_portal_agent_refs(item, out);
            }
        }
        _ => {}
    }
}

/// `https://<host>.search.windows.net/knowledgebases/<name>/mcp[?...]` → name.
fn parse_kb_mcp_url(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    if !host.to_ascii_lowercase().ends_with(".search.windows.net") {
        return None;
    }
    let path = path.split('?').next().unwrap_or(path);
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    let (a, name, c) = (segs.next()?, segs.next()?, segs.next()?);
    (a.eq_ignore_ascii_case("knowledgebases") && c.eq_ignore_ascii_case("mcp"))
        .then(|| name.to_string())
}

/// Platform-provided resource instances (e.g. Microsoft's built-in guardrail
/// policies) cannot be modified or deleted by the user. They are excluded
/// from adoption and from "unmanaged" reporting: local files should only
/// track configuration the user actually controls. References to them (e.g.
/// a deployment's `raiPolicyName`) live in the referencing resource's file.
pub fn is_platform_managed(kind: ResourceKind, body: &Value) -> bool {
    match kind {
        ResourceKind::Guardrail => {
            let system = body
                .pointer("/properties/type")
                .and_then(Value::as_str)
                .map(|t| t.eq_ignore_ascii_case("SystemManaged"))
                .unwrap_or(false);
            // Name-prefix fallback for docs that omit properties.type.
            let name = body.get("name").and_then(Value::as_str).unwrap_or("");
            system || name.starts_with("Microsoft.")
        }
        _ => false,
    }
}

/// Managed-ingestion knowledge sources auto-create their backing pipeline
/// (index, indexer, data source, skillset); Azure names them in the KS's
/// `createdResources`. Rigg never manages these sub-resources — the knowledge
/// source definition is their source of truth — so they are excluded from
/// adoption and unmanaged reporting. Returns resource key → creating KS name.
pub fn auto_created_by(
    snapshot: &[(ResourceRef, Value)],
) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    for (r, doc) in snapshot {
        if r.kind != ResourceKind::KnowledgeSource {
            continue;
        }
        collect_created_resources(doc, &r.name, &mut out);
    }
    out
}

fn collect_created_resources(
    v: &Value,
    ks_name: &str,
    out: &mut std::collections::BTreeMap<String, String>,
) {
    if let Value::Object(map) = v {
        if let Some(Value::Object(created)) = map.get("createdResources") {
            for (member, name) in created {
                let kind = match member.as_str() {
                    "datasource" => Some(ResourceKind::DataSource),
                    "indexer" => Some(ResourceKind::Indexer),
                    "skillset" => Some(ResourceKind::Skillset),
                    "index" => Some(ResourceKind::Index),
                    _ => None, // future member names: ignore
                };
                if let (Some(kind), Some(name)) = (kind, name.as_str()) {
                    out.insert(
                        ResourceRef::new(kind, name.to_string()).key(),
                        ks_name.to_string(),
                    );
                }
            }
        }
        for val in map.values() {
            collect_created_resources(val, ks_name, out);
        }
    } else if let Value::Array(arr) = v {
        for item in arr {
            collect_created_resources(item, ks_name, out);
        }
    }
}

fn collect_x_rigg_refs(v: &Value, out: &mut Vec<(ResourceKind, String)>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == X_RIGG_REF {
                    if let Some(s) = val.as_str() {
                        if let Some((dir, name)) = s.split_once('/') {
                            if let Some(kind) = ResourceKind::from_directory_name(dir) {
                                out.push((kind, name.to_string()));
                            }
                        }
                    }
                } else {
                    collect_x_rigg_refs(val, out);
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                collect_x_rigg_refs(item, out);
            }
        }
        _ => {}
    }
}

/// Walk a registry path (`a.b`, `arr[].field`, `arr[]`) and invoke `f` on each
/// matched terminal value.
pub fn collect_path(v: &Value, path: &str, f: &mut dyn FnMut(&Value)) {
    fn walk(v: &Value, segments: &[&str], f: &mut dyn FnMut(&Value)) {
        let Some((head, rest)) = segments.split_first() else {
            f(v);
            return;
        };
        if let Some(key) = head.strip_suffix("[]") {
            let target = if key.is_empty() { Some(v) } else { v.get(key) };
            if let Some(Value::Array(arr)) = target {
                for item in arr {
                    walk(item, rest, f);
                }
            }
        } else if let Some(next) = v.get(*head) {
            walk(next, rest, f);
        }
    }
    let segments: Vec<&str> = path.split('.').collect();
    walk(v, &segments, f);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn meta_is_total_and_consistent() {
        for kind in all_kinds() {
            let m = meta(*kind);
            assert_eq!(m.kind, *kind);
            assert!(!m.collection_path.is_empty());
            assert!(!m.dir_name.is_empty());
        }
        assert_eq!(all_kinds().len(), 12);
    }

    #[test]
    fn dir_names_unique() {
        let mut dirs: Vec<_> = all_kinds().iter().map(|k| meta(*k).dir_name).collect();
        dirs.sort();
        dirs.dedup();
        assert_eq!(dirs.len(), 12);
    }

    #[test]
    fn indexer_references() {
        let indexer = json!({
            "name": "idxr",
            "dataSourceName": "my-ds",
            "targetIndexName": "my-index",
            "skillsetName": "my-skills"
        });
        let refs = extract_references(ResourceKind::Indexer, &indexer);
        assert!(refs.contains(&(ResourceKind::DataSource, "my-ds".into())));
        assert!(refs.contains(&(ResourceKind::Index, "my-index".into())));
        assert!(refs.contains(&(ResourceKind::Skillset, "my-skills".into())));
    }

    #[test]
    fn knowledge_base_and_alias_references() {
        let kb = json!({
            "name": "kb",
            "knowledgeSources": [{"name": "ks-a"}, {"name": "ks-b"}]
        });
        let refs = extract_references(ResourceKind::KnowledgeBase, &kb);
        assert_eq!(
            refs,
            vec![
                (ResourceKind::KnowledgeSource, "ks-a".to_string()),
                (ResourceKind::KnowledgeSource, "ks-b".to_string()),
            ]
        );

        let alias = json!({"name": "a", "indexes": ["i1"]});
        let refs = extract_references(ResourceKind::Alias, &alias);
        assert_eq!(refs, vec![(ResourceKind::Index, "i1".to_string())]);
    }

    #[test]
    fn x_rigg_ref_extracted_at_depth() {
        let agent = json!({
            "name": "agent",
            "model": "gpt-5-mini",
            "tools": [
                {"type": "mcp", "x-rigg-ref": "knowledge-bases/support-kb", "server_url": ""}
            ]
        });
        let refs = extract_references(ResourceKind::Agent, &agent);
        assert!(refs.contains(&(ResourceKind::KnowledgeBase, "support-kb".into())));
        assert!(refs.contains(&(ResourceKind::Deployment, "gpt-5-mini".into())));
    }

    #[test]
    fn agent_extracts_portal_kb_url_and_connection_id() {
        let agent = serde_json::json!({
            "name": "Regulus",
            "model": "gpt-5.2-chat",
            "tools": [{
                "type": "mcp",
                "server_label": "kb_regulatory_kb",
                "server_url": "https://mklabsrch.search.windows.net/knowledgebases/regulatory-kb/mcp?api-version=2025-11-01-Preview",
                "project_connection_id": "kb-regulatory-kb-9kdyn"
            }]
        });
        let refs = extract_references(ResourceKind::Agent, &agent);
        assert!(
            refs.contains(&(ResourceKind::KnowledgeBase, "regulatory-kb".to_string())),
            "{refs:?}"
        );
        assert!(
            refs.contains(&(
                ResourceKind::Connection,
                "kb-regulatory-kb-9kdyn".to_string()
            )),
            "{refs:?}"
        );
        assert!(
            refs.contains(&(ResourceKind::Deployment, "gpt-5.2-chat".to_string())),
            "{refs:?}"
        );
    }

    #[test]
    fn agent_ignores_non_search_mcp_urls() {
        let agent = serde_json::json!({
            "name": "a",
            "tools": [{"type": "mcp", "server_url": "https://example.com/knowledgebases/x/mcp"}]
        });
        let refs = extract_references(ResourceKind::Agent, &agent);
        assert!(
            !refs.iter().any(|(k, _)| *k == ResourceKind::KnowledgeBase),
            "{refs:?}"
        );
    }

    #[test]
    fn deployment_runtime_state_is_volatile() {
        let vf = meta(ResourceKind::Deployment).volatile_fields;
        assert!(vf.contains(&"properties.currentCapacity"));
        assert!(vf.contains(&"properties.deploymentState"));
    }

    #[test]
    fn agent_portal_timestamp_is_volatile() {
        assert!(
            meta(ResourceKind::Agent)
                .volatile_fields
                .contains(&"metadata.modified_at")
        );
    }

    #[test]
    fn is_platform_managed_true_for_system_managed_guardrail() {
        let doc = json!({"name": "Microsoft.DefaultV2", "properties": {"type": "SystemManaged"}});
        assert!(is_platform_managed(ResourceKind::Guardrail, &doc));
    }

    #[test]
    fn is_platform_managed_false_for_user_managed_guardrail() {
        let doc = json!({"name": "my-policy", "properties": {"type": "UserManaged"}});
        assert!(!is_platform_managed(ResourceKind::Guardrail, &doc));
    }

    #[test]
    fn is_platform_managed_falls_back_to_name_prefix_without_properties() {
        let doc = json!({"name": "Microsoft.Default"});
        assert!(is_platform_managed(ResourceKind::Guardrail, &doc));
    }

    #[test]
    fn is_platform_managed_false_for_user_named_guardrail_without_properties() {
        let doc = json!({"name": "my-policy"});
        assert!(!is_platform_managed(ResourceKind::Guardrail, &doc));
    }

    #[test]
    fn is_platform_managed_only_applies_to_guardrail_kind() {
        let doc = json!({"name": "Microsoft.whatever"});
        assert!(!is_platform_managed(ResourceKind::Index, &doc));
    }

    #[test]
    fn auto_created_by_finds_nested_created_resources() {
        // Live shape: createdResources nests under azureBlobParameters.
        let ks = serde_json::json!({
            "name": "regulatory",
            "kind": "azureBlob",
            "azureBlobParameters": {
                "containerName": "regulatory",
                "createdResources": {
                    "datasource": "regulatory-datasource",
                    "indexer": "regulatory-indexer",
                    "skillset": "regulatory-skillset",
                    "index": "regulatory-index",
                    "somethingFuture": "ignored-name"
                }
            }
        });
        let index_doc = serde_json::json!({"name": "regulatory-index"});
        let snapshot = vec![
            (
                ResourceRef::new(ResourceKind::KnowledgeSource, "regulatory".to_string()),
                ks,
            ),
            (
                ResourceRef::new(ResourceKind::Index, "regulatory-index".to_string()),
                index_doc,
            ),
        ];
        let map = auto_created_by(&snapshot);
        assert_eq!(
            map.get("indexes/regulatory-index").map(String::as_str),
            Some("regulatory")
        );
        assert_eq!(
            map.get("indexers/regulatory-indexer").map(String::as_str),
            Some("regulatory")
        );
        assert_eq!(
            map.get("data-sources/regulatory-datasource")
                .map(String::as_str),
            Some("regulatory")
        );
        assert_eq!(
            map.get("skillsets/regulatory-skillset").map(String::as_str),
            Some("regulatory")
        );
        assert!(
            !map.values().any(|v| v == "ignored-name"),
            "unknown member names ignored: {map:?}"
        );
        assert_eq!(map.len(), 4);
    }

    #[test]
    fn auto_created_by_ignores_non_knowledge_source_docs() {
        let idx = serde_json::json!({
            "name": "i",
            "createdResources": {"index": "x"}
        });
        let snapshot = vec![(ResourceRef::new(ResourceKind::Index, "i".to_string()), idx)];
        assert!(auto_created_by(&snapshot).is_empty());
    }

    #[test]
    fn datasource_types_per_channel() {
        assert!(valid_datasource_types(Channel::Stable).contains(&"cosmosdb"));
        assert!(valid_datasource_types(Channel::Stable).contains(&"onelake"));
        assert!(!valid_datasource_types(Channel::Stable).contains(&"sharepoint"));
        assert!(valid_datasource_types(Channel::Preview).contains(&"sharepoint"));
        // Azure's own spelling inconsistency: both accepted in preview.
        assert!(valid_datasource_types(Channel::Preview).contains(&"azurefile"));
        assert!(valid_datasource_types(Channel::Preview).contains(&"azurefiles"));
    }

    #[test]
    fn ks_points_at_index() {
        let ks = json!({
            "name": "ks",
            "kind": "searchIndex",
            "searchIndexParameters": {"searchIndexName": "docs"}
        });
        let refs = extract_references(ResourceKind::KnowledgeSource, &ks);
        assert_eq!(refs, vec![(ResourceKind::Index, "docs".to_string())]);
    }
}
