//! TUI-144 — `/login`'s top-level "Sign in with Radius" option (`radius-login-selector.ts` and its
//! wiring in `interactive-mode.ts:5874-5960` @f1b2e77f5).
//!
//! The ledger row's Verify line, one test per clause:
//! 1. `/login` lists "Sign in with Radius" LAST at the top level, with its configured status —
//!    [`bare_login_lists_sign_in_with_radius_last_with_its_real_status`];
//! 2. cancelling it returns to the `/login` menu rather than the editor —
//!    [`cancelling_the_radius_login_returns_to_the_login_menu`];
//! 3. the shimmer advances on a 50 ms timer and stops when the overlay closes —
//!    [`the_shimmer_runs_on_a_50ms_tick_only_while_radius_is_highlighted`] and
//!    [`closing_the_menu_by_any_path_stops_the_shimmer`].
//!
//! Like `login_flow.rs`, only the provider REGISTRY is substituted (`set_login_provider_source`);
//! the strategies are pure in-process functions and nothing opens a socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::radius_login_selector::{
    ANIMATION_FRAME, RADIUS_COLORS, RadiusLoginMenu, RadiusOption, radius_shimmer,
};
use crate::selector::{ListSelector, Selector};
use crate::{
    App, AppAction, AppCommand, LoginProviderSource, LoginUiMsg, SPINNER_INTERVAL, SelectorKind,
    UiTheme,
};
use cyrup_config::login::AuthType;
use cyrup_core::{EventStream, ProviderId, StopReason};
use cyrup_provider::AuthError as ProviderAuthError;
use cyrup_provider::auth::oauth::{AuthEvent, AuthInteraction, AuthPrompt, OAuthError};
use cyrup_provider::auth::{
    ApiKeyAuth, AuthContext, AuthResult, LoginOptions, ModelAuth, OAuthAuth, ProviderAuth,
};
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_provider::{Context, Credential, Model, Provider, StreamEvent, StreamOptions};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use tempfile::TempDir;

// ---------------------------------------------------------------- stubs

/// Notifies an auth URL, then blocks on a prompt — so a test can cancel a login that is really
/// running.
struct PromptingOauth;

#[async_trait::async_trait]
impl OAuthAuth for PromptingOauth {
    fn name(&self) -> &str {
        "Stub OAuth"
    }
    async fn login(
        &self,
        interaction: &dyn AuthInteraction,
        _options: &LoginOptions,
    ) -> Result<Credential, OAuthError> {
        interaction.notify(AuthEvent::AuthUrl {
            url: "https://stub.invalid/authorize".to_string(),
            instructions: None,
        });
        let code = interaction
            .prompt(AuthPrompt::text("Paste the authorization code"))
            .await?;
        Ok(Credential::Oauth {
            refresh: format!("rt-{code}"),
            access: format!("at-{code}"),
            expires: 1_700_000_000_000,
            ext: serde_json::Map::new(),
        })
    }
    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderAuthError> {
        Ok(cred.clone())
    }
    async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, ProviderAuthError> {
        Ok(ModelAuth::default())
    }
}

/// An api-key strategy with NO interactive login — pi's ambient case ("configured outside"),
/// whose dialog has no flow behind it.
struct AmbientKey;

#[async_trait::async_trait]
impl ApiKeyAuth for AmbientKey {
    fn name(&self) -> &str {
        "Ambient credentials"
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, ProviderAuthError> {
        Ok(None)
    }
}

struct StubProvider {
    id: ProviderId,
    auth: ProviderAuth,
}

#[async_trait::async_trait]
impl Provider for StubProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &[]
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        Box::pin(futures::stream::empty())
    }
}

fn registry(providers: Vec<(&'static str, ProviderAuth)>) -> LoginProviderSource {
    Arc::new(move || {
        providers
            .iter()
            .map(|(id, auth)| {
                let provider: Arc<dyn Provider> = Arc::new(StubProvider {
                    id: ProviderId::from(*id),
                    auth: auth.clone(),
                });
                provider
            })
            .collect()
    })
}

/// `stub` (both methods), plus `radius` (OAuth) — the registry shape `/login` sees on a real
/// install, where `radius` is a built-in OAuth provider (`providers/all.rs`).
fn with_radius() -> LoginProviderSource {
    registry(vec![
        (
            "stub",
            ProviderAuth {
                api_key: Some(cyrup_provider::auth::env_key(
                    "Stub API key",
                    Vec::<String>::new(),
                )),
                oauth: Some(Arc::new(PromptingOauth)),
            },
        ),
        ("radius", ProviderAuth::with_oauth(Arc::new(PromptingOauth))),
    ])
}

struct Fixture {
    _tmp: TempDir,
    session: Arc<AgentSession>,
}

/// A session whose auth store reads `<agent_dir>/auth.json` with the ambient env tier pinned EMPTY,
/// so a `RADIUS_API_KEY` on the machine running the suite cannot change the status under test.
async fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.trust_override = Some(true);
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let auth = Arc::new(
        cyrup_config::AuthStore::at(agent_dir.join("auth.json"))
            .with_ambient_env(std::collections::HashMap::new()),
    );
    let session = SessionBuilder::new(provider, config)
        .auth(auth)
        .build()
        .await
        .unwrap();
    Fixture {
        _tmp: tmp,
        session: Arc::new(session),
    }
}

