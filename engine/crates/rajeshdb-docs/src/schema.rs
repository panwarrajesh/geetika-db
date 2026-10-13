//! Optional per-collection schema:
//!   {"fields": {"name": {"type": "string", "required": true}, "age": {"type": "integer"}},
//!    "additionalProperties": false}
//! Types: string, number, integer, boolean, object, array, null.
//! Only top-level fields are checked. "_id" is always allowed.

use crate::error::{invalid, DocError, Result};
use serde_json::{Map, Value};

const TYPES: [&str; 6] = ["string", "number", "integer", "boolean", "object", "array"];

pub fn validate_schema_def(s: &Value) -> Result<()> {
    let obj = s.as_object().ok_or_else(|| invalid("schema must be a JSON object"))?;
    for (k, v) in obj {
        match k.as_str() {
            "fields" => {
                let fields = v.as_object().ok_or_else(|| invalid("schema.fields must be an object"))?;
                for (name, spec) in fields {
                    let spec = spec
                        .as_object()
                        .ok_or_else(|| invalid(format!("schema field '{}' must be an object", name)))?;
                    for (sk, sv) in spec {
                        match sk.as_str() {
                            "type" => {
                                let t = sv.as_str().unwrap_or("");
                                if !TYPES.contains(&t) && t != "null" {
                                    return Err(invalid(format!("unknown type for field '{}'", name)));
                                }
                            }
                            "required" => {
                                if !sv.is_boolean() {
                                    return Err(invalid("'required' must be true or false"));
                                }
                            }
                            _ => return Err(invalid(format!("unknown schema key '{}'", sk))),
                        }
                    }
                }
            }
            "additionalProperties" => {
                if !v.is_boolean() {
                    return Err(invalid("additionalProperties must be true or false"));
                }
            }
            _ => return Err(invalid(format!("unknown schema key '{}'", k))),
        }
    }
    Ok(())
}

fn type_ok(t: &str, v: &Value) -> bool {
    match t {
        "string" => v.is_string(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64(),
        "boolean" => v.is_boolean(),
        "object" => v.is_object(),
        "array" => v.is_array(),
        "null" => v.is_null(),
        _ => false,
    }
}

pub fn validate_doc(schema: &Value, doc: &Map<String, Value>) -> Result<()> {
    let violation = |m: String| DocError::SchemaViolation(m);
    let empty = Map::new();
    let fields = schema.get("fields").and_then(|f| f.as_object()).unwrap_or(&empty);
    for (name, spec) in fields {
        match doc.get(name) {
            None => {
                if spec.get("required").and_then(|r| r.as_bool()).unwrap_or(false) {
                    return Err(violation(format!("missing required field '{}'", name)));
                }
            }
            Some(v) => {
                if let Some(t) = spec.get("type").and_then(|t| t.as_str()) {
                    if !type_ok(t, v) {
                        return Err(violation(format!("field '{}' must be of type {}", name, t)));
                    }
                }
            }
        }
    }
    let extra_ok = schema.get("additionalProperties").and_then(|a| a.as_bool()).unwrap_or(true);
    if !extra_ok {
        for k in doc.keys() {
            if k != "_id" && !fields.contains_key(k) {
                return Err(violation(format!("unexpected field '{}'", k)));
            }
        }
    }
    Ok(())
}
