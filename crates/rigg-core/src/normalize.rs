//! JSON normalization for consistent Git diffs

use serde_json::{Map, Value};

use crate::resources::traits::ResourceKind;

/// Normalize a JSON value for consistent Git diffs
///
/// This performs:
/// 1. Strips volatile fields (@odata.etag, @odata.context, credentials, etc.)
/// 2. Preserves the property order from the Azure API response
/// 3. Preserves array element order as returned by the API
pub fn normalize(value: &Value, volatile_fields: &[&str]) -> Value {
    normalize_value(value, volatile_fields)
}

fn normalize_value(value: &Value, volatile_fields: &[&str]) -> Value {
    match value {
        Value::Object(map) => {
            // Preserve original key order, just filter out volatile fields
            let filtered: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| !volatile_fields.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), normalize_value(v, volatile_fields)))
                .collect();

            Value::Object(filtered)
        }
        Value::Array(arr) => {
            let normalized: Vec<Value> = arr
                .iter()
                .map(|v| normalize_value(v, volatile_fields))
                .collect();

            Value::Array(normalized)
        }
        _ => value.clone(),
    }
}

/// Normalize a resource for storage on disk: strips the kind's volatile and
/// read-only fields (registry-driven) while preserving Azure's property order.
/// Rigg-local `x-rigg-*` annotations are kept.
pub fn normalize_for_disk(kind: ResourceKind, value: &Value) -> Value {
    let meta = crate::registry::meta(kind);
    let mut out = value.clone();
    for field in meta.volatile_fields.iter().chain(meta.read_only_fields) {
        strip_field(&mut out, field);
    }
    out
}

/// Normalize a resource for pushing to Azure: everything `normalize_for_disk`
/// strips, plus all `x-rigg-*` annotation keys at any depth.
pub fn normalize_for_push(kind: ResourceKind, value: &Value) -> Value {
    let mut out = normalize_for_disk(kind, value);
    strip_x_rigg_keys(&mut out);
    out
}

/// Normalize for comparison: like `normalize_for_push`, and additionally
/// strips write-only fields (which the server never echoes back).
pub fn normalize_for_compare(kind: ResourceKind, value: &Value) -> Value {
    let mut out = normalize_for_push(kind, value);
    for field in crate::registry::meta(kind).write_only_fields {
        strip_field(&mut out, field);
    }
    out
}

/// Are two documents semantically equal for this kind (after normalization)?
pub fn semantic_eq(kind: ResourceKind, a: &Value, b: &Value) -> bool {
    let na = normalize_for_compare(kind, a);
    let nb = normalize_for_compare(kind, b);
    rigg_diff::semantic::diff(&na, &nb, "name").is_equal
}

/// Strip one registry field spec from a document.
///
/// - Specs containing `.` or `[]` are paths from the root (e.g.
///   `properties.provisioningState`, `models[].apiKey`).
/// - Bare names are removed at any depth (e.g. `@odata.etag` — note the
///   leading `@` key itself contains dots but is matched as a literal key).
fn strip_field(value: &mut Value, spec: &str) {
    let is_literal_key = spec.starts_with('@') || (!spec.contains('.') && !spec.contains("[]"));
    if is_literal_key {
        remove_key_recursive(value, spec);
    } else {
        remove_path(value, &spec.split('.').collect::<Vec<_>>());
    }
}

fn remove_key_recursive(value: &mut Value, key: &str) {
    match value {
        Value::Object(map) => {
            map.remove(key);
            for (_, v) in map.iter_mut() {
                remove_key_recursive(v, key);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                remove_key_recursive(item, key);
            }
        }
        _ => {}
    }
}

fn remove_path(value: &mut Value, segments: &[&str]) {
    let Some((head, rest)) = segments.split_first() else {
        return;
    };
    if let Some(key) = head.strip_suffix("[]") {
        let target = if key.is_empty() {
            Some(value)
        } else {
            value.get_mut(key)
        };
        if let Some(Value::Array(arr)) = target {
            for item in arr {
                if rest.is_empty() {
                    continue; // removing whole array elements is not a thing
                }
                remove_path(item, rest);
            }
        }
    } else if rest.is_empty() {
        if let Value::Object(map) = value {
            map.remove(*head);
        }
    } else if let Some(next) = value.get_mut(*head) {
        remove_path(next, rest);
    }
}

/// Remove every `x-rigg-*` key at any depth (Rigg-local annotations).
pub fn strip_x_rigg_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|k, _| !k.starts_with("x-rigg-"));
            for (_, v) in map.iter_mut() {
                strip_x_rigg_keys(v);
            }
        }
        Value::Array(arr) => {
            for item in arr {
                strip_x_rigg_keys(item);
            }
        }
        _ => {}
    }
}

