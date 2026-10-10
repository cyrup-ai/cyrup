//! `ToolMetadata`, the two readers that are shaped around it, and the proxy's view of the
//! tool-name grammar.
//!
//! **The move this module's header used to predict has happened.** The naming grammar it carried
//! was a second implementation of what [`crate::registration`] already owned, and the two had
//! drifted; it is now a `pub use` block (MCP-073/MCP-075/MCP-076, see the note below it), so
//! `crate::proxy::format_tool_name` and its siblings resolve to the one definition in
//! `registration.rs`. What remains here is genuinely 13e's and genuinely local: the
//! [`ToolMetadata`] shape and the two functions that read it.
//!
//! **The instruction for what is left still stands.** [`ToolMetadata`],
//! [`is_ui_tool_visible_to_model`] and [`find_tool_by_name`] are `types.ts`' and are owned by
//! section 13e (MCP-200…MCP-207), which lands them in [`crate::renderers`]. They are ported here
//! byte-faithfully so 13d compiles and is testable standalone; when 13e lands, delete them and
//! replace them with `pub use crate::renderers::{…};` alongside the block below. The shapes are
//! upstream's, so that swap is a delete, not a rewrite — the same move the naming grammar has
//! already made.
//!
//! See [`crate::proxy`] for the module overview.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ==================================================================================================
// 2 · `ToolMetadata` and the tool-name grammar
// ==================================================================================================

/// `types.ts:575` `ToolMetadata` — one model-visible MCP tool or resource tool.
///
/// `uiResourceUri` and `uiStreamMode` are **Cut 2** (MCP Apps); `uiVisibility` survives the cut
/// because `buildProxyDescription`'s counts use it to hide tools the server explicitly marked
/// app-only (13d §2, MCP-208).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolMetadata {
    /// Prefixed, model-visible name (e.g. `xcodebuild_list_sims`).
    pub name: String,
    /// The name the MCP server knows (e.g. `list_sims`) — what `tools/call` is sent.
    pub original_name: String,
    /// `tool.description ?? ""`.
    pub description: String,
    /// Resource tools only: the URI `resources/read` is issued against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    /// `_meta.ui.visibility`, when the server declared one. `None` == visible to the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_visibility: Option<Vec<String>>,
    /// The raw JSON Schema, stored for `describe` and for the `Expected parameters:` error suffix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// `types.ts:756` — the behaviour hints the server declared on this tool (MCP-601).
    ///
    /// Read **only where a decision is made**: `mcp({ describe })`'s `Hints:` line and the approval
    /// prompt's destructive / read-only sentence. Search results and direct-tool descriptions are
    /// deliberately unchanged upstream, and porting the hints into those would be a divergence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<McpToolAnnotations>,
}

impl ToolMetadata {
    /// A plain tool, for tests and for callers that only need the three required fields.
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        original_name: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            original_name: original_name.into(),
            description: description.into(),
            resource_uri: None,
            ui_visibility: None,
            input_schema: None,
            annotations: None,
        }
    }
}

/// `types.ts:752` `McpToolAnnotations` — "Hints, not guarantees."
///
/// The four booleans are the spec's `readOnlyHint` / `destructiveHint` / `idempotentHint` /
/// `openWorldHint` and `title` is the server's display name. Every member is optional and
/// **absent is not false**: upstream's renderer prints a hint only for a member that is present,
/// and the approval prompt's warning fires on `=== true`, never on "not false".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolAnnotations {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

impl McpToolAnnotations {
    /// `Object.keys(kept).length > 0` — upstream returns `undefined` rather than an empty object,
    /// and the difference is observable: `if (!annotations) return ""` in the describe renderer.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// The four boolean hints as the host's [`cyrup_core::ToolAnnotations`] (pi `ToolAnnotations`,
    /// `core/extensions/types.ts:512-523` @v1.0.4), without `title`: pi-mcp-adapter drops it
    /// (`const { title: _title, ...annotations } = tool?.annotations`, `deferredToolFields`,
    /// `index.ts:441` @v5.0.0) and the built-in MCP extension keeps only the booleans
    /// (`toToolAnnotations`, `extensions/mcp/tools.ts` @v1.0.4). `None` when none is set, which is
    /// pi's omitted field.
    #[must_use]
    pub fn to_tool_annotations(&self) -> Option<cyrup_core::ToolAnnotations> {
        let annotations = cyrup_core::ToolAnnotations {
            read_only_hint: self.read_only_hint,
            destructive_hint: self.destructive_hint,
            idempotent_hint: self.idempotent_hint,
            open_world_hint: self.open_world_hint,
        };
        (!annotations.is_empty()).then_some(annotations)
    }

