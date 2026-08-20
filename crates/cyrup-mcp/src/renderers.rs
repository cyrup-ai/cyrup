//! Result and call rendering, content-block transformation, and the model-facing output guard.
//!
//! Upstream is three files:
//!
//! | upstream | what lands here |
//! |---|---|
//! | `tool-result-renderer.ts` (463 lines) | the call rows, the collapsed/expanded result rows, the compact-vs-boxed fork (MCP-237..MCP-245) |
//! | `tool-registrar.ts` (`transformMcpContent` and friends, `:95-265`) | every MCP content type collapsed onto `cyrup_core::Content` (MCP-220..MCP-224) |
//! | `mcp-output-guard.ts` (408 lines) | the byte/line cap, the private-directory spill and `details.mcpResult` bounding (MCP-225..MCP-230) |
//!
//! # The seam, and the four things that do not cross it
//!
//! Upstream's renderers are pi `Component`s: `renderCall(args, theme, context)` and
//! `renderResult(result, options, theme, context)` return a live `pi-tui` object that the
//! interactive mode adopts as a child of `ToolExecutionComponent`, and pi re-invokes `render(width)`
//! on it at every repaint. cyrup's seam is
//! [`cyrup_ext::native::NativeExtension::render_call`]/[`render_result`][cyrup_ext::native::NativeExtension::render_result]:
//! `(key, payload) -> Option<serde_json::Value>`, where the `Value` is a **serialized widget tree**
//! that `cyrup_tui::app::extension_render::rendered_text` flattens to a `String` through a fixed
//! vocabulary (bare string, `text`, `markdown`, `truncated-text`, `spacer`, `box`/`container`,
//! `hstack`, bare array). `key` is the registered tool name, declared at `init` by
//! `InitApi::register_tool_renderer` (`crate::extension::McpExtension::init`).
//!
//! Four upstream inputs are absent from that seam and each one is a *named* delta, not a shortcut:
//!
//! 1. **No render width** (MCP-241/MCP-245). `CompactMcpToolResult.render(width)` chooses between a
//!    21-char `" … (Ctrl+O to expand)"`, a 9-char `" (Ctrl+O)"` and a bare truncated hint by
//!    comparing them against `safeWidth`; with no width the port always emits the long form and
//!    hands the host a `truncated-text` node to clip. `CollapsibleText`'s
//!    `charBudget = width * (maxCollapsedLines + 1) * 8` re-collapse is likewise width-free here,
//!    so the collapse is done once, by line count, in [`format_mcp_tool_result_lines`].
//! 2. **No theme** (MCP-244). Every upstream line is `theme.fg("toolTitle"|"toolOutput"|"muted"|"warning", …)`.
//!    `HostServices::theme()` would return the palette, but the widget vocabulary has no styled-span
//!    node to emit it into, so the port draws upstream's own `plainTheme` path — uncoloured, and
//!    therefore free of escape sequences in the flattened string.
//! 3. **No per-row expansion flag and no per-row state** (MCP-242/MCP-243). `options.expanded` is
//!    per row upstream; cyrup's expand toggle is global (`HostServices::tools_expanded()`), so
//!    [`render_result`] takes it as a parameter and ORs it with `details.error` exactly as upstream
//!    does. The `context.state.compactTitle` stash — by which compact mode makes `renderCall` return
//!    an `EmptyComponent` so the call row and the result row collapse into one line — has no cyrup
//!    equivalent at all (`render_call` and `render_result` are separate stateless calls with no
//!    shared row context and no call id, and `None` means "use the default framing", not "draw
//!    nothing"). Per MCP-243 the stash is **dropped**: cyrup draws the two-row shape under both
//!    `compact` and `boxed`, differing only in the collapsed line budget.
//! 4. **No `isPartial`** — `cyrup_tui::app::extension_render` routes only `ToolExecutionStart` and
//!    `ToolExecutionEnd` to a renderer, never `ToolExecutionUpdate`, so upstream's
//!    `"Running MCP tool..."` arm is unreachable from the seam. It is ported anyway, on
//!    [`render_mcp_tool_result`]'s `is_partial` parameter, so the mechanism survives for the day the
//!    host routes updates.
//!
//! Two host-side limits bound what a renderer may legally emit, both in
//! `crates/cyrup-tui/src/app/extension_render.rs`: `MAX_WIDGET_DEPTH = 16` (a deeper tree is not
//! partially drawn — the WHOLE tree falls back to pretty-printed JSON) and
//! `EXTENSION_RENDER_TIMEOUT = 2 s` (a slower renderer is **aborted** and the row draws with the
//! built-in framing). Everything in this module is therefore sync, allocation-bounded and free of
//! I/O — the only I/O in the file is the output guard's spill, which runs on the tool-execution
//! path, never on the render path.
//!
//! # The output guard's security contract, stated exactly (MCP-230)
//!
//! `mcp-output-guard.ts` is a **size** guard, not a safety guard. It performs **no** prompt-injection
//! detection, **no** secret or credential redaction and **no** content classification. What it does
//! is cap the model-facing text at `maxBytes`/`maxLines`, spill the remainder to a `0600` file in a
//! `0700` per-invocation directory, bound `details.mcpResult` so a session file cannot grow without
//! limit, and clamp a hostile `image/*` mime type to 100 characters. That is the whole of it.
//!
//! The cyrup-side fact that completes the picture: the permission gate runs at `EventKind::ToolCall`
//! — **before** the call, on the arguments — and never inspects the result, so MCP tool *output* is
//! unfiltered text entering the model's context under either system. A result-side hook would be a
//! new `EventKind` plus a new `EventPatch` arm and has no upstream counterpart; gap-analysis 13e
//! explicitly declines to file it as a port unit.
//!
//! # Port units
//!
//! Implemented: MCP-220, MCP-221, MCP-222, MCP-223, MCP-226, MCP-227, MCP-228, MCP-229, MCP-230,
//! MCP-237, MCP-238, MCP-239, MCP-240, MCP-241, MCP-242, MCP-243, MCP-244, MCP-245.
//! MCP-225's resolution half already lives in [`crate::config::McpSettings::output_guard`] and is
//! **reused**, not re-derived. MCP-224 (the cleanup drain's 30 s / 3-attempt retry) is stubbed —
//! see [`MaterializedResources::cleanup`].

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use indexmap::IndexMap;
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::config::{McpSettings, ToolResultRendering};

// ===================================================================================================
// 1 · Constants — `tool-result-renderer.ts:55-60`, `mcp-output-guard.ts:7-13`, `tool-registrar.ts:10-14`
// ===================================================================================================

/// `tool-result-renderer.ts:55` `DEFAULT_MAX_CALL_INPUT_CHARS`.
pub const DEFAULT_MAX_CALL_INPUT_CHARS: usize = 1500;
/// `tool-result-renderer.ts:57` `DEFAULT_BOXED_COLLAPSED_LINES`.
pub const DEFAULT_BOXED_COLLAPSED_LINES: u8 = 3;
/// `tool-result-renderer.ts:58` `DEFAULT_COMPACT_COLLAPSED_LINES`.
pub const DEFAULT_COMPACT_COLLAPSED_LINES: u8 = 1;
/// `tool-result-renderer.ts:59` `DEFAULT_MAX_COLLAPSED_CHARS`.
pub const DEFAULT_MAX_COLLAPSED_CHARS: usize = 8000;

/// `mcp-output-guard.ts:11` `CONTENT_SUMMARY_LIMIT`.
const CONTENT_SUMMARY_LIMIT: usize = 20;
/// `mcp-output-guard.ts:12` `KEY_PREVIEW_LIMIT`.
const KEY_PREVIEW_LIMIT: usize = 20;
/// `mcp-output-guard.ts:13` `KEY_MAX_CHARS`.
const KEY_MAX_CHARS: usize = 120;

/// `tool-registrar.ts:10` `MAX_BINARY_RESOURCE_BYTES` — 10 MiB per resource.
const MAX_BINARY_RESOURCE_BYTES: u64 = 10 * 1024 * 1024;
/// `tool-registrar.ts:11` `MAX_SESSION_RESOURCE_BYTES` — 100 MiB per session.
const MAX_SESSION_RESOURCE_BYTES: u64 = 100 * 1024 * 1024;
/// `tool-registrar.ts:12` `MAX_SESSION_RESOURCE_FILES` — bounds metadata from tiny resources.
const MAX_SESSION_RESOURCE_FILES: u64 = 10_000;

/// `mcp-output-guard.ts:360` — the `mkdtemp` prefix used for BOTH artifact kinds.
const OUTPUT_ARTIFACT_DIR_PREFIX: &str = "pi-mcp-output-";
/// `tool-registrar.ts:151` — the materialized-resource directory prefix.
const RESOURCE_DIR_PREFIX: &str = "pi-mcp-resource-";

// ===================================================================================================
// 2 · Text primitives — JS `String.length` is UTF-16, `Buffer.byteLength` is UTF-8
// ===================================================================================================

/// JS `value.length` — UTF-16 code units, not `char`s and not bytes.
#[must_use]
fn utf16_len(value: &str) -> usize {
    value.encode_utf16().count()
}

/// The longest **char-boundary** prefix of `value` whose UTF-16 length is at most `units`.
///
/// JS `value.slice(0, n)` cuts at a UTF-16 index and can leave a lone surrogate; Rust cannot
/// represent one, so a cut that would land inside an astral character backs off to the character
/// before it. For every string below U+10000 — which is every tool name, every JSON key and the
/// overwhelming majority of tool output — the two agree exactly.
#[must_use]
fn take_utf16(value: &str, units: usize) -> &str {
    let mut used = 0usize;
    for (idx, ch) in value.char_indices() {
        let width = ch.len_utf16();
        if used + width > units {
            return value.get(..idx).unwrap_or("");
        }
        used += width;
    }
    value
}

/// `tool-result-renderer.ts:189-192` `truncateText`.
#[must_use]
pub fn truncate_text(value: &str, max_chars: usize) -> String {
    if utf16_len(value) <= max_chars {
        return value.to_string();
    }
    format!("{}…", take_utf16(value, max_chars.saturating_sub(1)))
}

/// `mcp-output-guard.ts:386-388` `byteLength` — `Buffer.byteLength(text, "utf8")`.
#[must_use]
fn byte_length(text: &str) -> usize {
    text.len()
}

/// `mcp-output-guard.ts:382-384` `textStats`. Note the empty string is **0** lines, not 1.
#[must_use]
fn text_stats(text: &str) -> (usize, usize) {
    let lines = if text.is_empty() { 0 } else { text.split('\n').count() };
    (byte_length(text), lines)
}

/// `Number.prototype.toLocaleString()` under Node's default ICU locale: thousands-grouped with `,`.
///
/// gap-analysis 13e (MCP-227) is explicit that this must be reproduced rather than approximated, and
/// equally explicit that it must not pull an i18n crate to do it.
#[must_use]
fn format_thousands(value: u64) -> String {
    let digits = value.to_string();
    let bytes = digits.as_bytes();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(char::from(*b));
    }
    out
}

/// `Number.prototype.toFixed(1)` — round half away from zero, which `f64::round` does and Rust's
/// `{:.1}` (round half to even) does not.
#[must_use]
fn to_fixed_1(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    format!("{rounded:.1}")
}

/// `mcp-output-guard.ts:404-408` `formatSize`. **Note the space, and `KiB`/`MiB` — not `KB`/`MB`.**
///
/// `cyrup_tools::truncate::format_size` emits `50.0KB` (no space, decimal units) and is deliberately
/// NOT reused: this string is asserted byte-for-byte upstream.
#[must_use]
fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    #[allow(clippy::cast_precision_loss)]
    let as_f64 = bytes as f64;
    if bytes < 1024 * 1024 {
        return format!("{} KiB", to_fixed_1(as_f64 / 1024.0));
    }
    format!("{} MiB", to_fixed_1(as_f64 / (1024.0 * 1024.0)))
}

/// `mcp-output-guard.ts:243-249` `truncateStringToBytes` — back off UTF-8 continuation bytes.
///
/// Upstream walks `(buffer.readUInt8(end) & 0xc0) === 0x80` backwards from the byte *after* the last
/// kept byte; `str::is_char_boundary` is the same predicate, cheaper and total.
#[must_use]
fn truncate_string_to_bytes(value: &str, max_bytes: usize) -> &str {
    if byte_length(value) <= max_bytes {
        return value;
    }
    let mut end = max_bytes.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.get(..end).unwrap_or("")
}

/// JS truthiness for a `serde_json::Value` — `undefined`/`null`/`false`/`0`/`NaN`/`""` are falsy and
/// **every** object and array (including `{}` and `[]`) is truthy.
#[must_use]
fn is_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// A non-empty string field, JS `if (args.tool)` style.
#[must_use]
fn truthy_str<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

/// JS `typeof`. `typeof null === "object"` is deliberate, not a bug being ported.
#[must_use]
fn js_typeof(value: &Value) -> &'static str {
    match value {
        Value::Null | Value::Array(_) | Value::Object(_) => "object",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
    }
}

// ===================================================================================================
// 3 · An order-preserving JSON value — `JSON.parse` / `JSON.stringify`
// ===================================================================================================

/// A JSON value that keeps object key order, which `serde_json::Value` does not under this
/// workspace's feature set (`serde_json::Map` is a `BTreeMap`; `preserve_order` is off workspace-wide
/// and `xtask/Cargo.toml` records why it stays off).
///
/// It exists for exactly one reason: `formatJsonish`'s string arm
/// (`tool-result-renderer.ts:194-208`) is `JSON.stringify(JSON.parse(value), null, 2)`, and the
/// value it re-indents is the `mcp` gateway's `args` parameter — a JSON string the **model** wrote,
/// whose key order is the order the user reads in the transcript. Round-tripping it through
/// `serde_json::Value` would sort those keys.
///
/// Payloads that arrive already typed as `serde_json::Value` (the `render_call` args object, a
/// `CallToolResult`'s `structuredContent`) have had their order destroyed upstream of this seam;
/// [`Self::from_value`] carries them through the same stringifier so there is exactly ONE
/// `JSON.stringify` in this crate.
#[derive(Debug, Clone, PartialEq)]
enum OrderedJson {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Str(String),
    Array(Vec<OrderedJson>),
    Object(IndexMap<String, OrderedJson>),
}

