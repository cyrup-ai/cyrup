//! TypeScript declarations for the script-visible API (pi `packages/codemode/src/declarations.ts`
//! @v1.0.1, CODE-004).
//!
//! Tools become members of `declare const tools`, globals become `declare function` statements,
//! and `ns.member` globals members of `declare const ns`. Descriptions become doc comments and JSON
//! Schemas become types. The output is byte-identical to upstream's for the same schemas: the
//! property sort, the `JSON.stringify` of `const`/`enum` values, the whitespace `trim`, the
//! character budget and the line splitting all follow JavaScript's rules through [`crate::js`].
//!
//! # Production call path
//!
//! * [`render_tool_sample`] builds the `ALL_TOOLS` entry and `describeTool()` text for every nested
//!   tool, and [`render_declarations`] the `declare const tools` block of the model-facing
//!   `codemode` description (pi `execute.ts:347`, `tool.ts` `createCodemodeDescription`).
//! * [`MCP_TYPESCRIPT_PREAMBLE`] is emitted once ahead of those declarations when any listed tool's
//!   output schema is an MCP result ([`mcp_structured_content_schema`]).
//!
//! # `$ref` and the two budgets
//!
//! A local reference (`#`, `#/$defs/...`, `#/definitions/...`) resolves against the schema being
//! rendered. Recursive references and remote references render `unknown`, a schema may expand at
//! most [`MAX_REF_EXPANSIONS`] references, and an input type longer than
//! [`DEFAULT_INPUT_SCHEMA_MAX_CHARS`] UTF-16 units degrades to `unknown`. These bound what a hostile
//! MCP server's schema can add to a prompt.
//!
//! `[CYRUP-DELTA]` A `$ref` whose percent-escapes are malformed (`#/$defs/%E0%A4%A`) makes
//! upstream's `decodeURIComponent` throw `URIError`, and the throw escapes `renderDeclarations`.
//! Here the reference renders `unknown`, like any other reference that does not resolve. Full
//! feature parity: every schema upstream renders, this renders identically.

use serde_json::{Map, Value};

use crate::identifier::{is_identifier, to_codemode_identifier};
use crate::js::{
    decode_uri_component, js_string_cmp, js_trim, json_stringify, split_lines, utf16_len,
};
use crate::types::ToolDeclaration;

const INDENT: &str = "  ";
/// Largest rendered input type, in characters, before it becomes `unknown`
/// (`declarations.ts:10`).
pub const DEFAULT_INPUT_SCHEMA_MAX_CHARS: usize = 16_000;
/// Local `$ref` expansions per rendered schema, so shared definitions cannot blow up the output
/// (`declarations.ts:12`).
pub const MAX_REF_EXPANSIONS: usize = 32;

