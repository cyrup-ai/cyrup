//! Per-item dynamic fan-out semantics — a faithful port of pi's `runs/shared/dynamic-fanout.ts`
//! (`dynamic-fanout.ts:1-296`, the T4/C16 slice: `{item}`/`{item.path}` per-element template
//! substitution, `expand.key` JSON-pointer item keys, the `maxItems` cap, `onEmpty`, duplicate-key
//! and colliding-id detection, and the collect-record shape + aggregate-schema validation).
//!
//! # Why this lives beside `chain_graph.rs` rather than inside it
//!
//! [`crate::spawn::chain_graph::walk_chain`]'s `DynamicGroup` arm previously cloned the resolved
//! template task N times, so every fanned-out child received the *identical* task string (the file's
//! own doc comment admitted this gap). This module supplies the missing machinery: given a resolved
//! source array and one template, it materializes one distinct, per-item-substituted
//! [`crate::spawn::chain_graph::SingleStepSpec`] task per array element (`materialize`-shaped), and —
//! after the group runs — folds the per-child results into the ordered collect-record array pi
//! registers under `collect.as` (`collectDynamicResults`).
//!
//! # Error strategy
//!
//! pi raises a single `DynamicFanoutError` for every failure here. This module instead returns
//! `Result<_, String>` carrying the *exact same message text*, leaving each caller to wrap it in the
//! error taxonomy it owns: [`crate::spawn::chain_graph::walk_chain`] wraps into
//! [`crate::error::SubagentError::StructuredOutputInvalid`] (matching the pre-existing
//! expand-pointer-failure handling in that file, which every `DynamicGroup` test already asserts
//! against), and the chain-parse validator (`discovery/chains.rs`) — which is itself `String`-error
//! based — can call [`assert_no_unresolved_item_references`] directly. Keeping this module
//! taxonomy-agnostic is what lets a single port serve both the parse-time and run-time call sites pi
//! itself exercises (`validateChainOutputBindings` at parse; `materializeDynamicParallelStep` at
//! run).
//!
//! Every function is pure over its inputs (no I/O, no subprocess, no clock), so the unit tests below
//! reproduce `dynamic-fanout.test.ts`'s scenarios directly.

use std::collections::HashSet;

use serde_json::Value;

/// pi `RESERVED_TEMPLATE_NAMES` (`dynamic-fanout.ts:43`): template reference names that are NOT item
/// references and must be left for the chain-level template engine (`{previous}`, `{task}`,
/// `{chain_dir}`, `{outputs.x}`) rather than rejected as unknown.
const RESERVED_TEMPLATE_NAMES: &[&str] = &["task", "previous", "chain_dir", "outputs"];

// ============================================================================================
// JSON Pointer resolution (pi assertJsonPointer / resolveJsonPointer, dynamic-fanout.ts:61-102)
// ============================================================================================

/// pi `assertJsonPointer` (`dynamic-fanout.ts:61-71`): `""` is valid; otherwise the pointer must
/// start with `/` and contain no invalid `~`-escape (a `~` not immediately followed by `0` or `1`).
///
/// # Errors
///
/// A message mirroring pi's `DynamicFanoutError` text when the pointer is malformed.
pub fn assert_json_pointer(pointer: &str, label: &str) -> Result<(), String> {
    if pointer.is_empty() {
        return Ok(());
    }
    let Some(rest) = pointer.strip_prefix('/') else {
        return Err(format!("{label} must be a JSON Pointer starting with '/'."));
    };
    for segment in rest.split('/') {
        if has_invalid_tilde_escape(segment) {
            return Err(format!("{label} contains invalid JSON Pointer escape."));
        }
    }
    Ok(())
}

/// A `~` not immediately followed by `0` or `1` (mirrors pi's `/~(?![01])/`).
fn has_invalid_tilde_escape(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    (0..bytes.len()).any(|index| {
        bytes.get(index) == Some(&b'~')
            && !matches!(bytes.get(index + 1), Some(&b'0') | Some(&b'1'))
    })
}

/// pi `decodePointerSegment` (`dynamic-fanout.ts:73-75`): `~1` -> `/`, then `~0` -> `~` (in that
/// order, reversing the RFC-6901 encode order).
fn decode_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