impl<'de> serde::Deserialize<'de> for OrderedJson {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct AnyJson;
        impl<'d> Visitor<'d> for AnyJson {
            type Value = OrderedJson;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Null)
            }
            fn visit_none<E: serde::de::Error>(self) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Null)
            }
            fn visit_some<D2: serde::Deserializer<'d>>(self, d: D2) -> Result<OrderedJson, D2::Error> {
                d.deserialize_any(AnyJson)
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Bool(v))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Int(v))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<OrderedJson, E> {
                Ok(OrderedJson::UInt(v))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Float(v))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Str(v.to_string()))
            }
            fn visit_string<E: serde::de::Error>(self, v: String) -> Result<OrderedJson, E> {
                Ok(OrderedJson::Str(v))
            }
            fn visit_seq<A: SeqAccess<'d>>(self, mut a: A) -> Result<OrderedJson, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = a.next_element::<OrderedJson>()? {
                    items.push(item);
                }
                Ok(OrderedJson::Array(items))
            }
            fn visit_map<A: MapAccess<'d>>(self, mut a: A) -> Result<OrderedJson, A::Error> {
                let mut map = IndexMap::new();
                while let Some((k, v)) = a.next_entry::<String, OrderedJson>()? {
                    // `JSON.parse` is last-wins on a duplicate key; `IndexMap::insert` keeps the
                    // FIRST key's position and overwrites the value, which is the same observable
                    // result for every well-formed payload and differs only in the position of a
                    // duplicated key — a shape no serializer emits.
                    map.insert(k, v);
                }
                Ok(OrderedJson::Object(map))
            }
        }
        d.deserialize_any(AnyJson)
    }
}

impl OrderedJson {
    /// `JSON.parse(text)` — `None` when the text is not JSON, which is upstream's `catch`.
    fn parse(text: &str) -> Option<Self> {
        serde_json::from_str::<Self>(text).ok()
    }

    /// Carry an already-sorted [`Value`] through the same stringifier.
    fn from_value(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => n.as_u64().map_or_else(
                || n.as_i64().map_or_else(|| Self::Float(n.as_f64().unwrap_or(0.0)), Self::Int),
                Self::UInt,
            ),
            Value::String(s) => Self::Str(s.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from_value).collect()),
            Value::Object(map) => {
                Self::Object(map.iter().map(|(k, v)| (k.clone(), Self::from_value(v))).collect())
            }
        }
    }

    /// `JSON.stringify(value)` (compact) or `JSON.stringify(value, null, 2)` (`indent = Some(2)`).
    fn stringify(&self, indent: Option<usize>) -> String {
        let mut out = String::new();
        self.write(&mut out, indent, 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        match self {
            Self::Null => out.push_str("null"),
            Self::Bool(true) => out.push_str("true"),
            Self::Bool(false) => out.push_str("false"),
            Self::Int(v) => out.push_str(&v.to_string()),
            Self::UInt(v) => out.push_str(&v.to_string()),
            Self::Float(v) => out.push_str(&js_number(*v)),
            Self::Str(s) => out.push_str(&json_quote(s)),
            Self::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_break(out, indent, depth + 1);
                    item.write(out, indent, depth + 1);
                }
                write_break(out, indent, depth);
                out.push(']');
            }
            Self::Object(map) => {
                if map.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_break(out, indent, depth + 1);
                    out.push_str(&json_quote(k));
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    v.write(out, indent, depth + 1);
                }
                write_break(out, indent, depth);
                out.push('}');
            }
        }
    }
}

fn write_break(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(step) = indent {
        out.push('\n');
        for _ in 0..(step * depth) {
            out.push(' ');
        }
    }
}

/// A JSON string literal. `serde_json`'s escape set is `JSON.stringify`'s escape set — `"`, `\`,
/// `\b`, `\f`, `\n`, `\r`, `\t`, `\u00XX` for the remaining C0 controls — and neither escapes
/// non-ASCII.
fn json_quote(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("{s:?}"))
}

/// `String(number)` / the number grammar `JSON.stringify` emits.
///
/// The three ways Rust's `f64` `Display` differs from JS and what is done about each: an integral
/// float prints `1.0` where JS prints `1` (fixed); Rust never uses exponent notation where JS
/// switches at `1e21` and `1e-7` (fixed); Rust writes `1e21` where JS writes `1e+21` (fixed).
fn js_number(value: f64) -> String {
    if !value.is_finite() {
        // `JSON.stringify(NaN) === "null"`, and the same for ±Infinity.
        return "null".to_string();
    }
    if value == 0.0 {
        // Covers -0.0, which `JSON.stringify` renders as `0`.
        return "0".to_string();
    }
    let magnitude = value.abs();
    if !(1e-6..1e21).contains(&magnitude) {
        let raw = format!("{value:e}");
        return match raw.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => {
                format!("{mantissa}e+{exponent}")
            }
            _ => raw,
        };
    }
    if value.fract() == 0.0 {
        #[allow(clippy::cast_possible_truncation)]
        return format!("{}", value as i128);
    }
    format!("{value}")
}

/// `String(value)` over a JSON value — the `estimateValueBytes` number/boolean measure.
fn js_number_string(number: &serde_json::Number) -> String {
    number
        .as_u64()
        .map(|v| v.to_string())
        .or_else(|| number.as_i64().map(|v| v.to_string()))
        .unwrap_or_else(|| js_number(number.as_f64().unwrap_or(0.0)))
}

/// `mcp-output-guard.ts:373-380` `safeStringify` — plain `JSON.stringify`, degrading to `String(v)`.
///
/// A Rust `Value` cannot contain a cycle, so the `catch` arm is structurally unreachable; it is kept
/// as the `unwrap_or_else` so the shape is visible.
fn safe_stringify(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

/// `tool-result-renderer.ts:194-208` `formatJsonish`.
///
/// A string that parses as JSON is **re-indented**, preserving its key order (see [`OrderedJson`]);
/// a string that does not is used raw; anything else is `JSON.stringify(value, null, 2)`. Either way
/// through [`truncate_text`].
#[must_use]
pub fn format_jsonish(value: &Value, max_chars: usize) -> String {
    if let Some(text) = value.as_str() {
        return match OrderedJson::parse(text) {
            Some(parsed) => truncate_text(&parsed.stringify(Some(2)), max_chars),
            None => truncate_text(text, max_chars),
        };
    }
    truncate_text(&OrderedJson::from_value(value).stringify(Some(2)), max_chars)
}

/// `tool-result-renderer.ts:210-212` `hasUsefulObjectContent` — a non-array object with ≥1 key.
#[must_use]
fn has_useful_object_content(value: &Value) -> bool {
    value.as_object().is_some_and(|o| !o.is_empty())
}

// ===================================================================================================
// 4 · Content blocks — `tool-registrar.ts:179-265` (MCP-220, MCP-221, MCP-222)
// ===================================================================================================

/// pi's `ContentBlock` — `TextContent | ImageContent`, the only two shapes an MCP result may reach
/// the provider as. It maps 1:1 onto [`cyrup_core::Content::Text`]/[`cyrup_core::Content::Image`],
/// which is *why* `transformMcpContent` collapsing `audio`/`resource`/`resource_link` to text is
/// mandatory rather than a shortcut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpContentBlock {
    /// `{type:"text", text}`.
    Text(String),
    /// `{type:"image", data, mimeType}` — base64 payload, delivered natively, never as text.
    Image { data: String, mime_type: String },
}

impl McpContentBlock {
    /// A text block, for the many call sites that build one.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    /// The `cyrup-core` form the agent loop consumes.
    #[must_use]
    pub fn into_core(self) -> cyrup_core::Content {
        match self {
            Self::Text(text) => cyrup_core::Content::Text { text, text_signature: None },
            Self::Image { data, mime_type } => cyrup_core::Content::Image { data, mime_type },
        }
    }

    /// The inverse of [`Self::into_core`].
    ///
    /// `Thinking` and `ToolCall` have no `ContentBlock` counterpart — pi's union is
    /// `TextContent | ImageContent` and an MCP tool never produces either — so they collapse onto
    /// their text, which is what upstream's `.filter(block => block.type === "text")` walks would do
    /// to them anyway. A `ToolCall` carries no text and becomes an empty text block.
    #[must_use]
    pub fn from_core(content: &cyrup_core::Content) -> Self {
        match content {
            cyrup_core::Content::Text { text, .. } => Self::Text(text.clone()),
            cyrup_core::Content::Thinking { thinking, .. } => Self::Text(thinking.clone()),
            cyrup_core::Content::ToolCall(_) => Self::Text(String::new()),
            cyrup_core::Content::Image { data, mime_type } => {
                Self::Image { data: data.clone(), mime_type: mime_type.clone() }
            }
        }
    }

    /// Bulk [`Self::from_core`] — the bridge `proxy.rs`'s
    /// `ProxyEnv::guard_mcp_output(Vec<Content>, …) -> GuardedOutput` needs on the way in.
    #[must_use]
    pub fn vec_from_core(content: &[cyrup_core::Content]) -> Vec<Self> {
        content.iter().map(Self::from_core).collect()
    }

    /// Bulk [`Self::into_core`] — the same bridge on the way out.
    #[must_use]
    pub fn vec_into_core(blocks: Vec<Self>) -> Vec<cyrup_core::Content> {
        blocks.into_iter().map(Self::into_core).collect()
    }

    /// Read one block back out of a serialized `AgentToolResult.content` entry
    /// (`cyrup_core::Content`'s internally-tagged form: `{"type":"text"|"image", …}`).
    ///
    /// A block that is neither — a `thinking` or `toolCall` entry, which an MCP tool never emits —
    /// takes upstream's non-text branch and renders as an image placeholder, exactly as
    /// `blockToLines` does for any `block.type !== "text"`.
    #[must_use]
    fn from_result_value(value: &Value) -> Self {
        if value.get("type").and_then(Value::as_str) == Some("text") {
            return Self::Text(value.get("text").and_then(Value::as_str).unwrap_or("").to_string());
        }
        Self::Image {
            data: value.get("data").and_then(Value::as_str).unwrap_or("").to_string(),
            // JS interpolates a missing `mimeType` as the literal `undefined`; reproduced so a
            // malformed block is visibly malformed rather than silently blank.
            mime_type: value
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("undefined")
                .to_string(),
        }
    }

    #[must_use]
    const fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

/// `tool-registrar.ts:187-234` `transformMcpContent` — every standard MCP content type, in the
/// source's branch order (MCP-220).
///
/// `ui://` resource rendering is the only cut half and it never lived in this function; the unknown
/// arm re-serializes the **original** JSON so a lossy typed enum cannot destroy it.
#[must_use]
pub fn transform_mcp_content(
    content: &[Value],
    scope: Option<&MaterializedResources>,
) -> Vec<McpContentBlock> {
    content
        .iter()
        .map(|c| match c.get("type").and_then(Value::as_str) {
            Some("text") => {
                McpContentBlock::Text(c.get("text").and_then(Value::as_str).unwrap_or("").to_string())
            }
            Some("image") => McpContentBlock::Image {
                data: c.get("data").and_then(Value::as_str).unwrap_or("").to_string(),
                mime_type: c
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .unwrap_or("image/png")
                    .to_string(),
            },
            Some("resource") => {
                let resource = c.get("resource").filter(|r| !r.is_null());
                let uri = resource
                    .and_then(|r| r.get("uri"))
                    .and_then(Value::as_str)
                    .unwrap_or("(no URI)");
                if let Some(resource) = resource
                    && let Some(blob) = resource.get("blob").and_then(Value::as_str)
                {
                    return McpContentBlock::Text(materialize(scope, resource, blob));
                }
                // The `(no content)` fallback fires when the `resource` object itself is absent —
                // NEVER as a `JSON.stringify` fallback. Getting this wrong turns a stringifiable
                // record into the placeholder.
                let body = match resource {
                    Some(r) => r
                        .get("text")
                        .and_then(Value::as_str)
                        .map_or_else(|| safe_stringify(r), ToString::to_string),
                    None => "(no content)".to_string(),
                };
                McpContentBlock::Text(format!("[Resource: {uri}]\n{body}"))
            }
            Some("resource_link") => {
                let uri = c.get("uri").and_then(Value::as_str);
                let name = c.get("name").and_then(Value::as_str).or(uri).unwrap_or("unknown");
                McpContentBlock::Text(format!(
                    "[Resource Link: {name}]\nURI: {}",
                    uri.unwrap_or("(no URI)")
                ))
            }
            Some("audio") => McpContentBlock::Text(format!(
                "[Audio content: {}]",
                c.get("mimeType").and_then(Value::as_str).unwrap_or("audio/*")
            )),
            _ => McpContentBlock::Text(safe_stringify(c)),
        })
        .collect()
}

/// `tool-registrar.ts:179-185` `transformMcpResourceContents` — the `resources/read` shape (MCP-221).
///
/// A string `text` wins, then a string `blob` is materialized, then the **whole** record is
/// stringified — `uri` and `mimeType` included, not just an unknown field.
#[must_use]
pub fn transform_mcp_resource_contents(
    contents: &[Value],
    scope: Option<&MaterializedResources>,
) -> Vec<McpContentBlock> {
    contents
        .iter()
        .map(|resource| {
            if let Some(text) = resource.get("text").and_then(Value::as_str) {
                return McpContentBlock::Text(text.to_string());
            }
            if let Some(blob) = resource.get("blob").and_then(Value::as_str) {
                return McpContentBlock::Text(materialize(scope, resource, blob));
            }
            McpContentBlock::Text(safe_stringify(resource))
        })
        .collect()
}

/// `tool-registrar.ts:236-255` `resolveMcpResultContent` + `stringifyStructuredContent` (MCP-222).
///
/// The `structuredContent` fallback fires only when the transformed block list is **empty** and only
/// when `structuredContent` is neither absent nor `null`.
#[must_use]
pub fn resolve_mcp_result_content(
    result: &Value,
    scope: Option<&MaterializedResources>,
) -> Vec<McpContentBlock> {
    let content = result.get("content").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
    let blocks = transform_mcp_content(content, scope);
    if !blocks.is_empty() {
        return blocks;
    }
    match result.get("structuredContent").filter(|v| !v.is_null()) {
        Some(value) => {
            vec![McpContentBlock::Text(OrderedJson::from_value(value).stringify(Some(2)))]
        }
        None => Vec::new(),
    }
}

fn materialize(scope: Option<&MaterializedResources>, resource: &Value, blob: &str) -> String {
    let uri = resource.get("uri").and_then(Value::as_str);
    let mime = resource.get("mimeType").and_then(Value::as_str);
    match scope {
        Some(session) => session.materialize(uri, mime, blob),
        None => MaterializedResources::global().materialize(uri, mime, blob),
    }
}

// ===================================================================================================
// 5 · Binary-resource materialization — `tool-registrar.ts:95-176` (MCP-223, MCP-224)
// ===================================================================================================

#[derive(Debug, Default)]
struct MaterializedSessionState {
    directory: Option<PathBuf>,
    bytes: u64,
    files: u64,
    sequence: u64,
}

/// One MCP runtime's materialized-resource session — upstream's `WeakMap`-keyed
/// `MaterializedResourceSession`, which Rust has no weak-keyed identity map for.
///
/// The scope's "is this runtime still alive" test is upstream's `isAbortedScope`
/// (`"aborted" in scope && scope.aborted === true`); here it is the owner's
/// [`cyrup_core::CancelToken`]. A cancelled token yields **no** session at all, so every blob in
/// flight when the runtime stops is omitted with `runtime stopped` rather than written to a
/// directory nothing will ever clean up.
///
/// **Security posture (MCP-223/MCP-228).** MCP resource blobs routinely carry API payloads and
/// customer data. The directory is created `0700` and each file `create_new` + `0600`, so nothing
/// lands world-readable in the shared temp dir and a write can never clobber an existing path.
/// `cyrup_tools::output` spills through `std::env::temp_dir().join(name)` + `File::create` with a
/// predictable `pid-nanos-counter` name, the default umask and no exclusive create — that posture is
/// deliberately **not** copied here.
#[derive(Debug)]
pub struct MaterializedResources {
    state: Mutex<MaterializedSessionState>,
    cancel: Option<cyrup_core::CancelToken>,
}

impl MaterializedResources {
    /// A session fenced by the runtime owner's cancellation token.
    #[must_use]
    pub fn new(cancel: Option<cyrup_core::CancelToken>) -> Self {
        Self { state: Mutex::new(MaterializedSessionState::default()), cancel }
    }

