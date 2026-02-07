//! JSON normalization for consistent Git diffs

use serde_json::{Map, Value};

/// Normalize a JSON value for consistent Git diffs
///
/// This performs:
/// 1. Strips volatile fields (@odata.etag, @odata.context, credentials, etc.)
/// 2. Preserves the property order from the Azure API response
/// 3. Sorts arrays by identity key (usually "name") for stable diffs
pub fn normalize(value: &Value, volatile_fields: &[&str], identity_key: &str) -> Value {
    normalize_value(value, volatile_fields, identity_key)
}

fn normalize_value(value: &Value, volatile_fields: &[&str], identity_key: &str) -> Value {
    match value {
        Value::Object(map) => {
            // Preserve original key order, just filter out volatile fields
            let filtered: Map<String, Value> = map
                .iter()
                .filter(|(k, _)| !volatile_fields.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), normalize_value(v, volatile_fields, identity_key)))
                .collect();

            Value::Object(filtered)
        }
        Value::Array(arr) => {
            let mut normalized: Vec<Value> = arr
                .iter()
                .map(|v| normalize_value(v, volatile_fields, identity_key))
                .collect();

            // Sort arrays by identity key if present (for stable diffs)
            if !normalized.is_empty() && normalized[0].get(identity_key).is_some() {
                normalized.sort_by(|a, b| {
                    let a_key = a.get(identity_key).and_then(|v| v.as_str()).unwrap_or("");
                    let b_key = b.get(identity_key).and_then(|v| v.as_str()).unwrap_or("");
                    a_key.cmp(b_key)
                });
            }

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

        let result = normalize(&input, &["@odata.etag", "@odata.context"], "name");

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

        let result = normalize(&input, &[], "name");
        let formatted = serde_json::to_string(&result).unwrap();

        // Keys should preserve insertion order (not alphabetical)
        let zebra_pos = formatted.find("zebra").unwrap();
        let apple_pos = formatted.find("apple").unwrap();
        let mango_pos = formatted.find("mango").unwrap();

        assert!(zebra_pos < apple_pos);
        assert!(apple_pos < mango_pos);
    }

    #[test]
    fn test_sorts_arrays_by_identity_key() {
        let input = json!({
            "items": [
                {"name": "charlie", "value": 3},
                {"name": "alice", "value": 1},
                {"name": "bob", "value": 2}
            ]
        });

        let result = normalize(&input, &[], "name");
        let items = result.get("items").unwrap().as_array().unwrap();

        assert_eq!(items[0].get("name").unwrap(), "alice");
        assert_eq!(items[1].get("name").unwrap(), "bob");
        assert_eq!(items[2].get("name").unwrap(), "charlie");
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
}
