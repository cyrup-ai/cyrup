//! `/login` end to end from the interactive TUI (port of pi v0.83.0 `handleLoginCommand` →
//! `startProviderLogin` → `showLoginDialog`/`showApiKeyLoginDialog` → `loginProvider`,
//! `interactive-mode.ts:4993-5026`, `:5017-5025`, `:5252-5296`, `:5362-5403`).
//!
//! These drive the REAL path — `AppCommand::ConfirmSelection { kind: Login }` → the spawned flow →
//! `cyrup_config::login::login` → the session's `AuthStore` — and assert a credential lands in
//! `<agent_dir>/auth.json`. There is no mock of the middle: only the *provider registry* is
//! substituted, through `App::set_login_provider_source`, so the flow under test is the one
//! production runs.
//!
//! **No network.** The stub provider's `OAuthAuth::login` / `ApiKeyAuth` are pure in-process
//! functions: they open no socket, resolve no host and carry no real token (`"tok-"` +
//! whatever the test typed). Nothing here reaches a provider endpoint even by accident, because
//! nothing here has an endpoint.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_config::login::AuthType;
use cyrup_core::{ProviderId, StopReason};
use cyrup_provider::auth::oauth::{AuthEvent, AuthInteraction, AuthPrompt, OAuthError};
use cyrup_provider::auth::{
    ApiKeyAuth, ModelAuth, OAuthAuth, ProviderAuth,
};
use cyrup_provider::AuthError as ProviderAuthError;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_core::EventStream;
use cyrup_provider::{Context, Credential, Model, Provider, StreamEvent, StreamOptions};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use crate::crossterm::event::KeyCode;
use crate::{App, AppCommand, Entry, LoginProviderSource, LoginUiMsg, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;
use tempfile::TempDir;
use super::harness::*;

// ---------------------------------------------------------------- stub provider

/// An OAuth strategy that talks ONLY to the interaction: it emits an auth URL, then asks for the
/// code, then mints a credential from the answer. Shaped like the real ported flows
/// (`auth/oauth/openrouter.rs` notifies `AuthUrl` then prompts) minus every byte of I/O.
struct ScriptedOauth;

#[async_trait::async_trait]
impl OAuthAuth for ScriptedOauth {
    fn name(&self) -> &str {
        "Stub (Pro/Max)"
    }
    fn login_label(&self) -> Option<&str> {
        Some("Sign in with Stub")
    }
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        interaction.notify(AuthEvent::AuthUrl {
            url: "https://stub.invalid/authorize".to_string(),
            instructions: Some("Approve, then paste the code".to_string()),
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

/// An OAuth strategy that always fails, for the error-banner case.
struct FailingOauth;

#[async_trait::async_trait]
impl OAuthAuth for FailingOauth {
    fn name(&self) -> &str {
        "Stub (Pro/Max)"
    }
    async fn login(&self, _interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        Err(OAuthError::Failed("token endpoint said no".to_string()))
    }
    async fn refresh(&self, cred: &Credential) -> Result<Credential, ProviderAuthError> {
        Ok(cred.clone())
    }
    async fn to_auth(&self, _cred: &Credential) -> Result<ModelAuth, ProviderAuthError> {
        Ok(ModelAuth::default())
    }
}

/// The REAL shared `envApiKeyAuth` strategy (`cyrup_provider::auth::env_key`,
/// `ai/src/auth/helpers.ts:9-27` @v0.83.0), carrying upstream's display string the way
/// `providers/openrouter.ts:13` does.
///
/// CFG-005 / ADR-0010 step 2: this used to be a local stub reporting the `"env-key"` sentinel,
/// because `cyrup_config::login` decided "does this strategy have a login?" by SNIFFING that name.
/// The sniffer is gone — `/login` now reads `ApiKeyAuth::supports_login()` and dispatches to
/// `ApiKeyAuth::login()` — so the fixture must be the real strategy or the dialog it drives is not
/// the one production runs.
fn env_key_like() -> std::sync::Arc<dyn ApiKeyAuth> {
    cyrup_provider::auth::env_key("Stub API key", Vec::<String>::new())
}

struct StubProvider {
    id: ProviderId,
    auth: ProviderAuth,
    models: Vec<Model>,
}

#[async_trait::async_trait]
impl Provider for StubProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
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

fn stub_registry(auth: ProviderAuth) -> LoginProviderSource {
    Arc::new(move || {
        let provider: Arc<dyn Provider> = Arc::new(StubProvider {
            id: ProviderId::from("stub"),
            auth: auth.clone(),
            models: Vec::new(),
        });
        vec![provider]
    })
}

// ---------------------------------------------------------------- fixture

struct Fixture {
    _tmp: TempDir,
    agent_dir: std::path::PathBuf,
    session: Arc<AgentSession>,
}

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
    let session = SessionBuilder::new(provider, config).build().await.unwrap();
    Fixture {
        _tmp: tmp,
        agent_dir,
        session: Arc::new(session),
    }
}

fn app_with(registry: LoginProviderSource) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    app.set_login_provider_source(registry);
    app
}

fn type_text(app: &mut App<TestBackend>, text: &str) {
    for c in text.chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
}

/// Await the next message the spawned flow posts, failing loudly rather than hanging the suite.
async fn next_msg(rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>) -> LoginUiMsg {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the login flow should post a message within 5s")
        .expect("the login channel should stay open")
}

/// Every status / error / warning line the transcript holds, joined — the assertion surface for
/// Pi's `showStatus` / `showError` copy.
fn transcript_text(app: &App<TestBackend>) -> String {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Status(s) | Entry::Error(s) | Entry::Warning(s) => Some(s.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ================================================================ the proof

/// **The end-to-end.** `/login` → picker → confirm → dialog → answer the prompt → a credential is
/// in the store.
///
/// This is the test that fails if the `ConfirmSelection { kind: Login }` arm goes back to printing
/// "set the API key via `X` env var" and returning: with no flow spawned, `next_msg` times out at
/// the first `await` and the test panics.
#[tokio::test]
async fn login_confirm_runs_the_flow_and_writes_a_credential() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        ScriptedOauth,
    ))));
    let mut rx = app.install_login_channel();

    // `/login stub`: the argument pins one provider, and it offers exactly one method, so
    // `handleLoginCommand` starts that login outright with no selector in between
    // (`interactive-mode.ts:5000-5003`).
    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginDialog),
        "the login dialog must occupy the input slot"
    );

    // `notify({type:"auth_url"})` → `dialog.showAuth(url, instructions)` (`:5352`).
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Notify(_)), "expected the auth url");
    app.apply_login_msg(msg);
    let body = app.login_dialog_body().expect("dialog is open");
    assert!(body.contains("https://stub.invalid/authorize"), "{body}");
    assert!(body.contains("Approve, then paste the code"), "{body}");

    // `prompt(...)` → `dialog.showPrompt(message)` (`:5331`), which BLOCKS the flow.
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "expected a prompt");
    app.apply_login_msg(msg);
    let body = app.login_dialog_body().expect("dialog is open");
    assert!(body.contains("Paste the authorization code"), "{body}");
    // `showPrompt` appends — the auth URL is still on screen (`login-dialog.ts:152-153`).
    assert!(body.contains("https://stub.invalid/authorize"), "{body}");

    // Type the answer and submit. The dialog must STAY open (`input.onSubmit` resolves the
    // resolver and does not touch `editorContainer`, `login-dialog.ts:56-64`).
    type_text(&mut app, "CODE42");
    app.handle_input(&key(KeyCode::Enter));
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginDialog),
        "submitting a prompt must not close the dialog"
    );

    // The flow resumes, `cyrup_config::login::login` persists, and it settles.
    let msg = next_msg(&mut rx).await;
    match &msg {
        LoginUiMsg::Finished(f) => {
            assert!(f.result.is_ok(), "login failed: {:?}", f.result);
            assert!(f.oauth);
        }
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(msg);

    // ---- THE ASSERTION THAT MATTERS: a credential reached the store.
    let stored = fx
        .session
        .services()
        .auth
        .read(&ProviderId::from("stub"))
        .await
        .unwrap()
        .expect("a credential must be persisted for `stub`");
    match stored {
        cyrup_config::auth::Credential::Oauth { access, refresh, .. } => {
            assert_eq!(access, "at-CODE42", "the typed answer shaped the credential");
            assert_eq!(refresh, "rt-CODE42");
        }
        other => panic!("expected an oauth credential, got {other:?}"),
    }
    // …and on DISK, not just in memory (`getAuthPath()` = `<agent_dir>/auth.json`).
    let on_disk = std::fs::read_to_string(fx.agent_dir.join("auth.json")).unwrap();
    assert!(on_disk.contains("at-CODE42"), "auth.json: {on_disk}");

    // `completeProviderAuthentication`'s status (`interactive-mode.ts:5219`).
    assert_eq!(app.active_selector_kind(), None, "the editor is restored");
    let text = transcript_text(&app);
    assert!(text.contains("Logged in to Stub"), "{text}");
    assert!(text.contains("Credentials saved to"), "{text}");
    assert!(text.contains("auth.json"), "{text}");
}

