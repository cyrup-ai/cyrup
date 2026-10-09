//! `/login` + `/logout` (oauth) selector rendering + routing (spec/tui/05 §6 data-bound selectors;
//! port of `oauth-selector.ts` + `getLoginProviderOptions`/`getLogoutProviderOptions`,
//! `interactive-mode.ts:4941-4979`).
//!
//! This file covers the PICKER half: the rows Pi's `OAuthSelectorComponent` draws
//! (`oauth-selector.ts:124-181`) and what confirming one routes to. The half that actually runs a
//! login — dialog, `AuthInteraction`, credential write — is `tests/login_flow.rs`, which drives it
//! end to end against a live `AgentSession`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::{
    App, AppAction, AppCommand, AuthState, OAuthMode, OAuthSelector, SelectorKind, UiTheme,
    format_status_indicator, login_selector_rows, provider_display_name, provider_rows,
};
use cyrup_config::login::{AuthCheck, AuthType, LoginProviderOption};
use cyrup_core::ProviderId;
use ratatui::backend::TestBackend;

/// Open the real `OAuthSelectorComponent` port (`oauth-selector.ts`) in the editor slot — the same
/// thing `App::execute_command` now does for `/login` and `/logout`. These tests used to open a
/// bare `ListSelector` instead, which is precisely the component S5/S21 say is the wrong one.
fn open_oauth(app: &mut App<TestBackend>, mode: OAuthMode, options: &[LoginProviderOption]) {
    let kind = match mode {
        OAuthMode::Login => SelectorKind::Login,
        OAuthMode::Logout => SelectorKind::Logout,
    };
    app.open_boxed_selector(kind, Box::new(OAuthSelector::new(mode, options, None)));
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
        subscription: None,
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
    open_oauth(&mut app, OAuthMode::Login, &login_options());
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
    assert!(
        text.contains("[subscription]"),
        "missing oauth tag:\n{text}"
    );
    assert!(text.contains("[API key]"), "missing api-key tag:\n{text}");
    // `formatStatusIndicator` (`oauth-selector.ts:164-181`).
    assert!(
        text.contains("✓ configured"),
        "missing stored status:\n{text}"
    );
    assert!(
        text.contains("✓ env: OPENAI_API_KEY"),
        "env-var sources render as `env: …`:\n{text}"
    );
    assert!(
        text.contains("• not configured"),
        "missing not configured:\n{text}"
    );
}

#[test]
fn login_confirm_routes_the_row_index_not_the_provider_id() {
    // One provider contributing TWO rows is exactly why the id alone cannot be the confirm value:
    // Pi calls back with `(providerId, authType)` (`interactive-mode.ts:5106`), which the index
    // collapses to.
    let mut app = App::new(TestBackend::new(78, 20), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &login_options());
    app.handle_input(&key(KeyCode::Down)); // → Anthropic [API key], index 1
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        app.active_selector_kind(),
        None,
        "confirm closes the picker"
    );
    match action {
        crate::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
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
    open_oauth(&mut app, OAuthMode::Logout, &options);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("Select provider to logout:"),
        "missing logout title:\n{text}"
    );
    assert!(
        text.contains("Anthropic"),
        "missing stored provider:\n{text}"
    );
    assert!(
        text.contains("✓ configured"),
        "stored cred shows configured:\n{text}"
    );
}

