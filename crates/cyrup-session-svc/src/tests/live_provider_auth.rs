//! H3 — an extension's LIVE provider is first-class in auth: its models are available exactly when
//! its own auth strategy says so, a native can read the auth its strategy resolves
//! (`provider_auth`), and it can let the provider resolve its own credential on every request
//! (`provider_credentials`).
//!
//! pi composes a native extension's provider into the same `Models` collection as the built-ins, so
//! `configuredProviders` is filled by `models.checkAuth` for it like for any other provider
//! (`core/model-runtime.ts:334-362` @v0.99.2-17), and `ctx.modelRegistry.getProviderAuth(id)` is
//! `runtime.getAuth(id)` (`core/model-registry.ts:180`). The strategy is shaped like pi's llama.cpp
//! one (`extensions/llama/provider.ts:156-196`): `check` is active only when a server URL is known,
//! from the stored credential's `env.LLAMA_BASE_URL` or from the `LLAMA_BASE_URL` environment
//! variable.
//!
//! A STATIC JSON-registered guest provider keeps its old behaviour — always available — and one test
//! pins that.
//!
//! **No network.** The strategy resolves strings; nothing here opens a socket.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{EventStream, ExtensionId, ProviderId};
use cyrup_ext::host::HostServices;
use cyrup_ext::provider::ProviderRegistration;
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::auth::{ApiKeyAuth, ProviderAuth};
use cyrup_provider::collection::{AuthCheck, AuthType};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{
    AuthContext, AuthError, AuthResult, Context, Credential, Model, ModelAuth, Provider,
    StreamEvent, StreamOptions,
};
use tempfile::TempDir;

use crate::session::LIVE_AUTH_CHECK_TIMEOUT;
use crate::{AgentSession, SessionBuilder, SessionConfig};

pub(super) const LLAMA: &str = "llama.cpp";
pub(super) const URL_ENV: &str = "LLAMA_BASE_URL";
pub(super) const SERVER: &str = "http://127.0.0.1:9";

// ---------------------------------------------------------------------- the strategy under test

async fn server_url(ctx: &dyn AuthContext, cred: Option<&Credential>) -> Option<String> {
    let stored = cred
        .and_then(Credential::env)
        .and_then(|env| env.get(URL_ENV))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match stored {
        Some(url) => Some(url),
        None => ctx.env(URL_ENV).await.map(|v| v.trim().to_string()),
    }
}

fn source_of(cred: Option<&Credential>) -> String {
    if cred.is_some() {
        "stored credential".to_string()
    } else {
        URL_ENV.to_string()
    }
}

pub(super) struct UrlGatedKey;

#[async_trait::async_trait]
impl ApiKeyAuth for UrlGatedKey {
    fn name(&self) -> &str {
        "llama.cpp server"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        Ok(server_url(ctx, cred).await.map(|_| AuthCheck {
            auth_type: AuthType::ApiKey,
            source: Some(source_of(cred)),
        }))
    }
    async fn resolve(
        &self,
        _model: &Model,
        ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let Some(url) = server_url(ctx, cred).await else {
            return Ok(None);
        };
        let key = match cred {
            Some(Credential::ApiKey { key: Some(key), .. }) => key.clone(),
            _ => "local".to_string(),
        };
        let mut env = cred.and_then(Credential::env).cloned().unwrap_or_default();
        env.insert(URL_ENV.to_string(), url.clone());
        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: Some(key),
                headers: None,
                base_url: Some(format!("{url}/v1")),
            },
            env: Some(env),
            source: Some(source_of(cred)),
        }))
    }
}

// ----------------------------------------------------------------------------- the live provider

pub(super) struct LiveProvider {
    pub(super) id: ProviderId,
    pub(super) auth: Option<ProviderAuth>,
    pub(super) models: Vec<Model>,
}

#[async_trait::async_trait]
impl Provider for LiveProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.auth.as_ref()
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

pub(super) fn models_of(provider: &str, ids: &[&str]) -> Vec<Model> {
    let reg = ProviderRegistration {
        id: provider.to_string(),
        config: serde_json::from_value(serde_json::json!({
            "name": provider,
            "baseUrl": "http://127.0.0.1:8080/v1",
            "api": "openai-completions",
            "models": ids
                .iter()
                .map(|id| serde_json::json!({ "id": id, "name": id }))
                .collect::<Vec<_>>(),
        }))
        .unwrap(),
        resolved_api_key: None,
    };
    reg.build_models()
}

