//! Tool-argument validation + coercion (func-01 R-01-034; arch-01 §3.9).
//!
//! A hand-rolled, schema-driven coercion pass using `serde_json` only (no external validator):
//! it validates model-emitted tool arguments against a tool's JSON-Schema `parameters` and
//! **coerces** compatible mismatches — `"123"` → `123`, `"true"` → `true`, recursing into objects
//! and arrays element-wise, and trying each branch of an `anyOf`/`oneOf` union (taking the first
//! that fits). Required fields and types are enforced; on an unrecoverable mismatch a typed
//! [`ToolValidationError`] describing the failure is returned.
//!
//! The coercion table is Pi's `coercePrimitiveByType`
//! (`pi/packages/ai/src/utils/validation.ts:58-126`) — including the falsy cross-coercions
//! `null`→`0`/`false`/`""`, `true`/`false`→`1`/`0`, `1`/`0`→`true`/`false` and
//! `""`/`0`/`false`→`null`. Recursion into containers follows `applySchemaObjectCoercion`
//! (`validation.ts:129-147`, which also coerces keys *not* named in `properties` through an
//! `additionalProperties` sub-schema) and `applySchemaArrayCoercion` (`validation.ts:150-166`,
//! which supports both the single-schema and the tuple form of `items`).
//!
//! Union disambiguation runs in two passes: first a **strict** pass that accepts a branch only when
//! the value already has the exact JSON type the branch wants (so a value that is already the right
//! shape is never lossily re-coerced), then a **lenient** pass that allows scalar coercion. This
//! keeps e.g. an object from being stringified just because a `{type:"string"}` branch appears
//! first in the union. A multi-entry `"type"` array is a union too and gets the same two passes —
//! Pi's `matchesUnionMember` guard (`validation.ts:204-214`) suppresses primitive coercion outright
//! whenever the value already satisfies one member, so `{"type":["number","null"]}` must leave
//! `null` as `null` rather than folding it to `0`.
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
        Self::Schema {
            path: path.to_string(),
            detail: detail.into(),
        }
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
        Value::Bool(false) => Err(ToolValidationError::schema(
            path,
            "schema `false` rejects all values",
        )),
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
    // Composition keywords. Pi runs three INDEPENDENT sequential passes inside
    // `coerceWithJsonSchema` (`pi/packages/ai/src/utils/validation.ts:189-201` @v0.83.0): `allOf`
    // merges every nested schema in order (`:189-193`), then a non-`else` `anyOf` pass
    // (`:195-197`), then a non-`else` `oneOf` pass (`:199-201`). The previous
    // `anyOf`.or_else(`oneOf`) treated the two as mutually exclusive alternatives and ignored
    // `allOf` entirely (PROV-016).
    let mut value = value;
    if let Some(nested) = schema.get("allOf").and_then(Value::as_array) {
        for sub in nested {
            value = coerce(sub, value, path, strict)?;
        }
    }
    if let Some(branches) = schema.get("anyOf").and_then(Value::as_array) {
        value = coerce_union(branches, value, path, strict)?;
    }
    if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
        value = coerce_union(branches, value, path, strict)?;
    }

    // type-driven coercion.
    let coerced = match schema.get("type") {
        Some(Value::String(t)) => coerce_to_type(schema, t, value, path, strict)?,
        Some(Value::Array(types)) => {
            // A `"type"` array is a union, so it gets the same strict-then-lenient treatment as
            // `anyOf` above: a type the value *already is* wins before any cross-type coercion is
            // attempted. This is Pi's `matchesUnionMember` guard (validation.ts:204-214) — with a
            // multi-member type list, a value that already matches one member is not run through
            // `coercePrimitiveByType` at all, which is what keeps `{"type":["number","null"]}`
            // from folding a `null` into `0` via the leading `"number"`.
            let mut done = None;
            let mut last_err = None;
            for t in types.iter().filter_map(Value::as_str) {
                if let Ok(v) = coerce_to_type(schema, t, value.clone(), path, true) {
                    done = Some(v);
                    break;
                }
            }
            if done.is_none() && !strict {
                for t in types.iter().filter_map(Value::as_str) {
                    match coerce_to_type(schema, t, value.clone(), path, false) {
                        Ok(v) => {
                            done = Some(v);
                            break;
                        }
                        Err(e) => last_err = Some(e),
                    }
                }
            }
            match done {
                Some(v) => v,
                None => {
                    return Err(last_err.unwrap_or_else(|| {
                        ToolValidationError::schema(path, "no listed type matched")
                    }));
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
        && !allowed.iter().any(|a| a == &coerced)
    {
        return Err(ToolValidationError::schema(
            path,
            format!("value {coerced} is not one of the permitted enum values"),
        ));
    }
    Ok(coerced)
}

/// One union pass over `branches` — Pi's `coerceWithUnionSchema` (`validation.ts:174-184`
/// @v0.83.0), which walks the members in order and takes the first whose coerced candidate
/// validates.
///
/// Strict-then-lenient: a branch the value already matches exactly wins before any cross-type
/// coercion is attempted, so a value that is already the right shape is never lossily re-coerced.
fn coerce_union(
    branches: &[Value],
    value: Value,
    path: &str,
    strict: bool,
) -> Result<Value, ToolValidationError> {
    for branch in branches {
        if let Ok(v) = coerce(branch, value.clone(), path, true) {
            return Ok(v);
        }
    }
    let mut last_err = None;
    if !strict {
        for branch in branches {
            match coerce(branch, value.clone(), path, false) {
                Ok(v) => return Ok(v),
                Err(e) => last_err = Some(e),
            }
        }
    }
    Err(last_err.unwrap_or_else(|| ToolValidationError::schema(path, "no union branch matched")))
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
        "null" => coerce_null(value, path, strict),
        // Unknown type keyword: nothing to enforce.
        _ => Ok(value),
    }
}

fn coerce_string(value: Value, path: &str, strict: bool) -> Result<Value, ToolValidationError> {
    match value {
        Value::String(_) => Ok(value),
        _ if strict => Err(ToolValidationError::schema(
            path,
            format!("expected string, got {}", type_name(&value)),
        )),
        Value::Number(n) => Ok(Value::String(n.to_string())),
        Value::Bool(b) => Ok(Value::String(b.to_string())),
        // `case "string"` null arm, validation.ts:112-114 — `null` becomes the empty string.
        Value::Null => Ok(Value::String(String::new())),
        other => Err(ToolValidationError::schema(
            path,
            format!("cannot coerce {} to string", type_name(&other)),
        )),
    }
}

/// `case "null"`, validation.ts:117-122: the three falsy JSON values `""`, `0` and `false` coerce
/// to `null`; everything else is left for the caller to reject.
fn coerce_null(value: Value, path: &str, strict: bool) -> Result<Value, ToolValidationError> {
    match value {
        Value::Null => Ok(value),
        _ if strict => Err(ToolValidationError::schema(
            path,
            format!("expected null, got {}", type_name(&value)),
        )),
        Value::String(ref s) if s.is_empty() => Ok(Value::Null),
        Value::Bool(false) => Ok(Value::Null),
        Value::Number(ref n) if is_js_zero(n) => Ok(Value::Null),
        other => Err(ToolValidationError::schema(
            path,
            format!("cannot coerce {} to null", type_name(&other)),
        )),
    }
}

/// `n === 0` in JS terms, for a `serde_json::Number` that may be stored as `i64`, `u64` or `f64`
/// (`0`, `0.0` and `-0.0` are all `=== 0`).
fn is_js_zero(n: &Number) -> bool {
    n.as_f64().is_some_and(|f| f == 0.0)
}

/// `n === 1` in JS terms; see [`is_js_zero`].
fn is_js_one(n: &Number) -> bool {
    n.as_f64().is_some_and(|f| f == 1.0)
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
            _ => Err(ToolValidationError::schema(
                path,
                "number is not an integer",
            )),
        },
        Value::String(s) => parse_integer(s.trim()).ok_or_else(|| {
            ToolValidationError::schema(path, format!("cannot coerce {s:?} to integer"))
        }),
        // `case "integer"` null/boolean arms, validation.ts:76-87.
        Value::Null => Ok(Value::Number(Number::from(0))),
        Value::Bool(b) => Ok(Value::Number(Number::from(i64::from(b)))),
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
        Value::String(s) => parse_number(s.trim()).ok_or_else(|| {
            ToolValidationError::schema(path, format!("cannot coerce {s:?} to number"))
        }),
        // `case "number"` null/boolean arms, validation.ts:60-73.
        Value::Null => Ok(Value::Number(Number::from(0))),
        Value::Bool(b) => Ok(Value::Number(Number::from(i64::from(b)))),
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
        // Pi compares the string EXACTLY — `if (value === "true") … if (value === "false")`
        // (`validation.ts:94-100` @v0.83.0, arm `:90-111`) — with no trim and no case fold; anything
        // else falls through unchanged and is then rejected by the type check. A `to_ascii_lowercase`
        // here executed a call pi refuses, on identical model output (PROV-046).
        Value::String(ref s) => match s.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(ToolValidationError::schema(
                path,
                format!("cannot coerce {s:?} to boolean"),
            )),
        },
        // `case "boolean"` null/number arms, validation.ts:89-109: `null` is `false`, and only the
        // two numbers `1` and `0` convert (Pi compares `value === 1` / `value === 0`, so `2` is
        // left alone rather than being treated as truthy).
        Value::Null => Ok(Value::Bool(false)),
        Value::Number(ref n) if is_js_one(n) => Ok(Value::Bool(true)),
        Value::Number(ref n) if is_js_zero(n) => Ok(Value::Bool(false)),
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
            ));
        }
    };

    let props = schema.get("properties").and_then(Value::as_object);
    if let Some(props) = props {
        for (key, subschema) in props {
            if let Some(v) = obj.remove(key) {
                let child = format!("{path}.{key}");
                obj.insert(key.clone(), coerce(subschema, v, &child, strict)?);
            }
        }
    }

    // `applySchemaObjectCoercion`'s second loop (validation.ts:139-146): when
    // `additionalProperties` carries a *sub-schema*, every key not named in `properties` is coerced
    // through it. Pi guards on `typeof … === "object"`, so a bare `true`/`false` — which declares
    // only whether extra keys are allowed, not their shape — is skipped here as it is there.
    if let Some(extra) = schema.get("additionalProperties").filter(|s| s.is_object()) {
        let keys: Vec<String> = obj
            .keys()
            .filter(|k| !props.is_some_and(|p| p.contains_key(k.as_str())))
            .cloned()
            .collect();
        for key in keys {
            // Coerce in place: `remove`/`insert` would move the key to the end of the map under
            // serde_json's `preserve_order` feature.
            if let Some(slot) = obj.get_mut(&key) {
                let child = format!("{path}.{key}");
                let taken = slot.take();
                *slot = coerce(extra, taken, &child, strict)?;
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
            ));
        }
    };

    let coerced = match schema.get("items") {
        // Tuple form (`applySchemaArrayCoercion`, validation.ts:151-158): element *i* is coerced by
        // sub-schema *i*, and an element past the end of the tuple has no schema, so Pi's
        // `if (!itemSchema) continue` leaves it untouched.
        Some(Value::Array(tuple)) => {
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.into_iter().enumerate() {
                match tuple.get(i) {
                    Some(item_schema) => {
                        let child = format!("{path}[{i}]");
                        out.push(coerce(item_schema, v, &child, strict)?);
                    }
                    None => out.push(v),
                }
            }
            out
        }
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
    s.parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .and_then(Number::from_f64)
        .map(Value::Number)
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
        let out =
            validate_tool_call(&schema, json!({ "n": "123", "f": "1.5", "b": "true" })).unwrap();
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
        let out = validate_tool_call(&schema, json!({ "inner": { "k": "9" } })).unwrap();
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
        assert_eq!(
            validate_tool_call(&schema, json!("hi")).unwrap(),
            json!("hi")
        );
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

    // ------------------------------------------------------------------ pi coercion-table parity
    // Ground truth for every case below is `coercePrimitiveByType`
    // (pi/packages/ai/src/utils/validation.ts:58-126), `applySchemaObjectCoercion` (:129-147) and
    // `applySchemaArrayCoercion` (:150-166) at the ported tag v0.83.0.

    /// validation.ts:139-146 — keys absent from `properties` are coerced through an
    /// `additionalProperties` sub-schema, and keys present in `properties` are not run through it.
    #[test]
    fn additional_properties_subschema_coerces_undeclared_keys() {
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
            "additionalProperties": { "type": "number" },
        });
        let out = validate_tool_call(&schema, json!({ "n": "1", "x": "2.5", "y": true })).unwrap();
        assert_eq!(out, json!({ "n": 1, "x": 2.5, "y": 1 }));
    }

    /// validation.ts:139 guards on `typeof … === "object"`, so a boolean `additionalProperties`
    /// declares only *whether* extra keys are allowed and coerces nothing.
    #[test]
    fn boolean_additional_properties_coerces_nothing() {
        let schema = json!({
            "type": "object",
            "properties": { "n": { "type": "integer" } },
            "additionalProperties": true,
        });
        let out = validate_tool_call(&schema, json!({ "n": "1", "x": "2.5" })).unwrap();
        assert_eq!(out, json!({ "n": 1, "x": "2.5" }));
    }

    /// validation.ts:151-158 — tuple-form `items`: element *i* uses sub-schema *i*, and an element
    /// past the end of the tuple has no schema and is left alone.
    #[test]
    fn tuple_form_items_coerce_positionally() {
        let schema = json!({
            "type": "array",
            "items": [{ "type": "integer" }, { "type": "string" }, { "type": "boolean" }],
        });
        let out = validate_tool_call(&schema, json!(["1", 2, "true", { "keep": 1 }])).unwrap();
        assert_eq!(out, json!([1, "2", true, { "keep": 1 }]));
    }

    /// validation.ts:61-63,77-79,90-92,112-114 — `null` folds to each type's zero value.
    #[test]
    fn null_coerces_to_each_types_zero_value() {
        assert_eq!(
            validate_tool_call(&json!({ "type": "number" }), json!(null)).unwrap(),
            json!(0)
        );
        assert_eq!(
            validate_tool_call(&json!({ "type": "integer" }), json!(null)).unwrap(),
            json!(0)
        );
        assert_eq!(
            validate_tool_call(&json!({ "type": "boolean" }), json!(null)).unwrap(),
            json!(false)
        );
        assert_eq!(
            validate_tool_call(&json!({ "type": "string" }), json!(null)).unwrap(),
            json!("")
        );
    }

    /// validation.ts:69-71,84-86 — `true`/`false` become `1`/`0` for number and integer.
    #[test]
    fn booleans_coerce_to_one_and_zero() {
        assert_eq!(
            validate_tool_call(&json!({ "type": "number" }), json!(true)).unwrap(),
            json!(1)
        );
        assert_eq!(
            validate_tool_call(&json!({ "type": "integer" }), json!(false)).unwrap(),
            json!(0)
        );
    }

    /// validation.ts:100-107 — only the exact numbers `1` and `0` become booleans; Pi compares
    /// `value === 1` / `value === 0`, so `2` is *not* treated as truthy.
    #[test]
    fn only_one_and_zero_coerce_to_boolean() {
        let schema = json!({ "type": "boolean" });
        assert_eq!(validate_tool_call(&schema, json!(1)).unwrap(), json!(true));
        assert_eq!(validate_tool_call(&schema, json!(0)).unwrap(), json!(false));
        assert_eq!(validate_tool_call(&schema, json!(1.0)).unwrap(), json!(true));
        assert!(validate_tool_call(&schema, json!(2)).is_err());
    }

    /// validation.ts:117-122 — the three falsy JSON values coerce to `null`; nothing else does.
    #[test]
    fn falsy_values_coerce_to_null() {
        let schema = json!({ "type": "null" });
        assert_eq!(validate_tool_call(&schema, json!("")).unwrap(), json!(null));
        assert_eq!(validate_tool_call(&schema, json!(0)).unwrap(), json!(null));
        assert_eq!(
            validate_tool_call(&schema, json!(false)).unwrap(),
            json!(null)
        );
        assert_eq!(
            validate_tool_call(&schema, json!(null)).unwrap(),
            json!(null)
        );
        assert!(validate_tool_call(&schema, json!("x")).is_err());
        assert!(validate_tool_call(&schema, json!(1)).is_err());
    }

    /// validation.ts:204-214 `matchesUnionMember` — with a multi-entry `"type"`, a value that
    /// already matches one member is never run through `coercePrimitiveByType`, so the leading
    /// member does not get to claim it.
    #[test]
    fn multi_type_union_keeps_a_value_that_already_matches() {
        assert_eq!(
            validate_tool_call(&json!({ "type": ["number", "null"] }), json!(null)).unwrap(),
            json!(null)
        );
        assert_eq!(
            validate_tool_call(&json!({ "type": ["string", "number"] }), json!(42)).unwrap(),
            json!(42)
        );
        // With no exact member the lenient pass still runs, left to right.
        assert_eq!(
            validate_tool_call(&json!({ "type": ["string", "number"] }), json!(true)).unwrap(),
            json!("true")
        );
    }

    /// The container coercions recurse, so a tuple entry and an `additionalProperties` value are
    /// themselves coerced by the full table rather than only at the top level.
    #[test]
    fn container_coercions_recurse() {
        let schema = json!({
            "type": "object",
            "additionalProperties": {
                "type": "array",
                "items": [{ "type": "boolean" }, { "type": "integer" }],
            },
        });
        let out = validate_tool_call(&schema, json!({ "a": [1, null], "b": [0, "7"] })).unwrap();
        assert_eq!(out, json!({ "a": [true, 0], "b": [false, 7] }));
    }

    /// PROV-016. `allOf` composition is a sequential merge (`validation.ts:189-193` @v0.83.0):
    /// every nested schema's properties are coerced, not just the first. Red before the fix — the
    /// `anyOf`/`oneOf` `or_else` never looked at `allOf` at all, so both values arrived uncoerced.
    #[test]
    fn all_of_coerces_every_branch() {
        let schema = json!({
            "allOf": [
                { "type": "object", "properties": { "n": { "type": "integer" } } },
                { "type": "object", "properties": { "b": { "type": "boolean" } } },
            ],
        });
        let out = validate_tool_call(&schema, json!({ "n": "42", "b": "true" })).unwrap();
        assert_eq!(out, json!({ "n": 42, "b": true }));
    }

    /// PROV-016, second half: `anyOf` and `oneOf` are INDEPENDENT passes (`:195-197`, `:199-201`),
    /// not alternatives — a schema carrying both applies both. Red before the fix: `.or_else` took
    /// `anyOf` and dropped `oneOf` on the floor.
    #[test]
    fn any_of_and_one_of_both_apply() {
        let schema = json!({
            "anyOf": [{ "type": "integer" }],
            "oneOf": [{ "type": "string" }],
        });
        // `anyOf` coerces "5" → 5, then `oneOf` independently coerces 5 → "5".
        assert_eq!(validate_tool_call(&schema, json!("5")).unwrap(), json!("5"));
    }

    /// PROV-046. Pi compares boolean strings exactly (`validation.ts:94-100` @v0.83.0), so `"True"`
    /// is left unchanged and then rejected by the type check. Red before the fix: cyrup trimmed and
    /// case-folded, so it EXECUTED a tool call pi refuses.
    #[test]
    fn boolean_string_coercion_is_exact() {
        let schema = json!({ "type": "boolean" });
        assert_eq!(validate_tool_call(&schema, json!("true")).unwrap(), json!(true));
        assert_eq!(
            validate_tool_call(&schema, json!("false")).unwrap(),
            json!(false)
        );
        for rejected in ["True", " true ", "TRUE", "False"] {
            assert!(
                validate_tool_call(&schema, json!(rejected)).is_err(),
                "{rejected:?} must be rejected, as pi rejects it"
            );
        }
        // The number/null arms are unchanged (`validation.ts:102-109`).
        assert_eq!(validate_tool_call(&schema, json!(1)).unwrap(), json!(true));
        assert_eq!(validate_tool_call(&schema, json!(0)).unwrap(), json!(false));
        assert_eq!(validate_tool_call(&schema, json!(null)).unwrap(), json!(false));
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
