//! Pinned-version schema fixtures (property names per OpenAPI definition):
//! the registry is checked against them, and pull/adopt report fields Azure
//! returns that the pinned version does not know (an API-drift canary).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::{registry, resources::ResourceKind};

pub struct SchemaFixture {
    pub provider: &'static str,
    pub version: &'static str,
    definitions: BTreeMap<String, BTreeSet<String>>,
}

impl SchemaFixture {
    fn parse(provider: &'static str, version: &'static str, text: &str) -> Self {
        let v: Value = serde_json::from_str(text).expect("fixture is valid JSON");
        assert_eq!(
            v["version"].as_str(),
            Some(version),
            "schema fixture for {provider} is stale: regenerate with `rigg dev api-fixture`"
        );
        let definitions = v["definitions"]
            .as_object()
            .expect("definitions")
            .iter()
            .map(|(k, arr)| {
                (
                    k.clone(),
                    arr.as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                )
            })
            .collect();
        Self {
            provider,
            version,
            definitions,
        }
    }

    pub fn definition(&self, name: &str) -> Option<&BTreeSet<String>> {
        self.definitions.get(name)
    }
}

static SEARCH_STABLE: std::sync::OnceLock<SchemaFixture> = std::sync::OnceLock::new();
static SEARCH_PREVIEW: std::sync::OnceLock<SchemaFixture> = std::sync::OnceLock::new();
static COGNITIVE_ARM: std::sync::OnceLock<SchemaFixture> = std::sync::OnceLock::new();

pub fn fixture_for(kind: ResourceKind) -> &'static SchemaFixture {
    match registry::meta(kind).domain {
        registry::Domain::Search => match registry::meta(kind).channel {
            registry::Channel::Stable => SEARCH_STABLE.get_or_init(|| {
                SchemaFixture::parse(
                    "search-data",
                    registry::SEARCH_STABLE_API_VERSION,
                    include_str!("../fixtures/schema/search-data-2026-04-01.json"),
                )
            }),
            registry::Channel::Preview => SEARCH_PREVIEW.get_or_init(|| {
                SchemaFixture::parse(
                    "search-data",
                    registry::SEARCH_PREVIEW_API_VERSION,
                    include_str!("../fixtures/schema/search-data-2026-08-01-preview.json"),
                )
            }),
        },
        _ => COGNITIVE_ARM.get_or_init(|| {
            SchemaFixture::parse(
                "cognitiveservices-arm",
                registry::ARM_COGNITIVE_API_VERSION,
                include_str!("../fixtures/schema/cognitiveservices-arm-2026-07-01.json"),
            )
        }),
    }
}