/// A live provider with the llama-like strategy (`strategy == true`) or with no `auth` at all.
pub(super) fn live(id: &str, models: &[&str], strategy: bool) -> Arc<dyn Provider> {
    Arc::new(LiveProvider {
        id: ProviderId::from(id),
        auth: strategy.then(|| ProviderAuth::with_api_key(Arc::new(UrlGatedKey))),
        models: models_of(id, models),
    })
}

pub(super) struct LiveNative(pub(super) Vec<Arc<dyn Provider>>);

#[async_trait::async_trait]
impl NativeExtension for LiveNative {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("llama-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        for provider in &self.0 {
            api.register_provider_live(provider.id().as_str(), Arc::clone(provider));
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

// ----------------------------------------------------------------------------------- the fixture

struct Fx {
    _tmp: TempDir,
    session: AgentSession,
}

/// A session over a store whose ambient environment is `env` and nothing else, with `providers`
/// registered live by a native extension.
async fn fixture(env: &[(&str, &str)], providers: Vec<Arc<dyn Provider>>) -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir: PathBuf = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let ambient: HashMap<String, String> = env
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let auth = Arc::new(
        cyrup_config::AuthStore::at(agent_dir.join("auth.json")).with_ambient_env(ambient),
    );
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .auth(auth)
        .with_native_extension(Arc::new(LiveNative(providers)) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("session builds");
    Fx { _tmp: tmp, session }
}

async fn store_llama_credential(session: &AgentSession, key: Option<&str>, url: Option<&str>) {
    let env = url.map(|u| {
        let mut m = BTreeMap::new();
        m.insert(URL_ENV.to_string(), u.to_string());
        m
    });
    let credential = cyrup_config::Credential::ApiKey {
        key: key.map(str::to_string),
        env,
    };
    session
        .services()
        .auth
        .modify(
            &ProviderId::from(LLAMA),
            |_| async move { Ok(Some(credential)) },
        )
        .await
        .unwrap();
}

fn available_ids(session: &AgentSession, provider: &str) -> Vec<String> {
    session
        .available_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() == provider)
        .map(|m| m.id.as_str().to_string())
        .collect()
}

fn llama_model(session: &AgentSession) -> Model {
    session
        .full_model_catalog()
        .into_iter()
        .find(|m| m.provider.as_str() == LLAMA)
        .expect("the live provider's model is in the full registry")
}

// ===================================================================================== availability

/// With no stored credential and no `LLAMA_BASE_URL`, the strategy's `check` says "not configured",
/// so the live provider's models are in the registry but NOT in the available set — today every
/// guest-registered provider was always available.
#[tokio::test]
async fn a_live_provider_is_unavailable_without_config() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;

    assert_eq!(
        fx.session
            .full_model_catalog()
            .iter()
            .filter(|m| m.provider.as_str() == LLAMA)
            .count(),
        1,
        "the model is registered"
    );
    assert!(available_ids(&fx.session, LLAMA).is_empty());
    assert!(!fx.session.has_configured_auth(&llama_model(&fx.session)));
}

/// A stored credential naming the server makes the provider available — and the answer follows the
/// store immediately, with no restart, as `/login` requires.
#[tokio::test]
async fn a_live_provider_is_available_with_a_stored_credential() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    assert!(available_ids(&fx.session, LLAMA).is_empty());

    store_llama_credential(&fx.session, Some("sk"), Some(SERVER)).await;

    assert_eq!(available_ids(&fx.session, LLAMA), vec!["tiny"]);
    assert!(fx.session.has_configured_auth(&llama_model(&fx.session)));
}

/// `LLAMA_BASE_URL` in the environment alone makes it available (pi: "active only when a URL is
/// known, from a stored credential or `LLAMA_BASE_URL`", `provider.ts:181-186`).
#[tokio::test]
async fn a_live_provider_is_available_with_the_env_var() {
    let fx = fixture(&[(URL_ENV, SERVER)], vec![live(LLAMA, &["tiny"], true)]).await;
    assert_eq!(available_ids(&fx.session, LLAMA), vec!["tiny"]);
}