fn app_with(source: LoginProviderSource) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.set_login_provider_source(source);
    app
}

async fn bare_login(app: &mut App<TestBackend>, session: &Arc<AgentSession>) {
    app.execute_command(AppCommand::LoginCommand(None), session, None)
        .await;
}

/// The menu rows, top to bottom, as drawn: every row of the frame that holds one of the menu's
/// labels, trimmed.
fn menu_rows(app: &mut App<TestBackend>) -> Vec<String> {
    app.draw().unwrap();
    let h = app.terminal().backend().buffer().area.height;
    (0..h)
        .map(|y| row_text(app, y).trim().to_string())
        .filter(|row| row.contains("Sign in with"))
        .collect()
}

async fn next_msg(rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>) -> LoginUiMsg {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the login flow should post a message within 5s")
        .expect("the login channel should stay open")
}

/// Move the highlight to the Radius row (the third) and confirm it, returning the command the
/// selector emitted — the user's path, keys and all.
fn choose_radius(app: &mut App<TestBackend>) -> AppCommand {
    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Down));
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(command) => command,
        other => panic!("Enter on the Radius row should confirm it, got {other:?}"),
    }
}

/// Run the Radius login up to its blocking prompt.
async fn start_radius_login(
    app: &mut App<TestBackend>,
    session: &Arc<AgentSession>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>,
) {
    bare_login(app, session).await;
    let command = choose_radius(app);
    assert_eq!(
        command,
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: crate::app::login::RADIUS_MENU_VALUE.to_string(),
        }
    );
    app.execute_command(command, session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    assert_eq!(app.login_dialog_title().as_deref(), Some("Login to Radius"));
    for _ in 0..2 {
        let msg = next_msg(rx).await;
        app.apply_login_msg(session, msg).await;
    }
}

// ================================================================ Done 1

/// **Verify 1.** Bare `/login` lists "Sign in with Radius" as the LAST top-level option, with the
/// status `/login`'s provider rows report for `radius` — read from the credential store, not
/// spelled: `• not configured` with nothing stored, `✓ stored` (green) once a Radius OAuth
/// credential is. `stored` is pi's own text for that state: `getLoginProviderOptions` passes
/// `authStatus.label ?? authStatus.source` (`interactive-mode.ts:5795`), `getProviderAuthStatus`
/// answers `source: "stored"` for a stored credential, and `formatAuthSelectorProviderStatus` maps
/// only `"OAuth"` / `"stored credential"` to `configured` (`oauth-selector.ts:42-48`).
///
/// RED at HEAD: the menu had two rows and no Radius option (`menu_rows` returns the account and
/// API-key labels only). Red under the mutation "status hard-coded to ` ✓ configured`" (both halves
/// fail) and "Radius pushed FIRST" (the order assertion fails).
#[tokio::test]
async fn bare_login_lists_sign_in_with_radius_last_with_its_real_status() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    bare_login(&mut app, &fx.session).await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType)
    );
    assert_eq!(
        menu_rows(&mut app),
        vec![
            "→ Sign in with an account".to_string(),
            "Sign in with an API key".to_string(),
            "Sign in with Radius • not configured".to_string(),
        ]
    );
    let (y, row) = row_with(&app, "Sign in with Radius");
    let bullet = col_of(&row, "• not configured");
    assert_eq!(
        fg_at(&app, bullet, y),
        UiTheme::dark().muted_style().fg,
        "the status keeps its `muted` tone, as `formatAuthSelectorProviderStatus` themes it"
    );

    // Store a Radius OAuth credential and reopen: the status follows the store.
    app.close_selector(true);
    fx.session
        .services()
        .auth
        .modify(&ProviderId::from("radius"), |_| async {
            Ok(Some(cyrup_config::auth::Credential::Oauth {
                refresh: "rt".to_string(),
                access: "at".to_string(),
                expires: i64::MAX,
                ext: serde_json::Map::new(),
            }))
        })
        .await
        .unwrap();
    bare_login(&mut app, &fx.session).await;
    let rows = menu_rows(&mut app);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("Sign in with Radius ✓ stored")
    );
    let (y, row) = row_with(&app, "Sign in with Radius");
    let check = col_of(&row, "✓ stored");
    assert_eq!(fg_at(&app, check, y), UiTheme::dark().success_style().fg);
}

// ================================================================ Done 2

