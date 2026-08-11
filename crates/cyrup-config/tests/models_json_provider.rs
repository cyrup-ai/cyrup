//! CFG-002 blocker proof: a provider that exists ONLY in `<agent_dir>/models.json` must be a real,
//! **streamable** provider — not just an extra row in a catalog `Vec<Model>`.
//!
//! Pi's `composeModelProvider` synthesizes a `Provider` even when `base` is `undefined`
//! (provider-composer.ts:411-437) and its `streamWith` falls through to `getApiProvider(model.api)`
//! (:459-465); `ModelRuntime.rebuildProviders` registers it for every id in `providerIds()` =
//! `builtins ∪ … ∪ config.getProviderIds()` (model-runtime.ts:193-231). So in Pi a `mycorp` block
//! yields a provider you can actually stream against.
//!
//! These tests drive a REAL request through the composed provider: `Models::complete` →
//! auth resolution → `ApiRegistry` dispatch on `model.api` → the wire impl. The wire impl is
//! scripted (no network), and it RECORDS what it was handed, so the assertions are about the
//! request that would have gone out: the declared `baseUrl`, the resolved API key, the configured
//! provider AND per-model headers, the merged `compat` (whose routing members are copied verbatim
//! into the request payload) and the merged `thinkingLevelMap` (which decides the wire
//! `reasoning_effort`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use cyrup_config::{ModelFile, load_models_file};
use cyrup_provider::api::{ApiImpl, EventSink};
use cyrup_provider::{
    ApiId, ApiRegistry, AuthContext, AuthResult, Context, CreateModelsOptions, Credential,
    CredentialStore, HeaderMap, InMemoryCredentialStore, Model, Models, StreamEvent, StreamOptions,
    create_models,
};
use cyrup_core::{AssistantMessage, CancelToken, Content, ProviderId, StopReason, Usage};

/// What the wire protocol was actually handed for one request.
#[derive(Clone, Debug, Default)]
struct Seen {
    base_url: String,
    api_key: Option<String>,
    headers: Option<HeaderMap>,
    context_window: u64,
    /// The raw `model.compat` the wire impl reads to shape the payload — `openai-completions.ts`'s
    /// `buildParams` copies `compat.openRouterRouting` straight into the request's `provider` field
    /// and `compat.vercelGatewayRouting` into `providerOptions.gateway`
    /// (cyrup `api/openai_completions.rs:366-380`).
    compat: Option<cyrup_provider::api::compat::ModelCompat>,
    /// The map `apply_reasoning` (`api/openai_completions.rs:402`) consults per request to pick the
    /// wire `reasoning_effort` for the requested thinking level.
    thinking_level_map: Option<cyrup_provider::model::ThinkingLevelMap>,
}

/// A scripted `openai-completions` impl: records the request inputs, then emits one text turn.
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
        if let Ok(mut slot) = self.seen.lock() {
            *slot = Some(Seen {
                base_url: model.base_url.clone(),
                api_key: auth.auth.api_key.clone(),
                headers: auth.auth.headers.clone(),
                context_window: model.context_window,
                compat: model.compat.clone(),
                thinking_level_map: model.thinking_level_map.clone(),
            });
        }
        let message = AssistantMessage {
            content: vec![Content::text("ok")],
            provider: model.provider.clone(),
            model: model.id.as_str().to_string(),
            api: model.api.clone(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        sink.send(StreamEvent::terminal(message)).await;
    }
}

/// An auth context with a scripted environment — the seam Pi's `configContextEnv` reads
/// (provider-composer.ts:279-291), so a `${VAR}` template resolves without touching the process env.
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

fn model_file(json: &str) -> ModelFile {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("models.json");
    std::fs::write(&path, json).unwrap();
    load_models_file(&path).expect("models.json parses")
}

