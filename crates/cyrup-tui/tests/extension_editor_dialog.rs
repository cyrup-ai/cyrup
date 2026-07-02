//! Assembled-app render proof for the L4 review §3 fix: `ui.editor` must open an INLINE dialog by
//! default (Pi's `ExtensionEditorComponent`, `interactive/components/extension-editor.ts`), not tear
//! the terminal down for `$VISUAL`/`$EDITOR`. Mirrors `extension_dialog_wrapping.rs`/
//! `extension_dialog_countdown.rs`'s harness: a real ratatui render into a fixed-size `TestBackend`
//! buffer, not a unit test on the selector's internals alone (per this crate's own live-render
//! discipline — `App::draw` reveals assembled-layout bugs a bare `Selector::render` unit test can't).
//!
//! Critically, none of these tests can hang: if `open_extension_dialog` still routed `UiKind::Editor`
//! through the old teardown-to-`$EDITOR` path, `app.active_selector_kind()` would be `None`
//! (`open_extension_dialog` was never reached) and the assertions below would fail immediately rather
//! than the test blocking on a real child process — so a red run here is unambiguous, not a hang.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, InputEvent, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn buf_text(app: &App<TestBackend>) -> String {
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

fn editor_request(
    reply: tokio::sync::oneshot::Sender<UiReply>,
    title: &str,
    initial: &str,
) -> UiRequest {
    UiRequest {
        kind: UiKind::Editor,
        prompt: title.to_string(),
        options: serde_json::Value::Null,
        message: initial.to_string(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: None, signal_id: None },
        reply,
    }
}

/// The dialog opens INLINE — `SelectorKind::ExtensionEditor` occupies the slot, and the rendered
/// frame shows BOTH the guest's real title (L4 review §2's fix — previously always `""`) and the
/// seed text, right there in the live terminal buffer. `$EDITOR` is never spawned to get here.
#[test]
fn ui_editor_opens_inline_showing_the_real_title_and_seed_text() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(editor_request(tx, "edit the changelog", "## seed content"));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionEditor));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("edit the changelog"), "the real title must render inline:\n{text}");
    assert!(text.contains("## seed content"), "the seed text must render inline:\n{text}");
}

/// Typing extra text then `Enter` resolves the guest's suspended call with the FULL edited buffer
/// (seed + typed), not the seed alone and not a default — proving the inline editor is genuinely
/// live-editable, not a static readout.
#[test]
fn ui_editor_enter_confirms_with_the_live_edited_text() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(editor_request(tx, "title", "seed"));
    app.draw().unwrap();
    for c in " more".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None, "the dialog closes on Enter");
    let reply = rx.try_recv().expect("a reply was sent");
    assert_eq!(reply, UiReply::Text(Some("seed more".to_string())));
}

/// `Esc` cancels to the per-kind deny default (`None`) — Pi's `Esc`-cancelled dialogs never resolve
/// with a value (`interactive-mode.ts:2172-2179`'s pattern, same as confirm/input/select).
#[test]
fn ui_editor_esc_cancels_to_none() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(editor_request(tx, "title", "seed"));
    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(app.active_selector_kind(), None, "the dialog closes on Esc");
    let reply = rx.try_recv().expect("a reply was sent");
    assert_eq!(reply, UiReply::Text(None));
}

/// `Ctrl+G` (`app.editor.external`) is a REQUEST, never an immediate resolution: the run loop must
/// see [`AppAction::OpenExternalEditorForSelector`] and the dialog must STILL be open afterward — Pi
/// never calls `onSubmitCallback`/`onCancelCallback` from `openExternalEditor`
/// (`extension-editor.ts:119-157`), it only ever mutates the SAME inline buffer. This is the seam
/// that makes `$VISUAL`/`$EDITOR` an escape hatch rather than the default: a guest's dialog survives
/// the keypress instead of being torn down/resolved by it.
#[test]
fn ui_editor_ctrl_g_requests_the_external_editor_without_closing_the_dialog() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(editor_request(tx, "title", "seed"));
    let action = app.handle_input(&InputEvent::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)));
    assert_eq!(action, cyrup_tui::AppAction::OpenExternalEditorForSelector);
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ExtensionEditor),
        "the dialog must still be open — Ctrl+G never resolves it directly"
    );
    assert!(rx.try_recv().is_err(), "no reply was sent yet — the guest is still suspended");
}
