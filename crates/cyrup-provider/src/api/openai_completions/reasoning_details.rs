//! `reasoning_details` — the OpenRouter-style structured reasoning replay metadata (Pi
//! `openai-completions.ts:118-147,191-266` @v0.87.1).
//!
//! Three detail variants (`reasoning.text`, `reasoning.summary`, `reasoning.encrypted`) stream as
//! deltas, are merged into logical entries, are parked on the thinking block's signature slot at
//! block finalization, and are replayed verbatim on the next request.
//!
//! `[CYRUP-DELTA]` — openai-completions.ts:191-213 models the three variants as a discriminated
//! union. cyrup keeps them as `serde_json::Value` objects: the details carry provider-specific extra
//! keys (`OpenAIReasoningDetailBase extends Record<string, JsonValue>`) that MUST round-trip
//! byte-for-byte, and a typed enum in Rust would have to either drop or flatten them. Validation
//! ([`is_openai_reasoning_detail`]) therefore does the narrowing the TypeScript type does.

use serde_json::{Map, Value};

/// `hasValidCommonReasoningDetailFields` (openai-completions.ts:122-129): `id` may be absent, null
/// or a string; `format` absent or a string; `index` absent or a number.
fn has_valid_common_fields(obj: &Map<String, Value>) -> bool {
    let id_ok = matches!(
        obj.get("id"),
        None | Some(Value::Null) | Some(Value::String(_))
    );
    let format_ok = matches!(obj.get("format"), None | Some(Value::String(_)));
    let index_ok = matches!(obj.get("index"), None | Some(Value::Number(_)));
    id_ok && format_ok && index_ok
}

/// `isOpenAIReasoningDetail` (openai-completions.ts:131-147): a non-array object whose common
/// fields validate and whose `type` names one of the three variants with its required payload.
pub(super) fn is_openai_reasoning_detail(detail: &Value) -> bool {
    let Some(obj) = detail.as_object() else {
        return false;
    };
    if !has_valid_common_fields(obj) {
        return false;
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("reasoning.summary") => matches!(obj.get("summary"), Some(Value::String(_))),
        Some("reasoning.encrypted") => matches!(obj.get("data"), Some(Value::String(_))),
        Some("reasoning.text") => {
            matches!(obj.get("text"), Some(Value::String(_)))
                && matches!(
                    obj.get("signature"),
                    None | Some(Value::Null) | Some(Value::String(_))
                )
        }
        _ => false,
    }
}

/// `target[key] ??= source[key]` (openai-completions.ts:244-248). Absent or null on the target is
/// overwritten; a source that has no such key assigns `undefined`, which `JSON.stringify` then
/// drops — so the key is REMOVED rather than written as null.
fn assign_if_nullish(target: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    if !matches!(target.get(key), None | Some(Value::Null)) {
        return;
    }
    match source.get(key) {
        Some(v) => {
            target.insert(key.to_string(), v.clone());
        }
        None => {
            target.remove(key);
        }
    }
}

/// `target[key] ||= source[key]`: as [`assign_if_nullish`], but every JS-falsy value (including the
/// empty string, `false` and `0`) is also overwritten (openai-completions.ts:246, :255).
fn assign_if_falsy(target: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    let falsy = match target.get(key) {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Bool(b)) => !*b,
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
        _ => false,
    };
    if !falsy {
        return;
    }
    match source.get(key) {
        Some(v) => {
            target.insert(key.to_string(), v.clone());
        }
        None => {
            target.remove(key);
        }
    }
}

/// `fillMissingCommonReasoningDetailFields` (openai-completions.ts:243-250).
fn fill_missing_common_fields(target: &mut Map<String, Value>, source: &Map<String, Value>) {
    assign_if_nullish(target, source, "id");
    assign_if_falsy(target, source, "format");
    assign_if_nullish(target, source, "index");
}

fn detail_type(detail: &Value) -> Option<&str> {
    detail.get("type").and_then(Value::as_str)
}

/// `appendOpenAIReasoningDetail` (openai-completions.ts:252-266). Consecutive `reasoning.text` or
/// `reasoning.summary` deltas are concatenated into the previous entry; anything else is pushed as
/// a new entry. `detail` must already have passed [`is_openai_reasoning_detail`].
pub(super) fn append_openai_reasoning_detail(details: &mut Vec<Value>, detail: &Value) {
    let (Some(kind), Some(src)) = (detail_type(detail), detail.as_object()) else {
        return;
    };
    let mergeable = matches!(kind, "reasoning.text" | "reasoning.summary")
        && details.last().and_then(detail_type) == Some(kind);
    if mergeable && let Some(last) = details.last_mut().and_then(Value::as_object_mut) {
        let field = if kind == "reasoning.text" {
            "text"
        } else {
            "summary"
        };
        let appended = {
            let prev = last.get(field).and_then(Value::as_str).unwrap_or_default();
            let next = src.get(field).and_then(Value::as_str).unwrap_or_default();
            format!("{prev}{next}")
        };
        last.insert(field.to_string(), Value::String(appended));
        if kind == "reasoning.text" {
            // `lastDetail.signature ||= detail.signature` (openai-completions.ts:255).
            assign_if_falsy(last, src, "signature");
        }
        fill_missing_common_fields(last, src);
        return;
    }
    details.push(detail.clone());
}

/// `parseOpenAIReasoningDetails` (openai-completions.ts:215-223): a thinking signature that is a
/// JSON array of at least one valid detail, else `None`.
pub(super) fn parse_openai_reasoning_details(signature: Option<&str>) -> Option<Vec<Value>> {
    let sig = signature.filter(|s| !s.is_empty())?;
    let parsed: Value = serde_json::from_str(sig).ok()?;
    let arr = parsed.as_array()?;
    if arr.is_empty() || !arr.iter().all(is_openai_reasoning_detail) {
        return None;
    }
    Some(arr.clone())
}

/// `parseLegacyEncryptedReasoningDetail` (openai-completions.ts:225-241): the pre-`reasoning_details`
/// shape, one encrypted detail with a non-empty `id` and `data`, stored on a tool call's
/// `thoughtSignature`.
pub(super) fn parse_legacy_encrypted_reasoning_detail(signature: Option<&str>) -> Option<Value> {
    let sig = signature.filter(|s| !s.is_empty())?;
    let parsed: Value = serde_json::from_str(sig).ok()?;
    if !is_openai_reasoning_detail(&parsed) {
        return None;
    }
    let obj = parsed.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("reasoning.encrypted") {
        return None;
    }
    let id_ok = obj
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    let data_ok = obj
        .get("data")
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty());
    if id_ok && data_ok { Some(parsed) } else { None }
}