/// Build the composed registry over a recording wire impl. Returns the collection, the recorder,
/// and the composition errors.
fn composed(
    file: &ModelFile,
    env: BTreeMap<String, String>,
    store: Arc<dyn CredentialStore>,
) -> (Models, Arc<Mutex<Option<Seen>>>, Vec<String>) {
    let seen = Arc::new(Mutex::new(None));
    let registry = ApiRegistry::new();
    registry.register_impl(Arc::new(RecordingApi {
        api: ApiId::from("openai-completions"),
        seen: seen.clone(),
    }));
    let ctx: Arc<dyn AuthContext> = Arc::new(MapEnv(env));
    let mut models = create_models(CreateModelsOptions {
        credentials: Some(store.clone()),
        auth_context: Some(ctx.clone()),
        catalog_overlay: None,
    });
    // No built-ins registered: this asserts the models.json provider stands entirely on its own,
    // exactly as Pi's `composeModelProvider(id, /* base */ undefined, ...)` does.
    let errors = file.compose_providers(&mut models, store, Arc::new(registry), Some(ctx));
    (models, seen, errors)
}

const MYCORP: &str = r#"{
  // A provider that exists nowhere but here.
  "providers": {
    "mycorp": {
      "name": "MyCorp",
      "baseUrl": "https://gateway.mycorp.example/v1",
      "api": "openai-completions",
      "apiKey": "${MYCORP_API_KEY}",
      "headers": { "X-Mycorp-Tenant": "${MYCORP_TENANT}" },
      "models": [
        { "id": "mycorp-large", "name": "MyCorp Large", "contextWindow": 321000 },
      ],
    },
  },
}"#;

/// The core blocker assertion: a `models.json`-only provider STREAMS. Before this pass the
/// composition produced `Vec<Model>` rows that no `Provider` owned, so the request below had
/// nowhere to go.
#[tokio::test]
async fn a_models_json_only_provider_streams_to_its_declared_base_url() {
    let file = model_file(MYCORP);
    let env = BTreeMap::from([
        ("MYCORP_API_KEY".to_string(), "sk-mycorp-123".to_string()),
        ("MYCORP_TENANT".to_string(), "acme".to_string()),
    ]);
    let (models, seen, errors) = composed(
        &file,
        env,
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
    );
    assert!(errors.is_empty(), "the block composes cleanly: {errors:?}");

    // 1. The provider exists and owns the declared model.
    let provider = models
        .get_provider("mycorp")
        .expect("a models.json-declared provider must be a real Provider in the registry");
    assert_eq!(provider.id().as_str(), "mycorp");
    let model = models
        .get_model("mycorp", "mycorp-large")
        .expect("the declared model must resolve through the registry");

    // 2. It STREAMS: a real request goes through auth resolution + api dispatch.
    let message = models
        .complete(&model, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(
        message.stop_reason,
        StopReason::Stop,
        "the composed provider must stream, not error: {:?}",
        message.error_message
    );

    // 3. The request carried the declared endpoint, the resolved key and the configured header.
    let seen = seen.lock().unwrap().clone().expect(
        "the wire protocol must have been reached — a declared provider with nothing to stream \
         through is exactly the CFG-002 defect",
    );
    assert_eq!(seen.base_url, "https://gateway.mycorp.example/v1");
    assert_eq!(
        seen.api_key.as_deref(),
        Some("sk-mycorp-123"),
        "the `${{VAR}}` apiKey must resolve through the config-value language"
    );
    assert_eq!(
        seen.headers
            .as_ref()
            .and_then(|h| h.get("X-Mycorp-Tenant"))
            .and_then(Option::as_deref),
        Some("acme"),
        "configured provider headers ride along (Pi `withConfiguredAuth`)"
    );
    assert_eq!(seen.context_window, 321_000);
}

/// Pi's `composeApiKeyAuth` credential branch (provider-composer.ts:335-340): a base-less provider
/// that declares NO `apiKey` still authenticates from the existing credential store. This is the
/// "resolve through the existing auth store/env path" half of the scope line — no acquisition.
#[tokio::test]
async fn a_models_json_provider_without_an_api_key_authenticates_from_the_credential_store() {
    let file = model_file(
        r#"{"providers":{"mycorp":{"baseUrl":"https://gateway.mycorp.example/v1",
             "api":"openai-completions",
             "models":[{"id":"mycorp-large"}]}}}"#,
    );
    let store: Arc<dyn CredentialStore> = Arc::new(
        InMemoryCredentialStore::new()
            .with_credential(ProviderId::from("mycorp"), Credential::api_key("sk-stored")),
    );
    let (models, seen, errors) = composed(&file, BTreeMap::new(), store);
    assert!(errors.is_empty(), "{errors:?}");
    let model = models.get_model("mycorp", "mycorp-large").unwrap();
    let message = models
        .complete(&model, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(
        message.stop_reason,
        StopReason::Stop,
        "stored-credential auth must carry the request: {:?}",
        message.error_message
    );
    assert_eq!(
        seen.lock().unwrap().clone().unwrap().api_key.as_deref(),
        Some("sk-stored")
    );
}

