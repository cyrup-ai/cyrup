//! Options + request body (Pi `ProxyStreamOptions` / `buildProxyRequestOptions`, proxy.ts:59-114).

use cyrup_core::{CancelToken, ModelRef, SessionId, ThinkingLevel};
use cyrup_provider::{CacheRetention, HeaderMap, ProviderEnv, ThinkingBudgets, Transport};
use serde_json::{Map, Value};

/// Options for a single proxied request (Pi `ProxyStreamOptions`, proxy.ts:73-80). The serializable
/// subset (Pi `ProxySerializableStreamOptions`, proxy.ts:59-71) is forwarded in the request body;
/// `cancel`/`auth_token`/`proxy_url` are local-only. `reasoning` is the **unified** on-level and
/// `thinking_budgets` overrides per-level token budgets — the server lowers both via `streamSimple`,
/// so per-level token budgets ARE honored on the proxy path.
#[derive(Clone, Default)]
pub struct ProxyStreamOptions {
    pub temperature: Option<f32>,
    /// AGENT-026 — arbitrary sampling parameters merged into the request body as-is, after the named
    /// request fields, so keys here override them (e.g. `top_p`, `top_k`, `min_p`,
    /// `repetition_penalty`); merged over `Model.samplingParams` per key. Pi
    /// `SimpleStreamOptions.samplingParams` (`packages/ai/src/types.ts:183-189`), added to the
    /// proxy's `ProxySerializableStreamOptions` Pick at `proxy.ts:59-71` and to
    /// `buildProxyRequestOptions` at `:102-114` in v0.84.1 — the entire v0.83.0→v0.84.1 diff of that
    /// file. Only OpenAI-compatible adapters apply it, so on the proxy path the server decides.
    pub sampling_params: Option<Map<String, Value>>,
    pub max_tokens: Option<u64>,
    pub reasoning: Option<ThinkingLevel>,
    pub cache_retention: Option<CacheRetention>,
    pub session_id: Option<SessionId>,
    pub headers: Option<HeaderMap>,
    pub metadata: Option<Map<String, Value>>,
    pub transport: Option<Transport>,
    pub thinking_budgets: Option<ThinkingBudgets>,
    pub max_retry_delay_ms: Option<u64>,
    /// Local abort signal for the proxy request (Pi `signal`, proxy.ts:74-75).
    pub cancel: Option<CancelToken>,
    /// Auth token for the proxy server (Pi `authToken`, proxy.ts:76-77).
    pub auth_token: String,
    /// Proxy server URL, e.g. `https://genai.example.com` (Pi `proxyUrl`, proxy.ts:78-79).
    pub proxy_url: String,
    /// Provider-scoped environment overlay (Pi `options.env`) consulted when resolving whether the
    /// request to [`Self::proxy_url`] is itself routed through an HTTP proxy. Local-only: never
    /// serialized into the request body.
    ///
    /// The ported resolver has always supported this overlay winning over the ambient process env
    /// (`node_http_proxy::get_proxy_env`), but this transport had no way to express it — it called
    /// the ambient-only [`cyrup_provider::build_client_for`], so `http(s)_proxy`/`no_proxy` for the
    /// proxy hop could only ever come from the host. `None` keeps exactly that behavior.
    pub env: Option<ProviderEnv>,
}

/// The serializable request-options body (Pi `ProxySerializableStreamOptions`, proxy.ts:59-71 /
/// `buildProxyRequestOptions`, proxy.ts:101-114). Only present fields are emitted, matching Pi's
/// `JSON.stringify` (which drops `undefined`).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProxyRequestOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// AGENT-026 — `samplingParams` sits between `temperature` and `maxTokens` in pi's Pick and in
    /// `buildProxyRequestOptions` (proxy.ts:59-71 / :102-114 @v0.84.1).
    #[serde(skip_serializing_if = "Option::is_none")]
    sampling_params: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ThinkingLevel>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cache_retention: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session_id: Option<SessionId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    headers: Option<HeaderMap>,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<Transport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking_budgets: Option<ProxyThinkingBudgets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_retry_delay_ms: Option<u64>,
}

