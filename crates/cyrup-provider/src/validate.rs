//! Tool-argument validation + coercion (func-01 R-01-034; arch-01 §3.9).
//!
//! A hand-rolled, schema-driven coercion pass using `serde_json` only (no external validator):
//! it validates model-emitted tool arguments against a tool's JSON-Schema `parameters` and
//! **coerces** compatible mismatches — `"123"` → `123`, `"true"` → `true`, recursing into objects
//! and arrays element-wise, and trying each branch of an `anyOf`/`oneOf` union (taking the first
//! that fits). Required fields and types are enforced; on an unrecoverable mismatch a typed
//! [`ToolValidationError`] describing the failure is returned.
//!
//! Union disambiguation runs in two passes: first a **strict** pass that accepts a branch only when
//! the value already has the exact JSON type the branch wants (so a value that is already the right
//! shape is never lossily re-coerced), then a **lenient** pass that allows scalar coercion. This
//! keeps e.g. an object from being stringified just because a `{type:"string"}` branch appears
//! first in the union.
//!
//! Authoring guidance (R-01-035): prefer a `StringEnum` (`{"type":"string","enum":[…]}`) over a
//! tagged enum for Google compatibility.

use serde_json::{Map, Number, Value};

/// Tool-argument validation failure (arch-01 §8).
#[derive(Debug, thiserror::Error)]
pub enum ToolValidationError {
    /// No tool with the requested name (used by the by-name convenience).
    #[error("tool not found: {0}")]
    NotFound(String),
    /// Arguments could not be validated/coerced against the schema.
    #[error("schema validation failed at `{path}`: {detail}")]
    Schema { path: String, detail: String },
}

impl ToolValidationError {
    fn schema(path: &str, detail: impl Into<String>) -> Self {
        Self::Schema { path: path.to_string(), detail: detail.into() }
    }
}

/// Validate AND coerce `arguments` against a tool's JSON-Schema `parameters` (R-01-034).
///
/// On success returns the coerced arguments; on an unrecoverable mismatch returns a
/// [`ToolValidationError::Schema`] describing where it failed. `serde_json`-only; no panics.
pub fn validate_tool_call(schema: &Value, arguments: Value) -> Result<Value, ToolValidationError> {
    coerce(schema, arguments, "$", false)
}

/// By-name convenience matching the arch-01 §3.9 contract: locate the tool's schema in a
/// `(name, parameters)` table, then validate + coerce (R-01-034).
pub fn validate_named_tool_call<'a, I>(
    tools: I,
    name: &str,
    arguments: Value,
) -> Result<Value, ToolValidationError>
where
    I: IntoIterator<Item = (&'a str, &'a Value)>,
{
    match tools.into_iter().find(|(n, _)| *n == name) {
        Some((_, schema)) => validate_tool_call(schema, arguments),
        None => Err(ToolValidationError::NotFound(name.to_string())),
    }
}

/// Coerce `value` to satisfy `schema`. When `strict`, only accept values that already have the
/// exact JSON type the schema wants (no scalar cross-type coercion); used for the first pass of
/// union disambiguation.
fn coerce(
    schema: &Value,
    value: Value,
    path: &str,
    strict: bool,
) -> Result<Value, ToolValidationError> {
    match schema {
        // Boolean schemas: `true` accepts anything, `false` rejects everything.
        Value::Bool(true) => Ok(value),
        Value::Bool(false) => Err(ToolValidationError::schema(path, "schema `false` rejects all values")),
        Value::Object(map) => coerce_object_schema(map, value, path, strict),
        // A non-schema node is treated permissively (nothing to enforce).
        _ => Ok(value),
    }
}

