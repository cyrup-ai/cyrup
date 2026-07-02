//! Closes an L4 review finding (§4, LOW): a `ui.select` request carrying an empty `options` array
//! used to short-circuit straight to `UiReply::Text(None)` in `open_extension_dialog` BEFORE the
//! dialog ever opened — diverging from both Pi's `ExtensionSelectorComponent`
//! (`extension-selector.ts:101-103`, which renders whatever it's given including `[]`, with Enter a
//! no-op and resolution only via Esc/timeout/signal) and cyrup's own RPC path (`rpc.rs`, which
//! forwards `options: []` verbatim with no such short-circuit). Real ratatui `TestBackend` render +
//! real key routing, matching this crate's live-render discipline (a bare `Selector`/`SelectList`
//! unit test wouldn't catch the app-level short-circuit at all — it never reaches either).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, InputEvent, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn select_request(reply: tokio::sync::oneshot::Sender<UiReply>, options: serde_json::Value) -> UiRequest {
    UiRequest {
        kind: UiKind::Select,
        prompt: "Pick one".to_string(),
        options,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: None, signal_id: None },
        reply,
    }
}

/// An empty `options: []` request must still open the dialog (`SelectorKind::ExtensionSelect`
/// occupies the slot) rather than resolving `None` before the guest's call is ever shown.
#[test]
fn ui_select_with_empty_options_still_opens_the_dialog() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(select_request(tx, serde_json::json!([])));
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ExtensionSelect),
        "an empty options list must still open the selector, not short-circuit before it opens"
    );
    assert!(rx.try_recv().is_err(), "no reply sent yet — the guest is still suspended");
    // A real frame renders without panicking (`SelectList`'s empty-state path).
    app.draw().unwrap();
}

/// The empty dialog resolves the SAME way as a non-empty one dismissed without a pick: Esc → the
/// per-kind deny default (`None`) via the shared cancel path, not a special early-return value.
#[test]
fn ui_select_with_empty_options_esc_cancels_to_none() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(select_request(tx, serde_json::json!([])));
    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(app.active_selector_kind(), None, "the dialog closes on Esc");
    let reply = rx.try_recv().expect("a reply was sent");
    assert_eq!(reply, UiReply::Text(None));
}

/// A non-empty options list is unaffected by the fix — still opens and still confirms a real pick
/// (regression guard against a naive fix that broke the populated case).
#[test]
fn ui_select_with_real_options_still_confirms_the_picked_value() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(select_request(tx, serde_json::json!(["alpha", "beta", "gamma"])));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionSelect));
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None);
    let reply = rx.try_recv().expect("a reply was sent");
    assert_eq!(reply, UiReply::Text(Some("gamma".to_string())));
}