/// The strategy is AUTHORITATIVE for a live provider: a stored credential that names no server
/// configures nothing, though the generic "any stored credential counts" predicate would say yes.
#[tokio::test]
async fn a_stored_credential_without_a_server_url_does_not_make_it_available() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    store_llama_credential(&fx.session, Some("sk"), None).await;

    assert!(
        fx.session
            .services()
            .auth
            .has_auth(&ProviderId::from(LLAMA), None),
        "the generic predicate counts the stored credential"
    );
    assert!(available_ids(&fx.session, LLAMA).is_empty());
}

/// REGRESSION PIN. A static JSON-registered guest provider keeps today's behaviour exactly: its
/// models are available with no credential and no environment, because the registration carries its
/// own key. The live-provider rule must not leak onto it.
#[tokio::test]
async fn a_json_registered_guest_provider_stays_always_available() {
    let fx = fixture(&[], Vec::new()).await;
    fx.session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("acme-ext"),
            "acme",
            serde_json::json!({
                "name": "Acme",
                "baseUrl": "http://127.0.0.1:1/v1",
                "api": "openai-completions",
                "models": [{ "id": "acme-fast", "name": "Acme Fast" }],
            }),
        )
        .unwrap();

    assert_eq!(available_ids(&fx.session, "acme"), vec!["acme-fast"]);
    assert!(
        fx.session.live_extension_providers().is_empty(),
        "a JSON registration is not a live provider"
    );
}

/// A live provider that carries NO auth strategy keeps today's behaviour too: there is no strategy
/// to ask, so it stays always available.
#[tokio::test]
async fn a_live_provider_without_an_auth_strategy_stays_always_available() {
    let fx = fixture(&[], vec![live("bare", &["b1"], false)]).await;
    assert_eq!(available_ids(&fx.session, "bare"), vec!["b1"]);
}

/// `live_extension_providers` is what `/login` lists: the live provider, and not a JSON one.
#[tokio::test]
async fn live_extension_providers_lists_only_live_registrations() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    fx.session
        .services()
        .ext_host
        .registry()
        .register_provider(
            ExtensionId::from("acme-ext"),
            "acme",
            serde_json::json!({
                "baseUrl": "http://127.0.0.1:1/v1",
                "api": "openai-completions",
                "models": [{ "id": "acme-fast" }],
            }),
        )
        .unwrap();

    let ids: Vec<String> = fx
        .session
        .live_extension_providers()
        .iter()
        .map(|p| p.id().as_str().to_string())
        .collect();
    assert_eq!(ids, vec![LLAMA]);
}

// ====================================================================================== provider_auth

fn attach(session: &AgentSession) {
    let services = session.services();
    services.host_services.attach_provider_auth(
        Arc::clone(&services.auth),
        Arc::clone(&services.guest_providers),
    );
}

/// `provider_auth` returns the base URL and key the provider's strategy resolves from the stored
/// credential, plus the env overlay and the source — what `/llama` reads
/// (`extensions/llama/index.ts:30-37`).
#[tokio::test]
async fn provider_auth_returns_the_resolved_base_url_and_key() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    attach(&fx.session);
    store_llama_credential(&fx.session, Some("secret"), Some(SERVER)).await;

    let host = &fx.session.services().host_services;
    let result = host
        .provider_auth(LLAMA)
        .await
        .unwrap()
        .expect("configured");
    assert_eq!(result.api_key.as_deref(), Some("secret"));
    assert_eq!(result.base_url.as_deref(), Some("http://127.0.0.1:9/v1"));
    assert_eq!(result.env.get(URL_ENV).map(String::as_str), Some(SERVER));
    assert_eq!(result.source.as_deref(), Some("stored credential"));
}

/// The environment arm resolves too, and the key defaults to the strategy's own.
#[tokio::test]
async fn provider_auth_falls_back_to_the_ambient_environment() {
    let fx = fixture(&[(URL_ENV, SERVER)], vec![live(LLAMA, &["tiny"], true)]).await;
    attach(&fx.session);

    let result = fx
        .session
        .services()
        .host_services
        .provider_auth(LLAMA)
        .await
        .unwrap()
        .expect("configured by the env var");
    assert_eq!(result.api_key.as_deref(), Some("local"));
    assert_eq!(result.base_url.as_deref(), Some("http://127.0.0.1:9/v1"));
    assert_eq!(result.source.as_deref(), Some(URL_ENV));
}