    /// `tool-registrar.ts:31` `defaultMaterializedResourceSession` — the session used when a call
    /// site passes no scope.
    #[must_use]
    pub fn global() -> &'static Self {
        static GLOBAL: OnceLock<MaterializedResources> = OnceLock::new();
        GLOBAL.get_or_init(|| Self::new(None))
    }

    /// `tool-registrar.ts:133-176` `materializeBinaryResource` — returns the replacement text, which
    /// upstream writes back over the resource's `blob` key (`replaceBlob`). The port returns it
    /// instead of mutating its input; nothing downstream reads the resource object again.
    #[must_use]
    pub fn materialize(&self, uri: Option<&str>, mime: Option<&str>, blob: &str) -> String {
        // 1. An aborted scope has no session (`getMaterializedResourceSession` → `undefined`).
        if self.cancel.as_ref().is_some_and(cyrup_core::CancelToken::is_cancelled) {
            return omit_binary_resource(uri, mime, "runtime stopped");
        }
        // 2. `Buffer.byteLength(blob, "base64")` — the DECODED size, measured without decoding.
        let decoded_bytes = base64_decoded_len(blob);
        // 3. Per-resource cap.
        if decoded_bytes > MAX_BINARY_RESOURCE_BYTES {
            return omit_binary_resource(uri, mime, "decoded size exceeds 10 MiB");
        }

        let Ok(mut state) = self.state.lock() else {
            return omit_binary_resource(uri, mime, "could not be saved");
        };
        // 4. Per-session caps.
        if state.bytes.saturating_add(decoded_bytes) > MAX_SESSION_RESOURCE_BYTES
            || state.files >= MAX_SESSION_RESOURCE_FILES
        {
            return omit_binary_resource(uri, mime, "session resource limit reached");
        }
        // 5. `session.directory ??= mkdtempSync(...)`.
        if state.directory.is_none() {
            match make_private_temp_dir(RESOURCE_DIR_PREFIX) {
                Ok(dir) => state.directory = Some(dir),
                Err(_) => return omit_binary_resource(uri, mime, "could not be saved"),
            }
        }
        let Some(directory) = state.directory.clone() else {
            return omit_binary_resource(uri, mime, "could not be saved");
        };

        // 6. The counters are incremented BEFORE the write and rolled back on failure — except when
        //    the removal of the partial file itself fails, where the reservation is deliberately
        //    KEPT so a half-written file still counts against the session budget.
        state.sequence += 1;
        let file_path = directory.join(format!("resource-{}.bin", state.sequence));
        state.bytes = state.bytes.saturating_add(decoded_bytes);
        state.files += 1;

        let decoded = decode_base64_lenient(blob);
        let wrote = decoded.and_then(|bytes| {
            use std::io::Write as _;
            let mut file = create_private_file(&file_path)?;
            file.write_all(&bytes)?;
            file.flush()
        });
        if wrote.is_err() {
            if std::fs::remove_file(&file_path).is_ok() {
                state.bytes = state.bytes.saturating_sub(decoded_bytes);
                state.files = state.files.saturating_sub(1);
            }
            return omit_binary_resource(uri, mime, "could not be saved");
        }

        // 7. `replaceBlob` — the three-line body.
        format!(
            "[Resource: {}]\nBinary content saved to {}\nMIME type: {}",
            uri.unwrap_or("(no URI)"),
            file_path.display(),
            mime.unwrap_or("application/octet-stream"),
        )
    }

    /// `tool-registrar.ts:104-116` `cleanupMaterializedBinaryResources` — drop the session's
    /// directory and zero the counters.
    ///
    /// TODO(MCP-224): the pending-set drain is not ported. Upstream keeps a module-global
    /// `pendingCleanupDirectories` with per-directory attempt counters capped at
    /// `MAX_CLEANUP_RETRY_ATTEMPTS = 3`, a single `CLEANUP_RETRY_DELAY_MS = 30_000` timer guarded by
    /// "already pending or nothing retryable", and an `AggregateError("Failed to clean materialized
    /// MCP resources")` on any failure. gap-analysis 13e says the module-global becomes an instance
    /// field (so two sessions do not share a retry budget) and the timer becomes `tokio::spawn` +
    /// `tokio::time::sleep` behind an `Option<JoinHandle>`. Until that lands, a directory that
    /// cannot be removed — a Windows lock, a stale NFS handle — is reported once and then leaked
    /// rather than retried. MCP-224 is `medium`; the security-relevant half (0700/0600 creation) is
    /// MCP-223 and is complete above.
    ///
    /// # Errors
    /// The `std::io::Error` from the recursive removal, when one occurred.
    pub fn cleanup(&self) -> Result<(), std::io::Error> {
        let Ok(mut state) = self.state.lock() else {
            return Ok(());
        };
        let directory = state.directory.take();
        state.bytes = 0;
        state.files = 0;
        state.sequence = 0;
        drop(state);
        match directory {
            Some(dir) => std::fs::remove_dir_all(dir),
            None => Ok(()),
        }
    }
}

/// `tool-registrar.ts:125-131` `omitBinaryResource` — the same three-line shape as a successful
/// materialization, with the reason in place of the path.
fn omit_binary_resource(uri: Option<&str>, mime: Option<&str>, reason: &str) -> String {
    format!(
        "[Resource: {}]\nBinary content omitted: {reason}\nMIME type: {}",
        uri.unwrap_or("(no URI)"),
        mime.unwrap_or("application/octet-stream"),
    )
}

/// Node's `Buffer.byteLength(str, "base64")`: strip at most two trailing `=` and return
/// `(len * 3) >> 2`. It is an upper-bound estimate, not a decode, which is exactly what the 10 MiB
/// gate needs — a hostile blob is rejected before any allocation.
#[must_use]
fn base64_decoded_len(blob: &str) -> u64 {
    let mut len = blob.len();
    if blob.ends_with('=') {
        len -= 1;
        if len > 1 && blob.get(..len).is_some_and(|s| s.ends_with('=')) {
            len -= 1;
        }
    }
    (len as u64 * 3) >> 2_u32
}

/// Node's base64 decoder ignores every character outside the alphabet and tolerates missing padding;
/// Rust's does neither, so the input is filtered and decoded unpadded. `-`/`_` are folded to `+`/`/`
/// because Node accepts the URL-safe alphabet on the same code path.
fn decode_base64_lenient(blob: &str) -> Result<Vec<u8>, std::io::Error> {
    use base64::Engine as _;
    let filtered: String = blob
        .chars()
        .filter_map(|c| match c {
            '-' => Some('+'),
            '_' => Some('/'),
            c if c.is_ascii_alphanumeric() || c == '+' || c == '/' => Some(c),
            _ => None,
        })
        .collect();
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(filtered)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// `mkdtemp(join(tmpdir(), prefix))` — a fresh directory nobody else can read.
fn make_private_temp_dir(prefix: &str) -> Result<PathBuf, std::io::Error> {
    let base = std::env::temp_dir();
    let mut last: Option<std::io::Error> = None;
    for _ in 0..16 {
        let candidate = base.join(format!("{prefix}{}", random_hex(6)));
        match create_private_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) => last = Some(err),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::AlreadyExists, "could not create a temp directory")
    }))
}

#[cfg(unix)]
fn create_private_dir(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(not(unix))]
fn create_private_dir(path: &Path) -> Result<(), std::io::Error> {
    std::fs::DirBuilder::new().create(path)
}

#[cfg(unix)]
fn create_private_file(path: &Path) -> Result<std::fs::File, std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)
}

#[cfg(not(unix))]
fn create_private_file(path: &Path) -> Result<std::fs::File, std::io::Error> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(path)
}

/// `randomBytes(n).toString("hex")`. `uuid` is already a crate dependency and its v7 layout carries
/// 74 random bits after the timestamp; the directory's `0700` mode is the actual protection, so the
/// name only has to be collision-resistant.
fn random_hex(bytes: usize) -> String {
    let id = uuid::Uuid::now_v7();
    let raw = id.as_bytes();
    let mut out = String::with_capacity(bytes * 2);
    for b in raw.iter().rev().take(bytes) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

// ===================================================================================================
// 6 · The output guard — `mcp-output-guard.ts` (MCP-226..MCP-230)
// ===================================================================================================

/// `McpOutputGuardOptions` (`mcp-output-guard.ts:43-58`). The three limits are pre-resolved by
/// [`crate::config::McpSettings::output_guard`] (MCP-225) and arrive as a
/// [`crate::config::ResolvedOutputGuard`] via [`Self::from_resolved`].
#[derive(Debug, Clone)]
pub struct McpOutputGuardOptions<'a> {
    /// `enabled === false` short-circuits the whole guard, `details.mcpResult` bounding included.
    pub enabled: bool,
    /// Prepended to the **first** text block, or unshifted as a new one when there is none.
    pub prefix: &'a str,
    /// Appended to the **last** text block, or pushed as a new one when there is none.
    pub suffix: &'a str,
    /// Replaces the whole array when the joined text is empty. `None` is upstream's `undefined`,
    /// which disables `withEmptyTextFallback` entirely.
    pub empty_text_fallback: Option<&'a str>,
    pub max_bytes: usize,
    pub max_lines: usize,
    pub details_max_bytes: usize,
    /// The raw MCP result to expose as `details.mcpResult`. **Direct tools never pass one**, so the
    /// bounding path is proxy-only.
    pub raw_mcp_result: Option<&'a Value>,
}

impl<'a> McpOutputGuardOptions<'a> {
    /// Build from the already-resolved settings, reusing MCP-225's answer rather than re-deriving it.
    #[must_use]
    pub fn from_resolved(resolved: crate::config::ResolvedOutputGuard) -> Self {
        Self {
            enabled: resolved.enabled,
            prefix: "",
            suffix: "",
            empty_text_fallback: None,
            max_bytes: usize::try_from(resolved.max_bytes).unwrap_or(usize::MAX),
            max_lines: usize::try_from(resolved.max_lines).unwrap_or(usize::MAX),
            details_max_bytes: usize::try_from(resolved.details_max_bytes).unwrap_or(usize::MAX),
            raw_mcp_result: None,
        }
    }
}

/// `GuardedMcpOutput` (`mcp-output-guard.ts:60-64`).
#[derive(Debug, Clone, Default)]
pub struct GuardedMcpOutput {
    pub content: Vec<McpContentBlock>,
    /// `details.outputGuard` — present only when truncation fired.
    pub output_guard: Option<Value>,
    /// `details.mcpResult` — present only when a raw result was supplied.
    pub mcp_result: Option<Value>,
}

impl GuardedMcpOutput {
    /// `mcp-output-guard.ts:78-83` `guardedMcpDetails` — the spread helper; each key appears only
    /// when defined.
    #[must_use]
    pub fn details(&self) -> Map<String, Value> {
        let mut map = Map::new();
        if let Some(result) = &self.mcp_result {
            map.insert("mcpResult".to_string(), result.clone());
        }
        if let Some(guard) = &self.output_guard {
            map.insert("outputGuard".to_string(), guard.clone());
        }
        map
    }

    /// The guarded content in the form [`cyrup_core::ToolResult::content`] wants.
    #[must_use]
    pub fn into_core_content(self) -> Vec<cyrup_core::Content> {
        McpContentBlock::vec_into_core(self.content)
    }
}

