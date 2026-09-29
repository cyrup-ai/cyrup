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

    // Rung 2: `!hasDefaultModelProvider(providerId)` (`:5904-5905`). `"stub"` is absent from
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