/// A host built directly, with nothing attached: what a `LiveHostServices` answers before the
/// session builder gives it the credential store (the builder always does, see
/// `a_built_session_answers_provider_auth_without_any_attachment`).
fn bare_host() -> crate::LiveHostServices {
    crate::LiveHostServices::new(
        Arc::new(FauxProvider::new()) as Arc<dyn Provider>,
        cyrup_tools::Backend::default().proc,
        std::env::temp_dir(),
    )
}

/// pi's `undefined`: not configured, not registered, and a host with no credential store attached
/// each answer `Ok(None)`.
#[tokio::test]
async fn provider_auth_is_none_when_unconfigured_unknown_or_unattached() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    let host = &fx.session.services().host_services;

    // Nothing attached: the default host's answer, even with a credential on disk.
    store_llama_credential(&fx.session, Some("secret"), Some(SERVER)).await;
    assert!(bare_host().provider_auth(LLAMA).await.unwrap().is_none());

    assert!(host.provider_auth(LLAMA).await.unwrap().is_some());
    assert!(
        host.provider_auth("no-such-provider")
            .await
            .unwrap()
            .is_none()
    );

    fx.session
        .services()
        .auth
        .delete(&ProviderId::from(LLAMA))
        .await
        .unwrap();
    assert!(
        host.provider_auth(LLAMA).await.unwrap().is_none(),
        "no credential and no env var: the strategy says not configured"
    );
}

// ============================================================================ provider_credentials

fn llama() -> ProviderId {
    ProviderId::from(LLAMA)
}

/// The scoped store reads the CURRENT credential on every call — a `/login` after the native took
/// the store is seen — which is what lets a live provider resolve per request instead of baking a
/// key at registration (`ConfigProvider::new` bakes it).
#[tokio::test]
async fn provider_credentials_reads_the_current_credential_per_call() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    attach(&fx.session);
    let host = &fx.session.services().host_services;
    let store = host.provider_credentials(LLAMA).expect("attached");
    assert!(store.read(&llama()).await.unwrap().is_none());

    store_llama_credential(&fx.session, Some("sk-1"), Some(SERVER)).await;
    match store.read(&llama()).await.unwrap().expect("seen at once") {
        Credential::ApiKey { key, env } => {
            assert_eq!(key.as_deref(), Some("sk-1"));
            assert_eq!(
                env.as_ref()
                    .and_then(|e| e.get(URL_ENV))
                    .map(String::as_str),
                Some(SERVER)
            );
        }
        other => panic!("expected an api-key credential, got {other:?}"),
    }

    store_llama_credential(&fx.session, Some("sk-2"), Some(SERVER)).await;
    match store.read(&llama()).await.unwrap().unwrap() {
        Credential::ApiKey { key, .. } => assert_eq!(key.as_deref(), Some("sk-2")),
        other => panic!("expected an api-key credential, got {other:?}"),
    }
}

/// The store answers for its own provider only: another provider's credential is invisible and
/// cannot be modified or deleted through it.
#[tokio::test]
async fn provider_credentials_cannot_touch_another_providers_credential() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    attach(&fx.session);
    let other = ProviderId::from("anthropic");
    fx.session
        .services()
        .auth
        .modify(&other, |_| async {
            Ok(Some(cyrup_config::Credential::ApiKey {
                key: Some("sk-ant".to_string()),
                env: None,
            }))
        })
        .await
        .unwrap();
    store_llama_credential(&fx.session, Some("sk"), Some(SERVER)).await;

    let store = fx
        .session
        .services()
        .host_services
        .provider_credentials(LLAMA)
        .unwrap();
    assert!(store.read(&other).await.unwrap().is_none());
    let listed: Vec<String> = store
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|info| info.provider.as_str().to_string())
        .collect();
    assert_eq!(listed, vec![LLAMA], "the other provider is not listed");
    assert!(
        store
            .modify(
                &other,
                Box::new(|_| Box::pin(async { Ok(Some(Credential::api_key("stolen"))) })),
            )
            .await
            .is_err()
    );
    assert!(store.delete(&other).await.is_err());

    // …and the refused write really did not land.
    match fx
        .session
        .services()
        .auth
        .read(&other)
        .await
        .unwrap()
        .expect("still stored")
    {
        cyrup_config::Credential::ApiKey { key, .. } => assert_eq!(key.as_deref(), Some("sk-ant")),
        other => panic!("expected an api-key credential, got {other:?}"),
    }
}

