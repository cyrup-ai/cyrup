//! The handful of TypeBox constructors pi's schema sources use, reproduced as `serde_json::Value`
//! builders that emit the same JSON Schema shapes in the same key order.
//!
//! pi writes its configuration schemas with TypeBox (`typebox` 1.x, pinned by
//! `packages/coding-agent/package.json` @f1b2e77f5) and serializes the resulting objects with
//! `JSON.stringify` (`scripts/generate-schemas.ts:129-141`). TypeBox puts the structural keys
//! first — `type`, then `required` / `properties` for an object, `items` for an array,
//! `patternProperties` for a record, `anyOf` for a union — and appends the caller's options
//! (`description`, `default`, `minimum`, …) after them in the order the caller wrote them. The
//! committed `packages/coding-agent/schemas/*.schema.json` @f1b2e77f5 show exactly that layout, and
//! these builders reproduce it so the cyrup documents read like pi's. `serde_json/preserve_order`
//! is declared workspace-wide (`Cargo.toml`), so the insertion order below is the emitted order.

use serde_json::{Map, Value, json};

/// `Type.String()`.
pub(super) fn string() -> Value {
    json!({ "type": "string" })
}

/// `Type.String({ minLength: 1 })`.
pub(super) fn non_empty_string() -> Value {
    json!({ "type": "string", "minLength": 1 })
}

/// `Type.Number()`.
pub(super) fn number() -> Value {
    json!({ "type": "number" })
}

/// `Type.Integer()`.
pub(super) fn integer() -> Value {
    json!({ "type": "integer" })
}

/// `Type.Boolean()`.
pub(super) fn boolean() -> Value {
    json!({ "type": "boolean" })
}

/// `Type.Null()`.
pub(super) fn null() -> Value {
    json!({ "type": "null" })
}

/// `Type.Unknown()` — the empty schema, which accepts every value.
pub(super) fn unknown() -> Value {
    json!({})
}

/// `Type.Literal(value)`: `{ type: typeof value, const: value }`.
pub(super) fn literal(value: impl Into<Value>) -> Value {
    let value = value.into();
    let ty = match &value {
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        _ => "string",
    };
    json!({ "type": ty, "const": value })
}

/// `Type.Union([...Type.Literal(v)])` over string literals, the shape pi writes for every closed
/// string set in `settings-schema.ts` and `compat-schema.ts`.
pub(super) fn string_literals(values: &[&str]) -> Value {
    union(values.iter().map(|v| literal(*v)).collect())
}

/// `Type.Union(items)`.
pub(super) fn union(items: Vec<Value>) -> Value {
    json!({ "anyOf": items })
}

/// `Type.Enum(values)` — emitted as a bare `enum`, with no `type` (see `$defs.ModelThinkingLevel`
/// in pi's committed `settings.schema.json`).
pub(super) fn enumeration(values: &[&str]) -> Value {
    json!({ "enum": values })
}

/// `Type.Array(items)`.
pub(super) fn array(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

/// `Type.Record(Type.String(), value)` — TypeBox spells a string-keyed record as the catch-all
/// pattern `^.*$`.
pub(super) fn record(value: Value) -> Value {
    json!({ "type": "object", "patternProperties": { "^.*$": value } })
}

/// One `Type.Object` property: its name, its schema, and whether it is wrapped in
/// `Type.Optional` (an optional property is simply left out of `required`).
pub(super) struct Prop {
    name: &'static str,
    schema: Value,
    optional: bool,
}

/// A required property.
pub(super) fn req(name: &'static str, schema: Value) -> Prop {
    Prop {
        name,
        schema,
        optional: false,
    }
}

/// A `Type.Optional(...)` property.
pub(super) fn opt(name: &'static str, schema: Value) -> Prop {
    Prop {
        name,
        schema,
        optional: true,
    }
}

/// `Type.Object(properties)`: `type`, then `required` (only when some property is required), then
/// `properties`.
pub(super) fn object(props: Vec<Prop>) -> Value {
    let required: Vec<&str> = props
        .iter()
        .filter(|p| !p.optional)
        .map(|p| p.name)
        .collect();
    let mut out = Map::new();
    out.insert("type".into(), "object".into());
    if !required.is_empty() {
        out.insert("required".into(), json!(required));
    }
    let properties: Map<String, Value> = props
        .into_iter()
        .map(|p| (p.name.to_string(), p.schema))
        .collect();
    out.insert("properties".into(), Value::Object(properties));
    Value::Object(out)
}

/// `Type.Partial(...)` over a `Type.Object` — drops `required`.
pub(super) fn partial(mut schema: Value) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("required");
    }
    schema
}

/// The TypeBox `options` argument: append each `(key, value)` after the structural keys, in the
/// caller's order, replacing a key the schema already has.
pub(super) fn with(mut schema: Value, options: Value) -> Value {
    if let (Some(target), Value::Object(options)) = (schema.as_object_mut(), options) {
        for (key, value) in options {
            target.insert(key, value);
        }
    }
    schema
}

/// `with(schema, { description })`.
pub(super) fn described(schema: Value, description: &str) -> Value {
    with(schema, json!({ "description": description }))
}
