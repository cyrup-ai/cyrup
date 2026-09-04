//! Byte-diff payload-fidelity tests (L4 WIT residuals): each guest event-payload struct must
//! serialize to the EXACT Pi `extensions/types.ts` shape — field names, optionality, and the set of
//! keys. Pi (`packages/coding-agent/src/core/extensions/types.ts`) is ground truth; each test cites
//! the interface and `serde_json::to_value`-compares the struct against the literal Pi object.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::events::*;
use serde_json::json;

/// `tool_result` (Pi `ToolResultEventBase` + subtype, types.ts:883-929): the prior struct dropped
/// `input` and `details`, and later `usage` (types.ts:919-921). Full shape:
/// `{toolCallId, toolName, input, content, isError, details, usage?}`.
#[test]
fn tool_result_carries_input_details_and_usage() {
    let ev = ToolResultEvent {
        call_id: "call_1".into(),
        name: "bash".into(),
        input: json!({ "command": "ls" }),
        content: json!([{ "type": "text", "text": "ok" }]),
        is_error: false,
        details: Some(json!({ "exitCode": 0 })),
        usage: Some(json!({ "input": 3, "output": 4 })),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({
            "toolCallId": "call_1",
            "toolName": "bash",
            "input": { "command": "ls" },
            "content": [{ "type": "text", "text": "ok" }],
            "isError": false,
            "details": { "exitCode": 0 },
            "usage": { "input": 3, "output": 4 }
        })
    );
}

/// `details` and `usage` are `undefined`-optional (Pi `WriteToolResultEvent.details: undefined`,
/// types.ts:908; `ToolResultEventBase.usage?`, types.ts:921): `None` must be OMITTED, not
/// serialized as `null`.
#[test]
fn tool_result_omits_absent_details_and_usage() {
    let ev = ToolResultEvent {
        call_id: "c".into(),
        name: "write".into(),
        input: json!({}),
        content: json!([]),
        is_error: false,
        details: None,
        usage: None,
    };
    let v = serde_json::to_value(&ev).unwrap();
    assert!(!v.as_object().unwrap().contains_key("details"));
    assert!(!v.as_object().unwrap().contains_key("usage"));
}

/// `input` (Pi `InputEvent`, types.ts:800-810): the prior struct dropped `images`, `source`, and
/// `streamingBehavior`. Full shape: `{text, images?, source, streamingBehavior?}`.
#[test]
fn input_carries_images_source_and_streaming_behavior() {
    let ev = InputEvent {
        text: "hello".into(),
        images: Some(json!([{ "type": "image", "data": "..." }])),
        source: "interactive".into(),
        streaming_behavior: Some("steer".into()),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({
            "text": "hello",
            "images": [{ "type": "image", "data": "..." }],
            "source": "interactive",
            "streamingBehavior": "steer"
        })
    );
}

/// `images?`/`streamingBehavior?` are Pi-optional (types.ts:805,809): `None` must be omitted.
#[test]
fn input_omits_absent_optionals() {
    let ev = InputEvent {
        text: "hi".into(),
        images: None,
        source: "rpc".into(),
        streaming_behavior: None,
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({ "text": "hi", "source": "rpc" })
    );
}

/// `user_bash` (Pi `UserBashEvent`, types.ts:782-790): the prior struct carried `operations` (a
/// RESULT field, not an event field). Full shape: `{command, excludeFromContext, cwd}`.
#[test]
fn user_bash_is_command_exclude_cwd() {
    let ev = UserBashEvent {
        command: "ls -la".into(),
        exclude_from_context: true,
        cwd: "/work".into(),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({ "command": "ls -la", "excludeFromContext": true, "cwd": "/work" })
    );
}

/// `message_start` (Pi `MessageStartEvent`, types.ts:711-715): the prior struct collapsed to
/// `{role}`. Full shape: `{message}` (the full AgentMessage).
#[test]
fn message_start_carries_full_message() {
    let ev = MessageStartEvent {
        message: json!({ "role": "assistant", "content": [] }),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({ "message": { "role": "assistant", "content": [] } })
    );
}