/// A host with nothing attached grants no store.
#[tokio::test]
async fn provider_credentials_is_none_when_unattached() {
    assert!(bare_host().provider_credentials(LLAMA).is_none());
}

// ============================================================================ availability error

/// A strategy whose `check` the test scripts: it FAILS while `fail` is set, PANICS while `panic` is
/// set, and otherwise answers "configured". When `hold` is armed, the next check samples `fail`,
/// meets the test at `entered`, and waits at `release` before answering — which is how
/// [`a_late_failing_check_cannot_overwrite_a_newer_success`] interleaves two checks without a sleep.
pub(super) struct ScriptedCheck {
    fail: std::sync::atomic::AtomicBool,
    panic: std::sync::atomic::AtomicBool,
    hold: std::sync::atomic::AtomicBool,
    entered: std::sync::Barrier,
    release: std::sync::Barrier,
}

impl ScriptedCheck {
    fn failing() -> Arc<Self> {
        Arc::new(Self {
            fail: std::sync::atomic::AtomicBool::new(true),
            panic: std::sync::atomic::AtomicBool::new(false),
            hold: std::sync::atomic::AtomicBool::new(false),
            entered: std::sync::Barrier::new(2),
            release: std::sync::Barrier::new(2),
        })
    }
    fn set(flag: &std::sync::atomic::AtomicBool, on: bool) {
        flag.store(on, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl ApiKeyAuth for ScriptedCheck {
    fn name(&self) -> &str {
        "scripted"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        use std::sync::atomic::Ordering::SeqCst;
        assert!(!self.panic.load(SeqCst), "scripted strategy panicked");
        let fail = self.fail.load(SeqCst);
        if self.hold.swap(false, SeqCst) {
            // On the check's own thread (`live_provider_is_configured`), so blocking is fine.
            self.entered.wait();
            self.release.wait();
        }
        if fail {
            Err(AuthError::api_key(
                ProviderId::from(LLAMA),
                "server probe refused",
            ))
        } else {
            Ok(Some(AuthCheck {
                auth_type: AuthType::ApiKey,
                source: Some("scripted".to_string()),
            }))
        }
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        Ok(None)
    }
}

fn scripted(check: &Arc<ScriptedCheck>) -> Arc<dyn Provider> {
    Arc::new(LiveProvider {
        id: ProviderId::from(LLAMA),
        auth: Some(ProviderAuth::with_api_key(
            Arc::clone(check) as Arc<dyn ApiKeyAuth>
        )),
        models: models_of(LLAMA, &["tiny"]),
    })
}

/// SEAM-139 Verify: "a strategy whose check errors yields a visible diagnostic". The failing check
/// still reads as unavailable, and the reason — the strategy's own message, with the provider — is
/// now on pi's `getError()` channel as `Availability refresh: …` (`core/model-runtime.ts:510`
/// @v1.1.0), both alone and inside the `getError()` port.
#[tokio::test]
async fn a_failing_live_auth_check_yields_a_visible_diagnostic() {
    let check = ScriptedCheck::failing();
    let fx = fixture(&[], vec![scripted(&check)]).await;
    assert_eq!(fx.session.availability_error(), None, "nothing checked yet");

    assert!(available_ids(&fx.session, LLAMA).is_empty());

    let error = fx
        .session
        .availability_error()
        .expect("the failing check is recorded");
    assert!(error.starts_with("Availability refresh: "), "{error}");
    assert!(error.contains(LLAMA), "{error}");
    assert!(error.contains("server probe refused"), "{error}");
    let runtime_error = fx
        .session
        .model_runtime_error()
        .expect("getError() carries it");
    assert!(runtime_error.contains(&error), "{runtime_error}");
}

/// pi clears `availabilityError` when a later pass succeeds (`core/model-runtime.ts:363`, `:421`
/// @v1.1.0): once the strategy answers again the diagnostic is gone and the provider is available.
#[tokio::test]
async fn a_recovered_live_auth_check_clears_the_diagnostic() {
    let check = ScriptedCheck::failing();
    let fx = fixture(&[], vec![scripted(&check)]).await;
    assert!(available_ids(&fx.session, LLAMA).is_empty());
    assert!(fx.session.availability_error().is_some());

    ScriptedCheck::set(&check.fail, false);

    assert_eq!(available_ids(&fx.session, LLAMA), vec!["tiny"]);
    assert_eq!(fx.session.availability_error(), None);
    assert_eq!(fx.session.model_runtime_error(), None);
}

/// A check that PANICS is recorded too — it used to read as a silent "unavailable".
#[tokio::test]
async fn a_panicking_live_auth_check_yields_a_visible_diagnostic() {
    let check = ScriptedCheck::failing();
    ScriptedCheck::set(&check.panic, true);
    let fx = fixture(&[], vec![scripted(&check)]).await;

    assert!(available_ids(&fx.session, LLAMA).is_empty());

    let error = fx.session.availability_error().expect("recorded");
    assert_eq!(
        error,
        format!("Availability refresh: auth check for {LLAMA} panicked")
    );
}

/// pi's `availabilityErrorSeq` guard (`core/model-runtime.ts:363`, `:373` @v1.1.0): only the
/// LATEST started check may write. Check A starts while the strategy fails and is held; check B
/// starts after it, succeeds and clears; A is then released and fails. A finished last but started
/// first, so its failure must NOT resurrect the diagnostic. Interleaved with barriers, no sleeps.
///
/// Red-proof precondition: A's CALLER must receive the scripted failure, not give up first. If it
/// hit [`LIVE_AUTH_CHECK_TIMEOUT`] while B ran, it would record its timeout BEFORE B cleared, and
/// the final `None` would hold even with the seq guard removed. A's caller starts its wait after
/// `a_started`, so a join inside that window proves it did not time out; past it (only under
/// extreme load) the test fails loudly instead of passing without exercising the guard.
#[tokio::test]
async fn a_late_failing_check_cannot_overwrite_a_newer_success() {
    let check = ScriptedCheck::failing();
    let fx = fixture(&[], vec![scripted(&check)]).await;
    let session = &fx.session;
    ScriptedCheck::set(&check.hold, true);

    std::thread::scope(|scope| {
        let a_started = std::time::Instant::now();
        let a = scope.spawn(|| available_ids(session, LLAMA));
        // A is inside its check, having sampled `fail == true`.
        check.entered.wait();
        ScriptedCheck::set(&check.fail, false);
        assert_eq!(available_ids(session, LLAMA), vec!["tiny"], "B succeeds");
        assert_eq!(session.availability_error(), None, "B cleared");
        check.release.wait();
        assert!(a.join().unwrap().is_empty(), "A failed");
        let a_took = a_started.elapsed();
        assert!(
            a_took < LIVE_AUTH_CHECK_TIMEOUT,
            "A's caller may have timed out ({a_took:?}) before B cleared, so this run could not \
             tell a missing seq guard from a present one; re-run without build load"
        );
    });

    assert_eq!(
        session.availability_error(),
        None,
        "the older check's late failure was dropped"
    );
}

/// A failing provider's record does not outlive the provider: pi's `unregisterProvider` ends in a
/// `refresh({ allowNetwork: false })` whose successful availability pass clears
/// `availabilityError` (`core/model-runtime.ts:944-950`, `:363` @f1b2e77f5). Nothing in cyrup
/// checks a removed id again, so the record is dropped when its provider is no longer live. RED
/// with the prune disabled: `Some("Availability refresh: api key auth failed for llama.cpp: server
/// probe refused")` for `None` after the removal.
#[tokio::test]
async fn an_unregistered_providers_failure_is_no_longer_reported() {
    use cyrup_ext::provider::ModelRegistrySink;

    let check = ScriptedCheck::failing();
    let fx = fixture(&[], vec![scripted(&check)]).await;
    assert!(available_ids(&fx.session, LLAMA).is_empty());
    assert!(fx.session.availability_error().is_some(), "recorded first");

    fx.session.services().guest_providers.remove_provider(LLAMA);

    assert_eq!(fx.session.availability_error(), None);
    assert_eq!(fx.session.model_runtime_error(), None);
}
