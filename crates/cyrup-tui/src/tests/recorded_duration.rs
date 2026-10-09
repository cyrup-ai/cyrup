//! A REPLAYED bash result shows its recorded duration and nothing measured now (pi commit
//! 36a686ee8, #10549). pi's replay never marks a call's execution as started
//! (`markExecutionStarted` is only on the live `tool_execution_start`, `interactive-mode.ts:3628`
//! @v1.1.0) and hands the result over with `updateResult(message)` (`:4070`), so the renderer sees
//! the stored `durationMs` and no `startedAt`: `Took 4.2s` with one, no footer without one.
//! cyrup used to stamp the replayed call's start NOW and show `Took 0.0s` for every reloaded
//! command.

#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use crate::{App, UiTheme};
use cyrup_core::{
    ApiId, AssistantMessage, Content, Message, ProviderId, StopReason, ToolCall, ToolCallId,
};
use cyrup_session_svc::agent_message::AgentMessage;
use ratatui::backend::TestBackend;

fn replayed(duration_ms: Option<u64>) -> String {
    let mut app = App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap();
    let mut call = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        StopReason::ToolUse,
        String::new(),
    );
    call.error_message = None;
    call.content = vec![Content::ToolCall(ToolCall {
        id: ToolCallId::from("call_1"),
        name: "bash".to_string(),
        arguments: serde_json::json!({ "command": "sleep 4" })
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into(),
        thought_signature: None,
        namespace: None,
    })];
    app.replay_session(&[
        AgentMessage::Core(Message::Assistant(call)),
        AgentMessage::Core(Message::ToolResult {
            tool_call_id: ToolCallId::from("call_1"),
            tool_name: "bash".to_string(),
            content: vec![Content::text("slept")],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            duration_ms,
            timestamp: 0,
            nested_calls: None,
        }),
    ]);
    app.draw().unwrap();
    app.scrollback_text()
}

#[test]
fn a_replayed_result_shows_its_recorded_duration() {
    let out = replayed(Some(4_200));
    assert!(out.contains("Took 4.2s"), "{out}");
}

#[test]
fn a_replayed_result_without_one_shows_no_duration_at_all() {
    let out = replayed(None);
    assert!(out.contains("slept"), "{out}");
    assert!(
        !out.contains("Took "),
        "nothing measured the replayed call: {out}"
    );
    assert!(!out.contains("Elapsed "), "{out}");
}

/// An extension's tool-result renderer is handed `durationMs` (pi `ToolRenderContext.durationMs`,
/// `core/extensions/types.ts` @v1.1.0) — and because the host re-invokes a renderer whenever the
/// options its text was drawn under no longer match the row's, the duration must be part of that
/// comparison: a render drawn WITH it is current, and one drawn without it is stale.
#[test]
fn a_rows_recorded_duration_is_part_of_its_extension_render_options() {
    use crate::transcript::{RenderSource, RenderSurface, RenderedText};
    use serde_json::json;

    let live = cyrup_ext::RenderOptions::new(false, 1, None);
    let drawn_under = |under: cyrup_ext::RenderOptions| {
        let mut t = crate::TranscriptView::new();
        t.set_retain_document(true);
        t.push_tool_start_rendered("ext", Some("c1".into()), json!({}), None);
        t.push_tool_end_rendered(
            "ext",
            Some("c1"),
            false,
            Some(json!({ "content": [] })),
            Some(RenderedText::new(
                "snapshot",
                RenderSource {
                    surface: RenderSurface::ToolResult,
                    key: "ext".into(),
                    payload: json!({}),
                    under,
                },
            )),
            Some(4_200),
        );
        t.commit_tools();
        t.drain_committed();
        t.stale_extension_renders(&live)
    };

    assert!(
        drawn_under(live.clone().partial(false).recorded(Some(4_200))).is_empty(),
        "drawn with the row's duration: current, not re-rendered on every pass"
    );
    let stale = drawn_under(live.clone().partial(false));
    assert_eq!(stale.len(), 1, "drawn without it: stale");
    assert_eq!(stale[0].next.under.duration_ms, Some(4_200));
}
