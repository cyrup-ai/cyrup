//! TUI-165 — the live tool row grows as the model writes the call's argument JSON.
//!
//! Upstream creates the `ToolExecutionComponent` the moment the streaming assistant message carries
//! a `tool_call` block, re-renders it on every argument delta via `updateArgs`
//! (`tool-execution.ts:170-173`, driven from `interactive-mode.ts:3551`), flips `argsComplete` in
//! `setArgsComplete()` (`:181-185`, called at `:3590`), and only then marks execution started
//! (`markExecutionStarted`, `:175-178`).
//!
//! cyrup created the row at `ToolExecutionStart` — which it emits with the arguments already
//! COMPLETE — so a call tree appeared whole instead of filling in, and for codemode the script never
//! appeared as it was generated. The three `toolcall*` frames reached `App::ingest_stream_event` and
//! fell to its `_` arm; the chain above them was already intact, which is why this is a `cyrup-tui`
//! change and not a four-crate one.
//!
//! `argsComplete` matters beyond display: upstream renderers BRANCH on it —
//! `renderers/edit.ts:186` computes its diff preview only once the arguments are final, and
//! `renderers/write.ts:153` keys its cache on it — so a renderer invoked mid-stream must be told.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{App, UiTheme};
use cyrup_agent::AgentMessage;
use cyrup_core::{
    ApiId, AssistantMessage, Content, LazyArgs, ProviderId, StopReason, ToolCall, ToolCallId,
};
use cyrup_provider::StreamEvent;
use cyrup_session_svc::AgentSessionEvent;
use ratatui::backend::TestBackend;
use std::sync::Arc;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 20), UiTheme::dark()).unwrap()
}

fn blank() -> AssistantMessage {
    let mut msg = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        StopReason::Stop,
        String::new(),
    );
    msg.error_message = None;
    msg
}

/// A partial assistant message whose single content block is a `tool_call` carrying `args` so far —
/// the snapshot every `toolcall*` frame rides (`StreamEvent::ToolCallDelta.partial`).
fn partial_with_call(args: serde_json::Value) -> Arc<AssistantMessage> {
    let mut msg = blank();
    let map = args.as_object().cloned().unwrap_or_default();
    msg.content = vec![Content::ToolCall(ToolCall {
        id: ToolCallId::from("call_1"),
        name: "write".into(),
        arguments: LazyArgs::from(map),
        thought_signature: None,
        namespace: None,
    })];
    Arc::new(msg)
}

/// Post one non-terminal frame the way `cyrup-agent` posts it (`agent.rs:827-831`).
fn feed(app: &mut App<TestBackend>, ev: StreamEvent) {
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(Arc::new(blank())),
        assistant_message_event: Box::new(ev),
    });
}

/// The live run's `(args, args_complete)`, or `None` when no row exists yet.
fn live_call(app: &App<TestBackend>) -> Option<(serde_json::Value, bool)> {
    app.state()
        .transcript
        .active_tools()
        .iter()
        .find(|r| r.call_id.as_deref() == Some("call_1"))
        .map(|r| (r.args.clone(), r.args_complete))
}

/// The Verify line: a call whose arguments arrive in three deltas updates the row three times, the
/// last with `argsComplete` true.
#[test]
fn a_calls_arguments_arrive_delta_by_delta_and_complete_at_the_end() {
    let mut app = app();

    // `toolcall_start` opens the block: the row must exist from here, as pi's does, even though no
    // argument text has arrived.
    feed(
        &mut app,
        StreamEvent::ToolCallStart {
            content_index: 0,
            partial: partial_with_call(serde_json::json!({})),
        },
    );
    let (args, complete) = live_call(&app).expect("the row exists from `toolcall_start`");
    assert_eq!(args, serde_json::json!({}), "nothing streamed yet");
    assert!(!complete, "arguments are still streaming");

    // Three deltas, each a longer snapshot of the same call.
    for (i, snapshot) in [
        serde_json::json!({ "file_path": "/tmp/a" }),
        serde_json::json!({ "file_path": "/tmp/a", "content": "he" }),
        serde_json::json!({ "file_path": "/tmp/a", "content": "hello" }),
    ]
    .into_iter()
    .enumerate()
    {
        feed(
            &mut app,
            StreamEvent::ToolCallDelta {
                content_index: 0,
                delta: "…".into(),
                partial: partial_with_call(snapshot.clone()),
            },
        );
        let (args, complete) = live_call(&app).expect("the row survives each delta");
        assert_eq!(args, snapshot, "delta {i} must reach the row");
        assert!(!complete, "delta {i} is not the end of the arguments");
    }

    // `toolcall_end` carries the finished `ToolCall`: pi's `setArgsComplete()`.
    let mut final_call = ToolCall {
        id: ToolCallId::from("call_1"),
        name: "write".into(),
        arguments: LazyArgs::from(
            serde_json::json!({ "file_path": "/tmp/a", "content": "hello" })
                .as_object()
                .cloned()
                .unwrap(),
        ),
        thought_signature: None,
        namespace: None,
    };
    final_call.name = "write".into();
    feed(
        &mut app,
        StreamEvent::ToolCallEnd {
            content_index: 0,
            tool_call: final_call,
            partial: partial_with_call(
                serde_json::json!({ "file_path": "/tmp/a", "content": "hello" }),
            ),
        },
    );
    let (args, complete) = live_call(&app).expect("the row survives the end frame");
    assert_eq!(
        args,
        serde_json::json!({ "file_path": "/tmp/a", "content": "hello" })
    );
    assert!(complete, "`toolcall_end` is `setArgsComplete()`");
}

/// The reconciliation half: the `ToolExecutionStart` that follows the streaming frames adopts the
/// row they built rather than pushing a second one for the same call — pi's `markExecutionStarted`
/// updates the component it already has (`tool-execution.ts:175-178`).
#[test]
fn tool_execution_start_adopts_the_streamed_row_instead_of_duplicating_it() {
    let mut app = app();
    feed(
        &mut app,
        StreamEvent::ToolCallStart {
            content_index: 0,
            partial: partial_with_call(serde_json::json!({ "file_path": "/tmp/a" })),
        },
    );
    assert_eq!(
        app.state().transcript.active_tools().len(),
        1,
        "one streamed row"
    );

    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call_1"),
        tool_name: "write".into(),
        args: serde_json::json!({ "file_path": "/tmp/a", "content": "hello" }),
    });
    assert_eq!(
        app.state().transcript.active_tools().len(),
        1,
        "the execution start must ADOPT the streamed row, not add another"
    );
    let (args, complete) = live_call(&app).expect("the adopted row");
    assert_eq!(
        args,
        serde_json::json!({ "file_path": "/tmp/a", "content": "hello" }),
        "the authoritative arguments replace the streamed snapshot"
    );
    assert!(complete, "`ToolExecutionStart` carries complete arguments");
}

/// A call the TUI first hears about at `ToolExecutionStart` — a `/resume` replay, an embedder, a
/// provider that emits no `toolcall*` frames — still draws, and is born complete.
#[test]
fn a_call_with_no_streaming_frames_still_draws_and_is_born_complete() {
    let mut app = app();
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call_1"),
        tool_name: "write".into(),
        args: serde_json::json!({ "file_path": "/tmp/a" }),
    });
    let (args, complete) = live_call(&app).expect("the row exists");
    assert_eq!(args, serde_json::json!({ "file_path": "/tmp/a" }));
    assert!(complete, "nothing was streaming, so nothing is incomplete");
}
