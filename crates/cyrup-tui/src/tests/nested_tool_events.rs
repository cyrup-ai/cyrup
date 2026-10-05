//! CODE-006 — the interactive renderer ignores the `tool_execution_*` events of calls a tool made.
//!
//! Pi's `case "tool_execution_start"` opens with
//! `// Nested calls (from codemode scripts) are shown inside their parent's row.` and
//! `if (event.parentToolCallId) break;` (`modes/interactive/interactive-mode.ts:3558-3559`
//! @v1.0.1): no row is filed under a nested call's id, and the matching update and end find none
//! to update. A fold that drew a nested call as a top-level tool row would show every script call
//! twice — once on its own and once inside its parent's `nestedCalls` record.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{App, UiTheme};
use cyrup_core::ToolCallId;
use cyrup_session_svc::AgentSessionEvent;
use ratatui::backend::TestBackend;
use serde_json::json;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

/// A model-issued `ls` with a nested `read` made by it while it ran, in the order a session emits
/// them: the parent starts, the nested call starts / updates / ends, the parent ends.
fn events(with_nested: bool) -> Vec<AgentSessionEvent> {
    let mut evs = vec![AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-ls"),
        tool_name: "ls".to_string(),
        args: json!({ "path": "." }),
    }];
    if with_nested {
        evs.extend([
            AgentSessionEvent::NestedToolExecutionStart {
                tool_call_id: ToolCallId::from("call-ls/1"),
                tool_name: "read".to_string(),
                args: json!({ "path": "NESTED_ONLY_FILE.md" }),
                parent_tool_call_id: ToolCallId::from("call-ls"),
            },
            AgentSessionEvent::NestedToolExecutionUpdate {
                tool_call_id: ToolCallId::from("call-ls/1"),
                tool_name: "read".to_string(),
                args: json!({ "path": "NESTED_ONLY_FILE.md" }),
                partial_result: json!({ "content": [{ "type": "text", "text": "partial" }] }),
                parent_tool_call_id: ToolCallId::from("call-ls"),
            },
            AgentSessionEvent::NestedToolExecutionEnd {
                tool_call_id: ToolCallId::from("call-ls/1"),
                tool_name: "read".to_string(),
                result: json!({ "content": [{ "type": "text", "text": "NESTED_RESULT_TEXT" }] }),
                is_error: false,
                parent_tool_call_id: ToolCallId::from("call-ls"),
            },
        ]);
    }
    evs.push(AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-ls"),
        tool_name: "ls".to_string(),
        result: json!({ "content": [{ "type": "text", "text": "src\nREADME.md" }] }),
        is_error: false,
    });
    evs
}

fn transcript_after(evs: &[AgentSessionEvent]) -> String {
    let mut app = app();
    for ev in evs {
        app.ingest_event(ev);
    }
    app.draw().unwrap();
    app.scrollback_text()
}

/// The nested call draws no row of its own: the transcript is exactly what the parent alone makes.
#[test]
fn nested_tool_execution_events_draw_no_row() {
    let with = transcript_after(&events(true));
    let without = transcript_after(&events(false));
    assert!(
        with.contains("README.md"),
        "the parent's own row is lost:\n{with}"
    );
    assert!(
        !with.contains("NESTED_ONLY_FILE.md") && !with.contains("NESTED_RESULT_TEXT"),
        "a nested call was drawn as a tool row:\n{with}"
    );
    assert_eq!(with, without, "nested events changed what is on screen");
}

/// The nested events are ignored by the fold, not merely invisible: an end for a nested id does not
/// close or disturb the parent's still-running row.
#[test]
fn a_nested_end_does_not_close_the_parents_running_row() {
    let mut app = app();
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from("call-ls"),
        tool_name: "ls".to_string(),
        args: json!({ "path": "." }),
    });
    app.ingest_event(&AgentSessionEvent::NestedToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-ls/1"),
        tool_name: "ls".to_string(),
        result: json!({ "content": [{ "type": "text", "text": "NESTED_RESULT_TEXT" }] }),
        is_error: true,
        parent_tool_call_id: ToolCallId::from("call-ls"),
    });
    app.draw().unwrap();
    let mid = app.scrollback_text();
    assert!(!mid.contains("NESTED_RESULT_TEXT"), "{mid}");
    // The parent can still finish normally afterwards.
    app.ingest_event(&AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from("call-ls"),
        tool_name: "ls".to_string(),
        result: json!({ "content": [{ "type": "text", "text": "PARENT_RESULT" }] }),
        is_error: false,
    });
    app.draw().unwrap();
    assert!(app.scrollback_text().contains("PARENT_RESULT"));
}
