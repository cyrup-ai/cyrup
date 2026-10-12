//! PROV-162 — the ONE composer ([`crate::compose::compose_over_base`], pi `composeModelProvider`,
//! `coding-agent/src/core/provider-composer.ts:542-683` @f1b2e77f5) keeps its base's auth layers
//! and streams a base-api model THROUGH the base.
//!
//! Every test drives a real request: `Models::stream` → `applyAuth` → the composed provider → the
//! base. The auth tests reach a loopback Anthropic server through the BUILT-IN anthropic provider
//! (`providers/anthropic.rs` `anthropic_auth()`), so the `x-api-key` they read is the one
//! anthropic's own strategy put on the wire (`api/anthropic_messages/headers.rs`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_core::{AssistantMessage, CancelToken, Content, EventStream, Message, StopReason, Usage};

use crate::api::test_server::serve_sse;
use crate::api::{ApiImpl, ApiRegistry, EventSink, builtin_registry};
use crate::auth::{
    AuthContext, AuthResult, Credential, CredentialStore, InMemoryCredentialStore, ProviderAuth,
    env_key,
};
use crate::collection::{CreateModelsOptions, create_models};
use crate::compose::{AuthLayer, LiteralValues, ProviderLayer, compose_over_base};
use crate::context::Context;
use crate::model::Model;
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions, collect_message};
use crate::utils::simple_options::SimpleStreamOptions;
use crate::wire::WireProvider;
use crate::{ApiId, ProviderEnv, anthropic_provider_with};
use cyrup_core::ProviderId;

/// A scripted environment — what the composed provider's ambient resolution reads instead of the
/// process env, so no test races another over `ANTHROPIC_API_KEY`.
struct MapEnv(BTreeMap<String, String>);

#[async_trait::async_trait]
impl AuthContext for MapEnv {
    async fn env(&self, name: &str) -> Option<String> {
        self.0.get(name).cloned()
    }
    async fn file_exists(&self, _path: &str) -> bool {
        false
    }
}

fn anthropic_transcript() -> String {
    concat!(
        "event: message_start\n",
        "data: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_proxy\",\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
        "event: content_block_start\n",
        "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: content_block_delta\n",
        "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
        "event: content_block_stop\n",
        "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
        "event: message_delta\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n",
        "event: message_stop\n",
        "data: {\"type\":\"message_stop\"}\n\n",
    )
    .to_string()
}