    /// `Object.keys(kept).length > 0 ? kept : undefined`.
    #[must_use]
    pub fn non_empty(self) -> Option<Self> {
        if self.is_empty() { None } else { Some(self) }
    }

    /// The hints a **live** `tools/list` answer carried, out of rmcp's typed
    /// [`rmcp::model::ToolAnnotations`].
    ///
    /// No type filtering is needed on this path and none is done: rmcp has already rejected a
    /// non-boolean hint at the wire. [`extract_tool_annotations`] is the untrusted-input half, for
    /// the cache file.
    #[must_use]
    pub fn from_wire(annotations: &rmcp::model::ToolAnnotations) -> Option<Self> {
        Self {
            title: annotations.title.clone(),
            read_only_hint: annotations.read_only_hint,
            destructive_hint: annotations.destructive_hint,
            idempotent_hint: annotations.idempotent_hint,
            open_world_hint: annotations.open_world_hint,
        }
        .non_empty()
    }
}

/// `utils.ts:500` `extractToolAnnotations(annotations: unknown)` (MCP-601).
///
/// Upstream's doc comment gives the reason this is type-filtered rather than deserialised:
/// "Server and cache input is untrusted, so a malformed field is dropped instead of failing the
/// tool list." `mcp-cache.json` is a cross-product file a co-installed `pi-mcp-adapter` writes
/// too, so `{"readOnlyHint": "yes"}` must cost that one hint — not the entry, and not the file.
#[must_use]
pub fn extract_tool_annotations(annotations: Option<&Value>) -> Option<McpToolAnnotations> {
    // `!annotations || typeof annotations !== "object"` — a JSON array is `typeof "object"` in JS,
    // but every member read below is a key lookup, which yields `undefined` on an array, so an
    // array lands on the same empty result as `{}`.
    let source = annotations?.as_object()?;
    let boolean = |key: &str| source.get(key).and_then(Value::as_bool);
    McpToolAnnotations {
        title: source
            .get("title")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        read_only_hint: boolean("readOnlyHint"),
        destructive_hint: boolean("destructiveHint"),
        idempotent_hint: boolean("idempotentHint"),
        open_world_hint: boolean("openWorldHint"),
    }
    .non_empty()
}

/// `proxy-modes.ts:709` `formatToolHints(toolMeta)` (MCP-601) — the `describe` `Hints:` line.
///
/// Each of the four booleans renders its positive or its negative word when **present**, and a
/// `title` renders only when it differs from both names the tool is known by. The title is
/// `JSON.stringify`-escaped, which is why it goes through [`serde_json::to_string`] rather than
/// `format!("{title:?}")`: Rust's `Debug` for `str` is not JSON (`\u{7f}` against `\u007f`).
#[must_use]
pub fn format_tool_hints(tool: &ToolMetadata) -> String {
    let Some(annotations) = tool.annotations.as_ref() else {
        return String::new();
    };
    let mut hints: Vec<String> = Vec::new();
    let mut flag = |value: Option<bool>, yes: &str, no: &str| {
        if let Some(value) = value {
            hints.push(if value { yes } else { no }.to_string());
        }
    };
    flag(annotations.read_only_hint, "read-only", "not read-only");
    flag(
        annotations.destructive_hint,
        "destructive",
        "non-destructive",
    );
    flag(annotations.idempotent_hint, "idempotent", "not idempotent");
    flag(annotations.open_world_hint, "open-world", "closed-world");
    if let Some(title) = annotations.title.as_deref().map(str::trim).filter(|title| {
        !title.is_empty() && *title != tool.original_name.as_str() && *title != tool.name.as_str()
    }) {
        let quoted = serde_json::to_string(title).unwrap_or_else(|_| format!("\"{title}\""));
        hints.push(format!("title {quoted}"));
    }
    hints.join(", ")
}

