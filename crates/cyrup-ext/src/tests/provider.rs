//! Custom-provider registration fidelity (arch-08 §5.6; A-08-7). Exercises API-key resolution
//! (literal / `$ENV`/`${ENV}` / `!command`) and the [`ProviderHub`] defer→bind→flush lifecycle (Pi
//! `registerProvider`/`bindCore`), including post-bind immediate upsert and `unregisterProvider`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::provider::{ModelRegistrySink, ProviderHub, ProviderRegistration, resolve_api_key};
use serde_json::json;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct FakeSink {
    upserts: Mutex<Vec<String>>,
    removes: Mutex<Vec<String>>,
}
impl ModelRegistrySink for FakeSink {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        self.upserts.lock().unwrap().push(reg.id.clone());
    }
    fn upsert_live_provider(&self, id: &str, _provider: Arc<dyn cyrup_provider::Provider>) {
        self.upserts.lock().unwrap().push(format!("live:{id}"));
    }
    fn remove_provider(&self, id: &str) {
        self.removes.lock().unwrap().push(id.to_string());
    }
    fn upsert_virtual_model(
        &self,
        definition: &cyrup_provider::VirtualModelDefinition,
    ) -> Result<(), String> {
        self.upserts.lock().unwrap().push(format!(
            "virtual:{}/{}",
            definition.spec.provider.as_str(),
            definition.spec.id.as_str()
        ));
        Ok(())
    }
    fn remove_virtual_model(&self, provider: &str, id: &str) {
        self.removes
            .lock()
            .unwrap()
            .push(format!("virtual:{provider}/{id}"));
    }
}

#[test]
fn api_key_resolution_literal_env_command() {
    // literal
    assert_eq!(
        resolve_api_key(Some("sk-literal")).unwrap(),
        Some("sk-literal".to_string())
    );
    // absent
    assert_eq!(resolve_api_key(None).unwrap(), None);
    assert_eq!(resolve_api_key(Some("")).unwrap(), None);
    // env interpolation against a var that is reliably present in the test environment.
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        assert_eq!(resolve_api_key(Some("$HOME")).unwrap(), Some(home.clone()));
        assert_eq!(
            resolve_api_key(Some("pre-${HOME}-post")).unwrap(),
            Some(format!("pre-{home}-post"))
        );
    }
    // unknown var expands to empty (Pi behavior).
    assert_eq!(
        resolve_api_key(Some("$CYRUP_DEFINITELY_UNSET_VAR_XZ")).unwrap(),
        Some(String::new())
    );
    // `!command`: stdout, trimmed.
    assert_eq!(
        resolve_api_key(Some("!printf secret123")).unwrap(),
        Some("secret123".to_string())
    );
}

/// PROV-134, extension surface: pi declares `inputLimits` on `ProviderModelConfigBase`
/// (`coding-agent/src/core/extensions/types.ts:1961` @v1.0.4), immediately after `input` and
/// before `promptCache` on the chat subtype. Without it an extension-registered provider cannot
/// declare a resize profile at all, and the profile read off a model an extension's
/// `before_agent_start` handler selects would have nothing to read.
///
/// Declared with NON-DEFAULT numbers on purpose: the catalog stamp fills an image-capable row with
/// 2000/2000/4718592/80, so default values here would prove nothing about the seam.
#[test]
fn registered_model_carries_input_limits_across_the_seam() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "apiKey": "sk-x",
        "api": "openai-completions",
        "baseUrl": "https://acme.example/v1",
        "models": [{
            "id": "acme-vision",
            "name": "Acme Vision",
            "input": ["text", "image"],
            "inputLimits": {
                "maxRequestBytes": 1_234_567,
                "images": {
                    "maxPerMessage": 4,
                    "maxPerRequest": 11,
                    "resize": {
                        "maxWidth": 512,
                        "maxHeight": 384,
                        "maxBytes": 65_536,
                        "jpegQuality": 55
                    }
                }
            },
            "contextWindow": 200_000,
            "maxTokens": 8_192,
            "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0}
        }]
    });
    hub.register("acme".into(), &cfg).unwrap();
    let reg = hub.get("acme").expect("registration stored");

    let models = reg.build_models();
    assert_eq!(models.len(), 1);
    let limits = models[0]
        .input_limits
        .as_ref()
        .expect("inputLimits survived the seam");
    assert_eq!(limits.max_request_bytes, Some(1_234_567));
    assert_eq!(
        limits.images,
        Some(cyrup_provider::ModelImageInputLimits {
            resize: Some(cyrup_provider::ModelImageResizeOptions {
                max_width: Some(512),
                max_height: Some(384),
                max_bytes: Some(65_536),
                jpeg_quality: Some(55),
            }),
            max_per_message: Some(4),
            max_per_request: Some(11),
        }),
        "the profile an extension-selected model decides the request with"
    );
}