fn coerce_object_schema(
    schema: &Map<String, Value>,
    value: Value,
    path: &str,
    strict: bool,
) -> Result<Value, ToolValidationError> {
    // Unions (anyOf/oneOf): take the first branch that fits.
    if let Some(branches) =
        schema.get("anyOf").or_else(|| schema.get("oneOf")).and_then(Value::as_array)
    {
        // Strict pass first: a branch the value already matches exactly wins with no coercion.
        for branch in branches {
            if let Ok(v) = coerce(branch, value.clone(), path, true) {
                return Ok(v);
            }
        }
        // Lenient pass: allow scalar coercion within a branch (skipped if already strict).
        let mut last_err = None;
        if !strict {
            for branch in branches {
                match coerce(branch, value.clone(), path, false) {
                    Ok(v) => return Ok(v),
                    Err(e) => last_err = Some(e),
                }
            }
        }
        return Err(last_err
            .unwrap_or_else(|| ToolValidationError::schema(path, "no union branch matched")));
    }

    // type-driven coercion.
    let coerced = match schema.get("type") {
        Some(Value::String(t)) => coerce_to_type(schema, t, value, path, strict)?,
        Some(Value::Array(types)) => {
            let mut done = None;
            let mut last_err = None;
            for t in types.iter().filter_map(Value::as_str) {
                match coerce_to_type(schema, t, value.clone(), path, strict) {
                    Ok(v) => {
                        done = Some(v);
                        break;
                    }
                    Err(e) => last_err = Some(e),
                }
            }
            match done {
                Some(v) => v,
                None => {
                    return Err(last_err.unwrap_or_else(|| {
                        ToolValidationError::schema(path, "no listed type matched")
                    }))
                }
            }
        }
        _ => {
            // No explicit type: infer object/array from the presence of structural keywords.
            if schema.contains_key("properties") || schema.contains_key("required") {
                coerce_to_type(schema, "object", value, path, strict)?
            } else if schema.contains_key("items") {
                coerce_to_type(schema, "array", value, path, strict)?
            } else {
                value
            }
        }
    };

    // enum membership (checked after coercion).
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.iter().any(|a| a == &coerced) {
            return Err(ToolValidationError::schema(
                path,
                format!("value {coerced} is not one of the permitted enum values"),
            ));
        }
    Ok(coerced)
}

fn coerce_to_type(
    schema: &Map<String, Value>,
    ty: &str,
    value: Value,
    path: &str,
    strict: bool,
) -> Result<Value, ToolValidationError> {
    match ty {
        "string" => coerce_string(value, path, strict),
        "integer" => coerce_integer(value, path, strict),
        "number" => coerce_number(value, path, strict),
        "boolean" => coerce_boolean(value, path, strict),
        "object" => coerce_object(schema, value, path, strict),
        "array" => coerce_array(schema, value, path, strict),
        "null" => {
            if value.is_null() {
                Ok(value)
            } else {
                Err(ToolValidationError::schema(path, "expected null"))
            }
        }
        // Unknown type keyword: nothing to enforce.
        _ => Ok(value),
    }
}

fn coerce_string(value: Value, path: &str, strict: bool) -> Result<Value, ToolValidationError> {
    match value {
        Value::String(_) => Ok(value),
        _ if strict => {
            Err(ToolValidationError::schema(path, format!("expected string, got {}", type_name(&value))))
        }
        Value::Number(n) => Ok(Value::String(n.to_string())),
        Value::Bool(b) => Ok(Value::String(b.to_string())),
        other => Err(ToolValidationError::schema(
            path,
            format!("cannot coerce {} to string", type_name(&other)),
        )),
    }
}

fn coerce_integer(value: Value, path: &str, strict: bool) -> Result<Value, ToolValidationError> {
    match value {
        Value::Number(ref n) if n.is_i64() || n.is_u64() => Ok(value),
        _ if strict => Err(ToolValidationError::schema(
            path,
            format!("expected integer, got {}", type_name(&value)),
        )),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() && f.fract() == 0.0 => integral_f64(f)
                .ok_or_else(|| ToolValidationError::schema(path, "integer out of range")),
            _ => Err(ToolValidationError::schema(path, "number is not an integer")),
        },
        Value::String(s) => parse_integer(s.trim())
            .ok_or_else(|| ToolValidationError::schema(path, format!("cannot coerce {s:?} to integer"))),
        other => Err(ToolValidationError::schema(
            path,
            format!("cannot coerce {} to integer", type_name(&other)),
        )),
    }
}