fn hello() -> Context {
    Context {
        system_prompt: None,
        messages: vec![Message::User {
            content: vec![Content::text("hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    }
}

/// The registration `registerProvider("anthropic", { baseUrl, apiKey? })` composes: the base's
/// rows with only `baseUrl` replaced (pi `applyExtension`, `provider-composer.ts:347-348`), its
/// key literal (resolved at registration time).
fn proxy_layer(base: &dyn Provider, proxy: &str, api_key: Option<&str>) -> ProviderLayer {
    let mut auth = AuthLayer::new(Arc::new(LiteralValues));
    auth.api_key = api_key.map(str::to_string);
    ProviderLayer {
        name: None,
        base_url: Some(proxy.to_string()),
        models: base
            .models()
            .iter()
            .cloned()
            .map(|mut m| {
                m.base_url = proxy.to_string();
                m
            })
            .collect(),
        image_models: Vec::new(),
        classifier_models: Vec::new(),
        auth,
    }
}

/// Stream one turn through `anthropic` composed over the built-in with a proxy `baseUrl`, and
/// return the request head the proxy saw.
async fn proxied_request(
    stored: Option<&str>,
    registration_key: Option<&str>,
    ambient: Option<&str>,
) -> String {
    let (origin, seen) = serve_sse(anthropic_transcript()).await;
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new());
    if let Some(key) = stored {
        store
            .modify(
                &ProviderId::from("anthropic"),
                Box::new({
                    let key = key.to_string();
                    move |_| Box::pin(async move { Ok(Some(Credential::api_key(key))) })
                }),
            )
            .await
            .unwrap();
    }
    let mut env = BTreeMap::new();
    if let Some(key) = ambient {
        env.insert("ANTHROPIC_API_KEY".to_string(), key.to_string());
    }
    let ctx: Arc<dyn AuthContext> = Arc::new(MapEnv(env));
    let base: Arc<dyn Provider> = Arc::new(
        anthropic_provider_with(store.clone(), Arc::new(builtin_registry()))
            .with_auth_context(ctx.clone()),
    );
    let layer = proxy_layer(base.as_ref(), &origin, registration_key);
    let composed = compose_over_base(
        "anthropic",
        Some(base),
        layer,
        store.clone(),
        Arc::new(builtin_registry()),
        Some(ctx.clone()),
    )
    .expect("anthropic keeps an auth method");
    let model = composed
        .get_model("claude-sonnet-4-5")
        .or_else(|| composed.models().first())
        .cloned()
        .expect("the base's models survive a baseUrl-only registration");
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(store),
        auth_context: Some(ctx),
        catalog_overlay: None,
    });
    models.set_provider(Arc::new(composed));
    // Keep the loopback request off the session's HTTP proxy.
    let mut options = StreamOptions::default();
    let mut no_proxy = ProviderEnv::new();
    no_proxy.insert("no_proxy".to_string(), "*".to_string());
    options.env = Some(no_proxy);
    let message = models.complete(&model, &hello(), &options).await;
    assert_eq!(
        message.stop_reason,
        StopReason::Stop,
        "the turn must complete against the proxy: {:?}",
        message.error_message
    );
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 1, "exactly one request reached the proxy");
    seen[0].0.to_lowercase()
}

/// PROV-162 behaviour 1 — `registerProvider("anthropic", { baseUrl })` keeps Anthropic's auth.
/// pi `composeApiKeyAuth` (`provider-composer.ts:417-481`): `inherited = base.auth.apiKey`, and
/// `resolve` tries a stored credential, then the registration's key, then the inherited ambient
/// resolution — each handed to anthropic's OWN strategy, which puts it on `x-api-key`. And the
/// request goes to the registration's URL: `POST <proxy>/v1/messages`.
///
/// RED at `360dc2bb`: `compose_over_base` does not exist (the guest path built a base-less
/// `ConfigProvider` with no inherited strategy). Named mutation: `compose_provider_auth` passing
/// `inherited: None` turns all three arms red (the ambient arm reports `Provider is not
/// configured: anthropic`, the other two send no anthropic-shaped auth).
#[tokio::test]
async fn a_registration_over_anthropic_keeps_its_auth_layers_and_streams_to_the_new_url() {
    // stored credential > registration key > inherited ambient.
    let head = proxied_request(
        Some("sk-stored"),
        Some("sk-registration"),
        Some("sk-ambient"),
    )
    .await;
    assert!(head.starts_with("post /v1/messages "), "{head}");
    assert!(head.contains("\r\nx-api-key: sk-stored\r\n"), "{head}");

    let head = proxied_request(None, Some("sk-registration"), Some("sk-ambient")).await;
    assert!(head.starts_with("post /v1/messages "), "{head}");
    assert!(
        head.contains("\r\nx-api-key: sk-registration\r\n"),
        "{head}"
    );

    let head = proxied_request(None, None, Some("sk-ambient")).await;
    assert!(head.starts_with("post /v1/messages "), "{head}");
    assert!(head.contains("\r\nx-api-key: sk-ambient\r\n"), "{head}");
}

/// The composed provider keeps the base's identity where the layer sets none (pi
/// `provider-composer.ts:607-608`): the name `Anthropic`, the layer's base URL over the base's,
/// and the base's OAuth (`oauth = extension.oauth ?? base.auth.oauth`, `:425`, `:489`).
///
/// RED at `360dc2bb` (does not compile — no `compose_over_base`). Named mutation: dropping
/// `oauth` from `compose_provider_auth`'s result turns it red.
#[test]
fn the_composed_provider_keeps_the_bases_name_oauth_and_layers_its_url() {
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new());
    let base: Arc<dyn Provider> = Arc::new(anthropic_provider_with(
        store.clone(),
        Arc::new(builtin_registry()),
    ));
    let base_oauth = base.provider_auth().and_then(|a| a.oauth.clone());
    assert!(base_oauth.is_some(), "fixture: anthropic has a login");
    let layer = proxy_layer(base.as_ref(), "http://proxy.test", None);
    let base_count = base.models().len();
    let composed = compose_over_base(
        "anthropic",
        Some(base),
        layer,
        store,
        Arc::new(builtin_registry()),
        None,
    )
    .unwrap();
    assert_eq!(Provider::name(&composed), "Anthropic");
    assert_eq!(composed.base_url(), Some("http://proxy.test"));
    assert_eq!(composed.models().len(), base_count);
    let auth = composed.provider_auth().unwrap();
    assert_eq!(auth.api_key.as_ref().unwrap().name(), "Anthropic API key");
    assert_eq!(
        auth.oauth.as_ref().map(|o| o.name().to_string()),
        base_oauth.map(|o| o.name().to_string())
    );
}