/// `ui-tool-visibility.ts` `isUiToolVisibleToModel(v)` = `v === undefined || v.includes("model")`.
///
/// **Kept from the cut file** (13d §2): dropping it would expose to the model tools the server
/// explicitly marked app-only.
#[must_use]
pub fn is_ui_tool_visible_to_model(visibility: Option<&[String]>) -> bool {
    match visibility {
        None => true,
        Some(list) => list.iter().any(|entry| entry == "model"),
    }
}

// **De-duplicated (MCP-073/MCP-075/MCP-076).** The tool-name grammar had two implementations in
// this crate and they had drifted: the copy that stood here re-escaped an already-escaped prefix in
// `formatLegacyToolName` (`mcp__github` → `mcp_5f__5f_github`), so every legacy `excludeTools` /
// `approveTools` / `searchKeywords` selector under `ToolPrefix::Mcp` — and every selector at all for
// a server whose name carries a character outside `[A-Za-z0-9_-]` — silently failed to match, and
// its `globToRegExp` compiled config-supplied patterns without a size ceiling.
// [`crate::registration`] is the surviving grammar; it is `types.ts` verbatim, its
// `ToolSelectorCandidateIndex` port memoises what the copy recomputed per (tool, pattern) pair, and
// it carries the inverse function `resolveServerFromToolName` the copy never had.
//
// Two earlier de-duplications are folded into the same list: `resourceNameToToolName` (MCP-203),
// whose copy yielded `"resource_"` where upstream yields `"resource"` for an all-punctuation name,
// and `truncateAtWord` (MCP-206), whose copy counted Unicode scalar values where JS `.length`
// counts UTF-16 code units.
pub use crate::registration::{
    CandidateIndex, format_tool_name, is_tool_allowed, matches_tool_pattern,
    resolve_server_from_tool_name, resolve_tool_prefix, resource_name_to_tool_name,
    sanitize_server_prefix, server_prefix, tool_name_candidates, truncate_at_word,
};

/// `tool-metadata.ts:154` `findToolByName(metadata, toolName)` — exact `name` match first,
/// otherwise compare with `-` globally replaced by `_` on **both** sides.
#[must_use]
pub fn find_tool_by_name<'a>(
    metadata: &'a [ToolMetadata],
    tool_name: &str,
) -> Option<&'a ToolMetadata> {
    if let Some(exact) = metadata.iter().find(|tool| tool.name == tool_name) {
        return Some(exact);
    }
    let normalized = tool_name.replace('-', "_");
    metadata
        .iter()
        .find(|tool| tool.name.replace('-', "_") == normalized)
}

// ==================================================================================================
// 3 · `formatSchema` (MCP-211)
// ==================================================================================================

/// JavaScript truthiness of a JSON value: what `if (schema.type)` asks.
fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0 && !n.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// `String(value)` for a JSON value: an array joins its members with `,` (a `null` member is
/// empty), an object is `[object Object]`.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number
            .as_f64()
            .map_or_else(|| number.to_string(), cyrup_codemode::js::number_to_string),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_owned(),
    }
}

