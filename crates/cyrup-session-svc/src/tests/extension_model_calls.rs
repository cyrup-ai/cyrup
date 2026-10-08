//! EXT-086 — an extension's model call through the session's providers, at the SESSION seam: pi
//! `ctx.modelRegistry.complete()` / `stream()` / `streamSimple()` (`core/model-registry.ts`
//! @v1.0.4, delegating to `ModelRuntime`, `core/model-runtime.ts:691-741`).
//!
//! Everything here goes through [`HostServices::model_stream`] / [`HostServices::model_complete`]
//! on the session's own `LiveHostServices` — the exact `Arc` a native extension is handed through
//! `set_host_services`, and the backend a WASM guest's `models.*` imports call. The headline test
//! drives it from a native extension's command handler, which is the native tier end to end; the
//! guest tier is proven live in `cyrup-it` (`session_svc::wasm_model_calls`).
//!
//! The provider is a RECORDING one, so each assertion is on what the provider was actually asked
//! for — which model, which output cap, which reasoning level, which headers, and whether any of the
//! agent's per-request hooks rode along.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, EventStream, ExtensionId, Message, ModelThinkingLevel, ProviderId,
    StopReason,
};
use cyrup_ext::host::{ModelCall, ModelCallOptions, ModelCallVerb};
use cyrup_ext::{
    CommandDescriptor, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text};
use cyrup_provider::{
    Context, Modality, Model, ModelCost, ModelRoute, ModelRouteError, ModelRouteReason,
    ModelRouteRequest, ModelRouter, Provider, StreamEvent, StreamOptions, VirtualModelDefinition,
    VirtualModelSpec,
};
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

// ----------------------------------------------------------------------------- fixtures ----

/// One request as the provider saw it.
#[derive(Clone, Debug, PartialEq)]
struct Seen {
    provider: String,
    model: String,
    max_tokens: Option<u64>,
    reasoning: ModelThinkingLevel,
    caller_header: Option<String>,
    /// Whether any of the AGENT's per-request hooks (`on_payload` = `before_provider_request`,
    /// `transform_headers` = `before_provider_headers` + attribution, `on_response` =
    /// `after_provider_response`) reached the provider.
    agent_hooks: bool,
    session_id: Option<String>,
}

/// A provider that records every request and answers `"<id> says hi"`.
struct Recording {
    id: ProviderId,
    models: Vec<Model>,
    seen: Arc<Mutex<Vec<Seen>>>,
}

fn model(provider: &str, id: &str, max_tokens: u64) -> Model {
    Model {
        id: id.into(),
        name: id.to_string(),
        api: "openai-completions".into(),
        provider: ProviderId::from(provider),
        base_url: "https://example.invalid".into(),
        reasoning: true,
        input: vec![Modality::Text],
        cost: ModelCost::default(),
        context_window: 200_000,
        max_tokens,
        sampling_params: None,
        thinking_level_map: None,
        compat: None,
        headers: None,
        prompt_cache: None,
    }
}

impl Recording {
    fn new(id: &str, seen: &Arc<Mutex<Vec<Seen>>>) -> Arc<Self> {
        Arc::new(Self {
            id: ProviderId::from(id),
            models: vec![model(id, "small", 1_000), model(id, "large", 4_000)],
            seen: Arc::clone(seen),
        })
    }
}

#[async_trait::async_trait]
impl Provider for Recording {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    fn stream(
        &self,
        model: &Model,
        _context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.seen.lock().unwrap().push(Seen {
            provider: self.id.as_str().to_string(),
            model: model.id.as_str().to_string(),
            max_tokens: options.max_tokens,
            reasoning: options.reasoning,
            caller_header: options
                .headers
                .as_ref()
                .and_then(|h| h.get("x-caller").cloned().flatten()),
            agent_hooks: options.on_payload.is_some()
                || options.transform_headers.is_some()
                || options.on_response.is_some(),
            session_id: options.session_id.as_ref().map(|s| s.as_str().to_string()),
        });
        let mut reply = faux_assistant_message(
            vec![faux_text(format!("{} says hi", model.id.as_str()))],
            StopReason::Stop,
        );
        reply.provider = self.id.clone();
        reply.model = model.id.as_str().to_string();
        Box::pin(futures::stream::iter(vec![StreamEvent::terminal(reply)]))
    }
}