/// **Verify 2.** Esc in the Radius login dialog cancels the running flow and lands back on the
/// top-level `/login` menu — Radius row and all — not in the editor (pi's
/// `startProviderLogin(radiusOption, () => this.showLoginAuthTypeSelector())`, `:5929`, and the
/// `errorMsg === "Login cancelled"` → `onBack?.()` arm, `:6365-6366`).
///
/// RED at HEAD (no Radius row to choose — `choose_radius` gets no confirm for a third row), and
/// under the mutation "`finish_login`'s cancelled arm back to `{}`" (the slot is empty after the
/// settle).
#[tokio::test]
async fn cancelling_the_radius_login_returns_to_the_login_menu() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    let mut rx = app.install_login_channel();
    start_radius_login(&mut app, &fx.session, &mut rx).await;

    app.handle_input(&key(KeyCode::Esc));
    let msg = next_msg(&mut rx).await;
    assert!(
        matches!(&msg, LoginUiMsg::Finished(f) if f.cancelled),
        "Esc must unwind the flow as a cancel: {msg:?}"
    );
    app.apply_login_msg(&fx.session, msg).await;

    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType),
        "a cancelled Radius login returns to the /login menu, not the editor"
    );
    let rows = menu_rows(&mut app);
    assert_eq!(
        rows.last().map(String::as_str),
        Some("Sign in with Radius • not configured"),
        "it is the TOP-LEVEL menu again, with its Radius row: {rows:?}"
    );
    assert!(
        !crate::tests::harness::buf_text(&app).contains("Failed to login"),
        "a cancel raises no error banner"
    );
}

/// Exit paths that must NOT return to the menu, each of which pi also leaves in the editor:
/// a flow that unwound because its dialog was torn down under it (a session rebind clearing the
/// slot — the user did not cancel anything), and a cancel while something else already holds the
/// slot (an extension dialog must not be replaced by a menu). RED under the mutation
/// "`return_to_login_menu` drops its `selector.is_some()` guard" (second half) and "`finish_login`
/// reads `login_back` instead of `login_back_requested`" (first half).
#[tokio::test]
async fn a_dialog_torn_down_underneath_the_login_does_not_reopen_the_menu() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    let mut rx = app.install_login_channel();

    // (a) The slot is cleared while the flow waits (what `rebind_session` and
    // `reset_extension_ui` do); the flow's prompt finds no dialog, is rejected, and settles as a
    // cancel — with no user cancel behind it.
    bare_login(&mut app, &fx.session).await;
    let command = choose_radius(&mut app);
    app.execute_command(command, &fx.session, None).await;
    app.state_mut().selector = None;
    loop {
        let msg = next_msg(&mut rx).await;
        let done = matches!(msg, LoginUiMsg::Finished(_));
        app.apply_login_msg(&fx.session, msg).await;
        if done {
            break;
        }
    }
    assert_eq!(
        app.active_selector_kind(),
        None,
        "nothing the user did asked for the menu back"
    );

    // (b) The user cancels, but another selector took the slot before the flow settled.
    start_radius_login(&mut app, &fx.session, &mut rx).await;
    app.handle_input(&key(KeyCode::Esc));
    app.open_selector(SelectorKind::Theme);
    let msg = next_msg(&mut rx).await;
    app.apply_login_msg(&fx.session, msg).await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::Theme),
        "the menu must not replace a selector that already holds the slot"
    );
}

/// pi passes an `onBack` from every menu a login can start from, not just Radius (`:5935`,
/// `:5985`): a cancel returns to a pinned provider's method chooser, and to the provider picker
/// with its filter. RED under the mutation "`confirm_login_auth_type` passes `None` for the pinned
/// chooser's `back`" (the first half lands in the editor), and under "`finish_login`'s cancelled
/// arm ignores `back`" (both halves).
#[tokio::test]
async fn cancelling_returns_to_the_menu_each_login_was_started_from() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    let mut rx = app.install_login_channel();

    // Pinned provider with two methods: `/login stub` → its method chooser → subscription.
    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType)
    );
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: AuthType::Oauth.as_str().to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    app.handle_input(&key(KeyCode::Esc));
    loop {
        let msg = next_msg(&mut rx).await;
        let done = matches!(msg, LoginUiMsg::Finished(_));
        app.apply_login_msg(&fx.session, msg).await;
        if done {
            break;
        }
    }
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType)
    );
    assert_eq!(
        menu_rows(&mut app),
        vec![
            "→ Sign in with an account".to_string(),
            "Sign in with an API key".to_string(),
        ],
        "back on the PINNED chooser, which never carries the Radius row"
    );
    app.close_selector(true);

    // Bare menu → subscription → provider picker (filtered to OAuth) → radius.
    bare_login(&mut app, &fx.session).await;
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: AuthType::Oauth.as_str().to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::Login,
            value: "0".to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    app.handle_input(&key(KeyCode::Esc));
    loop {
        let msg = next_msg(&mut rx).await;
        let done = matches!(msg, LoginUiMsg::Finished(_));
        app.apply_login_msg(&fx.session, msg).await;
        if done {
            break;
        }
    }
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::Login),
        "back on the provider picker"
    );
    app.draw().unwrap();
    let text = crate::tests::harness::buf_text(&app);
    assert!(
        !text.contains("[API key]"),
        "the picker keeps its OAuth filter, so no row carries the mixed-kinds badge:\n{text}"
    );
}