#[test]
fn logout_confirm_routes_the_row_index() {
    let options = vec![option("anthropic", "Anthropic", AuthType::Oauth, None)];
    let mut app = App::new(TestBackend::new(78, 20), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Logout, &options);
    let action = app.handle_input(&key(KeyCode::Enter));
    assert_eq!(app.active_selector_kind(), None);
    match action {
        crate::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
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

/// The fg colour of the buffer cell at the first column of `needle` within row `y`.
fn fg_at(app: &App<TestBackend>, y: u16, row: &str, needle: &str) -> ratatui::style::Color {
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    let x = row[..byte].chars().count() as u16;
    app.terminal().backend().buffer().cell((x, y)).unwrap().fg
}

fn mixed_status_options() -> Vec<LoginProviderOption> {
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
        // A stored credential of the OTHER kind — `oauth-selector.ts:166-169`.
        option(
            "anthropic",
            "Anthropic",
            AuthType::ApiKey,
            Some(AuthCheck {
                auth_type: AuthType::Oauth,
                source: Some("stored".to_string()),
            }),
        ),
        option("groq", "Groq", AuthType::ApiKey, None),
    ]
}

/// **S21.** `formatStatusIndicator` returns *styled runs*, not one string:
/// `theme.fg("success", " ✓ configured")` (`oauth-selector.ts:175`),
/// `theme.fg("muted", " • ") + theme.fg("warning", label)` (`:168`), and
/// `theme.fg("muted", " • not configured")` (`:37` @f1b2e77f5). Routing `/login` through
/// `ListSelector` painted all three uniformly `muted` (or the whole row `accent` when highlighted).
#[test]
fn login_status_runs_keep_their_own_colours() {
    let mut app = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &mixed_status_options());
    app.draw().unwrap();
    let theme = UiTheme::dark();

    let (y, row) = row_with(&app, "✓ configured");
    assert_eq!(
        fg_at(&app, y, &row, "✓ configured"),
        theme.success_style().fg.unwrap(),
        "`✓ configured` is `success` green (`:175`): {row:?}"
    );
    let (y, row) = row_with(&app, "subscription configured");
    assert_eq!(
        fg_at(&app, y, &row, "subscription configured"),
        theme.warning_style().fg.unwrap(),
        "a credential-kind mismatch is `warning` (`:168`): {row:?}"
    );
    assert_eq!(
        fg_at(&app, y, &row, "•"),
        theme.muted_style().fg.unwrap(),
        "the bullet before it stays `muted` (`:168`): {row:?}"
    );
    let (y, row) = row_with(&app, "• not configured");
    assert_eq!(
        fg_at(&app, y, &row, "• not configured"),
        theme.muted_style().fg.unwrap(),
        "`• not configured` is `muted` (`:37` @f1b2e77f5): {row:?}"
    );
    assert_ne!(
        theme.success_style().fg,
        theme.muted_style().fg,
        "the tones must differ or this test proves nothing"
    );
    assert_ne!(theme.warning_style().fg, theme.muted_style().fg);
}

/// **S21, geometry half.** The status is CONCATENATED onto the name — `prefix + text +
/// authTypeLabel + statusIndicator` (`oauth-selector.ts:138`/`:141`) — so it starts exactly one
/// space after the badge, not in a padded 32-column description column. The badge itself is
/// `theme.fg("muted", ` [${type}]`)` (`:132`), also concatenated, and the row is inset one column
/// by `new TruncatedText(line, 1, 0)` (`:144`).
#[test]
fn login_row_is_a_single_concatenation_not_a_padded_column() {
    let mut app = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &mixed_status_options());
    app.draw().unwrap();
    let (_, row) = row_with(&app, "Groq");
    assert_eq!(
        row.trim_end(),
        "   Groq [API key] • not configured",
        "one inset column + `\"  \"` prefix + name + badge + status, no padding between them"
    );
    let (_, selected) = row_with(&app, "→ Anthropic");
    assert_eq!(
        selected.trim_end(),
        " → Anthropic [subscription] ✓ configured",
        "the highlighted row is the same concatenation behind an accent cursor"
    );
}

/// **S5 + S31.** `OAuthSelectorComponent` puts a real `Input` above the list (`oauth-selector.ts:
/// 76-87`) with a fuzzy filter over `` `${name} ${id} ${authType} ${method?.name}` `` (`:102-112`).
/// cyrup's `ListSelector` had none, so a 30-provider `/login` was un-searchable. The prompt is
/// `Input.render`'s shared unstyled `"> "` at column 0 (`input.ts:380`).
#[test]
fn login_has_a_search_input_that_filters_the_provider_list() {
    let mut app = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &mixed_status_options());
    app.draw().unwrap();
    let (_, row) = row_with(&app, "> ");
    assert!(
        row.starts_with("> "),
        "`input.ts:380` prompt at column 0: {row:?}"
    );

    for c in "groq".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
    let text = buf_text(&app);
    // The dialog is shorter once the list is filtered, so re-find the input row rather than
    // reusing the pre-filter y.
    let (_, typed) = row_with(&app, "> ");
    assert!(
        typed.starts_with("> groq"),
        "the query is echoed in the box: {typed:?}"
    );
    assert!(
        text.contains("Groq"),
        "the match survives the filter:\n{text}"
    );
    assert!(
        !text.contains("Anthropic"),
        "non-matching providers are filtered out:\n{text}"
    );
}

/// The filter reorders rows, so the confirm value must be resolved through the FILTERED view —
/// `this.filteredProviders[this.selectedIndex]` (`oauth-selector.ts:199`) — and still carry the
/// index into the ORIGINAL options slice the chrome holds.
#[test]
fn login_confirm_after_a_search_carries_the_original_index() {
    let mut app = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &mixed_status_options());
    for c in "groq".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    let action = app.handle_input(&key(KeyCode::Enter));
    match action {
        crate::AppAction::Command(AppCommand::ConfirmSelection { kind, value }) => {
            assert_eq!(kind, SelectorKind::Login);
            assert_eq!(value, "2", "Groq is index 2 of the unfiltered options");
        }
        other => panic!("expected ConfirmSelection command, got {other:?}"),
    }
}