/// `mcp-output-guard.ts:90-155` `guardMcpOutput` (MCP-226/MCP-227).
///
/// Sync where upstream is `async`: the two writes it can perform are on the tool-execution path, are
/// rare (only a truncation or an oversized `details.mcpResult` reaches them) and are a single small
/// file each. A caller already inside a `tokio` task that cares about the reactor may wrap the call
/// in `spawn_blocking`; nothing here awaits, locks a shared runtime, or blocks on the UI.
#[must_use]
pub fn guard_mcp_output(
    content: &[McpContentBlock],
    options: &McpOutputGuardOptions<'_>,
) -> GuardedMcpOutput {
    // 1. Normalize: sanitize image mimes, then apply the empty-text fallback.
    let normalized = if content.is_empty() {
        vec![McpContentBlock::Text(options.empty_text_fallback.unwrap_or("(empty result)").to_string())]
    } else {
        sanitize_content(content)
    };
    let normalized = with_empty_text_fallback(normalized, options.empty_text_fallback);

    // 2. The kill switch. It disables `details.mcpResult` bounding too — that is the point of
    //    `MCP_OUTPUT_GUARD=0`: raw MCP output, for debugging.
    if !options.enabled {
        return GuardedMcpOutput {
            content: add_affixes(normalized, options.prefix, options.suffix),
            output_guard: None,
            mcp_result: options.raw_mcp_result.cloned(),
        };
    }

    // 3. Compose the model-facing text and measure it.
    let image_blocks: Vec<McpContentBlock> =
        normalized.iter().filter(|b| b.is_image()).cloned().collect();
    let text_output = normalized
        .iter()
        .filter_map(|b| match b {
            McpContentBlock::Text(t) => Some(t.as_str()),
            McpContentBlock::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    let composed = format!("{}{text_output}{}", options.prefix, options.suffix);
    let (original_bytes, original_lines) = text_stats(&composed);

    // 4. The affixed content is the answer unless truncation fires.
    let mut guarded_content = add_affixes(normalized, options.prefix, options.suffix);
    let mut output_guard = None;

    // 5. Truncate.
    if original_bytes > options.max_bytes || original_lines > options.max_lines {
        let (full_output_path, write_error) = save_artifact("output", &composed);
        let notice =
            format_truncation_notice(original_bytes, original_lines, full_output_path.as_deref(), write_error.as_deref());
        let (budget_bytes, budget_lines) = reserve_budget(options.max_bytes, options.max_lines, &notice);
        let preview = truncate_head(&composed, budget_bytes, budget_lines);
        let final_text = format!("{preview}\n\n{notice}");
        let (returned_bytes, returned_lines) = text_stats(&final_text);

        // Every original block other than the images is DISCARDED; images pass through untouched
        // because they reach the provider as native image content, not as context text.
        let mut next = vec![McpContentBlock::Text(final_text)];
        next.extend(image_blocks.iter().cloned());
        guarded_content = next;

        let mut guard = Map::new();
        guard.insert("truncated".to_string(), Value::Bool(true));
        guard.insert("originalBytes".to_string(), Value::from(original_bytes));
        guard.insert("returnedBytes".to_string(), Value::from(returned_bytes));
        guard.insert("originalLines".to_string(), Value::from(original_lines));
        guard.insert("returnedLines".to_string(), Value::from(returned_lines));
        if !image_blocks.is_empty() {
            guard.insert("imageBlocksPassedThrough".to_string(), Value::from(image_blocks.len()));
        }
        if let Some(path) = full_output_path {
            guard.insert("fullOutputPath".to_string(), Value::String(path));
        }
        if let Some(err) = write_error {
            guard.insert("writeError".to_string(), Value::String(err));
        }
        output_guard = Some(Value::Object(guard));
    }

    // 6. Bound `details.mcpResult`. Direct tools pass none, so this is proxy-only.
    let mcp_result = options.raw_mcp_result.map(|raw| bound_mcp_result(raw, options.details_max_bytes));

    GuardedMcpOutput { content: guarded_content, output_guard, mcp_result }
}

/// `mcp-output-guard.ts:157-165` `sanitizeContent` — image blocks only: a non-blank mime is trimmed
/// and cut to 100 characters, anything else becomes `image/png`.
///
/// The 100-unit cut is a UTF-16 slice in JS and safe only because a mime type is ASCII in practice;
/// [`take_utf16`] is char-boundary-safe so a hostile mime cannot panic here.
fn sanitize_content(content: &[McpContentBlock]) -> Vec<McpContentBlock> {
    content
        .iter()
        .map(|block| match block {
            McpContentBlock::Image { data, mime_type } => {
                let trimmed = mime_type.trim();
                let mime = if trimmed.is_empty() {
                    "image/png".to_string()
                } else {
                    take_utf16(trimmed, 100).to_string()
                };
                McpContentBlock::Image { data: data.clone(), mime_type: mime }
            }
            McpContentBlock::Text(_) => block.clone(),
        })
        .collect()
}

/// `mcp-output-guard.ts:167-175` `withEmptyTextFallback`. `if (!fallback) return content` is JS
/// truthiness, so an **empty-string** fallback also disables the substitution.
fn with_empty_text_fallback(
    content: Vec<McpContentBlock>,
    fallback: Option<&str>,
) -> Vec<McpContentBlock> {
    let Some(fallback) = fallback.filter(|f| !f.is_empty()) else {
        return content;
    };
    let joined = content
        .iter()
        .filter_map(|b| match b {
            McpContentBlock::Text(t) => Some(t.as_str()),
            McpContentBlock::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if !joined.is_empty() {
        return content;
    }
    let mut next = vec![McpContentBlock::Text(fallback.to_string())];
    next.extend(content.into_iter().filter(McpContentBlock::is_image));
    next
}

/// `mcp-output-guard.ts:177-208` `addAffixes` — a prefix lands on the **first** text block and a
/// suffix on the **last**; non-text blocks keep their positions.
fn add_affixes(
    mut content: Vec<McpContentBlock>,
    prefix: &str,
    suffix: &str,
) -> Vec<McpContentBlock> {
    if prefix.is_empty() && suffix.is_empty() {
        return content;
    }
    if !prefix.is_empty() {
        match content.iter_mut().find_map(|b| match b {
            McpContentBlock::Text(t) => Some(t),
            McpContentBlock::Image { .. } => None,
        }) {
            Some(text) => *text = format!("{prefix}{text}"),
            None => content.insert(0, McpContentBlock::Text(prefix.to_string())),
        }
    }
    if !suffix.is_empty() {
        match content.iter_mut().rev().find_map(|b| match b {
            McpContentBlock::Text(t) => Some(t),
            McpContentBlock::Image { .. } => None,
        }) {
            Some(text) => text.push_str(suffix),
            None => content.push(McpContentBlock::Text(suffix.to_string())),
        }
    }
    content
}

/// `mcp-output-guard.ts:210-216` `reserveBudget` — the notice is charged against BOTH caps, clamped
/// at zero, and it is measured with its own leading `"\n\n"`.
fn reserve_budget(max_bytes: usize, max_lines: usize, notice: &str) -> (usize, usize) {
    let (bytes, lines) = text_stats(&format!("\n\n{notice}"));
    (max_bytes.saturating_sub(bytes), max_lines.saturating_sub(lines))
}

/// `mcp-output-guard.ts:218-241` `truncateHead`.
///
/// The load-bearing detail is that this **does** emit a partial line: when a whole line would
/// overflow `maxBytes`, the remaining budget is spent on a byte-safe prefix of it before breaking.
fn truncate_head(text: &str, max_bytes: usize, max_lines: usize) -> String {
    let mut output: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    for line in text.split('\n') {
        if output.len() >= max_lines {
            break;
        }
        let separator = usize::from(!output.is_empty());
        let line_bytes = byte_length(line);
        if bytes + separator + line_bytes > max_bytes {
            let remaining = max_bytes.saturating_sub(bytes).saturating_sub(separator);
            if remaining > 0 {
                output.push(truncate_string_to_bytes(line, remaining));
            }
            break;
        }
        output.push(line);
        bytes += separator + line_bytes;
    }
    output.join("\n")
}

/// `mcp-output-guard.ts:251-261` `formatTruncationNotice`. Both wordings are asserted upstream.
fn format_truncation_notice(
    bytes: usize,
    lines: usize,
    path: Option<&str>,
    write_error: Option<&str>,
) -> String {
    let base = format!(
        "[MCP text output truncated: original {} lines / {}.",
        format_thousands(lines as u64),
        format_size(bytes)
    );
    match path {
        Some(path) => format!(
            "{base} Full text saved to: {path} — use read with offset/limit or grep to inspect.]"
        ),
        None => format!(
            "{base} Full output could not be saved: {}]",
            write_error.unwrap_or("unknown error")
        ),
    }
}

/// `mcp-output-guard.ts:358-367` `saveArtifact` (MCP-228).
///
/// Returns `(path, error)`, exactly one of which is `Some`. Every failure is captured and surfaced
/// through the notice rather than thrown, and the directory deliberately **outlives** the call so
/// the model can `read` the path it was just told about — which is why nothing here holds a
/// self-deleting temp-dir guard.
fn save_artifact(kind: &str, text: &str) -> (Option<String>, Option<String>) {
    let write = || -> Result<String, std::io::Error> {
        use std::io::Write as _;
        let dir = make_private_temp_dir(OUTPUT_ARTIFACT_DIR_PREFIX)?;
        let path = dir.join(format!("{kind}-{}.txt", random_hex(4)));
        let mut file = create_private_file(&path)?;
        file.write_all(text.as_bytes())?;
        file.flush()?;
        Ok(path.display().to_string())
    };
    match write() {
        Ok(path) => (Some(path), None),
        Err(err) => (None, Some(err.to_string())),
    }
}

/// `mcp-output-guard.ts:268-273` `boundMcpResult` (MCP-229) — the raw value survives untouched under
/// the threshold.
fn bound_mcp_result(result: &Value, details_max_bytes: usize) -> Value {
    let raw = safe_stringify(result);
    let raw_bytes = byte_length(&raw);
    if raw_bytes <= details_max_bytes {
        return result.clone();
    }
    summarize_mcp_result(result, &raw, raw_bytes)
}

/// `mcp-output-guard.ts:275-307` `summarizeMcpResult`.
fn summarize_mcp_result(result: &Value, raw: &str, raw_bytes: usize) -> Value {
    let (full_result_path, result_write_error) = save_artifact("mcp-result", raw);
    let record = result.as_object();
    let content = record
        .and_then(|r| r.get("content"))
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);

    let mut summary = Map::new();
    summary.insert("omitted".to_string(), Value::Bool(true));
    summary.insert(
        "reason".to_string(),
        Value::String(
            "Raw MCP result exceeded the details size limit and was replaced with this summary to keep session context bounded."
                .to_string(),
        ),
    );
    summary.insert(
        "isError".to_string(),
        Value::Bool(record.and_then(|r| r.get("isError")) == Some(&Value::Bool(true))),
    );
    summary.insert("contentBlocks".to_string(), Value::from(content.len()));
    summary.insert("contentSummary".to_string(), Value::Array(summarize_content(content)));
    summary.insert("rawResultBytes".to_string(), Value::from(raw_bytes));
    if let Some(path) = full_result_path {
        summary.insert("fullResultPath".to_string(), Value::String(path));
    }
    if let Some(err) = result_write_error {
        summary.insert("resultWriteError".to_string(), Value::String(err));
    }

    if let Some(record) = record {
        // `"structuredContent" in record` — key PRESENCE, so an explicit `null` is still summarized.
        if let Some(value) = record.get("structuredContent") {
            summary.insert("structuredContent".to_string(), summarize_value(value));
        }
        if let Some(value) = record.get("_meta") {
            summary.insert("meta".to_string(), summarize_value(value));
        }
        let standard: HashSet<&str> =
            ["content", "isError", "structuredContent", "_meta"].into_iter().collect();
        let extra: Vec<Value> = record
            .iter()
            .filter(|(k, _)| !standard.contains(k.as_str()))
            .take(KEY_PREVIEW_LIMIT)
            .map(|(k, v)| {
                let mut field = Map::new();
                field.insert("key".to_string(), Value::String(truncate_key(k)));
                field.insert("type".to_string(), Value::String(js_typeof(v).to_string()));
                field.insert("estimatedBytes".to_string(), Value::from(estimate_value_bytes(v, 0)));
                field.insert("omitted".to_string(), Value::Bool(true));
                Value::Object(field)
            })
            .collect();
        if !extra.is_empty() {
            summary.insert("extraFields".to_string(), Value::Array(extra));
        }
    }

    Value::Object(summary)
}

/// `mcp-output-guard.ts:309-327` `summarizeContent` — the first 20 blocks plus an
/// `{type:"omitted", count}` tail.
fn summarize_content(content: &[Value]) -> Vec<Value> {
    let mut summaries: Vec<Value> = content
        .iter()
        .take(CONTENT_SUMMARY_LIMIT)
        .map(|block| {
            let Some(record) = block.as_object() else {
                let mut map = Map::new();
                map.insert("type".to_string(), Value::String(js_typeof(block).to_string()));
                map.insert("omitted".to_string(), Value::Bool(true));
                return Value::Object(map);
            };
            let kind = record.get("type").and_then(Value::as_str);
            let mut map = Map::new();
            match kind {
                Some("text") => {
                    let text = record.get("text").and_then(Value::as_str).unwrap_or("");
                    let (bytes, lines) = text_stats(text);
                    map.insert("type".to_string(), Value::String("text".to_string()));
                    map.insert("bytes".to_string(), Value::from(bytes));
                    map.insert("lines".to_string(), Value::from(lines));
                    map.insert("textOmitted".to_string(), Value::Bool(true));
                }
                Some("image") => {
                    let data = record.get("data").and_then(Value::as_str).unwrap_or("");
                    map.insert("type".to_string(), Value::String("image".to_string()));
                    // `mimeType: undefined` is dropped by `JSON.stringify`, so a non-string mime
                    // emits NO key at all rather than a `null`.
                    if let Some(mime) = record.get("mimeType").and_then(Value::as_str) {
                        map.insert("mimeType".to_string(), Value::String(mime.to_string()));
                    }
                    map.insert("dataBytes".to_string(), Value::from(byte_length(data)));
                    map.insert("dataOmitted".to_string(), Value::Bool(true));
                }
                _ => {
                    map.insert(
                        "type".to_string(),
                        Value::String(kind.unwrap_or("unknown").to_string()),
                    );
                    map.insert(
                        "estimatedBytes".to_string(),
                        Value::from(estimate_value_bytes(block, 0)),
                    );
                    map.insert("omitted".to_string(), Value::Bool(true));
                }
            }
            Value::Object(map)
        })
        .collect();
    if content.len() > CONTENT_SUMMARY_LIMIT {
        let mut tail = Map::new();
        tail.insert("type".to_string(), Value::String("omitted".to_string()));
        tail.insert("count".to_string(), Value::from(content.len() - CONTENT_SUMMARY_LIMIT));
        summaries.push(Value::Object(tail));
    }
    summaries
}

/// `mcp-output-guard.ts:329-342` `summarizeValue`.
///
/// An **array** takes the object branch (`asRecord` accepts it), so its `keysPreview` is its indices
/// as strings — reproduced verbatim rather than "fixed".
fn summarize_value(value: &Value) -> Value {
    let mut map = Map::new();
    let keys: Vec<String> = match value {
        Value::Object(o) => o.keys().cloned().collect(),
        Value::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
        _ => {
            map.insert(
                "type".to_string(),
                Value::String(
                    if value.is_null() { "null" } else { js_typeof(value) }.to_string(),
                ),
            );
            map.insert("estimatedBytes".to_string(), Value::from(estimate_value_bytes(value, 0)));
            map.insert("omitted".to_string(), Value::Bool(true));
            return Value::Object(map);
        }
    };
    map.insert(
        "type".to_string(),
        Value::String(if value.is_array() { "array" } else { "object" }.to_string()),
    );
    map.insert("estimatedBytes".to_string(), Value::from(estimate_value_bytes(value, 0)));
    map.insert("keyCount".to_string(), Value::from(keys.len()));
    map.insert(
        "keysPreview".to_string(),
        Value::Array(
            keys.iter().take(KEY_PREVIEW_LIMIT).map(|k| Value::String(truncate_key(k))).collect(),
        ),
    );
    map.insert("omitted".to_string(), Value::Bool(true));
    Value::Object(map)
}

/// `mcp-output-guard.ts:344-352` `estimateValueBytes` — a **depth-2**, 20-entry-wide recursive sum.
///
/// A number contributes the byte length of its `String()`, not 8.
fn estimate_value_bytes(value: &Value, depth: usize) -> usize {
    match value {
        Value::Null => 0,
        Value::String(s) => byte_length(s),
        Value::Number(n) => byte_length(&js_number_string(n)),
        Value::Bool(b) => byte_length(if *b { "true" } else { "false" }),
        Value::Array(items) if depth < 2 => items
            .iter()
            .take(KEY_PREVIEW_LIMIT)
            .map(|item| estimate_value_bytes(item, depth + 1))
            .sum(),
        Value::Object(map) if depth < 2 => map
            .values()
            .take(KEY_PREVIEW_LIMIT)
            .map(|item| estimate_value_bytes(item, depth + 1))
            .sum(),
        Value::Array(_) | Value::Object(_) => 0,
    }
}

/// `mcp-output-guard.ts:354-356` `truncateKey` — 120 chars, the 120th replaced by `…`.
fn truncate_key(key: &str) -> String {
    if utf16_len(key) <= KEY_MAX_CHARS {
        return key.to_string();
    }
    format!("{}…", take_utf16(key, KEY_MAX_CHARS - 1))
}

// ===================================================================================================
// 7 · Render options — `tool-result-renderer.ts:265-273` (MCP-238)
// ===================================================================================================

/// `McpToolRenderOptions` (`tool-result-renderer.ts:40-43`).
///
/// The `renderShell` half of MCP-238 lives on the tool, not here:
/// `Tool::render_kind() -> ToolRenderKind::SelfRendered` **is** upstream's `renderShell: "self"`,
/// selected in compact mode, and `ToolRenderKind::Default` is `"default"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolRenderOptions {
    pub result_rendering: ToolResultRendering,
    /// Whitelisted to 1, 2 or 3; the default is mode-dependent.
    pub collapsed_result_lines: u8,
}

impl Default for McpToolRenderOptions {
    /// `resolveMcpToolRenderOptions()` with no settings: compact, one collapsed line.
    fn default() -> Self {
        Self {
            result_rendering: ToolResultRendering::Compact,
            collapsed_result_lines: DEFAULT_COMPACT_COLLAPSED_LINES,
        }
    }
}

impl McpToolRenderOptions {
    #[must_use]
    const fn is_compact(self) -> bool {
        matches!(self.result_rendering, ToolResultRendering::Compact)
    }
}

/// `tool-result-renderer.ts:265-273` `resolveMcpToolRenderOptions`.
///
/// Both halves are **reused** from [`crate::config::McpSettings`], which already ports the two
/// asymmetries: `toolResultRendering` is `=== "boxed" ? "boxed" : "compact"` (so any other value is
/// compact) and `collapsedResultLines` is a *whitelist* of 1/2/3, so `7` falls back to the mode
/// default rather than clamping to 3.
#[must_use]
pub fn resolve_mcp_tool_render_options(settings: &McpSettings) -> McpToolRenderOptions {
    let result_rendering = settings.tool_result_rendering();
    let boxed = matches!(result_rendering, ToolResultRendering::Boxed);
    McpToolRenderOptions {
        result_rendering,
        collapsed_result_lines: settings.collapsed_result_lines(boxed),
    }
}

// ===================================================================================================
// 8 · Call rows — `tool-result-renderer.ts:214-315` (MCP-237, MCP-243)
// ===================================================================================================

/// `tool-result-renderer.ts:214-242` `formatMcpProxyToolCallLines` — first match wins.
///
/// The `action === "ui-messages"` branch is **cut** (Cut 2); it is the file's only `ui` reference and
/// the seven surviving branches are unchanged. Upstream tests it *first*, above `tool`; here a
/// `ui-messages` action falls through to the generic `mcp {action}` arm, so it renders identically
/// (`"mcp ui-messages"`) when it is the only key and differently when another key outranks it
/// (`{action:"ui-messages", server:"s"}` renders `"mcp list s"`). That divergence is unreachable:
/// the MCP-Apps subsystem is cut, so the `mcp` gateway's parameter schema has no `ui-messages`
/// action for a model to name.
#[must_use]
pub fn format_mcp_proxy_tool_call_lines(args: &Value, max_input_chars: usize) -> Vec<String> {
    if let Some(tool) = truthy_str(args, "tool") {
        let target = match truthy_str(args, "server") {
            Some(server) => format!("{tool} @ {server}"),
            None => tool.to_string(),
        };
        let mut lines = vec![format!("mcp call {target}")];
        if let Some(call_args) = args.get("args").filter(|v| is_truthy(Some(v))) {
            lines.push(format_jsonish(call_args, max_input_chars));
        }
        return lines;
    }
    if let Some(connect) = truthy_str(args, "connect") {
        return vec![format!("mcp connect {connect}")];
    }
    if let Some(describe) = truthy_str(args, "describe") {
        return vec![format!("mcp describe {describe}")];
    }
    if let Some(search) = truthy_str(args, "search") {
        let mut line = format!("mcp search {search}");
        if let Some(server) = truthy_str(args, "server") {
            line.push_str(&format!(" @ {server}"));
        }
        if args.get("regex") == Some(&Value::Bool(true)) {
            line.push_str(" (regex)");
        }
        if args.get("includeSchemas") == Some(&Value::Bool(false)) {
            line.push_str(" (schemas hidden)");
        }
        return vec![line];
    }
    if let Some(server) = truthy_str(args, "server") {
        return vec![format!("mcp list {server}")];
    }
    if let Some(action) = truthy_str(args, "action") {
        return vec![format!("mcp {action}")];
    }
    vec!["mcp status".to_string()]
}

/// `tool-result-renderer.ts:244-251` `formatMcpDirectToolCallLines` — the display name alone unless
/// the arguments are a non-array object with at least one key.
#[must_use]
pub fn format_mcp_direct_tool_call_lines(
    display_name: &str,
    args: &Value,
    max_input_chars: usize,
) -> Vec<String> {
    if !has_useful_object_content(args) {
        return vec![display_name.to_string()];
    }
    vec![display_name.to_string(), format_jsonish(args, max_input_chars)]
}

/// `tool-result-renderer.ts:253-259` `renderToolCallLines` — the first line is the title (defaulting
/// to `"mcp"`) and the rest are muted, joined into one `Text`.
///
/// MCP-244: `theme.fg(...)`/`theme.bold(...)` have no styled node to emit into, so this is upstream's
/// own `plainTheme` path. MCP-243: upstream returns an `EmptyComponent` here in compact mode after
/// stashing the title for the result row to re-print; cyrup has no shared row context, so the call
/// row is always drawn and the two-row shape is used under both settings.
#[must_use]
fn render_tool_call_lines(lines: &[String]) -> Value {
    let title = lines.first().map_or("mcp", String::as_str);
    let mut out = vec![title.to_string()];
    out.extend(lines.iter().skip(1).cloned());
    text_widget(&out.join("\n"))
}

// ===================================================================================================
// 9 · Result rows — `tool-result-renderer.ts:317-454` (MCP-239..MCP-242)
// ===================================================================================================

/// `McpToolResultDisplay` (`tool-result-renderer.ts:50-53`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct McpToolResultDisplay {
    pub lines: Vec<String>,
    pub truncated: bool,
}

/// `tool-result-renderer.ts:317-322` `blockToLines`.
fn block_to_lines(block: &McpContentBlock) -> Vec<String> {
    match block {
        McpContentBlock::Text(text) => text.split('\n').map(ToString::to_string).collect(),
        McpContentBlock::Image { mime_type, .. } => vec![format!("[image: {mime_type}]")],
    }
}

/// `tool-result-renderer.ts:324-387` `collectCollapsedResultLines` (MCP-239).
///
/// Four details decide correctness and all four are asserted upstream:
///
/// * the char budget is JS `String.length` — UTF-16 code units, not bytes and not `char`s;
/// * a text block is split on `"\n"` **without materializing the array** (an `indexOf` walk), which
///   matters only for allocation but is kept because a 60 MiB block would otherwise be split in full
///   before the first line is even measured;
/// * while nothing has been pushed yet, a **leading blank** line is charged against the budget and
///   *skipped* rather than pushed — and a leading blank longer than the whole remaining budget ends
///   the walk with `(leading blank output omitted)`;
/// * a lone `"…"` is appended only when truncated **and** already at the line cap.
#[must_use]
pub fn collect_collapsed_result_lines(
    content: &[McpContentBlock],
    max_lines: usize,
    max_chars: usize,
) -> McpToolResultDisplay {
    if content.is_empty() {
        return McpToolResultDisplay {
            lines: vec!["(empty result)".to_string()],
            truncated: false,
        };
    }

    // `appendLine`'s three mutable captures (`lines`, `remainingChars`, `truncated`) live on a
    // struct rather than in a closure: a `FnMut` closure capturing `truncated` would hold the
    // borrow across the walk's own `if truncated { break }` test.
    struct Collector {
        lines: Vec<String>,
        remaining: usize,
        truncated: bool,
        max_lines: usize,
    }
    impl Collector {
        /// Returns `false` to stop the whole walk, exactly as upstream's `appendLine` does.
        fn append_line(&mut self, line: &str) -> bool {
            let length = utf16_len(line);
            if self.lines.is_empty() {
                let preview_width = length.min(self.remaining);
                if take_utf16(line, preview_width).trim().is_empty() {
                    if length >= self.remaining {
                        self.truncated = true;
                        self.remaining = 0;
                        return false;
                    }
                    self.remaining -= length + 1;
                    return true;
                }
            }
            if self.lines.len() >= self.max_lines || self.remaining == 0 {
                self.truncated = true;
                return false;
            }
            if length > self.remaining {
                self.lines.push(take_utf16(line, self.remaining).to_string());
                self.truncated = true;
                self.remaining = 0;
                return false;
            }
            self.lines.push(line.to_string());
            // `tool-result-renderer.ts:383` is a bare `remainingChars -= line.length + 1`, which
            // reaches `-1` when the pushed line is exactly as long as the budget. That `-1` is only
            // ever read by `:348`'s `remainingChars <= 0`, which stops the walk — so saturating at
            // `0` reproduces it exactly, and is the ONLY faithful spelling here: `usize` has no
            // `-1`, a wrapped `usize::MAX` never satisfies this port's `remaining == 0` guard, and
            // the budget would silently become unbounded (release) or panic (debug).
            self.remaining = self.remaining.saturating_sub(length + 1);
            true
        }
    }

    let mut state =
        Collector { lines: Vec::new(), remaining: max_chars, truncated: false, max_lines };

    for block in content {
        let McpContentBlock::Text(text) = block else {
            let McpContentBlock::Image { mime_type, .. } = block else { continue };
            if !state.append_line(&format!("[image: {mime_type}]")) {
                break;
            }
            continue;
        };
        // Split on `"\n"` WITHOUT materializing the array (upstream's `indexOf` walk): a 60 MiB
        // block would otherwise be fully split before its first line is even measured.
        let mut start = 0usize;
        while start <= text.len() {
            let rest = text.get(start..).unwrap_or("");
            let (line, next) = match rest.find('\n') {
                Some(idx) => (rest.get(..idx).unwrap_or(""), Some(start + idx + 1)),
                None => (rest, None),
            };
            if !state.append_line(line) {
                break;
            }
            match next {
                Some(next) => start = next,
                None => break,
            }
        }
        if state.truncated {
            break;
        }
    }

    let Collector { mut lines, truncated, .. } = state;
    if lines.is_empty() {
        lines.push(if truncated { "(leading blank output omitted)" } else { "" }.to_string());
    }
    if truncated && lines.len() >= max_lines {
        lines.push("…".to_string());
    }
    McpToolResultDisplay { lines, truncated }
}

/// `tool-result-renderer.ts:403-416` `formatMcpToolResultLines` — expanded is never truncated.
#[must_use]
pub fn format_mcp_tool_result_lines(
    content: &[McpContentBlock],
    expanded: bool,
    max_collapsed_lines: usize,
    max_collapsed_chars: usize,
) -> McpToolResultDisplay {
    if !expanded {
        return collect_collapsed_result_lines(content, max_collapsed_lines, max_collapsed_chars);
    }
    let all: Vec<String> = content.iter().flat_map(block_to_lines).collect();
    let lines = if all.is_empty() { vec!["(empty result)".to_string()] } else { all };
    McpToolResultDisplay { lines, truncated: false }
}

/// `tool-result-renderer.ts:389-401` `formatMcpToolResultIdentity` (MCP-240).
///
/// `null` unless `details.mode === "call"`, which **only the proxy's `executeCall` sets** — a direct
/// tool's `details` carry `server`/`tool` but no `mode`, so a direct tool never renders an identity
/// line. Server resolution is `server` then `hintServer`; the name is `tool`, then `resourceUri`,
/// then `requestedTool`, in that order.
#[must_use]
pub fn format_mcp_tool_result_identity(details: Option<&Value>) -> Option<String> {
    let details = details?;
    if details.get("mode").and_then(Value::as_str) != Some("call") {
        return None;
    }
    let server = details
        .get("server")
        .and_then(Value::as_str)
        .or_else(|| details.get("hintServer").and_then(Value::as_str))?;
    if let Some(tool) = details.get("tool").and_then(Value::as_str) {
        return Some(format!("MCP {server}/{tool}"));
    }
    if let Some(uri) = details.get("resourceUri").and_then(Value::as_str) {
        return Some(format!("MCP {server} resource {uri}"));
    }
    if let Some(requested) = details.get("requestedTool").and_then(Value::as_str) {
        return Some(format!("MCP {server}/{requested}"));
    }
    None
}

/// `tool-result-renderer.ts:418-454` `renderMcpToolResult` (MCP-241, MCP-242).
///
/// `result` is the serialized `AgentToolResult` the host hands the renderer — `{content, details, …}`
/// as built by `cyrup_agent`'s `result_value_of`.
///
/// `expanded` is MCP-242's port of `options.expanded || context.isError === true ||
/// Boolean(details.error)`: the first disjunct is `HostServices::tools_expanded()` (cyrup's expand
/// model is global rather than per row, so there is no per-row flag to pass), the second has no seam
/// and is folded into the third, and the third is read straight off the payload. **Any truthy
/// `details.error` forces the expanded rendering** regardless of the toggle — that is the arm
/// MCP-249's twelve error codes feed.
#[must_use]
pub fn render_mcp_tool_result(
    result: &Value,
    is_partial: bool,
    tools_expanded: bool,
    options: McpToolRenderOptions,
) -> Value {
    // 1. Unreachable from today's seam (`extension_render` routes no `ToolExecutionUpdate`), ported
    //    so the mechanism survives if the host ever does.
    if is_partial {
        return text_widget("Running MCP tool...");
    }

    let details = result.get("details");
    let has_error_details = is_truthy(details.and_then(|d| d.get("error")));
    let expanded = tools_expanded || has_error_details;

    let content: Vec<McpContentBlock> = result
        .get("content")
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().map(McpContentBlock::from_result_value).collect())
        .unwrap_or_default();
    let collapsed_lines = usize::from(options.collapsed_result_lines);

    // 2. Compact, settled, not expanded — upstream's `CompactMcpToolResult`.
    if !expanded && options.is_compact() {
        let display =
            format_mcp_tool_result_lines(&content, false, collapsed_lines, DEFAULT_MAX_COLLAPSED_CHARS);
        let title = format_mcp_tool_result_identity(details).unwrap_or_default();
        return compact_result_widget(&title, &display);
    }

    // 3. Boxed, or expanded — upstream's `CollapsibleText`.
    let display =
        format_mcp_tool_result_lines(&content, expanded, collapsed_lines, DEFAULT_MAX_COLLAPSED_CHARS);
    let identity = format_mcp_tool_result_identity(details);
    let mut lines: Vec<String> = identity.iter().cloned().collect();
    lines.extend(display.lines.iter().cloned());

    if expanded {
        return text_widget(&lines.join("\n"));
    }

    // `CollapsibleText`'s collapsed arm, with the width-derived `charBudget` re-slice removed: the
    // body has already been collapsed by line count and char budget above, and with no width there
    // is no wrapping for the re-slice to account for.
    let max_collapsed = collapsed_lines + usize::from(identity.is_some());
    if !display.truncated && lines.len() <= max_collapsed {
        return text_widget(&lines.join("\n"));
    }
    let mut rendered: Vec<String> = lines.into_iter().take(max_collapsed).collect();
    rendered.push("…".to_string());
    rendered.push("(Ctrl+O to expand)".to_string());
    text_widget(&rendered.join("\n"))
}