/// The ambient dialog ("configured outside") has no flow behind it, so its `onBack`
/// (`interactive-mode.ts:6172-6176`) rides [`LoginUiMsg::Back`]. RED under the mutation
/// "`cancel_login` posts nothing when there is no flow" (no message ever arrives).
#[tokio::test]
async fn cancelling_an_ambient_dialog_returns_to_the_provider_picker() {
    let fx = fixture().await;
    let mut app = app_with(registry(vec![(
        "ambient",
        ProviderAuth {
            api_key: Some(Arc::new(AmbientKey)),
            oauth: None,
        },
    )]));
    let mut rx = app.install_login_channel();
    // A single api-key provider: the bare menu offers both methods; API key → the picker.
    bare_login(&mut app, &fx.session).await;
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: AuthType::ApiKey.as_str().to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::Login,
            value: "0".to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(app.active_selector_kind(), None, "Esc closes the notice");
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Back), "{msg:?}");
    app.apply_login_msg(&fx.session, msg).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
}

// ================================================================ Done 3

/// **Verify 3, first half.** While the Radius row is highlighted the open selector asks for the
/// 50 ms frame and the loop's one animation tick runs at it; with another row highlighted the
/// shimmer is off (pi animates only the SELECTED Radius line, `:79-81`) and the tick is the
/// spinner's own. RED at HEAD (no `Selector::animation_frame`; nothing animates) and under the
/// mutation "`animation_period` ignores the selector".
#[tokio::test]
async fn the_shimmer_runs_on_a_50ms_tick_only_while_radius_is_highlighted() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    bare_login(&mut app, &fx.session).await;
    app.draw().unwrap();
    assert_eq!(app.selector_animation_frame(), None, "row 0 is highlighted");
    assert_eq!(app.animation_period(), SPINNER_INTERVAL);

    app.handle_input(&key(KeyCode::Down));
    app.handle_input(&key(KeyCode::Down));
    app.draw().unwrap();
    assert_eq!(
        app.selector_animation_frame(),
        Some(Duration::from_millis(50))
    );
    assert_eq!(app.animation_period(), ANIMATION_FRAME);
    assert_eq!(ANIMATION_FRAME, Duration::from_millis(50));

    // The highlighted Radius text is painted in the logo colours, not the accent.
    let (y, row) = row_with(&app, "Sign in with Radius");
    let x = col_of(&row, "Sign in with Radius");
    let colours: std::collections::BTreeSet<String> = (x..x + 19)
        .map(|cx| format!("{:?}", fg_at(&app, cx, y)))
        .collect();
    assert!(
        colours.len() > 3,
        "the shimmer paints a gradient across the text: {colours:?}"
    );
    assert!(
        !colours.contains(&format!("{:?}", UiTheme::dark().accent_style().fg)),
        "no character keeps the plain accent: {colours:?}"
    );

    // Moving off the row stops it again.
    app.handle_input(&key(KeyCode::Up));
    app.draw().unwrap();
    assert_eq!(app.selector_animation_frame(), None);
}

/// **Verify 3, second half.** The shimmer stops when the overlay closes — by every path that
/// removes it from the slot: Esc, confirming another row, confirming Radius itself, and the slot
/// being cleared from outside (`rebind_session`, `reset_extension_ui`). After each, nothing asks
/// for the 50 ms tick and the period is the spinner's again, i.e. no animation subscription is
/// left running. Non-regression guard for the slot-derived design (every path removes the
/// selector, so nothing is left to stop); compile-red at HEAD (no `selector_animation_frame`). The
/// run-loop half — that the tick really re-arms back to 80 ms and disarms — is red-proven by
/// [`the_run_loop_drives_the_shimmer_from_its_one_animation_tick`].
#[tokio::test]
async fn closing_the_menu_by_any_path_stops_the_shimmer() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    let _rx = app.install_login_channel();

    async fn open_animating(app: &mut App<TestBackend>, session: &Arc<AgentSession>) {
        bare_login(app, session).await;
        app.handle_input(&key(KeyCode::Down));
        app.handle_input(&key(KeyCode::Down));
        app.draw().unwrap();
        assert_eq!(app.selector_animation_frame(), Some(ANIMATION_FRAME));
    }
    let stopped = |app: &App<TestBackend>, path: &str| {
        assert_eq!(app.selector_animation_frame(), None, "after {path}");
        assert_eq!(app.animation_period(), SPINNER_INTERVAL, "after {path}");
    };

    open_animating(&mut app, &fx.session).await;
    app.handle_input(&key(KeyCode::Esc));
    stopped(&app, "Esc");

    open_animating(&mut app, &fx.session).await;
    app.handle_input(&key(KeyCode::Up));
    let _ = app.handle_input(&key(KeyCode::Enter));
    stopped(&app, "confirming another row");

    open_animating(&mut app, &fx.session).await;
    let _ = app.handle_input(&key(KeyCode::Enter));
    stopped(&app, "confirming Radius");

    open_animating(&mut app, &fx.session).await;
    app.state_mut().selector = None;
    stopped(&app, "the slot being cleared (rebind / extension-UI reset)");

    open_animating(&mut app, &fx.session).await;
    app.open_selector(SelectorKind::Theme);
    stopped(&app, "another selector replacing the menu");
}

