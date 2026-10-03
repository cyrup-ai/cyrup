//! EXT-027 — what the PRODUCTION session builder wires around an extension-registered provider.
//!
//! The pieces (`GuestProviderRegistry::attach_models_store` / `attach_refresh_auth` /
//! `restore_cached`, `LiveHostServices::attach_provider_refresher` / `attach_provider_auth`) were
//! each tested on their own and attached by hand; no production code attached any of them, so a
//! built session never read `models-store.json`, never resolved a credential for the refresh engine,
//! and answered a native's `provider_auth` / `refresh_provider` with "nothing" / "no refresh
//! backend". These tests build the session through [`SessionBuilder::build`] and attach NOTHING.
//!
//! pi: `createAgentSessionServices` registers the extensions' providers and then runs
//! `modelRuntime.refresh({ allowNetwork: false })` (`core/agent-session-services.ts:190-206`
//! @v0.99.2-17); the runtime owns the models store, the credential store and the auth context the
//! refresh reads (`core/model-runtime.ts`). `--model` is resolved after that
//! (`main.ts` `buildSessionOptions`), so a model only an extension's provider offers is selectable
//! at launch.
//!
//! **No network.** The providers are scripted in memory; "network" is a phase flag.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, EventStream, ExtensionId, ModelThinkingLevel, ProviderId};
use cyrup_ext::host::HostServices;
use cyrup_ext::host::services::{ModelsPersist, ModelsPublication};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::collection::{AuthCheck, AuthType};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{
    ApiKeyAuth, AuthContext, AuthError, AuthResult, ClassifierModel, Context, Credential, Modality,
    Model, ModelAuth, ModelCost, ModelsStoreEntry, Provider, ProviderAuth, ProviderError,
    RefreshModelsContext, StreamEvent, StreamOptions,
};
use tempfile::TempDir;

use crate::provider_swap::ProviderResolver;
use crate::{AgentSession, SessionBuilder, SessionConfig, SessionServiceError};

const LLAMA: &str = "llama.cpp";
const SERVER: &str = "http://127.0.0.1:9";

// --------------------------------------------------------------------------- the scripted provider

/// What one `refresh_models` call was handed.
#[derive(Clone, Debug)]
struct Seen {
    allow_network: bool,
    credential: Option<Credential>,
    stored_ids: Option<Vec<String>>,
}

/// An API-key strategy that is always configured, with the credential's key and env passed through
/// (the shape of pi's llama.cpp strategy once a server is known, `provider.ts:187-196`).
struct PassThrough;