/// What a [`RecordingApi`] was handed.
#[derive(Clone, Debug)]
struct Seen {
    base_url: String,
    api_key: Option<String>,
}

/// A scripted wire protocol that only the BASE's registry carries: it records the request and
/// ends the turn.
struct RecordingApi {
    api: ApiId,
    seen: Arc<Mutex<Option<Seen>>>,
}

#[async_trait::async_trait]
impl ApiImpl for RecordingApi {
    fn api(&self) -> &ApiId {
        &self.api
    }

    async fn run(
        &self,
        model: &Model,
        _ctx: &Context,
        auth: &AuthResult,
        _opts: &StreamOptions,
        _cancel: CancelToken,
        sink: EventSink,
    ) {
        *self.seen.lock().unwrap() = Some(Seen {
            base_url: model.base_url.clone(),
            api_key: auth.auth.api_key.clone(),
        });
        let message = AssistantMessage {
            content: vec![Content::text("ok")],
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: model.api.clone(),
            response_model: None,
            response_id: None,
            provider_thinking_level: None,
            thinking_level: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            end_turn: None,
            timestamp: 0,
            duration_ms: None,
        };
        sink.send(StreamEvent::terminal(message)).await;
    }
}

fn acme_model(api: &str, base_url: &str) -> Model {
    serde_json::from_value(serde_json::json!({
        "id": "acme-1",
        "name": "Acme 1",
        "api": api,
        "provider": "acme",
        "baseUrl": base_url,
        "reasoning": false,
        "input": ["text"],
        "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0},
        "contextWindow": 32000,
        "maxTokens": 4096
    }))
    .unwrap()
}

/// PROV-162 behaviour 2 — a model whose api the base lists streams THROUGH THE BASE (pi
/// `supportsBaseApi`, `provider-composer.ts:582`; `if (base && supportsBaseApi(model)) return
/// base.stream(model, context, options)`, `:593-596`), with the COMPOSED provider's auth. The
/// base's wire protocol lives only in the base's own registry, so only the base can serve it.
///
/// RED at `360dc2bb`: the composed provider dispatched through its own (process-wide) registry and
/// only borrowed the base's miss TEXT, so the turn ended `Provider acme has no API implementation
/// for "acme-wire"` (the file does not compile there; the same assertion fails under the named
/// mutation `WireProvider::base_for` → `None`, which reports `No API provider registered for api:
/// acme-wire`).
#[tokio::test]
async fn a_model_on_an_api_the_base_lists_streams_through_the_base() {
    let seen = Arc::new(Mutex::new(None));
    let base_registry = ApiRegistry::new();
    base_registry.register_impl(Arc::new(RecordingApi {
        api: ApiId::from("acme-wire"),
        seen: seen.clone(),
    }));
    let store: Arc<dyn CredentialStore> = Arc::new(InMemoryCredentialStore::new());
    let base: Arc<dyn Provider> = Arc::new(WireProvider::new(
        "acme",
        "Acme",
        vec![acme_model("acme-wire", "https://acme.example/v1")],
        ProviderAuth::with_api_key(env_key("Acme API key", ["ACME_API_KEY"])),
        store.clone(),
        Arc::new(base_registry),
    ));
    let mut auth = AuthLayer::new(Arc::new(LiteralValues));
    auth.api_key = Some("sk-registration".to_string());
    let layer = ProviderLayer {
        name: None,
        base_url: Some("http://proxy.test/v1".to_string()),
        models: vec![acme_model("acme-wire", "http://proxy.test/v1")],
        image_models: Vec::new(),
        classifier_models: Vec::new(),
        auth,
    };
    let ctx: Arc<dyn AuthContext> = Arc::new(MapEnv(BTreeMap::new()));
    let composed = compose_over_base(
        "acme",
        Some(base),
        layer,
        store.clone(),
        Arc::new(builtin_registry()),
        Some(ctx.clone()),
    )
    .unwrap();
    let model = composed.models()[0].clone();
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(store),
        auth_context: Some(ctx),
        catalog_overlay: None,
    });
    models.set_provider(Arc::new(composed));

    let message = models
        .complete(&model, &hello(), &StreamOptions::default())
        .await;
    assert_eq!(message.error_message, None);
    assert_eq!(message.stop_reason, StopReason::Stop);
    let seen = seen.lock().unwrap().clone().expect("the base's api ran");
    assert_eq!(seen.base_url, "http://proxy.test/v1");
    assert_eq!(seen.api_key.as_deref(), Some("sk-registration"));
}