/// The run loop arms its ONE animation tick from these two answers — there is no second clock.
/// A source pin, because the loop needs a real terminal: the spinner arm's guard must include the
/// selector's answer, and the loop must re-arm the tick from `animation_period()`. RED under the
/// mutation "drop `|| self.selector_animation_frame().is_some()` from the guard" (the shimmer would
/// then advance only when some OTHER event redraws — exactly what the row says is not the feature).
#[test]
fn the_run_loop_drives_the_shimmer_from_its_one_animation_tick() {
    const RUN_SRC: &str = include_str!("../app/run.rs");
    const ARMS_SRC: &str = include_str!("../app/run_arms.rs");
    let arm_start = RUN_SRC.find("_ = ctx.spinner.tick(),").unwrap();
    let arm = RUN_SRC.get(arm_start..arm_start + 400).unwrap();
    assert!(
        arm.contains("self.selector_animation_frame().is_some()"),
        "the spinner arm's guard must arm while a selector animates:\n{arm}"
    );
    assert!(
        RUN_SRC.contains("if ctx.spinner.period() != self.animation_period() {"),
        "the loop must re-arm the tick when the wanted period changes"
    );
    assert!(
        !ARMS_SRC.contains("interval(self.state.indicator.spinner_period())"),
        "every re-arm must go through `animation_period()`, or a 50 ms menu would be reset to 80 ms"
    );
    assert_eq!(
        RUN_SRC
            .matches("tokio::time::interval(SPINNER_INTERVAL)")
            .count(),
        1,
        "one animation clock"
    );
}

// ================================================================ the shimmer itself

/// `radiusShimmer`'s colours, against pi's own output. The expected values were produced by
/// RUNNING `radius-login-selector.ts`'s `radiusShimmer` body with pi's `mixColors`/`foregroundAnsi`
/// (`packages/tui/src/colors.ts` @f1b2e77f5) under Bun, in `"truecolor"` mode, over
/// `"Sign in with Radius"` at 0, 50 and 1234 ms. RED under the mutations "mix in OKLCH", "no
/// smoothstep (linear `t`)" and "`index + offset`" (colours flow right to left) — each changes
/// most of these triples.
#[test]
fn the_shimmer_matches_pis_frames_colour_for_colour() {
    let golden: [(f64, [[u8; 3]; 19]); 3] = [
        (
            0.0,
            [
                [77, 154, 191],
                [85, 162, 194],
                [104, 179, 201],
                [123, 196, 207],
                [131, 204, 210],
                [148, 202, 191],
                [186, 197, 149],
                [224, 192, 106],
                [241, 190, 87],
                [241, 183, 94],
                [241, 167, 109],
                [240, 151, 123],
                [240, 144, 130],
                [215, 146, 140],
                [159, 149, 161],
                [102, 152, 181],
                [77, 154, 191],
                [85, 162, 194],
                [104, 179, 201],
            ],
        ),
        (
            50.0,
            [
                [84, 154, 188],
                [79, 156, 192],
                [94, 170, 197],
                [114, 188, 204],
                [129, 202, 209],
                [136, 203, 205],
                [166, 200, 171],
                [206, 194, 126],
                [236, 191, 92],
                [241, 188, 89],
                [241, 175, 101],
                [240, 159, 116],
                [240, 146, 128],
                [233, 144, 133],
                [188, 147, 149],
                [129, 151, 172],
                [84, 154, 188],
                [79, 156, 192],
                [94, 170, 197],
            ],
        ),
        (
            1234.0,
            [
                [130, 203, 210],
                [139, 203, 201],
                [172, 199, 164],
                [212, 194, 119],
                [239, 190, 90],
                [241, 187, 90],
                [241, 173, 103],
                [240, 156, 119],
                [240, 145, 129],
                [228, 145, 134],
                [179, 148, 153],
                [120, 151, 175],
                [80, 154, 190],
                [81, 158, 192],
                [97, 173, 198],
                [117, 191, 205],
                [130, 203, 210],
                [139, 203, 201],
                [172, 199, 164],
            ],
        ),
    ];
    for (ms, expected) in golden {
        let got: Vec<[u8; 3]> = radius_shimmer(19, ms)
            .into_iter()
            .map(|c| [c.r, c.g, c.b])
            .collect();
        assert_eq!(got, expected.to_vec(), "at {ms} ms");
    }
    // The cycle starts on the first logo colour and the 50 ms step moves every character.
    assert_eq!(radius_shimmer(1, 0.0)[0], RADIUS_COLORS[0]);
    assert_ne!(radius_shimmer(19, 0.0), radius_shimmer(19, 50.0));
}