/// `filteredProviders.length === 0` with a non-empty catalog is `"No matching providers"`
/// (`oauth-selector.ts:159`); an empty catalog gets the mode-specific copy (`:155-158`).
#[test]
fn login_empty_states_use_upstreams_two_distinct_messages() {
    let mut app = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &mixed_status_options());
    for c in "zzzz".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
    assert!(buf_text(&app).contains("No matching providers"));

    let mut empty = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut empty, OAuthMode::Logout, &[]);
    empty.draw().unwrap();
    assert!(buf_text(&empty).contains("No providers logged in. Use /login first."));
}

// ---------------------------------------------------------------------------------------------
// Behaviours the new `OAuthSelector` added and nothing asserted — each was verified by disabling
// it and watching the whole suite stay green.
// ---------------------------------------------------------------------------------------------

/// A catalog longer than `maxVisible = 8` (`oauth-selector.ts:117`).
fn many_options(n: usize) -> Vec<LoginProviderOption> {
    (0..n)
        .map(|i| {
            option(
                &format!("p{i:02}"),
                &format!("Provider {i:02}"),
                AuthType::ApiKey,
                None,
            )
        })
        .collect()
}

/// `oauth-selector.ts:147-150`:
/// `if (startIndex > 0 || endIndex < filteredProviders.length)` push
/// `theme.fg("muted", \`  (${selectedIndex + 1}/${filteredProviders.length})\`)` as a
/// `TruncatedText(_, 1, 0)` — so the row is inset one column like every other list row.
/// NOTHING asserted this row existed.
#[test]
fn login_reports_its_scroll_position_past_the_eight_row_window() {
    let mut app = App::new(TestBackend::new(78, 30), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &many_options(12));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("   (1/12)"),
        "no scroll row (:147-150):\n{text}"
    );
    assert_eq!(
        text.lines().filter(|l| l.contains("Provider ")).count(),
        8,
        "only maxVisible = 8 rows are drawn (:117):\n{text}"
    );

    // It tracks the highlight.
    app.handle_input(&key(KeyCode::Down));
    app.draw().unwrap();
    assert!(buf_text(&app).contains("   (2/12)"), "{}", buf_text(&app));

    // A list that fits the window gets no readout at all.
    let mut small = App::new(TestBackend::new(78, 30), UiTheme::dark()).unwrap();
    open_oauth(&mut small, OAuthMode::Login, &many_options(4));
    small.draw().unwrap();
    assert!(!buf_text(&small).contains("(1/4)"), "{}", buf_text(&small));
}

/// `/login` **clamps** — `Math.max(0, selectedIndex - 1)` / `Math.min(len - 1, selectedIndex + 1)`
/// (`oauth-selector.ts:186-196`) — where `/scoped-models` wraps (`scoped-models-selector.ts:
/// 286-297`). Nothing asserted the difference, so wrapping here would have passed.
#[test]
fn login_navigation_clamps_at_both_ends_instead_of_wrapping() {
    let mut app = App::new(TestBackend::new(78, 24), UiTheme::dark()).unwrap();
    let options = login_options();
    open_oauth(&mut app, OAuthMode::Login, &options);

    // Up at the top row stays on row 0 rather than jumping to the last.
    app.handle_input(&key(KeyCode::Up));
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(AppCommand::ConfirmSelection { value, .. }) => {
            assert_eq!(value, "0", "Up at index 0 must not wrap (:186-190)");
        }
        other => panic!("expected ConfirmSelection, got {other:?}"),
    }

    // Down past the last row stays on the last.
    open_oauth(&mut app, OAuthMode::Login, &options);
    for _ in 0..6 {
        app.handle_input(&key(KeyCode::Down));
    }
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(AppCommand::ConfirmSelection { value, .. }) => {
            assert_eq!(value, "2", "Down past the end must not wrap (:192-196)");
        }
        other => panic!("expected ConfirmSelection, got {other:?}"),
    }
}

// ================================================================ TUI-141

/// An option carrying pi's `subscription?: boolean` (`oauth-selector.ts:20-24` @f1b2e77f5).
fn option_sub(
    id: &str,
    name: &str,
    auth_type: AuthType,
    status: Option<AuthCheck>,
    subscription: Option<bool>,
) -> LoginProviderOption {
    LoginProviderOption {
        subscription,
        ..option(id, name, auth_type, status)
    }
}

