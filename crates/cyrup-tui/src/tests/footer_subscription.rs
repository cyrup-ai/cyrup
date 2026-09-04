//! The footer's ` (sub)` marker, driven through the REAL entry points.
//!
//! Ports pi v0.84.1 `coding-agent/src/modes/interactive/components/footer.ts:138-145`:
//!
//! ```text
//! // Kimi Coding is subscription-backed despite using API-key authentication.
//! const usingSubscription = state.model
//!     ? state.model.provider === "kimi-coding" || this.session.modelRuntime.isUsingSubscription(state.model.provider)
//!     : false;
//! if (usageTotals.cost || usingSubscription) {
//!     const costStr = `$${usageTotals.cost.toFixed(3)}${usingSubscription ? " (sub)" : ""}`;
//!     statsParts.push(costStr);
//! }
//! ```
//!
//! and `coding-agent/src/core/model-runtime.ts:458-464`:
//!
//! ```text
//! isUsingOAuth(providerId)      { return this.snapshot.auth.get(providerId)?.type === "oauth"; }
//! isUsingSubscription(providerId) { return this.isUsingOAuth(providerId) && this.models.getProvider(providerId)?.auth.oauth?.isSubscription === true; }
//! ```
//!
//! **What "real entry point" means here.** `StatusLine::set_using_subscription` is never called by
//! these tests. Each one drives a user action — `/login <provider>` answered in the dialog, a
//! `/logout` confirmation, or the `model_changed` session event — and then asserts on the text the
//! footer actually painted into the terminal buffer. Before this wiring existed the ONLY caller of
//! `set_using_subscription` workspace-wide was `tests/render.rs:387`, so every assertion below
//! fails with the marker simply absent.
//!
//! **No network.** Same stub-provider construction as `tests/login_flow.rs`: the OAuth strategy is
//! a pure in-process function with no endpoint, so nothing here can reach a provider even by
//! accident.
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
use crate::{App, AppCommand, LoginProviderSource, LoginUiMsg, SelectorKind, UiTheme};
use cyrup_config::login::AuthType;
use cyrup_core::EventStream;
use cyrup_core::{ProviderId, StopReason};
use cyrup_provider::AuthError as ProviderAuthError;
use cyrup_provider::auth::oauth::{AuthInteraction, AuthPrompt, OAuthError};
use cyrup_provider::auth::{ApiKeyAuth, ModelAuth, OAuthAuth, ProviderAuth};
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_provider::{Context, Credential, Model, Provider, StreamEvent, StreamOptions};
use cyrup_session_svc::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

// ---------------------------------------------------------------- stub provider

/// An OAuth strategy that mints a credential from one prompt answer. `SUB` is `isSubscription`
/// (`ai/src/auth/types.ts:210-211`) — `true` models Anthropic/xAI/Kimi/Copilot/Codex, `false`
/// models OpenRouter and Radius, the metered OAuth flows upstream deliberately leaves unset
/// (`oauth/openrouter.ts:301-311`, `oauth/radius.ts:357-361`).
struct ScriptedOauth<const SUB: bool>;

#[async_trait::async_trait]
impl<const SUB: bool> OAuthAuth for ScriptedOauth<SUB> {
    fn name(&self) -> &str {
        "Stub"
    }
    fn is_subscription(&self) -> bool {
        SUB
    }
    async fn login(&self, interaction: &dyn AuthInteraction) -> Result<Credential, OAuthError> {
        let code = interaction.prompt(AuthPrompt::text("code?")).await?;
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

/// A one-provider registry under the given id — cyrup's stand-in for `models.getProvider(id)`,
/// which is where pi reads `auth.oauth?.isSubscription` (`model-runtime.ts:463`).
fn stub_registry(id: &'static str, auth: ProviderAuth) -> LoginProviderSource {
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
    session: Arc<AgentSession>,
}

async fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
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
        session: Arc::new(session),
    }
}

fn app_with(registry: LoginProviderSource) -> App<TestBackend> {
    // 120 columns so the whole left cluster fits without the `footer.ts:174-177` truncation.
    let mut app = App::new(TestBackend::new(120, 8), UiTheme::dark()).unwrap();
    app.set_login_provider_source(registry);
    app
}

