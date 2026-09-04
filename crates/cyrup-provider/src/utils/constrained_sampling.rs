//! Provider-side constrained sampling for tools — a 1:1 port of Pi
//! `packages/ai/src/api/constrained-sampling.ts` @**v0.84.2** (the file was byte-identical from
//! v0.83.0 through v0.84.1; pi commit `7915cdac` — *"feat(ai): add strict tool schema
//! conversion"*, first tagged v0.84.2 — added the strict-conversion half). PROV-011 / DRIFT-018.
//!
//! Two independent mechanisms share this module because upstream keeps them in one file:
//!
//! 1. **JSON-schema strict sampling** — [`resolve_json_schema_strict_sampling`],
//!    [`make_strict_json_schema`] and [`json_schema_tool_parameters`]. A tool declaring
//!    `constrainedSampling: {type:"json_schema", strict:"prefer"|"require"}` asks the provider to
//!    constrain generation to the declared schema. `prefer` degrades silently on a model that
//!    cannot do it; `require` fails the request. Resolving `strict` and CONVERTING the schema are
//!    one indivisible step: a provider told `strict: true` rejects any schema outside the strict
//!    subset, so every adapter serializes [`json_schema_tool_parameters`] rather than the tool's
//!    raw `parameters`.
//! 2. **Grammar-constrained tools** — [`resolve_grammar_constrained_sampling`] and
//!    [`create_grammar_tool_input_properties`]. A tool declaring
//!    `constrainedSampling: {type:"grammar", variants:{openai_lark|openai_regex}}` is serialized as
//!    an OpenAI *custom* tool whose single required string property is generated under a Lark
//!    grammar or a regex. Because the wire form is a bare string rather than a JSON object, the
//!    decoder has to synthesize the JSON argument object itself — that is
//!    [`GrammarToolInputJsonBuffer`] / [`append_grammar_tool_input_json_delta`] on the streaming
//!    side and [`get_grammar_tool_input`] on the non-streaming side.
//!
//! # `[CYRUP-DELTA]` — thrown `Error` becomes `Err(ConstrainedSamplingError)`
//!
//! Upstream throws; every consuming site is inside a `try` that turns the throw into the turn's
//! terminal error message. cyrup's api impls do not unwind, so the five fallible functions return
//! `Result` and each caller maps the error into its own `ProviderError`. The **message text** is
//! reproduced verbatim, because it reaches `AssistantMessage.error_message` on both sides.

use crate::context::{ConstrainedSamplingConfig, StrictSampling, ToolDef};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::fmt;

/// A constrained-sampling configuration that cannot be honoured. Its `Display` is byte-identical to
/// the message pi's corresponding `throw new Error(...)` carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstrainedSamplingError(pub String);

impl fmt::Display for ConstrainedSamplingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConstrainedSamplingError {}

impl From<ConstrainedSamplingError> for crate::error::ProviderError {
    fn from(e: ConstrainedSamplingError) -> Self {
        crate::error::ProviderError::Decode(e.0)
    }
}

type Result<T> = std::result::Result<T, ConstrainedSamplingError>;

fn err<T>(message: impl Into<String>) -> Result<T> {
    Err(ConstrainedSamplingError(message.into()))
}

/// Pi `UNSUPPORTED_STRICT_SCHEMA_KEYS` (`constrained-sampling.ts:12-29` @v0.84.2). A schema
/// carrying any of these cannot be expressed in the strict subset the providers constrain against.
const UNSUPPORTED_STRICT_SCHEMA_KEYS: [&str; 16] = [
    "$ref",
    "$defs",
    "definitions",
    "allOf",
    "oneOf",
    "patternProperties",
    "dependentSchemas",
    "dependencies",
    "unevaluatedProperties",
    "propertyNames",
    "contains",
    "prefixItems",
    "not",
    "if",
    "then",
    "else",
];

/// Pi `isStructuredSchema` (`constrained-sampling.ts:35-44` @v0.84.2).
fn is_structured_schema(schema: &Value) -> bool {
    let Some(o) = schema.as_object() else {
        return false;
    };
    let types: Vec<&str> = match o.get("type") {
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    types.contains(&"object")
        || types.contains(&"array")
        || o.contains_key("properties")
        || o.contains_key("items")
}

/// Pi `schemaAllowsNull` (`constrained-sampling.ts:46-51` @v0.84.2).
fn schema_allows_null(schema: &Value) -> bool {
    let Some(o) = schema.as_object() else {
        return false;
    };
    let ty_is_null = match o.get("type") {
        Some(Value::String(s)) => s == "null",
        Some(Value::Array(a)) => a.iter().any(|v| v.as_str() == Some("null")),
        _ => false,
    };
    if ty_is_null {
        return true;
    }
    if o.get("const") == Some(&Value::Null) {
        return true;
    }
    if o.get("enum")
        .and_then(Value::as_array)
        .is_some_and(|a| a.contains(&Value::Null))
    {
        return true;
    }
    o.get("anyOf")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(schema_allows_null))
}

