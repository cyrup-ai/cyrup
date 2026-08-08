//! `/login` + `/logout` (oauth) selector rendering + routing (spec/tui/05 §6 data-bound selectors;
//! port of `oauth-selector.ts` + `getLoginProviderOptions`/`getLogoutProviderOptions`,
//! `interactive-mode.ts:4941-4979`).
//!
//! This file covers the PICKER half: the rows Pi's `OAuthSelectorComponent` draws
//! (`oauth-selector.ts:124-181`) and what confirming one routes to. The half that actually runs a
//! login — dialog, `AuthInteraction`, credential write — is `tests/login_flow.rs`, which drives it
//! end to end against a live `AgentSession`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_config::login::{AuthCheck, AuthType, LoginProviderOption};
use cyrup_core::ProviderId;
use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{
    format_status_indicator, login_selector_rows, provider_display_name, provider_rows, App,
    AppCommand, AuthState, InputEvent, SelectorKind, UiTheme,
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

fn option(
    id: &str,
    name: &str,
    auth_type: AuthType,
    status: Option<AuthCheck>,
) -> LoginProviderOption {
    LoginProviderOption {
        id: ProviderId::from(id),
        name: name.to_string(),
        auth_type,
        method_name: None,
        login_label: None,
        supports_login: true,
        status,
    }
}

/// The shape `execute_command` hands `open_data_selector` for `/login`: the resolved
/// `AuthSelectorProvider[]`, sorted by display name, one row per available auth method.
fn login_options() -> Vec<LoginProviderOption> {
    vec![
        option(
            "anthropic",
            "Anthropic",
            AuthType::Oauth,
            Some(AuthCheck {
                auth_type: AuthType::Oauth,
                source: Some("stored credential".to_string()),
            }),
        ),
        option("anthropic", "Anthropic", AuthType::ApiKey, None),
        option(
            "openai",
            "OpenAI",
            AuthType::ApiKey,
            Some(AuthCheck {
                auth_type: AuthType::ApiKey,
                source: Some("OPENAI_API_KEY".to_string()),
            }),
        ),
    ]
}

#[test]
fn login_selector_renders_providers_status_and_borders() {
    let mut app = App::new(TestBackend::new(78, 20), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Login, login_selector_rows(&login_options()), 0);
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    app.draw().unwrap();
    let text = buf_text(&app);

    // Full-width DynamicBorder rules top & bottom (spec/tui/05 §11).
    let rule_rows = text.lines().filter(|l| l.contains("──────────")).count();
    assert!(rule_rows >= 2, "expected top+bottom `─` rules:\n{text}");
    // Pi's verbatim picker title (`oauth-selector.ts:70`).
    assert!(
        text.contains("Select provider to configure:"),
        "missing selector title:\n{text}"
    );
    for name in ["Anthropic", "OpenAI"] {
        assert!(text.contains(name), "missing provider {name}:\n{text}");
    }
    // `showAuthTypeLabels` (`oauth-selector.ts:61`): the list mixes both kinds, so each row is
    // tagged.
    assert!(text.contains("[subscription]"), "missing oauth tag:\n{text}");
    assert!(text.contains("[API key]"), "missing api-key tag:\n{text}");
    // `formatStatusIndicator` (`oauth-selector.ts:164-181`).
    assert!(text.contains("✓ configured"), "missing stored status:\n{text}");
    assert!(
        text.contains("✓ env: OPENAI_API_KEY"),
        "env-var sources render as `env: …`:\n{text}"
    );
    assert!(text.contains("• unconfigured"), "missing unconfigured:\n{text}");
}

#[test]
fn login_confirm_routes_the_row_index_not_the_provider_id() {
    // One provider contributing TWO rows is exactly why the id alone cannot be the confirm value:
    // Pi calls back with `(providerId, authType)` (`interactive-mode.ts:5106`), which the index
    // collapses to.
    let mut app = App::new(TestBackend::new(78, 20), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Login, login_selector_rows(&login_options()), 0);
    app.handle_input(&key(KeyCode::Down)); // → Anthropic [API key], index 1
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None, "confirm closes the picker");
    match action {
        cyrup_tui::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
            assert_eq!(kind, SelectorKind::Login);
            assert_eq!(value, "1", "confirm carries the row index");
        }
        other => panic!("expected ConfirmSelection command, got {other:?}"),
    }
}

#[test]
fn status_indicator_flags_a_credential_of_the_other_kind() {
    // `if (provider.status.type !== provider.authType)` (`oauth-selector.ts:166-169`) — an API-key
    // row on a provider whose STORED credential is an OAuth one must not claim `✓ configured`.
    let mismatched = option(
        "anthropic",
        "Anthropic",
        AuthType::ApiKey,
        Some(AuthCheck {
            auth_type: AuthType::Oauth,
            source: Some("stored".to_string()),
        }),
    );
    assert_eq!(
        format_status_indicator(&mismatched),
        "• subscription configured"
    );
    // A non-env, non-sentinel source is echoed verbatim (`:178-180`).
    let runtime = option(
        "openai",
        "OpenAI",
        AuthType::ApiKey,
        Some(AuthCheck {
            auth_type: AuthType::ApiKey,
            source: Some("runtime".to_string()),
        }),
    );
    assert_eq!(format_status_indicator(&runtime), "✓ runtime");
}

#[test]
fn single_kind_list_drops_the_auth_type_tags() {
    // `showAuthTypeLabels` is false when every row is the same kind (`oauth-selector.ts:61`).
    let rows = login_selector_rows(&[
        option("openai", "OpenAI", AuthType::ApiKey, None),
        option("groq", "Groq", AuthType::ApiKey, None),
    ]);
    assert_eq!(rows[0].1, "OpenAI");
    assert_eq!(rows[1].1, "Groq");
}

#[test]
fn logout_selector_lists_stored_providers_only() {
    let options = vec![option(
        "anthropic",
        "Anthropic",
        AuthType::Oauth,
        Some(AuthCheck {
            auth_type: AuthType::Oauth,
            source: Some("stored credential".to_string()),
        }),
    )];
    let mut app = App::new(TestBackend::new(78, 20), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Logout, login_selector_rows(&options), 0);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("Select provider to logout:"),
        "missing logout title:\n{text}"
    );
    assert!(text.contains("Anthropic"), "missing stored provider:\n{text}");
    assert!(text.contains("✓ configured"), "stored cred shows configured:\n{text}");
}

#[test]
fn logout_confirm_routes_the_row_index() {
    let options = vec![option("anthropic", "Anthropic", AuthType::Oauth, None)];
    let mut app = App::new(TestBackend::new(78, 20), UiTheme::dark()).unwrap();
    app.open_data_selector(SelectorKind::Logout, login_selector_rows(&options), 0);
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None);
    match action {
        cyrup_tui::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
            assert_eq!(kind, SelectorKind::Logout);
            assert_eq!(value, "0", "logout carries the row index to delete");
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

#[test]
fn id_only_row_builder_still_sorts_by_display_name() {
    // `provider_rows` is the older id-only shape, kept for callers that have no resolved options.
    let rows = provider_rows(vec![
        ("openai".to_string(), AuthState::Unconfigured),
        ("anthropic".to_string(), AuthState::Configured),
    ]);
    assert_eq!(rows[0].0, "anthropic");
    assert_eq!(rows[1].0, "openai");
}