/// PROV-001, extension surface: Pi's `ProviderModelConfig.cost` is a full `ModelCost`, tiers
/// included (coding-agent/src/core/extensions/types.ts:1493). A registered long-context model whose
/// tiers were dropped at the seam gets billed at half the real rate above the threshold.
#[test]
fn registered_model_carries_long_context_pricing_tiers_across_the_seam() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "apiKey": "sk-x",
        "api": "openai-completions",
        "baseUrl": "https://acme.example/v1",
        "models": [{
            "id": "acme-long",
            "name": "Acme Long",
            "contextWindow": 1_000_000,
            "maxTokens": 64_000,
            "cost": {
                "input": 2.5,
                "output": 15.0,
                "cacheRead": 0.25,
                "cacheWrite": 0.0,
                "tiers": [{
                    "inputTokensAbove": 272_000,
                    "input": 5.0,
                    "output": 22.5,
                    "cacheRead": 0.5,
                    "cacheWrite": 0.0
                }]
            }
        }]
    });
    hub.register("acme".into(), &cfg).unwrap();
    let reg = hub.get("acme").expect("registration stored");

    let models = reg.build_models();
    assert_eq!(models.len(), 1);
    let cost = &models[0].cost;
    let tiers = cost.tiers.as_ref().expect("tiers survived the seam");
    assert_eq!(tiers.len(), 1);
    assert_eq!(tiers[0].input_tokens_above, 272_000);

    // Observable consequence: a 300k-token request bills at the tier rate, not the base rate.
    let mut usage = cyrup_core::Usage {
        input: 300_000,
        ..Default::default()
    };
    cyrup_provider::apply_cost(cost, &mut usage);
    assert!(
        (usage.cost.input - 1.5).abs() < 1e-9,
        "long-context input cost was {} (base-rate billing is 0.75)",
        usage.cost.input
    );
}

/// The outgoing request body of one `stream` call through an extension-registered provider, read
/// at `on_payload` (the endpoint is a closed port, so nothing leaves the box).
async fn registered_request_body(
    provider: &Arc<dyn cyrup_provider::Provider>,
    model: &cyrup_provider::Model,
    reasoning: cyrup_core::ModelThinkingLevel,
) -> serde_json::Value {
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let opts = cyrup_provider::StreamOptions {
        reasoning,
        timeout_ms: Some(5_000),
        max_retries: Some(0),
        on_payload: Some(Arc::new(
            move |body: serde_json::Value,
                  _model: cyrup_provider::Model|
                  -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send>,
            > {
                *sink.lock().unwrap() = Some(body);
                Box::pin(async { None })
            },
        )),
        ..Default::default()
    };
    let stream = provider.stream(model, &cyrup_provider::Context::default(), &opts);
    cyrup_provider::collect_message(stream).await;
    captured
        .lock()
        .unwrap()
        .take()
        .expect("the adapter must have built a request body")
}

