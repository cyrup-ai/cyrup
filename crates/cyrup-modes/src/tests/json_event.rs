//! The json/rpc stdout wire projection (Pi `coding-agent/src/modes/json-event.ts`, v0.84.4).
//!
//! Proves that `message_update` records leave the json and rpc stdout streams DELTA-ONLY: the
//! cumulative outer `message` and the inner `assistantMessageEvent.partial` are both absent, exactly
//! as Pi's `toJsonEvent` (json-event.ts:48-61 @v0.84.4) produces and
//! `coding-agent/docs/rpc.md:938-995` @v0.84.4 documents — while the CONSTANT-sized metadata pi
//! kept in the v0.84.1→v0.84.4 window (SEAM-117) is present: the top-level cumulative `usage`
//! (json-event.ts:58) and, on `toolcall_start`, the call's `id` + `toolName` (json-event.ts:23-30).
//! Mirrors Pi's own regression tests, `coding-agent/test/suite/regressions/7290-json-stream-linear.test.ts`,
//! `7911-json-stream-usage.test.ts` and `7925-toolcall-start-metadata.test.ts`.
//!
//! Every wire test drives the REAL adapter over a scripted `FauxProvider` and asserts on the
//! produced bytes, so it is the wire that is under test, not the projection helper in isolation.
//! The two `throw` branches pi's projector has are pinned on the helper directly, because no
//! provider stream can reach them.

use std::io::Cursor;
use std::sync::Arc;

use crate::{run_json, run_rpc, to_json_event};
use cyrup_core::{StopReason, Usage};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, faux_assistant_message, faux_text, faux_tool_call, faux_tool_call_with_id,
};
use cyrup_session_svc::{
    AgentMessage, AgentSessionEvent, AgentSessionRuntime, InputSource, SessionFactory, StreamEvent,
    UserInput,
};
use serde_json::Value;

use super::support::{Fixture, base_config, create_runtime, fixture, kind, parse_lines};

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

/// Pi json-event.ts:56-60 @v0.84.4 builds a fresh THREE-key object (`type`, `usage`,
/// `assistantMessageEvent`), so BOTH cumulative snapshots leave the wire. Pi asserts exactly this
/// pair at 7290-json-stream-linear.test.ts:30-33: `expect(update).not.toHaveProperty("message")`
/// and `expect(update.assistantMessageEvent).not.toHaveProperty("partial")`.
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
            "message_update must not carry the cumulative `message` (Pi json-event.ts:56-60 builds \
             a fresh object with no spread): {update}"
        );
        // The record has EXACTLY the three keys Pi emits (json-event.ts:56-60 @v0.84.4) — not
        // merely "message happens to be absent".
        let obj = update.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["assistantMessageEvent", "type", "usage"],
            "message_update is exactly {{type, usage, assistantMessageEvent}}: {update}"
        );
        // The inner cumulative `AssistantMessage` snapshot.
        let delta = &update["assistantMessageEvent"];
        assert!(
            delta.get("partial").is_none(),
            "assistantMessageEvent must not carry `partial` (Pi's rest-destructure, \
             json-event.ts:36): {update}"
        );
    }
}

/// Byte-for-byte against Pi's own documented example, `coding-agent/docs/rpc.md:977-980` @v0.84.4:
/// ```json
/// {"type":"message_update","usage":{...},"assistantMessageEvent":{"type":"text_start","contentIndex":0}}
/// {"type":"message_update","usage":{...},"assistantMessageEvent":{"type":"text_delta","contentIndex":0,"delta":"Hello"}}
/// {"type":"message_update","usage":{...},"assistantMessageEvent":{"type":"text_end","contentIndex":0,"content":"Hello world"}}
/// ```
/// Key ORDER is part of the assertion: `serde_json::to_string` preserves insertion order, so this
/// pins the projection's field emission order to Pi's (`type`, `usage`, `assistantMessageEvent` —
/// json-event.ts:56-60), not just its key set. The `{...}` the docs elide is pinned by
/// [`split_update`]: it must be a [`Usage`] that re-renders to the same bytes (so its own key order
/// is the struct's, `input, output, cacheRead, cacheWrite, totalTokens, cost` — pi `Usage`,
/// `ai/src/types.ts:382-403`).
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

    let start = r#"{"type":"text_start","contentIndex":0}"#;
    let end = r#"{"type":"text_end","contentIndex":0,"content":"Hello world"}"#;
    assert!(
        rendered
            .iter()
            .any(|l| split_update(l).is_some_and(|(_, event)| event == start)),
        "rpc.md:977 text_start line not emitted verbatim: {rendered:?}"
    );
    assert!(
        rendered
            .iter()
            .any(|l| split_update(l).is_some_and(|(_, event)| event == end)),
        "rpc.md:980 text_end line not emitted verbatim: {rendered:?}"
    );
    // Every text_delta is exactly {type, contentIndex, delta} in that order, behind the usage.
    for line in &rendered {
        if line.contains(r#""type":"text_delta""#) {
            let (_, event) = split_update(line)
                .unwrap_or_else(|| panic!("rpc.md:978 shape violated (no leading usage): {line}"));
            assert!(
                event.starts_with(r#"{"type":"text_delta","contentIndex":0,"delta":"#)
                    && event.ends_with('}'),
                "rpc.md:978 shape violated: {line}"
            );
        }
    }
}