async fn next_msg(rx: &mut tokio::sync::mpsc::UnboundedReceiver<LoginUiMsg>) -> LoginUiMsg {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("the login flow should post a message within 5s")
        .expect("the login channel should stay open")
}

/// **The user action.** `/login <id>` → answer the dialog prompt → the flow settles. Exactly the
/// path `tests/login_flow.rs` proves writes a credential; here it is the *event* the footer marker
/// hangs off.
async fn run_login(app: &mut App<TestBackend>, session: &Arc<AgentSession>, id: &str) {
    let mut rx = app.install_login_channel();
    app.execute_command(
        AppCommand::LoginCommand(Some(id.to_string())),
        session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginDialog),
        "`/login {id}` must open the login dialog"
    );
    // `prompt(...)` → `dialog.showPrompt(message)` (`interactive-mode.ts:5331`).
    let msg = next_msg(&mut rx).await;
    app.apply_login_msg(msg);
    for c in "CODE42".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.handle_input(&key(KeyCode::Enter));
    // The settle message — pi's `completeProviderAuthentication` → `footer.invalidate()`
    // (`interactive-mode.ts:5448-5449`).
    let msg = next_msg(&mut rx).await;
    match &msg {
        LoginUiMsg::Finished(f) => assert!(f.result.is_ok(), "login failed: {:?}", f.result),
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(msg);
}

/// `model_changed` (`agent-session.ts` → `interactive-mode.ts:3068-3070`, which ends in
/// `footer.invalidate()`). The real dispatch, not a setter.
fn select_model(app: &mut App<TestBackend>, provider: &str, model: &str) {
    app.ingest_event(&AgentSessionEvent::ModelChanged {
        provider: provider.to_string(),
        model: model.to_string(),
    });
}

// ================================================================ the proofs

/// **The end-to-end.** Sign in to a subscription-backed provider with `/login`, keep the model on
/// that provider, and the footer prints `$0.000 (sub)` — with no restart and no further event.
#[tokio::test]
async fn login_to_a_subscription_provider_lights_the_footer_marker() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(
        "stub",
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth::<true>)),
    ));

    // The active model is on `stub` BEFORE the login, so the only thing that moves is the
    // credential — i.e. this asserts the login-settled recompute, not the model-changed one.
    select_model(&mut app, "stub", "stub-model");
    app.draw().unwrap();
    assert!(
        !buf_text(&app).contains("(sub)"),
        "no credential yet ⇒ `isUsingOAuth` is false ⇒ no marker:\n{}",
        buf_text(&app)
    );

    run_login(&mut app, &fx.session, "stub").await;
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(
        t.contains("$0.000 (sub)"),
        "after signing in to a subscription-backed provider the footer must show `$0.000 (sub)` \
         (footer.ts:142-145):\n{t}"
    );
}

/// **The case that separates a correct footer from a wrong one.** OpenRouter-shaped: a real OAuth
/// credential on a provider whose flow is metered (`isSubscription` unset,
/// `oauth/openrouter.ts:301-311`). pi v0.84.0's changelog records fixing exactly this — *"Fixed the
/// footer showing `(sub)` for generic OAuth/OpenID sign-ins without a known subscription"*
/// (`coding-agent/CHANGELOG.md:155`). The pre-v0.84.0 predicate (`isUsingOAuth` alone,
/// `v0.83.0:footer.ts:140`) fails this test.
#[tokio::test]
async fn oauth_login_to_a_metered_provider_does_not_light_the_marker() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(
        "stub",
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth::<false>)),
    ));
    select_model(&mut app, "stub", "stub-model");
    run_login(&mut app, &fx.session, "stub").await;
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(
        !t.contains("(sub)"),
        "a metered OAuth sign-in must NOT be labelled a subscription \
         (model-runtime.ts:463 requires `auth.oauth?.isSubscription === true`):\n{t}"
    );
}