/// CFG-104, extension path: pi's `ProviderChatModelConfig` declares `samplingParams` and
/// `samplingParamsByThinkingLevel` (`core/provider-composer.ts:71-72` @f1b2e77f5) and
/// `extensionModelFromDefinition` spreads the definition onto the model (`:297`), so an
/// extension provider's per-level sampling reaches the request. Before the fix serde dropped both
/// keys at `ProviderModelConfig` and `build_models` hard-coded `sampling_params: None`, so the
/// `high` request carried neither `top_p` nor `temperature`.
#[tokio::test]
async fn registered_model_sampling_params_by_thinking_level_reach_the_request() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "apiKey": "sk-x",
        "api": "openai-completions",
        "baseUrl": "http://127.0.0.1:9/v1",
        "models": [{
            "id": "acme-think",
            "reasoning": true,
            "samplingParams": {"top_p": 0.9},
            "samplingParamsByThinkingLevel": {"high": {"temperature": 0.6}},
            "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0}
        }]
    });
    hub.register("acme".into(), &cfg).unwrap();
    let reg = hub.get("acme").expect("registration stored");
    let provider = reg.build_provider();
    let model = provider
        .get_model("acme-think")
        .cloned()
        .expect("model registered");

    let body =
        registered_request_body(&provider, &model, cyrup_core::ModelThinkingLevel::High).await;
    assert_eq!(body.get("top_p"), Some(&json!(0.9)), "{body}");
    assert_eq!(
        body.get("temperature"),
        Some(&json!(0.6)),
        "the `high` entry must reach the wire: {body}"
    );

    let body =
        registered_request_body(&provider, &model, cyrup_core::ModelThinkingLevel::Off).await;
    assert_eq!(body.get("top_p"), Some(&json!(0.9)), "{body}");
    assert_eq!(
        body.get("temperature"),
        None,
        "`off` has no entry, so only the flat params go out: {body}"
    );
}

/// CFG-104, guest side of the seam: the SDK's `ProviderModelConfig` (what a Rust guest builds)
/// must serialize both sampling keys under the names the host's `ProviderModelConfig` reads, and
/// the registered model must put them on the wire. The wire test above starts from a raw `json!`
/// literal, so it cannot catch a rename mismatch on the SDK side; this one starts from the SDK
/// struct and goes through the same `on_payload` capture.
#[tokio::test]
async fn sdk_provider_model_config_sampling_keys_reach_the_request() {
    let mut sampling = serde_json::Map::new();
    sampling.insert("top_p".into(), json!(0.9));
    let sdk_cfg = cyrup_ext_sdk::ProviderConfig {
        name: "Acme".into(),
        api_key: Some("sk-x".into()),
        api: Some("openai-completions".into()),
        base_url: Some("http://127.0.0.1:9/v1".into()),
        models: vec![cyrup_ext_sdk::ProviderModelConfig {
            id: "acme-think".into(),
            reasoning: true,
            sampling_params: Some(sampling),
            sampling_params_by_thinking_level: Some(json!({"high": {"temperature": 0.6}})),
            ..Default::default()
        }],
        auth_header: None,
        headers: std::collections::BTreeMap::new(),
        oauth: None,
        has_stream_simple: false,
    };
    let wire = serde_json::to_value(&sdk_cfg).unwrap();

    let host: crate::provider::ProviderConfig = serde_json::from_value(wire.clone()).unwrap();
    assert!(host.models[0].sampling_params.is_some(), "{wire}");
    assert!(
        host.models[0].sampling_params_by_thinking_level.is_some(),
        "{wire}"
    );

    let mut hub = ProviderHub::new();
    hub.register("acme".into(), &wire).unwrap();
    let provider = hub
        .get("acme")
        .expect("registration stored")
        .build_provider();
    let model = provider
        .get_model("acme-think")
        .cloned()
        .expect("model registered");
    let body =
        registered_request_body(&provider, &model, cyrup_core::ModelThinkingLevel::High).await;
    assert_eq!(body.get("top_p"), Some(&json!(0.9)), "{body}");
    assert_eq!(body.get("temperature"), Some(&json!(0.6)), "{body}");
}

/// [CYRUP-DELTA] A per-level value that is not an object fails registration here, where pi accepts
/// it. pi never schema-checks an extension model: `ProviderChatModelConfig.samplingParamsByThinkingLevel`
/// (`core/provider-composer.ts:72` @f1b2e77f5) is only a TypeScript type, and
/// `extensionModelFromDefinition` (`:281-297`) spreads the definition onto the model unchecked, so
/// `{high: 0.6}` registers and `resolveSamplingParams` spreads nothing for it (`{...0.6}` is `{}`).
/// `SamplingParamsSchema` (`core/model-config.ts:19`) applies to `models.json` only. cyrup's field
/// is typed, so the bad shape is a registration error rather than a silently ignored entry.
#[test]
fn registered_model_rejects_a_non_object_thinking_level_entry() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "api": "openai-completions",
        "baseUrl": "http://127.0.0.1:9/v1",
        "models": [{"id": "m", "samplingParamsByThinkingLevel": {"high": 0.6}}]
    });
    assert!(hub.register("acme".into(), &cfg).is_err());
}

