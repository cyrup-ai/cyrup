//! The json/rpc stdout wire projection (Pi `coding-agent/src/modes/json-event.ts`, v0.84.1).
//!
//! Proves that `message_update` records leave the json and rpc stdout streams DELTA-ONLY: the
//! cumulative outer `message` and the inner `assistantMessageEvent.partial` are both absent, exactly
//! as Pi's `toJsonEvent` (json-event.ts:28-40) produces and `coding-agent/docs/rpc.md:952-956`
//! documents. Mirrors Pi's own regression test,
//! `coding-agent/test/suite/regressions/7290-json-stream-linear.test.ts`.
//!
//! Every test drives the REAL adapter over a scripted `FauxProvider` and asserts on the produced
//! bytes, so it is the wire that is under test, not the projection helper in isolation.

use std::io::Cursor;
use std::sync::Arc;

use cyrup_core::StopReason;
use crate::{run_json, run_rpc};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSessionRuntime, InputSource, SessionFactory, UserInput};
use serde_json::Value;

use super::support::{base_config, create_runtime, fixture, kind, parse_lines, Fixture};

/// The plain runtime: no provider resolver and no native extension, because nothing here drives a
/// model command or an extension — just the two one-shot adapters over a scripted provider.
async fn build_runtime(fx: &Fixture, faux: Arc<FauxProvider>) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = faux;
    let cfg = base_config(fx);
    let target = cfg.target.clone();
    create_runtime(SessionFactory::new(provider, cfg), target).await
}

/// Run one prompt through json mode and return the RAW emitted lines, byte for byte.
///
/// Raw, because `serde_json::Value`'s map is a `BTreeMap` in this build (the `preserve_order`
/// feature is off), so parsing and re-rendering a line SORTS its keys and destroys exactly the
/// field-order property [`text_stream_records_match_pi_byte_for_byte`] exists to pin.
async fn run_once_raw(responses: Vec<cyrup_core::AssistantMessage>) -> Vec<String> {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    run_json(
        &runtime,
        [UserInput::text("go", InputSource::Cli)],
        &mut out,
    )
    .await
    .expect("json mode runs");
    String::from_utf8(out)
        .expect("utf8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect()
}

/// Run one prompt through json mode and return the emitted lines, parsed.
async fn run_once(responses: Vec<cyrup_core::AssistantMessage>) -> Vec<Value> {
    let raw = run_once_raw(responses).await;
    parse_lines(raw.join("\n").as_bytes())
}

fn updates(lines: &[Value]) -> Vec<&Value> {
    lines
        .iter()
        .filter(|l| kind(l) == "message_update")
        .collect()
}

// ----------------------------------------------------------------------------------------------
// The projection
// ----------------------------------------------------------------------------------------------

/// Pi json-event.ts:35/:39 build a fresh TWO-key object, so BOTH cumulative snapshots leave the
/// wire. Pi asserts exactly this pair at 7290-json-stream-linear.test.ts:30-33:
/// `expect(update).not.toHaveProperty("message")` and
/// `expect(update.assistantMessageEvent).not.toHaveProperty("partial")`.
#[tokio::test]
async fn message_update_carries_no_cumulative_snapshot() {
    let lines = run_once(vec![faux_assistant_message(
        vec![faux_text("Hello world")],
        StopReason::Stop,
    )])
    .await;

    let updates = updates(&lines);
    assert!(!updates.is_empty(), "no message_update emitted: {lines:?}");

    for update in &updates {
        // The outer cumulative `AgentMessage` snapshot.
        assert!(
            update.get("message").is_none(),
            "message_update must not carry the cumulative `message` (Pi json-event.ts:35,:39 build \
             a fresh two-key object with no spread): {update}"
        );
        // The record has EXACTLY the two keys Pi emits — not merely "message happens to be absent".
        let obj = update.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["assistantMessageEvent", "type"],
            "message_update is exactly {{type, assistantMessageEvent}}: {update}"
        );
        // The inner cumulative `AssistantMessage` snapshot.
        let delta = &update["assistantMessageEvent"];
        assert!(
            delta.get("partial").is_none(),
            "assistantMessageEvent must not carry `partial` (Pi's rest-destructure, \
             json-event.ts:38): {update}"
        );
    }
}