/// Format JSON with consistent formatting (2-space indent, trailing newline, sorted keys)
pub fn format_json(value: &Value) -> String {
    let mut output = serde_json::to_string_pretty(value).unwrap_or_default();
    if !output.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Strip sensitive fields from credentials objects
pub fn redact_credentials(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        // Redact connection strings
        if let Some(creds) = obj.get_mut("credentials") {
            if let Some(creds_obj) = creds.as_object_mut() {
                if creds_obj.contains_key("connectionString") {
                    creds_obj.insert(
                        "connectionString".to_string(),
                        Value::String("<REDACTED>".to_string()),
                    );
                }
            }
        }

        // Redact storage connection strings
        if obj.contains_key("storageConnectionStringSecret") {
            obj.insert(
                "storageConnectionStringSecret".to_string(),
                Value::String("<REDACTED>".to_string()),
            );
        }

        // Recursively process nested objects
        for (_, v) in obj.iter_mut() {
            redact_credentials(v);
        }
    } else if let Some(arr) = value.as_array_mut() {
        for item in arr {
            redact_credentials(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn disk_normalization_strips_volatile_and_read_only() {
        let indexer = json!({
            "@odata.etag": "0x123",
            "name": "idxr",
            "dataSourceName": "ds",
            "status": "running",
            "lastResult": {"status": "success"},
            "nested": {"@odata.etag": "0x456", "keep": true}
        });
        let out = normalize_for_disk(ResourceKind::Indexer, &indexer);
        assert!(out.get("@odata.etag").is_none());
        assert!(out.get("status").is_none(), "read-only stripped");
        assert!(out.get("lastResult").is_none());
        assert!(
            out["nested"].get("@odata.etag").is_none(),
            "etag stripped at depth"
        );
        assert_eq!(out["nested"]["keep"], json!(true));
        assert_eq!(out["dataSourceName"], json!("ds"));
    }

    #[test]
    fn dotted_path_stripping_for_arm_kinds() {
        let dep = json!({
            "name": "gpt-5-mini",
            "properties": {
                "model": {"name": "gpt-5-mini", "callRateLimit": {"count": 1}},
                "provisioningState": "Succeeded",
                "raiPolicyName": "default"
            },
            "systemData": {"createdAt": "2026-01-01"}
        });
        let out = normalize_for_disk(ResourceKind::Deployment, &dep);
        assert!(out.get("systemData").is_none());
        assert!(out["properties"].get("provisioningState").is_none());
        assert!(out["properties"]["model"].get("callRateLimit").is_none());
        assert_eq!(out["properties"]["raiPolicyName"], json!("default"));
    }

    #[test]
    fn push_normalization_strips_x_rigg_but_disk_keeps() {
        let agent = json!({
            "name": "a",
            "tools": [{"type": "mcp", "x-rigg-ref": "knowledge-bases/kb", "server_url": ""}]
        });
        let disk = normalize_for_disk(ResourceKind::Agent, &agent);
        assert_eq!(disk["tools"][0]["x-rigg-ref"], json!("knowledge-bases/kb"));
        let push = normalize_for_push(ResourceKind::Agent, &agent);
        assert!(push["tools"][0].get("x-rigg-ref").is_none());
        assert_eq!(push["tools"][0]["type"], json!("mcp"));
    }

    #[test]
    fn semantic_eq_ignores_volatile_and_order() {
        let a = json!({"name": "i", "@odata.etag": "1", "fields": [{"name": "f1"}]});
        let b = json!({"@odata.etag": "2", "fields": [{"name": "f1"}], "name": "i"});
        assert!(semantic_eq(ResourceKind::Index, &a, &b));
        let c = json!({"name": "i", "fields": [{"name": "f2"}]});
        assert!(!semantic_eq(ResourceKind::Index, &a, &c));
    }

    #[test]
    fn test_strips_volatile_fields() {
        let input = json!({
            "@odata.etag": "abc123",
            "@odata.context": "https://...",
            "name": "test",
            "fields": []
        });

        let result = normalize(&input, &["@odata.etag", "@odata.context"]);

        assert!(result.get("@odata.etag").is_none());
        assert!(result.get("@odata.context").is_none());
        assert_eq!(result.get("name"), Some(&json!("test")));
    }

    #[test]
    fn test_preserves_key_order() {
        // Build a map with explicit insertion order
        let mut map = serde_json::Map::new();
        map.insert("zebra".to_string(), json!(1));
        map.insert("apple".to_string(), json!(2));
        map.insert("mango".to_string(), json!(3));
        let input = Value::Object(map);

        let result = normalize(&input, &[]);
        let formatted = serde_json::to_string(&result).unwrap();

        // Keys should preserve insertion order (not alphabetical)
        let zebra_pos = formatted.find("zebra").unwrap();
        let apple_pos = formatted.find("apple").unwrap();
        let mango_pos = formatted.find("mango").unwrap();

        assert!(zebra_pos < apple_pos);
        assert!(apple_pos < mango_pos);
    }

    #[test]
    fn test_preserves_array_order() {
        let input = json!({
            "items": [
                {"name": "charlie", "value": 3},
                {"name": "alice", "value": 1},
                {"name": "bob", "value": 2}
            ]
        });

        let result = normalize(&input, &[]);
        let items = result.get("items").unwrap().as_array().unwrap();

        // Order should be preserved as-is, not sorted
        assert_eq!(items[0].get("name").unwrap(), "charlie");
        assert_eq!(items[1].get("name").unwrap(), "alice");
        assert_eq!(items[2].get("name").unwrap(), "bob");
    }

    #[test]
    fn test_redact_credentials() {
        let mut input = json!({
            "name": "test",
            "credentials": {
                "connectionString": "secret-connection-string"
            }
        });

        redact_credentials(&mut input);

        assert_eq!(input["credentials"]["connectionString"], "<REDACTED>");
    }

    #[test]
    fn test_deeply_nested_volatile_fields() {
        let input = json!({
            "name": "top",
            "@odata.etag": "top-etag",
            "nested": {
                "@odata.etag": "nested-etag",
                "value": 1,
                "deeper": {
                    "@odata.context": "ctx",
                    "keep": true
                }
            }
        });

        let result = normalize(&input, &["@odata.etag", "@odata.context"]);

        assert!(result.get("@odata.etag").is_none());
        let nested = result.get("nested").unwrap();
        assert!(nested.get("@odata.etag").is_none());
        assert_eq!(nested.get("value"), Some(&json!(1)));
        let deeper = nested.get("deeper").unwrap();
        assert!(deeper.get("@odata.context").is_none());
        assert_eq!(deeper.get("keep"), Some(&json!(true)));
    }

    #[test]
    fn test_primitive_array_order_preserved() {
        let input = json!({
            "values": [3, 1, 2]
        });

        let result = normalize(&input, &[]);
        let values = result.get("values").unwrap().as_array().unwrap();

        assert_eq!(values[0], json!(3));
        assert_eq!(values[1], json!(1));
        assert_eq!(values[2], json!(2));
    }

    #[test]
    fn test_empty_object_preserved() {
        let input = json!({});
        let result = normalize(&input, &[]);
        assert_eq!(result, json!({}));
    }

    #[test]
    fn test_empty_array_preserved() {
        let input = json!({
            "items": []
        });

        let result = normalize(&input, &[]);
        let items = result.get("items").unwrap().as_array().unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_redact_nested_credentials() {
        let mut input = json!({
            "name": "test",
            "outer": {
                "credentials": {
                    "connectionString": "nested-secret"
                }
            }
        });

        redact_credentials(&mut input);

        assert_eq!(
            input["outer"]["credentials"]["connectionString"],
            "<REDACTED>"
        );
    }

    #[test]
    fn test_redact_storage_connection_string() {
        let mut input = json!({
            "name": "test",
            "storageConnectionStringSecret": "my-storage-secret"
        });

        redact_credentials(&mut input);

        assert_eq!(input["storageConnectionStringSecret"], "<REDACTED>");
    }

    #[test]
    fn test_redact_multiple_targets() {
        let mut input = json!({
            "name": "test",
            "credentials": {
                "connectionString": "secret-conn"
            },
            "storageConnectionStringSecret": "secret-storage"
        });

        redact_credentials(&mut input);

        assert_eq!(input["credentials"]["connectionString"], "<REDACTED>");
        assert_eq!(input["storageConnectionStringSecret"], "<REDACTED>");
    }

    #[test]
    fn test_redact_credentials_in_array() {
        let mut input = json!({
            "dataSources": [
                {
                    "name": "ds1",
                    "credentials": {
                        "connectionString": "secret1"
                    }
                },
                {
                    "name": "ds2",
                    "credentials": {
                        "connectionString": "secret2"
                    }
                }
            ]
        });

        redact_credentials(&mut input);

        assert_eq!(
            input["dataSources"][0]["credentials"]["connectionString"],
            "<REDACTED>"
        );
        assert_eq!(
            input["dataSources"][1]["credentials"]["connectionString"],
            "<REDACTED>"
        );
    }

    #[test]
    fn test_format_json_trailing_newline() {
        let input = json!({"key": "value"});
        let output = format_json(&input);
        assert!(output.ends_with('\n'));
    }

    #[test]
    fn test_format_json_empty_object() {
        let input = json!({});
        let output = format_json(&input);
        assert_eq!(output, "{}\n");
    }
}