/// **The mirror.** Same flow, same fixture, but the wiring is exercised through the *picker*
/// confirm arm — `ConfirmSelection { kind: Login, value: "<index>" }` — which is the exact arm the
/// task's gap lived in. It stays green only while that arm calls `begin_provider_login`; revert it
/// to a status push and the `LoginDialog` assertion fails immediately (no timeout needed), which is
/// what makes this a fast revert-detector next to the slower end-to-end above.
#[tokio::test]
async fn picker_confirm_arm_opens_the_dialog_and_starts_the_flow() {
    let fx = fixture().await;
    // Two methods on one provider ⇒ the auth-type selector, then the picker path.
    let auth = ProviderAuth {
        api_key: Some(env_key_like()),
        oauth: Some(Arc::new(ScriptedOauth)),
    };
    let mut app = app_with(stub_registry(auth));
    let mut rx = app.install_login_channel();

    // Bare `/login` with two methods ⇒ `showLoginAuthTypeSelector()` (`:4997`).
    app.execute_command(AppCommand::LoginCommand(None), &fx.session, None)
        .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginAuthType));

    // Choosing "subscription" with NO pinned provider opens the provider picker filtered to oauth
    // (`:5071`).
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

    // THE ARM UNDER TEST.
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::Login,
            value: "0".to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginDialog),
        "confirming a provider must open the login dialog, not print a hint"
    );
    // The old behaviour's exact copy must be gone.
    let text = transcript_text(&app);
    assert!(
        !text.contains("set the API key via"),
        "the env-var hint is the pre-wiring residual: {text}"
    );

    // And the flow really is running: it has already reached its first `notify`.
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Notify(_)), "got {msg:?}");
}