/// `tool-result-renderer.ts:70-128` `CompactMcpToolResult.render(width)`, width-free (MCP-241).
///
/// What survives: the trailing `"…"` line is dropped when the display was truncated, the first line
/// carries the `"{title} → "` prefix when there is a title, and a truncated body earns the
/// `" … (Ctrl+O to expand)"` affordance.
///
/// What is lost, stated once: with no width crossing the seam there is no
/// `visibleWidth(body) > safeWidth` test, so `hiddenText` reduces to `display.truncated`; and the
/// three-way *choice* between the 21-char suffix, the 9-char `" (Ctrl+O)"` and a bare truncated hint
/// collapses to the long form. The row is emitted as a `truncated-text` node so the host clips it to
/// the width only the host knows. MCP-243 also removes the `compactInputPreview` half of the prefix,
/// which upstream sourced from the call row's stashed state.
fn compact_result_widget(title: &str, display: &McpToolResultDisplay) -> Value {
    let mut lines: Vec<String> = display.lines.clone();
    if display.truncated && lines.last().is_some_and(|l| l == "…") {
        lines.pop();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    if !title.is_empty()
        && let Some(first) = lines.first_mut()
    {
        *first = format!("{title} → {first}");
    }
    if display.truncated
        && let Some(last) = lines.last_mut()
    {
        last.push_str(" … (Ctrl+O to expand)");
    }
    truncated_text_widget(&lines.join("\n"))
}

// ===================================================================================================
// 10 · The widget vocabulary and the two seam entry points
// ===================================================================================================

/// `{"widget":"text","text":…}` — the dominant node of the host's flattener vocabulary
/// (`cyrup-tui/src/app/extension_render.rs`).
fn text_widget(text: &str) -> Value {
    let mut map = Map::new();
    map.insert("widget".to_string(), Value::String("text".to_string()));
    map.insert("text".to_string(), Value::String(text.to_string()));
    Value::Object(map)
}

/// `{"widget":"truncated-text","text":…}` — the node that tells the host it may clip.
fn truncated_text_widget(text: &str) -> Value {
    let mut map = Map::new();
    map.insert("widget".to_string(), Value::String("truncated-text".to_string()));
    map.insert("text".to_string(), Value::String(text.to_string()));
    Value::Object(map)
}

/// [`cyrup_ext::native::NativeExtension::render_call`]'s body, for `crate::extension::McpExtension`
/// to delegate to.
///
/// `key` is the registered tool name: [`crate::registration::PROXY_TOOL_NAME`] selects the gateway
/// formatter and every other name is a **direct tool**, whose display name is that same registered,
/// *prefixed* name — upstream's `createMcpDirectToolCallRenderer(spec.prefixedName, …)`.
///
/// Only tool names this extension declared a renderer for ever reach here (the host routes by
/// `tool_renderer_owner`), so there is no "is this mine" test to perform beyond that split.
#[must_use]
pub fn render_call(key: &str, call: &Value, options: McpToolRenderOptions) -> Option<Value> {
    let _ = options; // MCP-243: the compact fork is a call-row *suppression* upstream; cyrup keeps both rows.
    let lines = if key == crate::registration::PROXY_TOOL_NAME {
        format_mcp_proxy_tool_call_lines(call, DEFAULT_MAX_CALL_INPUT_CHARS)
    } else {
        format_mcp_direct_tool_call_lines(key, call, DEFAULT_MAX_CALL_INPUT_CHARS)
    };
    Some(render_tool_call_lines(&lines))
}

/// [`cyrup_ext::native::NativeExtension::render_result`]'s body.
///
/// `tools_expanded` is `HostServices::tools_expanded()` read off the `Arc` the extension stashed in
/// `set_host_services` (MCP-350a) — the caller reads it because this function must stay sync and
/// allocation-bounded (`EXTENSION_RENDER_TIMEOUT` is 2 s and the call is aborted, not detached,
/// when it expires).
#[must_use]
pub fn render_result(
    key: &str,
    result: &Value,
    options: McpToolRenderOptions,
    tools_expanded: bool,
) -> Option<Value> {
    let _ = key; // Every MCP tool — gateway and direct — renders its result identically.
    Some(render_mcp_tool_result(result, false, tools_expanded, options))
}

#[cfg(test)]
// Same posture as the crate's other test modules (`runtime.rs:1408`): a failed assertion in a test
// IS a panic, and the no-panic policy is about the shipped surface.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(text: &str) -> McpContentBlock {
        McpContentBlock::Text(text.to_string())
    }

    fn result_value(content: Value) -> Value {
        json!({ "content": content })
    }

    // --- MCP-237: the call-row formatters ------------------------------------------------------

    #[test]
    fn proxy_call_with_json_string_arguments_preserves_key_order() {
        // `__tests__/tool-result-renderer.test.ts:25-36` — and the key order is the ORDER WRITTEN,
        // which a `serde_json::Value` round-trip would sort into `accountId, scriptName` by luck
        // and `scriptName, accountId` would not survive.
        let lines = format_mcp_proxy_tool_call_lines(
            &json!({
                "tool": "cf-portal_list_worker_tail_events",
                "server": "cf-portal",
                "args": "{\"scriptName\":\"worker\",\"accountId\":\"abc\"}",
            }),
            DEFAULT_MAX_CALL_INPUT_CHARS,
        );
        assert_eq!(
            lines,
            vec![
                "mcp call cf-portal_list_worker_tail_events @ cf-portal".to_string(),
                "{\n  \"scriptName\": \"worker\",\n  \"accountId\": \"abc\"\n}".to_string(),
            ]
        );
    }

    #[test]
    fn proxy_call_with_object_arguments() {
        // `:38-48`. `limit: 10` must render as `10`, not `10.0`.
        let lines = format_mcp_proxy_tool_call_lines(
            &json!({ "tool": "cf-portal_list_worker_tail_events", "args": { "accountId": "abc", "limit": 10 } }),
            DEFAULT_MAX_CALL_INPUT_CHARS,
        );
        assert_eq!(
            lines,
            vec![
                "mcp call cf-portal_list_worker_tail_events".to_string(),
                "{\n  \"accountId\": \"abc\",\n  \"limit\": 10\n}".to_string(),
            ]
        );
    }

    #[test]
    fn proxy_discovery_branches() {
        // `:50-57` plus the cut `ui-messages` arm, which the generic `action` arm reproduces.
        assert_eq!(
            format_mcp_proxy_tool_call_lines(
                &json!({ "search": "tail events", "server": "cf-portal", "regex": true }),
                DEFAULT_MAX_CALL_INPUT_CHARS
            ),
            vec!["mcp search tail events @ cf-portal (regex)".to_string()]
        );
        assert_eq!(
            format_mcp_proxy_tool_call_lines(
                &json!({ "search": "x", "includeSchemas": false }),
                DEFAULT_MAX_CALL_INPUT_CHARS
            ),
            vec!["mcp search x (schemas hidden)".to_string()]
        );
        assert_eq!(
            format_mcp_proxy_tool_call_lines(&json!({ "connect": "cf-portal" }), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["mcp connect cf-portal".to_string()]
        );
        assert_eq!(
            format_mcp_proxy_tool_call_lines(&json!({ "describe": "cf-portal" }), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["mcp describe cf-portal".to_string()]
        );
        assert_eq!(
            format_mcp_proxy_tool_call_lines(&json!({ "server": "cf-portal" }), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["mcp list cf-portal".to_string()]
        );
        // Cut 2 removed upstream's first-position `ui-messages` arm (`:218`), so a bare
        // `ui-messages` still renders identically through the generic `action` arm …
        assert_eq!(
            format_mcp_proxy_tool_call_lines(&json!({ "action": "ui-messages" }), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["mcp ui-messages".to_string()]
        );
        // … while one paired with a higher-precedence key now takes that key's arm. Unreachable in
        // the port: the gateway's schema has no `ui-messages` action.
        assert_eq!(
            format_mcp_proxy_tool_call_lines(
                &json!({ "action": "ui-messages", "server": "cf-portal" }),
                DEFAULT_MAX_CALL_INPUT_CHARS
            ),
            vec!["mcp list cf-portal".to_string()]
        );
        assert_eq!(
            format_mcp_proxy_tool_call_lines(&json!({}), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["mcp status".to_string()]
        );
    }

    #[test]
    fn direct_call_rows() {
        // `:63-77`.
        assert_eq!(
            format_mcp_direct_tool_call_lines(
                "cf-portal_list_worker_tail_events",
                &json!({ "accountId": "abc", "scriptName": "worker" }),
                DEFAULT_MAX_CALL_INPUT_CHARS
            ),
            vec![
                "cf-portal_list_worker_tail_events".to_string(),
                "{\n  \"accountId\": \"abc\",\n  \"scriptName\": \"worker\"\n}".to_string(),
            ]
        );
        assert_eq!(
            format_mcp_direct_tool_call_lines("cf-portal_status", &json!({}), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["cf-portal_status".to_string()]
        );
        // An ARRAY is not "useful object content".
        assert_eq!(
            format_mcp_direct_tool_call_lines("t", &json!([1, 2]), DEFAULT_MAX_CALL_INPUT_CHARS),
            vec!["t".to_string()]
        );
    }

    #[test]
    fn truncate_text_cuts_to_max_minus_one_and_appends_an_ellipsis() {
        assert_eq!(truncate_text("abcde", 5), "abcde");
        assert_eq!(truncate_text("abcdef", 5), "abcd…");
        assert_eq!(truncate_text("abc", 0), "…");
    }

    // --- MCP-239: the collapsed body -----------------------------------------------------------

    #[test]
    fn collapsed_shows_three_lines_and_an_ellipsis() {
        // `:81-91`.
        let display = format_mcp_tool_result_lines(
            &[text("one\ntwo\nthree\nfour")],
            false,
            usize::from(DEFAULT_BOXED_COLLAPSED_LINES),
            DEFAULT_MAX_COLLAPSED_CHARS,
        );
        assert_eq!(display.lines, vec!["one", "two", "three", "…"]);
        assert!(display.truncated);
    }

    #[test]
    fn collapsed_adds_no_ellipsis_at_or_under_the_cap() {
        // `:92-102`.
        let display = format_mcp_tool_result_lines(
            &[text("one\ntwo\nthree")],
            false,
            usize::from(DEFAULT_BOXED_COLLAPSED_LINES),
            DEFAULT_MAX_COLLAPSED_CHARS,
        );
        assert_eq!(display.lines, vec!["one", "two", "three"]);
        assert!(!display.truncated);
    }

    #[test]
    fn expanded_shows_everything_and_images_are_placeholders() {
        // `:103-121`.
        let display = format_mcp_tool_result_lines(
            &[
                text("before"),
                McpContentBlock::Image {
                    data: "abc".to_string(),
                    mime_type: "image/png".to_string(),
                },
            ],
            true,
            3,
            DEFAULT_MAX_COLLAPSED_CHARS,
        );
        assert_eq!(display.lines, vec!["before", "[image: image/png]"]);
        assert!(!display.truncated);
    }

    #[test]
    fn empty_content_is_the_empty_result_placeholder() {
        // `:123-127`.
        let display =
            format_mcp_tool_result_lines(&[], false, 3, DEFAULT_MAX_COLLAPSED_CHARS);
        assert_eq!(display.lines, vec!["(empty result)"]);
        assert!(!display.truncated);
    }

    #[test]
    fn leading_blank_lines_are_skipped_then_bounded() {
        // `:184-198` — both arms.
        let display =
            format_mcp_tool_result_lines(&[text("\n\nuseful\nextra")], false, 1, DEFAULT_MAX_COLLAPSED_CHARS);
        assert_eq!(display.lines, vec!["useful", "…"]);
        assert!(display.truncated);

        let blanks = "\n".repeat(100);
        let display = format_mcp_tool_result_lines(&[text(&format!("{blanks}useful"))], false, 1, 50);
        assert_eq!(display.lines, vec!["(leading blank output omitted)", "…"]);
        assert!(display.truncated);
    }

    #[test]
    fn a_line_that_lands_exactly_on_the_char_budget_does_not_underflow() {
        // Regression, found while reconciling `6686b12` (which only supplies context here).
        // `tool-result-renderer.ts:383` lets `remainingChars` fall to `-1` when a pushed line is
        // exactly as long as the budget, and catches it on the next call with `remainingChars <= 0`
        // (`:348`). `usize` has no `-1`: the port's guard is `remaining == 0`, which a wrapped
        // `usize::MAX` never satisfies, so the budget would become unbounded (release) or panic
        // (debug). `saturating_sub` lands on `0`, which is the same guard upstream's `-1` trips.
        let display = format_mcp_tool_result_lines(&[text("abcde")], false, 3, 5);
        assert_eq!(display.lines, vec!["abcde"]);
        assert!(!display.truncated);

        // The budget really is spent: a second line is refused rather than admitted by a
        // wrapped-around `remaining`.
        let display = format_mcp_tool_result_lines(&[text("abcde\nfghij")], false, 3, 5);
        assert_eq!(display.lines, vec!["abcde"]);
        assert!(display.truncated);
    }

    #[test]
    fn five_blocks_of_three_lines_with_max_three_yields_four_lines() {
        // gap-analysis 13e MCP-239's own `verify` line.
        let content: Vec<McpContentBlock> = (0..5).map(|i| text(&format!("a{i}\nb{i}\nc{i}"))).collect();
        let display =
            format_mcp_tool_result_lines(&content, false, 3, DEFAULT_MAX_COLLAPSED_CHARS);
        assert_eq!(display.lines.len(), 4);
        assert_eq!(display.lines.last().map(String::as_str), Some("…"));
    }

    #[test]
    fn a_single_line_over_the_char_budget_is_sliced() {
        let display = format_mcp_tool_result_lines(&[text(&"x".repeat(200))], false, 3, 50);
        assert_eq!(display.lines, vec!["x".repeat(50)]);
        assert!(display.truncated);
    }

    // --- MCP-240: the identity line ------------------------------------------------------------

    #[test]
    fn identity_line_branches() {
        // `:138-144`.
        assert_eq!(
            format_mcp_tool_result_identity(Some(
                &json!({ "mode": "call", "server": "figma", "tool": "get_nodes" })
            )),
            Some("MCP figma/get_nodes".to_string())
        );
        assert_eq!(
            format_mcp_tool_result_identity(Some(
                &json!({ "mode": "call", "server": "files", "resourceUri": "file://demo" })
            )),
            Some("MCP files resource file://demo".to_string())
        );
        assert_eq!(
            format_mcp_tool_result_identity(Some(
                &json!({ "mode": "call", "hintServer": "figma", "requestedTool": "figma_get_nodes" })
            )),
            Some("MCP figma/figma_get_nodes".to_string())
        );
        assert_eq!(
            format_mcp_tool_result_identity(Some(
                &json!({ "mode": "list", "server": "figma", "tool": "get_nodes" })
            )),
            None
        );
        // A DIRECT tool's details carry server/tool but no `mode`, so it never renders an identity.
        assert_eq!(
            format_mcp_tool_result_identity(Some(&json!({ "server": "figma", "tool": "get_nodes" }))),
            None
        );
    }

    // --- MCP-238: the render options -----------------------------------------------------------

    #[test]
    fn render_options_default_and_whitelist() {
        // `:265-275`, and MCP-238's `verify`: `collapsedResultLines: 7` → the MODE default.
        let settings = McpSettings::default();
        assert_eq!(
            resolve_mcp_tool_render_options(&settings),
            McpToolRenderOptions {
                result_rendering: ToolResultRendering::Compact,
                collapsed_result_lines: 1
            }
        );

        let boxed: McpSettings =
            serde_json::from_value(json!({ "toolResultRendering": "boxed" })).unwrap_or_default();
        assert_eq!(
            resolve_mcp_tool_render_options(&boxed),
            McpToolRenderOptions {
                result_rendering: ToolResultRendering::Boxed,
                collapsed_result_lines: 3
            }
        );

        let seven: McpSettings =
            serde_json::from_value(json!({ "collapsedResultLines": 7 })).unwrap_or_default();
        assert_eq!(resolve_mcp_tool_render_options(&seven).collapsed_result_lines, 1);

        let two: McpSettings =
            serde_json::from_value(json!({ "collapsedResultLines": 2 })).unwrap_or_default();
        assert_eq!(resolve_mcp_tool_render_options(&two).collapsed_result_lines, 2);
    }

    // --- MCP-241 / MCP-242 / MCP-243: the emitted widget trees ----------------------------------

    #[test]
    fn compact_result_is_one_truncated_text_node_with_the_affordance() {
        let result = result_value(json!([{ "type": "text", "text": "one\ntwo\nthree" }]));
        let widget = render_mcp_tool_result(&result, false, false, McpToolRenderOptions::default());
        assert_eq!(widget.get("widget").and_then(Value::as_str), Some("truncated-text"));
        let drawn = widget.get("text").and_then(Value::as_str).unwrap_or_default();
        assert_eq!(drawn, "one … (Ctrl+O to expand)");
    }

    #[test]
    fn compact_result_that_fits_carries_no_affordance() {
        let result = result_value(json!([{ "type": "text", "text": "ok" }]));
        let widget = render_mcp_tool_result(&result, false, false, McpToolRenderOptions::default());
        assert_eq!(widget.get("text").and_then(Value::as_str), Some("ok"));
    }

    #[test]
    fn compact_result_prefixes_the_proxy_identity() {
        let mut result = result_value(json!([{ "type": "text", "text": "ok" }]));
        if let Some(map) = result.as_object_mut() {
            map.insert(
                "details".to_string(),
                json!({ "mode": "call", "server": "figma", "tool": "get_nodes" }),
            );
        }
        let widget = render_mcp_tool_result(&result, false, false, McpToolRenderOptions::default());
        assert_eq!(
            widget.get("text").and_then(Value::as_str),
            Some("MCP figma/get_nodes → ok")
        );
    }

    #[test]
    fn boxed_collapsed_result_carries_the_identity_and_the_footer() {
        // `:228-243`.
        let mut result = result_value(json!([{ "type": "text", "text": "one\ntwo\nthree\nfour" }]));
        if let Some(map) = result.as_object_mut() {
            map.insert(
                "details".to_string(),
                json!({ "mode": "call", "server": "figma", "tool": "get_nodes" }),
            );
        }
        let options = McpToolRenderOptions {
            result_rendering: ToolResultRendering::Boxed,
            collapsed_result_lines: 3,
        };
        let widget = render_mcp_tool_result(&result, false, false, options);
        let drawn = widget.get("text").and_then(Value::as_str).unwrap_or_default();
        assert!(drawn.contains("MCP figma/get_nodes"));
        assert!(drawn.contains("one") && drawn.contains("two") && drawn.contains("three"));
        assert!(!drawn.contains("four"));
        assert!(drawn.contains("(Ctrl+O to expand)"));
    }

    #[test]
    fn details_error_forces_the_expanded_form_even_when_collapsed() {
        // `:292-302` and MCP-242's `verify`.
        let mut result = result_value(json!([{ "type": "text", "text": "line 1\nline 2\nline 3\nline 4" }]));
        if let Some(map) = result.as_object_mut() {
            map.insert("details".to_string(), json!({ "error": "tool_error" }));
        }
        let widget = render_mcp_tool_result(&result, false, false, McpToolRenderOptions::default());
        let drawn = widget.get("text").and_then(Value::as_str).unwrap_or_default();
        assert!(drawn.contains("line 4"));
        assert!(!drawn.contains("Ctrl+O to expand"));
        assert_eq!(widget.get("widget").and_then(Value::as_str), Some("text"));
    }

    #[test]
    fn the_global_expand_toggle_expands_a_clean_result() {
        // MCP-242's second `verify` clause.
        let result = result_value(json!([{ "type": "text", "text": "a\nb\nc\nd" }]));
        let widget = render_mcp_tool_result(&result, false, true, McpToolRenderOptions::default());
        assert_eq!(widget.get("text").and_then(Value::as_str), Some("a\nb\nc\nd"));
    }

    #[test]
    fn a_falsy_details_error_does_not_expand() {
        // `Boolean(details.error)`: `""`, `0`, `false` and `null` are all falsy.
        for falsy in [json!(""), json!(0), json!(false), json!(null)] {
            let mut result = result_value(json!([{ "type": "text", "text": "a\nb" }]));
            if let Some(map) = result.as_object_mut() {
                map.insert("details".to_string(), json!({ "error": falsy }));
            }
            let widget =
                render_mcp_tool_result(&result, false, false, McpToolRenderOptions::default());
            assert_eq!(widget.get("widget").and_then(Value::as_str), Some("truncated-text"));
        }
    }

    #[test]
    fn both_rows_are_drawn_in_compact_mode() {
        // MCP-243's `verify`: neither row is empty.
        let call = render_call("srv_search", &json!({ "q": "x" }), McpToolRenderOptions::default());
        let result = render_result(
            "srv_search",
            &result_value(json!([{ "type": "text", "text": "found" }])),
            McpToolRenderOptions::default(),
            false,
        );
        assert_eq!(
            call.as_ref().and_then(|v| v.get("text")).and_then(Value::as_str),
            Some("srv_search\n{\n  \"q\": \"x\"\n}")
        );
        assert_eq!(
            result.as_ref().and_then(|v| v.get("text")).and_then(Value::as_str),
            Some("found")
        );
    }

    #[test]
    fn render_call_routes_the_gateway_to_the_proxy_formatter() {
        let widget = render_call(
            crate::registration::PROXY_TOOL_NAME,
            &json!({ "connect": "srv" }),
            McpToolRenderOptions::default(),
        );
        assert_eq!(
            widget.as_ref().and_then(|v| v.get("text")).and_then(Value::as_str),
            Some("mcp connect srv")
        );
    }

    #[test]
    fn the_partial_arm_is_ported_even_though_the_seam_never_takes_it() {
        let widget =
            render_mcp_tool_result(&json!({}), true, false, McpToolRenderOptions::default());
        assert_eq!(widget.get("text").and_then(Value::as_str), Some("Running MCP tool..."));
    }

    #[test]
    fn no_escape_sequences_reach_the_flattened_string() {
        // MCP-244's `verify`: the plainTheme path emits no SGR.
        let result = result_value(json!([{ "type": "text", "text": "plain" }]));
        let widget = render_mcp_tool_result(&result, false, false, McpToolRenderOptions::default());
        let drawn = widget.get("text").and_then(Value::as_str).unwrap_or_default();
        assert!(!drawn.contains('\u{1b}'));
    }

    // --- MCP-220 / MCP-221 / MCP-222: content transformation -----------------------------------

    #[test]
    fn every_standard_content_type() {
        let blocks = transform_mcp_content(
            &[
                json!({ "type": "text", "text": "hi" }),
                json!({ "type": "image", "data": "AAA" }),
                json!({ "type": "resource", "resource": { "uri": "file://a", "text": "body" } }),
                json!({ "type": "resource" }),
                json!({ "type": "resource_link", "uri": "file://b", "name": "B" }),
                json!({ "type": "resource_link" }),
                json!({ "type": "audio" }),
                json!({ "type": "video", "x": 1 }),
            ],
            None,
        );
        assert_eq!(blocks.len(), 8);
        assert_eq!(blocks.first(), Some(&text("hi")));
        assert_eq!(
            blocks.get(1),
            Some(&McpContentBlock::Image {
                data: "AAA".to_string(),
                mime_type: "image/png".to_string()
            })
        );
        assert_eq!(blocks.get(2), Some(&text("[Resource: file://a]\nbody")));
        assert_eq!(blocks.get(3), Some(&text("[Resource: (no URI)]\n(no content)")));
        assert_eq!(blocks.get(4), Some(&text("[Resource Link: B]\nURI: file://b")));
        assert_eq!(blocks.get(5), Some(&text("[Resource Link: unknown]\nURI: (no URI)")));
        assert_eq!(blocks.get(6), Some(&text("[Audio content: audio/*]")));
        assert_eq!(blocks.get(7), Some(&text("{\"type\":\"video\",\"x\":1}")));
    }

    #[test]
    fn a_resource_without_text_stringifies_the_whole_record() {
        let blocks =
            transform_mcp_content(&[json!({ "type": "resource", "resource": { "uri": "u", "mimeType": "m" } })], None);
        assert_eq!(
            blocks.first(),
            Some(&text("[Resource: u]\n{\"mimeType\":\"m\",\"uri\":\"u\"}"))
        );
    }

    #[test]
    fn resource_contents_prefer_text_then_stringify_the_record() {
        let blocks = transform_mcp_resource_contents(
            &[json!({ "uri": "u", "text": "body" }), json!({ "uri": "u2", "mimeType": "m" })],
            None,
        );
        assert_eq!(blocks.first(), Some(&text("body")));
        assert_eq!(blocks.get(1), Some(&text("{\"mimeType\":\"m\",\"uri\":\"u2\"}")));
    }

    #[test]
    fn structured_content_is_the_fallback_only_for_an_empty_block_list() {
        let blocks =
            resolve_mcp_result_content(&json!({ "content": [], "structuredContent": { "a": 1 } }), None);
        assert_eq!(blocks, vec![text("{\n  \"a\": 1\n}")]);

        let blocks = resolve_mcp_result_content(
            &json!({ "content": [{ "type": "text", "text": "x" }], "structuredContent": { "a": 1 } }),
            None,
        );
        assert_eq!(blocks, vec![text("x")]);

        assert!(resolve_mcp_result_content(&json!({ "content": [], "structuredContent": null }), None).is_empty());
        assert!(resolve_mcp_result_content(&json!({ "content": [] }), None).is_empty());
    }

    // --- MCP-223: materialization --------------------------------------------------------------

    #[test]
    fn an_oversized_blob_is_omitted_without_being_decoded() {
        let session = MaterializedResources::new(None);
        // 11 MiB decoded ⇒ ~14.7 MiB of base64, all of it ASCII 'A'.
        let blob = "A".repeat(11 * 1024 * 1024 * 4 / 3 + 8);
        let out = session.materialize(Some("file://big"), Some("application/pdf"), &blob);
        assert_eq!(
            out,
            "[Resource: file://big]\nBinary content omitted: decoded size exceeds 10 MiB\nMIME type: application/pdf"
        );
    }

    #[test]
    fn a_cancelled_scope_omits_with_runtime_stopped() {
        let token = cyrup_core::CancelToken::new();
        token.cancel();
        let session = MaterializedResources::new(Some(token));
        let out = session.materialize(None, None, "QUJD");
        assert_eq!(
            out,
            "[Resource: (no URI)]\nBinary content omitted: runtime stopped\nMIME type: application/octet-stream"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_materialized_resource_is_0600_inside_a_0700_directory() {
        use std::os::unix::fs::PermissionsExt as _;
        let session = MaterializedResources::new(None);
        let out = session.materialize(Some("file://a"), Some("image/png"), "QUJD");
        let path = out
            .lines()
            .nth(1)
            .and_then(|l| l.strip_prefix("Binary content saved to "))
            .map(PathBuf::from);
        let Some(path) = path else {
            panic!("no path in {out}");
        };
        let file_mode = std::fs::metadata(&path).map(|m| m.permissions().mode() & 0o777);
        assert_eq!(file_mode.ok(), Some(0o600));
        let dir_mode = path
            .parent()
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.permissions().mode() & 0o777);
        assert_eq!(dir_mode, Some(0o700));
        assert_eq!(std::fs::read(&path).ok(), Some(b"ABC".to_vec()));
        let _ = session.cleanup();
        assert!(!path.exists());
    }

    #[test]
    fn node_base64_byte_length() {
        assert_eq!(base64_decoded_len("QUJD"), 3);
        assert_eq!(base64_decoded_len("QQ=="), 1);
        assert_eq!(base64_decoded_len("QUI="), 2);
    }

    // --- MCP-226 / MCP-227 / MCP-228: the output guard ------------------------------------------

    fn guard_options<'a>() -> McpOutputGuardOptions<'a> {
        McpOutputGuardOptions {
            enabled: true,
            prefix: "",
            suffix: "",
            empty_text_fallback: None,
            max_bytes: 50 * 1024,
            max_lines: 2000,
            details_max_bytes: 16 * 1024,
            raw_mcp_result: None,
        }
    }

    #[test]
    fn a_prefix_lands_on_the_first_text_block_not_a_new_one() {
        // MCP-226's `verify`, both clauses.
        let mut options = guard_options();
        options.prefix = "Error: ";
        let guarded = guard_mcp_output(
            &[
                McpContentBlock::Image { data: "d".to_string(), mime_type: "image/png".to_string() },
                text("body"),
            ],
            &options,
        );
        assert_eq!(guarded.content.len(), 2);
        assert_eq!(guarded.content.get(1), Some(&text("Error: body")));

        let guarded = guard_mcp_output(
            &[McpContentBlock::Image { data: "d".to_string(), mime_type: "image/png".to_string() }],
            &options,
        );
        assert_eq!(guarded.content.first(), Some(&text("Error: ")));
    }

    #[test]
    fn a_hostile_image_mime_is_trimmed_and_clamped_to_100_chars() {
        let guarded = guard_mcp_output(
            &[
                McpContentBlock::Image {
                    data: "d".to_string(),
                    mime_type: format!("  {}  ", "x".repeat(300)),
                },
                McpContentBlock::Image { data: "d".to_string(), mime_type: "   ".to_string() },
            ],
            &guard_options(),
        );
        assert_eq!(
            guarded.content.first(),
            Some(&McpContentBlock::Image {
                data: "d".to_string(),
                mime_type: "x".repeat(100)
            })
        );
        assert_eq!(
            guarded.content.get(1),
            Some(&McpContentBlock::Image {
                data: "d".to_string(),
                mime_type: "image/png".to_string()
            })
        );
    }

    #[test]
    fn the_kill_switch_bypasses_truncation_and_details_bounding() {
        let raw = json!({ "content": [], "big": "y".repeat(64 * 1024) });
        let mut options = guard_options();
        options.enabled = false;
        options.raw_mcp_result = Some(&raw);
        let guarded = guard_mcp_output(&[text(&"x".repeat(200 * 1024))], &options);
        assert!(guarded.output_guard.is_none());
        assert_eq!(guarded.mcp_result.as_ref(), Some(&raw));
        assert_eq!(guarded.content.len(), 1);
    }

    #[test]
    fn a_huge_single_line_is_truncated_to_one_partial_line_plus_a_notice() {
        // MCP-227's `verify`, first clause.
        let mut options = guard_options();
        options.max_bytes = 1024;
        options.max_lines = 10;
        let guarded = guard_mcp_output(&[text(&"z".repeat(60 * 1024))], &options);
        let Some(guard) = guarded.output_guard.as_ref() else {
            panic!("expected truncation");
        };
        assert_eq!(guard.get("truncated"), Some(&Value::Bool(true)));
        assert_eq!(guard.get("originalBytes").and_then(Value::as_u64), Some(61440));
        assert_eq!(guard.get("originalLines").and_then(Value::as_u64), Some(1));
        let returned = guard.get("returnedBytes").and_then(Value::as_u64).unwrap_or(u64::MAX);
        assert!(returned <= 1024, "returnedBytes {returned} must fit maxBytes");
        let Some(McpContentBlock::Text(body)) = guarded.content.first() else {
            panic!("expected a text block");
        };
        assert!(body.contains("[MCP text output truncated:"));
        assert!(body.contains("Full text saved to:"));
        // The spilled file OUTLIVES the guard call — that is the whole point of the notice.
        let path = guard.get("fullOutputPath").and_then(Value::as_str).unwrap_or_default();
        assert!(Path::new(path).exists());
        if let Some(dir) = Path::new(path).parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn the_truncation_notice_is_byte_exact() {
        // MCP-227's `verify`, second clause: 2 500 lines / 60 000 bytes → `2,500 lines / 58.6 KiB`.
        let notice = format_truncation_notice(60_000, 2_500, Some("/tmp/x/output-aa.txt"), None);
        assert_eq!(
            notice,
            "[MCP text output truncated: original 2,500 lines / 58.6 KiB. Full text saved to: /tmp/x/output-aa.txt — use read with offset/limit or grep to inspect.]"
        );
        let notice = format_truncation_notice(512, 3, None, Some("EACCES"));
        assert_eq!(
            notice,
            "[MCP text output truncated: original 3 lines / 512 B. Full output could not be saved: EACCES]"
        );
        assert_eq!(
            format_truncation_notice(2 * 1024 * 1024, 1, None, None),
            "[MCP text output truncated: original 1 lines / 2.0 MiB. Full output could not be saved: unknown error]"
        );
    }

    #[test]
    fn truncate_head_emits_a_partial_line_on_a_char_boundary() {
        // A 3-byte character straddling the cut must not be split.
        let text = "aa\u{4e00}\u{4e00}\u{4e00}";
        assert_eq!(truncate_head(text, 6, 10), "aa\u{4e00}");
        assert_eq!(truncate_head("one\ntwo\nthree", 100, 2), "one\ntwo");
    }

    #[test]
    fn images_pass_through_a_truncation_and_are_counted() {
        let mut options = guard_options();
        options.max_bytes = 64;
        options.max_lines = 5;
        let guarded = guard_mcp_output(
            &[
                text(&"q".repeat(4096)),
                McpContentBlock::Image { data: "d".to_string(), mime_type: "image/png".to_string() },
            ],
            &options,
        );
        assert_eq!(guarded.content.len(), 2);
        assert!(guarded.content.get(1).is_some_and(McpContentBlock::is_image));
        let count = guarded
            .output_guard
            .as_ref()
            .and_then(|g| g.get("imageBlocksPassedThrough"))
            .and_then(Value::as_u64);
        assert_eq!(count, Some(1));
        if let Some(dir) = guarded
            .output_guard
            .as_ref()
            .and_then(|g| g.get("fullOutputPath"))
            .and_then(Value::as_str)
            .map(Path::new)
            .and_then(Path::parent)
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn the_empty_text_fallback_replaces_a_blank_body_but_keeps_images() {
        let mut options = guard_options();
        options.empty_text_fallback = Some("(no output)");
        let guarded = guard_mcp_output(
            &[
                text(""),
                McpContentBlock::Image { data: "d".to_string(), mime_type: "image/png".to_string() },
            ],
            &options,
        );
        assert_eq!(guarded.content.first(), Some(&text("(no output)")));
        assert_eq!(guarded.content.len(), 2);
    }

    // --- MCP-229: `details.mcpResult` bounding --------------------------------------------------

    #[test]
    fn a_small_raw_result_is_kept_verbatim() {
        let raw = json!({ "content": [{ "type": "text", "text": "ok" }] });
        let mut options = guard_options();
        options.raw_mcp_result = Some(&raw);
        let guarded = guard_mcp_output(&[text("ok")], &options);
        assert_eq!(guarded.mcp_result.as_ref(), Some(&raw));
        assert_eq!(guarded.details().get("mcpResult"), Some(&raw));
    }

    #[test]
    fn an_oversized_raw_result_becomes_a_summary_with_a_21st_omitted_entry() {
        // MCP-229's `verify`: 25 content blocks ⇒ 21 summary entries, the last `{omitted, count:5}`.
        let blocks: Vec<Value> =
            (0..25).map(|i| json!({ "type": "text", "text": format!("block {i}") })).collect();
        let raw = json!({
            "content": blocks,
            "isError": true,
            "structuredContent": { "a": 1, "b": 2 },
            "_meta": { "trace": "x" },
            "vendorField": "y".repeat(32 * 1024),
        });
        let summary = bound_mcp_result(&raw, 16 * 1024);
        assert_eq!(summary.get("omitted"), Some(&Value::Bool(true)));
        assert_eq!(summary.get("isError"), Some(&Value::Bool(true)));
        assert_eq!(summary.get("contentBlocks").and_then(Value::as_u64), Some(25));
        assert_eq!(
            summary.get("reason").and_then(Value::as_str),
            Some("Raw MCP result exceeded the details size limit and was replaced with this summary to keep session context bounded.")
        );
        let content_summary =
            summary.get("contentSummary").and_then(Value::as_array).cloned().unwrap_or_default();
        assert_eq!(content_summary.len(), 21);
        assert_eq!(
            content_summary.get(20),
            Some(&json!({ "type": "omitted", "count": 5 }))
        );
        assert_eq!(
            summary.get("structuredContent").and_then(|v| v.get("type")).and_then(Value::as_str),
            Some("object")
        );
        assert_eq!(
            summary.get("meta").and_then(|v| v.get("keyCount")).and_then(Value::as_u64),
            Some(1)
        );
        let extra = summary.get("extraFields").and_then(Value::as_array).cloned().unwrap_or_default();
        assert_eq!(extra.len(), 1);
        assert_eq!(extra.first().and_then(|f| f.get("key")).and_then(Value::as_str), Some("vendorField"));
        if let Some(dir) = summary
            .get("fullResultPath")
            .and_then(Value::as_str)
            .map(Path::new)
            .and_then(Path::parent)
        {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn summarize_value_treats_an_array_as_an_object_with_index_keys() {
        let summarized = summarize_value(&json!(["a", "bb"]));
        assert_eq!(summarized.get("type").and_then(Value::as_str), Some("array"));
        assert_eq!(summarized.get("keyCount").and_then(Value::as_u64), Some(2));
        assert_eq!(summarized.get("keysPreview"), Some(&json!(["0", "1"])));
        assert_eq!(summarized.get("estimatedBytes").and_then(Value::as_u64), Some(3));

        assert_eq!(
            summarize_value(&Value::Null).get("type").and_then(Value::as_str),
            Some("null")
        );
        assert_eq!(
            summarize_value(&json!(12)).get("estimatedBytes").and_then(Value::as_u64),
            Some(2)
        );
    }

    #[test]
    fn estimate_value_bytes_stops_at_depth_two() {
        // depth 0 = the outer object, depth 1 = the inner object, depth 2 = nothing counted.
        assert_eq!(estimate_value_bytes(&json!({ "a": { "b": { "c": "xxxx" } } }), 0), 0);
        assert_eq!(estimate_value_bytes(&json!({ "a": { "b": "xxxx" } }), 0), 4);
        assert_eq!(estimate_value_bytes(&json!({ "a": true }), 0), 4);
    }

    #[test]
    fn truncate_key_caps_at_120_with_a_trailing_ellipsis() {
        let key = "k".repeat(200);
        let truncated = truncate_key(&key);
        assert_eq!(utf16_len(&truncated), KEY_MAX_CHARS);
        assert!(truncated.ends_with('…'));
        assert_eq!(truncate_key("short"), "short");
    }

    // --- number and size formatting -------------------------------------------------------------

    #[test]
    fn js_number_formatting() {
        assert_eq!(js_number(1.0), "1");
        assert_eq!(js_number(-0.0), "0");
        assert_eq!(js_number(1.5), "1.5");
        assert_eq!(js_number(f64::NAN), "null");
        assert_eq!(js_number(1e21), "1e+21");
        assert_eq!(js_number(1e-7), "1e-7");
    }

    #[test]
    fn size_and_thousands_formatting() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(60_000), "58.6 KiB");
        assert_eq!(format_size(1024 * 1024), "1.0 MiB");
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(2_500), "2,500");
        assert_eq!(format_thousands(1_234_567), "1,234,567");
    }
}