/// The strings of `schema.required`, which is ignored unless it is an array.
fn required_names(schema: &serde_json::Map<String, Value>) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// `tool-metadata.ts:168` `formatSchema(schema, indent = "  ")` (MCP-211): a JSON Schema as the
/// indented parameter list `mcp({ describe })` and `mcp({ search })` show a model and a failed call
/// ends with (`Expected parameters:`). Model-facing text the model uses to correct a bad call, so it
/// is upstream's byte for byte, including the cases a JavaScript reader takes for granted: members
/// come in `Object.entries` order, a value shown with `JSON.stringify` is shown as that writes it
/// (`1.0` is `1`), and `schema.type` is tested for truthiness (`""` is no type).
///
/// A schema that is not an object is `(no schema)`; an object schema without properties is
/// `(no parameters)`; a schema with nothing to list is its type, or `(complex schema)`.
#[must_use]
pub fn format_schema(schema: &Value, indent: &str) -> String {
    let Some(schema) = schema.as_object() else {
        return format!("{indent}(no schema)");
    };

    if schema.get("type").and_then(Value::as_str) == Some("object")
        && let Some(properties) = schema.get("properties").and_then(Value::as_object)
    {
        if properties.is_empty() {
            return format!("{indent}(no parameters)");
        }
        let required = required_names(schema);
        let mut lines = Vec::new();
        for name in cyrup_codemode::js::own_keys(properties) {
            if let Some(property) = properties.get(name) {
                lines.extend(format_property(
                    name,
                    property,
                    required.contains(&name),
                    indent,
                ));
            }
        }
        return lines.join("\n");
    }

    let lines = format_nested_schema(schema, indent);
    if !lines.is_empty() {
        return lines.join("\n");
    }

    let type_str = format_type(schema);
    if !type_str.is_empty() {
        return format!("{indent}({type_str})");
    }

    format!("{indent}(complex schema)")
}

/// `formatProperty(name, schema, required, indent)`: the property's own line, then whatever nests
/// under it two columns further in.
fn format_property(name: &str, schema: &Value, required: bool, indent: &str) -> Vec<String> {
    let Some(schema) = schema.as_object() else {
        let marker = if required { " *required*" } else { "" };
        return vec![format!("{indent}{name}{marker}")];
    };

    let mut parts = vec![format!("{indent}{name}")];
    let type_str = format_type(schema);
    if !type_str.is_empty() {
        parts.push(format!("({type_str})"));
    }
    if required {
        parts.push("*required*".to_owned());
    }
    append_schema_annotations(&mut parts, schema);

    let mut lines = vec![parts.join(" ")];
    lines.extend(format_nested_schema(schema, &format!("{indent}  ")));
    lines
}

/// `formatNestedSchema(schema, indent)`: `anyOf`, `oneOf`, `items`, then `properties`, each only
/// when the schema has it.
fn format_nested_schema(schema: &serde_json::Map<String, Value>, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(variants) = schema.get("anyOf").and_then(Value::as_array) {
        lines.extend(format_variants("anyOf", variants, indent));
    }
    if let Some(variants) = schema.get("oneOf").and_then(Value::as_array) {
        lines.extend(format_variants("oneOf", variants, indent));
    }
    // `schema.items !== undefined`: a present `null` counts, and is listed as a bare `items`.
    if let Some(items) = schema.get("items") {
        lines.extend(format_property("items", items, false, indent));
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let required = required_names(schema);
        for name in cyrup_codemode::js::own_keys(properties) {
            if let Some(property) = properties.get(name) {
                lines.extend(format_property(
                    name,
                    property,
                    required.contains(&name),
                    indent,
                ));
            }
        }
    }

    lines
}

/// `formatVariants(keyword, variants, indent)`: the keyword, then one `- type [annotations]` line
/// per variant with the variant's own nested lines four columns in. A variant that is not an object
/// is shown as its JSON.
fn format_variants(keyword: &str, variants: &[Value], indent: &str) -> Vec<String> {
    let mut lines = vec![format!("{indent}{keyword}:")];

    for variant in variants {
        let Some(schema) = variant.as_object() else {
            lines.push(format!(
                "{indent}  - {}",
                cyrup_codemode::js::json_stringify(variant)
            ));
            continue;
        };
        let type_str = format_type(schema);
        let type_str = if type_str.is_empty() {
            "schema"
        } else {
            &type_str
        };
        let mut parts = vec![format!("{indent}  - {type_str}")];
        append_schema_annotations(&mut parts, schema);
        lines.push(parts.join(" "));
        lines.extend(format_nested_schema(schema, &format!("{indent}    ")));
    }

    lines
}

