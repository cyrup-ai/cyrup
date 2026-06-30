//! Scoped-models checkbox+reorder selector tests (spec/tui/05 §6; `scoped-models-selector.ts`).
//!
//! Drives the bespoke `/scoped-models` selector through `App::handle_input`: the `✓`/`✗` checkbox
//! render over the full catalog, `Enter` **toggling** membership (not confirming), Alt+↑/↓ reorder of
//! the enabled cycle order, Ctrl+A/Ctrl+X enable/clear-all, and Ctrl+S confirming with the ordered
//! enabled set (or the `SCOPED_MODELS_ALL` sentinel).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, AppAction, AppCommand, InputEvent, SelectorKind, UiTheme, SCOPED_MODELS_ALL};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ctrl(c: char) -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}
fn alt(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::ALT))
}

/// A 3-model catalog: ids `m0`,`m1`,`m2` across two providers.
fn catalog() -> Vec<(String, String, String, Option<String>)> {
    vec![
        ("m0".into(), "Model Zero".into(), "openai".into(), Some("openai".into())),
        ("m1".into(), "Model One".into(), "openai".into(), Some("openai".into())),
        ("m2".into(), "Model Two".into(), "anthropic".into(), Some("anthropic".into())),
    ]
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

/// Pull the confirm value out of a `ConfirmSelection { ScopedModels, .. }` action.
fn confirm_value(action: AppAction) -> String {
    match action {
        AppAction::Command(AppCommand::ConfirmSelection { kind: SelectorKind::ScopedModels, value }) => {
            value
        }
        other => panic!("expected ScopedModels confirm, got {other:?}"),
    }
}

#[test]
fn renders_checkboxes_title_and_footer_hint() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    // Explicit scope: only m0 enabled → markers must show (not the all-enabled blank form).
    app.open_checkbox_selector(catalog(), Some(vec!["m0".into()]));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ScopedModels));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Scoped Models"), "title missing:\n{text}");
    assert!(text.contains("✓ Model Zero"), "enabled checkbox missing:\n{text}");
    assert!(text.contains("✗ Model One"), "disabled checkbox missing:\n{text}");
    assert!(text.contains("toggle"), "footer hint missing:\n{text}");
}

#[test]
fn enter_toggles_membership_and_ctrl_s_confirms() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec![])); // start with nothing enabled
    // Highlight is on row0 (m0). Enter toggles it ON (does NOT confirm/close).
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::ScopedModels), "Enter must not close");
    // Move down to m1, enable it too.
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Enter));
    // Ctrl+S confirms with the ordered enabled set "m0\nm1".
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "m0\nm1");
    assert_eq!(app.active_selector_kind(), None, "Ctrl+S closes the selector");
}

#[test]
fn alt_down_reorders_enabled_cycle_order() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec!["m0".into(), "m1".into()]));
    // Highlight m0 (row0); Alt+Down moves it down in cycle order → [m1, m0].
    app.handle_input(&alt(KeyCode::Down));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "m1\nm0");
}

#[test]
fn ctrl_a_enables_all_sentinel_and_ctrl_x_clears() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    app.open_checkbox_selector(catalog(), Some(vec!["m0".into()]));
    app.handle_input(&ctrl('a'));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), SCOPED_MODELS_ALL, "Ctrl+A → all-enabled sentinel");

    // Reopen and clear all → empty confirm value.
    app.open_checkbox_selector(catalog(), None);
    app.handle_input(&ctrl('x'));
    let action = app.handle_input(&ctrl('s'));
    assert_eq!(confirm_value(action), "", "Ctrl+X → empty scoped set");
}

#[test]
fn ctrl_p_toggles_whole_provider() {
    let mut app = App::new(TestBackend::new(70, 20), UiTheme::dark()).unwrap();
    // Start empty; highlight m0 (openai). Ctrl+P enables the whole openai provider (m0, m1).
    app.open_checkbox_selector(catalog(), Some(vec![]));
    app.handle_input(&ctrl('p'));
    let action = app.handle_input(&ctrl('s'));
    let value = confirm_value(action);
    assert!(value.contains("m0") && value.contains("m1"), "provider enable failed: {value:?}");
    assert!(!value.contains("m2"), "anthropic must stay disabled: {value:?}");
}