/// Wire mirror of [`cyrup_provider::ThinkingBudgets`] (which is not itself `Serialize`). Per-level
/// fields are omitted when unset, matching Pi `ThinkingBudgets` (types.ts:88-94).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxyThinkingBudgets {
    #[serde(skip_serializing_if = "Option::is_none")]
    minimal: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    low: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    medium: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    high: Option<u64>,
}

fn cache_retention_wire(r: CacheRetention) -> &'static str {
    match r {
        CacheRetention::None => "none",
        CacheRetention::Short => "short",
        CacheRetention::Long => "long",
    }
}

/// Project the serializable request body from [`ProxyStreamOptions`] (Pi `buildProxyRequestOptions`,
/// proxy.ts:101-114).
pub(super) fn build_proxy_request_options(options: &ProxyStreamOptions) -> ProxyRequestOptions {
    ProxyRequestOptions {
        temperature: options.temperature,
        sampling_params: options.sampling_params.clone(),
        max_tokens: options.max_tokens,
        reasoning: options.reasoning,
        cache_retention: options.cache_retention.map(cache_retention_wire),
        session_id: options.session_id.clone(),
        headers: options.headers.clone(),
        metadata: options.metadata.clone(),
        transport: options.transport,
        thinking_budgets: options.thinking_budgets.map(|b| ProxyThinkingBudgets {
            minimal: b.minimal,
            low: b.low,
            medium: b.medium,
            high: b.high,
        }),
        max_retry_delay_ms: options.max_retry_delay_ms,
    }
}

/// Serialize the `model` field of the request body. Pi sends the full `Model`; cyrup-agent's
/// provider-agnostic `StreamFn` seam only carries a [`ModelRef`], so the routing identity
/// (`provider`/`api`/`model`) is sent — a documented `[CYRUP-DELTA]`. ([`ModelRef`] is not itself
/// `Serialize`.)
pub(super) fn model_wire(model: &ModelRef) -> Value {
    serde_json::json!({
        "provider": model.provider.as_str(),
        "api": model.api.as_ref().map(|a| a.as_str()),
        "model": model.model.as_str(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn request_body_serializes_pi_serializable_subset() {
        let opts = ProxyStreamOptions {
            temperature: Some(0.5),
            max_tokens: Some(1024),
            reasoning: Some(ThinkingLevel::High),
            cache_retention: Some(CacheRetention::Long),
            session_id: Some("sess-1".into()),
            transport: Some(Transport::Sse),
            thinking_budgets: Some(ThinkingBudgets { medium: Some(5000), ..ThinkingBudgets::default() }),
            max_retry_delay_ms: Some(2000),
            auth_token: "secret".into(),
            proxy_url: "https://proxy.example".into(),
            ..ProxyStreamOptions::default()
        };
        let body = serde_json::to_value(build_proxy_request_options(&opts)).unwrap();
        assert_eq!(body["temperature"], serde_json::json!(0.5));
        assert_eq!(body["maxTokens"], serde_json::json!(1024));
        assert_eq!(body["reasoning"], serde_json::json!("high"));
        assert_eq!(body["cacheRetention"], serde_json::json!("long"));
        assert_eq!(body["sessionId"], serde_json::json!("sess-1"));
        assert_eq!(body["transport"], serde_json::json!("sse"));
        assert_eq!(body["thinkingBudgets"]["medium"], serde_json::json!(5000));
        // Unset per-level budgets are omitted (Pi drops undefined).
        assert!(body["thinkingBudgets"].get("low").is_none());
        assert_eq!(body["maxRetryDelayMs"], serde_json::json!(2000));
        // Local-only fields are NOT in the wire body.
        assert!(body.get("authToken").is_none());
        assert!(body.get("proxyUrl").is_none());
    }

    #[test]
    fn request_body_omits_unset_fields() {
        let body = serde_json::to_value(build_proxy_request_options(&ProxyStreamOptions::default())).unwrap();
        assert_eq!(body, serde_json::json!({}));
    }
}