/// The menu paints those colours onto the highlighted Radius text through the theme's colour
/// mode — pi's `foregroundAnsi(color, theme.getColorMode())` — and leaves the other rows and the
/// status suffix alone. On a 256-colour terminal the projected indices match pi's `"256color"` run
/// at 1234 ms (Bun, same source): RED under the mutation "skip `color_mode.project`" (the cells
/// stay `Rgb`). The other-rows half is a non-regression guard.
#[test]
fn the_menu_paints_the_shimmer_on_the_selected_radius_text_only() {
    let theme = UiTheme::dark().with_color_mode(crate::theme::ColorMode::Ansi256);
    let rows = vec![
        (
            "oauth".to_string(),
            "Sign in with an account".to_string(),
            None,
        ),
        (
            "api_key".to_string(),
            "Sign in with an API key".to_string(),
            None,
        ),
        (
            "radius".to_string(),
            "Sign in with Radius • not configured".to_string(),
            None,
        ),
    ];
    let list = ListSelector::data(SelectorKind::LoginAuthType, rows, 2);
    let option = RadiusOption {
        text: "Sign in with Radius".to_string(),
        status: vec![(
            crate::auth_select::StatusTone::Muted,
            " • not configured".to_string(),
        )],
    };
    let mut menu = RadiusLoginMenu::new(list, option);
    assert_eq!(
        menu.option().label(),
        "Sign in with Radius • not configured"
    );
    menu.freeze_clock_at(Duration::from_millis(1234));
    let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            menu.render(frame, area, &theme);
        })
        .unwrap();
    assert_eq!(menu.animation_frame(), Some(ANIMATION_FRAME));
    let buf = terminal.backend().buffer().clone();
    let row_of = |needle: &str| {
        (0..buf.area.height)
            .find(|&y| {
                (0..buf.area.width)
                    .filter_map(|x| buf.cell((x, y)))
                    .map(|c| c.symbol())
                    .collect::<String>()
                    .contains(needle)
            })
            .unwrap()
    };
    let y = row_of("Sign in with Radius");
    let start = (0..buf.area.width)
        .find(|&x| buf.cell((x, y)).unwrap().symbol() == "S")
        .unwrap();
    let got: Vec<Color> = (start..start + 19)
        .map(|x| buf.cell((x, y)).unwrap().fg)
        .collect();
    let pi_256: [u8; 19] = [
        116, 116, 151, 180, 215, 215, 215, 216, 210, 174, 138, 103, 67, 73, 74, 110, 116, 116, 151,
    ];
    assert_eq!(
        got,
        pi_256
            .iter()
            .map(|i| Color::Indexed(*i))
            .collect::<Vec<_>>()
    );
    // The status suffix keeps its own tone.
    assert_eq!(
        buf.cell((start + 20, y)).unwrap().fg,
        theme.muted_style().fg.unwrap()
    );
    // Every OTHER row is cell-for-cell what the plain list draws (non-regression guard): the
    // wrapper recolours the Radius row and nothing else.
    let mut plain = ListSelector::data(
        SelectorKind::LoginAuthType,
        vec![
            (
                "oauth".to_string(),
                "Sign in with an account".to_string(),
                None,
            ),
            (
                "api_key".to_string(),
                "Sign in with an API key".to_string(),
                None,
            ),
            (
                "radius".to_string(),
                "Sign in with Radius • not configured".to_string(),
                None,
            ),
        ],
        2,
    );
    let mut reference = Terminal::new(TestBackend::new(80, 12)).unwrap();
    reference
        .draw(|frame| {
            let area = frame.area();
            plain.render(frame, area, &theme);
        })
        .unwrap();
    let plain_buf = reference.backend().buffer().clone();
    for row in 0..buf.area.height {
        if row == y {
            continue;
        }
        for x in 0..buf.area.width {
            assert_eq!(
                buf.cell((x, row)),
                plain_buf.cell((x, row)),
                "cell ({x}, {row}) differs from the plain menu"
            );
        }
    }
}

// ================================================================ guards

/// Non-regression guard (passes at HEAD): a pinned provider's method chooser and a registry with
/// no `radius` provider produce exactly the menu they always did — the Radius row is the bare
/// menu's, and only when Radius exists — and neither animates.
#[tokio::test]
async fn menus_without_a_radius_row_are_unchanged() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(
        menu_rows(&mut app),
        vec![
            "→ Sign in with an account".to_string(),
            "Sign in with an API key".to_string(),
        ]
    );
    assert_eq!(app.selector_animation_frame(), None);
    app.close_selector(true);

    let mut app = app_with(registry(vec![(
        "stub",
        ProviderAuth::with_oauth(Arc::new(PromptingOauth)),
    )]));
    bare_login(&mut app, &fx.session).await;
    assert_eq!(
        menu_rows(&mut app),
        vec![
            "→ Sign in with an account".to_string(),
            "Sign in with an API key".to_string(),
        ]
    );
    app.handle_input(&key(KeyCode::Down));
    app.draw().unwrap();
    assert_eq!(app.selector_animation_frame(), None);
    assert_eq!(app.animation_period(), SPINNER_INTERVAL);
}

/// Non-regression guard: the two existing rows still route as before — the account row opens
/// the provider picker filtered to OAuth, and that picker does not list Radius twice or drop it.
#[tokio::test]
async fn the_existing_rows_route_as_before() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    bare_login(&mut app, &fx.session).await;
    let command = match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(c) => c,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        command,
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: "oauth".to_string(),
        }
    );
    app.execute_command(command, &fx.session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    app.draw().unwrap();
    let text = crate::tests::harness::buf_text(&app);
    assert_eq!(text.matches("Radius").count(), 1, "{text}");
    assert!(text.contains("Stub"), "{text}");
}

// ================================================================ TUI-185: Esc in the provider picker