fn coerce_number(value: Value, path: &str, strict: bool) -> Result<Value, ToolValidationError> {
    match value {
        Value::Number(_) => Ok(value),
        _ if strict => Err(ToolValidationError::schema(
            path,
            format!("expected number, got {}", type_name(&value)),
        )),
        Value::String(s) => parse_number(s.trim())
            .ok_or_else(|| ToolValidationError::schema(path, format!("cannot coerce {s:?} to number"))),
        other => Err(ToolValidationError::schema(
            path,
            format!("cannot coerce {} to number", type_name(&other)),
        )),
    }
}

fn coerce_boolean(value: Value, path: &str, strict: bool) -> Result<Value, ToolValidationError> {
    match value {
        Value::Bool(_) => Ok(value),
        _ if strict => Err(ToolValidationError::schema(
            path,
            format!("expected boolean, got {}", type_name(&value)),
        )),
        Value::String(ref s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(ToolValidationError::schema(
                path,
                format!("cannot coerce {s:?} to boolean"),
            )),
        },
        other => Err(ToolValidationError::schema(
            path,
            format!("cannot coerce {} to boolean", type_name(&other)),
        )),
    }
}

fn coerce_object(
    schema: &Map<String, Value>,
    value: Value,
    path: &str,
    strict: bool,
) -> Result<Value, ToolValidationError> {
    let mut obj = match value {
        Value::Object(m) => m,
        other => {
            return Err(ToolValidationError::schema(
                path,
                format!("expected object, got {}", type_name(&other)),
            ))
        }
    };

    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (key, subschema) in props {
            if let Some(v) = obj.remove(key) {
                let child = format!("{path}.{key}");
                obj.insert(key.clone(), coerce(subschema, v, &child, strict)?);
            }
        }
    }

    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for rk in required.iter().filter_map(Value::as_str) {
            if !obj.contains_key(rk) {
                return Err(ToolValidationError::schema(
                    path,
                    format!("missing required field `{rk}`"),
                ));
            }
        }
    }

    Ok(Value::Object(obj))
}

fn coerce_array(
    schema: &Map<String, Value>,
    value: Value,
    path: &str,
    strict: bool,
) -> Result<Value, ToolValidationError> {
    let arr = match value {
        Value::Array(a) => a,
        other => {
            return Err(ToolValidationError::schema(
                path,
                format!("expected array, got {}", type_name(&other)),
            ))
        }
    };

    let coerced = match schema.get("items") {
        Some(item_schema) if item_schema.is_object() || item_schema.is_boolean() => {
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.into_iter().enumerate() {
                let child = format!("{path}[{i}]");
                out.push(coerce(item_schema, v, &child, strict)?);
            }
            out
        }
        _ => arr,
    };
    Ok(Value::Array(coerced))
}

fn parse_integer(s: &str) -> Option<Value> {
    if let Ok(i) = s.parse::<i64>() {
        Some(Value::Number(Number::from(i)))
    } else if let Ok(u) = s.parse::<u64>() {
        Some(Value::Number(Number::from(u)))
    } else {
        None
    }
}

fn parse_number(s: &str) -> Option<Value> {
    if let Some(v) = parse_integer(s) {
        return Some(v);
    }
    s.parse::<f64>().ok().filter(|f| f.is_finite()).and_then(Number::from_f64).map(Value::Number)
}

