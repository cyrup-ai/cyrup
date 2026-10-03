//! How the host treats an extension's LIVE provider once it is registered: a replacement reaches
//! the provider the session streams through, the offline switch reaches its catalog refresh, the
//! startup restore is cheap and bounded, its auth check cannot wedge the availability predicate,
//! and a runtime `--api-key` counts.
//!
//! The providers are the in-memory ones of `live_provider_auth.rs`; **no network** is touched.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, EventStream, ProviderId};
use cyrup_ext::NativeExtension;
use cyrup_ext::provider::ModelRegistrySink;
use cyrup_provider::auth::{ApiKeyAuth, ProviderAuth};
use cyrup_provider::collection::{AuthCheck, AuthType};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{
    AuthContext, AuthError, AuthResult, Context, Credential, Model, ModelAuth, Provider,
    ProviderError, RefreshModelsContext, StreamEvent, StreamOptions,
};
use tempfile::TempDir;

use super::live_provider_auth::{LLAMA, LiveNative, SERVER, URL_ENV, live, models_of};
use crate::{AgentSession, SessionBuilder, SessionConfig};

struct Fx {
    _tmp: TempDir,
    agent_dir: PathBuf,
    session: AgentSession,
}

/// A session over a store whose ambient environment is `env`, with `providers` registered live by
/// a native extension, after `configure` has had its say about the config.
async fn fixture_with(
    env: &[(&str, &str)],
    providers: Vec<Arc<dyn Provider>>,
    configure: impl FnOnce(&mut SessionConfig),
) -> Fx {
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
    let mut cfg = SessionConfig::new(cwd, agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    configure(&mut cfg);
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .auth(auth)
        .with_native_extension(Arc::new(LiveNative(providers)) as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("session builds");
    Fx {
        _tmp: tmp,
        agent_dir,
        session,
    }
}

async fn fixture(env: &[(&str, &str)], providers: Vec<Arc<dyn Provider>>) -> Fx {
    fixture_with(env, providers, |_| {}).await
}

fn llama_ids(models: Vec<Model>) -> Vec<String> {
    models
        .into_iter()
        .filter(|m| m.provider.as_str() == LLAMA)
        .map(|m| m.id.as_str().to_string())
        .collect()
}

// ============================================================================ replacement

/// A provider an extension replaces while it is the installed one (`/llama`, a catalog refresh)
/// replaces the installed provider too. The composed registry starts from the installed provider's
/// models, so a stale installed provider shadowed the replacement for good: its models were never
/// refreshed (the replacement's context windows were skipped as duplicates) and the ones the
/// replacement dropped stayed listed.
///
/// **Red** without `follow_installed`: `old` is still listed and `new` is missing.
#[tokio::test]
async fn a_provider_replaced_while_installed_replaces_the_installed_provider() {
    let fx = fixture(&[(URL_ENV, SERVER)], vec![live(LLAMA, &["old"], true)]).await;
    fx.session.set_model(&format!("{LLAMA}/old")).await.unwrap();
    assert_eq!(llama_ids(fx.session.full_model_catalog()), vec!["old"]);

    fx.session
        .services()
        .guest_providers
        .upsert_live_provider(LLAMA, live(LLAMA, &["new"], true));

    assert_eq!(
        llama_ids(fx.session.full_model_catalog()),
        vec!["new"],
        "the replacement's catalog is the registry's, not the installed provider's"
    );
    assert_eq!(
        llama_ids(fx.session.available_model_catalog()),
        vec!["new"],
        "and the selector offers exactly it"
    );
}

/// A replacement of a provider the session is NOT on leaves the installed provider alone.
#[tokio::test]
async fn replacing_a_provider_the_session_is_not_on_leaves_the_installed_one_alone() {
    let fx = fixture(&[(URL_ENV, SERVER)], vec![live(LLAMA, &["old"], true)]).await;
    let before = fx
        .session
        .available_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() != LLAMA)
        .count();

    fx.session
        .services()
        .guest_providers
        .upsert_live_provider(LLAMA, live(LLAMA, &["new"], true));

    let after = fx
        .session
        .available_model_catalog()
        .into_iter()
        .filter(|m| m.provider.as_str() != LLAMA)
        .count();
    assert_eq!(
        before, after,
        "the faux session provider's models are untouched"
    );
    assert_eq!(llama_ids(fx.session.full_model_catalog()), vec!["new"]);
}

// ================================================================================== offline

/// A live provider whose `refresh_models` records which phase it was asked for.
struct PhaseRecorder {
    id: ProviderId,
    inner: Arc<dyn Provider>,
    phases: Arc<Mutex<Vec<bool>>>,
}

#[async_trait::async_trait]
impl Provider for PhaseRecorder {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        self.inner.models()
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        self.inner.provider_auth()
    }
    fn has_refresh_models(&self) -> bool {
        true
    }
    async fn refresh_models(
        &self,
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        self.phases.lock().unwrap().push(ctx.allow_network);
        Some(Ok(()))
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

fn recording_provider(phases: &Arc<Mutex<Vec<bool>>>) -> Arc<dyn Provider> {
    Arc::new(PhaseRecorder {
        id: ProviderId::from(LLAMA),
        inner: live(LLAMA, &["tiny"], true),
        phases: Arc::clone(phases),
    })
}

/// `/login llama.cpp` refreshes the provider it just authenticated without naming `allowNetwork`,
/// so the registry's switch decides (`options.allowNetwork ?? this.modelNetworkEnabled`,
/// `core/model-runtime.ts:850`). An offline session must make no network phase: only the
/// cache-only restore runs. The two directions are asserted together, so a switch that is never
/// applied reads the same in both.
///
/// **Red** if the builder does not apply `SessionConfig::model_network_enabled`: the offline
/// session then runs the network phase too.
#[tokio::test]
async fn an_offline_session_makes_no_network_phase_on_the_login_refresh() {
    let refresh_phases = |enabled: bool| async move {
        let phases: Arc<Mutex<Vec<bool>>> = Arc::default();
        let fx = fixture_with(
            &[(URL_ENV, SERVER)],
            vec![recording_provider(&phases)],
            move |cfg| cfg.model_network_enabled = enabled,
        )
        .await;
        // The session's own build-time restore ran, cache only.
        phases.lock().unwrap().clear();
        let result = fx
            .session
            .refresh_provider_catalog(CancelToken::new(), LLAMA)
            .await;
        assert!(result.errors.is_empty(), "{result:?}");
        let recorded = phases.lock().unwrap();
        recorded.clone()
    };

    assert_eq!(
        refresh_phases(false).await,
        vec![false],
        "offline: only the cache-only restore"
    );
    assert_eq!(
        refresh_phases(true).await,
        vec![false, true],
        "online: the restore, then the network phase"
    );
}

// ======================================================================== startup restore

/// A session that stores no catalog never creates `models-store.json` nor its `.lock` sidecar:
/// every build restores the extension providers' stored catalogs, and the sidecar is never
/// unlinked, so a reader that locks a file that is not there would leave one behind in every agent
/// directory, and make concurrent subagent children contend on it at start.
#[tokio::test]
async fn building_a_session_that_stores_no_catalog_creates_no_store_file() {
    let fx = fixture(&[(URL_ENV, SERVER)], vec![live(LLAMA, &["tiny"], true)]).await;

    assert!(!fx.agent_dir.join("models-store.json").exists());
    assert!(
        !fx.agent_dir.join("models-store.json.lock").exists(),
        "a read of a store that is not there must not lock it"
    );
}

/// A provider whose `refresh_models` never answers and ignores the cancellation (a bug in a native
/// extension, in a phase it was told has no network) must not stall session construction: the
/// restore is bounded and the session starts with what it has.
///
/// **Red** without the bound: `build` waits for the provider forever.
#[tokio::test]
async fn a_restore_that_never_answers_does_not_stall_the_build() {
    struct Wedged {
        id: ProviderId,
        inner: Arc<dyn Provider>,
    }

    #[async_trait::async_trait]
    impl Provider for Wedged {
        fn id(&self) -> &ProviderId {
            &self.id
        }
        fn models(&self) -> &[Model] {
            self.inner.models()
        }
        fn has_refresh_models(&self) -> bool {
            true
        }
        async fn refresh_models(
            &self,
            _ctx: &RefreshModelsContext,
        ) -> Option<Result<(), ProviderError>> {
            std::future::pending().await
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

    let wedged: Arc<dyn Provider> = Arc::new(Wedged {
        id: ProviderId::from(LLAMA),
        inner: live(LLAMA, &["tiny"], false),
    });
    let built = tokio::time::timeout(
        Duration::from_secs(20),
        fixture_with(&[], vec![wedged], |cfg| {
            cfg.provider_restore_timeout = Duration::from_millis(200);
        }),
    )
    .await;

    let fx = built.expect("the build did not wait for the wedged provider");
    assert_eq!(llama_ids(fx.session.full_model_catalog()), vec!["tiny"]);
}

// ============================================================================== auth check

/// A native extension whose provider's strategy awaits a timer in `check` (nothing in the trait
/// forbids it; llama.cpp's own does not).
struct TimerCheck;

#[async_trait::async_trait]
impl ApiKeyAuth for TimerCheck {
    fn name(&self) -> &str {
        "timer check"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn AuthContext,
        _cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(Some(AuthCheck {
            auth_type: AuthType::ApiKey,
            source: Some("timer".to_string()),
        }))
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

fn timer_provider() -> Arc<dyn Provider> {
    Arc::new(super::live_provider_auth::LiveProvider {
        id: ProviderId::from(LLAMA),
        auth: Some(ProviderAuth::with_api_key(Arc::new(TimerCheck))),
        models: models_of(LLAMA, &["tiny"]),
    })
}

/// The availability predicate is synchronous, so a provider's async check cannot be driven by
/// parking the calling thread on it. Asked from a thread with no runtime at all, a check that needs
/// a timer fails (there is no reactor to drive it): that reads as "unavailable", as a failing check
/// does in pi, and does not take the caller down with it.
///
/// **Red** with `block_on` on the calling thread: the check panics ("no reactor running") on it.
#[tokio::test]
async fn a_live_auth_check_that_cannot_run_reads_as_unavailable_not_as_a_panic() {
    let fx = fixture(&[], vec![timer_provider()]).await;
    let session = Arc::new(fx.session);

    let answered = std::thread::spawn({
        let session = Arc::clone(&session);
        move || llama_ids(session.available_model_catalog())
    })
    .join();

    assert_eq!(
        answered.expect("the predicate did not panic the caller"),
        Vec::<String>::new(),
        "a check that could not run leaves the provider unavailable"
    );
}

/// On a multi-thread runtime the same check works: it runs on its own thread inside the runtime's
/// context, so the timer it awaits is driven by the runtime's other threads and it answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_auth_check_that_awaits_a_timer_answers_on_a_multi_thread_runtime() {
    let fx = fixture(&[], vec![timer_provider()]).await;
    let session = Arc::new(fx.session);

    let listed = tokio::task::spawn_blocking({
        let session = Arc::clone(&session);
        move || llama_ids(session.available_model_catalog())
    })
    .await
    .unwrap();

    assert_eq!(listed, vec!["tiny"]);
}

/// A check that never answers does not hold the predicate forever: after the bound the provider
/// reads as unavailable.
#[tokio::test]
async fn a_live_auth_check_that_never_answers_reads_as_unavailable() {
    struct Silent;

    #[async_trait::async_trait]
    impl ApiKeyAuth for Silent {
        fn name(&self) -> &str {
            "silent"
        }
        fn supports_check(&self) -> bool {
            true
        }
        async fn check(
            &self,
            _ctx: &dyn AuthContext,
            _cred: Option<&Credential>,
        ) -> Result<Option<AuthCheck>, AuthError> {
            std::future::pending().await
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

    let provider: Arc<dyn Provider> = Arc::new(super::live_provider_auth::LiveProvider {
        id: ProviderId::from(LLAMA),
        auth: Some(ProviderAuth::with_api_key(Arc::new(Silent))),
        models: models_of(LLAMA, &["tiny"]),
    });
    let fx = fixture(&[], vec![provider]).await;
    let session = Arc::new(fx.session);

    let started = std::time::Instant::now();
    let listed = tokio::task::spawn_blocking({
        let session = Arc::clone(&session);
        move || llama_ids(session.available_model_catalog())
    })
    .await
    .unwrap();

    assert!(listed.is_empty());
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "bounded, not forever"
    );
}

// ============================================================================ runtime key

/// A strategy with no `check` that resolves exactly when a credential with a key exists.
struct KeyOnly;

#[async_trait::async_trait]
impl ApiKeyAuth for KeyOnly {
    fn name(&self) -> &str {
        "key only"
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        Ok(match cred {
            Some(Credential::ApiKey { key: Some(key), .. }) => Some(AuthResult {
                auth: ModelAuth {
                    api_key: Some(key.clone()),
                    headers: None,
                    base_url: None,
                },
                env: None,
                source: Some("key".to_string()),
            }),
            _ => None,
        })
    }
}

/// A runtime `--api-key` is a credential for the provider's strategy (pi wraps the store in
/// `RuntimeCredentials`), so a live provider whose strategy resolves from a key is available once
/// one is given, as the login status row already says.
///
/// **Red** if the check reads only the file snapshot: the provider stays unavailable.
#[tokio::test]
async fn a_runtime_api_key_makes_a_key_strategy_provider_available() {
    let provider: Arc<dyn Provider> = Arc::new(super::live_provider_auth::LiveProvider {
        id: ProviderId::from(LLAMA),
        auth: Some(ProviderAuth::with_api_key(Arc::new(KeyOnly))),
        models: models_of(LLAMA, &["tiny"]),
    });
    let fx = fixture(&[], vec![provider]).await;
    assert!(
        llama_ids(fx.session.available_model_catalog()).is_empty(),
        "no key, not available"
    );

    fx.session
        .services()
        .auth
        .set_runtime_api_key(ProviderId::from(LLAMA), "cli-key".to_string());

    assert_eq!(
        llama_ids(fx.session.available_model_catalog()),
        vec!["tiny"]
    );
}

/// The ordinary llama.cpp strategy is unaffected: a runtime key carries no server URL, so the
/// strategy that wants one still says "not configured" without it.
#[tokio::test]
async fn a_runtime_api_key_does_not_configure_a_strategy_that_needs_a_server_url() {
    let fx = fixture(&[], vec![live(LLAMA, &["tiny"], true)]).await;
    fx.session
        .services()
        .auth
        .set_runtime_api_key(ProviderId::from(LLAMA), "cli-key".to_string());

    assert!(llama_ids(fx.session.available_model_catalog()).is_empty());
}