/// A virtual-model router that records the reason and state it was asked with and always answers
/// `target` at `High`.
struct Router {
    target: Model,
    asked: Arc<Mutex<Vec<(ModelRouteReason, bool)>>>,
}

#[async_trait::async_trait]
impl ModelRouter for Router {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        self.asked
            .lock()
            .unwrap()
            .push((request.reason, request.state.is_some()));
        Ok(ModelRoute {
            model: self.target.clone(),
            thinking_level: ModelThinkingLevel::High,
            state: None,
        })
    }
}

fn spec(provider: &str, id: &str) -> VirtualModelSpec {
    VirtualModelSpec {
        provider: provider.into(),
        id: id.into(),
        name: id.to_string(),
        thinking_levels: Some(vec![ModelThinkingLevel::Off, ModelThinkingLevel::High]),
        context_window: None,
        max_tokens: None,
        input: None,
    }
}

/// Register `provider/id` as a virtual model routing to `target`; returns the router's log.
fn register_router(
    session: &AgentSession,
    provider: &str,
    id: &str,
    target: Model,
) -> Arc<Mutex<Vec<(ModelRouteReason, bool)>>> {
    let asked = Arc::new(Mutex::new(Vec::new()));
    session
        .register_virtual_model(VirtualModelDefinition::new(
            spec(provider, id),
            Arc::new(Router {
                target,
                asked: Arc::clone(&asked),
            }),
        ))
        .expect("register the virtual model");
    asked
}

fn call(verb: ModelCallVerb, provider: &str, id: &str) -> ModelCall {
    ModelCall {
        verb,
        provider: provider.to_string(),
        model_id: id.to_string(),
        context: Context {
            system_prompt: Some("be brief".to_string()),
            messages: vec![Message::User {
                content: vec![Content::Text {
                    text: "hi".into(),
                    text_signature: None,
                }],
                timestamp: 0,
            }],
            tools: Vec::new(),
        },
        options: ModelCallOptions::default(),
        cancel: CancelToken::new(),
    }
}

fn text_of(message: &cyrup_core::AssistantMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// A native extension whose `/ask <provider>/<id>` command makes ONE `complete` through the host
/// services it was handed — pi's `ctx.modelRegistry.complete(…)` from a command handler.
struct Asker {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    reply: Arc<Mutex<Option<String>>>,
}

#[async_trait::async_trait]
impl NativeExtension for Asker {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("model-asker")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "ask",
            CommandDescriptor {
                description: "ask a model".into(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        _name: &str,
        args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        let services = self.services.lock().unwrap().clone().unwrap();
        let (provider, id) = args.trim().split_once('/').unwrap();
        let reply = services
            .model_complete(call(ModelCallVerb::Stream, provider, id))
            .await;
        let text = text_of(&reply);
        *self.reply.lock().unwrap() = Some(text.clone());
        Ok(Some(text))
    }
}

struct Harness {
    _tmp: TempDir,
    session: Arc<AgentSession>,
    seen: Arc<Mutex<Vec<Seen>>>,
    /// What the `/ask` native's last `complete` answered.
    asked: Arc<Mutex<Option<String>>>,
}

fn config(tmp: &TempDir) -> SessionConfig {
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// A SHARED session (the production shape: `into_shared` attaches the model-call backend) on a
/// recording `rec` provider, with the `/ask` native loaded.
async fn harness() -> Harness {
    let tmp = TempDir::new().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::new(Mutex::new(None));
    let session = SessionBuilder::new(
        Recording::new("rec", &seen) as Arc<dyn Provider>,
        config(&tmp),
    )
    .with_native_extension(Arc::new(Asker {
        services: Mutex::new(None),
        reply: Arc::clone(&asked),
    }) as Arc<dyn NativeExtension>)
    .build()
    .await
    .expect("build")
    .into_shared();
    Harness {
        _tmp: tmp,
        session,
        seen,
        asked,
    }
}

fn services(h: &Harness) -> Arc<dyn HostServices> {
    h.session.services().host_services.clone()
}

// -------------------------------------------------------------------------------- tests ----

/// The native tier end to end: a native's command handler calls `model_complete` on the services
/// it was handed and gets the session provider's reply. Red against a `model_stream` that answers
/// the trait default (no backend): the reply is the "no model registry" error, not `small says hi`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_native_extension_completes_through_the_sessions_provider() {
    let h = harness().await;
    // The real slash-command path: prompt -> the extension command -> the native's handler.
    let _ = h.session.prompt("/ask rec/small").await.unwrap();
    h.session.wait_for_idle().await;
    assert_eq!(h.asked.lock().unwrap().as_deref(), Some("small says hi"));
    let seen = h.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].model, "small");
}

