//! The record of the tool calls a tool made while it ran — pi `NestedToolCalls` /
//! `NestedToolCallRecord` (`packages/ai/src/types.ts:570-593` @v1.0.1) — and the bounded
//! [`NestedCallRecorder`] that builds it (`packages/coding-agent/src/core/nested-tool-calls.ts:26-103`
//! @v1.0.1).
//!
//! The record lives on the tool-result message as `nestedCalls` ([`crate::Message::ToolResult`]),
//! beside the model-visible result: it is *"Kept for the session record; not sent to the model"*
//! (`types.ts:605`). Session files are shared with pi, so every shape here round-trips pi's
//! persisted JSON byte for byte — field order is pi's, which is the order the recorder assigns the
//! keys in (`id`, `name`, `status`, then `arguments` or `argumentsBytes`, then `durationMs` and
//! `error` when the call finishes).
//!
//! The recorder is pure bookkeeping: no clock but [`Instant`], no I/O, no async. The runner that
//! drives it (one call at a time, from a tool's execution) is `cyrup_agent::NestedToolCallRunner`;
//! it lives with the data model rather than with the runner because the limits are properties of
//! the *record* — what a session file may contain — and the same recorder is what the tests of
//! those limits drive directly.

use std::time::Instant;

use serde_json::{Map, Value};

use super::tool_call::ToolCall;
use super::usage::{Usage, combine_usage};

/// Limits of the nested-call record on a tool result (pi `NESTED_CALL_LIMITS`,
/// `nested-tool-calls.ts:26-31`): arguments over the per-call or total size are omitted, calls
/// beyond the count are dropped, and the record is marked incomplete when any of that happens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedCallLimits {
    /// Calls kept in one record; the next is dropped and the record marked incomplete.
    pub max_calls: usize,
    /// UTF-8 bytes of one call's JSON-serialized arguments before they are omitted.
    pub max_argument_bytes_per_call: usize,
    /// UTF-8 bytes of all recorded arguments together before further ones are omitted.
    pub max_argument_bytes_total: usize,
    /// Characters (UTF-16 code units, as JavaScript's `slice` counts them) of error text kept.
    pub max_error_chars: usize,
}

/// pi `NESTED_CALL_LIMITS` (`nested-tool-calls.ts:26-31` @v1.0.1).
pub const NESTED_CALL_LIMITS: NestedCallLimits = NestedCallLimits {
    max_calls: 256,
    max_argument_bytes_per_call: 8 * 1024,
    max_argument_bytes_total: 32 * 1024,
    max_error_chars: 500,
};

/// How a recorded nested call ended (pi `NestedToolCallRecord.status`, `types.ts:581`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NestedCallStatus {
    /// The call was still running when the calling tool finished.
    Unfinished,
    Ok,
    Error,
}

/// One call a tool made to another tool (pi `NestedToolCallRecord`, `types.ts:572-585`). The
/// result is not recorded; `arguments` are omitted when over the size limits.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedToolCallRecord {
    /// `<calling id>/<n>`.
    pub id: String,
    pub name: String,
    pub status: NestedCallStatus,
    /// Omitted when over the size limits; [`Self::arguments_bytes`] then gives their size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Map<String, Value>>,
    /// UTF-8 size of the arguments as JSON, set when [`Self::arguments`] is omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Error text, truncated to [`NestedCallLimits::max_error_chars`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Bounded record of the nested calls a tool made (pi `NestedToolCalls`, `types.ts:588-593`).
/// Results are not recorded.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NestedToolCalls {
    pub calls: Vec<NestedToolCallRecord>,
    /// `false` when calls were dropped, arguments omitted, or calls had not finished.
    pub complete: bool,
}

/// What a nested call settled as, for [`NestedCallRecorder::finish`]. The error text exists only
/// for an error: a successful call has none to pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NestedCallResult<'a> {
    Ok,
    /// The call failed; the text is the result's text content (empty when it had none).
    Error(&'a str),
}

/// Names one record inside the [`NestedCallRecorder`] that issued it. [`NestedCallRecorder::start`]
/// returns one only for a call that was recorded — a dropped call has no handle, which is what
/// pi's `NestedToolCallRecord | undefined` carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NestedCallHandle(usize);