/// Press Esc in whatever occupies the slot and run the command it emits (if any), as the run loop
/// does — returning the action so a test can also pin WHICH path was taken.
async fn esc_and_run(app: &mut App<TestBackend>, session: &Arc<AgentSession>) -> AppAction {
    let action = app.handle_input(&key(KeyCode::Esc));
    if let AppAction::Command(command) = &action {
        app.execute_command(command.clone(), session, None).await;
    }
    action
}

/// Drive a login dialog that is blocked on its prompt to its cancel settle.
async fn cancel_running_login(
    app: &mut App<TestBackend>,
    session: &Arc<AgentSession>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>,
) {
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    app.handle_input(&key(KeyCode::Esc));
    loop {
        let msg = next_msg(rx).await;
        let done = matches!(msg, LoginUiMsg::Finished(_));
        app.apply_login_msg(session, msg).await;
        if done {
            break;
        }
    }
}

/// **TUI-185 Done 1.** Bare `/login` → "Sign in with an account" (and, separately, "Sign in with an
/// API key") → the provider picker → Esc lands back on the METHOD MENU, Radius row and all — pi's
/// `onCancel`: `done(); if (authType) this.showLoginAuthTypeSelector(); …`
/// (`interactive-mode.ts:5992-5999` @f1b2e77f5).
///
/// RED with the new arm disabled (HEAD's `_ => {}`): `Oauth: Esc asks for pi's bare
/// \`showLoginAuthTypeSelector()\`` with `left: Redraw, right: Command(OpenSelector(Login))`.
#[tokio::test]
async fn esc_in_the_picker_opened_from_the_method_menu_returns_to_the_method_menu() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    for method in [AuthType::Oauth, AuthType::ApiKey] {
        app.state_mut().editor.set_text("draft");
        bare_login(&mut app, &fx.session).await;
        app.execute_command(
            AppCommand::ConfirmSelection {
                kind: SelectorKind::LoginAuthType,
                value: method.as_str().to_string(),
            },
            &fx.session,
            None,
        )
        .await;
        assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));

        let action = esc_and_run(&mut app, &fx.session).await;

        assert_eq!(
            action,
            AppAction::Command(AppCommand::OpenSelector(SelectorKind::Login)),
            "{method:?}: Esc asks for pi's bare `showLoginAuthTypeSelector()`"
        );
        assert_eq!(
            app.active_selector_kind(),
            Some(SelectorKind::LoginAuthType),
            "{method:?}: back on the method menu"
        );
        assert_eq!(
            menu_rows(&mut app),
            vec![
                "→ Sign in with an account".to_string(),
                "Sign in with an API key".to_string(),
                "Sign in with Radius • not configured".to_string(),
            ],
            "{method:?}: the TOP-LEVEL menu (pi passes no `providerOptions`)"
        );
        // Esc on the menu itself is pi's `done(); requestRender()` → the editor, draft intact.
        assert_eq!(esc_and_run(&mut app, &fx.session).await, AppAction::Redraw);
        assert_eq!(app.active_selector_kind(), None);
        assert_eq!(app.state().editor.text(), "draft");
    }
}

/// **TUI-185 Done 2.** `/login <unknown>` opens the picker with `authType` undefined
/// (`interactive-mode.ts:5871`), so Esc lands in the EDITOR, draft intact.
///
/// Non-regression guard: green at HEAD (the editor was where every picker Esc landed). Red under
/// the mutation "reopen the menu whatever `login_picker_args` holds" (`Some((_, _))`), with
/// `left: Command(OpenSelector(Login)), right: Redraw`.
#[tokio::test]
async fn esc_in_the_picker_opened_by_an_unknown_provider_name_returns_to_the_editor() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    app.state_mut().editor.set_text("draft");
    app.execute_command(
        AppCommand::LoginCommand(Some("nope".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));

    assert_eq!(esc_and_run(&mut app, &fx.session).await, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None, "in the editor");
    assert_eq!(app.state().editor.text(), "draft");
}