/// LOUD AND SAFE (constraint 6): a malformed provider block is rejected WHOLE and named, and the
/// rest of the file still composes. It must not panic and must not take the good blocks with it.
#[tokio::test]
async fn a_malformed_provider_block_is_named_and_the_rest_of_the_file_still_composes() {
    let file = model_file(
        r#"{"providers":{
             "broken": {"models":[{"id":"nope"}]},
             "mycorp": {"baseUrl":"https://gateway.mycorp.example/v1","api":"openai-completions",
                        "models":[{"id":"mycorp-large"}]}
           }}"#,
    );
    let (models, _seen, errors) = composed(
        &file,
        BTreeMap::new(),
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
    );
    assert_eq!(errors.len(), 1, "exactly the bad block is reported: {errors:?}");
    assert!(
        errors[0].contains("broken"),
        "the message must name the offending provider: {}",
        errors[0]
    );
    assert!(
        models.get_provider("broken").is_none(),
        "a rejected base-less block registers nothing (Pi `deleteProvider`)"
    );
    assert!(
        models.get_model("mycorp", "mycorp-large").is_some(),
        "a sibling block still composes — one bad block never costs the rest of the registry"
    );
}

/// `authHeader: true` with no resolvable key is Pi's `throw new Error("authHeader requires a
/// resolved API key")` (provider-composer.ts:257). It must surface as a terminal stream error —
/// never a panic, and never a silently unauthenticated request.
#[tokio::test]
async fn auth_header_without_a_resolvable_key_fails_the_stream_not_the_process() {
    let file = model_file(
        r#"{"providers":{"mycorp":{"baseUrl":"https://gateway.mycorp.example/v1",
             "api":"openai-completions","authHeader":true,"apiKey":"${ABSENT_KEY_FOR_MYCORP}",
             "models":[{"id":"mycorp-large"}]}}}"#,
    );
    let (models, seen, errors) = composed(
        &file,
        BTreeMap::new(),
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
    );
    assert!(errors.is_empty(), "composition itself is credential-blind: {errors:?}");
    let model = models.get_model("mycorp", "mycorp-large").unwrap();
    let message = models
        .complete(&model, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(message.stop_reason, StopReason::Error);
    assert!(
        seen.lock().unwrap().is_none(),
        "no request may reach the wire without the credential the config demands"
    );
}

