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
use crate::{App, AppCommand, Entry, LoginProviderSource, LoginUiMsg, SelectorKind, UiTheme};
use cyrup_config::login::AuthType;
use cyrup_core::EventStream;
use cyrup_core::{ProviderId, StopReason};
use cyrup_provider::AuthError as ProviderAuthError;
use cyrup_provider::auth::oauth::{AuthEvent, AuthInteraction, AuthPrompt, OAuthError};
use cyrup_provider::auth::{ApiKeyAuth, ModelAuth, OAuthAuth, ProviderAuth};
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_provider::{Context, Credential, Model, Provider, StreamEvent, StreamOptions};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

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
    registry_for("stub", auth)
}

/// [`stub_registry`] under an arbitrary provider id — **TUI-105** needs one that appears in
/// [`cyrup_config::default_model_per_provider`], which `"stub"` deliberately does not.
///
/// Only the LOGIN registry is substituted; the model catalog still comes from
/// `cyrup_provider::all_providers()`, so a login under `"deepseek"` credentials the real deepseek
/// provider and its real built-in models become available. Nothing here opens a socket: the strategy
/// is still `ScriptedOauth`.
fn registry_for(id: &'static str, auth: ProviderAuth) -> LoginProviderSource {
    Arc::new(move || {
        let provider: Arc<dyn Provider> = Arc::new(StubProvider {
            id: ProviderId::from(id),
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
    assert!(
        matches!(msg, LoginUiMsg::Notify(_)),
        "expected the auth url"
    );
    app.apply_login_msg(&fx.session, msg).await;
    let body = app.login_dialog_body().expect("dialog is open");
    assert!(body.contains("https://stub.invalid/authorize"), "{body}");
    assert!(body.contains("Approve, then paste the code"), "{body}");

    // `prompt(...)` → `dialog.showPrompt(message)` (`:5331`), which BLOCKS the flow.
    let msg = next_msg(&mut rx).await;
    assert!(
        matches!(msg, LoginUiMsg::Prompt { .. }),
        "expected a prompt"
    );
    app.apply_login_msg(&fx.session, msg).await;
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
    app.apply_login_msg(&fx.session, msg).await;

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
        cyrup_config::auth::Credential::Oauth {
            access, refresh, ..
        } => {
            assert_eq!(
                access, "at-CODE42",
                "the typed answer shaped the credential"
            );
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
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType)
    );

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
    app.apply_login_msg(&fx.session, next_msg(&mut rx).await)
        .await; // auth url
    app.apply_login_msg(&fx.session, next_msg(&mut rx).await)
        .await; // prompt

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
    app.apply_login_msg(&fx.session, msg).await;

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
    app.apply_login_msg(&fx.session, msg).await;

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
    app.apply_login_msg(&fx.session, msg).await;
    // `Enter ${method.name}` — `method.name` verbatim off the strategy, no reconstruction
    // (`ai/src/auth/helpers.ts:12-13`; `interactive-mode.ts:4880` carries `method` whole).
    let body = app.login_dialog_body().unwrap();
    assert!(body.contains("Stub API key"), "{body}");

    type_text(&mut app, "sk-test-123");
    app.handle_input(&key(KeyCode::Enter));
    app.apply_login_msg(&fx.session, next_msg(&mut rx).await)
        .await;

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
    app.apply_login_msg(&fx.session, next_msg(&mut rx).await)
        .await; // auth url
    app.apply_login_msg(&fx.session, next_msg(&mut rx).await)
        .await; // prompt
    type_text(&mut app, "CODE7");
    app.handle_input(&key(KeyCode::Enter));
    app.apply_login_msg(&fx.session, next_msg(&mut rx).await)
        .await; // finished
    assert!(
        fx.session
            .services()
            .auth
            .read(&ProviderId::from("stub"))
            .await
            .unwrap()
            .is_some()
    );

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
    assert!(
        app.login_dialog_title()
            .is_some_and(|t| t == "Login to Stub")
    );
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

// ================================================================ TUI-105

// **TUI-105** — a completed login selects the provider's default model, recounts the available
// providers, and refreshes that provider's catalog.

/// A [`cyrup_session_svc::ProviderResolver`] backed by the built-in registry — the same source the
/// real bin resolves against (`cyrup/src/session_launch.rs:177`), so the provider swap a default-model
/// selection performs is the production one. Opens no socket: resolving a provider only constructs it.
struct RegistryResolver;

impl cyrup_session_svc::ProviderResolver for RegistryResolver {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        cyrup_provider::default_models(cyrup_provider::CreateModelsOptions {
            credentials: None,
            auth_context: None,
            catalog_overlay: None,
        })
        .get_provider(provider_id)
        .ok_or_else(|| format!("no built-in provider '{provider_id}'"))
    }
}

/// A provider with NO models of its own, so `SessionBuilder` resolves no initial model and
/// `session.model()` is `None` — pi's `isUnknownModel(previousModel)` state
/// (`interactive-mode.ts:298-300`, the `unknown/unknown/unknown` sentinel from `agent.ts:57-68`),
/// which cyrup spells as `None` (`cyrup-session-svc/src/session/accessors.rs:33`).
///
/// [`fixture`]'s faux provider always supplies `faux/faux-1`, so it can never produce this state.
struct ModellessProvider(ProviderId);

#[async_trait::async_trait]
impl Provider for ModellessProvider {
    fn id(&self) -> &ProviderId {
        &self.0
    }
    fn models(&self) -> &[Model] {
        &[]
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

/// [`fixture`] on a session whose `model()` is `None` — the credential-less first run.
async fn first_run_fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(ModellessProvider(ProviderId::from("none")));
    let session = SessionBuilder::new(provider, config)
        // `set_model_resolved` swaps the owning provider in place when the target belongs to another
        // one (`cyrup-session-svc/src/session/model.rs:47`), which needs a resolver — the real bin
        // wires the same built-in registry (`cyrup/src/session_launch.rs:177`).
        .provider_resolver(Arc::new(RegistryResolver))
        // The ambient env tier pinned EMPTY: which providers are credentialed is the fixture's
        // decision (`credential`), never the host's. Ambient AWS keys used to make
        // `amazon-bedrock` available and change every provider-set assertion in this file.
        .auth(hermetic_auth(&agent_dir))
        .build()
        .await
        .unwrap();
    assert_eq!(
        session.model(),
        None,
        "the whole point of this fixture is pi's isUnknownModel state"
    );
    Fixture {
        _tmp: tmp,
        agent_dir,
        session: Arc::new(session),
    }
}

/// The provider TUI-105's end-to-end tests log in to: one whose curated default
/// (`cyrup-config/src/model/defaults.rs:8`) really is in its built-in catalog, so the selection has
/// something to select.
/// An auth store over `<agent_dir>/auth.json` whose ambient env tier is pinned empty, so no host
/// credential (AWS keys, `*_API_KEY`) can make a provider available behind the fixture's back.
/// Scrubbing the process env instead would race the tests running in parallel.
fn hermetic_auth(agent_dir: &std::path::Path) -> Arc<cyrup_config::AuthStore> {
    Arc::new(
        cyrup_config::AuthStore::at(agent_dir.join("auth.json"))
            .with_ambient_env(std::collections::HashMap::new()),
    )
}

const DEFAULTED_PROVIDER: &str = "deepseek";
const DEFAULTED_MODEL: &str = "deepseek-v4-pro";

/// A second provider with embedded models, credentialed alongside [`DEFAULTED_PROVIDER`] where a
/// test needs the available set to span more than one provider without relying on the host's env.
const SECOND_PROVIDER: &str = "groq";

/// The provider TUI-105's DEFERRED tests log in to: one whose curated default is NOT in any built-in
/// catalog, because it has none — `radius` ships zero embedded models
/// (`cyrup_provider::all_providers()`) and its default is `"auto"`
/// (`cyrup-config/src/model/defaults.rs:14`). That is pi's `deferSelection` condition exactly, and
/// the state its comment names: "Dynamic catalogs may be empty until the first authenticated network
/// refresh" (`interactive-mode.ts:5888`).
const DYNAMIC_PROVIDER: &str = "radius";
const DYNAMIC_MODEL: &str = "auto";

/// Stand in for what a completed catalog refresh does — install the provider's freshly fetched models
/// into the session's live overlay slot, which is the very mechanism
/// `refresh_model_catalogs` uses (`cyrup-session-svc/src/session/model.rs:274-277`,
/// `cyrup-provider/src/catalog_refresh.rs:71`). The fixture wires no catalog service, so there is
/// nothing to fetch FROM; the installed result is the same either way.
fn install_refreshed_catalog(session: &Arc<AgentSession>, provider: &str, ids: &[&str]) {
    let models: Vec<Model> = ids.iter().map(|id| model_named(provider, id)).collect();
    session
        .services()
        .catalog_overlay
        .install(cyrup_provider::CatalogOverlay::from_entries([(
            provider.to_string(),
            models,
        )]));
}

/// Make `provider` credentialed so its built-in models pass `has_configured_auth`
/// (`cyrup-session-svc/src/session/model.rs:106`) and therefore appear in
/// `available_model_catalog()` — pi's `getAvailableSnapshot()`.
fn credential(session: &Arc<AgentSession>, provider: &str) {
    session
        .services()
        .auth
        .set_runtime_api_key(ProviderId::from(provider), "test-key".to_string());
}

/// Drive a whole OAuth login to its `Finished`, returning pi's `actionLabel` for the provider that
/// was logged into (`interactive-mode.ts:5885`) so the assertions can build upstream's templates
/// without hardcoding a display name.
///
/// The `Finished` is NOT applied here: the tests want to inspect or adjust state between the flow
/// settling and `finish_login` running.
async fn drive_login_to_finished(
    app: &mut App<TestBackend>,
    session: &Arc<AgentSession>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>,
    provider_id: &str,
) -> (String, LoginUiMsg) {
    app.execute_command(
        AppCommand::LoginCommand(Some(provider_id.to_string())),
        session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginDialog),
        "`/login {provider_id}` must open the dialog"
    );
    app.apply_login_msg(session, next_msg(rx).await).await; // auth url
    app.apply_login_msg(session, next_msg(rx).await).await; // prompt
    type_text(app, "CODE42");
    app.handle_input(&key(KeyCode::Enter));
    let msg = next_msg(rx).await;
    let action = match &msg {
        LoginUiMsg::Finished(f) => {
            assert!(f.result.is_ok(), "login failed: {:?}", f.result);
            format!("Logged in to {}", f.provider_name)
        }
        other => panic!("expected Finished, got {other:?}"),
    };
    (action, msg)
}

/// **TUI-105**, the headline. Pi's `completeProviderAuthentication` selects the provider's curated
/// default when the session had no model, and says so
/// (`` `${actionLabel}. Selected ${selectedModel.id}. Credentials saved to ${getAuthPath()}` ``,
/// `interactive-mode.ts:5898-5932`). cyrup's `finish_login` did nothing but push
/// `` `${action}. Credentials saved to {path}` ``, so a first run ended a successful `/login` still
/// unable to send a message until the user found `/model` on their own.
///
/// **Red without the change:** `session.model()` stays `None` and the status carries no `Selected`
/// clause.
#[tokio::test]
async fn tui105_successful_login_selects_the_provider_default_model() {
    let fx = first_run_fixture().await;
    // The provider's catalog is already available, so `deferSelection` (`:5889-5895`) is false and
    // the selection happens inline. The deferred route has its own test below.
    credential(&fx.session, DEFAULTED_PROVIDER);
    let mut app = app_with(registry_for(
        DEFAULTED_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();

    let (action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DEFAULTED_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;

    let model = fx.session.model().expect("the login must select a model");
    assert_eq!(model.provider.as_str(), DEFAULTED_PROVIDER);
    assert_eq!(
        model.model.as_str(),
        DEFAULTED_MODEL,
        "`defaultModelPerProvider[providerId]` (`interactive-mode.ts:5911`)"
    );
    let path = fx.agent_dir.join("auth.json").display().to_string();
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!(
            "{action}. Selected {DEFAULTED_MODEL}. Credentials saved to {path}"
        )),
        "`:5932` verbatim; got:\n{text}"
    );
}

/// **TUI-105.** Pi's `isUnknownModel(previousModel)` gate (`interactive-mode.ts:5898`): a session
/// that ALREADY has a model keeps it, and the status is the bare
/// `` `${actionLabel}. Credentials saved to ${getAuthPath()}` `` (`:5937`) with no `Selected` clause.
///
/// Without this gate the fix would silently retarget a user who ran `/model` and then `/login`, which
/// is a worse bug than the one it fixes — so this test is the one that stops it. [`fixture`]'s faux
/// provider supplies `faux/faux-1`, which is exactly the "already chose a model" state.
#[tokio::test]
async fn tui105_login_does_not_override_an_existing_model() {
    let fx = fixture().await;
    let before = fx.session.model().expect("the faux fixture has a model");
    credential(&fx.session, DEFAULTED_PROVIDER);
    let mut app = app_with(registry_for(
        DEFAULTED_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();

    let (action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DEFAULTED_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;

    assert_eq!(
        fx.session.model(),
        Some(before),
        "`isUnknownModel(previousModel)` is false, so nothing is selected"
    );
    let path = fx.agent_dir.join("auth.json").display().to_string();
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!("{action}. Credentials saved to {path}")),
        "got:\n{text}"
    );
    assert!(
        !text.contains("Selected"),
        "a session that already had a model must not be retargeted:\n{text}"
    );
}

/// **TUI-105.** Pi's `updateAvailableProviderCount` (`interactive-mode.ts:5092-5098`), called
/// unconditionally at the end of `finishAuthentication` (`:5928`).
///
/// **Red without the change:** `StatusLine::set_provider_count` had exactly ONE production caller
/// and it is at boot (`cyrup/src/interactive.rs:466-469`), so the footer's `(provider)` prefix gate
/// (`status.rs:597`) could not move after startup — logging in to a second provider left the footer
/// claiming one.
#[tokio::test]
async fn tui105_login_recounts_providers_from_the_auth_filtered_catalog() {
    let fx = first_run_fixture().await;
    credential(&fx.session, DEFAULTED_PROVIDER);
    // A second credentialed provider, so the auth-filtered set is wider than the pre-login count of
    // one by construction rather than by whatever credentials the host happens to export.
    credential(&fx.session, SECOND_PROVIDER);
    let mut app = app_with(registry_for(
        DEFAULTED_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();
    app.status_mut().set_provider_count(1);

    let (_action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DEFAULTED_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;

    // `new Set(getAvailableSnapshot().map(m => m.provider)).size` (`:5096-5097`).
    let expected: std::collections::BTreeSet<String> = fx
        .session
        .available_model_catalog()
        .iter()
        .map(|m| m.provider.as_str().to_string())
        .collect();
    assert!(
        expected.len() > 1,
        "the credentialed provider must have widened the catalog: {expected:?}"
    );
    assert_eq!(app.state().status.provider_count, expected.len());

    // `session.scopedModels.length > 0 ? scopedModels.map(s => s.model) : …` (`:5094-5095`) — the
    // scoped set WINS when one is configured.
    let one = fx
        .session
        .available_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == DEFAULTED_PROVIDER)
        .expect("the credentialed provider has models");
    fx.session
        .set_scoped_models(vec![cyrup_session_svc::ScopedModel {
            model: one,
            thinking_level: None,
        }]);
    app.refresh_provider_count(&fx.session);
    assert_eq!(
        app.state().status.provider_count,
        1,
        "one scoped provider ⇒ one"
    );
}

/// **TUI-105.** `deferSelection` (`interactive-mode.ts:5889-5895`), whose comment is "Dynamic
/// catalogs may be empty until the first authenticated network refresh": the credential exists but
/// the provider's models do not yet, so pi postpones the selection, says
/// `` `… Refreshing model catalog…` `` (`:5948`), and runs `finishAuthentication` from the refresh
/// continuation instead (`:5962`).
///
/// **Red without the change:** neither the deferred status string nor `LoginRefreshMsg` exists, and
/// `finish_login` never selected a model on either route.
#[tokio::test]
async fn tui105_deferred_selection_reports_refreshing_then_selects() {
    let fx = first_run_fixture().await;
    let mut app = app_with(registry_for(
        DYNAMIC_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    let (action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DYNAMIC_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;

    let path = fx.agent_dir.join("auth.json").display().to_string();
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!(
            "{action}. Credentials saved to {path}. Refreshing model catalog…"
        )),
        "`:5948` verbatim; got:\n{text}"
    );
    assert!(
        !text.contains("Selected"),
        "the selection is postponed until the refresh lands:\n{text}"
    );
    assert_eq!(fx.session.model(), None);

    // The refresh lands, carrying the provider's account-specific catalog.
    install_refreshed_catalog(&fx.session, DYNAMIC_PROVIDER, &[DYNAMIC_MODEL]);
    let refresh = tokio::time::timeout(Duration::from_secs(5), refresh_rx.recv())
        .await
        .expect("the post-login refresh must settle")
        .expect("the refresh channel stays open");
    assert!(refresh.defer, "`deferSelection` travels on the message");
    app.apply_login_refresh(&fx.session, refresh).await;

    let model = fx
        .session
        .model()
        .expect("the deferred selection must run from the refresh continuation");
    assert_eq!(model.provider.as_str(), DYNAMIC_PROVIDER);
    assert_eq!(model.model.as_str(), DYNAMIC_MODEL);
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!(
            "{action}. Selected {DYNAMIC_MODEL}. Credentials saved to {path}"
        )),
        "got:\n{text}"
    );
}

/// **TUI-105.** Pi's `if (deferSelection && this.session === session && session.model === previousModel)`
/// guard (`interactive-mode.ts:5960-5963`), comment included: "Do not replace a model or session
/// selected while the refresh was running."
///
/// This is the one clause whose absence is a WRONG RESULT rather than a missing feature: a `/model`
/// issued during the 15 s refresh window would be silently overwritten by the provider default. A fix
/// without it passes the two tests above and is still broken, which is why this one exists.
#[tokio::test]
async fn tui105_a_stale_refresh_does_not_replace_a_model_chosen_meanwhile() {
    let fx = first_run_fixture().await;
    let mut app = app_with(registry_for(
        DYNAMIC_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    let (_action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DYNAMIC_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;
    assert_eq!(fx.session.model(), None, "the selection was deferred");

    // The user runs `/model` while the refresh is in flight and picks something else.
    install_refreshed_catalog(
        &fx.session,
        DYNAMIC_PROVIDER,
        &[DYNAMIC_MODEL, "something-else"],
    );
    let chosen = fx
        .session
        .available_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == DYNAMIC_PROVIDER && m.id.as_str() == "something-else")
        .expect("the refreshed catalog has a second model");
    fx.session.set_model_resolved(chosen).await.unwrap();

    let refresh = tokio::time::timeout(Duration::from_secs(5), refresh_rx.recv())
        .await
        .expect("the post-login refresh must settle")
        .expect("the refresh channel stays open");
    assert!(refresh.defer);
    app.apply_login_refresh(&fx.session, refresh).await;

    assert_eq!(
        fx.session
            .model()
            .expect("the user's choice stands")
            .model
            .as_str(),
        "something-else",
        "`session.model === previousModel` is false, so the deferred selection is abandoned"
    );
    assert!(
        !transcript_text(&app).contains(&format!("Selected {DYNAMIC_MODEL}")),
        "no selection status for a selection that did not happen"
    );
}

/// **TUI-105.** `if (result.aborted) showWarning(...)` (`interactive-mode.ts:5956-5958`).
///
/// cyrup's coordinator splits pi's one `aborted` flag into `timed_out` (THIS caller's token fired)
/// and `aborted` (the shared operation gave up) — `cyrup-provider/src/catalog_refresh.rs:90-95` —
/// and both are upstream's, so both take this arm.
#[tokio::test]
async fn tui105_refresh_timeout_warns_and_keeps_cached_models() {
    let fx = first_run_fixture().await;
    credential(&fx.session, DEFAULTED_PROVIDER);
    let mut app = app_with(registry_for(
        DEFAULTED_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    let (action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DEFAULTED_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;
    // The catalog was already available, so this login selected inline and did NOT defer.
    let selected = fx.session.model().expect("selected inline");

    let mut refresh = tokio::time::timeout(Duration::from_secs(5), refresh_rx.recv())
        .await
        .expect("the post-login refresh must settle")
        .expect("the refresh channel stays open");
    assert!(!refresh.defer);
    refresh.result.timed_out = true;
    app.apply_login_refresh(&fx.session, refresh).await;

    let text = transcript_text(&app);
    assert!(
        text.contains(&format!(
            "Warning: {action}, but its model catalog refresh timed out; using cached models."
        )),
        "`:5956` verbatim, through `showWarning`'s prefix (TUI-062); got:\n{text}"
    );
    assert_eq!(
        fx.session.model(),
        Some(selected),
        "a timed-out refresh leaves the already-selected model alone"
    );
}

/// **TUI-105.** Pi's `selectionError` ladder (`interactive-mode.ts:5901-5923`), every rung and every
/// message byte-for-byte, including the `radius` catalog-order fallback (`:5913-5915`).
///
/// Asserted on the pure decision rather than end to end because three of the four rungs need a
/// catalog shape the offline fixture cannot produce — a provider that is credentialed, has models,
/// and is missing its own curated default. That is the same reason `clipboard_write_plan` takes its
/// platform as a parameter.
///
/// **Red without the change:** `default_model_selection` and `DefaultModelFailure` do not exist.
#[test]
fn tui105_login_reports_the_missing_default_by_name() {
    use crate::app::login::{DefaultModelFailure, default_model_selection};

    let action = "Logged in to Deepseek";

    // Rung 2 (after llama.cpp's, pinned by `tui105_llama_cpp_login_ends_in_guidance_not_a_default`): `!hasDefaultModelProvider(providerId)` (`:5904-5905`). `"stub"` is absent from
    // `default_model_per_provider`, which is why the other tests log in to `deepseek`.
    assert_eq!(
        default_model_selection("stub", &[]),
        Err(DefaultModelFailure::NoDefaultConfigured)
    );
    assert_eq!(
        DefaultModelFailure::NoDefaultConfigured.message(action, "stub"),
        "Logged in to Deepseek, but no default model is configured for provider \"stub\". \
         Use /model to select a model."
    );

    // Rung 3: `providerModels.length === 0` (`:5906-5907`).
    assert_eq!(
        default_model_selection(DEFAULTED_PROVIDER, &[]),
        Err(DefaultModelFailure::NoModelsAvailable)
    );
    assert_eq!(
        DefaultModelFailure::NoModelsAvailable.message(action, DEFAULTED_PROVIDER),
        "Logged in to Deepseek, but no models are available for that provider. \
         Use /model to select a model."
    );

    // Rung 4: the default is configured but absent from the catalog (`:5915-5916`).
    let other = model_named(DEFAULTED_PROVIDER, "deepseek-v4-flash");
    assert_eq!(
        default_model_selection(DEFAULTED_PROVIDER, std::slice::from_ref(&other)),
        Err(DefaultModelFailure::DefaultNotAvailable(DEFAULTED_MODEL))
    );
    assert_eq!(
        DefaultModelFailure::DefaultNotAvailable(DEFAULTED_MODEL)
            .message(action, DEFAULTED_PROVIDER),
        "Logged in to Deepseek, but its default model \"deepseek-v4-pro\" is not available. \
         Use /model to select a model."
    );

    // …and the hit, which is the whole point of the ladder.
    let want = model_named(DEFAULTED_PROVIDER, DEFAULTED_MODEL);
    assert_eq!(
        default_model_selection(DEFAULTED_PROVIDER, &[other.clone(), want.clone()])
            .unwrap()
            .id
            .as_str(),
        DEFAULTED_MODEL
    );

    // `providerId === "radius" ? providerModels[0] : undefined` (`:5914`) — "Radius catalogs vary by
    // account; prefer balanced, then use catalog order" (`:5912`). ONLY radius gets it.
    let radius_first = model_named("radius", "some-account-model");
    assert_eq!(
        default_model_selection("radius", std::slice::from_ref(&radius_first))
            .unwrap()
            .id
            .as_str(),
        "some-account-model"
    );

    // Rung 5: `setModel` threw (`:5919-5923`).
    assert_eq!(
        DefaultModelFailure::SetModelFailed(
            "no configured auth for deepseek/deepseek-v4-pro".into()
        )
        .message(action, DEFAULTED_PROVIDER),
        "Logged in to Deepseek, but selecting its default model failed: \
         no configured auth for deepseek/deepseek-v4-pro. Use /model to select a model."
    );
}

/// A bare [`Model`] for the pure ladder test — only `provider` and `id` are read
/// (`interactive-mode.ts:5900`, `:5913`).
fn model_named(provider: &str, id: &str) -> Model {
    // The faux provider's own model as a template — every other field is untouched and unread.
    let mut model = FauxProvider::new().model().clone();
    model.provider = ProviderId::from(provider);
    model.id = cyrup_core::ModelId::from(id);
    model
}

/// **TUI-105, the documentation half.** Every intra-doc link `app/login.rs` writes must RESOLVE: the
/// workspace denies `rustdoc::broken_intra_doc_links`, so one bare name turns
/// `cargo doc -p cyrup-tui --no-deps` into `error: could not document `cyrup-tui``. The original
/// offender was a bare ``[`LoginRefreshMsg::previous_model`]`` — the type is
/// `crate::login_dialog::LoginRefreshMsg` (`login_dialog.rs:623`) and `app::login` imports no such
/// name.
///
/// Read out of the source because rustdoc, not the binary, is what consumes the link.
///
/// The second assertion moved with the prose it guards. It used to pin the `CYRUP-DELTA` that
/// recorded the post-login refresh being WIDER than pi's; that delta is now DELETED, because the
/// refresh was narrowed to upstream's own scope instead (pi's direct
/// `modelRuntime.refresh({ providers: [providerId], signal })`, `interactive-mode.ts:5953`) and a
/// `CYRUP-DELTA` may only record a mechanism difference at full parity, never a lost guarantee. What
/// replaced it is the port note naming the scoped call, so that is what is pinned by its resolvable
/// path.
///
/// **Red without the change:** a bare `[`LoginRefreshMsg::` spelling is back, or the port note stops
/// naming the scoped refresh by a path rustdoc can follow.
#[test]
fn tui105_the_post_login_refresh_doc_resolves_its_links() {
    const LOGIN_SRC: &str = include_str!("../app/login.rs");

    assert!(
        !LOGIN_SRC.contains("[`LoginRefreshMsg::"),
        "app/login.rs links `LoginRefreshMsg` by a bare name it never imports; the type is \
         `crate::login_dialog::LoginRefreshMsg`, and the workspace denies \
         `rustdoc::broken_intra_doc_links`, so this one link makes \
         `cargo doc -p cyrup-tui --no-deps` fail with `could not document `cyrup-tui``"
    );
    assert!(
        LOGIN_SRC.contains("[`cyrup_session_svc::AgentSession::refresh_provider_catalog`]"),
        "the post-login refresh's doc must name the SCOPED session call it now makes — pi's \
         `refresh({{ providers: [providerId], signal }})` (`interactive-mode.ts:5953`) — by a path \
         rustdoc can resolve"
    );
    assert!(
        !LOGIN_SRC.contains("CYRUP-DELTA` pi scopes the refresh"),
        "the WIDENED-refresh `CYRUP-DELTA` recorded a lost guarantee, not a mechanism at parity; it \
         must stay deleted now that the refresh is scoped like upstream's"
    );
}

// ================================================ TUI-105 — the post-login refresh is SCOPED

/// An `AuthContext` over an EMPTY environment, so the catalog fetch below cannot be rerouted by an
/// ambient `HTTP_PROXY` on the machine running the suite.
struct EmptyEnv;

#[async_trait::async_trait]
impl cyrup_provider::AuthContext for EmptyEnv {
    async fn env(&self, _name: &str) -> Option<String> {
        None
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

/// A loopback catalog origin that records which providers were fetched and answers each immediately.
///
/// **No network.** `127.0.0.1:0`, the technique `cyrup-provider/src/tests/remote_catalog.rs`
/// established; a test that "passed" by reaching `https://pi.dev` would be worse than no test.
struct RecordingOrigin {
    base_url: String,
    requested: Arc<std::sync::Mutex<Vec<String>>>,
}

impl RecordingOrigin {
    async fn spawn() -> Self {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let requested = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen = Arc::clone(&requested);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let seen = Arc::clone(&seen);
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    // `GET /api/models/providers/<id> HTTP/1.1`
                    if let Some(provider) = head
                        .split_whitespace()
                        .nth(1)
                        .and_then(|p| p.rsplit('/').next())
                    {
                        seen.lock().unwrap().push(provider.to_string());
                    }
                    let body = "[]";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                });
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            requested,
        }
    }

    fn fetched(&self) -> std::collections::BTreeSet<String> {
        self.requested.lock().unwrap().iter().cloned().collect()
    }
}

/// [`first_run_fixture`] with a real [`cyrup_provider::ModelCatalogService`] wired at `base_url`.
///
/// The plain fixture wires `model_catalog: None`, which makes every post-login refresh a clean no-op
/// — fine for the selection tests above, useless for proving WHICH providers get fetched. This one
/// gives the session something to fetch from.
async fn first_run_fixture_with_catalog(base_url: &str) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.trust_override = Some(true);
    let catalog = Arc::new(
        cyrup_provider::RemoteCatalog::new(Arc::new(cyrup_provider::InMemoryModelsStore::new()))
            .with_base_url(base_url)
            .with_auth_context(Arc::new(EmptyEnv))
            .with_request_timeout(Duration::from_secs(30)),
    );
    let svc = Arc::new(
        cyrup_provider::ModelCatalogService::new(
            catalog,
            Arc::new(cyrup_provider::CatalogOverlaySlot::new()),
        )
        .with_overlay_providers(
            cyrup_provider::all_providers()
                .iter()
                .map(|p| p.id().as_str().to_string())
                .collect(),
        )
        .with_options(cyrup_provider::RefreshOptions {
            allow_network: true,
            force: true,
        }),
    );
    let provider: Arc<dyn Provider> = Arc::new(ModellessProvider(ProviderId::from("none")));
    let session = SessionBuilder::new(provider, config)
        .provider_resolver(Arc::new(RegistryResolver))
        .model_catalog_service(svc)
        .build()
        .await
        .unwrap();
    Fixture {
        _tmp: tmp,
        agent_dir,
        session: Arc::new(session),
    }
}

/// **TUI-105, end to end.** The post-login catalog refresh fetches ONLY the provider that was just
/// logged into, even though another provider is credentialed and the whole-catalog path would sweep
/// both.
///
/// This is pi's `completeProviderAuthentication` tail: `session.modelRuntime.refresh({ providers:
/// [providerId], signal: controller.signal })` (`interactive-mode.ts:5953`), called DIRECTLY rather
/// than through `refreshModelCatalogs` (`model-catalog-refresh.ts:46-51`). The unit tests in
/// `cyrup-provider` and `cyrup-session-svc` pin the API; this one pins the LOGIN PATH, which is what
/// the row was filed about — when the shared budget was eaten by a stranger, `apply_login_refresh`
/// never re-ran the deferred selection and the user was left modelless.
///
/// Uses [`DEFAULTED_PROVIDER`] rather than the deferred [`DYNAMIC_PROVIDER`] deliberately: `radius`
/// is excluded from the FETCH list by design (the pi.dev route 404s for it, so its scoped fetch set
/// is empty), which would make "only the provider logged into" vacuous here. The radius exclusion has
/// its own test in `cyrup-session-svc`.
///
/// **Red without the change:** `begin_post_login_catalog_refresh` called
/// `session.refresh_model_catalogs(cancel)`, whose provider list is every credentialed provider, so
/// the recorded set also contains the bystander.
#[tokio::test]
async fn tui105_post_login_refresh_fetches_only_the_provider_logged_into() {
    const BYSTANDER: &str = "groq";

    let origin = RecordingOrigin::spawn().await;
    let fx = first_run_fixture_with_catalog(&origin.base_url).await;
    // A second provider the user logged in to earlier. The whole-catalog refresh would sweep it.
    credential(&fx.session, BYSTANDER);
    credential(&fx.session, DEFAULTED_PROVIDER);

    let mut app = app_with(registry_for(
        DEFAULTED_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    let (_action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DEFAULTED_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;

    let refresh = tokio::time::timeout(Duration::from_secs(10), refresh_rx.recv())
        .await
        .expect("the post-login refresh must settle")
        .expect("the refresh channel stays open");
    assert_eq!(
        refresh.provider_id, DEFAULTED_PROVIDER,
        "the settled message names the provider that was logged into"
    );

    let fetched = origin.fetched();
    assert!(
        fetched.contains(DEFAULTED_PROVIDER),
        "the provider just authenticated must be fetched, saw {fetched:?}"
    );
    assert!(
        !fetched.contains(BYSTANDER),
        "pi spends the WHOLE budget on the provider it just authenticated (`:5953`); fetching \
         `{BYSTANDER}` too is the widening this row removed, saw {fetched:?}"
    );
    assert_eq!(
        fetched.len(),
        1,
        "exactly one provider is fetched by a scoped post-login refresh, saw {fetched:?}"
    );
}

// ================================================================ EXT-027 (H3): extension providers

// An extension's LIVE provider is a first-class `/login` target. pi's `getLoginProviderOptions`
// reads the ONE composed provider list (`this.session.modelRuntime.getProviders()`,
// `interactive-mode.ts:4943-4947`), which holds a native extension's provider beside the built-ins
// (`nativeExtensionProviders.get(id)`, `core/model-runtime.ts:298` @v0.99.2-17); cyrup keeps the two
// apart, so `build_login_inputs` joins them. The strategy below is shaped like pi's llama.cpp one
// (`extensions/llama/provider.ts:156-196`): a two-prompt login (URL, optional key) that returns
// `{ type: "api_key", key, env: { LLAMA_BASE_URL } }`, and a `check` active only when a URL is known.
//
// No network: the login flow does not call the server (pi's does, `provider.ts:174`; that probe is
// the real strategy's, not what is under test here).

const LLAMA_ID: &str = "llama.cpp";
const LLAMA_URL_ENV: &str = "LLAMA_BASE_URL";
const LLAMA_URL: &str = "http://127.0.0.1:9";

async fn llama_server_url(
    ctx: &dyn cyrup_provider::AuthContext,
    cred: Option<&Credential>,
) -> Option<String> {
    let stored = cred
        .and_then(Credential::env)
        .and_then(|env| env.get(LLAMA_URL_ENV))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match stored {
        Some(url) => Some(url),
        None => ctx.env(LLAMA_URL_ENV).await.map(|v| v.trim().to_string()),
    }
}

struct LlamaLikeAuth;

#[async_trait::async_trait]
impl ApiKeyAuth for LlamaLikeAuth {
    fn name(&self) -> &str {
        "llama.cpp server"
    }
    fn supports_login(&self) -> bool {
        true
    }
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let entered = interaction
            .prompt(AuthPrompt::text("llama.cpp server URL"))
            .await?;
        let key = interaction
            .prompt(AuthPrompt::secret("API key (optional)"))
            .await?;
        let key = Some(key.trim().to_string()).filter(|k| !k.is_empty());
        let mut env = std::collections::BTreeMap::new();
        env.insert(LLAMA_URL_ENV.to_string(), entered.trim().to_string());
        Ok(Credential::ApiKey {
            key,
            env: Some(env),
        })
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        ctx: &dyn cyrup_provider::AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<cyrup_provider::collection::AuthCheck>, ProviderAuthError> {
        Ok(llama_server_url(ctx, cred)
            .await
            .map(|_| cyrup_provider::collection::AuthCheck {
                auth_type: cyrup_provider::collection::AuthType::ApiKey,
                source: Some(if cred.is_some() {
                    "stored credential".to_string()
                } else {
                    LLAMA_URL_ENV.to_string()
                }),
            }))
    }
    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn cyrup_provider::AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<cyrup_provider::AuthResult>, ProviderAuthError> {
        Ok(llama_server_url(ctx, cred)
            .await
            .map(|url| cyrup_provider::AuthResult::from_key("local", url)))
    }
}

/// A native whose `init` registers one LIVE provider under `id` with the llama-like strategy.
struct LiveProviderNative {
    id: &'static str,
    provider: Arc<dyn Provider>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for LiveProviderNative {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("live-provider-ext")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.register_provider_live(self.id, Arc::clone(&self.provider));
        Ok(())
    }
    async fn on_event(
        &self,
        _ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        cyrup_ext::HookOutcome::Noop
    }
}

fn llama_like_provider(id: &'static str, display: &str) -> Arc<dyn Provider> {
    llama_like_provider_with(id, display, vec![model_named(id, "tiny")])
}

/// [`llama_like_provider`] over an explicit model list — the llama.cpp post-login guidance counts
/// the provider's models (`interactive-mode.ts:5958`), so its tests need both zero and several.
fn llama_like_provider_with(
    id: &'static str,
    display: &str,
    models: Vec<Model>,
) -> Arc<dyn Provider> {
    struct Named {
        inner: StubProvider,
        display: String,
    }
    #[async_trait::async_trait]
    impl Provider for Named {
        fn id(&self) -> &ProviderId {
            self.inner.id()
        }
        fn name(&self) -> &str {
            &self.display
        }
        fn models(&self) -> &[Model] {
            self.inner.models()
        }
        fn provider_auth(&self) -> Option<&ProviderAuth> {
            self.inner.provider_auth()
        }
        fn stream(
            &self,
            model: &Model,
            context: &Context,
            options: &StreamOptions,
        ) -> EventStream<StreamEvent> {
            self.inner.stream(model, context, options)
        }
    }
    Arc::new(Named {
        inner: StubProvider {
            id: ProviderId::from(id),
            auth: ProviderAuth::with_api_key(Arc::new(LlamaLikeAuth)),
            models,
        },
        display: display.to_string(),
    })
}

/// A session with `provider` registered LIVE under `id`, over a credential store whose ambient
/// environment is `env` and nothing else.
async fn live_provider_fixture(
    id: &'static str,
    provider: Arc<dyn Provider>,
    env: &[(&str, &str)],
) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.trust_override = Some(true);
    config.no_extensions = true;
    let ambient: std::collections::HashMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let auth = Arc::new(
        cyrup_config::AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(ambient),
    );
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, config)
        .auth(auth)
        .with_native_extension(
            Arc::new(LiveProviderNative { id, provider }) as Arc<dyn cyrup_ext::NativeExtension>
        )
        .build()
        .await
        .unwrap();
    Fixture {
        _tmp: tmp,
        agent_dir,
        session: Arc::new(session),
    }
}

/// An empty built-in registry, so the live provider is the only row and a flow cannot be confused by
/// a built-in of the same name.
fn no_builtin_providers() -> LoginProviderSource {
    Arc::new(Vec::new)
}

/// **The picker half.** A live provider carrying an api-key strategy with a `login` appears in the
/// `/login` inputs and options beside the built-ins, with its OWN display name and the normal
/// two-prompt api-key flow; a static JSON-registered guest provider does NOT (it has no login of its
/// own, and its key is baked at registration).
///
/// **Red without the change:** `build_login_inputs` iterates `all_providers()` alone, so there is no
/// `llama.cpp` row at all.
#[tokio::test]
async fn a_live_extension_provider_appears_in_the_login_picker() {
    let fx = live_provider_fixture(LLAMA_ID, llama_like_provider(LLAMA_ID, "llama.cpp"), &[]).await;
    fx.session
        .services()
        .ext_host
        .registry()
        .register_provider(
            cyrup_core::ExtensionId::from("acme-ext"),
            "acme",
            serde_json::json!({
                "name": "Acme",
                "baseUrl": "http://127.0.0.1:1/v1",
                "api": "openai-completions",
                "models": [{ "id": "acme-fast" }],
            }),
        )
        .unwrap();
    // The DEFAULT registry: the built-ins are there too.
    let app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

    let inputs = app.login_provider_inputs(&fx.session).await;

    let row = inputs
        .iter()
        .find(|p| p.id.as_str() == LLAMA_ID)
        .expect("the live provider has a /login row");
    assert_eq!(row.name, "llama.cpp", "the provider's own display name");
    assert!(!row.status.configured, "nothing is configured yet");
    assert!(
        inputs.iter().any(|p| p.id.as_str() == "anthropic"),
        "the built-ins are still listed"
    );
    assert!(
        inputs.iter().all(|p| p.id.as_str() != "acme"),
        "a static JSON registration is not a /login target"
    );

    let options = cyrup_config::login::login_provider_options(&inputs, None);
    let option = options
        .iter()
        .find(|o| o.id.as_str() == LLAMA_ID)
        .expect("the live provider has a picker option");
    assert_eq!(option.auth_type, AuthType::ApiKey);
    assert!(
        option.supports_login,
        "the strategy has a `login`, so the normal prompt flow runs rather than the ambient dialog"
    );
    assert_eq!(option.method_name.as_deref(), Some("llama.cpp server"));
}

/// A live provider registered under a BUILT-IN id replaces that row (pi's registry holds one
/// provider per id and `registerProvider` "replaces all models"), so `/login anthropic` runs the
/// extension's strategy rather than listing the id twice.
#[tokio::test]
async fn a_live_provider_replaces_the_builtin_row_of_the_same_id() {
    let fx = live_provider_fixture(
        "anthropic",
        llama_like_provider("anthropic", "Proxy Anthropic"),
        &[],
    )
    .await;
    let app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

    let inputs = app.login_provider_inputs(&fx.session).await;

    let rows: Vec<_> = inputs
        .iter()
        .filter(|p| p.id.as_str() == "anthropic")
        .collect();
    assert_eq!(rows.len(), 1, "one row per provider id");
    assert_eq!(rows[0].name, "Proxy Anthropic");
    assert!(
        rows[0].auth.oauth.is_none(),
        "the extension's strategy replaced the built-in's subscription login"
    );
}

/// **The login half.** `/login llama.cpp` runs the strategy's own two-prompt flow with the scripted
/// answers, stores `Credential::ApiKey { key, env: { LLAMA_BASE_URL } }` under the provider's id, and
/// the credential makes the provider's models available — the end-to-end of the whole lane.
///
/// **Red without the change:** the picker has no such provider, so `/login llama.cpp` opens the
/// provider selector and no flow ever runs.
#[tokio::test]
async fn logging_in_to_a_live_provider_stores_the_api_key_and_env_credential() {
    let fx = live_provider_fixture(LLAMA_ID, llama_like_provider(LLAMA_ID, "llama.cpp"), &[]).await;
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();
    assert!(
        fx.session
            .available_model_catalog()
            .iter()
            .all(|m| m.provider.as_str() != LLAMA_ID),
        "unavailable before the login"
    );

    app.execute_command(
        AppCommand::LoginCommand(Some(LLAMA_ID.to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginDialog),
        "`/login llama.cpp` starts the flow directly"
    );

    // Prompt 1: the server URL.
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "got {msg:?}");
    app.apply_login_msg(&fx.session, msg).await;
    assert!(
        app.login_dialog_body()
            .unwrap()
            .contains("llama.cpp server URL"),
        "{}",
        app.login_dialog_body().unwrap()
    );
    type_text(&mut app, LLAMA_URL);
    app.handle_input(&key(KeyCode::Enter));

    // Prompt 2: the optional key.
    let msg = next_msg(&mut rx).await;
    assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "got {msg:?}");
    app.apply_login_msg(&fx.session, msg).await;
    assert!(
        app.login_dialog_body()
            .unwrap()
            .contains("API key (optional)"),
        "{}",
        app.login_dialog_body().unwrap()
    );
    type_text(&mut app, "sk-llama");
    app.handle_input(&key(KeyCode::Enter));

    let finished = next_msg(&mut rx).await;
    match &finished {
        LoginUiMsg::Finished(f) => assert!(f.result.is_ok(), "login failed: {:?}", f.result),
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(&fx.session, finished).await;

    match fx
        .session
        .services()
        .auth
        .read(&ProviderId::from(LLAMA_ID))
        .await
        .unwrap()
        .expect("credential persisted")
    {
        cyrup_config::auth::Credential::ApiKey { key, env } => {
            assert_eq!(key.as_deref(), Some("sk-llama"));
            assert_eq!(
                env.as_ref()
                    .and_then(|e| e.get(LLAMA_URL_ENV))
                    .map(String::as_str),
                Some(LLAMA_URL),
                "the server URL rides in the credential's env, which is how it reaches the provider"
            );
        }
        other => panic!("expected an api-key credential, got {other:?}"),
    }
    assert!(
        fx.session
            .available_model_catalog()
            .iter()
            .any(|m| m.provider.as_str() == LLAMA_ID),
        "the stored credential makes the provider's models available"
    );
    let text = transcript_text(&app);
    assert!(text.contains("Saved API key for llama.cpp"), "{text}");
}

/// The picker's status line for a live provider is its strategy's `check`
/// (`getProviderAuthStatus`, `core/model-runtime.ts:428-437`): unconfigured with nothing, the
/// environment arm (labelled with the check's source) when only `LLAMA_BASE_URL` is set, and
/// `stored` once a credential is on disk. The `env_keys` table knows nothing of this provider id, so
/// without the strategy the environment case could never read as configured.
#[tokio::test]
async fn a_live_providers_login_status_is_its_strategys_check() {
    let app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

    let bare =
        live_provider_fixture(LLAMA_ID, llama_like_provider(LLAMA_ID, "llama.cpp"), &[]).await;
    let inputs = app.login_provider_inputs(&bare.session).await;
    let status = &inputs
        .iter()
        .find(|p| p.id.as_str() == LLAMA_ID)
        .unwrap()
        .status;
    assert!(!status.configured);

    let env = live_provider_fixture(
        LLAMA_ID,
        llama_like_provider(LLAMA_ID, "llama.cpp"),
        &[(LLAMA_URL_ENV, LLAMA_URL)],
    )
    .await;
    let inputs = app.login_provider_inputs(&env.session).await;
    let status = &inputs
        .iter()
        .find(|p| p.id.as_str() == LLAMA_ID)
        .unwrap()
        .status;
    assert!(status.configured);
    assert_eq!(
        status.source,
        Some(cyrup_config::auth::AuthSource::Environment)
    );
    assert_eq!(status.label.as_deref(), Some(LLAMA_URL_ENV));

    env.session
        .services()
        .auth
        .modify(&ProviderId::from(LLAMA_ID), |_| async {
            Ok(Some(cyrup_config::auth::Credential::api_key("sk")))
        })
        .await
        .unwrap();
    let inputs = app.login_provider_inputs(&env.session).await;
    let status = &inputs
        .iter()
        .find(|p| p.id.as_str() == LLAMA_ID)
        .unwrap()
        .status;
    assert_eq!(status.source, Some(cyrup_config::auth::AuthSource::Stored));
}

/// `isUsingOAuth(id)` (`interactive-mode.ts:4863`) is the STORED credential's kind, for a live
/// provider as for a built-in: an OAuth credential stored under the live provider's id marks its row
/// `using_oauth` (and the picker option's status `oauth`), while an api-key credential does not.
///
/// **Red without the change:** a live row hard-coded to `using_oauth: false` reports the stored OAuth
/// login as an api key.
#[tokio::test]
async fn a_live_providers_row_reports_a_stored_oauth_credential_as_oauth() {
    let app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let fx = live_provider_fixture(LLAMA_ID, llama_like_provider(LLAMA_ID, "llama.cpp"), &[]).await;
    let option_status_kind = |inputs: &[cyrup_config::login::ProviderLoginInput]| {
        cyrup_config::login::login_provider_options(inputs, None)
            .into_iter()
            .find(|o| o.id.as_str() == LLAMA_ID)
            .and_then(|o| o.status)
            .map(|s| s.auth_type)
    };

    fx.session
        .services()
        .auth
        .modify(&ProviderId::from(LLAMA_ID), |_| async {
            Ok(Some(cyrup_config::auth::Credential::api_key("sk")))
        })
        .await
        .unwrap();
    let inputs = app.login_provider_inputs(&fx.session).await;
    let row = inputs.iter().find(|p| p.id.as_str() == LLAMA_ID).unwrap();
    assert!(!row.using_oauth, "an api-key credential is not oauth");
    assert_eq!(option_status_kind(&inputs), Some(AuthType::ApiKey));

    fx.session
        .services()
        .auth
        .modify(&ProviderId::from(LLAMA_ID), |_| async {
            Ok(Some(cyrup_config::auth::Credential::Oauth {
                refresh: "rt".to_string(),
                access: "at".to_string(),
                expires: i64::MAX,
                ext: Default::default(),
            }))
        })
        .await
        .unwrap();
    let inputs = app.login_provider_inputs(&fx.session).await;
    let row = inputs.iter().find(|p| p.id.as_str() == LLAMA_ID).unwrap();
    assert!(row.using_oauth, "a stored oauth credential marks the row");
    assert_eq!(option_status_kind(&inputs), Some(AuthType::Oauth));
}

// ================================================ EXT-027 — llama.cpp post-login guidance
//
// `llamaCppPostLoginGuidance` (`interactive-mode.ts:349-353`), reached from `finishAuthentication`
// as its FIRST rung (`:5957-5958`) and only when the session had no model (`isUnknownModel`,
// `:5953`).

/// Both guidance templates byte-for-byte, the rung's priority over every other rung, and the
/// provider gate: only the exact id `llama.cpp` takes it.
///
/// **Red without the change:** `DefaultModelFailure::LlamaCppGuidance` does not exist, and without
/// the first rung `llama.cpp` (absent from `default_model_per_provider`) falls to
/// `NoDefaultConfigured` / `NoModelsAvailable`.
#[test]
fn tui105_llama_cpp_login_ends_in_guidance_not_a_default() {
    use crate::app::login::{DefaultModelFailure, default_model_selection};

    let action = "Saved API key for llama.cpp";

    // `loadedModelCount === 0` (`:350-351`).
    assert_eq!(
        default_model_selection(LLAMA_ID, &[]),
        Err(DefaultModelFailure::LlamaCppGuidance(0)),
        "FIRST rung: ahead of both `hasDefaultModelProvider` (`:5959`) and the empty-catalog rung"
    );
    assert_eq!(
        DefaultModelFailure::LlamaCppGuidance(0).message(action, LLAMA_ID),
        "Saved API key for llama.cpp. No llama.cpp models are loaded. \
         Use /llama to load a model, then /model to select it."
    );

    // Otherwise (`:352`): the count is `providerModels.length`, any positive number.
    let two = [model_named(LLAMA_ID, "a"), model_named(LLAMA_ID, "b")];
    assert_eq!(
        default_model_selection(LLAMA_ID, &two),
        Err(DefaultModelFailure::LlamaCppGuidance(2)),
        "loaded models do NOT make the login select one: pi never picks a llama.cpp default"
    );
    for count in [1, 2, 40] {
        assert_eq!(
            DefaultModelFailure::LlamaCppGuidance(count).message(action, LLAMA_ID),
            "Saved API key for llama.cpp. Use /model to select a loaded llama.cpp model, \
             or /llama to manage models."
        );
    }

    // Other providers are unaffected: the gate is `providerId === "llama.cpp"` exactly.
    assert_eq!(
        default_model_selection("stub", &[]),
        Err(DefaultModelFailure::NoDefaultConfigured)
    );
    assert_eq!(
        default_model_selection("llama.cpp-2", &two),
        Err(DefaultModelFailure::NoDefaultConfigured),
        "a different id, even one that starts with `llama.cpp`, is not the llama.cpp provider"
    );
    assert_eq!(
        default_model_selection(DEFAULTED_PROVIDER, &[]),
        Err(DefaultModelFailure::NoModelsAvailable)
    );
    let want = model_named(DEFAULTED_PROVIDER, DEFAULTED_MODEL);
    assert_eq!(
        default_model_selection(DEFAULTED_PROVIDER, std::slice::from_ref(&want))
            .unwrap()
            .id
            .as_str(),
        DEFAULTED_MODEL
    );
}

/// A credential-less first run (`session.model() == None`, pi's `isUnknownModel`) with `models`
/// registered LIVE under `llama.cpp`, the same way the `cyrup-llama` extension does.
async fn first_run_llama_fixture(models: Vec<Model>) -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.trust_override = Some(true);
    config.no_extensions = true;
    let provider: Arc<dyn Provider> = Arc::new(ModellessProvider(ProviderId::from("none")));
    let session = SessionBuilder::new(provider, config)
        .auth(hermetic_auth(&agent_dir))
        .with_native_extension(Arc::new(LiveProviderNative {
            id: LLAMA_ID,
            provider: llama_like_provider_with(LLAMA_ID, "llama.cpp", models),
        }) as Arc<dyn cyrup_ext::NativeExtension>)
        .build()
        .await
        .unwrap();
    assert_eq!(session.model(), None, "pi's isUnknownModel state");
    Fixture {
        _tmp: tmp,
        agent_dir,
        session: Arc::new(session),
    }
}

/// Run `/login llama.cpp` through its two prompts and apply the `Finished`.
async fn login_to_llama(
    app: &mut App<TestBackend>,
    session: &Arc<AgentSession>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>,
) {
    app.execute_command(
        AppCommand::LoginCommand(Some(LLAMA_ID.to_string())),
        session,
        None,
    )
    .await;
    for answer in [LLAMA_URL, "sk-llama"] {
        let msg = next_msg(rx).await;
        assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "got {msg:?}");
        app.apply_login_msg(session, msg).await;
        type_text(app, answer);
        app.handle_input(&key(KeyCode::Enter));
    }
    let finished = next_msg(rx).await;
    match &finished {
        LoginUiMsg::Finished(f) => assert!(f.result.is_ok(), "login failed: {:?}", f.result),
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(session, finished).await;
}

/// End to end, zero models: a first-run `/login llama.cpp` reports the "no models are loaded"
/// guidance as an error line after the credentials line, selects nothing, and is NOT deferred —
/// `llama.cpp` has no curated default, so `deferSelection` (`:5944-5949`) is false and the
/// guidance is produced by the inline `finishAuthentication()` call, not by the refresh.
///
/// **Red without the change:** the transcript carries the generic `no default model is configured
/// for provider "llama.cpp"` line instead.
#[tokio::test]
async fn tui105_first_run_llama_cpp_login_with_no_models_says_to_load_one() {
    let fx = first_run_llama_fixture(Vec::new()).await;
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    login_to_llama(&mut app, &fx.session, &mut rx).await;

    let path = fx.agent_dir.join("auth.json").display().to_string();
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!(
            "Saved API key for llama.cpp. Credentials saved to {path}"
        )),
        "`:5991`; got:\n{text}"
    );
    assert!(
        text.contains(
            "Saved API key for llama.cpp. No llama.cpp models are loaded. \
             Use /llama to load a model, then /model to select it."
        ),
        "`llamaCppPostLoginGuidance(_, 0)` (`:350-351`); got:\n{text}"
    );
    assert!(
        !text.contains("no default model is configured"),
        "the llama.cpp rung precedes the generic one:\n{text}"
    );
    assert!(
        !text.contains("Refreshing model catalog"),
        "llama.cpp is not a `hasDefaultModelProvider`, so `deferSelection` is false:\n{text}"
    );
    assert_eq!(
        fx.session.model(),
        None,
        "the guidance path selects nothing"
    );

    let refresh = tokio::time::timeout(Duration::from_secs(5), refresh_rx.recv())
        .await
        .expect("the post-login refresh must settle")
        .expect("the refresh channel stays open");
    assert!(
        !refresh.defer,
        "`deferSelection` (`:5944-5949`) is false for llama.cpp"
    );
}

/// End to end, models loaded: the other template, and still no model selected.
///
/// **Red without the change:** a loaded `llama.cpp` model is neither selected (no curated default)
/// nor guided to, so the transcript has the generic `no default model is configured` line.
#[tokio::test]
async fn tui105_first_run_llama_cpp_login_with_models_points_at_model_and_llama() {
    let fx = first_run_llama_fixture(vec![
        model_named(LLAMA_ID, "tiny"),
        model_named(LLAMA_ID, "big"),
    ])
    .await;
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();

    login_to_llama(&mut app, &fx.session, &mut rx).await;

    assert!(
        fx.session
            .available_model_catalog()
            .iter()
            .filter(|m| m.provider.as_str() == LLAMA_ID)
            .count()
            == 2,
        "the credential must have made both models available"
    );
    let text = transcript_text(&app);
    assert!(
        text.contains(
            "Saved API key for llama.cpp. Use /model to select a loaded llama.cpp model, \
             or /llama to manage models."
        ),
        "`llamaCppPostLoginGuidance(_, n > 0)` (`:352`); got:\n{text}"
    );
    assert!(
        !text.contains("No llama.cpp models are loaded"),
        "the zero-model template must not be used when models are loaded:\n{text}"
    );
    assert_eq!(
        fx.session.model(),
        None,
        "pi never selects a llama.cpp model on login"
    );
}

/// The guidance only exists when the previous model is unknown (`isUnknownModel(previousModel)`,
/// `:5953`): a session that already has a model logs in to llama.cpp with the bare credentials line
/// and no advice.
///
/// **Red without the change:** a rung placed outside the `isUnknownModel` gate would print the
/// guidance to every user who logs in with a model already chosen.
#[tokio::test]
async fn tui105_llama_cpp_guidance_is_only_for_an_unknown_previous_model() {
    let fx = live_provider_fixture(LLAMA_ID, llama_like_provider(LLAMA_ID, "llama.cpp"), &[]).await;
    let before = fx.session.model().expect("the faux fixture has a model");
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();

    login_to_llama(&mut app, &fx.session, &mut rx).await;

    let path = fx.agent_dir.join("auth.json").display().to_string();
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!(
            "Saved API key for llama.cpp. Credentials saved to {path}"
        )),
        "got:\n{text}"
    );
    assert!(
        !text.contains("/llama") && !text.contains("llama.cpp models"),
        "a session that already has a model gets no llama.cpp guidance:\n{text}"
    );
    assert_eq!(fx.session.model(), Some(before));
}

/// Other providers are unaffected end to end: a first-run login to a curated-default provider still
/// selects its default and says nothing about `/llama`.
///
/// **Red without the change:** a rung that fired for every provider (or lost its id gate) would
/// replace the `Selected …` line with the llama.cpp advice.
#[tokio::test]
async fn tui105_other_providers_never_get_the_llama_cpp_guidance() {
    let fx = first_run_fixture().await;
    credential(&fx.session, DEFAULTED_PROVIDER);
    let mut app = app_with(registry_for(
        DEFAULTED_PROVIDER,
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth)),
    ));
    let mut rx = app.install_login_channel();

    let (action, msg) =
        drive_login_to_finished(&mut app, &fx.session, &mut rx, DEFAULTED_PROVIDER).await;
    app.apply_login_msg(&fx.session, msg).await;

    let text = transcript_text(&app);
    assert!(
        text.contains(&format!("{action}. Selected {DEFAULTED_MODEL}.")),
        "got:\n{text}"
    );
    assert!(
        !text.contains("/llama") && !text.contains("llama.cpp"),
        "{text}"
    );
}

// ================================================ EXT-027 — the post-login refresh reaches a LIVE provider
//
// pi's `completeProviderAuthentication` ends in `session.modelRuntime.refresh({ providers:
// [providerId], signal })` (`interactive-mode.ts:5953`). pi holds a native extension's provider in
// the SAME composed collection as the built-ins (`nativeExtensionProviders`,
// `core/model-runtime.ts:298` @v0.99.2-17), so that call reaches the provider's own `refreshModels`
// (`models.ts:546-606`) and the models it publishes are what `/model` lists afterwards. cyrup keeps
// such a provider in the guest registry, which the pi.dev catalog service behind
// `AgentSession::refresh_provider_catalog` never touched: after `/login llama.cpp` nothing was
// refreshed and the models appeared only after a restart or `/llama`.
//
// **No network.** The provider's "refresh" is a scripted in-memory publication; the credential
// strategy is the same pure-function `LlamaLikeAuth` the tests above use.

/// What a [`Refreshable`] does when the host runs its NETWORK-phase `refresh_models`.
#[derive(Clone)]
enum RefreshPlan {
    /// Publish a catalog of these model ids, by re-registering the provider live (the way the
    /// `cyrup-llama` controller swaps its provider, `cyrup-llama/src/provider.rs`).
    Publish(Vec<&'static str>),
    /// Fail with this message (pi's `refreshModels` throwing).
    Fail(&'static str),
    /// Block until the caller aborts (`context.signal`).
    Hang,
}

type RegistrarSlot = Arc<std::sync::Mutex<Option<Arc<dyn cyrup_ext::LateRegistrar>>>>;

/// A live provider with a scripted `refresh_models`. `seen` records the `allow_network` flag of every
/// refresh the host drove through it, `models` is what the registry exposes until a publication swaps
/// the provider for a fresh one.
struct Refreshable {
    id: ProviderId,
    models: Vec<Model>,
    plan: RefreshPlan,
    registrar: RegistrarSlot,
    seen: Arc<std::sync::Mutex<Vec<bool>>>,
    auth: ProviderAuth,
}

impl Refreshable {
    fn new(
        id: &str,
        cached: &[&'static str],
        plan: RefreshPlan,
        registrar: RegistrarSlot,
        seen: Arc<std::sync::Mutex<Vec<bool>>>,
    ) -> Self {
        Self {
            id: ProviderId::from(id),
            models: cached.iter().map(|m| model_named(id, m)).collect(),
            plan,
            registrar,
            seen,
            auth: ProviderAuth::with_api_key(Arc::new(LlamaLikeAuth)),
        }
    }
}

#[async_trait::async_trait]
impl Provider for Refreshable {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn name(&self) -> &str {
        "llama.cpp"
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        Some(&self.auth)
    }
    fn has_refresh_models(&self) -> bool {
        true
    }
    async fn refresh_models(
        &self,
        ctx: &cyrup_provider::RefreshModelsContext,
    ) -> Option<Result<(), cyrup_provider::ProviderError>> {
        self.seen.lock().unwrap().push(ctx.allow_network);
        // The cache-only restore the startup performs has nothing persisted to restore here.
        if !ctx.allow_network {
            return Some(Ok(()));
        }
        match &self.plan {
            RefreshPlan::Fail(message) => Some(Err(cyrup_provider::ProviderError::ModelSource(
                (*message).into(),
            ))),
            RefreshPlan::Hang => {
                ctx.cancelled().await;
                Some(Ok(()))
            }
            RefreshPlan::Publish(ids) => {
                let refresh = ctx.clone();
                let next: Arc<dyn Provider> = Arc::new(Refreshable::new(
                    self.id.as_str(),
                    ids,
                    self.plan.clone(),
                    Arc::clone(&self.registrar),
                    Arc::clone(&self.seen),
                ));
                let registrar = self.registrar.lock().unwrap().clone();
                let id = self.id.as_str().to_string();
                let published = refresh
                    .publish(cyrup_ext::host::services::ModelsPublication {
                        persist: None,
                        update: Some(Box::new(move || {
                            if let Some(registrar) = registrar {
                                let _ = registrar.register_provider_live(id, next);
                            }
                        })),
                    })
                    .await;
                Some(published.map(|_| ()))
            }
        }
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

/// The native that registers a [`Refreshable`] live and keeps the late registrar the host hands it.
struct RefreshableNative {
    provider: Arc<dyn Provider>,
    registrar: RegistrarSlot,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for RefreshableNative {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("refreshable-ext")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.register_provider_live(self.provider.id().as_str(), Arc::clone(&self.provider));
        Ok(())
    }
    async fn on_event(
        &self,
        _ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        cyrup_ext::HookOutcome::Noop
    }
    fn set_late_registrar(&self, registrar: Arc<dyn cyrup_ext::LateRegistrar>) {
        *self.registrar.lock().unwrap() = Some(registrar);
    }
}

/// A credential-less first run (`session.model() == None`) with a [`Refreshable`] registered live
/// under `id`, holding `cached` as the catalog the registry already knows. Returns the session and
/// the `allow_network` log of its refreshes.
async fn first_run_refreshable_fixture(
    id: &'static str,
    cached: &[&'static str],
    plan: RefreshPlan,
) -> (Fixture, Arc<std::sync::Mutex<Vec<bool>>>) {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir.clone());
    config.trust_override = Some(true);
    config.no_extensions = true;
    let registrar: RegistrarSlot = Arc::default();
    let seen: Arc<std::sync::Mutex<Vec<bool>>> = Arc::default();
    let provider: Arc<dyn Provider> = Arc::new(Refreshable::new(
        id,
        cached,
        plan,
        Arc::clone(&registrar),
        Arc::clone(&seen),
    ));
    let session = SessionBuilder::new(
        Arc::new(ModellessProvider(ProviderId::from("none"))) as Arc<dyn Provider>,
        config,
    )
    .auth(hermetic_auth(&agent_dir))
    .with_native_extension(Arc::new(RefreshableNative {
        provider,
        registrar,
    }) as Arc<dyn cyrup_ext::NativeExtension>)
    .build()
    .await
    .unwrap();
    assert_eq!(session.model(), None, "pi's isUnknownModel state");
    (
        Fixture {
            _tmp: tmp,
            agent_dir,
            session: Arc::new(session),
        },
        seen,
    )
}

/// `/login <id>` through the llama-like strategy's two prompts, applying the `Finished`.
async fn login_to_live(
    app: &mut App<TestBackend>,
    session: &Arc<AgentSession>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>,
    id: &str,
) {
    app.execute_command(
        AppCommand::LoginCommand(Some(id.to_string())),
        session,
        None,
    )
    .await;
    for answer in [LLAMA_URL, "sk-live"] {
        let msg = next_msg(rx).await;
        assert!(matches!(msg, LoginUiMsg::Prompt { .. }), "got {msg:?}");
        app.apply_login_msg(session, msg).await;
        type_text(app, answer);
        app.handle_input(&key(KeyCode::Enter));
    }
    let finished = next_msg(rx).await;
    match &finished {
        LoginUiMsg::Finished(f) => assert!(f.result.is_ok(), "login failed: {:?}", f.result),
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(session, finished).await;
}

/// The next settled post-login refresh, failing loudly rather than hanging.
async fn next_refresh(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::login_dialog::LoginRefreshMsg>,
) -> crate::login_dialog::LoginRefreshMsg {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the post-login refresh must settle")
        .expect("the refresh channel stays open")
}

fn live_models(session: &Arc<AgentSession>, id: &str) -> Vec<String> {
    let mut ids: Vec<String> = session
        .available_model_catalog()
        .iter()
        .filter(|m| m.provider.as_str() == id)
        .map(|m| m.id.as_str().to_string())
        .collect();
    ids.sort();
    ids
}

/// **The headline.** `/login llama.cpp` refreshes the LIVE provider's catalog, so the models its
/// refresh publishes are listed once the post-login refresh settles — pi's
/// `modelRuntime.refresh({ providers: ["llama.cpp"], signal })` (`interactive-mode.ts:5953`) reaching
/// the extension provider's `refreshModels`.
///
/// The guidance printed at login is computed BEFORE the refresh, from the cached catalog, exactly as
/// pi orders it: `llama.cpp` has no curated default so `deferSelection` is false and
/// `await finishAuthentication()` (`:5950`) runs ahead of the refresh (`:5951-5973`). The refresh
/// then moves the count (`:5964`), which is what `/model` and the footer read.
///
/// **Red without the change:** `refresh_provider_catalog` consults only the pi.dev catalog service,
/// which a fixture session does not even wire, so the result is a clean no-op, the provider's
/// `refresh_models` is never called and `available_model_catalog()` still holds no llama.cpp model.
#[tokio::test]
async fn ext027_login_to_a_live_provider_refreshes_its_catalog() {
    let (fx, seen) =
        first_run_refreshable_fixture(LLAMA_ID, &[], RefreshPlan::Publish(vec!["tiny", "big"]))
            .await;
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    login_to_live(&mut app, &fx.session, &mut rx, LLAMA_ID).await;
    assert!(
        live_models(&fx.session, LLAMA_ID).is_empty(),
        "nothing is cached, so nothing is listed before the refresh lands"
    );
    assert!(
        transcript_text(&app).contains("No llama.cpp models are loaded."),
        "the guidance counts the catalog as it stood at `finishAuthentication` (`:5950`)"
    );

    let refresh = next_refresh(&mut refresh_rx).await;
    assert!(refresh.result.is_clean(), "{:?}", refresh.result);
    app.apply_login_refresh(&fx.session, refresh).await;

    assert_eq!(
        live_models(&fx.session, LLAMA_ID),
        vec!["big".to_string(), "tiny".to_string()],
        "the models the provider's refresh published must be listed after the login refresh"
    );
    assert!(
        seen.lock().unwrap().contains(&true),
        "the provider's NETWORK phase must have run (`models.ts:573-576`): {:?}",
        seen.lock().unwrap()
    );
    let text = transcript_text(&app);
    assert!(
        !text.contains("could not be refreshed") && !text.contains("timed out"),
        "a clean refresh warns about nothing:\n{text}"
    );
    assert_eq!(
        fx.session.model(),
        None,
        "pi never selects a llama.cpp model on login (`:5957-5958`)"
    );
}

/// The DEFERRED shape on a live provider: the curated default is absent until the refresh publishes
/// it, so `finishAuthentication` runs from the refresh continuation (`:5958-5963`) and its
/// `providerModels.length` — the count the guidance and every other rung read — is the REAL one.
///
/// **Red without the change:** the refresh publishes nothing, the continuation finds zero models
/// and reports `no models are available for that provider`, selecting nothing.
#[tokio::test]
async fn ext027_a_deferred_live_login_selects_from_the_refreshed_catalog() {
    let (fx, _seen) = first_run_refreshable_fixture(
        DYNAMIC_PROVIDER,
        &[],
        RefreshPlan::Publish(vec![DYNAMIC_MODEL]),
    )
    .await;
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    login_to_live(&mut app, &fx.session, &mut rx, DYNAMIC_PROVIDER).await;
    let before = transcript_text(&app);
    assert!(
        before.contains("Refreshing model catalog…"),
        "`deferSelection` (`:5944-5949`) postpones the selection:\n{before}"
    );
    assert_eq!(fx.session.model(), None);

    let refresh = next_refresh(&mut refresh_rx).await;
    assert!(refresh.defer);
    app.apply_login_refresh(&fx.session, refresh).await;

    let model = fx.session.model().expect("the continuation must select");
    assert_eq!(
        (model.provider.as_str(), model.model.as_str()),
        (DYNAMIC_PROVIDER, DYNAMIC_MODEL)
    );
    let text = transcript_text(&app);
    assert!(
        text.contains(&format!("Selected {DYNAMIC_MODEL}.")),
        "got:\n{text}"
    );
    assert!(!text.contains("no models are available"), "{text}");
}

/// A refresh that FAILS is surfaced the way pi surfaces it for any provider
/// (`` `${actionLabel}, but its model catalog could not be refreshed; using cached models.` ``,
/// `interactive-mode.ts:5959`), and the cached catalog stays usable.
///
/// **Red without the change:** the live provider is never refreshed, so no failure exists to report
/// and the transcript carries no warning.
#[tokio::test]
async fn ext027_a_failed_live_refresh_warns_and_keeps_the_cached_models() {
    let (fx, seen) =
        first_run_refreshable_fixture(LLAMA_ID, &["cached"], RefreshPlan::Fail("server down"))
            .await;
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    login_to_live(&mut app, &fx.session, &mut rx, LLAMA_ID).await;
    let refresh = next_refresh(&mut refresh_rx).await;
    assert!(
        refresh.result.errors.contains_key(LLAMA_ID),
        "the provider's failure is reported under its id: {:?}",
        refresh.result
    );
    assert!(
        refresh
            .result
            .errors
            .get(LLAMA_ID)
            .is_some_and(|e| e.contains("server down")),
        "{:?}",
        refresh.result
    );
    app.apply_login_refresh(&fx.session, refresh).await;

    assert!(
        transcript_text(&app).contains(
            "Saved API key for llama.cpp, but its model catalog could not be refreshed; \
             using cached models."
        ),
        "got:\n{}",
        transcript_text(&app)
    );
    assert_eq!(
        live_models(&fx.session, LLAMA_ID),
        vec!["cached".to_string()],
        "a failed refresh leaves the cached catalog in place"
    );
    assert!(seen.lock().unwrap().contains(&true));
}

/// Cancellation reaches the provider: a refresh blocked on the server returns as soon as the
/// caller's token fires (the login's 15 s deadline, `:5951-5952`), reported as THIS caller's timeout
/// so the continuation prints pi's `timed out` warning (`:5957`).
///
/// **Red without the change:** the live provider is never asked, the call settles clean at once and
/// `timed_out` is false.
#[tokio::test]
async fn ext027_cancelling_the_login_refresh_stops_a_blocked_live_provider() {
    let (fx, seen) = first_run_refreshable_fixture(LLAMA_ID, &[], RefreshPlan::Hang).await;
    // The credential the login would have stored, so the network phase resolves one.
    let mut cred_env = std::collections::BTreeMap::new();
    cred_env.insert(LLAMA_URL_ENV.to_string(), LLAMA_URL.to_string());
    fx.session
        .services()
        .auth
        .modify(&ProviderId::from(LLAMA_ID), |_| async move {
            Ok::<_, cyrup_config::AuthError>(Some(cyrup_config::Credential::ApiKey {
                key: None,
                env: Some(cred_env),
            }))
        })
        .await
        .unwrap();

    let cancel = cyrup_core::CancelToken::new();
    let trip = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(200)).await;
        trip.cancel();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(10),
        fx.session.refresh_provider_catalog(cancel, LLAMA_ID),
    )
    .await
    .expect("a cancelled refresh must return, not hang on the provider");

    assert!(
        seen.lock().unwrap().contains(&true),
        "the provider was blocked in its network phase: {:?}",
        seen.lock().unwrap()
    );
    assert!(result.timed_out, "the caller's token fired: {result:?}");
    assert!(
        result.errors.is_empty(),
        "an abort is not a failure: {result:?}"
    );
}

/// pi passes no `allowNetwork` on the login refresh, so the runtime's own switch decides
/// (`options.allowNetwork ?? this.modelNetworkEnabled`, `core/model-runtime.ts:850`): an offline
/// session's login refresh restores the cache and touches no network.
///
/// **Red without the change:** a refresh that forced `allow_network: true` would run the provider's
/// network phase under an offline run.
#[tokio::test]
async fn ext027_the_login_refresh_honours_the_offline_switch() {
    let (fx, seen) =
        first_run_refreshable_fixture(LLAMA_ID, &[], RefreshPlan::Publish(vec!["tiny"])).await;
    fx.session
        .services()
        .guest_providers
        .set_network_enabled(false);
    let mut app = app_with(no_builtin_providers());
    let mut rx = app.install_login_channel();
    let mut refresh_rx = app.install_login_refresh_channel();

    login_to_live(&mut app, &fx.session, &mut rx, LLAMA_ID).await;
    let refresh = next_refresh(&mut refresh_rx).await;
    app.apply_login_refresh(&fx.session, refresh).await;

    assert!(
        !seen.lock().unwrap().contains(&true),
        "no network phase under the offline switch: {:?}",
        seen.lock().unwrap()
    );
    assert!(
        live_models(&fx.session, LLAMA_ID).is_empty(),
        "nothing was fetched, so nothing is listed"
    );
}