/// `formatType(schema)`: the first of `const`, `enum`, `type` (an array joins with ` | `), an
/// implied `object` and an implied `array` that the schema has; empty when none applies.
fn format_type(schema: &serde_json::Map<String, Value>) -> String {
    // `Object.hasOwn(schema, "const")`: `const: null` is a constant, an absent `const` is not.
    if let Some(constant) = schema.get("const") {
        return format!("const {}", cyrup_codemode::js::json_stringify(constant));
    }

    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let values: Vec<String> = values
            .iter()
            .map(cyrup_codemode::js::json_stringify)
            .collect();
        return format!("enum: {}", values.join(", "));
    }

    match schema.get("type") {
        Some(Value::Array(types)) => {
            let types: Vec<String> = types.iter().map(js_string).collect();
            return types.join(" | ");
        }
        Some(declared) if js_truthy(declared) => return js_string(declared),
        _ => {}
    }

    if schema.get("properties").is_some_and(Value::is_object) {
        return "object".to_owned();
    }

    if schema.contains_key("items") {
        return "array".to_owned();
    }

    String::new()
}

/// `appendSchemaAnnotations(parts, schema)`: ` - description`, then `[key: value]` for each bound
/// the schema sets, in upstream's fixed order, then `[default: value]`.
fn append_schema_annotations(parts: &mut Vec<String>, schema: &serde_json::Map<String, Value>) {
    if let Some(description) = schema.get("description").and_then(Value::as_str)
        && !description.is_empty()
    {
        parts.push(format!("- {description}"));
    }

    for key in [
        "minLength",
        "maxLength",
        "minimum",
        "maximum",
        "minItems",
        "maxItems",
        "format",
        "pattern",
    ] {
        if let Some(value) = schema.get(key) {
            parts.push(format!(
                "[{key}: {}]",
                cyrup_codemode::js::json_stringify(value)
            ));
        }
    }

    if let Some(default) = schema.get("default") {
        parts.push(format!(
            "[default: {}]",
            cyrup_codemode::js::json_stringify(default)
        ));
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- MCP-601 · `extractToolAnnotations` ---------------------------------------------------

    /// `utils.ts:500` — "Server and cache input is untrusted, so a malformed field is dropped
    /// instead of failing the tool list." One malformed member must cost that member only.
    #[test]
    fn a_malformed_annotation_member_is_dropped_and_the_rest_kept() {
        let kept = extract_tool_annotations(Some(&json!({
            "title": "List simulators",
            "readOnlyHint": true,
            "destructiveHint": "yes",
            "idempotentHint": 1,
            "openWorldHint": false,
            "unknownHint": true,
        })))
        .expect("the well-typed members survive");
        assert_eq!(kept.title.as_deref(), Some("List simulators"));
        assert_eq!(kept.read_only_hint, Some(true));
        assert_eq!(
            kept.destructive_hint, None,
            "a string is not a boolean hint"
        );
        assert_eq!(kept.idempotent_hint, None, "a number is not a boolean hint");
        assert_eq!(kept.open_world_hint, Some(false));
    }

    /// `Object.keys(kept).length > 0 ? kept : undefined` — the difference is observable, because
    /// the describe renderer's first line is `if (!annotations) return ""`.
    #[test]
    fn nothing_survivable_yields_none_rather_than_an_empty_object() {
        for input in [
            json!({}),
            json!({ "title": 7, "readOnlyHint": "true" }),
            json!("destructive"),
            json!(null),
        ] {
            assert_eq!(
                extract_tool_annotations(Some(&input)),
                None,
                "{input} must yield `undefined`, not an empty object"
            );
        }
        assert_eq!(extract_tool_annotations(None), None);
    }

    // ---- MCP-601 · `formatToolHints` ----------------------------------------------------------

    /// `proxy-modes.ts:709-723`. Each boolean renders its positive or its negative word when
    /// **present** — absent is not false — and the order is read-only, destructive, idempotent,
    /// open-world, title.
    #[test]
    fn the_describe_hints_render_present_members_in_upstream_order() {
        let mut tool = ToolMetadata::new("srv_wipe", "wipe", "");
        assert_eq!(
            format_tool_hints(&tool),
            "",
            "no annotations means no hints line at all"
        );

        tool.annotations = Some(McpToolAnnotations {
            read_only_hint: Some(false),
            destructive_hint: Some(true),
            open_world_hint: Some(false),
            ..McpToolAnnotations::default()
        });
        assert_eq!(
            format_tool_hints(&tool),
            "not read-only, destructive, closed-world",
            "an absent `idempotentHint` contributes nothing"
        );

        // A title renders only when it differs from both names, and is JSON-escaped.
        tool.annotations = Some(McpToolAnnotations {
            title: Some("wipe".to_string()),
            ..McpToolAnnotations::default()
        });
        assert_eq!(format_tool_hints(&tool), "");
        tool.annotations = Some(McpToolAnnotations {
            title: Some("  Wipe \"everything\"  ".to_string()),
            ..McpToolAnnotations::default()
        });
        assert_eq!(
            format_tool_hints(&tool),
            "title \"Wipe \\\"everything\\\"\"",
            "the server's title is escaped before it reaches the model"
        );
    }

    /// The live half: rmcp's typed hints, with upstream's "empty means absent" rule.
    #[test]
    fn wire_annotations_keep_their_values_and_collapse_when_empty() {
        let wire = rmcp::model::ToolAnnotations::new();
        assert_eq!(
            McpToolAnnotations::from_wire(&wire),
            None,
            "an all-absent `annotations` object is `undefined`"
        );
        let wire = wire.destructive(true);
        assert_eq!(
            McpToolAnnotations::from_wire(&wire),
            Some(McpToolAnnotations {
                destructive_hint: Some(true),
                ..McpToolAnnotations::default()
            })
        );
    }
    // ---- MCP-211 · `formatSchema` ---------------------------------------------------------------

    /// One row of `testdata/format_schema_upstream.json`.
    #[derive(serde::Deserialize)]
    struct GoldenCase {
        name: String,
        /// The schema as JSON text, so `1.0` and `1e21` reach the formatter as written.
        schema: String,
        indent: Option<String>,
        expected: String,
    }

    /// `expected` of every row is what upstream's own `formatSchema` returned for the schema
    /// (`tool-metadata.ts:168` of `pi-mcp-adapter` v5.0.0, the function's text run under bun, not
    /// transcribed), so this is a comparison against the original and not against this port's idea
    /// of it. The rows cover a nested `anyOf` + `items` + `enum` + `const: null` schema in both
    /// indents, every `typeof`/truthiness fork the TypeScript has (`type: ""`, `type: 0`, a `type`
    /// list of mixed values, `required` that is not a list of strings, a `null` item schema),
    /// `Object.entries` order for integer-like names, and the way `JSON.stringify` writes numbers,
    /// escapes and object members.
    #[test]
    fn format_schema_matches_upstreams_output_byte_for_byte() {
        let table: Vec<GoldenCase> =
            serde_json::from_str(include_str!("../../testdata/format_schema_upstream.json"))
                .unwrap();
        assert!(
            table.len() >= 30,
            "the table was cut short: {}",
            table.len()
        );
        for case in &table {
            let schema: Value = serde_json::from_str(&case.schema).unwrap();
            let indent = case.indent.as_deref().unwrap_or("  ");
            assert_eq!(
                format_schema(&schema, indent),
                case.expected,
                "{}: {}",
                case.name,
                case.schema
            );
        }
    }

    /// The case the ledger asks for by name, spelled out so a reader sees the shape rather than
    /// trusting a table.
    #[test]
    fn a_nested_schema_reads_as_an_indented_parameter_list() {
        let schema = json!({
            "type": "object",
            "required": ["target"],
            "properties": {
                "target": {
                    "anyOf": [
                        { "type": "string", "description": "a name" },
                        { "const": null }
                    ]
                },
                "mode": { "enum": ["fast", null], "default": "fast" },
                "tags": { "type": "array", "items": { "type": "string" }, "minItems": 1 }
            }
        });
        assert_eq!(
            format_schema(&schema, "  "),
            "  target *required*\n    anyOf:\n      - string - a name\n      - const null\n  mode (enum: \"fast\", null) [default: \"fast\"]\n  tags (array) [minItems: 1]\n    items (string)"
        );
        assert_eq!(format_schema(&json!(null), "  "), "  (no schema)");
        assert_eq!(
            format_schema(&json!({ "type": "object", "properties": {} }), "  "),
            "  (no parameters)"
        );
        assert_eq!(format_schema(&json!({}), "  "), "  (complex schema)");
    }
}
