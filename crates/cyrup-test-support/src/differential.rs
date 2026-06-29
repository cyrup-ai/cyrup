//! Pi differential runner (the crate's self-disclosed-deferred promise, lib.rs:5; func-00 R-00-012).
//!
//! Runs a scripted scenario through the cyrup harness and diffs the emitted event sequence against a
//! Pi-shaped expected sequence — asserting "identical emitted event sequence (types + ordering)"
//! (R-00-012). Provides both coarse type/ordering comparison and a normalized full-event diff
//! (volatile fields folded via [`crate::golden`]).

use cyrup_provider::StreamEvent;
use cyrup_session_svc::AgentSessionEvent;
use serde::Serialize;
use similar::TextDiff;

use crate::golden::normalize_value;
use crate::harness::{Harness, HarnessError};

/// The ordered `kind` discriminants of a session-event sequence (Pi event `type` ordering).
pub fn event_kind_sequence(events: &[AgentSessionEvent]) -> Vec<String> {
    events.iter().map(|e| e.kind().to_string()).collect()
}

/// The ordered serde `type` tags of a provider stream-event sequence (Pi `AssistantMessageEvent`
/// types).
pub fn stream_event_type_sequence(events: &[StreamEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| {
            serde_json::to_value(e).ok().and_then(|v| {
                v.get("type").and_then(|t| t.as_str()).map(|s| s.to_string())
            })
        })
        .collect()
}

/// Diff two ordered string sequences. `None` when identical; otherwise a unified diff (R-00-012
/// "types + ordering").
pub fn diff_sequences(expected: &[String], actual: &[String]) -> Option<String> {
    if expected == actual {
        return None;
    }
    let exp = expected.join("\n");
    let act = actual.join("\n");
    let diff = TextDiff::from_lines(&exp, &act);
    Some(diff.unified_diff().header("expected", "actual").to_string())
}

/// Assert two event-kind sequences are identical (types + ordering).
pub fn assert_event_kinds(expected: &[String], actual: &[String]) -> Result<(), String> {
    match diff_sequences(expected, actual) {
        None => Ok(()),
        Some(diff) => Err(format!("event sequence mismatch:\n{diff}")),
    }
}

/// A normalized full-event JSONL rendering for field-level differential comparison (volatile fields
/// folded). Pairs with [`diff_normalized`].
pub fn normalized_jsonl<T: Serialize>(items: &[T]) -> String {
    let mut out = String::new();
    for item in items {
        if let Ok(mut v) = serde_json::to_value(item) {
            normalize_value(&mut v);
            if let Ok(line) = serde_json::to_string(&v) {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }
    out
}

/// Field-level differential diff of two serializable sequences (normalized). `None` when identical.
pub fn diff_normalized<T: Serialize, U: Serialize>(expected: &[T], actual: &[U]) -> Option<String> {
    let exp = normalized_jsonl(expected);
    let act = normalized_jsonl(actual);
    if exp == act {
        return None;
    }
    let diff = TextDiff::from_lines(&exp, &act);
    Some(diff.unified_diff().header("expected", "actual").to_string())
}

// --- real-Pi cross-impl anchor (R-00-012) -----------------------------------
//
// A fixture captured from RUNNING Pi (its own faux core under node, see
// `fixtures/pi/*.pi-captured.events.jsonl`) is the only true cross-impl anchor: cyrup's emitted
// `StreamEvent` type+ordering MUST equal Pi's, and the terminal message MUST match field-level
// modulo two documented Pi<->cyrup representation deltas folded by [`canonicalize_cross_impl`].

/// Fold the two known Pi<->cyrup serialization deltas so a Pi-captured JSON value can be compared
/// field-level against a cyrup `StreamEvent`. Recurses into objects and arrays; volatile fields
/// (e.g. `timestamp`) are handled separately by [`crate::golden::normalize_value`].
///
/// - **`role`** — Pi serializes `role:"assistant"` on every `AssistantMessage`; cyrup type-encodes
///   the role (the Rust type *is* the assistant message), so it is absent at the seam. Dropped here.
/// - **number representation** — Pi emits JSON numbers (`0`); cyrup's `f64` cost/usage fields emit
///   `0.0`. Every number is coerced to `f64` so `0` and `0.0` compare equal.
pub fn canonicalize_cross_impl(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.remove("role");
            for (_k, v) in map.iter_mut() {
                canonicalize_cross_impl(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items.iter_mut() {
                canonicalize_cross_impl(v);
            }
        }
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64()
                && let Some(num) = serde_json::Number::from_f64(f)
            {
                *value = serde_json::Value::Number(num);
            }
        }
        _ => {}
    }
}

/// Parse a Pi-captured `*.events.jsonl` fixture into its event values, skipping provenance/comment
/// lines that carry no `type` field (the `{"_note":…}` header convention).
pub fn pi_fixture_events(jsonl: &str) -> Vec<serde_json::Value> {
    jsonl
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|v| v.get("type").and_then(|t| t.as_str()).is_some())
        .collect()
}

/// The ordered serde `type` tags of a parsed Pi-captured event-value sequence.
pub fn value_type_sequence(events: &[serde_json::Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .collect()
}

/// A cross-impl-canonical rendering of one event value (volatile fields zeroed, `role` dropped,
/// numbers coerced to `f64`) for field-level comparison between a Pi capture and a cyrup event.
pub fn canonical_event(mut value: serde_json::Value) -> serde_json::Value {
    crate::golden::normalize_value(&mut value);
    canonicalize_cross_impl(&mut value);
    value
}

/// Run one prompt through `harness` and assert the emitted session-event `kind` sequence matches
/// `expected_kinds` (Pi differential scenario). The harness must be freshly constructed (or its
/// events otherwise empty) so the run's sequence is the only one compared.
pub async fn run_differential(
    harness: &Harness,
    prompt: impl Into<String>,
    expected_kinds: &[&str],
) -> Result<(), String> {
    let events = harness.run(prompt).await.map_err(|e: HarnessError| e.to_string())?;
    let actual = event_kind_sequence(&events);
    let expected: Vec<String> = expected_kinds.iter().map(|s| s.to_string()).collect();
    assert_event_kinds(&expected, &actual)
}