/// TypeScript types for MCP results, from the MCP `CallToolResult` schema, so `CallToolResult<T>`
/// declarations can refer to them (`declarations.ts:18-93`).
pub const MCP_TYPESCRIPT_PREAMBLE: &str = r#"type Role = "user" | "assistant";
type MetaObject = Record<string, unknown>;
type Annotations = {
  audience?: Role[];
  priority?: number;
  lastModified?: string;
};
type Icon = {
  src: string;
  mimeType?: string;
  sizes?: string[];
  theme?: "light" | "dark";
};
type TextResourceContents = {
  uri: string;
  mimeType?: string;
  _meta?: MetaObject;
  text: string;
};
type BlobResourceContents = {
  uri: string;
  mimeType?: string;
  _meta?: MetaObject;
  blob: string;
};
type TextContent = {
  type: "text";
  text: string;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type ImageContent = {
  type: "image";
  data: string;
  mimeType: string;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type AudioContent = {
  type: "audio";
  data: string;
  mimeType: string;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type ResourceLink = {
  icons?: Icon[];
  name: string;
  title?: string;
  uri: string;
  description?: string;
  mimeType?: string;
  annotations?: Annotations;
  size?: number;
  _meta?: MetaObject;
  type: "resource_link";
};
type EmbeddedResource = {
  type: "resource";
  resource: TextResourceContents | BlobResourceContents;
  annotations?: Annotations;
  _meta?: MetaObject;
};
type ContentBlock =
  | TextContent
  | ImageContent
  | AudioContent
  | ResourceLink
  | EmbeddedResource;
type CallToolResult<TStructured = { [key: string]: unknown }> = {
  _meta?: MetaObject;
  content: ContentBlock[];
  isError?: boolean;
  structuredContent?: TStructured;
  [key: string]: unknown;
};"#;

/// Render TypeScript declarations for the script-visible API (`declarations.ts:105-130`): one
/// `declare const tools` block, then each plain global as `declare function`, then one
/// `declare const <namespace>` per `ns.member` group, in order of first appearance. Sections are
/// separated by a blank line.
#[must_use]
pub fn render_declarations(tools: &[ToolDeclaration], globals: &[ToolDeclaration]) -> String {
    let mut sections: Vec<String> = Vec::new();
    if !tools.is_empty() {
        let members: Vec<String> = tools
            .iter()
            .map(|tool| {
                format!(
                    "{}{INDENT}{}",
                    doc_comment(tool.description.as_deref(), INDENT),
                    render_tool_signature(tool, None)
                )
            })
            .collect();
        sections.push(format!(
            "declare const tools: {{\n{}\n}};",
            members.join("\n")
        ));
    }
    let mut namespaces: Vec<(&str, Vec<String>)> = Vec::new();
    for global in globals {
        let Some((namespace, member)) = global.name.split_once('.') else {
            sections.push(render_global(
                &format!("declare function {}", global.name),
                global,
                "",
            ));
            continue;
        };
        let rendered = render_global(member, global, INDENT);
        match namespaces.iter_mut().find(|(name, _)| *name == namespace) {
            Some((_, members)) => members.push(rendered),
            None => namespaces.push((namespace, vec![rendered])),
        }
    }
    for (namespace, members) in namespaces {
        sections.push(format!(
            "declare const {namespace}: {{\n{}\n}};",
            members.join("\n")
        ));
    }
    sections.join("\n\n")
}

/// One tool as a member of the `tools` object: `name(args: T): Promise<R>;`, the name being the
/// identifier scripts use (`declarations.ts:138-147`). An input type longer than `input_max_chars`
/// (default [`DEFAULT_INPUT_SCHEMA_MAX_CHARS`]) renders as `unknown`. A tool whose output schema is
/// an MCP `CallToolResult` renders as `Promise<CallToolResult<T>>`, which needs
/// [`MCP_TYPESCRIPT_PREAMBLE`].
#[must_use]
pub fn render_tool_signature(tool: &ToolDeclaration, input_max_chars: Option<usize>) -> String {
    let input = tool.input_schema.as_ref().map_or_else(
        || "unknown".to_owned(),
        |schema| {
            schema_to_type(
                schema,
                Some(input_max_chars.unwrap_or(DEFAULT_INPUT_SCHEMA_MAX_CHARS)),
            )
        },
    );
    format!(
        "{}(args: {input}): Promise<{}>;",
        to_codemode_identifier(&tool.name),
        render_tool_output_type(tool.output_schema.as_ref())
    )
}

/// A tool's sample: the description followed by the tool's declaration (`declarations.ts:153-159`).
/// Used for tool listings and `ALL_TOOLS` entries.
#[must_use]
pub fn render_tool_sample(tool: &ToolDeclaration, input_max_chars: Option<usize>) -> String {
    let declaration = format!(
        "declare const tools: {{ {} }};",
        render_tool_signature(tool, input_max_chars)
    );
    format!(
        "{}\n\ncodemode tool declaration:\n```ts\n{declaration}\n```",
        tool.description.as_deref().map_or("", js_trim)
    )
}

static TRUE_SCHEMA: Value = Value::Bool(true);

fn as_object(value: Option<&Value>) -> Option<&Map<String, Value>> {
    match value {
        Some(Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn type_is(value: Option<&Value>, expected: &str) -> bool {
    as_object(value)
        .and_then(|map| map.get("type"))
        .is_some_and(|ty| ty.as_str() == Some(expected))
}

/// The `structuredContent` schema of an MCP `CallToolResult` output schema, detected by a `content`
/// array of objects, a boolean `isError` and an object `_meta` (`declarations.ts:166-175`); `true`
/// when it declares none; `None` when the schema is not a `CallToolResult`.
#[must_use]
pub fn mcp_structured_content_schema(schema: Option<&Value>) -> Option<&Value> {
    let properties = as_object(schema)
        .and_then(|map| map.get("properties"))
        .and_then(Value::as_object)?;
    let content = properties.get("content");
    if !type_is(content, "array")
        || !type_is(
            as_object(content).and_then(|map| map.get("items")),
            "object",
        )
    {
        return None;
    }
    if !type_is(properties.get("isError"), "boolean") || !type_is(properties.get("_meta"), "object")
    {
        return None;
    }
    match properties.get("structuredContent") {
        Some(structured @ (Value::Object(_) | Value::Bool(_))) => Some(structured),
        _ => Some(&TRUE_SCHEMA),
    }
}

/// The type a tool call resolves to: `CallToolResult<T>` for an MCP output schema (needs
/// [`MCP_TYPESCRIPT_PREAMBLE`]), the schema's type otherwise, `unknown` without a schema
/// (`declarations.ts:181-188`). Unlike an input type this one has no character budget.
#[must_use]
pub fn render_tool_output_type(schema: Option<&Value>) -> String {
    if let Some(structured) = mcp_structured_content_schema(schema) {
        let ty = schema_to_type(structured, None);
        return if ty == "unknown" {
            "CallToolResult".to_owned()
        } else {
            format!("CallToolResult<{ty}>")
        };
    }
    schema.map_or_else(
        || "unknown".to_owned(),
        |schema| schema_to_type(schema, None),
    )
}

fn render_global(head: &str, global: &ToolDeclaration, indent: &str) -> String {
    let doc = doc_comment(global.description.as_deref(), indent);
    if let Some(signature) = &global.signature {
        return format!("{doc}{indent}{head}{signature};");
    }
    let input = global.input_schema.as_ref().map_or_else(
        || "unknown".to_owned(),
        |schema| schema_to_type(schema, None),
    );
    let output = global.output_schema.as_ref().map_or_else(
        || "unknown".to_owned(),
        |schema| schema_to_type(schema, None),
    );
    format!("{doc}{indent}{head}(args: {input}): Promise<{output}>;")
}

/// A `/** ... */` comment for a description, ending in a newline; empty for a blank description.
/// A `*/` inside the text is escaped so it cannot close the comment (`declarations.ts:199-205`).
fn doc_comment(description: Option<&str>, indent: &str) -> String {
    let text = description.map_or("", js_trim);
    if text.is_empty() {
        return String::new();
    }
    let escaped = text.replace("*/", "*\\/");
    let lines = split_lines(&escaped);
    if let [only] = lines.as_slice() {
        return format!("{indent}/** {only} */\n");
    }
    let body: Vec<String> = lines
        .iter()
        .map(|line| {
            if line.is_empty() {
                format!("{indent} *")
            } else {
                format!("{indent} * {line}")
            }
        })
        .collect();
    format!("{indent}/**\n{}\n{indent} */\n", body.join("\n"))
}

fn property_key(name: &str) -> String {
    if is_identifier(name) {
        name.to_owned()
    } else {
        json_stringify(&Value::String(name.to_owned()))
    }
}

/// `union` (`declarations.ts:215-219`): duplicates dropped in first-seen order, `unknown` absorbs
/// everything, no members is `never`.
fn union(types: Vec<String>) -> String {
    let mut unique: Vec<String> = Vec::new();
    for ty in types {
        if !unique.contains(&ty) {
            unique.push(ty);
        }
    }
    if unique.iter().any(|ty| ty == "unknown") {
        "unknown".to_owned()
    } else if unique.is_empty() {
        "never".to_owned()
    } else {
        unique.join(" | ")
    }
}

/// Convert a JSON Schema to a TypeScript type expression (`declarations.ts:228-231`): objects on
/// one line (`{ a: string; b?: number; }`) with properties sorted by name, or one property per line
/// with `//` comments when a property has a description; `Array<T>` for arrays. A result longer
/// than `max_chars` UTF-16 units renders as `unknown`.
#[must_use]
pub fn schema_to_type(schema: &Value, max_chars: Option<usize>) -> String {
    let mut context = SchemaContext {
        root: schema,
        resolving: Vec::new(),
        expansions: 0,
    };
    let ty = to_type(schema, &mut context);
    match max_chars {
        Some(max) if utf16_len(&ty) > max => "unknown".to_owned(),
        _ => ty,
    }
}

struct SchemaContext<'a> {
    root: &'a Value,
    /// References being expanded on the current path, to stop at recursive types.
    resolving: Vec<&'a str>,
    expansions: usize,
}

/// `resolveRef` (`declarations.ts:240-249`). Only `#` and `#/...` pointers resolve; a segment is
/// percent-decoded, then `~1` becomes `/` and `~0` becomes `~`; empty segments are skipped; the
/// target must be a boolean or an object.
fn resolve_ref<'a>(reference: &str, root: &'a Value) -> Option<&'a Value> {
    if reference != "#" && !reference.starts_with("#/") {
        return None;
    }
    let pointer = reference.get(2..).unwrap_or("");
    let mut current = root;
    for segment in pointer.split('/').filter(|segment| !segment.is_empty()) {
        let key = decode_uri_component(segment)?
            .replace("~1", "/")
            .replace("~0", "~");
        current = current.as_object()?.get(&key)?;
    }
    matches!(current, Value::Bool(_) | Value::Object(_)).then_some(current)
}

fn to_type<'a>(schema: &'a Value, context: &mut SchemaContext<'a>) -> String {
    let map = match schema {
        Value::Bool(true) => return "unknown".to_owned(),
        Value::Bool(false) => return "never".to_owned(),
        Value::Object(map) => map,
        _ => return "unknown".to_owned(),
    };
    if let Some(Value::String(reference)) = map.get("$ref") {
        if context.resolving.contains(&reference.as_str())
            || context.expansions >= MAX_REF_EXPANSIONS
        {
            return "unknown".to_owned();
        }
        let Some(target) = resolve_ref(reference, context.root) else {
            return "unknown".to_owned();
        };
        context.expansions += 1;
        context.resolving.push(reference);
        let ty = to_type(target, context);
        context.resolving.pop();
        return ty;
    }

    if let Some(constant) = map.get("const") {
        return json_stringify(constant);
    }
    if let Some(Value::Array(values)) = map.get("enum") {
        return union(values.iter().map(json_stringify).collect());
    }

    let variants = match (map.get("anyOf"), map.get("oneOf")) {
        (Some(Value::Array(variants)), _) | (_, Some(Value::Array(variants))) => Some(variants),
        _ => None,
    };
    if let Some(variants) = variants {
        return union(
            variants
                .iter()
                .map(|variant| to_type(variant, context))
                .collect(),
        );
    }
    if let Some(Value::Array(parts)) = map.get("allOf") {
        let parts: Vec<String> = parts
            .iter()
            .map(|part| to_type(part, context))
            .filter(|part| part != "unknown")
            .collect();
        return if parts.is_empty() {
            "unknown".to_owned()
        } else {
            parts
                .iter()
                .map(|part| {
                    if part.contains(" | ") {
                        format!("({part})")
                    } else {
                        part.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(" & ")
        };
    }

    type_to_string(map, map.get("type"), context)
}

/// The `type` switch of `toType` (`declarations.ts:283-307`). A `type` array renders each entry as
/// the same schema with that one type and unions the results, which is why `ty` is a parameter and
/// not read from `map` here.
fn type_to_string<'a>(
    map: &'a Map<String, Value>,
    ty: Option<&'a Value>,
    context: &mut SchemaContext<'a>,
) -> String {
    match ty {
        Some(Value::Array(entries)) => union(
            entries
                .iter()
                .map(|entry| type_to_string(map, Some(entry), context))
                .collect(),
        ),
        Some(Value::String(name)) => match name.as_str() {
            "string" => "string".to_owned(),
            "number" | "integer" => "number".to_owned(),
            "boolean" => "boolean".to_owned(),
            "null" => "null".to_owned(),
            "array" => array_type(map, context),
            "object" => object_type(map, context),
            _ => "unknown".to_owned(),
        },
        None => {
            if ["properties", "additionalProperties", "required"]
                .iter()
                .any(|key| map.contains_key(*key))
            {
                object_type(map, context)
            } else if ["items", "prefixItems"]
                .iter()
                .any(|key| map.contains_key(*key))
            {
                array_type(map, context)
            } else {
                "unknown".to_owned()
            }
        }
        Some(_) => "unknown".to_owned(),
    }
}

fn array_type<'a>(map: &'a Map<String, Value>, context: &mut SchemaContext<'a>) -> String {
    let items = map.get("items");
    if let Some(items) = items.filter(|items| !items.is_array()) {
        return format!("Array<{}>", to_type(items, context));
    }
    let tuple = match (map.get("prefixItems"), items) {
        (Some(Value::Array(prefix)), _) => prefix.as_slice(),
        (_, Some(Value::Array(items))) => items.as_slice(),
        _ => &[],
    };
    if tuple.is_empty() {
        return "unknown[]".to_owned();
    }
    let members: Vec<String> = tuple.iter().map(|item| to_type(item, context)).collect();
    format!("[{}]", members.join(", "))
}

fn description_of(property: &Value) -> &str {
    property
        .as_object()
        .and_then(|map| map.get("description"))
        .and_then(Value::as_str)
        .map_or("", js_trim)
}

fn object_type<'a>(map: &'a Map<String, Value>, context: &mut SchemaContext<'a>) -> String {
    let properties = map.get("properties").and_then(Value::as_object);
    let required: Vec<&str> = match map.get("required") {
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let mut names: Vec<&str> = properties
        .map(|properties| properties.keys().map(String::as_str).collect())
        .unwrap_or_default();
    names.sort_by(|a, b| js_string_cmp(a, b));
    let property = |name: &str| properties.and_then(|properties| properties.get(name));
    let mut members: Vec<String> = Vec::with_capacity(names.len());
    for name in &names {
        let optional = if required.contains(name) { "" } else { "?" };
        let ty =
            property(name).map_or_else(|| "unknown".to_owned(), |value| to_type(value, context));
        members.push(format!("{}{optional}: {ty};", property_key(name)));
    }
    match map.get("additionalProperties") {
        None if names.is_empty() => members.push("[key: string]: unknown;".to_owned()),
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => members.push("[key: string]: unknown;".to_owned()),
        Some(additional) => {
            members.push(format!("[key: string]: {};", to_type(additional, context)))
        }
    }
    if members.is_empty() {
        return "{}".to_owned();
    }
    let described = |name: &str| property(name).map_or("", description_of);
    if !names.iter().any(|name| !described(name).is_empty()) {
        return format!("{{ {} }}", members.join(" "));
    }

    let mut lines: Vec<String> = vec!["{".to_owned()];
    for (name, member) in names.iter().zip(&members) {
        for line in split_lines(described(name)) {
            if !js_trim(line).is_empty() {
                lines.push(format!("{INDENT}// {}", js_trim(line)));
            }
        }
        lines.push(format!(
            "{INDENT}{}",
            member.replace('\n', &format!("\n{INDENT}"))
        ));
    }
    for member in members.iter().skip(names.len()) {
        lines.push(format!("{INDENT}{member}"));
    }
    lines.push("}".to_owned());
    lines.join("\n")
}

#[cfg(test)]
mod tests;
