//! Rich message-component variants (`{skill-invocation,custom,branch-summary,compaction-summary}-`
//! `message.ts`): the labeled, markdown-bodied transcript blocks Pi renders for skill invocations,
//! extension custom messages, and branch/compaction summaries. Committed entries reach native
//! scrollback in their expanded (full-record) form, asserted via `App::scrollback_text`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::AgentMessage;
use cyrup_session_svc::AgentSessionEvent;
use crate::{App, UiTheme};
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

#[test]
fn skill_invocation_renders_label_name_and_content() {
    let mut app = new_app();
    app.transcript_mut().push_skill_invocation("commit-helper", "Run the **commit** flow.");
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("[skill]"), "skill label committed:\n{out}");
    assert!(out.contains("commit-helper"), "skill name committed:\n{out}");
    assert!(out.contains("commit"), "skill content body committed:\n{out}");
}

/// X14 — the full body is the EXPANDED form. `BranchSummaryMessageComponent` is constructed with
/// `expanded = false` (`branch-summary-message.ts:11`) and `interactive-mode.ts:3493` then calls
/// `component.setExpanded(this.toolOutputExpanded)`, so the body is on screen only once `Ctrl+O`
/// has been pressed. Every assertion below is the ORIGINAL one; the only change is that the test
/// now puts the transcript in the state that actually produces that render.
#[test]
fn branch_summary_renders_with_header() {
    let mut app = new_app();
    app.transcript_mut().set_tool_expanded(true);
    app.transcript_mut().push_branch_summary("Explored an alternative refactor, then reverted.");
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("[branch]"), "branch label committed:\n{out}");
    assert!(out.contains("Branch Summary"), "branch header committed:\n{out}");
    assert!(out.contains("alternative refactor"), "branch body committed:\n{out}");
}

/// X14 — and COLLAPSED (the default) it is `branch-summary-message.ts:46-56`'s single row:
/// `"Branch summary (" + keyText("app.tools.expand") + " to expand)"`, with the body withheld.
#[test]
fn branch_summary_collapses_to_one_expand_hint_row() {
    let mut app = new_app();
    app.transcript_mut().push_branch_summary("Explored an alternative refactor, then reverted.");
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("[branch]"), "the label still shows:\n{out}");
    assert!(out.contains("Branch summary (ctrl+o to expand)"), "collapsed hint row:\n{out}");
    assert!(!out.contains("alternative refactor"), "the body is withheld:\n{out}");
    assert!(!out.contains("Branch Summary"), "and so is the `**Branch Summary**` header:\n{out}");
}

#[test]
fn compaction_summary_groups_the_token_count() {
    let mut app = new_app();
    app.transcript_mut().set_tool_expanded(true);
    app.transcript_mut().push_compaction_summary(123_456, "Condensed the earlier turns.");
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("[compaction]"), "compaction label committed:\n{out}");
    // Pi formats the pre-compaction token count with thousands separators.
    assert!(out.contains("123,456"), "token count grouped:\n{out}");
    assert!(out.contains("Condensed"), "compaction body committed:\n{out}");
}

/// X14 — `compaction-summary-message.ts:48-56`: the collapsed row keeps the grouped token count
/// (`Compacted from 123,456 tokens (`) and drops the summary.
#[test]
fn compaction_summary_collapses_to_one_expand_hint_row() {
    let mut app = new_app();
    app.transcript_mut().push_compaction_summary(123_456, "Condensed the earlier turns.");
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("[compaction]"), "the label still shows:\n{out}");
    assert!(
        out.contains("Compacted from 123,456 tokens (ctrl+o to expand)"),
        "collapsed hint row keeps the grouped count:\n{out}"
    );
    assert!(!out.contains("Condensed"), "the body is withheld:\n{out}");
}

#[test]
fn custom_message_event_renders_a_labeled_block() {
    // A finished extension `Custom` message folds into a labeled transcript block via `ingest_event`
    // (the serde-projection decode, since `AgentMessage` is only a dev-dep of the crate).
    let mut app = new_app();
    let message = AgentMessage::Custom {
        kind: "review".to_string(),
        payload: serde_json::json!("Looks good to ship."),
        details: None,
        timestamp: None,
    };
    app.ingest_event(&AgentSessionEvent::MessageEnd { message });
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("[review]"), "custom-type label committed:\n{out}");
    assert!(out.contains("Looks good to ship"), "custom body committed:\n{out}");
}

#[test]
fn custom_message_event_handles_array_content() {
    let mut app = new_app();
    let message = AgentMessage::Custom {
        kind: "note".to_string(),
        payload: serde_json::json!([{ "type": "text", "text": "part one " }, { "type": "text", "text": "part two" }]),
        details: None,
        timestamp: None,
    };
    app.ingest_event(&AgentSessionEvent::MessageEnd { message });
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(out.contains("part one part two"), "array text parts joined:\n{out}");
}

#[test]
fn user_message_renders_as_multiline_markdown_box() {
    // The rich user-message variant (`user-message.ts`): the submitted text renders as multi-line
    // markdown. X1: no `you: ` label — the block is identified by its `userMessageBg` fill (`:40`).
    let mut app = new_app();
    app.transcript_mut().push_user("# Heading\n\nFirst paragraph.\n\nSecond paragraph.");
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(!out.contains("you:"), "invented `you: ` label committed:\n{out}");
    assert!(out.contains("Heading"), "markdown heading rendered:\n{out}");
    // Multi-paragraph markdown means more than one rendered line (vs. the old single label line).
    assert!(out.contains("First paragraph"), "first paragraph rendered:\n{out}");
    assert!(out.contains("Second paragraph"), "second paragraph rendered:\n{out}");
    let user_lines = out.lines().filter(|l| l.contains("paragraph")).count();
    assert!(user_lines >= 2, "user body spans multiple markdown lines:\n{out}");
}

#[test]
fn core_messages_do_not_render_as_custom_blocks() {
    // A core user/assistant message must NOT be mistaken for a custom block.
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: AgentMessage::user_text("just a normal user message"),
    });
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(!out.contains("[user]"), "core user message is not labeled custom:\n{out}");
}