/// `Esc` in the dialog rejects the in-flight prompt with `"Login cancelled"`, the flow unwinds, and
/// the settle prints NOTHING — Pi's `if (errorMsg !== "Login cancelled")` guard (`:5294`, `:5401`).
/// Nothing is written to the store.
#[tokio::test]
async fn escape_cancels_the_login_silently_and_writes_nothing() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        ScriptedOauth,
    ))));
    let mut rx = app.install_login_channel();

    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    app.apply_login_msg(next_msg(&mut rx).await); // auth url
    app.apply_login_msg(next_msg(&mut rx).await); // prompt

    app.handle_input(&key(KeyCode::Esc));
    assert_eq!(app.active_selector_kind(), None, "Esc closes the dialog");

    let msg = next_msg(&mut rx).await;
    match &msg {
        LoginUiMsg::Finished(f) => {
            assert!(f.cancelled, "a rejected prompt is a cancel");
            assert_eq!(f.result.as_ref().unwrap_err(), "Login cancelled");
        }
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(msg);

    let text = transcript_text(&app);
    assert!(
        !text.contains("Failed to login"),
        "a cancel must not raise an error banner: {text}"
    );
    assert!(
        fx.session
            .services()
            .auth
            .read(&ProviderId::from("stub"))
            .await
            .unwrap()
            .is_none(),
        "a cancelled login must leave the store untouched"
    );
}

/// A failing flow surfaces Pi's exact banner (`` `Failed to login to ${providerName}: ${msg}` ``,
/// `:5295`) and still leaves the store untouched — `Models.login` runs the flow BEFORE it writes
/// (`ai/src/models.ts:437-441`).
#[tokio::test]
async fn a_failed_login_shows_the_banner_and_writes_nothing() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        FailingOauth,
    ))));
    let mut rx = app.install_login_channel();

    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    let msg = next_msg(&mut rx).await;
    app.apply_login_msg(msg);

    let text = transcript_text(&app);
    assert!(
        text.contains("Failed to login to Stub: token endpoint said no"),
        "{text}"
    );
    assert!(
        fx.session
            .services()
            .auth
            .read(&ProviderId::from("stub"))
            .await
            .unwrap()
            .is_none()
    );
}