/// Per-model `headers` from `models.json` must reach the REQUEST. Pi keeps them off the composed
/// model (`modelFromJson` sets `headers: undefined`, provider-composer.ts:156) and resolves them per
/// request instead: `rawModelHeaders` (:384-396) merges `modelOverrides[id].headers` under
/// `models[].headers`, `resolveConfiguredModelHeaders` (:501-511) runs them through the config-value
/// language, and `ModelRuntime.getAuth` (model-runtime.ts:383-397) merges the result into the
/// resolved auth headers on EVERY call.
///
/// Without that second half the declaration parses and then does nothing — the same "declared but
/// inert" defect CFG-002 was filed for, with provider-level headers working and model-level ones
/// silently dropped.
#[tokio::test]
async fn per_model_headers_from_models_json_ride_on_the_request() {
    let file = model_file(
        r#"{"providers":{"mycorp":{
             "baseUrl":"https://gateway.mycorp.example/v1","api":"openai-completions",
             "apiKey":"sk-static",
             "headers":{"X-Provider-Wide":"yes","X-Both":"provider"},
             "models":[
               {"id":"mycorp-large","headers":{"X-Tenant":"${MYCORP_TENANT}","X-Both":"model"}},
               {"id":"mycorp-small"}
             ],
             "modelOverrides":{"mycorp-large":{"headers":{"X-Tenant":"overridden-by-definition",
                                                          "X-From-Override":"kept"}}}
           }}}"#,
    );
    let env = BTreeMap::from([("MYCORP_TENANT".to_string(), "acme".to_string())]);
    let (models, seen, errors) = composed(
        &file,
        env,
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
    );
    assert!(errors.is_empty(), "{errors:?}");

    let large = models.get_model("mycorp", "mycorp-large").unwrap();
    let message = models
        .complete(&large, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(
        message.stop_reason,
        StopReason::Stop,
        "the request must go out: {:?}",
        message.error_message
    );
    let sent = seen.lock().unwrap().clone().unwrap();
    let header = |name: &str| {
        sent.headers
            .as_ref()
            .and_then(|h| h.get(name))
            .and_then(Option::as_deref)
            .map(str::to_string)
    };
    assert_eq!(
        header("X-Tenant").as_deref(),
        Some("acme"),
        "a `models[].headers` entry must reach the wire, resolved through the config-value \
         language — headers were {:?}",
        sent.headers
    );
    assert_eq!(
        header("X-From-Override").as_deref(),
        Some("kept"),
        "a `modelOverrides[id].headers` entry must reach the wire too (Pi `rawModelHeaders` \
         merges both) — headers were {:?}",
        sent.headers
    );
    assert_eq!(
        header("X-Tenant").as_deref(),
        Some("acme"),
        "the `models[]` definition wins over the same-named `modelOverrides` header (Pi's spread \
         order, :391-394)"
    );
    assert_eq!(
        header("X-Both").as_deref(),
        Some("model"),
        "a per-model header outranks the provider-wide one (Pi merges the model layer LAST, \
         model-runtime.ts:392-396) — headers were {:?}",
        sent.headers
    );
    assert_eq!(
        header("X-Provider-Wide").as_deref(),
        Some("yes"),
        "provider-wide headers still ride along"
    );

    // ...and they are PER MODEL: a sibling model of the same provider gets only the provider layer.
    let small = models.get_model("mycorp", "mycorp-small").unwrap();
    let message = models
        .complete(&small, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(message.stop_reason, StopReason::Stop);
    let sent = seen.lock().unwrap().clone().unwrap();
    assert_eq!(
        sent.headers
            .as_ref()
            .and_then(|h| h.get("X-Tenant"))
            .and_then(Option::as_deref),
        None,
        "another model's headers must not leak onto this one: {:?}",
        sent.headers
    );
    assert_eq!(
        sent.headers
            .as_ref()
            .and_then(|h| h.get("X-Both"))
            .and_then(Option::as_deref),
        Some("provider"),
        "with no model layer the provider value stands: {:?}",
        sent.headers
    );
}

/// Pi's `mergeCompat` deep-merges three object-valued members instead of replacing them
/// (provider-composer.ts:87-95): `openRouterRouting`, `vercelGatewayRouting`, `chatTemplateKwargs`.
/// Declaring one field of `openRouterRouting` at the model level must KEEP the provider layer's
/// other routing fields — the merged object is copied verbatim into the request's `provider` field
/// (cyrup `api/openai_completions.rs:366-369`), so replacing it wholesale silently changes the wire
/// payload: here it would drop the `order` preference and the request would route anywhere.
#[tokio::test]
async fn nested_compat_objects_deep_merge_instead_of_replacing_the_routing_block() {
    let file = model_file(
        r#"{"providers":{"mycorp":{
             "baseUrl":"https://gateway.mycorp.example/v1","api":"openai-completions",
             "apiKey":"sk-static",
             "compat":{
               "supportsStore": false,
               "openRouterRouting":{"order":["alpha","beta"],"allow_fallbacks":false},
               "chatTemplateKwargs":{"enable_thinking":true}
             },
             "models":[{"id":"mycorp-large",
                        "compat":{"openRouterRouting":{"allow_fallbacks":true},
                                  "chatTemplateKwargs":{"top_k":5}}}]
           }}}"#,
    );
    let (models, seen, errors) = composed(
        &file,
        BTreeMap::new(),
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let model = models.get_model("mycorp", "mycorp-large").unwrap();
    let message = models
        .complete(&model, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(message.stop_reason, StopReason::Stop);
    let compat = seen
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .compat
        .expect("the composed model must carry compat to the wire impl");

    let routing = compat
        .open_router_routing
        .clone()
        .expect("openRouterRouting must survive the merge");
    assert_eq!(
        routing.get("order"),
        Some(&serde_json::json!(["alpha", "beta"])),
        "the provider layer's routing ORDER must survive a model-level partial override — this is \
         the field that goes out as the request's `provider.order`; routing was {routing}"
    );
    assert_eq!(
        routing.get("allow_fallbacks"),
        Some(&serde_json::json!(true)),
        "the model layer still wins per field: {routing}"
    );
    // The same rule for chatTemplateKwargs, which `build_chat_template_kwargs`
    // (`api/openai_completions.rs:515-525`) copies into the request body.
    let kwargs = compat
        .chat_template_kwargs
        .clone()
        .expect("chatTemplateKwargs must survive the merge");
    assert_eq!(
        kwargs.get("enable_thinking"),
        Some(&serde_json::json!(true)),
        "the provider layer's kwarg must survive a model-level partial override: {kwargs:?}"
    );
    assert_eq!(kwargs.get("top_k"), Some(&serde_json::json!(5)));
    // Non-nested members keep the plain shallow-spread semantics.
    assert_eq!(compat.supports_store, Some(false));
}

/// Pi `applyModelOverride` (provider-composer.ts:104-106): `thinkingLevelMap: override.
/// thinkingLevelMap ? { ...model.thinkingLevelMap, ...override.thinkingLevelMap } : model.
/// thinkingLevelMap` — a PARTIAL `modelOverrides` patch keeps the model's other levels. Replacing
/// the map wholesale changes what every unmentioned thinking level sends: `apply_reasoning`
/// (`api/openai_completions.rs:400-412`) looks the requested level up in exactly this map to pick
/// the wire `reasoning_effort`, so a wiped `low` silently falls back to the generic effort.
#[tokio::test]
async fn a_partial_thinking_level_map_override_patches_rather_than_wipes_the_map() {
    let file = model_file(
        r#"{"providers":{"mycorp":{
             "baseUrl":"https://gateway.mycorp.example/v1","api":"openai-completions",
             "apiKey":"sk-static",
             "models":[{"id":"mycorp-large","reasoning":true,
                        "thinkingLevelMap":{"off":null,"low":"lo","medium":"med","high":"hi"}}],
             "modelOverrides":{"mycorp-large":{"thinkingLevelMap":{"high":"xhigh"}}}
           }}}"#,
    );
    let (models, seen, errors) = composed(
        &file,
        BTreeMap::new(),
        Arc::new(InMemoryCredentialStore::new()) as Arc<dyn CredentialStore>,
    );
    assert!(errors.is_empty(), "{errors:?}");
    let model = models.get_model("mycorp", "mycorp-large").unwrap();
    let message = models
        .complete(&model, &Context::default(), &StreamOptions::default())
        .await;
    assert_eq!(message.stop_reason, StopReason::Stop);
    let map = seen
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .thinking_level_map
        .expect("the wire impl must be handed the model's thinking level map");

    assert_eq!(
        map.get("high"),
        Some(&Some("xhigh".to_string())),
        "the overridden level wins: {map:?}"
    );
    assert_eq!(
        map.get("low"),
        Some(&Some("lo".to_string())),
        "an unmentioned level must survive the patch — wiping it changes what a `low` request \
         sends on the wire: {map:?}"
    );
    assert_eq!(map.get("medium"), Some(&Some("med".to_string())), "{map:?}");
    assert_eq!(
        map.get("off"),
        Some(&None),
        "a `null` entry (level unsupported) must survive too: {map:?}"
    );
}
