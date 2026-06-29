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