fn integral_f64(f: f64) -> Option<Value> {
    if f >= 0.0 && f <= u64::MAX as f64 {
        Some(Value::Number(Number::from(f as u64)))
    } else if f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        Some(Value::Number(Number::from(f as i64)))
    } else {
        None
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn valid_args_pass_through() {
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
            "required": ["n"],
        });
        let out = validate_tool_call(&schema, json!({ "n": 42 })).unwrap();
        assert_eq!(out, json!({ "n": 42 }));
    }

    #[test]
    fn string_to_number_and_bool() {
        let schema = json!({
            "type": "object",
            "properties": {
                "n": { "type": "integer" },
                "f": { "type": "number" },
                "b": { "type": "boolean" },
            },
        });
        let out = validate_tool_call(
            &schema,
            json!({ "n": "123", "f": "1.5", "b": "true" }),
        )
        .unwrap();
        assert_eq!(out, json!({ "n": 123, "f": 1.5, "b": true }));
    }

    #[test]
    fn number_and_bool_to_string() {
        let schema = json!({
            "type": "object",
            "properties": { "s": { "type": "string" }, "t": { "type": "string" } },
        });
        let out = validate_tool_call(&schema, json!({ "s": 7, "t": false })).unwrap();
        assert_eq!(out, json!({ "s": "7", "t": "false" }));
    }

    #[test]
    fn integral_float_to_integer() {
        let schema = json!({ "type": "integer" });
        assert_eq!(validate_tool_call(&schema, json!(5.0)).unwrap(), json!(5));
        assert!(validate_tool_call(&schema, json!(5.5)).is_err());
    }

    #[test]
    fn missing_required_errors() {
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
            "required": ["n"],
        });
        let err = validate_tool_call(&schema, json!({})).unwrap_err();
        assert!(matches!(err, ToolValidationError::Schema { .. }));
        assert!(err.to_string().contains("required"));
    }

    #[test]
    fn uncoercible_type_errors() {
        let schema = json!({ "type": "integer" });
        assert!(validate_tool_call(&schema, json!("not-a-number")).is_err());
    }

    #[test]
    fn nested_object_recurses() {
        let schema = json!({
            "type": "object",
            "properties": {
                "inner": {
                    "type": "object",
                    "properties": { "k": { "type": "integer" } },
                    "required": ["k"],
                },
            },
            "required": ["inner"],
        });
        let out =
            validate_tool_call(&schema, json!({ "inner": { "k": "9" } })).unwrap();
        assert_eq!(out, json!({ "inner": { "k": 9 } }));
    }

    #[test]
    fn array_items_coerce_elementwise() {
        let schema = json!({ "type": "array", "items": { "type": "integer" } });
        let out = validate_tool_call(&schema, json!(["1", 2, "3"])).unwrap();
        assert_eq!(out, json!([1, 2, 3]));
    }

    #[test]
    fn union_takes_first_fitting_branch() {
        let schema = json!({
            "anyOf": [
                { "type": "object", "properties": { "k": { "type": "integer" } }, "required": ["k"] },
                { "type": "string" },
            ],
        });
        // An object already matches the object branch (strict pass) and coerces its field.
        let obj = validate_tool_call(&schema, json!({ "k": "5" })).unwrap();
        assert_eq!(obj, json!({ "k": 5 }));
        // A string skips the object branch and lands on the string branch unchanged.
        let s = validate_tool_call(&schema, json!("hello")).unwrap();
        assert_eq!(s, json!("hello"));
        // A bare number coerces into the string branch (lenient pass) rather than failing.
        let n = validate_tool_call(&schema, json!(8)).unwrap();
        assert_eq!(n, json!("8"));
    }

    #[test]
    fn union_strict_pass_avoids_lossy_match() {
        // string branch appears first, but a number value must NOT be stringified when a
        // number branch also exists.
        let schema = json!({ "anyOf": [ { "type": "string" }, { "type": "number" } ] });
        assert_eq!(validate_tool_call(&schema, json!(42)).unwrap(), json!(42));
        assert_eq!(validate_tool_call(&schema, json!("hi")).unwrap(), json!("hi"));
    }

    #[test]
    fn enum_membership_enforced() {
        let schema = json!({ "type": "string", "enum": ["a", "b"] });
        assert_eq!(validate_tool_call(&schema, json!("a")).unwrap(), json!("a"));
        assert!(validate_tool_call(&schema, json!("z")).is_err());
    }

    #[test]
    fn unknown_properties_pass_through() {
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
        });
        let out = validate_tool_call(&schema, json!({ "n": "1", "extra": "keep" })).unwrap();
        assert_eq!(out, json!({ "n": 1, "extra": "keep" }));
    }

    #[test]
    fn by_name_lookup() {
        let int = json!({ "type": "integer" });
        let tools = vec![("a", &int)];
        assert_eq!(
            validate_named_tool_call(tools.clone(), "a", json!("3")).unwrap(),
            json!(3)
        );
        assert!(matches!(
            validate_named_tool_call(tools, "missing", json!(1)),
            Err(ToolValidationError::NotFound(_))
        ));
    }
}
