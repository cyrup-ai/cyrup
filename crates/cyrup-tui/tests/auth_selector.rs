//! `/login` + `/logout` (oauth) selector rendering + routing (spec/tui/05 §6 data-bound selectors;
//! port of `oauth-selector.ts` + `getLoginProviderOptions`/`getLogoutProviderOptions`,
//! `interactive-mode.ts:4594-4636`).
//!
//! The live sourcing (catalog providers + stored credentials + per-provider auth status) requires an
//! `Arc<AgentSession>` which is not constructible in a unit test, so the row-builder half is verified
//! through the pure `cyrup_tui::provider_rows`/`AuthState` API and the *rendering + confirm routing*
//! half is driven through the real `App::open_data_selector` + `handle_input` path with synthetic
//! rows shaped exactly as `execute_command` produces them.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
    provider_display_name, provider_rows, App, AppCommand, AuthState, InputEvent, SelectorKind,
    UiTheme,
};
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

/// The catalog-shaped rows `execute_command` hands `open_data_selector` for `/login`: provider id →
/// (id, display name, status text), sorted by display name.
fn login_rows() -> Vec<(String, String, Option<String>)> {
    provider_rows(vec![
        ("openai".to_string(), AuthState::Unconfigured),
        ("anthropic".to_string(), AuthState::Configured),
        ("xai".to_string(), AuthState::EnvConfigured),
    ])
}

#[test]
fn login_selector_renders_providers_status_and_borders() {
    let mut app = App::new(TestBackend::new(72, 18), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Login, login_rows(), 0);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    app.draw().unwrap();
    let text = buf_text(&app);

    // Full-width DynamicBorder rules top & bottom (spec/tui/05 §11).
    let rule_rows = text.lines().filter(|l| l.contains("──────────")).count();
    assert!(rule_rows >= 2, "expected top+bottom `─` rules:\n{text}");
    // The selector title (`Login`).
    assert!(text.contains("Login"), "missing selector title:\n{text}");
    // Provider display names, faithfully cased (getProviderDisplayName).
    for name in ["Anthropic", "OpenAI", "xAI"] {
        assert!(text.contains(name), "missing provider {name}:\n{text}");
    }
    // Per-provider status lines (getStatusText, oauth-selector.ts:153-158).
    assert!(text.contains("✓ configured"), "missing stored status:\n{text}");
    assert!(text.contains("configured via env"), "missing env status:\n{text}");
    assert!(text.contains("• unconfigured"), "missing unconfigured status:\n{text}");
    // Sorted by display name: Anthropic preselected at the top with the `→` cursor.
    assert!(text.contains("→ Anthropic"), "expected cursor on first row:\n{text}");
}

#[test]
fn login_confirm_routes_provider_value_to_run_loop() {
    let mut app = App::new(TestBackend::new(72, 18), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Login, login_rows(), 0);
    // Down → OpenAI (idx 1), confirm.
    app.handle_input(&key(KeyCode::Down));
    let action = app.handle_input(&key(KeyCode::Enter));
    // The selector closes and hands the chosen provider id to the run loop as a data-bound confirm
    // (the credential write is the provider-tail residual the run loop applies).
    assert_eq!(app.active_selector_kind(), None, "confirm closes the selector");
    match action {
        cyrup_tui::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
            assert_eq!(kind, SelectorKind::Login);
            assert_eq!(value, "openai", "confirm carries the provider id, not the label");
        }
        other => panic!("expected ConfirmSelection command, got {other:?}"),
    }
}

#[test]
fn logout_selector_lists_stored_providers_only() {
    // `/logout` lists ONLY providers with a stored credential, each marked configured.
    let rows = provider_rows(vec![("anthropic".to_string(), AuthState::Configured)]);
    let mut app = App::new(TestBackend::new(72, 18), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Logout, rows, 0);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Logout"), "missing logout title:\n{text}");
    assert!(text.contains("Anthropic"), "missing stored provider:\n{text}");
    assert!(text.contains("✓ configured"), "stored cred should show configured:\n{text}");
}

#[test]
fn logout_confirm_routes_for_deletion() {
    let rows = provider_rows(vec![("anthropic".to_string(), AuthState::Configured)]);
    let mut app = App::new(TestBackend::new(72, 18), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Logout, rows, 0);
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None);
    match action {
        cyrup_tui::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
            assert_eq!(kind, SelectorKind::Logout);
            assert_eq!(value, "anthropic", "logout carries the provider id to delete");
        }
        other => panic!("expected ConfirmSelection command, got {other:?}"),
    }
}

#[test]
fn provider_display_name_is_faithfully_cased() {
    assert_eq!(provider_display_name("anthropic"), "Anthropic");
    assert_eq!(provider_display_name("openai"), "OpenAI");
    assert_eq!(provider_display_name("github-copilot"), "GitHub Copilot");
}