/// Byte-for-byte against Pi's own documented example, `coding-agent/docs/rpc.md:946-949`:
/// ```json
/// {"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}
/// {"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello"}}
/// {"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"Hello world"}}
/// ```
/// Key ORDER is part of the assertion: `serde_json::to_string` preserves insertion order, so this
/// pins the projection's field emission order to Pi's, not just its key set.
#[tokio::test]
async fn text_stream_records_match_pi_byte_for_byte() {
    let rendered: Vec<String> = run_once_raw(vec![faux_assistant_message(
        vec![faux_text("Hello world")],
        StopReason::Stop,
    )])
    .await
    .into_iter()
    .filter(|l| l.contains(r#""type":"message_update""#))
    .collect();

    let start = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#;
    let end = r#"{"type":"message_update","assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"Hello world"}}"#;
    assert!(
        rendered.iter().any(|l| l == start),
        "rpc.md:946 text_start line not emitted verbatim: {rendered:?}"
    );
    assert!(
        rendered.iter().any(|l| l == end),
        "rpc.md:948 text_end line not emitted verbatim: {rendered:?}"
    );
    // Every text_delta is exactly {type, contentIndex, delta} in that order.
    for line in &rendered {
        if line.contains(r#""type":"text_delta""#) {
            assert!(
                line.starts_with(
                    r#"{"type":"message_update","assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"#
                ) && line.ends_with("}}"),
                "rpc.md:947 shape violated: {line}"
            );
        }
    }
}

/// A tool call streams through the same projection: `toolcall_end` keeps its completed `toolCall`
/// (rpc.md:944 "Tool call ended (includes full `toolCall` object)") and loses only `partial`.
#[tokio::test]
async fn toolcall_updates_keep_their_payload_and_lose_only_partial() {
    let lines = run_once(vec![
        faux_assistant_message(
            vec![faux_tool_call(
                "write",
                serde_json::json!({"path": "hi.txt", "content": "x"}),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ])
    .await;

    let mut saw_end = false;
    for update in updates(&lines) {
        let delta = &update["assistantMessageEvent"];
        assert!(delta.get("partial").is_none(), "partial leaked: {update}");
        if kind(delta) == "toolcall_end" {
            saw_end = true;
            assert!(
                delta.get("toolCall").is_some(),
                "toolcall_end must keep its completed toolCall (rpc.md:944): {update}"
            );
            assert_eq!(
                delta["toolCall"]["name"], "write",
                "the completed call survives the projection: {update}"
            );
        }
    }
    assert!(saw_end, "no toolcall_end emitted: {lines:?}");
}

// ----------------------------------------------------------------------------------------------
// MIRROR — the projection must not be over-broad
// ----------------------------------------------------------------------------------------------

/// Pi json-event.ts:29-31 returns EVERY other event type by identity. In particular
/// `message_start` / `message_end` still carry their full `message` — rpc.md:954 makes
/// `message_end.message` the AUTHORITATIVE final message, so stripping it would break the very
/// reassembly the projection tells clients to do.
#[tokio::test]
async fn every_other_event_type_is_untouched() {
    let lines = run_once(vec![faux_assistant_message(
        vec![faux_text("Hello world")],
        StopReason::Stop,
    )])
    .await;

    let mut saw_start = false;
    let mut saw_end = false;
    for line in &lines {
        match kind(line) {
            "message_update" => {}
            "message_start" => {
                saw_start = true;
                assert!(
                    line.get("message").is_some(),
                    "message_start keeps its message (rpc.md:953 tells clients to seed from it): \
                     {line}"
                );
            }
            "message_end" => {
                saw_end = true;
                assert!(
                    line.get("message").is_some(),
                    "message_end.message is authoritative (rpc.md:954): {line}"
                );
            }
            "agent_end" => assert!(
                line.get("messages").is_some(),
                "agent_end keeps its messages array: {line}"
            ),
            _ => {}
        }
    }
    assert!(saw_start && saw_end, "missing lifecycle events: {lines:?}");
}

/// The terminal delta variants (`done`/`error`) have no `partial` at all — Pi's
/// `!("partial" in assistantMessageEvent)` branch (json-event.ts:34-36) returns them untouched, so
/// `done.message` must survive. This is the arm that would regress if the projection stripped by
/// key name instead of by variant.
#[tokio::test]
async fn terminal_delta_variants_keep_their_final_message() {
    let lines = run_once(vec![faux_assistant_message(
        vec![faux_text("Hello world")],
        StopReason::Stop,
    )])
    .await;

    let terminals: Vec<&Value> = updates(&lines)
        .into_iter()
        .map(|u| &u["assistantMessageEvent"])
        .filter(|d| matches!(kind(d), "done" | "error"))
        .collect();

    for terminal in &terminals {
        assert!(
            terminal.get("reason").is_some(),
            "a terminal delta keeps its reason: {terminal}"
        );
        let payload = if kind(terminal) == "done" {
            terminal.get("message")
        } else {
            terminal.get("error")
        };
        assert!(
            payload.is_some(),
            "a terminal delta keeps its final message (json-event.ts:34-36 returns it untouched): \
             {terminal}"
        );
    }
}

// ----------------------------------------------------------------------------------------------
// The SECOND serializer — rpc stdout
// ----------------------------------------------------------------------------------------------

/// Pi applies `toJsonEvent` at exactly two call sites: `print-mode.ts:110` (json mode, covered
/// above) and `rpc/rpc-mode.ts:356` (`output(toJsonEvent(event))`). This is the second one. It
/// carries its own assertion because rpc reaches the wire through a different path —
/// `RpcOut`'s `Serialize` — so a projection applied to only one serializer passes every test above.
///
/// The MIRROR half is in the same test: an rpc `response` record is untouched, proving the change
/// is scoped to the event lane and did not disturb the untagged `RpcOut::Response` shape.
#[tokio::test]
async fn rpc_stdout_projects_events_and_leaves_responses_alone() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("Hello world")],
        StopReason::Stop,
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let input = concat!(
        r#"{"type":"prompt","id":"1","message":"go"}"#,
        "\n",
        r#"{"type":"get_state","id":"2"}"#,
        "\n",
    );
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, Cursor::new(input.as_bytes().to_vec()), &mut out)
        .await
        .expect("rpc mode runs");

    let raw: Vec<String> = String::from_utf8(out)
        .expect("utf8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_owned)
        .collect();

    let update_lines: Vec<&String> = raw
        .iter()
        .filter(|l| l.contains(r#""type":"message_update""#))
        .collect();
    assert!(
        !update_lines.is_empty(),
        "rpc emitted no message_update: {raw:?}"
    );
    for line in &update_lines {
        let value: Value = serde_json::from_str(line).expect("json");
        assert!(
            value.get("message").is_none(),
            "rpc message_update must not carry the cumulative message (rpc-mode.ts:356): {line}"
        );
        assert!(
            value["assistantMessageEvent"].get("partial").is_none(),
            "rpc message_update must not carry partial: {line}"
        );
    }
    // rpc.md:946 — the documented line, verbatim, on the rpc stream too.
    assert!(
        raw.iter().any(|l| l
            == r#"{"type":"message_update","assistantMessageEvent":{"type":"text_start","contentIndex":0}}"#),
        "rpc.md:946 line not emitted verbatim on the rpc stream: {update_lines:?}"
    );

    // MIRROR: the response lane is unchanged — still `{"type":"response","id":…,"success":…}`.
    let responses: Vec<&String> = raw
        .iter()
        .filter(|l| l.contains(r#""type":"response""#))
        .collect();
    assert!(!responses.is_empty(), "rpc emitted no response: {raw:?}");
    for line in &responses {
        let value: Value = serde_json::from_str(line).expect("json");
        assert_eq!(value["type"], "response", "response lane intact: {line}");
        assert!(
            value.get("id").is_some() && value.get("success").is_some(),
            "an untagged RpcOut::Response still serializes as its inner value: {line}"
        );
    }
}

// ----------------------------------------------------------------------------------------------
// Linearity — the point of the change
// ----------------------------------------------------------------------------------------------

/// Pi 7290-json-stream-linear.test.ts:37-43: doubling the response length must not more than
/// ~double the bytes spent on `message_update` records. With the cumulative snapshots on the wire
/// the growth is QUADRATIC (each of N deltas re-emits an O(N)-sized message *twice*), so this ratio
/// blows past the bound.
#[tokio::test]
async fn message_update_bytes_scale_linearly() {
    async fn update_bytes(text: &str) -> usize {
        let lines = run_once(vec![faux_assistant_message(
            vec![faux_text(text)],
            StopReason::Stop,
        )])
        .await;
        let updates = updates(&lines);
        assert!(!updates.is_empty(), "no message_update emitted");
        updates
            .iter()
            .map(|u| serde_json::to_string(u).expect("render").len())
            .sum()
    }

    let small = update_bytes(&"x".repeat(2_000)).await;
    let large = update_bytes(&"x".repeat(4_000)).await;

    assert!(large > small, "the larger response must cost more bytes");
    // Pi's bound, verbatim (`expect(largeBytes / smallBytes).toBeLessThan(2.2)`).
    assert!(
        (large as f64) / (small as f64) < 2.2,
        "message_update stream is not linear: {small} -> {large} bytes (ratio {:.2}, Pi's bound is \
         2.2)",
        (large as f64) / (small as f64)
    );
}