/// Pi `makeJsonSchemaNodeStrict` (`constrained-sampling.ts:53-115` @v0.84.2) — mutates `schema` in
/// place. The error strings are pi's `UnsupportedStrictJsonSchemaError` messages verbatim; they
/// reach the model through [`ConstrainedSamplingError`] exactly as pi's thrown text does.
fn make_json_schema_node_strict(schema: &mut Value) -> Result<()> {
    let Some(o) = schema.as_object_mut() else {
        return err("boolean schemas are unsupported");
    };
    for key in UNSUPPORTED_STRICT_SCHEMA_KEYS {
        if o.contains_key(key) {
            return err(format!("{key} schemas are unsupported"));
        }
    }

    if let Some(any_of) = o.get_mut("anyOf") {
        let Some(variants) = any_of.as_array_mut().filter(|a| !a.is_empty()) else {
            return err("anyOf must contain at least one schema");
        };
        for variant in variants.iter_mut() {
            if is_structured_schema(variant) {
                return err("object and array unions are unsupported");
            }
            make_json_schema_node_strict(variant)?;
        }
    }

    if let Some(items) = o.get_mut("items") {
        if items.is_array() {
            return err("tuple schemas are unsupported");
        }
        make_json_schema_node_strict(items)?;
    }

    let is_object_schema = o.get("type") == Some(&Value::String("object".to_string()));
    if o.contains_key("properties") && !is_object_schema {
        return err("properties require type object");
    }
    if !is_object_schema {
        return Ok(());
    }
    match o.get("additionalProperties") {
        None | Some(Value::Bool(false)) => {}
        Some(_) => return err("schema-valued or true additionalProperties is unsupported"),
    }
    if o.get("properties").is_some_and(|p| !p.is_object()) {
        return err("object properties must be a schema map");
    }
    if o.get("required").is_some_and(|r| {
        r.as_array()
            .is_none_or(|a| a.iter().any(|k| !k.is_string()))
    }) {
        return err("object required must be a string array");
    }

    let required: Vec<String> = o
        .get("required")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let property_names: Vec<String> = o
        .get("properties")
        .and_then(Value::as_object)
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default();
    if required.iter().any(|k| !property_names.contains(k)) {
        return err("required contains an unknown property");
    }

    if let Some(properties) = o.get_mut("properties").and_then(Value::as_object_mut) {
        for (key, property) in properties.iter_mut() {
            make_json_schema_node_strict(property)?;
            // Pi wraps every non-required property in `anyOf: [property, {type:"null"}]`
            // (`constrained-sampling.ts:110-112`) so the constrainer can require EVERY key while
            // still letting the model decline one by emitting `null`.
            if !required.contains(key) && !schema_allows_null(property) {
                let inner = property.take();
                *property = json!({ "anyOf": [inner, { "type": "null" }] });
            }
        }
    }
    o.insert("required".to_string(), json!(property_names));
    o.insert("additionalProperties".to_string(), Value::Bool(false));
    Ok(())
}

/// Pi `makeStrictJsonSchema` (`constrained-sampling.ts:117-127` @v0.84.2). Clones first — the
/// caller's schema is never mutated (upstream `structuredClone`).
pub fn make_strict_json_schema(schema: &Value) -> Result<Value> {
    let mut cloned = schema.clone();
    if !cloned.is_object() {
        return err("root schema must have type object");
    }
    make_json_schema_node_strict(&mut cloned)?;
    if cloned.get("type") != Some(&Value::String("object".to_string())) {
        return err("root schema must have type object");
    }
    Ok(cloned)
}

/// Pi `getJsonSchemaToolParameters` (`constrained-sampling.ts:129-131` @v0.84.2) — the schema an
/// adapter must serialize. Upstream does NOT catch the throw here: a `strict === true` that came
/// from a caller DEFAULT rather than from [`resolve_json_schema_strict_sampling`] surfaces the raw
/// message, so this returns `Err` carrying that same bare text.
pub fn json_schema_tool_parameters(tool: &ToolDef, strict: bool) -> Result<Value> {
    if strict {
        make_strict_json_schema(&tool.parameters)
    } else {
        Ok(tool.parameters.clone())
    }
}