/// Collects the nested calls of one model-issued tool call, including calls made by nested tools
/// (pi `NestedCallRecorder`, `nested-tool-calls.ts:47-103` @v1.0.1). The snapshot becomes
/// `nestedCalls` on the tool-result message.
#[derive(Debug, Default)]
pub struct NestedCallRecorder {
    calls: Vec<NestedToolCallRecord>,
    /// Start time of each record, by index; `None` once it has finished.
    started_at: Vec<Option<Instant>>,
    /// `false` once a call was dropped or its arguments omitted.
    incomplete: bool,
    argument_bytes: usize,
    /// Summed usage of every nested result, including calls dropped from the record.
    usage: Option<Usage>,
}

impl NestedCallRecorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a call as it starts. `None` when the call is dropped (the record is full).
    ///
    /// The argument size is that of the arguments' JSON serialization in UTF-8 — what
    /// `new TextEncoder().encode(JSON.stringify(arguments)).length` counts upstream — and key
    /// order is the order the arguments arrived in, so the recorded value serializes as it was
    /// sent.
    pub fn start(&mut self, call: &ToolCall) -> Option<NestedCallHandle> {
        if self.calls.len() >= NESTED_CALL_LIMITS.max_calls {
            self.incomplete = true;
            return None;
        }
        let mut record = NestedToolCallRecord {
            id: call.id.to_string(),
            name: call.name.clone(),
            status: NestedCallStatus::Unfinished,
            arguments: None,
            arguments_bytes: None,
            duration_ms: None,
            error: None,
        };
        let arguments: &Map<String, Value> = &call.arguments;
        // `Map` serialization cannot fail; the fallback keeps the arm total without a panic path.
        let bytes = serde_json::to_vec(arguments).map_or(0, |json| json.len());
        if bytes > NESTED_CALL_LIMITS.max_argument_bytes_per_call
            || self.argument_bytes.saturating_add(bytes)
                > NESTED_CALL_LIMITS.max_argument_bytes_total
        {
            record.arguments_bytes = Some(bytes as u64);
            self.incomplete = true;
        } else {
            record.arguments = Some(arguments.clone());
            self.argument_bytes += bytes;
        }
        let handle = NestedCallHandle(self.calls.len());
        self.calls.push(record);
        self.started_at.push(Some(Instant::now()));
        Some(handle)
    }

    /// Settle a recorded call (pi `finish`, `:77-83`). A handle from another recorder, or one
    /// already finished, changes nothing.
    pub fn finish(&mut self, handle: Option<NestedCallHandle>, result: NestedCallResult<'_>) {
        let Some(NestedCallHandle(index)) = handle else {
            return;
        };
        let (Some(record), Some(started)) =
            (self.calls.get_mut(index), self.started_at.get_mut(index))
        else {
            return;
        };
        let Some(started) = started.take() else {
            return;
        };
        record.duration_ms = Some((started.elapsed().as_secs_f64() * 1000.0).round() as u64);
        match result {
            NestedCallResult::Ok => record.status = NestedCallStatus::Ok,
            NestedCallResult::Error(text) => {
                record.status = NestedCallStatus::Error;
                if !text.is_empty() {
                    record.error = Some(truncate_utf16(text, NESTED_CALL_LIMITS.max_error_chars));
                }
            }
        }
    }

    /// Add one nested result's usage to the running total (pi `addUsage`, `:85-87`).
    pub fn add_usage(&mut self, usage: &Usage) {
        self.usage = Some(match self.usage.take() {
            Some(total) => combine_usage(&total, usage),
            None => usage.clone(),
        });
    }

    /// Summed usage of every nested result so far (pi `totalUsage`).
    pub fn total_usage(&self) -> Option<&Usage> {
        self.usage.as_ref()
    }

    /// Copy of the record so far, or `None` when no nested call was made (pi `snapshot`,
    /// `:96-102`). Incomplete while any call is still unfinished.
    pub fn snapshot(&self) -> Option<NestedToolCalls> {
        if self.calls.is_empty() && !self.incomplete {
            return None;
        }
        let complete = !self.incomplete
            && self
                .calls
                .iter()
                .all(|call| call.status != NestedCallStatus::Unfinished);
        Some(NestedToolCalls {
            calls: self.calls.clone(),
            complete,
        })
    }
}