#[async_trait::async_trait]
impl ApiKeyAuth for PassThrough {
    fn name(&self) -> &str {
        "scripted"
    }
    fn supports_check(&self) -> bool {
        true
    }
    async fn check(
        &self,
        _ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthCheck>, AuthError> {
        Ok(Some(AuthCheck {
            auth_type: AuthType::ApiKey,
            source: cred.map(|_| "stored credential".to_string()),
        }))
    }
    async fn resolve(
        &self,
        _model: &Model,
        _ctx: &dyn AuthContext,
        cred: Option<&Credential>,
    ) -> Result<Option<AuthResult>, AuthError> {
        let key = match cred {
            Some(Credential::ApiKey { key, .. }) => key.clone(),
            _ => None,
        };
        Ok(Some(AuthResult {
            auth: ModelAuth {
                api_key: key,
                headers: None,
                base_url: Some(format!("{SERVER}/v1")),
            },
            env: cred.and_then(Credential::env).cloned(),
            source: Some("stored credential".to_string()),
        }))
    }
}

/// A live provider whose `refresh_models` records what it saw, and (in its network phase) persists
/// `fresh` through the host's publisher, as the llama.cpp provider persists a refreshed catalog
/// (`provider.ts:251-258`).
struct Scripted {
    id: ProviderId,
    models: Vec<Model>,
    auth: ProviderAuth,
    seen: Arc<Mutex<Vec<Seen>>>,
    fresh: Option<(Vec<Model>, Vec<ClassifierModel>)>,
}

#[async_trait::async_trait]
impl Provider for Scripted {
    fn id(&self) -> &ProviderId {
        &self.id
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
        ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        let context = ctx;
        self.seen.lock().unwrap().push(Seen {
            allow_network: ctx.allow_network,
            credential: context.credential.clone(),
            stored_ids: context.stored.as_ref().map(|entry| {
                entry
                    .models
                    .iter()
                    .map(|m| m.id.as_str().to_string())
                    .collect()
            }),
        });
        if ctx.allow_network
            && let Some((models, classifiers)) = self.fresh.clone()
        {
            let published = context
                .publish(ModelsPublication {
                    persist: Some(ModelsPersist::Write {
                        entry: ModelsStoreEntry {
                            models,
                            checked_at: Some(11),
                            ..ModelsStoreEntry::default()
                        },
                        classifiers,
                    }),
                    update: None,
                })
                .await;
            return Some(published.map(|_| ()));
        }
        Some(Ok(()))
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

fn model(id: &str, reasoning: bool) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: LLAMA.into(),
        base_url: format!("{SERVER}/v1"),
        reasoning,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 4096,
        max_tokens: 4096,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

fn classifier(id: &str) -> ClassifierModel {
    ClassifierModel {
        id: id.into(),
        name: id.to_string(),
        api: "llama-cpp-classify".into(),
        provider: LLAMA.into(),
        base_url: SERVER.to_string(),
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        headers: None,
        context_window: 4096,
    }
}

struct Probe {
    provider: Arc<Scripted>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn probe(models: Vec<Model>, fresh: Option<(Vec<Model>, Vec<ClassifierModel>)>) -> Probe {
    let seen = Arc::new(Mutex::new(Vec::new()));
    Probe {
        provider: Arc::new(Scripted {
            id: LLAMA.into(),
            models,
            auth: ProviderAuth::with_api_key(Arc::new(PassThrough)),
            seen: Arc::clone(&seen),
            fresh,
        }),
        seen,
    }
}

struct Registers(Arc<dyn Provider>);

#[async_trait::async_trait]
impl NativeExtension for Registers {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("llama-ext")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_provider_live(self.0.id().as_str(), Arc::clone(&self.0));
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

// ------------------------------------------------------------------------------------- the fixture

struct Fx {
    _tmp: TempDir,
    agent_dir: PathBuf,
}

fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    std::fs::create_dir_all(tmp.path().join("project")).unwrap();
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fx {
        _tmp: tmp,
        agent_dir,
    }
}

impl Fx {
    fn config(&self) -> SessionConfig {
        let mut cfg = SessionConfig::new(self._tmp.path().join("project"), self.agent_dir.clone());
        cfg.trust_override = Some(true);
        cfg.no_extensions = true;
        cfg
    }

    fn builder(&self, cfg: SessionConfig, provider: Arc<dyn Provider>) -> SessionBuilder {
        let auth = Arc::new(cyrup_config::AuthStore::at(
            self.agent_dir.join("auth.json"),
        ));
        SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
            .auth(auth)
            .with_native_extension(Arc::new(Registers(provider)) as Arc<dyn NativeExtension>)
    }

    async fn build(&self, provider: Arc<dyn Provider>) -> AgentSession {
        self.builder(self.config(), provider)
            .build()
            .await
            .expect("session builds")
    }

    /// `/login llama.cpp`'s result: `{ type: "api_key", key, env: { LLAMA_BASE_URL } }`
    /// (`provider.ts:175-179`).
    fn store_credential(&self, key: &str) {
        let mut env = BTreeMap::new();
        env.insert("LLAMA_BASE_URL".to_string(), SERVER.to_string());
        std::fs::write(
            self.agent_dir.join("auth.json"),
            serde_json::to_string(&serde_json::json!({
                LLAMA: { "type": "api_key", "key": key, "env": env }
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// `<agent_dir>/models-store.json` as a completed refresh leaves it.
    fn seed_store(&self, ids: &[&str]) {
        std::fs::write(
            self.agent_dir.join("models-store.json"),
            serde_json::to_string(&serde_json::json!({
                LLAMA: {
                    "models": ids.iter().map(|id| model(id, false)).collect::<Vec<_>>(),
                    "checkedAt": 7,
                }
            }))
            .unwrap(),
        )
        .unwrap();
    }
}

// ===================================================================================== the restore

/// The startup restore (`agent-session-services.ts:190-206`): building the session hands each
/// extension provider its PERSISTED catalog, read from `<agent_dir>/models-store.json`, with no
/// network — even though a credential is stored, which is what a network phase would resolve.
#[tokio::test]
async fn building_a_session_restores_the_persisted_catalog_without_the_network() {
    let fx = fixture();
    fx.seed_store(&["cached-a", "cached-b"]);
    fx.store_credential("sk-stored");
    let probe = probe(Vec::new(), None);

    let _session = fx.build(probe.provider.clone()).await;

    let seen = probe.seen.lock().unwrap().clone();
    assert_eq!(
        seen.len(),
        1,
        "exactly the cache-only restore ran, no network phase: {seen:?}"
    );
    assert!(!seen[0].allow_network);
    assert_eq!(
        seen[0].stored_ids.as_deref(),
        Some(&["cached-a".to_string(), "cached-b".to_string()][..]),
        "the entry came out of the session's `models-store.json`"
    );
}

// ============================================================================ the refresh engine

/// A refresh over the built session resolves the provider's credential from the SESSION's store (the
/// refresh engine's `credentials` / `authContext`, pi `resolveRefreshCredential`, `models.ts:608-635`),
/// and what the provider publishes lands in the session's `models-store.json`: the chat half and the
/// classifier half in one `models` array (`provider.ts:251-253`).
#[tokio::test]
async fn a_refresh_over_a_built_session_uses_its_credential_store_and_persists_to_its_models_store()
{
    let fx = fixture();
    fx.store_credential("sk-stored");
    let probe = probe(
        Vec::new(),
        Some((vec![model("fresh", false)], vec![classifier("fresh")])),
    );
    let session = fx.build(probe.provider.clone()).await;

    let result = session
        .services()
        .guest_providers
        .refresh_all(CancelToken::new())
        .await;

    assert!(result.is_clean(), "{result:?}");
    let seen = probe.seen.lock().unwrap().clone();
    let network = seen
        .iter()
        .find(|s| s.allow_network)
        .unwrap_or_else(|| panic!("no network phase ran, so no credential resolved: {seen:?}"));
    match network.credential.as_ref().expect("a resolved credential") {
        Credential::ApiKey { key, env } => {
            assert_eq!(key.as_deref(), Some("sk-stored"));
            assert_eq!(
                env.as_ref()
                    .and_then(|env| env.get("LLAMA_BASE_URL"))
                    .map(String::as_str),
                Some(SERVER)
            );
        }
        other => panic!("expected an api-key credential, got {other:?}"),
    }
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(fx.agent_dir.join("models-store.json"))
            .expect("the refresh wrote the session's models store"),
    )
    .unwrap();
    let members = stored[LLAMA]["models"].as_array().unwrap();
    let kinds: Vec<Option<&str>> = members.iter().map(|m| m["type"].as_str()).collect();
    assert_eq!(kinds, [None, Some("classifier")], "{stored}");
    assert_eq!(stored[LLAMA]["checkedAt"], 11);
}

/// The host verb `/llama` calls (`ctx.modelRegistry.refresh`, `index.ts:54-60`) reaches the same
/// engine: without the attachment it answers pi's "no refresh backend" error and nothing runs.
#[tokio::test]
async fn the_refresh_provider_verb_of_a_built_session_drives_the_engine() {
    let fx = fixture();
    fx.store_credential("sk-stored");
    let probe = probe(Vec::new(), None);
    let session = fx.build(probe.provider.clone()).await;
    probe.seen.lock().unwrap().clear();

    let result = session
        .services()
        .host_services
        .refresh_provider(LLAMA, true, false, CancelToken::new())
        .await;

    assert!(result.is_clean(), "{result:?}");
    assert!(
        probe.seen.lock().unwrap().iter().any(|s| s.allow_network),
        "the verb ran the provider's network phase"
    );
}

// ================================================================================ the auth verbs

/// `getProviderAuth` (`index.ts:30`) answers from the built session's credential store.
#[tokio::test]
async fn a_built_session_answers_provider_auth_without_any_attachment() {
    let fx = fixture();
    fx.store_credential("sk-stored");
    let probe = probe(Vec::new(), None);
    let session = fx.build(probe.provider.clone()).await;

    let auth = session
        .services()
        .host_services
        .provider_auth(LLAMA)
        .await
        .unwrap()
        .expect("configured");

    assert_eq!(auth.api_key.as_deref(), Some("sk-stored"));
    assert_eq!(
        auth.env.get("LLAMA_BASE_URL").map(String::as_str),
        Some(SERVER)
    );
}

/// The store a native streams with is granted by the built session, and reads the CURRENT
/// credential: one stored after the session was built is seen.
#[tokio::test]
async fn a_built_session_grants_a_credential_store_that_sees_a_later_login() {
    let fx = fixture();
    let probe = probe(Vec::new(), None);
    let session = fx.build(probe.provider.clone()).await;
    let store = session
        .services()
        .host_services
        .provider_credentials(LLAMA)
        .expect("the builder attached the credential source");
    assert!(store.read(&LLAMA.into()).await.unwrap().is_none());

    session
        .services()
        .auth
        .modify(&LLAMA.into(), |_| async {
            Ok(Some(cyrup_config::Credential::api_key("sk-later")))
        })
        .await
        .unwrap();

    match store
        .read(&LLAMA.into())
        .await
        .unwrap()
        .expect("seen at once")
    {
        Credential::ApiKey { key, .. } => assert_eq!(key.as_deref(), Some("sk-later")),
        other => panic!("expected an api-key credential, got {other:?}"),
    }
}

// ============================================================ the post-login scoped catalog refresh

/// A live provider whose every `refresh_models` phase fails, so the scoped refresh has a provider
/// error to report.
struct Failing {
    id: ProviderId,
}

#[async_trait::async_trait]
impl Provider for Failing {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &[]
    }
    fn has_refresh_models(&self) -> bool {
        true
    }
    async fn refresh_models(
        &self,
        _ctx: &RefreshModelsContext,
    ) -> Option<Result<(), ProviderError>> {
        Some(Err(ProviderError::ModelSource("server unreachable".into())))
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

/// `AgentSession::refresh_provider_catalog` (the `/login` continuation, pi `modelRuntime.refresh({
/// providers: [id], signal })`, `interactive-mode.ts:5953`) for a provider a native registered LIVE
/// reaches ITS `refresh_models` through the guest registry's engine, and — passing no
/// `allow_network`, as pi does — lets the registry's network switch decide.
///
/// **Red without the live arm:** the call falls through to the pi.dev catalog service (none is wired
/// on a fixture session), returns a clean no-op, and the provider is never asked. **Red with
/// `allow_network` forced `false`** (or `true`): the first call runs no network phase (or the
/// offline one does).
#[tokio::test]
async fn the_post_login_refresh_of_a_live_provider_runs_its_network_phase_unless_offline() {
    let fx = fixture();
    fx.store_credential("sk-stored");
    let probe = probe(Vec::new(), None);
    let session = fx.build(probe.provider.clone()).await;

    probe.seen.lock().unwrap().clear();
    let online = session
        .refresh_provider_catalog(CancelToken::new(), LLAMA)
        .await;
    assert!(online.is_clean(), "{online:?}");
    let seen = probe.seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|s| !s.allow_network),
        "the cache-only restore phase ran: {seen:?}"
    );
    assert!(
        seen.iter().any(|s| s.allow_network),
        "the network phase ran with the registry's switch on: {seen:?}"
    );

    session
        .services()
        .guest_providers
        .set_network_enabled(false);
    probe.seen.lock().unwrap().clear();
    let offline = session
        .refresh_provider_catalog(CancelToken::new(), LLAMA)
        .await;
    assert!(offline.is_clean(), "{offline:?}");
    let seen = probe.seen.lock().unwrap().clone();
    assert!(
        !seen.is_empty() && seen.iter().all(|s| !s.allow_network),
        "offline: the restore ran and the network did not: {seen:?}"
    );
}

/// The caller's own token having fired is reported as `timed_out` (the login continuation prints
/// pi's `timed out` warning from it), not as an error, and the provider is never asked.
///
/// **Red** if the engine's `aborted` is not folded into `timed_out`.
#[tokio::test]
async fn the_post_login_refresh_reports_the_callers_abort_as_a_timeout() {
    let fx = fixture();
    fx.store_credential("sk-stored");
    let probe = probe(Vec::new(), None);
    let session = fx.build(probe.provider.clone()).await;
    probe.seen.lock().unwrap().clear();

    let cancel = CancelToken::new();
    cancel.cancel();
    let result = session.refresh_provider_catalog(cancel, LLAMA).await;

    assert!(result.timed_out, "{result:?}");
    assert!(!result.aborted, "{result:?}");
    assert!(result.errors.is_empty(), "{result:?}");
    assert!(probe.seen.lock().unwrap().is_empty());
}

/// A live provider's refresh failure comes back keyed by provider id, rendered, for the login
/// continuation to print — and is not a timeout.
///
/// **Red** if the engine's per-provider errors are dropped when folded into the result.
#[tokio::test]
async fn the_post_login_refresh_reports_a_live_providers_failure_by_id() {
    let fx = fixture();
    let failing: Arc<dyn Provider> = Arc::new(Failing { id: LLAMA.into() });
    let session = fx.build(failing).await;

    let result = session
        .refresh_provider_catalog(CancelToken::new(), LLAMA)
        .await;

    assert!(!result.timed_out, "{result:?}");
    let message = result
        .errors
        .get(LLAMA)
        .unwrap_or_else(|| panic!("the failure is reported under the provider id: {result:?}"));
    assert!(message.contains("server unreachable"), "{message}");
}

// ====================================================================== a `--model` an extension owns

/// A resolver that knows no provider, with the text the launch path's own selection gives.
struct UnknownProvider;

impl ProviderResolver for UnknownProvider {
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        Err(format!(
            "model targets provider '{provider_id}', which is not a known provider."
        ))
    }
}

fn with_pattern(fx: &Fx, pattern: &str) -> SessionConfig {
    let mut cfg = fx.config();
    cfg.model_pattern = Some(pattern.to_string());
    cfg
}

/// `--model llama.cpp/qwen3` when only the extension's provider offers it: the session starts on
/// that model and streams through that provider. The injected provider (a different one) cannot
/// resolve the pattern, which used to fail the build with `ModelNotFound`; pi resolves `--model`
/// after the extensions' providers are registered (`agent-session-services.ts:190-206`).
#[tokio::test]
async fn a_model_only_an_extension_provider_offers_is_selected_at_launch() {
    let fx = fixture();
    let probe = probe(vec![model("qwen3", false)], None);

    let session = fx
        .builder(with_pattern(&fx, "llama.cpp/qwen3"), probe.provider.clone())
        .build()
        .await
        .expect("the extension's model resolves");

    let selected = session.model().expect("a model is selected");
    assert_eq!(selected.provider.as_str(), LLAMA);
    assert_eq!(selected.model.as_str(), "qwen3");
    assert_eq!(
        session.provider_swap().current().id().as_str(),
        LLAMA,
        "the session streams through the provider that owns the model"
    );
    assert!(session.model_fallback_message().is_none());
}

/// The thinking level of the launch (`--thinking`) is resolved against the extension's model, not
/// against the model the session was seeded with before the extension's provider existed.
#[tokio::test]
async fn a_deferred_model_takes_the_requested_thinking_level() {
    let fx = fixture();
    let probe = probe(vec![model("qwen3", true)], None);
    let mut cfg = with_pattern(&fx, "llama.cpp/qwen3");
    cfg.thinking_level = Some(ModelThinkingLevel::Medium);

    let session = fx
        .builder(cfg, probe.provider.clone())
        .build()
        .await
        .expect("builds");

    assert_eq!(session.thinking_level().await, ModelThinkingLevel::Medium);
}

/// An extension provider that owns one model and streams through a scripted faux provider, so a
/// session started on it can run a real tool-call round trip.
struct FauxBacked {
    id: ProviderId,
    models: Vec<Model>,
    faux: Arc<FauxProvider>,
}

#[async_trait::async_trait]
impl Provider for FauxBacked {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.faux.stream(model, context, options)
    }
}

/// The `bash` tool's `CYRUP_*` environment is re-pushed for the model the deferred `--model`
/// resolves to: a command run in the launched session sees the extension's provider, its model and
/// the requested reasoning level, not the seeds taken before the extension's provider existed.
///
/// **Red** if the deferred resolution does not republish the model pair or the reasoning level to the
/// bash environment handle.
#[tokio::test]
async fn a_deferred_model_is_published_to_bash_children() {
    use cyrup_core::StopReason;
    use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call};

    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let owner: Arc<dyn Provider> = Arc::new(FauxBacked {
        id: LLAMA.into(),
        models: vec![model("qwen3", true)],
        faux: Arc::clone(&faux),
    });
    let mut cfg = with_pattern(&fx, "llama.cpp/qwen3");
    cfg.thinking_level = Some(ModelThinkingLevel::Medium);
    let session = fx.builder(cfg, owner).build().await.expect("builds");

    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call(
                "bash",
                serde_json::json!({
                    "command": "printf '%s|%s|%s' \"${CYRUP_PROVIDER-}\" \"${CYRUP_MODEL-}\" \"${CYRUP_REASONING_LEVEL-}\" > env.txt"
                }),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let session = Arc::new(session);
    let _ = session
        .prompt(crate::UserInput::text("probe", crate::InputSource::Sdk))
        .await
        .expect("prompt");
    session.wait_for_idle().await;

    let written = std::fs::read_to_string(fx._tmp.path().join("project").join("env.txt"))
        .expect("the bash child ran and wrote its environment");
    assert_eq!(written, "llama.cpp|qwen3|medium");
}

/// A custom id on a provider an extension registered resolves the way `resolveCliModel`'s fallback
/// does for a built-in (`model-resolver.ts:475-501`): the provider is known, so the id is taken.
#[tokio::test]
async fn a_custom_id_on_an_extension_provider_is_accepted() {
    let fx = fixture();
    let probe = probe(vec![model("qwen3", false)], None);

    let session = fx
        .builder(
            with_pattern(&fx, "llama.cpp/not-listed"),
            probe.provider.clone(),
        )
        .build()
        .await
        .expect("a custom id on a known provider resolves");

    let selected = session.model().expect("a model is selected");
    assert_eq!(selected.provider.as_str(), LLAMA);
    assert_eq!(selected.model.as_str(), "not-listed");
}

/// Nothing registers the provider the pattern names: the build fails with the message the launch
/// path's provider resolver gives for an unknown provider, and still as `ModelNotFound`.
#[tokio::test]
async fn a_model_no_provider_offers_still_fails_the_build_with_the_resolvers_reason() {
    let fx = fixture();
    let probe = probe(vec![model("qwen3", false)], None);

    let outcome = fx
        .builder(with_pattern(&fx, "nowhere/ghost"), probe.provider.clone())
        .provider_resolver(Arc::new(UnknownProvider))
        .build()
        .await;

    match outcome {
        Err(SessionServiceError::ModelNotFound(message)) => {
            assert!(message.contains("nowhere/ghost"), "{message}");
            assert!(
                message.contains("'nowhere', which is not a known provider"),
                "{message}"
            );
        }
        Err(other) => panic!("expected ModelNotFound, got {other}"),
        Ok(_) => panic!("a pattern nothing resolves must fail the build"),
    }
}

/// With no resolver either, the failure is the plain `ModelNotFound(pattern)` it always was.
#[tokio::test]
async fn an_unresolvable_model_without_a_resolver_is_model_not_found() {
    let fx = fixture();
    let probe = probe(vec![model("qwen3", false)], None);

    let outcome = fx
        .builder(with_pattern(&fx, "ghost-model"), probe.provider.clone())
        .build()
        .await;

    assert!(
        matches!(outcome, Err(SessionServiceError::ModelNotFound(ref pattern)) if pattern == "ghost-model"),
        "{:?}",
        outcome.err().map(|e| e.to_string())
    );
}
