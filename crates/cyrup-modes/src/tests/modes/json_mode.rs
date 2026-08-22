//! JSON mode (R-11-007): the JSONL event stream `run_json` writes — pi's session header as line 1
//! exactly once per session, then every subscribed `AgentSessionEvent` verbatim, one object per
//! line, in order.

use std::sync::Arc;

use super::support::{build_runtime, fixture, parse_lines, type_of};
use crate::run_json;
use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_session_svc::{InputSource, UserInput};
use serde_json::Value;

#[tokio::test]
async fn json_mode_emits_ordered_event_stream() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    // turn 1: assistant w/ a `write` tool call; turn 2: final assistant text.
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("write", serde_json::json!({"path": "hi.txt", "content": "x"}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    run_json(&runtime, [UserInput::text("write a file", InputSource::Cli)], &mut out)
        .await
        .expect("json mode runs");

    let lines = parse_lines(&out);
    assert!(!lines.is_empty(), "no lines emitted");

    // Line 1 is the session header (Pi print-mode.ts:112-117); the event stream follows it.
    assert_eq!(type_of(&lines[0]), "session", "line 1 must be the session header: {lines:?}");
    let events = &lines[1..];
    assert!(!events.is_empty(), "no events emitted");

    // Every event line parsed; the ordered `type` tags bracket the run.
    let kinds: Vec<&str> = events.iter().map(type_of).collect();
    assert_eq!(kinds.first(), Some(&"agent_start"), "must start with agent_start: {kinds:?}");
    // SEAM-005: json mode writes EVERY subscribed session event verbatim (Pi print-mode.ts:103-108),
    // so the stream now closes with `agent_settled` — the run's last `agent_end` immediately
    // precedes it.
    assert_eq!(kinds.last(), Some(&"agent_settled"), "must end with agent_settled: {kinds:?}");
    assert_eq!(
        kinds.iter().rev().nth(1),
        Some(&"agent_end"),
        "…preceded by the run's last agent_end: {kinds:?}"
    );

    let tes = kinds.iter().position(|k| *k == "tool_execution_start").expect("tool_execution_start");
    let tee = kinds.iter().position(|k| *k == "tool_execution_end").expect("tool_execution_end");
    assert!(tes < tee, "tool exec start must precede end: {kinds:?}");

    // message_end role order = user -> assistant(toolCall) -> toolResult -> assistant.
    let roles: Vec<String> = events
        .iter()
        .filter(|e| type_of(e) == "message_end")
        .map(message_role)
        .collect();
    assert_eq!(
        roles,
        vec!["user", "assistant", "toolResult", "assistant"],
        "message_end roles out of order: {roles:?}"
    );
}

/// The `role` of a `message_end` event payload, distinguishing a tool-call assistant by content.
fn message_role(ev: &Value) -> String {
    ev.get("message")
        .and_then(|m| m.get("role"))
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string()
}

/// The raw (unparsed) LF-delimited lines of the sink, so a byte-exact assertion can be made about
/// the first line without a JSON round-trip normalizing whitespace or key order.
fn raw_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8(bytes.to_vec()).expect("utf8 output").lines().map(str::to_string).collect()
}

/// E / O2 — JSON mode writes the session header (`sessionManager.getHeader()`) as JSONL **line 1**,
/// BEFORE any event, so a consumer built to Pi's wire format learns the session id/cwd up front (Pi
/// `runPrintMode`: `writeRawStdout(JSON.stringify(header))` ahead of the subscription,
/// print-mode.ts:112-117). The emitted bytes must be the byte-exact Pi `SessionHeader` record:
/// `{"type":"session","version":3,"id":…,"timestamp":…,"cwd":…}` (session-manager.ts:32-39,856-863) —
/// compact, camelCase, keys in that order, `version` a number, `parentSession` omitted when unset.
/// Pre-fix cyrup emitted no header line at all and the first line was `agent_start`.
#[tokio::test]
async fn json_mode_writes_session_header_as_first_line() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("done")], StopReason::Stop)]);
    let runtime = build_runtime(&fx, faux).await;

    let session = runtime.session().await;

    let mut out: Vec<u8> = Vec::new();
    run_json(&runtime, [UserInput::text("hello", InputSource::Cli)], &mut out)
        .await
        .expect("json mode runs");

    let lines = raw_lines(&out);
    assert!(lines.len() >= 2, "expected a header line plus events, got {} lines", lines.len());

    // Build the byte-exact Pi header record from the session's own header field values and assert
    // line 1 equals it verbatim (serde compact output == Pi's `JSON.stringify(header)` for the same
    // `SessionHeader` interface: same key order, no spaces, `version` numeric, `id`/`cwd` strings).
    let header = session.session_header().await;
    let expected = format!(
        "{{\"type\":\"session\",\"version\":3,\"id\":\"{}\",\"timestamp\":\"{}\",\"cwd\":\"{}\"}}",
        header.id, header.timestamp, header.cwd
    );
    assert_eq!(lines[0], expected, "line 1 must be the byte-exact Pi session header");

    // Sanity: the header id is this session's id, cwd is the project cwd, no `parentSession` key,
    // and the very next line begins the event stream.
    let parsed: Value = serde_json::from_str(&lines[0]).expect("header line is valid json");
    assert_eq!(parsed["type"], "session");
    assert_eq!(parsed["version"], 3);
    assert_eq!(parsed["id"].as_str(), Some(session.session_id().as_str()));
    assert_eq!(parsed["cwd"].as_str(), Some(fx.cwd.to_string_lossy().as_ref()));
    assert!(parsed.get("parentSession").is_none(), "parentSession must be omitted when unset");
    let first_event: Value = serde_json::from_str(&lines[1]).expect("line 2 is valid json");
    assert_eq!(
        type_of(&first_event),
        "agent_start",
        "the event stream (agent_start) must immediately follow the header"
    );
}

/// The header is written exactly ONCE per session even across a multi-prompt `--mode json` run
/// (initial submission + follow-ups, each dispatched as a separate `run_json` call, mirroring
/// `run_json_dispatch`). Pi writes `getHeader()` a single time before its whole message loop
/// (print-mode.ts:112-119), so a second `run_json` on the same session must NOT re-emit it.
#[tokio::test]
async fn json_mode_writes_session_header_exactly_once_across_followups() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
    ]);
    let runtime = build_runtime(&fx, faux).await;

    // Replay the exact dispatch shape: initial prompt, then one follow-up, into ONE sink — and
    // additionally a SECOND `run_json` call on the same runtime, which must not re-emit the header.
    let mut out: Vec<u8> = Vec::new();
    run_json(&runtime, [UserInput::text("first", InputSource::Cli)], &mut out)
        .await
        .expect("initial json run");
    run_json(&runtime, [UserInput::text("second", InputSource::Cli)], &mut out)
        .await
        .expect("follow-up json run");

    let header_lines = parse_lines(&out).into_iter().filter(|v| type_of(v) == "session").count();
    assert_eq!(header_lines, 1, "the session header must be emitted exactly once across the run");
}