/// `message_update` (Pi `MessageUpdateEvent`, types.ts:717-722): the prior struct dropped the full
/// `message`. Full shape: `{message, assistantMessageEvent}`.
#[test]
fn message_update_carries_message_and_delta() {
    let ev = MessageUpdateEvent {
        message: json!({ "role": "assistant", "content": [{ "type": "text", "text": "p" }] }),
        assistant_message_event: json!({ "type": "text_delta", "delta": "p" }),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "p" }] },
            "assistantMessageEvent": { "type": "text_delta", "delta": "p" }
        })
    );
}

/// `turn_end` (Pi `TurnEndEvent`, types.ts:703-709): the prior struct dropped `toolResults`. Full
/// shape: `{turnIndex, message, toolResults}`.
#[test]
fn turn_end_carries_tool_results() {
    let ev = TurnEndEvent {
        turn_index: 3,
        message: json!({ "role": "assistant" }),
        tool_results: json!([{ "toolCallId": "c", "toolName": "bash", "content": [], "isError": false, "timestamp": 0 }]),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({
            "turnIndex": 3,
            "message": { "role": "assistant" },
            "toolResults": [{ "toolCallId": "c", "toolName": "bash", "content": [], "isError": false, "timestamp": 0 }]
        })
    );
}

/// `session_compact` (Pi `SessionCompactEvent`, types.ts:588-597): the prior struct invented a flat
/// `summary`. Full shape: `{compactionEntry, fromExtension, reason, willRetry}`.
#[test]
fn session_compact_is_pi_shaped() {
    let ev = SessionCompactEvent {
        compaction_entry: json!({ "summary": "did things", "type": "compaction" }),
        from_extension: false,
        reason: "manual".into(),
        will_retry: false,
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({
            "compactionEntry": { "summary": "did things", "type": "compaction" },
            "fromExtension": false,
            "reason": "manual",
            "willRetry": false
        })
    );
}

/// `session_before_compact` (Pi `SessionBeforeCompactEvent`, types.ts:577-587). Serializable shape:
/// `{preparation, branchEntries, customInstructions?, reason, willRetry}` — the computed
/// `CompactionPreparation` now crosses the seam (L4 gap #5); the non-serializable `signal` is omitted.
#[test]
fn session_before_compact_is_pi_shaped() {
    let ev = SessionBeforeCompactEvent {
        preparation: json!({ "firstKeptEntryId": "e3", "tokensBefore": 42 }),
        branch_entries: json!([{ "id": "e1" }]),
        custom_instructions: Some("be terse".into()),
        reason: "threshold".into(),
        will_retry: true,
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({
            "preparation": { "firstKeptEntryId": "e3", "tokensBefore": 42 },
            "branchEntries": [{ "id": "e1" }],
            "customInstructions": "be terse",
            "reason": "threshold",
            "willRetry": true
        })
    );
    // `customInstructions?` is Pi-optional (types.ts:581): None must be omitted.
    let ev2 = SessionBeforeCompactEvent {
        preparation: json!({}),
        branch_entries: json!([]),
        custom_instructions: None,
        reason: "overflow".into(),
        will_retry: false,
    };
    let v = serde_json::to_value(&ev2).unwrap();
    assert!(!v.as_object().unwrap().contains_key("customInstructions"));
}

/// `tool_call` (Pi `ToolCallEventBase`, types.ts:822-865): shape `{toolCallId, toolName, input}`.
#[test]
fn tool_call_is_pi_shaped() {
    let ev = ToolCallEvent {
        call_id: "c".into(),
        name: "read".into(),
        input: json!({ "path": "/x" }),
    };
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        json!({ "toolCallId": "c", "toolName": "read", "input": { "path": "/x" } })
    );
}