/// **TUI-185, the third path.** A picker RE-SHOWN by a cancelled login's `onBack`
/// (`() => this.showLoginProviderSelector(authType, initialSearchInput)`, `interactive-mode.ts:5989`)
/// keeps the `authType` it was first opened with, so its Esc follows the same rule: from the
/// method menu → back to the method menu; from `/login <unknown>` → the editor.
///
/// RED with the new arm disabled on the first half (`left: None, right: Some(LoginAuthType)`); the
/// second half is a guard, red under the same "always reopen the menu" mutation as the test above.
#[tokio::test]
async fn esc_in_a_picker_reopened_by_a_cancelled_login_follows_its_origin() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());
    let mut rx = app.install_login_channel();

    // Bare menu → account → picker → stub → login → Esc → picker (onBack) → Esc → menu.
    bare_login(&mut app, &fx.session).await;
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: AuthType::Oauth.as_str().to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::Login,
            value: "0".to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    cancel_running_login(&mut app, &fx.session, &mut rx).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    esc_and_run(&mut app, &fx.session).await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType),
        "the re-shown picker still knows it came from the method menu"
    );
    app.close_selector(true);

    // `/login nope` → picker → stub → login → Esc → picker (onBack) → Esc → editor.
    app.execute_command(
        AppCommand::LoginCommand(Some("nope".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::Login,
            value: "0".to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    cancel_running_login(&mut app, &fx.session, &mut rx).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    assert_eq!(esc_and_run(&mut app, &fx.session).await, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None, "in the editor");
}

/// TUI-185 control — cancelling a NON-provider-picker selector behaves as before: the method menu
/// itself (pi's `onCancel` is `done(); requestRender()`), `/logout`'s picker and the thinking picker
/// all land in the editor with no command, and `/tree`'s "Summarize branch?" still returns to the
/// tree (its own arm).
///
/// Non-regression guard: green at HEAD. Red under the mutation "match `Login | LoginAuthType`" in
/// the new arm (the method menu's Esc would then read the stale picker args and reopen itself:
/// `left: Command(OpenSelector(Login)), right: Redraw`).
#[tokio::test]
async fn cancelling_a_non_login_selector_behaves_as_before() {
    let fx = fixture().await;
    let mut app = app_with(with_radius());

    // Leave stale picker args behind (opened from the menu, then confirmed away) so a too-wide
    // arm would have something to act on.
    bare_login(&mut app, &fx.session).await;
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: AuthType::Oauth.as_str().to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
    // Replace the picker without going through its cancel (another selector took the slot).
    app.close_selector(false);

    bare_login(&mut app, &fx.session).await;
    assert_eq!(esc_and_run(&mut app, &fx.session).await, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None);

    app.open_selector(SelectorKind::Thinking);
    assert_eq!(esc_and_run(&mut app, &fx.session).await, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None);

    app.open_data_selector(
        SelectorKind::Logout,
        vec![("0".to_string(), "stub".to_string(), None)],
        0,
    );
    assert_eq!(esc_and_run(&mut app, &fx.session).await, AppAction::Redraw);
    assert_eq!(app.active_selector_kind(), None);

    app.open_data_selector(
        SelectorKind::BranchSummary,
        vec![("none".to_string(), "No summary".to_string(), None)],
        0,
    );
    assert_eq!(
        app.handle_input(&key(KeyCode::Esc)),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree)),
        "the BranchSummary arm is untouched"
    );
}

// ================================================================ TUI-186: the Radius intro

/// An OAuth strategy whose login opens with a `select` prompt — the shape of the Radius flow's
/// method choice (`cyrup-provider` `auth/oauth/radius.rs`, pi `radius.ts:365-376`).
struct SelectingOauth;

#[async_trait::async_trait]
impl OAuthAuth for SelectingOauth {
    fn name(&self) -> &str {
        "Selecting OAuth"
    }
    async fn login(
        &self,
        interaction: &dyn AuthInteraction,
        _options: &LoginOptions,
    ) -> Result<Credential, OAuthError> {
        let method = interaction
            .prompt(AuthPrompt::select(
                "Sign in to Radius:",
                vec![cyrup_provider::auth::oauth::AuthSelectOption {
                    id: "browser".to_string(),
                    label: "Sign in with browser (recommended)".to_string(),
                    description: None,
                }],
            ))
            .await?;
        Err(OAuthError::Failed(format!("stub stops after {method}")))
    }
    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderAuthError> {
        Ok(cred.clone())
    }
    async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, ProviderAuthError> {
        Ok(ModelAuth::default())
    }
}

/// `/login <id>` on a provider whose login opens with a select prompt; returns the dialog body once
/// the prompt is up.
async fn select_prompt_body(id: &'static str) -> String {
    let fx = fixture().await;
    let mut app = app_with(registry(vec![(
        id,
        ProviderAuth::with_oauth(Arc::new(SelectingOauth)),
    )]));
    let mut rx = app.install_login_channel();
    app.execute_command(
        AppCommand::LoginCommand(Some(id.to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "{msg:?}");
    app.apply_login_msg(&fx.session, msg).await;
    app.login_dialog_body().expect("the dialog is open")
}

/// **TUI-186 Done 1 + 2, through the App.** The Radius sign-in select prompt shows the intro under
/// its message; the same prompt from another provider's login does not. Proves the provider id is
/// threaded from `begin_provider_login` through the dialog into `show_auth_prompt`.
///
/// Red under the mutation "`open_login_dialog` gives the dialog an empty provider id" (the Radius
/// half: no intro); the non-Radius half is a guard, red under "`auth_select_description` answers
/// the intro for every provider". At HEAD no intro existed anywhere.
#[tokio::test]
async fn the_radius_sign_in_prompt_shows_the_intro_and_another_providers_does_not() {
    let radius = select_prompt_body("radius").await;
    assert!(
        radius.contains(crate::login_dialog::RADIUS_LOGIN_INTRO),
        "{radius}"
    );
    let at = |needle: &str| radius.find(needle).unwrap();
    assert!(at("Sign in to Radius:") < at(crate::login_dialog::RADIUS_LOGIN_INTRO));

    let other = select_prompt_body("stub").await;
    assert!(other.contains("Sign in to Radius:"), "{other}");
    assert!(
        !other.contains(crate::login_dialog::RADIUS_LOGIN_INTRO),
        "{other}"
    );
}