/// pi wires `before_provider_request` / `before_provider_headers` / `after_provider_response` into
/// the AGENT (`core/sdk.ts:296-385` @v1.0.4), never into `ModelRuntime`, so an extension's own call
/// carries none of them — nor the session's id. Red against a `dispatch` that attaches an
/// `on_payload`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_extension_call_carries_none_of_the_agents_request_hooks() {
    let h = harness().await;
    let reply = services(&h)
        .model_complete(call(ModelCallVerb::Stream, "rec", "large"))
        .await;
    assert_eq!(reply.stop_reason, StopReason::Stop);
    let seen = h.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert!(
        !seen[0].agent_hooks,
        "an agent request hook reached the provider"
    );
    assert_eq!(seen[0].session_id, None);
}

/// `stream` / `complete` on a VIRTUAL model settle with pi's unrouted error and never ask the
/// router — upstream's `ModelRuntime.stream` has no virtual arm (`model-runtime.ts:691-706`;
/// `test/virtual-models.test.ts` "fails unrouted stream calls on virtual models"). Red against a
/// backend that routes every verb: the router is asked and the provider answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn complete_on_a_virtual_model_settles_unrouted_and_never_asks_the_router() {
    let h = harness().await;
    let asked = register_router(&h.session, "router", "auto", model("rec", "large", 4_000));
    let reply = services(&h)
        .model_complete(call(ModelCallVerb::StreamSimple, "router", "auto"))
        .await;
    assert_eq!(reply.stop_reason, StopReason::Error);
    assert!(
        reply
            .error_message
            .as_deref()
            .unwrap()
            .contains("must be routed before streaming"),
        "{:?}",
        reply.error_message
    );
    assert!(asked.lock().unwrap().is_empty());
    assert!(h.seen.lock().unwrap().is_empty());
}

/// `streamSimple` on a virtual model ROUTES, exactly as `ModelRuntime.streamSimple`'s virtual arm
/// does (`model-runtime.ts:717-734`): reason `direct`, no state, the routed model's output cap and
/// the router's level. Upstream's test is "routes direct streamSimple calls within the routed
/// model's limits" (maxTokens 20_000 against a 5_000 model -> 5_000). Red against a backend that
/// refuses every virtual model, and — for the cap — against one that skips the `min`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stream_simple_on_a_virtual_model_routes_direct_within_the_routed_limits() {
    let h = harness().await;
    let asked = register_router(&h.session, "router", "auto", model("rec", "large", 4_000));
    let mut request = call(ModelCallVerb::StreamSimple, "router", "auto");
    request.options.max_tokens = Some(20_000);
    let reply =
        cyrup_ext::host::model_calls::settle(services(&h).model_stream(request), "router", "auto")
            .await;
    assert_eq!(
        reply.stop_reason,
        StopReason::Stop,
        "{:?}",
        reply.error_message
    );
    assert_eq!(text_of(&reply), "large says hi");
    assert_eq!(
        *asked.lock().unwrap(),
        vec![(ModelRouteReason::Direct, false)]
    );
    let seen = h.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].model, "large");
    assert_eq!(seen[0].max_tokens, Some(4_000));
    assert_eq!(seen[0].reasoning, ModelThinkingLevel::High);
}

