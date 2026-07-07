//! Minimal OpenAPI 3.x model for validating custom WebApiSkill contracts
//! (spec §9). Rigg does not validate the full OpenAPI grammar — only what a
//! skillset needs: the paths, and the `values[].data` property names of the
//! request and response envelopes.

use std::path::Path;

use serde_json::Value;

#[derive(Debug, Clone)]
pub struct ApiSpec {
    /// Path templates, e.g. `/api/enrich`.
    pub paths: Vec<String>,
    /// Property names available under request `values[].data`.
    pub request_data_props: Vec<String>,
    /// Property names available under response `values[].data`.
    pub response_data_props: Vec<String>,
    /// True when the data schemas allow additional properties (open contract).
    pub open_props: bool,
}

pub fn load(path: &Path) -> Result<ApiSpec, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read spec: {e}"))?;
    let doc: Value =
        serde_json::from_str(&text).map_err(|e| format!("spec is not valid JSON: {e}"))?;
    parse(&doc)
}

pub fn parse(doc: &Value) -> Result<ApiSpec, String> {
    if doc.get("openapi").and_then(Value::as_str).is_none() {
        return Err("missing `openapi` version field".to_string());
    }
    let Some(paths_obj) = doc.get("paths").and_then(Value::as_object) else {
        return Err("missing `paths`".to_string());
    };
    let paths: Vec<String> = paths_obj.keys().cloned().collect();

    // Find the first POST operation and walk its request/response envelopes.
    let mut request_data_props = Vec::new();
    let mut response_data_props = Vec::new();
    let mut open_props = true;
    for item in paths_obj.values() {
        let Some(post) = item.get("post") else {
            continue;
        };
        let request_schema = post
            .pointer("/requestBody/content/application~1json/schema")
            .map(|s| resolve_ref(doc, s));
        let response_schema = post
            .pointer("/responses/200/content/application~1json/schema")
            .map(|s| resolve_ref(doc, s));
        if let Some(schema) = request_schema {
            let (props, open) = data_props(doc, &schema);
            request_data_props = props;
            open_props = open;
        }
        if let Some(schema) = response_schema {
            let (props, open) = data_props(doc, &schema);
            response_data_props = props;
            open_props = open_props || open;
        }
        break;
    }
    Ok(ApiSpec {
        paths,
        request_data_props,
        response_data_props,
        open_props,
    })
}

/// Resolve a local `$ref` (`#/components/...`), or return the schema as-is.
fn resolve_ref(doc: &Value, schema: &Value) -> Value {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        if let Some(path) = reference.strip_prefix("#/") {
            let pointer = format!("/{}", path);
            if let Some(resolved) = doc.pointer(&pointer) {
                return resolve_ref(doc, resolved);
            }
        }
    }
    schema.clone()
}

/// Extract `values[].data` property names + openness from an envelope schema.
fn data_props(doc: &Value, envelope: &Value) -> (Vec<String>, bool) {
    let data = envelope
        .pointer("/properties/values/items")
        .map(|s| resolve_ref(doc, s))
        .and_then(|items| {
            items
                .pointer("/properties/data")
                .map(|d| resolve_ref(doc, d))
        });
    let Some(data) = data else {
        return (Vec::new(), true);
    };
    let props: Vec<String> = data
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let open = match data.get("additionalProperties") {
        Some(Value::Bool(false)) => false,
        Some(_) | None => props.is_empty() || data.get("additionalProperties").is_some(),
    };
    (props, open)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_scaffold_spec() {
        let spec_doc = crate::scaffold::scaffold_api_spec("enrich");
        let spec = parse(&spec_doc).unwrap();
        assert_eq!(spec.paths, vec!["/api/enrich"]);
        assert!(spec.open_props, "scaffold uses additionalProperties: true");
    }

    #[test]
    fn closed_contract_props_are_extracted() {
        let doc = serde_json::json!({
            "openapi": "3.1.0",
            "info": {"title": "t", "version": "1"},
            "paths": {
                "/api/translate": {
                    "post": {
                        "requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Req"}}}},
                        "responses": {"200": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Res"}}}}}
                    }
                }
            },
            "components": {"schemas": {
                "Req": {"type": "object", "properties": {"values": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"recordId": {"type": "string"}, "data": {
                        "type": "object",
                        "properties": {"text": {"type": "string"}, "language": {"type": "string"}},
                        "additionalProperties": false
                    }}
                }}}},
                "Res": {"type": "object", "properties": {"values": {"type": "array", "items": {
                    "type": "object",
                    "properties": {"recordId": {"type": "string"}, "data": {
                        "type": "object",
                        "properties": {"translation": {"type": "string"}},
                        "additionalProperties": false
                    }}
                }}}}
            }}
        });
        let spec = parse(&doc).unwrap();
        assert_eq!(spec.paths, vec!["/api/translate"]);
        let mut req = spec.request_data_props.clone();
        req.sort();
        assert_eq!(req, vec!["language".to_string(), "text".to_string()]);
        assert!(!spec.open_props);
        assert_eq!(spec.response_data_props, vec!["translation"]);
    }

    #[test]
    fn rejects_non_openapi() {
        assert!(parse(&serde_json::json!({"paths": {}})).is_err());
        assert!(parse(&serde_json::json!({"openapi": "3.1.0"})).is_err());
    }
}
