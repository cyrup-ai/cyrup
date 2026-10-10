//! TUI-171 — the `blocked` half of the OSC 7501 report, at the App level.
//!
//! The byte layer and the reporter's state machine are pinned in their own modules
//! (`crate::program_status`, `crate::program_status_reporter`). What those cannot reach is the part
//! Decision 3 of this row's survey warns about: pi's `blocked` reports come from five hand-placed
//! `setBlocked` calls, not from an event, so a MISSED clear strands a `blocked` status for the rest
//! of the session — the terminal keeps saying cyrup is waiting on a dialog that closed minutes ago.
//!
//! cyrup reconciles both sources from the UI state every frame instead
//! ([`crate::App::sync_blocked_dialog_status`]), and these tests drive real dialogs through their
//! real open/close paths to prove the reconciliation reaches the reporter.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::program_status::{BlockedKind, ProgramState, ProgramStatus};
use crate::{App, UiTheme};
use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use ratatui::backend::TestBackend;

fn request(
    kind: UiKind,
    prompt: &str,
    message: &str,
) -> (UiRequest, tokio::sync::oneshot::Receiver<UiReply>) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    (
        UiRequest {
            kind,
            prompt: prompt.to_string(),
            options: serde_json::json!(["Yes", "No"]),
            message: message.to_string(),
            placeholder: None,
            opts: DialogOptions {
                timeout_ms: None,
                signal_id: None,
            },
            reply: tx,
        },
        rx,
    )
}

/// Reconcile and drain, which is what `draw_synchronized` does on every frame
/// (`app/crossterm.rs`: `sync_blocked_dialog_status` then `flush_program_status`).
fn reconcile(app: &mut App<TestBackend>) -> Option<ProgramStatus> {
    app.sync_blocked_dialog_status();
    app.state_mut().program_status.take_pending()
}

/// An extension `ui.confirm` reports `blocked`/`permission` with the dialog's BARE TITLE, and the
/// status is cleared when the dialog closes.
///
/// The bare title is upstream's, and it is not the string the dialog shows: `showExtensionConfirm`
/// hands `` `${title}\n${message}` `` to the selector for display but passes
/// `{kind: "permission", message: title}` to `setBlocked` (`interactive-mode.ts:2750-2760`), where
/// `showExtensionSelector`'s own default is `{kind: "question", message: title}` (`:2694`).
#[test]
fn an_extension_confirm_reports_blocked_permission_with_the_bare_title() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    // Nothing open: the first reconcile reports the resting `idle`, not `blocked`.
    let first = reconcile(&mut app).expect("the first reconcile reports");
    assert_eq!(first.state, ProgramState::Idle);

    let (req, _rx) = request(UiKind::Confirm, "Allow bash?", "rm -rf /tmp/x");
    app.open_extension_dialog(req);
    let blocked = reconcile(&mut app).expect("opening a dialog reports");
    assert_eq!(blocked.state, ProgramState::Blocked);
    assert_eq!(blocked.kind, Some(BlockedKind::Permission));
    assert_eq!(
        blocked.message.as_deref(),
        Some("Allow bash?"),
        "the TITLE, never the joined body (interactive-mode.ts:2755-2758)"
    );

    // Esc closes the dialog through the shared selector-cancel path, one of the FOUR sites that
    // take `pending_ui_reply`. The reconciliation does not care which one ran.
    app.handle_input(&key(KeyCode::Esc));
    let cleared = reconcile(&mut app).expect("closing a dialog reports");
    assert_eq!(
        cleared.state,
        ProgramState::Idle,
        "a closed dialog must not strand a `blocked` status"
    );
}

/// A `ui.select` reports `question`, not `permission` — the kind split is per dialog kind
/// (`interactive-mode.ts:2694` vs `:2755-2758`).
#[test]
fn an_extension_select_reports_blocked_question() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let _ = reconcile(&mut app);
    let (req, _rx) = request(UiKind::Select, "Pick one", "");
    app.open_extension_dialog(req);
    let blocked = reconcile(&mut app).expect("a report");
    assert_eq!(blocked.kind, Some(BlockedKind::Question));
    assert_eq!(blocked.message.as_deref(), Some("Pick one"));
}

/// The dedup: a frame that changes nothing writes nothing (`program-status-reporter.ts:97-99`), so
/// reconciling every frame costs one OSC per transition and not one per frame.
#[test]
fn reconciling_an_unchanged_frame_writes_nothing() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    assert!(reconcile(&mut app).is_some(), "the first report is new");
    assert!(
        reconcile(&mut app).is_none(),
        "an unchanged frame must not re-report"
    );
    let (req, _rx) = request(UiKind::Input, "Name?", "");
    app.open_extension_dialog(req);
    assert!(reconcile(&mut app).is_some(), "the open is a transition");
    assert!(
        reconcile(&mut app).is_none(),
        "and the frames after it are not"
    );
}