/// Caller headers stay with the virtual model's own provider and are dropped for a routed model of
/// ANOTHER provider — upstream's "does not forward caller credentials to a routed model of another
/// provider" (`model-runtime.ts:729-732`). Red against a backend that forwards them unconditionally.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_routed_call_drops_caller_headers_bound_for_another_provider() {
    let h = harness().await;
    register_router(&h.session, "router", "auto", model("rec", "large", 4_000));
    register_router(&h.session, "rec", "auto", model("rec", "large", 4_000));
    for (provider, keeps) in [("router", false), ("rec", true)] {
        let mut request = call(ModelCallVerb::StreamSimple, provider, "auto");
        let mut headers = cyrup_provider::HeaderMap::new();
        headers.insert("x-caller".into(), Some("1".into()));
        request.options.headers = Some(headers);
        let reply = cyrup_ext::host::model_calls::settle(
            services(&h).model_stream(request),
            provider,
            "auto",
        )
        .await;
        assert_eq!(
            reply.stop_reason,
            StopReason::Stop,
            "{:?}",
            reply.error_message
        );
        let last = h.seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(
            last.caller_header.is_some(),
            keeps,
            "virtual model {provider}/auto: {last:?}"
        );
    }
}

/// A call to a model of ANOTHER provider reaches that provider WITHOUT swapping the one the
/// session streams through — pi keeps every provider live (`prepareRequest`); cyrup installs one at
/// a time, and an extension's call must not retarget the conversation. Red against a backend that
/// calls `install_owning_provider`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_to_another_provider_does_not_swap_the_sessions_provider() {
    let h = harness().await;
    let other_seen = Arc::new(Mutex::new(Vec::new()));
    let other = Recording::new("other", &other_seen);
    h.session
        .services()
        .ext_host
        .registry()
        .register_provider_live(
            ExtensionId::from("model-asker"),
            "other",
            other as Arc<dyn Provider>,
        )
        .unwrap();
    let reply = services(&h)
        .model_complete(call(ModelCallVerb::Stream, "other", "small"))
        .await;
    assert_eq!(
        text_of(&reply),
        "small says hi",
        "{:?}",
        reply.error_message
    );
    assert_eq!(other_seen.lock().unwrap().len(), 1);
    assert!(h.seen.lock().unwrap().is_empty());
    let svc = services(&h);
    assert!(
        svc.registered_provider("rec").is_some(),
        "the session's provider was swapped"
    );
    assert!(svc.registered_provider("other").is_none());
}

/// A model the catalog does not hold SETTLES as an error — pi's `complete` never rejects for a
/// request-time failure — and no provider is asked. Red against a backend that falls back to the
/// provider's first model, which is what `ProviderStreamFn`'s resolution does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_model_settles_as_an_error_and_reaches_no_provider() {
    let h = harness().await;
    let reply = services(&h)
        .model_complete(call(ModelCallVerb::Stream, "rec", "nope"))
        .await;
    assert_eq!(reply.stop_reason, StopReason::Error);
    assert!(
        reply
            .error_message
            .as_deref()
            .unwrap()
            .contains("not in this session's model catalog")
    );
    assert!(h.seen.lock().unwrap().is_empty());
}

/// A by-value session (no `into_shared`, so no session backend) still answers from the INSTALLED
/// provider — the fallback `complete_standalone` makes — and refuses anything else as settled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_by_value_session_answers_from_the_installed_provider_only() {
    let tmp = TempDir::new().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(
        Recording::new("rec", &seen) as Arc<dyn Provider>,
        config(&tmp),
    )
    .build()
    .await
    .expect("build");
    let svc: Arc<dyn HostServices> = session.services().host_services.clone();
    let ok = svc
        .model_complete(call(ModelCallVerb::Stream, "rec", "small"))
        .await;
    assert_eq!(text_of(&ok), "small says hi");
    let elsewhere = svc
        .model_complete(call(ModelCallVerb::Stream, "other", "small"))
        .await;
    assert_eq!(elsewhere.stop_reason, StopReason::Error);
    assert_eq!(seen.lock().unwrap().len(), 1);
}