/// A base that answers `stream_simple` itself (radius does, `providers/radius.rs`) and refuses a
/// plain `stream`.
struct SimpleOnlyBase {
    id: ProviderId,
    models: Vec<Model>,
    simple_called: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl Provider for SimpleOnlyBase {
    fn id(&self) -> &ProviderId {
        &self.id
    }
    fn models(&self) -> &[Model] {
        &self.models
    }
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        None
    }
    fn stream(
        &self,
        _model: &Model,
        _context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        panic!("a simple request must reach the base's stream_simple, not its stream")
    }
    fn stream_simple(
        &self,
        model: &Model,
        _context: &Context,
        _options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        self.simple_called.store(true, Ordering::SeqCst);
        let (sink, rx) = crate::api::channel(4);
        let model = model.clone();
        tokio::spawn(async move {
            sink.send(
                crate::error::ProviderError::Decode("simple".to_string()).into_error_event(
                    model.provider.clone(),
                    model.id.as_str(),
                    Some(model.api.clone()),
                ),
            )
            .await;
        });
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}

/// PROV-162 behaviour 2, `simple` leg — pi `streamWith(…, simple = true)` calls
/// `base.streamSimple(model, context, options)` (`provider-composer.ts:594-595`), so a base that
/// overrides it keeps its override.
///
/// RED at `360dc2bb` (does not compile — no `compose_over_base`). Named mutation: removing
/// `WireProvider::stream_simple`'s override turns it red (the default lowers into `self.stream`,
/// which then calls the base's `stream`).
#[tokio::test]
async fn a_simple_request_on_a_base_api_reaches_the_bases_stream_simple() {
    let simple_called = Arc::new(AtomicBool::new(false));
    let base: Arc<dyn Provider> = Arc::new(SimpleOnlyBase {
        id: ProviderId::from("acme"),
        models: vec![acme_model("acme-wire", "https://acme.example/v1")],
        simple_called: simple_called.clone(),
    });
    let mut auth = AuthLayer::new(Arc::new(LiteralValues));
    auth.api_key = Some("sk-registration".to_string());
    let layer = ProviderLayer {
        name: None,
        base_url: None,
        models: vec![acme_model("acme-wire", "https://acme.example/v1")],
        image_models: Vec::new(),
        classifier_models: Vec::new(),
        auth,
    };
    let composed = compose_over_base(
        "acme",
        Some(base),
        layer,
        Arc::new(InMemoryCredentialStore::new()),
        Arc::new(builtin_registry()),
        Some(Arc::new(MapEnv(BTreeMap::new()))),
    )
    .unwrap();
    let model = composed.models()[0].clone();
    let message =
        collect_message(composed.stream_simple(&model, &hello(), &SimpleStreamOptions::default()))
            .await;
    assert!(simple_called.load(Ordering::SeqCst));
    assert_eq!(
        message.error_message.as_deref(),
        Some("decode error: simple")
    );
}