#[test]
fn provider_hub_defers_until_bind_then_flushes() {
    let mut hub = ProviderHub::new();
    let cfg = json!({
        "name": "Acme",
        "apiKey": "sk-x",
        "api": "openai",
        "models": [{ "id": "m1", "name": "M1", "contextWindow": 1000, "maxOutputTokens": 100 }]
    });
    hub.register("acme".into(), &cfg).unwrap();

    // Before bind: queued, not flushed; the typed config + resolved key are stored.
    assert!(!hub.is_bound());
    assert_eq!(hub.pending_ids(), ["acme".to_string()]);
    let reg = hub.get("acme").expect("registration stored");
    assert_eq!(reg.resolved_api_key.as_deref(), Some("sk-x"));
    assert_eq!(reg.config.api.as_deref(), Some("openai"));
    assert_eq!(reg.config.models.len(), 1);
    assert_eq!(reg.config.models[0].id, "m1");

    // Bind: the pending registration flushes into the sink (Pi bindCore).
    let sink = Arc::new(FakeSink::default());
    hub.bind(sink.clone());
    assert!(hub.is_bound());
    assert!(hub.pending_ids().is_empty());
    assert_eq!(
        sink.upserts.lock().unwrap().clone(),
        vec!["acme".to_string()]
    );

    // Post-bind registration upserts immediately (no queue).
    hub.register("beta".into(), &json!({ "name": "Beta" }))
        .unwrap();
    assert_eq!(sink.upserts.lock().unwrap().len(), 2);
    assert!(hub.pending_ids().is_empty());

    // unregister notifies the sink + drops the registration (Pi unregisterProvider).
    assert!(hub.unregister("acme"));
    assert_eq!(
        sink.removes.lock().unwrap().clone(),
        vec!["acme".to_string()]
    );
    assert!(hub.get("acme").is_none());
    assert!(!hub.unregister("acme"), "second unregister is a no-op");
}

/// EXT-051 — pi's `oauth` block gained `isSubscription?: boolean` at
/// `pi/packages/coding-agent/src/core/extensions/types.ts:1475` @v0.84.1 ("Whether access through
/// this auth method is backed by a provider subscription"); it is ABSENT at the v0.83.0 baseline.
/// The value already crossed the seam inside the untyped `oauth` blob — what was missing was the
/// typed read, so an extension-supplied subscription provider was indistinguishable from a metered
/// API-key one on the host side.
#[test]
fn ext051_oauth_is_subscription_is_readable_on_a_guest_provider() {
    let mut hub = ProviderHub::new();
    hub.register(
        "sub".into(),
        &json!({ "name": "Sub", "oauth": { "name": "Sub Login", "isSubscription": true } }),
    )
    .unwrap();
    hub.register(
        "metered".into(),
        &json!({ "name": "Metered", "oauth": { "name": "Metered Login" } }),
    )
    .unwrap();
    hub.register(
        "keyed".into(),
        &json!({ "name": "Keyed", "apiKey": "sk-x" }),
    )
    .unwrap();

    let sub = hub.get("sub").expect("registered");
    assert!(sub.has_oauth());
    assert!(
        sub.oauth_is_subscription(),
        "a declared isSubscription must reach the host typed"
    );

    let metered = hub.get("metered").expect("registered");
    assert!(metered.has_oauth());
    assert!(
        !metered.oauth_is_subscription(),
        "an OMITTED optional reads false — upstream's `isSubscription?`, not a tri-state"
    );

    let keyed = hub.get("keyed").expect("registered");
    assert!(!keyed.has_oauth());
    assert!(
        !keyed.oauth_is_subscription(),
        "no oauth block at all is not a subscription"
    );
}