/// The grammar encoding chosen for a tool (Pi `GrammarConstrainedSampling.format`,
/// `constrained-sampling.ts:10`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrammarFormat {
    Lark,
    Regex,
}

impl GrammarFormat {
    /// The OpenAI wire discriminant (`"lark"` / `"regex"`).
    pub fn as_str(self) -> &'static str {
        match self {
            GrammarFormat::Lark => "lark",
            GrammarFormat::Regex => "regex",
        }
    }
}

/// Pi `GrammarConstrainedSampling` (`constrained-sampling.ts:9-13`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrammarConstrainedSampling {
    pub format: GrammarFormat,
    pub definition: String,
    pub input_property: String,
}

/// Pi `GrammarToolInputJsonBuffer` (`constrained-sampling.ts:15-19`) — the decoder-side state that
/// turns a stream of raw grammar text into a stream of JSON-object argument deltas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrammarToolInputJsonBuffer {
    pub input: String,
    pub started: bool,
    pub closed: bool,
}

/// Pi `getGrammarToolInput` (`constrained-sampling.ts:21-31`) — read the single grammar-generated
/// string out of an already-parsed argument object.
pub fn get_grammar_tool_input(
    tool_name: &str,
    arguments: &Map<String, Value>,
    input_property: &str,
) -> Result<String> {
    match arguments.get(input_property) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => err(format!(
            "Grammar tool call \"{tool_name}\" requires argument \"{input_property}\" to be a string."
        )),
    }
}

/// Pi `appendGrammarToolInputJsonDelta` (`constrained-sampling.ts:33-62`).
///
/// `next_input` is the **cumulative** grammar text seen so far, not a delta. Returns the JSON text
/// to append to the tool call's `arguments`, or `None` when there is nothing to emit.
pub fn append_grammar_tool_input_json_delta(
    buffer: &mut GrammarToolInputJsonBuffer,
    input_property: &str,
    next_input: &str,
    close: bool,
) -> Result<Option<String>> {
    if buffer.closed {
        if close && next_input == buffer.input {
            return Ok(None);
        }
        return err(format!(
            "grammar tool input for property \"{input_property}\" changed after it was closed"
        ));
    }
    if !next_input.starts_with(&buffer.input) {
        return err(format!(
            "grammar tool input for property \"{input_property}\" changed non-monotonically"
        ));
    }

    let input_delta = &next_input[buffer.input.len()..];
    if !close && input_delta.is_empty() {
        return Ok(None);
    }

    let mut delta = String::new();
    if !buffer.started {
        // `{${JSON.stringify(inputProperty)}:"` — note pi emits no space after the colon.
        delta.push('{');
        delta.push_str(&json_string(input_property));
        delta.push_str(":\"");
        buffer.started = true;
    }
    // `JSON.stringify(inputDelta).slice(1, -1)` — the escaped body without its quotes.
    let quoted = json_string(input_delta);
    delta.push_str(&quoted[1..quoted.len() - 1]);
    buffer.input = next_input.to_string();

    if close {
        delta.push_str("\"}");
        buffer.closed = true;
    }
    Ok(Some(delta))
}