/// pi array-index guard `^(0|[1-9][0-9]*)$` (`dynamic-fanout.ts:84`): `"0"`, or a non-zero leading
/// digit followed by any digits — no leading zeros, no sign, no decimal point.
fn is_array_index(segment: &str) -> bool {
    if segment == "0" {
        return true;
    }
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) if ('1'..='9').contains(&first) => chars.all(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// pi `resolveJsonPointer` (`dynamic-fanout.ts:77-102`): walk `pointer` (an RFC-6901 JSON Pointer)
/// into `value`, returning the addressed sub-value by reference. An empty pointer returns `value`
/// itself. Array segments must be canonical indices within bounds; object segments must be present
/// keys; walking through a scalar is an error.
///
/// # Errors
///
/// A message mirroring pi's `DynamicFanoutError` when a segment does not address an array index,
/// when the addressed value does not exist, or when the pointer walks through a non-object/array.
pub fn resolve_json_pointer<'a>(
    value: &'a Value,
    pointer: &str,
    label: &str,
) -> Result<&'a Value, String> {
    assert_json_pointer(pointer, label)?;
    if pointer.is_empty() {
        return Ok(value);
    }
    // `assert_json_pointer` already established the leading '/'.
    let rest = pointer.strip_prefix('/').unwrap_or(pointer);
    let mut current = value;
    for raw_segment in rest.split('/') {
        let segment = decode_pointer_segment(raw_segment);
        current = match current {
            Value::Array(items) => {
                if !is_array_index(&segment) {
                    return Err(format!(
                        "{label} segment '{segment}' does not address an array index."
                    ));
                }
                segment
                    .parse::<usize>()
                    .ok()
                    .and_then(|index| items.get(index))
                    .ok_or_else(|| format!("{label} does not exist."))?
            }
            Value::Object(map) => map
                .get(&segment)
                .ok_or_else(|| format!("{label} does not exist."))?,
            _ => return Err(format!("{label} does not exist.")),
        };
    }
    Ok(current)
}

/// pi `scalarToKey` (`dynamic-fanout.ts:104-113`): coerce a resolved `expand.key` value into a
/// string item key. Only string/number/boolean scalars are permitted; the resulting key must be
/// non-blank, contain no control characters, and be at most 200 characters.
///
/// # Errors
///
/// A message mirroring pi's `DynamicFanoutError` when the value is not a scalar, or when the coerced
/// key is empty, unsafe, or over-long.
fn scalar_to_key(value: &Value, label: &str) -> Result<String, String> {
    let key = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => {
            return Err(format!(
                "{label} must resolve to a string, number, or boolean."
            ));
        }
    };
    if key.trim().is_empty() {
        return Err(format!("{label} resolved to an empty key."));
    }
    if key
        .chars()
        .any(|c| (c as u32) <= 0x1F || (c as u32) == 0x7F)
    {
        return Err(format!("{label} resolved to an unsafe key."));
    }
    if key.chars().count() > 200 {
        return Err(format!(
            "{label} resolved to a key longer than 200 characters."
        ));
    }
    Ok(key)
}

/// pi `normalizeItemKeyForId` (`dynamic-fanout.ts:115-122`): lower-case the key, collapse every
/// maximal run of non-`[a-z0-9]` characters to a single `-`, trim leading/trailing `-`, truncate to
/// 80 characters, and fall back to `"item"` when nothing survives. This is the id-collision key —
/// two distinct raw keys that normalize identically (`"a/b"` vs `"a-b"`) are a hard error, so each
/// child gets a filesystem-safe, unambiguous directory id.
#[must_use]
pub fn normalize_item_key_for_id(key: &str) -> String {
    let lowered = key.to_lowercase();
    let mut collapsed = String::with_capacity(lowered.len());
    let mut in_separator = false;
    for ch in lowered.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            collapsed.push(ch);
            in_separator = false;
        } else if !in_separator {
            collapsed.push('-');
            in_separator = true;
        }
    }
    let trimmed = collapsed.trim_matches('-');
    let sliced: String = trimmed.chars().take(80).collect();
    if sliced.is_empty() {
        "item".to_string()
    } else {
        sliced
    }
}

// ============================================================================================
// Item-template substitution (pi resolveItemTemplate, dynamic-fanout.ts:124-145)
// ============================================================================================

/// pi `valueToTemplateText` (`dynamic-fanout.ts:124-129`): render a resolved item value as template
/// text — strings verbatim, numbers/booleans/null via their JS `String()` form, anything else as
/// compact JSON (`JSON.stringify`).
fn value_to_template_text(value: &Value, reference: &str) -> Result<String, String> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Null => Ok("null".to_string()),
        other => serde_json::to_string(other)
            .map_err(|err| format!("Unresolved item reference '{reference}': {err}")),
    }
}

/// pi `resolveItemPath` (`dynamic-fanout.ts:131-135`): map a dotted `{item.a.b}` path into an
/// RFC-6901 JSON Pointer (`/a/b`, escaping `~`->`~0` then `/`->`~1` per segment) and resolve it
/// against `item`; an absent path returns `item` itself.
fn resolve_item_path<'a>(
    item: &'a Value,
    path_text: Option<&str>,
    reference: &str,
) -> Result<&'a Value, String> {
    match path_text {
        None => Ok(item),
        Some(path) => {
            let pointer = format!(
                "/{}",
                path.split('.')
                    .map(|segment| segment.replace('~', "~0").replace('/', "~1"))
                    .collect::<Vec<_>>()
                    .join("/")
            );
            resolve_json_pointer(item, &pointer, reference)
        }
    }
}