/// The first `max` UTF-16 code units of `text` — JavaScript's `text.slice(0, max)`. A surrogate
/// pair that would be cut in half is dropped whole: a lone surrogate is not a Rust `String`.
fn truncate_utf16(text: &str, max: usize) -> String {
    let mut units = 0usize;
    let mut out = String::new();
    for ch in text.chars() {
        units += ch.len_utf16();
        if units > max {
            break;
        }
        out.push(ch);
    }
    out
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

    fn call(id: &str, args: Value) -> ToolCall {
        let Value::Object(map) = args else {
            panic!("test arguments are an object");
        };
        ToolCall {
            id: id.into(),
            name: "t".to_string(),
            arguments: map.into(),
            thought_signature: None,
            namespace: None,
        }
    }

    fn usage(input: u64, cost: f64) -> Usage {
        Usage {
            input,
            total_tokens: input,
            cost: crate::Cost {
                input: cost,
                total: cost,
                ..crate::Cost::default()
            },
            ..Usage::default()
        }
    }

    /// nested-tool-calls.test.ts "omits oversized arguments and drops calls beyond the limit".
    #[test]
    fn omits_oversized_arguments_and_drops_calls_beyond_the_limit() {
        let mut recorder = NestedCallRecorder::new();
        assert_eq!(recorder.snapshot(), None);
        let small = recorder.start(&call("a", json!({ "x": 1 })));
        recorder.finish(small, NestedCallResult::Ok);
        let snap = recorder.snapshot().unwrap();
        assert!(snap.complete);
        assert_eq!(snap.calls.len(), 1);
        assert_eq!(snap.calls[0].id, "a");
        assert_eq!(snap.calls[0].status, NestedCallStatus::Ok);
        assert_eq!(
            snap.calls[0].arguments,
            json!({ "x": 1 }).as_object().cloned()
        );
        assert!(snap.calls[0].duration_ms.is_some());

        let big_text = "x".repeat(NESTED_CALL_LIMITS.max_argument_bytes_per_call);
        let big = recorder.start(&call("b", json!({ "text": big_text })));
        let long_error = "e".repeat(1000);
        recorder.finish(big, NestedCallResult::Error(&long_error));
        let snap = recorder.snapshot().unwrap();
        assert!(!snap.complete);
        assert_eq!(snap.calls[1].id, "b");
        assert_eq!(snap.calls[1].status, NestedCallStatus::Error);
        assert_eq!(snap.calls[1].arguments, None);
        assert!(
            snap.calls[1].arguments_bytes.unwrap()
                > NESTED_CALL_LIMITS.max_argument_bytes_per_call as u64
        );
        assert_eq!(
            snap.calls[1].error.as_deref().map(|e| e.chars().count()),
            Some(NESTED_CALL_LIMITS.max_error_chars)
        );

        for i in 0..NESTED_CALL_LIMITS.max_calls {
            let handle = recorder.start(&call(&format!("c{i}"), json!({})));
            recorder.finish(handle, NestedCallResult::Ok);
        }
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap.calls.len(), NESTED_CALL_LIMITS.max_calls);
        assert!(!snap.complete);
    }

    /// nested-tool-calls.test.ts "caps the total argument size and marks unfinished calls
    /// incomplete".
    #[test]
    fn caps_the_total_argument_size_and_marks_unfinished_calls_incomplete() {
        let mut recorder = NestedCallRecorder::new();
        let chunk = json!({ "text": "x".repeat(7000) });
        let handles: Vec<_> = (0..6)
            .map(|i| recorder.start(&call(&format!("c{i}"), chunk.clone())))
            .collect();
        let snap = recorder.snapshot().unwrap();
        // 32 KiB fits four 7000-byte argument objects.
        assert_eq!(
            snap.calls.iter().filter(|c| c.arguments.is_some()).count(),
            4
        );
        assert!(
            snap.calls
                .iter()
                .all(|c| c.status == NestedCallStatus::Unfinished)
        );
        assert!(!snap.complete);
        assert_eq!(handles.len(), 6);
    }

    /// The 257th call is dropped, not recorded, and the record says so.
    #[test]
    fn the_call_past_the_count_limit_is_dropped_and_marks_the_record_incomplete() {
        let mut recorder = NestedCallRecorder::new();
        for i in 0..NESTED_CALL_LIMITS.max_calls {
            let handle = recorder.start(&call(&format!("c{i}"), json!({})));
            assert!(handle.is_some());
            recorder.finish(handle, NestedCallResult::Ok);
        }
        assert!(recorder.snapshot().unwrap().complete);
        assert_eq!(recorder.start(&call("dropped", json!({}))), None);
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap.calls.len(), 256);
        assert!(!snap.complete);
        assert!(snap.calls.iter().all(|c| c.id != "dropped"));
    }

    /// The size is the UTF-8 byte length of the JSON serialization, not the character count: a
    /// multi-byte string over the per-call limit in bytes but under it in characters is omitted.
    #[test]
    fn argument_size_is_counted_in_utf8_bytes_of_the_json_serialization() {
        let mut recorder = NestedCallRecorder::new();
        // 3000 three-byte characters: 3000 chars, 9000 bytes plus the JSON framing.
        let text = "\u{20ac}".repeat(3000);
        let args = json!({ "text": text });
        let expected = serde_json::to_vec(args.as_object().unwrap()).unwrap().len();
        assert!(expected > NESTED_CALL_LIMITS.max_argument_bytes_per_call);
        recorder.start(&call("a", args));
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap.calls[0].arguments, None);
        assert_eq!(snap.calls[0].arguments_bytes, Some(expected as u64));
    }

    /// Error text is cut at 500 UTF-16 code units, like JavaScript's `slice`, and never inside a
    /// surrogate pair.
    #[test]
    fn error_text_is_truncated_in_utf16_units_without_splitting_a_pair() {
        assert_eq!(truncate_utf16("abc", 500), "abc");
        assert_eq!(truncate_utf16(&"e".repeat(1000), 500).len(), 500);
        // An astral character is two units; 499 units + one pair would be 501, so the pair is out.
        let text = format!("{}\u{1F600}", "a".repeat(499));
        assert_eq!(truncate_utf16(&text, 500), "a".repeat(499));
        let text = format!("{}\u{1F600}", "a".repeat(498));
        assert_eq!(truncate_utf16(&text, 500).encode_utf16().count(), 500);
    }

    /// An empty error text leaves `error` absent (`if (isError && errorText)`, `:82`).
    #[test]
    fn an_error_without_text_records_no_error_key() {
        let mut recorder = NestedCallRecorder::new();
        let handle = recorder.start(&call("a", json!({})));
        recorder.finish(handle, NestedCallResult::Error(""));
        let snap = recorder.snapshot().unwrap();
        assert_eq!(snap.calls[0].status, NestedCallStatus::Error);
        assert_eq!(snap.calls[0].error, None);
    }

    /// Usage of every nested result is summed, whether or not the call made the record.
    #[test]
    fn usage_is_summed_across_calls() {
        let mut recorder = NestedCallRecorder::new();
        assert!(recorder.total_usage().is_none());
        recorder.add_usage(&usage(10, 0.01));
        recorder.add_usage(&usage(15, 0.015));
        let total = recorder.total_usage().unwrap();
        assert_eq!(total.input, 25);
        assert!((total.cost.total - 0.025).abs() < 1e-12);
    }

    /// A record serializes with pi's key order: id, name, status, arguments, durationMs, error.
    #[test]
    fn record_keys_are_in_pi_order() {
        let record = NestedToolCallRecord {
            id: "c/1".into(),
            name: "edit".into(),
            status: NestedCallStatus::Error,
            arguments: json!({ "path": "a.ts" }).as_object().cloned(),
            arguments_bytes: None,
            duration_ms: Some(3),
            error: Some("boom".into()),
        };
        assert_eq!(
            serde_json::to_string(&record).unwrap(),
            r#"{"id":"c/1","name":"edit","status":"error","arguments":{"path":"a.ts"},"durationMs":3,"error":"boom"}"#
        );
    }
}