/// Top-level keys of `doc` that the pinned schema does not declare for
/// `kind`. Empty for kinds without a fixture definition (agents).
///
/// This canary is Search-only for now: the `schema_definition` values kept
/// for the Foundry ARM kinds (`Deployment`, `Connection`, `Guardrail`) name
/// sub-objects of the ARM resource envelope (e.g. `Deployment`'s OpenAPI
/// definition describes the resource body under `properties`, not the
/// envelope itself), and one-level `allOf` resolution never reaches the
/// shared `common-types` definitions that actually contribute `id`, `name`,
/// `type`, and `systemData` (plus `properties` for connections). Comparing
/// those envelopes against the sub-object fixture would always flag them as
/// unknown, so this function is a no-op outside `Domain::Search` — the
/// `schema_definition` values for the Foundry kinds are kept only because
/// `api-diff` still uses them.
pub fn unknown_top_level_fields(kind: ResourceKind, doc: &Value) -> Vec<String> {
    if registry::meta(kind).domain != registry::Domain::Search {
        return Vec::new();
    }
    let name = registry::meta(kind).schema_definition;
    if name.is_empty() {
        return Vec::new();
    }
    let Some(props) = fixture_for(kind).definition(name) else {
        return Vec::new();
    };
    doc.as_object()
        .map(|m| {
            m.keys()
                .filter(|k| {
                    !k.starts_with("x-rigg-") && !k.starts_with("@odata") && !props.contains(*k)
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// `definitions` → property names, resolving `allOf` `$ref`s one level.
///
/// A definition with an OpenAPI `discriminator` (a polymorphic base, e.g.
/// Search's `KnowledgeSource`) also picks up the properties every subtype
/// adds via `allOf: [{$ref: <base>}, {properties: {...}}]` — e.g.
/// `azureBlobParameters`, `searchIndexParameters` — since rigg treats the
/// resource as one document shape across all `kind` variants rather than as
/// separate per-kind types.
pub fn extract_fixture(openapi: &Value) -> BTreeMap<String, BTreeSet<String>> {
    let defs = openapi["definitions"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let props_of = |d: &Value| -> BTreeSet<String> {
        d["properties"]
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    };
    let mut out: BTreeMap<String, BTreeSet<String>> = defs
        .iter()
        .map(|(name, d)| {
            let mut set = props_of(d);
            if let Some(all) = d["allOf"].as_array() {
                for part in all {
                    if let Some(r) = part["$ref"]
                        .as_str()
                        .and_then(|r| r.strip_prefix("#/definitions/"))
                        && let Some(base) = defs.get(r)
                    {
                        set.extend(props_of(base));
                    }
                    set.extend(props_of(part));
                }
            }
            (name.clone(), set)
        })
        .collect();
    for (name, d) in &defs {
        if d["discriminator"].as_str().is_none() {
            continue;
        }
        let mut extra = BTreeSet::new();
        for other in defs.values() {
            let Some(all) = other["allOf"].as_array() else {
                continue;
            };
            let extends_this = all.iter().any(|part| {
                part["$ref"]
                    .as_str()
                    .and_then(|r| r.strip_prefix("#/definitions/"))
                    == Some(name.as_str())
            });
            if extends_this {
                // The subtype's own additions can live inside the `allOf`
                // array (`{$ref}, {properties: {...}}`) or as sibling keys
                // of `allOf` on the subtype definition itself
                // (`{properties: {...}, allOf: [{$ref}]}`) — cover both.
                extra.extend(props_of(other));
                for part in all {
                    extra.extend(props_of(part));
                }
            }
        }
        out.entry(name.clone()).or_default().extend(extra);
    }
    out
}

pub struct DefinitionDiff {
    pub definition: String,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub enum_added: Vec<(String, String)>,
    pub enum_removed: Vec<(String, String)>,
    pub missing_in: Option<&'static str>,
}

pub fn diff_definitions(old: &Value, new: &Value, names: &[&str]) -> Vec<DefinitionDiff> {
    let (fo, fn_) = (extract_fixture(old), extract_fixture(new));
    let enums = |doc: &Value, def: &str| -> BTreeMap<String, BTreeSet<String>> {
        doc["definitions"][def]["properties"]
            .as_object()
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| {
                        v["enum"].as_array().map(|e| {
                            (
                                k.clone(),
                                e.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect(),
                            )
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    names
        .iter()
        .map(|n| {
            let (a, b) = (fo.get(*n), fn_.get(*n));
            let missing_in = match (a, b) {
                (None, _) => Some("old"),
                (_, None) => Some("new"),
                _ => None,
            };
            let (a, b) = (
                a.cloned().unwrap_or_default(),
                b.cloned().unwrap_or_default(),
            );
            let (eo, en) = (enums(old, n), enums(new, n));
            let mut enum_added = Vec::new();
            let mut enum_removed = Vec::new();
            for (k, vals) in &en {
                for v in vals {
                    if !eo.get(k).is_some_and(|s| s.contains(v)) {
                        enum_added.push((k.clone(), v.clone()));
                    }
                }
            }
            for (k, vals) in &eo {
                for v in vals {
                    if !en.get(k).is_some_and(|s| s.contains(v)) {
                        enum_removed.push((k.clone(), v.clone()));
                    }
                }
            }
            DefinitionDiff {
                definition: n.to_string(),
                added: b.difference(&a).cloned().collect(),
                removed: a.difference(&b).cloned().collect(),
                enum_added,
                enum_removed,
                missing_in,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_resolves_allof_one_level() {
        let doc = json!({"definitions": {
            "Base": {"properties": {"name": {}, "description": {}}},
            "Child": {"allOf": [{"$ref": "#/definitions/Base"}], "properties": {"extra": {}}}
        }});
        let f = extract_fixture(&doc);
        assert_eq!(
            f["Child"].iter().cloned().collect::<Vec<_>>(),
            vec!["description", "extra", "name"]
        );
    }

    #[test]
    fn unknown_fields_reports_only_keys_missing_from_the_fixture() {
        let doc =
            json!({"name": "kb", "knowledgeSources": [], "retrievalMode": "x", "@odata.etag": "e"});
        let unknown = unknown_top_level_fields(ResourceKind::KnowledgeBase, &doc);
        assert_eq!(unknown, vec!["retrievalMode"]);
    }

    #[test]
    fn unknown_fields_is_search_only_and_never_flags_foundry_arm_envelope_fields() {
        for kind in [
            ResourceKind::Deployment,
            ResourceKind::Connection,
            ResourceKind::Guardrail,
        ] {
            let doc = crate::scaffold::scaffold(kind, "d", None).unwrap();
            assert_eq!(
                unknown_top_level_fields(kind, &doc),
                Vec::<String>::new(),
                "{kind:?}: canary must be a no-op outside Search"
            );
        }
    }

    #[test]
    fn diff_definitions_lists_added_removed_and_enum_changes() {
        let old = json!({"definitions": {"A": {"properties": {"x": {}, "y": {"type": "string", "enum": ["p"]}}}}});
        let new = json!({"definitions": {"A": {"properties": {"x": {}, "z": {}, "y": {"type": "string", "enum": ["p", "q"]}}}}});
        let d = &diff_definitions(&old, &new, &["A"])[0];
        assert_eq!(d.added, vec!["z"]);
        assert!(d.removed.is_empty());
        assert_eq!(d.enum_added, vec![("y".to_string(), "q".to_string())]);
    }

    /// One-off: regenerates the pinned schema fixtures from OpenAPI documents
    /// on disk. Not run in CI — `RIGG_OPENAPI_DIR=<dir> cargo test -p
    /// rigg-core regenerate_fixtures -- --ignored`.
    #[test]
    #[ignore]
    fn regenerate_fixtures() {
        let dir = std::env::var("RIGG_OPENAPI_DIR").expect("set RIGG_OPENAPI_DIR");
        let dir = std::path::Path::new(&dir);
        let out_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/schema");
        std::fs::create_dir_all(&out_dir).unwrap();
        let sources: &[(&str, &str, &str, &str)] = &[
            (
                "search-2026-04-01.json",
                "search-data",
                registry::SEARCH_STABLE_API_VERSION,
                "search-data-2026-04-01.json",
            ),
            (
                "search-2026-08-01-preview.json",
                "search-data",
                registry::SEARCH_PREVIEW_API_VERSION,
                "search-data-2026-08-01-preview.json",
            ),
            (
                "cs-2026-07-01.json",
                "cognitiveservices-arm",
                registry::ARM_COGNITIVE_API_VERSION,
                "cognitiveservices-arm-2026-07-01.json",
            ),
        ];
        for (input, slug, version, output) in sources {
            let text = std::fs::read_to_string(dir.join(input))
                .unwrap_or_else(|e| panic!("reading {input}: {e}"));
            let doc: Value = serde_json::from_str(&text).unwrap();
            let defs = extract_fixture(&doc);
            let value =
                serde_json::json!({ "provider": slug, "version": version, "definitions": defs });
            let out_file = out_dir.join(output);
            std::fs::write(&out_file, serde_json::to_string_pretty(&value).unwrap()).unwrap();
            println!("wrote {}", out_file.display());
        }
    }
}