/// **The other conjunct.** A provider that OFFERS a subscription OAuth flow, used with an API key:
/// `isUsingOAuth` is false, so no marker. This is the Anthropic-with-`ANTHROPIC_API_KEY` case, and
/// it is why `model-runtime.ts:463` ANDs the two predicates instead of reading `isSubscription`
/// alone.
#[tokio::test]
async fn api_key_login_to_a_subscription_capable_provider_does_not_light_the_marker() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(
        "stub",
        ProviderAuth {
            api_key: Some(env_key_like()),
            oauth: Some(Arc::new(ScriptedOauth::<true>)),
        },
    ));
    select_model(&mut app, "stub", "stub-model");

    // `/login stub` on a provider offering BOTH methods opens the auth-type selector
    // (`interactive-mode.ts:4998-5009`); pick the API-key row.
    let mut rx = app.install_login_channel();
    app.execute_command(
        AppCommand::LoginCommand(Some("stub".to_string())),
        &fx.session,
        None,
    )
    .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::LoginAuthType),
        "two methods ⇒ the auth-type selector"
    );
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::LoginAuthType,
            value: AuthType::ApiKey.as_str().to_string(),
        },
        &fx.session,
        None,
    )
    .await;
    let msg = next_msg(&mut rx).await;
    app.apply_login_msg(msg);
    for c in "sk-test".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    app.handle_input(&key(KeyCode::Enter));
    let msg = next_msg(&mut rx).await;
    match &msg {
        LoginUiMsg::Finished(f) => {
            assert!(f.result.is_ok(), "api-key login failed: {:?}", f.result);
            assert!(!f.oauth, "this is the api_key leg");
        }
        other => panic!("expected Finished, got {other:?}"),
    }
    app.apply_login_msg(msg);

    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(
        !t.contains("(sub)"),
        "an API-key credential is not an OAuth one (`isUsingOAuth`, model-runtime.ts:458-460), \
         so a subscription-capable provider must still render unmarked:\n{t}"
    );
}

/// **`/logout` takes it back down.** The credential that lit the marker is removed through the real
/// `/logout` selector, and the footer stops claiming a subscription.
#[tokio::test]
async fn logout_clears_the_footer_marker() {
    let fx = fixture().await;
    let mut app = app_with(stub_registry(
        "stub",
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth::<true>)),
    ));
    select_model(&mut app, "stub", "stub-model");
    run_login(&mut app, &fx.session, "stub").await;
    app.draw().unwrap();
    assert!(
        buf_text(&app).contains("(sub)"),
        "precondition: marker is up"
    );

    // `/logout` → the stored-credential picker (`interactive-mode.ts:5354-5379`) → confirm row 0.
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

    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(
        !t.contains("(sub)"),
        "logging out removes the credential `isUsingOAuth` reads, so the marker must go:\n{t}"
    );
}

/// **The upstream special case.** `kimi-coding` is marked subscription-backed by provider id alone,
/// with no credential of any kind — *"Kimi Coding is subscription-backed despite using API-key
/// authentication"* (`footer.ts:138-140`). Driven through `model_changed`, the event a `/model`
/// switch emits.
#[tokio::test]
async fn kimi_coding_is_marked_by_provider_id_with_no_credential() {
    let mut app = app_with(stub_registry(
        "stub",
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth::<true>)),
    ));
    select_model(&mut app, "kimi-coding", "kimi-k2-turbo-preview");
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(
        t.contains("$0.000 (sub)"),
        "footer.ts:139-140 short-circuits on `state.model.provider === \"kimi-coding\"`:\n{t}"
    );
}

/// **Switching away turns it off.** A `/model` change to a provider with no subscription must clear
/// the marker the previous provider set — pi recomputes per repaint, cyrup recomputes on the
/// `model_changed` event.
#[tokio::test]
async fn switching_to_a_non_subscription_provider_clears_the_marker() {
    let mut app = app_with(stub_registry(
        "stub",
        ProviderAuth::with_oauth(Arc::new(ScriptedOauth::<true>)),
    ));
    select_model(&mut app, "kimi-coding", "kimi-k2-turbo-preview");
    app.draw().unwrap();
    assert!(
        buf_text(&app).contains("(sub)"),
        "precondition: marker is up"
    );

    select_model(&mut app, "stub", "stub-model");
    app.draw().unwrap();
    let t = buf_text(&app);
    assert!(
        !t.contains("(sub)"),
        "the marker must follow the ACTIVE provider (footer.ts:139-141):\n{t}"
    );
}
