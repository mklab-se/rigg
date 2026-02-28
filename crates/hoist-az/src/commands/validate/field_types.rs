//! EDM type validation for Azure Search index fields.

/// Valid Azure Search Edm types (base types).
const VALID_EDM_TYPES: &[&str] = &[
    "Edm.String",
    "Edm.Int32",
    "Edm.Int64",
    "Edm.Double",
    "Edm.Boolean",
    "Edm.DateTimeOffset",
    "Edm.GeographyPoint",
    "Edm.ComplexType",
    "Edm.Single",
    "Edm.Half",
    "Edm.SByte",
    "Edm.Byte",
];

/// Check if a type string is a valid Azure Search Edm type.
pub(super) fn is_valid_edm_type(type_str: &str) -> bool {
    if VALID_EDM_TYPES.contains(&type_str) {
        return true;
    }
    // Check Collection(Edm.*) pattern
    if let Some(inner) = type_str
        .strip_prefix("Collection(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return VALID_EDM_TYPES.contains(&inner);
    }
    false
}

/// Validate field types recursively (handles complex types with sub-fields).
pub(super) fn validate_field_types(
    index_name: &str,
    fields: &[serde_json::Value],
    path_prefix: &str,
    errors: &mut Vec<String>,
) {
    for (i, field) in fields.iter().enumerate() {
        let field_name = field
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("(unnamed)");
        let field_path = if path_prefix.is_empty() {
            format!("fields[{}]", i)
        } else {
            format!("{}fields[{}]", path_prefix, i)
        };

        if let Some(type_str) = field.get("type").and_then(|t| t.as_str()) {
            if !is_valid_edm_type(type_str) {
                errors.push(format!(
                    "indexes/{}.json: field '{}' ({}) has invalid type '{}'",
                    index_name, field_name, field_path, type_str
                ));
            }
        }

        // Recurse into sub-fields (for Edm.ComplexType)
        if let Some(sub_fields) = field.get("fields").and_then(|f| f.as_array()) {
            let sub_prefix = format!("{}.{}.fields.", field_path, field_name);
            validate_field_types(index_name, sub_fields, &sub_prefix, errors);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_valid_edm_types() {
        assert!(is_valid_edm_type("Edm.String"));
        assert!(is_valid_edm_type("Edm.Int32"));
        assert!(is_valid_edm_type("Edm.Int64"));
        assert!(is_valid_edm_type("Edm.Double"));
        assert!(is_valid_edm_type("Edm.Boolean"));
        assert!(is_valid_edm_type("Edm.DateTimeOffset"));
        assert!(is_valid_edm_type("Edm.GeographyPoint"));
        assert!(is_valid_edm_type("Edm.ComplexType"));
        assert!(is_valid_edm_type("Edm.Single"));
        assert!(is_valid_edm_type("Edm.Half"));
        assert!(is_valid_edm_type("Edm.SByte"));
        assert!(is_valid_edm_type("Edm.Byte"));
    }

    #[test]
    fn test_valid_collection_types() {
        assert!(is_valid_edm_type("Collection(Edm.String)"));
        assert!(is_valid_edm_type("Collection(Edm.Int32)"));
        assert!(is_valid_edm_type("Collection(Edm.Single)"));
        assert!(is_valid_edm_type("Collection(Edm.ComplexType)"));
    }

    #[test]
    fn test_invalid_edm_types() {
        assert!(!is_valid_edm_type("Edm.Strig")); // typo
        assert!(!is_valid_edm_type("String"));
        assert!(!is_valid_edm_type("int"));
        assert!(!is_valid_edm_type("Collection(Edm.Strig)"));
        assert!(!is_valid_edm_type("Collection(String)"));
        assert!(!is_valid_edm_type(""));
    }

    #[test]
    fn test_validate_field_types_catches_invalid() {
        let fields = vec![
            json!({"name": "id", "type": "Edm.String", "key": true}),
            json!({"name": "bad", "type": "Edm.Strig"}),
        ];
        let mut errors = Vec::new();
        validate_field_types("test-idx", &fields, "", &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("Edm.Strig"));
        assert!(errors[0].contains("bad"));
    }

    #[test]
    fn test_validate_field_types_valid_passes() {
        let fields = vec![
            json!({"name": "id", "type": "Edm.String", "key": true}),
            json!({"name": "vec", "type": "Collection(Edm.Single)"}),
        ];
        let mut errors = Vec::new();
        validate_field_types("test-idx", &fields, "", &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn test_validate_field_types_recurses_into_complex() {
        let fields = vec![json!({
            "name": "address",
            "type": "Edm.ComplexType",
            "fields": [
                {"name": "street", "type": "Edm.String"},
                {"name": "zip", "type": "BadType"}
            ]
        })];
        let mut errors = Vec::new();
        validate_field_types("test-idx", &fields, "", &mut errors);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("BadType"));
        assert!(errors[0].contains("zip"));
    }
}