/// TUI-141 — `formatAuthSelectorProviderType(authType, subscription)` (`oauth-selector.ts:27-33`
/// @f1b2e77f5): `API key` for an API-key row whatever the flag, `account` only for an OAuth row
/// whose flag is `false`, and `subscription` when it is `true` or unset.
#[test]
fn provider_type_label_is_three_way() {
    use crate::format_auth_selector_provider_type as label;
    assert_eq!(label(AuthType::ApiKey, None), "API key");
    assert_eq!(label(AuthType::ApiKey, Some(false)), "API key");
    assert_eq!(label(AuthType::ApiKey, Some(true)), "API key");
    assert_eq!(label(AuthType::Oauth, Some(false)), "account");
    assert_eq!(label(AuthType::Oauth, Some(true)), "subscription");
    assert_eq!(label(AuthType::Oauth, None), "subscription");
}

/// TUI-141 — `if (!provider.status) return theme.fg("muted", " • not configured")`
/// (`oauth-selector.ts:37` @f1b2e77f5; renamed from `unconfigured` by `0f8740bb6`), on every
/// path that prints it.
#[test]
fn no_status_reads_not_configured() {
    let bare = option("groq", "Groq", AuthType::ApiKey, None);
    assert_eq!(format_status_indicator(&bare), "• not configured");
    assert_eq!(
        crate::status_indicator_runs(&bare),
        vec![(crate::StatusTone::Muted, " • not configured".to_string())]
    );
    assert_eq!(AuthState::Unconfigured.status_text(), "• not configured");
}

/// TUI-141 — the mismatch label is
/// `` `${formatAuthSelectorProviderType(provider.status.type, provider.subscription)} configured` ``
/// (`oauth-selector.ts:39` @f1b2e77f5): an API-key row on an account-backed provider whose stored
/// credential is OAuth reads `account configured`, an unset flag keeps `subscription configured`,
/// and an OAuth row over an API-key credential reads `API key configured` whatever the flag.
#[test]
fn mismatch_label_is_built_from_the_type_label() {
    let oauth_stored = || {
        Some(AuthCheck {
            auth_type: AuthType::Oauth,
            source: Some("stored credential".to_string()),
        })
    };
    let account = option_sub(
        "radius",
        "Radius",
        AuthType::ApiKey,
        oauth_stored(),
        Some(false),
    );
    assert_eq!(format_status_indicator(&account), "• account configured");
    assert_eq!(
        crate::status_indicator_runs(&account),
        vec![
            (crate::StatusTone::Muted, " • ".to_string()),
            (crate::StatusTone::Warning, "account configured".to_string()),
        ]
    );
    let unset = option_sub(
        "anthropic",
        "Anthropic",
        AuthType::ApiKey,
        oauth_stored(),
        None,
    );
    assert_eq!(format_status_indicator(&unset), "• subscription configured");
    let api_key_stored = option_sub(
        "radius",
        "Radius",
        AuthType::Oauth,
        Some(AuthCheck {
            auth_type: AuthType::ApiKey,
            source: Some("stored credential".to_string()),
        }),
        Some(false),
    );
    assert_eq!(
        format_status_indicator(&api_key_stored),
        "• API key configured"
    );
}

/// TUI-141 — the `[…]` badge passes the flag too (`oauth-selector.ts:161` @f1b2e77f5), in the
/// drawn selector and in the row builder: `[account]` for `Some(false)`, `[subscription]` for
/// `None`, `[API key]` for an API-key row.
#[test]
fn login_selector_badges_an_account_backed_provider_as_account() {
    let options = vec![
        option_sub("anthropic", "Anthropic", AuthType::Oauth, None, None),
        option_sub("radius", "Radius", AuthType::Oauth, None, Some(false)),
        option_sub("radius", "Radius", AuthType::ApiKey, None, Some(false)),
    ];
    let mut app = App::new(TestBackend::new(78, 22), UiTheme::dark()).unwrap();
    open_oauth(&mut app, OAuthMode::Login, &options);
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("Anthropic [subscription] • not configured"),
        "an unset flag keeps `[subscription]`:\n{text}"
    );
    assert!(
        text.contains("Radius [account] • not configured"),
        "`subscription: false` labels the OAuth row `[account]`:\n{text}"
    );
    assert!(
        text.contains("Radius [API key] • not configured"),
        "an API-key row is `[API key]` whatever the flag:\n{text}"
    );
    let rows = login_selector_rows(&options);
    let labels: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
    assert_eq!(
        labels,
        [
            "Anthropic [subscription]",
            "Radius [account]",
            "Radius [API key]"
        ]
    );
}
