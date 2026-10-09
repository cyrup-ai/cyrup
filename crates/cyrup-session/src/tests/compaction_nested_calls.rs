//! CODE-006 — compaction's file-operation tracking includes the calls a tool made.
//!
//! Ports `packages/coding-agent/test/compaction-nested-calls.test.ts` @v1.0.1 ("include files
//! touched by nested calls recorded on tool results") and drives the same fact through the
//! production entry point, [`prepare_compaction`], which is where `FileOps` is fed from a session
//! branch.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;

use crate::agent_message::AgentMessage;
use crate::compaction::FileOps;
use crate::compaction::prepare_compaction;
use crate::compaction::tokens::TokenCache;
use crate::{CompactionSettings, Entry, NewSessionOpts, SessionManager};
use cyrup_core::{
    AssistantMessage, Content, Message, NestedCallStatus, NestedToolCallRecord, NestedToolCalls,
    StopReason, ToolCallId, Usage,
};
use serde_json::json;

fn record(
    id: &str,
    name: &str,
    arguments: Option<serde_json::Value>,
    arguments_bytes: Option<u64>,
) -> NestedToolCallRecord {
    NestedToolCallRecord {
        id: id.to_string(),
        name: name.to_string(),
        status: NestedCallStatus::Ok,
        arguments: arguments.and_then(|a| a.as_object().cloned()),
        arguments_bytes,
        duration_ms: None,
        error: None,
    }
}

/// The result of pi's test: a `codemode` result whose script read `a.ts`, edited `b.ts` and wrote a
/// file whose arguments were too large to record.
fn codemode_result() -> Message {
    Message::ToolResult {
        duration_ms: None,
        tool_call_id: ToolCallId::from("codemode-1"),
        tool_name: "codemode".to_string(),
        content: vec![],
        is_error: false,
        details: None,
        usage: None,
        added_tool_names: Vec::new(),
        timestamp: 0,
        nested_calls: Some(NestedToolCalls {
            calls: vec![
                record(
                    "codemode-1/1",
                    "read",
                    Some(json!({ "path": "a.ts" })),
                    None,
                ),
                record(
                    "codemode-1/2",
                    "edit",
                    Some(json!({ "path": "b.ts", "edits": [] })),
                    None,
                ),
                record("codemode-1/3", "write", None, Some(40000)),
            ],
            complete: false,
        }),
    }
}

/// compaction-nested-calls.test.ts "include files touched by nested calls recorded on tool
/// results": `readFiles: ["a.ts"], modifiedFiles: ["b.ts"]`.
#[test]
fn include_files_touched_by_nested_calls_recorded_on_tool_results() {
    let mut ops = FileOps::default();
    ops.absorb_message(&codemode_result());
    let (read, modified) = ops.compute_lists();
    assert_eq!(read, vec!["a.ts".to_string()]);
    assert_eq!(modified, vec!["b.ts".to_string()]);
}

/// The same record reached through the raw `AgentMessage` the compaction preparation walks.
#[test]
fn nested_calls_are_read_off_the_raw_agent_message_too() {
    let mut ops = FileOps::default();
    ops.absorb_agent_message(&AgentMessage::Core(codemode_result()));
    assert_eq!(
        ops.compute_lists(),
        (vec!["a.ts".to_string()], vec!["b.ts".to_string()])
    );
}

/// A result with no nested calls contributes nothing (pi `message.nestedCalls?.calls ?? []`).
#[test]
fn a_tool_result_without_nested_calls_contributes_nothing() {
    let mut ops = FileOps::default();
    ops.absorb_message(&Message::ToolResult {
        duration_ms: None,
        tool_call_id: ToolCallId::from("t"),
        tool_name: "read".to_string(),
        content: vec![Content::text("ok")],
        is_error: false,
        details: None,
        usage: None,
        added_tool_names: Vec::new(),
        timestamp: 0,
        nested_calls: None,
    });
    assert_eq!(ops.compute_lists(), (vec![], vec![]));
}

/// Only `read`, `write` and `edit` track, by exact name and only from `path` — for a nested call as
/// for a model-issued one.
#[test]
fn nested_calls_follow_the_same_tool_and_argument_rules_as_model_calls() {
    let mut ops = FileOps::default();
    ops.absorb_message(&Message::ToolResult {
        duration_ms: None,
        tool_call_id: ToolCallId::from("t"),
        tool_name: "codemode".to_string(),
        content: vec![],
        is_error: false,
        details: None,
        usage: None,
        added_tool_names: Vec::new(),
        timestamp: 0,
        nested_calls: Some(NestedToolCalls {
            calls: vec![
                record("t/1", "multiedit", Some(json!({ "path": "no.ts" })), None),
                record("t/2", "read", Some(json!({ "file_path": "no.ts" })), None),
                record("t/3", "write", Some(json!({ "path": "w.ts" })), None),
                record("t/4", "bash", Some(json!({ "command": "ls" })), None),
            ],
            complete: true,
        }),
    });
    assert_eq!(ops.compute_lists(), (vec![], vec!["w.ts".to_string()]));
}

fn assistant(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        provider: "faux".into(),
        model: "faux-1".into(),
        api: "faux".into(),
        response_model: None,
        response_id: None,
        provider_thinking_level: None,
        thinking_level: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        end_turn: None,
        timestamp: 0,
        duration_ms: None,
    })
}

/// The production path: a session branch whose summarized slice holds the script's result prepares
/// a compaction whose details list the script's files.
#[test]
fn prepare_compaction_lists_the_files_a_script_touched() {
    let cwd = PathBuf::from("/proj/nested");
    let mut m = SessionManager::in_memory(&cwd, NewSessionOpts::default()).unwrap();
    m.append_message(Message::User {
        content: vec![Content::text("run the script")],
        timestamp: 0,
    })
    .unwrap();
    m.append_message(assistant("calling codemode")).unwrap();
    m.append_message(codemode_result()).unwrap();
    m.append_message(assistant("done")).unwrap();
    for _ in 0..4 {
        m.append_message(Message::User {
            content: vec![Content::text("filler filler filler filler")],
            timestamp: 0,
        })
        .unwrap();
        m.append_message(assistant("filler filler filler filler filler"))
            .unwrap();
    }
    let path: Vec<Entry> = m.branch_path(None).into_iter().cloned().collect();
    let settings = CompactionSettings {
        enabled: true,
        reserve_tokens: 10,
        keep_recent_tokens: 5,
    };
    let prep =
        prepare_compaction(&path, &TokenCache::default(), &settings).expect("history to summarize");
    let details = prep.file_ops.to_details();
    assert_eq!(details.read_files, vec!["a.ts".to_string()]);
    assert_eq!(details.modified_files, vec!["b.ts".to_string()]);
}