/// `JSON.stringify(s)` for a string — quotes included.
fn json_string(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

/// Pi `inferGrammarInputProperty` (`constrained-sampling.ts:64-81`, module-private upstream).
fn infer_grammar_input_property(tool: &ToolDef) -> Result<String> {
    let schema = &tool.parameters;
    if schema.get("type").and_then(Value::as_str) != Some("object") {
        return err("grammar constrained sampling requires an object parameter schema");
    }
    let required = match schema.get("required") {
        Some(Value::Array(a)) => a,
        // `!Array.isArray(schema.required)` — a missing or non-array `required` lands here.
        _ => {
            return err(
                "grammar constrained sampling requires exactly one required string property",
            );
        }
    };
    if required.len() != 1 {
        return err("grammar constrained sampling requires exactly one required string property");
    }
    // `.first()` rather than `[0]`: the `len() != 1` guard above already makes the index
    // infallible, but the workspace denies `clippy::indexing_slicing` and an unindexed read is
    // the same instruction. `None` folds into the same `err` the non-string case takes, which is
    // what `schema.required[0]` being `undefined` would produce upstream.
    let Some(input_property) = required.first().and_then(Value::as_str) else {
        return err("grammar constrained sampling requires exactly one required string property");
    };

    // `!schema.properties?.[inputProperty]` — absent, `null` and `false` are all falsy upstream.
    let entry = schema
        .get("properties")
        .and_then(Value::as_object)
        .and_then(|p| p.get(input_property));
    let truthy = matches!(entry, Some(v) if !v.is_null() && v != &Value::Bool(false));
    if !truthy {
        return err(format!(
            "grammar constrained sampling requires a properties entry for {input_property}"
        ));
    }
    // `schema.properties[inputProperty]?.type !== "string"`.
    if entry.and_then(|v| v.get("type")).and_then(Value::as_str) != Some("string") {
        return err(format!(
            "grammar constrained sampling property {input_property} must have type string"
        ));
    }
    Ok(input_property.to_string())
}

/// Pi `resolveJsonSchemaStrictSampling` (`constrained-sampling.ts:208-227` @v0.84.2).
///
/// `Ok(None)` is upstream's `undefined` — the tool is serialized with no `strict` decision of its
/// own. `Ok(Some(true))` is upstream's `true`; upstream never returns `false` here.
///
/// A route that CAN do strict mode still only gets `true` when the tool's schema actually converts
/// to the strict subset ([`make_strict_json_schema`]); a schema that does not convert degrades to
/// `None` under `prefer` and fails the request under `require`, carrying the conversion's own
/// reason. Upstream distinguishes `UnsupportedStrictJsonSchemaError` from any other throw and
/// rethrows the latter; every error [`make_strict_json_schema`] can produce here is of that one
/// kind, so the Rust match needs no discriminant.
pub fn resolve_json_schema_strict_sampling(
    tool: &ToolDef,
    supports_strict_mode: bool,
) -> Result<Option<bool>> {
    let Some(ConstrainedSamplingConfig::JsonSchema { strict }) =
        tool.constrained_sampling.as_ref().and_then(|c| c.config())
    else {
        return Ok(None);
    };

    if supports_strict_mode {
        return match make_strict_json_schema(&tool.parameters) {
            Ok(_) => Ok(Some(true)),
            Err(e) if *strict == StrictSampling::Require => err(format!(
                "Tool \"{}\" requires JSON-schema constrained sampling, but {}.",
                tool.name, e.0
            )),
            Err(_) => Ok(None),
        };
    }
    if *strict == StrictSampling::Require {
        return err(format!(
            "Tool \"{}\" requires JSON-schema constrained sampling, but strict tools are unsupported.",
            tool.name
        ));
    }
    Ok(None)
}

/// Pi `resolveGrammarConstrainedSampling` (`constrained-sampling.ts:99-130`).
pub fn resolve_grammar_constrained_sampling(
    tool: &ToolDef,
    supports_openai_grammar_tools: bool,
) -> Result<Option<GrammarConstrainedSampling>> {
    let Some(ConstrainedSamplingConfig::Grammar { variants }) =
        tool.constrained_sampling.as_ref().and_then(|c| c.config())
    else {
        return Ok(None);
    };

    if !supports_openai_grammar_tools {
        return Ok(None);
    }

    // `typeof d === "string" && d.trim().length > 0`.
    let lark = variants
        .openai_lark
        .as_deref()
        .filter(|d| !d.trim().is_empty());
    let regex = variants
        .openai_regex
        .as_deref()
        .filter(|d| !d.trim().is_empty());
    let Some((format, definition)) = lark
        .map(|d| (GrammarFormat::Lark, d))
        .or(regex.map(|d| (GrammarFormat::Regex, d)))
    else {
        return err(format!(
            "Tool \"{}\" cannot use grammar constrained sampling: no supported grammar variant was provided.",
            tool.name
        ));
    };

    // Upstream wraps ONLY `inferGrammarInputProperty` in the try/catch, re-prefixing its message
    // and appending a period; the "no supported grammar variant" throw above is outside it.
    match infer_grammar_input_property(tool) {
        Ok(input_property) => Ok(Some(GrammarConstrainedSampling {
            format,
            definition: definition.to_string(),
            input_property,
        })),
        Err(e) => err(format!(
            "Tool \"{}\" cannot use grammar constrained sampling: {}.",
            tool.name, e.0
        )),
    }
}

/// Pi `createGrammarToolInputProperties` (`constrained-sampling.ts:132-147`) — tool name → the
/// property name its grammar output is written to. Tools that resolve to no grammar are absent.
pub fn create_grammar_tool_input_properties(
    tools: &[ToolDef],
    supports_openai_grammar_tools: bool,
) -> Result<HashMap<String, String>> {
    let mut properties = HashMap::new();
    for tool in tools {
        if let Some(grammar) =
            resolve_grammar_constrained_sampling(tool, supports_openai_grammar_tools)?
        {
            properties.insert(tool.name.clone(), grammar.input_property);
        }
    }
    Ok(properties)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]
    use super::*;
    use crate::context::{ConstrainedSampling, GrammarVariants};
    use serde_json::json;

    fn tool(constrained: Option<ConstrainedSampling>) -> ToolDef {
        ToolDef {
            name: "grammar_tool".into(),
            description: "d".into(),
            parameters: json!({
                "type": "object",
                "properties": { "expression": { "type": "string" } },
                "required": ["expression"],
            }),
            constrained_sampling: constrained,
        }
    }

    fn json_schema(strict: StrictSampling) -> Option<ConstrainedSampling> {
        Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::JsonSchema { strict },
        ))
    }

    fn grammar(lark: Option<&str>, regex: Option<&str>) -> Option<ConstrainedSampling> {
        Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: lark.map(str::to_string),
                    openai_regex: regex.map(str::to_string),
                },
            },
        ))
    }

    // ---- resolveJsonSchemaStrictSampling (constrained-sampling.ts:83-97) ----

    #[test]
    fn strict_sampling_is_undefined_without_a_config() {
        assert_eq!(
            resolve_json_schema_strict_sampling(&tool(None), true),
            Ok(None)
        );
        // pi's `false` literal is indistinguishable from an absent field.
        assert_eq!(
            resolve_json_schema_strict_sampling(
                &tool(Some(ConstrainedSampling::Disabled(false))),
                true
            ),
            Ok(None)
        );
        // A grammar config is not a json_schema config.
        assert_eq!(
            resolve_json_schema_strict_sampling(&tool(grammar(Some("start: /x/"), None)), true),
            Ok(None)
        );
    }

    #[test]
    fn strict_sampling_prefers_then_degrades_and_require_throws() {
        for strict in [StrictSampling::Prefer, StrictSampling::Require] {
            assert_eq!(
                resolve_json_schema_strict_sampling(&tool(json_schema(strict)), true),
                Ok(Some(true)),
                "a strict-capable model always resolves true"
            );
        }
        // `prefer` on an incapable model degrades silently.
        assert_eq!(
            resolve_json_schema_strict_sampling(&tool(json_schema(StrictSampling::Prefer)), false),
            Ok(None)
        );
        // `require` on an incapable model fails, with pi's exact wording.
        assert_eq!(
            resolve_json_schema_strict_sampling(&tool(json_schema(StrictSampling::Require)), false),
            err(
                "Tool \"grammar_tool\" requires JSON-schema constrained sampling, but strict tools are unsupported."
            )
        );
    }

    // ---- resolveGrammarConstrainedSampling (constrained-sampling.ts:99-130) ----

    #[test]
    fn grammar_prefers_lark_over_regex_and_needs_capability() {
        // No capability => silently no grammar, even with a valid variant.
        assert_eq!(
            resolve_grammar_constrained_sampling(&tool(grammar(Some("start: /x/"), None)), false),
            Ok(None)
        );
        // Lark wins when both are present.
        assert_eq!(
            resolve_grammar_constrained_sampling(
                &tool(grammar(Some("start: /x/"), Some("[a-z]+"))),
                true
            ),
            Ok(Some(GrammarConstrainedSampling {
                format: GrammarFormat::Lark,
                definition: "start: /x/".into(),
                input_property: "expression".into(),
            }))
        );
        // Regex is used when lark is absent, and when lark is present but blank —
        // `d.trim().length > 0`.
        for lark in [None, Some("   ")] {
            assert_eq!(
                resolve_grammar_constrained_sampling(&tool(grammar(lark, Some("[a-z]+"))), true),
                Ok(Some(GrammarConstrainedSampling {
                    format: GrammarFormat::Regex,
                    definition: "[a-z]+".into(),
                    input_property: "expression".into(),
                }))
            );
        }
        // Neither variant usable.
        assert_eq!(
            resolve_grammar_constrained_sampling(&tool(grammar(Some(" "), None)), true),
            err(
                "Tool \"grammar_tool\" cannot use grammar constrained sampling: no supported grammar variant was provided."
            )
        );
    }

    #[test]
    fn grammar_input_property_inference_rejects_bad_schemas() {
        let cases: Vec<(Value, &str)> = vec![
            (
                json!({"type": "string"}),
                "grammar constrained sampling requires an object parameter schema",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string"}}}),
                "grammar constrained sampling requires exactly one required string property",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a", "b"]}),
                "grammar constrained sampling requires exactly one required string property",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "string"}}, "required": [7]}),
                "grammar constrained sampling requires exactly one required string property",
            ),
            (
                json!({"type": "object", "properties": {"b": {"type": "string"}}, "required": ["a"]}),
                "grammar constrained sampling requires a properties entry for a",
            ),
            (
                json!({"type": "object", "properties": {"a": {"type": "number"}}, "required": ["a"]}),
                "grammar constrained sampling property a must have type string",
            ),
        ];
        for (parameters, message) in cases {
            let mut t = tool(grammar(Some("start: /x/"), None));
            t.parameters = parameters.clone();
            assert_eq!(
                resolve_grammar_constrained_sampling(&t, true),
                err(format!(
                    "Tool \"grammar_tool\" cannot use grammar constrained sampling: {message}."
                )),
                "for {parameters}"
            );
        }
    }

    #[test]
    fn grammar_tool_input_properties_map_only_grammar_tools() {
        let mut plain = tool(None);
        plain.name = "plain".into();
        let tools = vec![plain, tool(grammar(Some("start: /x/"), None))];
        let map = create_grammar_tool_input_properties(&tools, true).unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(
            map.get("grammar_tool").map(String::as_str),
            Some("expression")
        );
        // Capability off ⇒ empty map, so callers fall back to normal function tools.
        assert!(
            create_grammar_tool_input_properties(&tools, false)
                .unwrap()
                .is_empty()
        );
    }

    // ---- getGrammarToolInput (constrained-sampling.ts:21-31) ----

    #[test]
    fn grammar_tool_input_requires_a_string_argument() {
        let mut args = Map::new();
        args.insert("expression".into(), json!("1 + 1"));
        assert_eq!(
            get_grammar_tool_input("calc", &args, "expression"),
            Ok("1 + 1".to_string())
        );

        let mut wrong = Map::new();
        wrong.insert("expression".into(), json!(7));
        let expected = err::<String>(
            "Grammar tool call \"calc\" requires argument \"expression\" to be a string.",
        );
        assert_eq!(
            get_grammar_tool_input("calc", &wrong, "expression"),
            expected
        );
        // Absent behaves exactly like the wrong type upstream (`typeof undefined !== "string"`).
        assert_eq!(
            get_grammar_tool_input("calc", &Map::new(), "expression"),
            expected
        );
    }

    // ---- appendGrammarToolInputJsonDelta (constrained-sampling.ts:33-62) ----

    #[test]
    fn grammar_json_deltas_build_a_valid_object() {
        let mut b = GrammarToolInputJsonBuffer::default();
        let mut out = String::new();
        // First chunk opens the object; note pi emits no space after the colon.
        out.push_str(
            &append_grammar_tool_input_json_delta(&mut b, "expression", "a\"b", false)
                .unwrap()
                .expect("first chunk emits"),
        );
        assert_eq!(out, "{\"expression\":\"a\\\"b");
        // An empty growth emits nothing while open.
        assert_eq!(
            append_grammar_tool_input_json_delta(&mut b, "expression", "a\"b", false),
            Ok(None)
        );
        out.push_str(
            &append_grammar_tool_input_json_delta(&mut b, "expression", "a\"b\nc", true)
                .unwrap()
                .expect("close emits"),
        );
        assert_eq!(out, "{\"expression\":\"a\\\"b\\nc\"}");
        let parsed: Value = serde_json::from_str(&out).expect("valid JSON object");
        assert_eq!(parsed["expression"], json!("a\"b\nc"));

        // A repeated close with identical input is a no-op, not an error.
        assert_eq!(
            append_grammar_tool_input_json_delta(&mut b, "expression", "a\"b\nc", true),
            Ok(None)
        );
        // Anything else after close is an error.
        assert_eq!(
            append_grammar_tool_input_json_delta(&mut b, "expression", "a\"b\nc!", true),
            err("grammar tool input for property \"expression\" changed after it was closed")
        );
    }

    #[test]
    fn grammar_json_deltas_reject_non_monotonic_input() {
        let mut b = GrammarToolInputJsonBuffer::default();
        append_grammar_tool_input_json_delta(&mut b, "expression", "abc", false).unwrap();
        assert_eq!(
            append_grammar_tool_input_json_delta(&mut b, "expression", "abX", false),
            err("grammar tool input for property \"expression\" changed non-monotonically")
        );
    }

    #[test]
    fn empty_grammar_input_still_closes_with_an_empty_string() {
        // `close` with nothing buffered must emit the whole `{"p":""}` object — the `inputDelta
        // .length === 0` early return is gated on `!close`.
        let mut b = GrammarToolInputJsonBuffer::default();
        assert_eq!(
            append_grammar_tool_input_json_delta(&mut b, "p", "", true),
            Ok(Some("{\"p\":\"\"}".to_string()))
        );
    }
    // ---------------------------------------------------------------------
    // Strict JSON-schema conversion (pi `7915cdac`, `constrained-sampling.ts:10-131` @v0.84.2)
    // ---------------------------------------------------------------------

    /// A tool whose parameters are `params` and which opted into JSON-schema strict sampling.
    fn schema_tool(name: &str, params: Value, strict: StrictSampling) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: "d".into(),
            parameters: params,
            constrained_sampling: json_schema(strict),
        }
    }

    /// `read`'s real schema (`cyrup-tools/src/tools/read.rs`): one required key, two optional
    /// numbers, no `additionalProperties`. Every provider that is told `strict: true` rejects that
    /// as-is, which is exactly the breakage the conversion exists to prevent.
    fn read_parameters() -> Value {
        json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "Path to the file to read (relative or absolute)" },
                "offset": { "type": "number", "description": "Line number to start reading from (1-indexed)" },
                "limit": { "type": "number", "description": "Maximum number of lines to read" }
            }
        })
    }

    /// DoD 4 — every key of `properties` lands in `required`, `additionalProperties` becomes
    /// `false`, and each previously-optional property is wrapped in `anyOf: [<original>, null]`.
    #[test]
    fn strict_conversion_requires_every_key_and_makes_optionals_nullable() {
        let converted = make_strict_json_schema(&read_parameters()).unwrap();
        // `serde_json::Map` is a `BTreeMap` in this workspace (no `preserve_order`), so
        // `Object::keys()` — and therefore the rewritten `required` — is alphabetical. Ordering is
        // immaterial: `required` is a SET, and every schema cyrup emits already has alphabetical
        // keys for the same reason.
        assert_eq!(converted["required"], json!(["limit", "offset", "path"]));
        assert_eq!(converted["additionalProperties"], json!(false));
        // The required property keeps its own schema verbatim.
        assert_eq!(
            converted["properties"]["path"],
            json!({ "type": "string", "description": "Path to the file to read (relative or absolute)" })
        );
        for optional in ["offset", "limit"] {
            assert_eq!(
                converted["properties"][optional],
                json!({
                    "anyOf": [
                        { "type": "number", "description": if optional == "offset" {
                            "Line number to start reading from (1-indexed)"
                        } else {
                            "Maximum number of lines to read"
                        } },
                        { "type": "null" }
                    ]
                }),
                "{optional} must become present-and-nullable"
            );
        }
    }

    /// DoD 4 — `edit`'s array-of-objects schema is converted at BOTH levels, and gains nothing but
    /// `additionalProperties: false` because every property is already required.
    #[test]
    fn strict_conversion_recurses_through_array_items() {
        let edit_parameters = json!({
            "type": "object",
            "required": ["path", "edits"],
            "properties": {
                "path": { "type": "string" },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "required": ["oldText", "newText"],
                        "properties": {
                            "oldText": { "type": "string" },
                            "newText": { "type": "string" }
                        }
                    }
                }
            }
        });
        let converted = make_strict_json_schema(&edit_parameters).unwrap();
        assert_eq!(converted["required"], json!(["edits", "path"]));
        assert_eq!(converted["additionalProperties"], json!(false));
        // No `anyOf` wrapping anywhere: nothing was optional.
        assert_eq!(converted["properties"]["path"], json!({ "type": "string" }));
        let items = &converted["properties"]["edits"]["items"];
        assert_eq!(items["required"], json!(["newText", "oldText"]));
        assert_eq!(items["additionalProperties"], json!(false));
        assert_eq!(items["properties"]["oldText"], json!({ "type": "string" }));
    }

    /// A property that ALREADY admits `null` is left alone (pi `schemaAllowsNull`).
    #[test]
    fn strict_conversion_does_not_double_wrap_a_nullable_optional() {
        let converted = make_strict_json_schema(&json!({
            "type": "object",
            "properties": {
                "a": { "type": ["string", "null"] },
                "b": { "anyOf": [{ "type": "string" }, { "type": "null" }] },
                "c": { "enum": ["x", null] }
            }
        }))
        .unwrap();
        assert_eq!(
            converted["properties"]["a"],
            json!({ "type": ["string", "null"] })
        );
        assert_eq!(
            converted["properties"]["b"],
            json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
        );
        assert_eq!(converted["properties"]["c"], json!({ "enum": ["x", null] }));
        assert_eq!(converted["required"], json!(["a", "b", "c"]));
    }

    /// `structuredClone` parity — the caller's schema is never mutated.
    #[test]
    fn strict_conversion_leaves_the_callers_schema_untouched() {
        let original = read_parameters();
        let _ = make_strict_json_schema(&original).unwrap();
        assert_eq!(original, read_parameters());
    }

    /// Pi's `UnsupportedStrictJsonSchemaError` messages, verbatim.
    #[test]
    fn strict_conversion_rejects_everything_outside_the_strict_subset() {
        let cases: [(Value, &str); 8] = [
            (json!(true), "root schema must have type object"),
            (
                json!({ "type": "string" }),
                "root schema must have type object",
            ),
            (
                json!({ "type": "object", "$ref": "#/$defs/x" }),
                "$ref schemas are unsupported",
            ),
            (
                json!({ "type": "object", "oneOf": [{ "type": "object" }] }),
                "oneOf schemas are unsupported",
            ),
            (
                json!({ "type": "object", "properties": { "a": { "anyOf": [] } } }),
                "anyOf must contain at least one schema",
            ),
            (
                json!({ "type": "object", "properties": { "a": { "anyOf": [{ "type": "object" }] } } }),
                "object and array unions are unsupported",
            ),
            (
                json!({ "type": "object", "properties": { "a": { "type": "array", "items": [{ "type": "string" }] } } }),
                "tuple schemas are unsupported",
            ),
            (
                json!({ "type": "object", "additionalProperties": true }),
                "schema-valued or true additionalProperties is unsupported",
            ),
        ];
        for (schema, message) in cases {
            let e = make_strict_json_schema(&schema).unwrap_err();
            assert_eq!(e.0, message, "for schema {schema}");
        }
    }

    /// DoD 7 — a schema that cannot convert degrades to `None` under `prefer` on a strict-capable
    /// route, and fails the request under `require` carrying the conversion's own reason.
    #[test]
    fn an_unconvertible_schema_degrades_under_prefer_and_fails_under_require() {
        let params = json!({ "type": "object", "$ref": "#/$defs/x" });
        let prefer = schema_tool("weird", params.clone(), StrictSampling::Prefer);
        assert_eq!(resolve_json_schema_strict_sampling(&prefer, true), Ok(None));

        let require = schema_tool("weird", params, StrictSampling::Require);
        assert_eq!(
            resolve_json_schema_strict_sampling(&require, true)
                .unwrap_err()
                .0,
            "Tool \"weird\" requires JSON-schema constrained sampling, but $ref schemas are unsupported."
        );
    }

    /// DoD 5 — a route without strict mode gets `None` and the RAW schema; `prefer` never fails.
    #[test]
    fn a_non_strict_route_keeps_the_raw_schema_and_does_not_fail() {
        let tool = schema_tool("read", read_parameters(), StrictSampling::Prefer);
        let strict = resolve_json_schema_strict_sampling(&tool, false).unwrap();
        assert_eq!(strict, None);
        assert_eq!(
            json_schema_tool_parameters(&tool, strict == Some(true)).unwrap(),
            read_parameters()
        );
    }

    /// A convertible schema on a strict-capable route resolves to `true` and serializes converted.
    #[test]
    fn a_strict_capable_route_resolves_true_and_serializes_the_converted_schema() {
        let tool = schema_tool("read", read_parameters(), StrictSampling::Prefer);
        let strict = resolve_json_schema_strict_sampling(&tool, true).unwrap();
        assert_eq!(strict, Some(true));
        assert_eq!(
            json_schema_tool_parameters(&tool, strict == Some(true)).unwrap(),
            make_strict_json_schema(&read_parameters()).unwrap()
        );
    }

    /// DoD 1 — a tool that never opted in is untouched on every route.
    #[test]
    fn a_tool_that_did_not_opt_in_is_never_converted() {
        let mut tool = schema_tool("read", read_parameters(), StrictSampling::Prefer);
        tool.constrained_sampling = None;
        for supports in [false, true] {
            let strict = resolve_json_schema_strict_sampling(&tool, supports).unwrap();
            assert_eq!(strict, None);
            assert_eq!(
                json_schema_tool_parameters(&tool, strict == Some(true)).unwrap(),
                read_parameters()
            );
        }
    }
}