/// The API-key leg: `envApiKeyAuth`'s one-secret prompt (`ai/src/auth/helpers.ts:9-27`) reaches the
/// dialog, the typed key is persisted, and the status is `Saved API key for …` (`:5183`), not
/// `Logged in to …`.
#[tokio::test]
async fn api_key_login_prompts_persists_and_uses_the_api_key_wording() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_api_key(env_key_like())));
    let mut rx = app.install_login_channel();

    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));

    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "got {msg:?}");
    app.apply_login_msg(msg);
    // `Enter ${method.name}` — `method.name` verbatim off the strategy, no reconstruction
    // (`ai/src/auth/helpers.ts:12-13`; `interactive-mode.ts:4880` carries `method` whole).
    let body = app.login_dialog_body().unwrap();
    assert!(body.contains("Stub API key"), "{body}");

    type_text(&mut app, "sk-test-123");
    app.handle_input(&key(KeyCode::Enter));
    app.apply_login_msg(next_msg(&mut rx).await);

    match fx
        .session
        .services()
        .auth
        .read(&ProviderId::from("stub"))
        .await
        .unwrap()
        .expect("api key persisted")
    {
        cyrup_config::auth::Credential::ApiKey { key, .. } => {
            assert_eq!(key.as_deref(), Some("sk-test-123"));
        }
        other => panic!("expected an api-key credential, got {other:?}"),
    }
    let text = transcript_text(&app);
    assert!(text.contains("Saved API key for Stub"), "{text}");
    assert!(!text.contains("Logged in to"), "{text}");
}

/// `/logout` deletes through the ported `cyrup_config::login::logout` and reports Pi's
/// kind-specific message (`interactive-mode.ts:5157-5161`).
#[tokio::test]
async fn logout_removes_the_stored_credential() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        ScriptedOauth,
    ))));
    let mut rx = app.install_login_channel();

    // Log in first, so there is something to remove.
    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    app.apply_login_msg(next_msg(&mut rx).await); // auth url
    app.apply_login_msg(next_msg(&mut rx).await); // prompt
    type_text(&mut app, "CODE7");
    app.handle_input(&key(KeyCode::Enter));
    app.apply_login_msg(next_msg(&mut rx).await); // finished
    assert!(fx
        .session
        .services()
        .auth
        .read(&ProviderId::from("stub"))
        .await
        .unwrap()
        .is_some());

    app.execute_command(
        AppCommand::OpenSelector(SelectorKind::Logout),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Logout));
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::Logout,
            value: "0".to_string(),
        },
        &fx.session,
        None,
    )
    .await;

    assert!(
        fx.session
            .services()
            .auth
            .read(&ProviderId::from("stub"))
            .await
            .unwrap()
            .is_none(),
        "/logout must delete the stored credential"
    );
    // The oauth wording (`:5159`).
    let text = transcript_text(&app);
    assert!(text.contains("Logged out of Stub"), "{text}");
}

/// `/logout` with nothing stored prints Pi's verbatim caveat and opens no selector
/// (`interactive-mode.ts:5136-5138`).
#[tokio::test]
async fn logout_with_nothing_stored_reports_the_caveat() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        ScriptedOauth,
    ))));
    app.execute_command(
        AppCommand::OpenSelector(SelectorKind::Logout),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), None);
    let text = transcript_text(&app);
    assert!(
        text.contains("/logout only removes credentials saved by /login"),
        "{text}"
    );
}

/// `/login <provider>` that matches exactly one option starts THAT login directly, with no picker
/// (`handleLoginCommand`, `interactive-mode.ts:5000-5003`). Matching is case-insensitive against
/// the id or the display name (`findLoginProviderOptions`, `:4985-4991`).
#[tokio::test]
async fn login_with_an_argument_skips_the_picker() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        ScriptedOauth,
    ))));
    let _rx = app.install_login_channel();
    app.execute_command(
        AppCommand::LoginCommand(Some("STUB".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::LoginDialog));
    assert!(app
        .login_dialog_title()
        .is_some_and(|t| t == "Login to Stub"));
}

/// An unmatched `/login <ref>` falls through to the full picker (`:5013`), it does not error out.
#[tokio::test]
async fn login_with_an_unknown_argument_opens_the_picker() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth {
        api_key: Some(env_key_like()),
        oauth: Some(Arc::new(ScriptedOauth)),
    }));
    let _rx = app.install_login_channel();
    app.execute_command(
        AppCommand::LoginCommand(Some("nope".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Login));
}

/// Without an installed login channel there is no run loop to answer prompts, so no task is
/// spawned — a flow that could never complete must not start (see `App::login_tx`).
#[tokio::test]
async fn no_channel_means_no_orphaned_flow() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(ProviderAuth::with_oauth(Arc::new(
        ScriptedOauth,
    ))));
    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), None);
    assert!(transcript_text(&app).contains("login unavailable"));
}
