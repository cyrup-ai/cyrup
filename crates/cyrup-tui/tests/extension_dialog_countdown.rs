//! Extension-UI dialog countdown/auto-dismiss (closes an L4 finding: TUI extension dialogs showed
//! no countdown for `opts.timeout_ms` and never auto-dismissed once the host's OWN independent
//! `LiveHostServices::ui_roundtrip` timeout had already resolved the guest's call — leaving the
//! dialog rendered and visibly answerable long after the answer no longer mattered). Mirrors Pi's
//! `CountdownTimer` (`countdown-timer.ts:7-38`, wired by `ExtensionSelectorComponent`/
//! `ExtensionInputComponent`): the title shows `(Ns)` from the instant the dialog opens, ticks down
//! once per second, and auto-resolves to the per-kind deny default (closing the widget) on expiry.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_ext::host::DialogOptions;
use cyrup_session_svc::{UiKind, UiReply, UiRequest};
use cyrup_tui::{App, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;
use std::time::Duration;

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

fn confirm_request(reply: tokio::sync::oneshot::Sender<UiReply>, timeout_ms: Option<u64>) -> UiRequest {
    UiRequest {
        kind: UiKind::Confirm,
        prompt: "Proceed?".to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions { timeout_ms, signal_id: None },
        reply,
    }
}

/// Opening a dialog WITH a timeout shows the countdown suffix in the title from the very first
/// frame — Pi calls `onTick` synchronously in `CountdownTimer`'s constructor
/// (`countdown-timer.ts:19`), so the dialog never renders even one frame without it.
#[test]
fn extension_dialog_with_timeout_shows_the_countdown_immediately() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(confirm_request(tx, Some(5_000)));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionConfirm));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Proceed? (5s)"), "missing initial countdown in title:\n{text}");
}

/// A dialog with NO timeout renders its plain title, no countdown suffix — matching Pi's own
/// `if (opts?.timeout && opts.timeout > 0 && opts.tui)` guard (the countdown is never armed for a
/// guest call that set no `opts.timeoutMs`).
#[test]
fn extension_dialog_without_timeout_shows_no_countdown() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(confirm_request(tx, None));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Proceed?"), "missing plain title:\n{text}");
    assert!(!text.contains("Proceed? ("), "must not show a countdown with no timeout set:\n{text}");

    // A tick with no deadline armed is a documented no-op: the dialog stays open indefinitely.
    app.tick_extension_dialog_countdown();
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionConfirm));
}

/// Ticking the countdown after real time passes (but before the deadline) live-updates the title
/// with the recomputed remaining seconds — Pi's `titleText.setText` on every `onTick`
/// (`extension-selector.ts:55`).
#[test]
fn extension_dialog_countdown_ticks_down_with_real_elapsed_time() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(confirm_request(tx, Some(3_000)));

    std::thread::sleep(Duration::from_millis(1_100));
    app.tick_extension_dialog_countdown();
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Proceed? (2s)"), "expected the countdown to have ticked to 2s:\n{text}");
    // The dialog is still open — 1.1s elapsed of a 3s budget.
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionConfirm));
}

/// THE headline fix: once the deadline passes, ticking the countdown auto-resolves the dialog to
/// its per-kind deny default (`Confirm` → `false`, Pi's `noOpUIContext`) and CLOSES the selector —
/// Pi's `onExpire` → `onCancelCallback` (`extension-selector.ts:56`) — rather than leaving a stale
/// dialog rendered after the guest's call has already resolved host-side
/// (`LiveHostServices::ui_roundtrip`'s own independent timeout race).
#[tokio::test]
async fn extension_dialog_auto_dismisses_and_replies_the_deny_default_on_expiry() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.open_extension_dialog(confirm_request(tx, Some(50)));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionConfirm));

    std::thread::sleep(Duration::from_millis(120));
    app.tick_extension_dialog_countdown();

    assert_eq!(app.active_selector_kind(), None, "the stale dialog must auto-close on expiry");
    let reply = tokio::time::timeout(Duration::from_secs(1), rx)
        .await
        .expect("the reply resolves promptly, not left hanging")
        .expect("the reply channel is fulfilled, not dropped");
    assert_eq!(reply, UiReply::Confirm(false), "Confirm's deny default is `false`");
}

/// The `Select`/`Input` kinds' deny default is `UiReply::Text(None)`, not `Confirm`'s — the auto-
/// dismiss must resolve per-kind, exactly like an `Esc` cancel does.
#[tokio::test]
async fn extension_input_dialog_auto_dismisses_with_its_own_text_none_default() {
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let req = UiRequest {
        kind: UiKind::Input,
        prompt: "Name?".to_string(),
        options: serde_json::Value::Null,
        message: String::new(),
        placeholder: None,
        opts: DialogOptions { timeout_ms: Some(50), signal_id: None },
        reply: tx,
    };
    app.open_extension_dialog(req);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ExtensionInput));

    std::thread::sleep(Duration::from_millis(120));
    app.tick_extension_dialog_countdown();

    assert_eq!(app.active_selector_kind(), None);
    let reply = tokio::time::timeout(Duration::from_secs(1), rx).await.unwrap().unwrap();
    assert_eq!(reply, UiReply::Text(None));
}