/// Split a `message_update` wire line into its `usage` bytes and its `assistantMessageEvent`
/// bytes — `None` unless the line is EXACTLY
/// `{"type":"message_update","usage":<U>,"assistantMessageEvent":<E>}` (pi's key order,
/// json-event.ts:56-60) with `<U>` a [`Usage`] whose canonical rendering is byte-identical to what
/// is on the wire. The first `,"assistantMessageEvent":` is the boundary: a `Usage` object cannot
/// contain that key.
fn split_update(line: &str) -> Option<(&str, &str)> {
    let head = r#"{"type":"message_update","usage":"#;
    let sep = r#","assistantMessageEvent":"#;
    let body = line.strip_prefix(head)?.strip_suffix('}')?;
    let (usage, event) = body.split_once(sep)?;
    let parsed: Usage = serde_json::from_str(usage).ok()?;
    (serde_json::to_string(&parsed).ok()? == usage).then_some((usage, event))
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
    // rpc.md:977 @v0.84.4 — the documented line, verbatim (usage included), on the rpc stream too.
    assert!(
        raw.iter().any(|l| split_update(l)
            .is_some_and(|(_, event)| event == r#"{"type":"text_start","contentIndex":0}"#)),
        "rpc.md:977 line not emitted verbatim on the rpc stream: {update_lines:?}"
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

// ----------------------------------------------------------------------------------------------
// SEAM-117 — the constant-sized metadata pi kept (v0.84.1 → v0.84.4)
// ----------------------------------------------------------------------------------------------

/// Pi 7911-json-stream-usage.test.ts:27-30 — `toJsonEvent(update).usage` equals
/// `update.message.usage` (json-event.ts:58 reads it off the OUTER cumulative message), while
/// `message` and `partial` stay absent. rpc.md:983-984 @v0.84.4: "The top-level `usage` field
/// contains the latest cumulative provider-reported usage." The authoritative comparand on the
/// wire is `message_end.message.usage` (rpc.md:992-993), which for a scripted faux turn is the
/// same estimate every partial carries.
#[tokio::test]
async fn message_update_carries_pi_s_cumulative_usage() {
    let lines = run_once(vec![faux_assistant_message(
        vec![faux_text("Hello world")],
        StopReason::Stop,
    )])
    .await;

    // The USER turn gets a `message_start`/`message_end` pair too (no usage on it); pi's test
    // filters on `message.role === "assistant"` (7911-json-stream-usage.test.ts:22) — same here.
    let end_usage = lines
        .iter()
        .filter(|l| kind(l) == "message_end")
        .filter_map(|l| l.get("message"))
        .find(|m| m["role"] == "assistant")
        .and_then(|m| m.get("usage"))
        .cloned()
        .expect("the assistant message_end carries message.usage");
    assert!(
        end_usage["totalTokens"].as_u64().unwrap_or(0) > 0,
        "the faux estimate must populate usage or this test proves nothing: {end_usage}"
    );

    let updates = updates(&lines);
    assert!(!updates.is_empty(), "no message_update emitted: {lines:?}");
    for update in &updates {
        let usage = update.get("usage").unwrap_or_else(|| {
            panic!("message_update must carry the cumulative usage (json-event.ts:58): {update}")
        });
        assert_eq!(
            usage, &end_usage,
            "usage is the outer message's, verbatim (7911-json-stream-usage.test.ts:28): {update}"
        );
        // Typed, so a missing or renamed Usage field cannot pass as "some object".
        let typed: Usage =
            serde_json::from_value(usage.clone()).expect("usage deserializes as cyrup_core::Usage");
        assert_eq!(
            typed.total_tokens,
            end_usage["totalTokens"].as_u64().unwrap_or(0)
        );
        assert!(
            update.get("message").is_none()
                && update["assistantMessageEvent"].get("partial").is_none(),
            "usage rides WITHOUT the cumulative snapshots (7911 test :29-30): {update}"
        );
    }
}

/// Pi 7925-toolcall-start-metadata.test.ts:32-41 — the `toolcall_start` update is EXACTLY
/// `{type, usage, assistantMessageEvent: {type:"toolcall_start", contentIndex:0, id:"call_7925",
/// toolName:"write"}}`, resolved by indexing `partial.content[contentIndex]` before `partial` is
/// dropped (json-event.ts:23-30). rpc.md:971 @v0.84.4: "Tool call started (includes `id` and
/// `toolName`)"; `:988` gives the line shape. The item's Verify paragraph asks that the pair match
/// what `toolcall_end.toolCall` later reports for the same `contentIndex` — asserted here too.
#[tokio::test]
async fn toolcall_start_carries_the_call_id_and_tool_name() {
    let raw = run_once_raw(vec![
        faux_assistant_message(
            vec![faux_tool_call_with_id(
                "write",
                serde_json::json!({"path": "output.txt", "content": "x".repeat(100)}),
                Some("call_7925".to_string()),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ])
    .await;
    let lines = parse_lines(raw.join("\n").as_bytes());

    let start = updates(&lines)
        .into_iter()
        .find(|u| kind(&u["assistantMessageEvent"]) == "toolcall_start")
        .cloned()
        .unwrap_or_else(|| panic!("no toolcall_start emitted: {lines:?}"));
    let end = updates(&lines)
        .into_iter()
        .find(|u| kind(&u["assistantMessageEvent"]) == "toolcall_end")
        .cloned()
        .unwrap_or_else(|| panic!("no toolcall_end emitted: {lines:?}"));

    assert_eq!(
        start["assistantMessageEvent"],
        serde_json::json!({
            "type": "toolcall_start",
            "contentIndex": 0,
            "id": "call_7925",
            "toolName": "write",
        }),
        "7925-toolcall-start-metadata.test.ts:35-40 shape: {start}"
    );
    let mut keys: Vec<&str> = start
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["assistantMessageEvent", "type", "usage"]);
    assert_eq!(
        start["usage"], end["usage"],
        "usage rides on the toolcall_start update too: {start}"
    );

    // Verify: the pair matches the completed call at the same contentIndex.
    assert_eq!(end["assistantMessageEvent"]["contentIndex"], 0);
    assert_eq!(
        start["assistantMessageEvent"]["id"], end["assistantMessageEvent"]["toolCall"]["id"],
        "toolcall_start.id must be the id toolcall_end.toolCall reports"
    );
    assert_eq!(
        start["assistantMessageEvent"]["toolName"],
        end["assistantMessageEvent"]["toolCall"]["name"],
        "toolcall_start.toolName must be the name toolcall_end.toolCall reports"
    );

    // rpc.md:988 @v0.84.4, byte order: `{ ...deltaEvent, id, toolName }` (json-event.ts:29) puts
    // `id` and `toolName` AFTER the stripped delta's own `type` and `contentIndex`.
    let documented =
        r#"{"type":"toolcall_start","contentIndex":0,"id":"call_7925","toolName":"write"}"#;
    assert!(
        raw.iter()
            .any(|l| split_update(l).is_some_and(|(_, event)| event == documented)),
        "rpc.md:988 toolcall_start line not emitted verbatim: {raw:?}"
    );
}

/// Pi json-event.ts:52-54 throws `message_update message is not an assistant message` when the
/// outer message is not the assistant arm. No provider stream can produce that event (the agent
/// only ever emits `AgentMessage::Assistant` on `message_update`), so it is pinned on the helper:
/// the serializer REFUSES rather than inventing a usage, and the refusal reaches the write sites
/// as an error (`json.rs` / `rpc/jsonl.rs` both `?` the serialization).
#[test]
fn message_update_with_a_non_assistant_message_is_refused_like_pi_s_throw() {
    let event = AgentSessionEvent::MessageUpdate {
        message: AgentMessage::User {
            content: vec![faux_text("not an assistant turn")],
            timestamp: None,
        },
        assistant_message_event: Box::new(StreamEvent::TextStart {
            content_index: 0,
            partial: Arc::new(faux_assistant_message(vec![], StopReason::Stop)),
        }),
    };
    let err = serde_json::to_string(&to_json_event(&event))
        .expect_err("a non-assistant message_update must not serialize");
    assert!(
        err.to_string()
            .contains("message_update message is not an assistant message"),
        "pi's exact throw text (json-event.ts:53): {err}"
    );
}

/// Pi json-event.ts:24-27 throws `toolcall_start content at index N is not a tool call` when the
/// cumulative partial has no tool call at `contentIndex` — both for a block of another kind and
/// for an out-of-range index (`toolCall?.type !== "toolCall"` is true for `undefined`). Pinned on
/// the helper for the same reason as its sibling.
#[test]
fn toolcall_start_without_a_tool_call_at_its_index_is_refused_like_pi_s_throw() {
    let partial = Arc::new(faux_assistant_message(
        vec![faux_text("a text block, not a tool call")],
        StopReason::Stop,
    ));
    for content_index in [0, 5] {
        let event = AgentSessionEvent::MessageUpdate {
            message: AgentMessage::Assistant(Arc::clone(&partial)),
            assistant_message_event: Box::new(StreamEvent::ToolCallStart {
                content_index,
                partial: Arc::clone(&partial),
            }),
        };
        let err = serde_json::to_string(&to_json_event(&event))
            .expect_err("a toolcall_start with no tool call at its index must not serialize");
        assert!(
            err.to_string().contains(&format!(
                "toolcall_start content at index {content_index} is not a tool call"
            )),
            "pi's exact throw text (json-event.ts:26): {err}"
        );
    }
}
