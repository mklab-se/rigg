//! `$file` sidecar handling.
//!
//! Any string field in a resource JSON file may instead be an object
//! `{"$file": "relative/path.md"}`. On push (and whenever Rigg loads the
//! file), the referenced file's content is inlined as the string value. On
//! pull, fields listed in the kind's registry `sidecar_fields` — or fields
//! that already have a sidecar file on disk — are extracted back out to the
//! sidecar and replaced with the `$file` reference.
//!
//! Sidecar paths are relative to the resource JSON file's directory.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use thiserror::Error;

use crate::registry;
use crate::resources::traits::ResourceKind;

pub const FILE_KEY: &str = "$file";

#[derive(Debug, Error)]
pub enum SidecarError {
    #[error("sidecar file not found: {path} (referenced from {referrer})")]
    NotFound { path: PathBuf, referrer: PathBuf },
    #[error("failed to read sidecar {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("failed to write sidecar {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid $file reference in {referrer}: value must be a relative path string")]
    InvalidRef { referrer: PathBuf },
}

/// Replace every `{"$file": "path"}` object with the referenced file content.
/// Returns the list of sidecar paths that were inlined.
pub fn inline_sidecars(json_path: &Path, value: &mut Value) -> Result<Vec<PathBuf>, SidecarError> {
    let base = json_path.parent().unwrap_or(Path::new("."));
    let mut inlined = Vec::new();
    inline_walk(json_path, base, value, &mut inlined)?;
    Ok(inlined)
}

fn inline_walk(
    referrer: &Path,
    base: &Path,
    value: &mut Value,
    inlined: &mut Vec<PathBuf>,
) -> Result<(), SidecarError> {
    match value {
        Value::Object(map) => {
            if let Some(file_ref) = file_ref(map) {
                let rel = file_ref.map_err(|_| SidecarError::InvalidRef {
                    referrer: referrer.to_path_buf(),
                })?;
                let path = base.join(&rel);
                if !path.is_file() {
                    return Err(SidecarError::NotFound {
                        path,
                        referrer: referrer.to_path_buf(),
                    });
                }
                let content =
                    std::fs::read_to_string(&path).map_err(|source| SidecarError::Read {
                        path: path.clone(),
                        source,
                    })?;
                inlined.push(path);
                *value = Value::String(content);
                return Ok(());
            }
            for (_, v) in map.iter_mut() {
                inline_walk(referrer, base, v, inlined)?;
            }
        }
        Value::Array(arr) => {
            for item in arr {
                inline_walk(referrer, base, item, inlined)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// If `map` is exactly a `$file` reference object, return its path.
fn file_ref(map: &Map<String, Value>) -> Option<Result<String, ()>> {
    let v = map.get(FILE_KEY)?;
    if map.len() != 1 {
        return Some(Err(()));
    }
    match v.as_str() {
        Some(s) if !s.is_empty() && !Path::new(s).is_absolute() => Some(Ok(s.to_string())),
        _ => Some(Err(())),
    }
}

/// Extract long-text fields to Markdown sidecars.
///
/// A top-level string field is extracted when:
/// - it is listed in the kind's registry `sidecar_fields`, or
/// - a sidecar file named `<resource-stem>.<field>.md` already exists next to
///   the JSON file (the user opted in by creating it).
///
/// The default sidecar filename is `<resource-stem>.<field>.md` — for agent
/// instructions this yields e.g. `support-agent.instructions.md`.
pub fn extract_sidecars(
    kind: ResourceKind,
    json_path: &Path,
    value: &mut Value,
) -> Result<(), SidecarError> {
    let base = json_path.parent().unwrap_or(Path::new("."));
    let stem = json_path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let default_fields = registry::meta(kind).sidecar_fields;

    let Some(map) = value.as_object_mut() else {
        return Ok(());
    };
    let field_names: Vec<String> = map.keys().cloned().collect();
    for field in field_names {
        let sidecar_name = format!("{stem}.{field}.md");
        let sidecar_path = base.join(&sidecar_name);
        let is_default = default_fields.contains(&field.as_str());
        let exists = sidecar_path.is_file();
        if !is_default && !exists {
            continue;
        }
        let Some(text) = map.get(&field).and_then(Value::as_str) else {
            continue;
        };
        std::fs::write(&sidecar_path, text).map_err(|source| SidecarError::Write {
            path: sidecar_path.clone(),
            source,
        })?;
        let mut ref_obj = Map::new();
        ref_obj.insert(FILE_KEY.to_string(), Value::String(sidecar_name));
        map.insert(field, Value::Object(ref_obj));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inlines_file_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("agent.json");
        std::fs::write(tmp.path().join("agent.instructions.md"), "Be helpful.").unwrap();
        let mut v = json!({
            "name": "agent",
            "instructions": {"$file": "agent.instructions.md"}
        });
        let inlined = inline_sidecars(&json_path, &mut v).unwrap();
        assert_eq!(v["instructions"], json!("Be helpful."));
        assert_eq!(inlined.len(), 1);
    }

    #[test]
    fn inline_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("agent.json");
        let mut v = json!({"instructions": {"$file": "missing.md"}});
        let err = inline_sidecars(&json_path, &mut v).unwrap_err();
        assert!(matches!(err, SidecarError::NotFound { .. }));
    }

    #[test]
    fn inline_rejects_absolute_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("agent.json");
        let mut v = json!({"instructions": {"$file": "/etc/passwd"}});
        let err = inline_sidecars(&json_path, &mut v).unwrap_err();
        assert!(matches!(err, SidecarError::InvalidRef { .. }));
    }

    #[test]
    fn extracts_default_sidecar_field_for_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("support-agent.json");
        let mut v = json!({"name": "support-agent", "instructions": "Long prose here."});
        extract_sidecars(ResourceKind::Agent, &json_path, &mut v).unwrap();
        assert_eq!(
            v["instructions"],
            json!({"$file": "support-agent.instructions.md"})
        );
        let md = std::fs::read_to_string(tmp.path().join("support-agent.instructions.md")).unwrap();
        assert_eq!(md, "Long prose here.");
    }

    #[test]
    fn extracts_opt_in_field_when_sidecar_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("skill.json");
        // user opted in by creating the sidecar file previously
        std::fs::write(tmp.path().join("skill.description.md"), "old").unwrap();
        let mut v = json!({"name": "skill", "description": "new text"});
        extract_sidecars(ResourceKind::Skillset, &json_path, &mut v).unwrap();
        assert_eq!(v["description"], json!({"$file": "skill.description.md"}));
        let md = std::fs::read_to_string(tmp.path().join("skill.description.md")).unwrap();
        assert_eq!(md, "new text");
    }

    #[test]
    fn round_trip_is_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("a.json");
        let mut v = json!({"name": "a", "instructions": "text body"});
        extract_sidecars(ResourceKind::Agent, &json_path, &mut v).unwrap();
        inline_sidecars(&json_path, &mut v).unwrap();
        assert_eq!(v, json!({"name": "a", "instructions": "text body"}));
    }

    #[test]
    fn non_default_field_without_sidecar_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let json_path = tmp.path().join("i.json");
        let mut v = json!({"name": "i", "description": "short"});
        extract_sidecars(ResourceKind::Index, &json_path, &mut v).unwrap();
        assert_eq!(v["description"], json!("short"));
    }
}