/// One strict `ITEM_REF_PATTERN` (`dynamic-fanout.ts:42`, `\{([A-Za-z_][A-Za-z0-9_]*)(?:\.([^{}]+))?\}`)
/// match: the exclusive end char-index, the identifier `name`, and the optional dotted `path`.
struct ItemRefMatch {
    end: usize,
    name: String,
    path: Option<String>,
}

/// Try to match a strict `ITEM_REF_PATTERN` token at `start` (which must be `{`), char-indexed to
/// stay UTF-8 safe. Returns `None` if the token at `start` is not a well-formed `{name}` /
/// `{name.path}` (path being one-or-more non-brace characters).
fn match_item_ref(chars: &[char], start: usize) -> Option<ItemRefMatch> {
    if chars.get(start).copied() != Some('{') {
        return None;
    }
    let id_start = start + 1;
    let first = chars.get(id_start).copied()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut pos = id_start + 1;
    while let Some(&c) = chars.get(pos) {
        if c.is_ascii_alphanumeric() || c == '_' {
            pos += 1;
        } else {
            break;
        }
    }
    let name: String = chars.get(id_start..pos)?.iter().collect();
    match chars.get(pos).copied() {
        Some('}') => Some(ItemRefMatch {
            end: pos + 1,
            name,
            path: None,
        }),
        Some('.') => {
            let path_start = pos + 1;
            let mut path_end = path_start;
            while let Some(&c) = chars.get(path_end) {
                if c == '{' || c == '}' {
                    break;
                }
                path_end += 1;
            }
            if path_end == path_start {
                return None; // `[^{}]+` requires at least one character.
            }
            if chars.get(path_end).copied() == Some('}') {
                let path: String = chars.get(path_start..path_end)?.iter().collect();
                Some(ItemRefMatch {
                    end: path_end + 1,
                    name,
                    path: Some(path),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// pi `resolveItemTemplate` (`dynamic-fanout.ts:137-145`): replace every `{item}` / `{item.path}`
/// occurrence naming `item_name` with the corresponding value from `item`, leaving every other
/// `{...}` reference (`{previous}`, `{outputs.x}`, another loop's item name) untouched for the
/// chain-level template engine to handle later.
///
/// # Errors
///
/// A message mirroring pi's `DynamicFanoutError` when an item reference has a blank or `..`-bearing
/// path.
pub fn resolve_item_template(
    template: &str,
    item_name: &str,
    item: &Value,
) -> Result<String, String> {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(template.len());
    let mut i = 0usize;
    while i < chars.len() {
        if chars.get(i).copied() == Some('{')
            && let Some(matched) = match_item_ref(&chars, i)
        {
            let raw: String = chars.iter().skip(i).take(matched.end - i).collect();
            if matched.name == item_name {
                if let Some(path) = &matched.path
                    && (path.trim().is_empty() || path.contains(".."))
                {
                    return Err(format!("Invalid item reference '{raw}'."));
                }
                let resolved = resolve_item_path(item, matched.path.as_deref(), &raw)?;
                out.push_str(&value_to_template_text(resolved, &raw)?);
            } else {
                out.push_str(&raw);
            }
            i = matched.end;
            continue;
        }
        if let Some(&c) = chars.get(i) {
            out.push(c);
        }
        i += 1;
    }
    Ok(out)
}

// ============================================================================================
// Template-reference validation (pi assertNoUnresolvedItemReferences, dynamic-fanout.ts:154-175)
// ============================================================================================

/// Extract every `{[^{}]*}` token (pi's generic `/\{([^{}]*)\}/g`) as `(raw, inner)` pairs.
fn scan_brace_tokens(chars: &[char]) -> Vec<(String, String)> {
    let mut tokens = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        if chars.get(i).copied() == Some('{') {
            let mut j = i + 1;
            while let Some(&c) = chars.get(j) {
                if c == '{' || c == '}' {
                    break;
                }
                j += 1;
            }
            if chars.get(j).copied() == Some('}') {
                let inner: String = chars
                    .get(i + 1..j)
                    .map(|s| s.iter().collect())
                    .unwrap_or_default();
                let raw: String = chars
                    .get(i..=j)
                    .map(|s| s.iter().collect())
                    .unwrap_or_default();
                tokens.push((raw, inner));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    tokens
}

/// The leading `[A-Za-z_][A-Za-z0-9_]*` identifier of `s`, if it starts with one.
fn leading_identifier(s: &str) -> Option<String> {
    let mut chars = s.chars();
    let first = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    let mut id = String::new();
    id.push(first);
    for c in chars {
        if c.is_ascii_alphanumeric() || c == '_' {
            id.push(c);
        } else {
            break;
        }
    }
    Some(id)
}

/// Whether `raw` (a full `{...}` token) matches the strict `ITEM_REF_PATTERN` in its entirety.
fn is_full_item_ref_token(raw: &str) -> bool {
    let chars: Vec<char> = raw.chars().collect();
    matches!(match_item_ref(&chars, 0), Some(matched) if matched.end == chars.len())
}

/// pi's final tail-guard `new RegExp(`\{${itemName}(?:\.|$)[^}]*$`)` (`dynamic-fanout.ts:172`): an
/// unclosed `{item` at end-of-string, or a `{item.` whose remainder never closes with `}`.
fn has_unclosed_item_ref(template: &str, item_name: &str) -> bool {
    let needle = format!("{{{item_name}");
    let mut search_start = 0usize;
    while let Some(rel) = template
        .get(search_start..)
        .and_then(|hay| hay.find(&needle))
    {
        let occurrence = search_start + rel;
        let after = occurrence + needle.len();
        let rest = template.get(after..).unwrap_or_default();
        if rest.is_empty() {
            return true; // `{item` at end -> `(?:\.|$)` matched `$`, `[^}]*$` matched empty.
        }
        if let Some(after_dot) = rest.strip_prefix('.')
            && !after_dot.contains('}')
        {
            return true; // `{item.` with no closing brace before end.
        }
        search_start = occurrence + 1;
    }
    false
}

/// pi `assertNoUnresolvedItemReferences` (`dynamic-fanout.ts:154-175`): reject a template that
/// contains a malformed item reference (`{item[path]}`, `{item.}`, `{item..x}`, an unclosed
/// `{item.path`) or an unknown, non-reserved reference (`{other}` when the item name is `item`),
/// while accepting well-formed item references and reserved chain references (`{previous}`,
/// `{outputs.x}`).
///
/// # Errors
///
/// A message mirroring pi's `DynamicFanoutError` (`Invalid item reference ...` /
/// `Unsupported template reference ...`).
pub fn assert_no_unresolved_item_references(
    template: &str,
    item_name: &str,
    label: &str,
) -> Result<(), String> {
    let chars: Vec<char> = template.chars().collect();
    let item_dot_prefix = format!("{item_name}.");
    for (raw, reference) in scan_brace_tokens(&chars) {
        if reference == item_name || reference.starts_with(&item_dot_prefix) {
            // An item reference: it must be strictly well-formed and free of `.`-only / `..` paths.
            if !is_full_item_ref_token(&raw)
                || reference == item_dot_prefix
                || reference.contains("..")
            {
                return Err(format!("Invalid item reference '{raw}' in {label}."));
            }
            continue;
        }
        let name = leading_identifier(&reference);
        if name.as_deref() == Some(item_name) {
            // e.g. `{item[path]}` — starts with the item name but is not a valid item reference.
            return Err(format!("Invalid item reference '{raw}' in {label}."));
        }
        match name {
            Some(n) if RESERVED_TEMPLATE_NAMES.contains(&n.as_str()) => {}
            Some(_) => {
                return Err(format!(
                    "Unsupported template reference '{raw}' in {label}."
                ));
            }
            None => {}
        }
    }
    let literal_dot = format!("{{{item_name}.}}");
    if template.contains(&literal_dot) || has_unclosed_item_ref(template, item_name) {
        return Err(format!("Invalid item reference in {label}."));
    }
    Ok(())
}

// ============================================================================================
// Item materialization (pi resolveDynamicFanoutItems, dynamic-fanout.ts:216-240)
// ============================================================================================

/// One materialized fan-out element (pi `DynamicMaterializedItem`, `dynamic-fanout.ts:13-18`): its
/// original array `index`, the raw `key` (from `expand.key` or the index), the normalized
/// filesystem-safe `id_key`, and the array element `item` itself.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterializedItem {
    /// The element's original 0-based position in the resolved source array.
    pub index: usize,
    /// The raw item key: the `expand.key` pointer value coerced via [`scalar_to_key`], or the
    /// stringified index when no `expand.key` is configured.
    pub key: String,
    /// The collision-detection id: [`normalize_item_key_for_id`] applied to `key`.
    pub id_key: String,
    /// The source array element itself (owned clone), used for per-item template substitution and
    /// echoed back into the collect record.
    pub item: Value,
}

/// pi `resolveDynamicFanoutItems` (`dynamic-fanout.ts:216-240`), minus the expand-source resolution
/// and shape validation the Rust runner performs at chain-parse time / in the walker's pointer
/// resolver. Given the already-resolved `source` array, apply the effective `max_items` cap, derive
/// each element's key (via the `key_pointer` JSON Pointer or the index), and reject duplicate keys
/// and colliding normalized ids.
///
/// `step_display` is the 1-based step number pi interpolates into error messages
/// (`Dynamic chain step N ...`).
///
/// # Errors
///
/// A message mirroring pi's `DynamicFanoutError` when: no effective `max_items` is available; the
/// array exceeds `max_items`; an `expand.key` value fails to coerce to a safe key; or two elements
/// produce a duplicate key or a colliding normalized id.
pub fn resolve_dynamic_fanout_items(
    source: &[Value],
    key_pointer: Option<&str>,
    max_items: Option<u32>,
    step_display: usize,
) -> Result<Vec<MaterializedItem>, String> {
    let max = max_items.ok_or_else(|| {
        format!("Dynamic chain step {step_display} requires an effective maxItems.")
    })?;
    if source.len() as u64 > u64::from(max) {
        return Err(format!(
            "Dynamic chain step {step_display} resolved {} items, exceeding maxItems {max}.",
            source.len()
        ));
    }

    let mut seen_keys: HashSet<String> = HashSet::new();
    let mut seen_ids: HashSet<String> = HashSet::new();
    let mut items = Vec::with_capacity(source.len());
    for (index, item) in source.iter().enumerate() {
        let key = match key_pointer {
            None => index.to_string(),
            Some(pointer) => {
                let label = format!("Dynamic chain step {step_display} expand.key");
                let resolved = resolve_json_pointer(item, pointer, &label)?;
                scalar_to_key(resolved, &label)?
            }
        };
        if !seen_keys.insert(key.clone()) {
            return Err(format!(
                "Dynamic chain step {step_display} produced duplicate item key '{key}'."
            ));
        }
        let id_key = normalize_item_key_for_id(&key);
        if !seen_ids.insert(id_key.clone()) {
            return Err(format!(
                "Dynamic chain step {step_display} produced colliding item id '{id_key}'."
            ));
        }
        items.push(MaterializedItem {
            index,
            key,
            id_key,
            item: item.clone(),
        });
    }
    Ok(items)
}

// ============================================================================================
// Collect-record shape (pi collectDynamicResults / validateDynamicCollection, dynamic-fanout.ts:263-295)
// ============================================================================================

/// The narrow per-child view [`collect_dynamic_results`] folds into each collect record — pi's
/// `Pick<SingleResult, "agent" | "exitCode" | "error" | "timedOut" | "structuredOutput" |
/// "artifactPaths" | "savedOutputPath"> & { output?: string; finalOutput?: string }`
/// (`dynamic-fanout.ts:266`). A `None` slot in the `results` list passed to
/// [`collect_dynamic_results`] models pi's `results[index] === undefined` (a child that was never
/// dispatched).
#[derive(Clone, Debug, Default)]
pub struct CollectChildResult {
    /// The child's own agent name (pi `agent`); falls back to the template agent when absent.
    pub agent: Option<String>,
    /// The child process exit code (pi `exitCode`); `None` renders as JSON `null`.
    pub exit_code: Option<i64>,
    /// A failure message (pi `error`); an empty string is treated as absent, matching pi's
    /// truthiness spread.
    pub error: Option<String>,
    /// Whether the child was killed by the run deadline (pi `timedOut`).
    pub timed_out: bool,
    /// The child's validated structured output (pi `structuredOutput`); `Some(Value::Null)` is a
    /// deliberate `structured: null`, `None` omits the field.
    pub structured_output: Option<Value>,
    /// The child's artifact-paths bundle (pi `artifactPaths`), carried opaquely.
    pub artifact_paths: Option<Value>,
    /// The child's saved-output file path (pi `savedOutputPath`).
    pub saved_output_path: Option<String>,
    /// pi's optional `output` text override — when a string, it wins over `final_output` for the
    /// record's `text`.
    pub output: Option<String>,
    /// The child's plain-text final output (pi `finalOutput`), used for `text` when `output` is
    /// absent.
    pub final_output: Option<String>,
}

/// One collect record (pi `DynamicCollectedResult`, `dynamic-fanout.ts:20-32`). Serializes to the
/// exact pi JSON shape: `key`/`index`/`item`/`agent`/`exitCode`/`text` are always present;
/// `structured`/`error`/`timedOut`/`outputPath`/`artifactPaths` are present only when pi would emit
/// them.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicCollectedResult {
    /// The raw item key (pi `key`).
    pub key: String,
    /// The item's original array index (pi `index`).
    pub index: usize,
    /// The source array element (pi `item`).
    pub item: Value,
    /// The producing agent (pi `agent`).
    pub agent: String,
    /// The child exit code, `null` when unknown (pi `exitCode`, always serialized).
    pub exit_code: Option<i64>,
    /// The child's text output (pi `text`).
    pub text: String,
    /// The child's structured output, omitted when absent (pi `structured`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured: Option<Value>,
    /// A failure message, omitted when absent/empty (pi `error`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `true` only when the child timed out, omitted otherwise (pi `timedOut`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub timed_out: bool,
    /// The child's saved-output path, omitted when absent (pi `outputPath`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    /// The child's artifact-paths bundle, omitted when absent (pi `artifactPaths`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_paths: Option<Value>,
}

/// pi `collectDynamicResults` (`dynamic-fanout.ts:263-287`): fold the per-child `results` (index-
/// aligned with `items`) into one ordered collect record per item. A `None` result slot yields an
/// empty `text`, the template `agent`, a `null` exit code, and no optional fields — exactly pi's
/// `results[index] === undefined` branch.
#[must_use]
pub fn collect_dynamic_results(
    items: &[MaterializedItem],
    results: &[Option<CollectChildResult>],
    template_agent: &str,
) -> Vec<DynamicCollectedResult> {
    items
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let result = results.get(index).and_then(Option::as_ref);
            let text = match result {
                Some(child) => match &child.output {
                    Some(output) => output.clone(),
                    None => child.final_output.clone().unwrap_or_default(),
                },
                None => String::new(),
            };
            DynamicCollectedResult {
                key: entry.key.clone(),
                index: entry.index,
                item: entry.item.clone(),
                agent: result
                    .and_then(|child| child.agent.clone())
                    .unwrap_or_else(|| template_agent.to_string()),
                exit_code: result.and_then(|child| child.exit_code),
                text,
                structured: result.and_then(|child| child.structured_output.clone()),
                error: result
                    .and_then(|child| child.error.clone())
                    .filter(|message| !message.is_empty()),
                timed_out: result.is_some_and(|child| child.timed_out),
                output_path: result.and_then(|child| child.saved_output_path.clone()),
                artifact_paths: result.and_then(|child| child.artifact_paths.clone()),
            }
        })
        .collect()
}

/// Render a collect-record slice as the JSON array pi registers under `collect.as`
/// (`chain-execution.ts:961`: `structured: collected`).
#[must_use]
pub fn collected_results_to_value(results: &[DynamicCollectedResult]) -> Value {
    Value::Array(
        results
            .iter()
            .map(|record| serde_json::to_value(record).unwrap_or(Value::Null))
            .collect(),
    )
}

/// pi `validateDynamicCollection` (`dynamic-fanout.ts:289-295`): if a `collect.outputSchema` is
/// declared, validate the whole collect-record array against it (reusing the crate's shared
/// JSON-Schema validator, [`crate::exec::structured::validate_structured_output`]).
///
/// # Errors
///
/// A `Collected output validation failed: ...` message mirroring pi's `DynamicFanoutError` when the
/// aggregate fails the declared schema.
pub fn validate_dynamic_collection(
    schema: Option<&Value>,
    value: &[DynamicCollectedResult],
) -> Result<(), String> {
    let Some(schema) = schema else {
        return Ok(());
    };
    let array = collected_results_to_value(value);
    crate::exec::structured::validate_structured_output(schema, &array)
        .map_err(|message| format!("Collected output validation failed: {message}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use serde_json::json;

    // ---- JSON pointer resolution (pi test: "resolves JSON Pointers ...") ----

    #[test]
    fn resolves_json_pointers_into_arrays_and_objects() {
        let value = json!({ "items": [1, 2] });
        assert_eq!(
            resolve_json_pointer(&value, "/items/1", "path").unwrap(),
            &json!(2)
        );
        assert_eq!(resolve_json_pointer(&value, "", "path").unwrap(), &value);
    }

    #[test]
    fn json_pointer_missing_and_non_index_segments_error() {
        let value = json!({ "items": [1, 2] });
        assert!(resolve_json_pointer(&value, "/items/9", "p").is_err());
        assert!(resolve_json_pointer(&value, "/items/x", "p").is_err());
        assert!(resolve_json_pointer(&value, "/missing", "p").is_err());
        // Leading-zero index is not a canonical array index.
        assert!(resolve_json_pointer(&value, "/items/01", "p").is_err());
        // Pointer that does not start with '/'.
        assert!(resolve_json_pointer(&value, "items", "p").is_err());
    }

    #[test]
    fn json_pointer_unescapes_tilde_segments() {
        let value = json!({ "a/b": { "c~d": 7 } });
        assert_eq!(
            resolve_json_pointer(&value, "/a~1b/c~0d", "p").unwrap(),
            &json!(7)
        );
    }

    // ---- normalize_item_key_for_id ----

    #[test]
    fn normalizes_keys_to_safe_ids_and_detects_the_collision_case() {
        assert_eq!(normalize_item_key_for_id("src/a.ts"), "src-a-ts");
        // The pi colliding-id case: "a/b" and "a-b" both normalize identically.
        assert_eq!(normalize_item_key_for_id("a/b"), "a-b");
        assert_eq!(normalize_item_key_for_id("a-b"), "a-b");
        // Nothing survives -> "item".
        assert_eq!(normalize_item_key_for_id("///"), "item");
        assert_eq!(normalize_item_key_for_id("--A--"), "a");
    }

    // ---- resolve_item_template (pi test: "materializes item templates") ----

    #[test]
    fn substitutes_item_path_references_and_leaves_others_untouched() {
        let item = json!({ "path": "src/a.ts", "nested": { "n": 3 } });
        assert_eq!(
            resolve_item_template("Review {target.path}", "target", &item).unwrap(),
            "Review src/a.ts"
        );
        // Whole-item reference renders as compact JSON.
        assert_eq!(
            resolve_item_template("{target}", "target", &json!({ "a": 1 })).unwrap(),
            "{\"a\":1}"
        );
        // Nested path.
        assert_eq!(
            resolve_item_template("n={target.nested.n}", "target", &item).unwrap(),
            "n=3"
        );
        // A reference to a different name / reserved name is left verbatim.
        assert_eq!(
            resolve_item_template("{previous} and {outputs.x} and {other.p}", "target", &item)
                .unwrap(),
            "{previous} and {outputs.x} and {other.p}"
        );
    }

    #[test]
    fn item_template_renders_scalars_and_null() {
        assert_eq!(
            resolve_item_template("{it.n}", "it", &json!({ "n": 42 })).unwrap(),
            "42"
        );
        assert_eq!(
            resolve_item_template("{it.b}", "it", &json!({ "b": true })).unwrap(),
            "true"
        );
        assert_eq!(
            resolve_item_template("{it.z}", "it", &json!({ "z": null })).unwrap(),
            "null"
        );
    }

    #[test]
    fn item_template_missing_path_errors() {
        let item = json!({ "path": "x" });
        assert!(resolve_item_template("{target.nope}", "target", &item).is_err());
    }

    // ---- assert_no_unresolved_item_references (pi test: "bad templates") ----

    #[test]
    fn rejects_unsupported_and_malformed_template_references() {
        // Unknown, non-reserved name.
        let err = assert_no_unresolved_item_references("Review {other.path}", "target", "task")
            .unwrap_err();
        assert!(err.contains("Unsupported template reference"), "got: {err}");
        // Bracket syntax that starts with the item name.
        let err = assert_no_unresolved_item_references("Review {target[path]}", "target", "task")
            .unwrap_err();
        assert!(err.contains("Invalid item reference"), "got: {err}");
        // Unclosed reference at end of string.
        let err = assert_no_unresolved_item_references("Review {target.path", "target", "task")
            .unwrap_err();
        assert!(err.contains("Invalid item reference"), "got: {err}");
        // Empty-path `{item.}`.
        let err =
            assert_no_unresolved_item_references("Review {target.}", "target", "task").unwrap_err();
        assert!(err.contains("Invalid item reference"), "got: {err}");
        // `..` traversal.
        let err = assert_no_unresolved_item_references("Review {target.a..b}", "target", "task")
            .unwrap_err();
        assert!(err.contains("Invalid item reference"), "got: {err}");
    }

    #[test]
    fn accepts_valid_item_and_reserved_references() {
        assert!(
            assert_no_unresolved_item_references(
                "Review {target.path} then {previous} and {outputs.x}",
                "target",
                "task"
            )
            .is_ok()
        );
        assert!(assert_no_unresolved_item_references("{item}", "item", "task").is_ok());
    }

    // ---- resolve_dynamic_fanout_items (pi test: over-limit / duplicate / colliding) ----

    fn targets() -> Vec<Value> {
        vec![json!({ "path": "src/a.ts" }), json!({ "path": "src/b.ts" })]
    }

    #[test]
    fn materializes_items_with_index_keys_by_default() {
        let items = resolve_dynamic_fanout_items(&targets(), None, Some(4), 2).unwrap();
        assert_eq!(
            items.iter().map(|i| i.key.clone()).collect::<Vec<_>>(),
            ["0", "1"]
        );
        assert_eq!(items.iter().map(|i| i.index).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn materializes_items_with_pointer_keys() {
        let items = resolve_dynamic_fanout_items(&targets(), Some("/path"), Some(4), 2).unwrap();
        assert_eq!(
            items.iter().map(|i| i.key.clone()).collect::<Vec<_>>(),
            ["src/a.ts", "src/b.ts"]
        );
        assert_eq!(
            items.iter().map(|i| i.id_key.clone()).collect::<Vec<_>>(),
            ["src-a-ts", "src-b-ts"]
        );
    }

    #[test]
    fn over_limit_array_errors() {
        let err = resolve_dynamic_fanout_items(&targets(), Some("/path"), Some(1), 2).unwrap_err();
        assert!(err.contains("exceeding maxItems"), "got: {err}");
    }

    #[test]
    fn missing_effective_max_items_errors() {
        let err = resolve_dynamic_fanout_items(&targets(), Some("/path"), None, 2).unwrap_err();
        assert!(err.contains("requires an effective maxItems"), "got: {err}");
    }

    #[test]
    fn duplicate_keys_error() {
        let dup = vec![json!({ "path": "x" }), json!({ "path": "x" })];
        let err = resolve_dynamic_fanout_items(&dup, Some("/path"), Some(4), 2).unwrap_err();
        assert!(err.contains("duplicate item key"), "got: {err}");
    }

    #[test]
    fn colliding_ids_error() {
        let collide = vec![json!({ "path": "a/b" }), json!({ "path": "a-b" })];
        let err = resolve_dynamic_fanout_items(&collide, Some("/path"), Some(4), 2).unwrap_err();
        assert!(err.contains("colliding item id"), "got: {err}");
    }

    // ---- collect_dynamic_results + validate_dynamic_collection (pi test: "collects ordered ...") ----

    #[test]
    fn collects_ordered_records_with_the_full_pi_shape() {
        let items = resolve_dynamic_fanout_items(&targets(), Some("/path"), Some(4), 2).unwrap();
        let ok = CollectChildResult {
            agent: Some("reviewer".to_string()),
            exit_code: Some(0),
            structured_output: Some(json!({ "ok": "a" })),
            final_output: Some("ok".to_string()),
            ..CollectChildResult::default()
        };
        let timed_out = CollectChildResult {
            agent: Some("reviewer".to_string()),
            exit_code: Some(1),
            error: Some("Subagent timed out after 300ms.".to_string()),
            timed_out: true,
            structured_output: Some(json!({ "ok": "b" })),
            final_output: Some("ok".to_string()),
            ..CollectChildResult::default()
        };
        let collected = collect_dynamic_results(&items, &[Some(ok), Some(timed_out)], "reviewer");

        assert_eq!(
            collected.iter().map(|r| r.key.clone()).collect::<Vec<_>>(),
            ["src/a.ts", "src/b.ts"]
        );
        assert_eq!(
            collected
                .iter()
                .map(|r| r.structured.clone())
                .collect::<Vec<_>>(),
            [Some(json!({ "ok": "a" })), Some(json!({ "ok": "b" }))]
        );
        assert!(collected[1].timed_out);
        assert_eq!(collected[0].agent, "reviewer");
        assert_eq!(collected[0].text, "ok");

        // Aggregate schema validation (pi: array minItems ok; object type fails).
        assert!(
            validate_dynamic_collection(
                Some(&json!({ "type": "array", "minItems": 2 })),
                &collected
            )
            .is_ok()
        );
        assert!(
            validate_dynamic_collection(Some(&json!({ "type": "object" })), &collected).is_err()
        );
        // No schema -> ok.
        assert!(validate_dynamic_collection(None, &collected).is_ok());
    }

    #[test]
    fn collect_record_json_omits_absent_optional_fields() {
        let items = resolve_dynamic_fanout_items(&targets(), Some("/path"), Some(4), 2).unwrap();
        // A None slot models a never-dispatched child: empty text, template agent, null exit code.
        let collected = collect_dynamic_results(&items, &[None, None], "reviewer");
        let value = collected_results_to_value(&collected);
        let first = &value[0];
        assert_eq!(first["key"], json!("src/a.ts"));
        assert_eq!(first["index"], json!(0));
        assert_eq!(first["agent"], json!("reviewer"));
        assert_eq!(first["exitCode"], Value::Null);
        assert_eq!(first["text"], json!(""));
        // Optional fields omitted, not null.
        assert!(first.get("structured").is_none());
        assert!(first.get("error").is_none());
        assert!(first.get("timedOut").is_none());
        assert!(first.get("outputPath").is_none());
        assert!(first.get("artifactPaths").is_none());
    }

    #[test]
    fn collect_record_output_text_wins_over_final_output() {
        let items = resolve_dynamic_fanout_items(&targets(), Some("/path"), Some(4), 2).unwrap();
        let child = CollectChildResult {
            agent: Some("reviewer".to_string()),
            exit_code: Some(0),
            output: Some("from-output".to_string()),
            final_output: Some("from-final".to_string()),
            ..CollectChildResult::default()
        };
        let collected = collect_dynamic_results(&items, &[Some(child), None], "reviewer");
        assert_eq!(collected[0].text, "from-output");
        assert_eq!(collected[1].text, "");
    }
}
