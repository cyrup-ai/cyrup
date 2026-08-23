//! TUI-005 — Esc during a run must give the queued messages BACK, not drop them.
//!
//! Pi's `defaultEditor.onEscape` (interactive-mode.ts:2635-2660) branches on `session.isStreaming`
//! FIRST and calls `restoreQueuedMessagesToEditor({abort: true})` (`:2636-2637`), which
//! (`:4064-4083`) take-alls both queues, joins `[...steering, ...followUp]` with a blank line,
//! prepends that to whatever is already typed — dropping empty parts — sets the editor text and
//! only THEN calls `agent.abort()`. Escaping a turn therefore hands the user's in-flight steering /
//! follow-up text back for editing.
//!
//! cyrup previously routed every Esc to a bare `AppAction::Interrupt`, whose run-loop arm was
//! `session.abort()` + kill the bash child — the queues were never read, so every message the user
//! typed while the turn ran was silently discarded.
//!
//! These tests drive the real `App::handle_input` Esc path and then the real
//! `App::restore_queued_to_editor` (the pure half the run loop calls with what
//! `AgentSession::drain_queue` returned), asserting the queued text actually lands in the RENDERED
//! editor row — not merely that a function was called.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{App, AppAction, UiTheme};
use ratatui::backend::TestBackend;
use super::harness::*;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 12), UiTheme::dark()).unwrap()
}

/// The whole rendered buffer as text.
fn screen(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// Mark a turn in flight the way `AgentSessionEvent::AgentStart` does.
fn start_streaming(app: &mut App<TestBackend>) {
    app.state_mut().status.set_streaming(true);
}

#[test]
fn escape_mid_turn_asks_the_run_loop_to_restore_the_queue() {
    let mut app = new_app();
    start_streaming(&mut app);
    // Pi: `if (this.session.isStreaming) restoreQueuedMessagesToEditor({abort: true})`.
    assert_eq!(
        app.handle_input(&esc()),
        AppAction::InterruptRestoreQueued,
        "Esc during a streaming turn must restore the queue, not just abort"
    );
}

/// **Corrected for TUI-005.** This asserted `AppAction::Interrupt` for an idle Escape on an empty
/// buffer, which is cyrup-original: pi's `onEscape` is four MUTUALLY EXCLUSIVE branches
/// (`interactive-mode.ts:2569-2595` @v0.83.0), and an idle Escape on an empty buffer falls to the
/// fourth — the 500 ms double-Escape window — which aborts nothing at all. cyrup returned a bare
/// `Interrupt` from every non-streaming Escape, so it called `session.abort()` + `abort_bash()`
/// against a session with nothing to abort, and the double-Escape branch could never be reached.
///
/// The property this test exists for — "not streaming ⇒ pi never reaches the restore branch" — is
/// unchanged and still asserted.
#[test]
fn escape_while_idle_does_not_restore_a_queue() {
    let mut app = new_app();
    let out = app.handle_input(&esc());
    assert_ne!(
        out,
        AppAction::InterruptRestoreQueued,
        "not streaming ⇒ pi never reaches the restore branch"
    );
    assert_eq!(out, AppAction::Redraw, "pi's empty-editor arm only arms the double-Escape window");
}

#[test]
fn restored_queue_lands_in_the_rendered_editor() {
    let mut app = new_app();
    start_streaming(&mut app);
    // What the user was mid-way through typing when they hit Esc.
    app.editor_mut().set_text("and also check the tests");
    let action = app.handle_input(&esc());
    assert_eq!(action, AppAction::InterruptRestoreQueued);

    // What the run loop drains from the session: steering first, then follow-up (Pi's
    // `[...steering, ...followUp]`, interactive-mode.ts:4066).
    let queued = vec!["use the async api".to_string(), "then run clippy".to_string()];
    assert_eq!(app.restore_queued_to_editor(&queued), 2);

    app.draw().unwrap();
    let out = screen(&app);
    for needle in ["use the async api", "then run clippy", "and also check the tests"] {
        assert!(out.contains(needle), "`{needle}` must be back in the editor:\n{out}");
    }
    // Queued text is PREPENDED, ahead of the partially-typed line (Pi joins
    // `[queuedText, currentText]`, `:4076`).
    let (q, typed) = (
        out.find("use the async api").unwrap(),
        out.find("and also check the tests").unwrap(),
    );
    assert!(q < typed, "queued text is prepended, not appended:\n{out}");
}

#[test]
fn an_empty_queue_leaves_the_typed_text_untouched() {
    let mut app = new_app();
    app.editor_mut().set_text("half-written prompt");
    // Pi returns 0 without touching the editor when both queues are empty (`:4067-4073`), which is
    // what makes `handleDequeue` print `No queued messages to restore`.
    assert_eq!(app.restore_queued_to_editor(&[]), 0);
    app.draw().unwrap();
    let out = screen(&app);
    assert!(out.contains("half-written prompt"), "typed text survives:\n{out}");
}

#[test]
fn restoring_into_an_empty_editor_adds_no_blank_padding() {
    let mut app = new_app();
    // `[queuedText, currentText].filter((t) => t.trim())` (`:4076`) drops the empty current text, so
    // the restored buffer is exactly the queued text — no trailing blank line.
    assert_eq!(app.restore_queued_to_editor(&["only queued".to_string()]), 1);
    assert_eq!(app.editor_mut().text(), "only queued");
}
