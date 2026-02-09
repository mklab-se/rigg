//! JSON normalization for consistent Git diffs

use serde_json::{Map, Value};

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
