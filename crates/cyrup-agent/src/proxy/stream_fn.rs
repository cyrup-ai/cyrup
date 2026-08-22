//! StreamFn adapter (Pi's `streamFn: (model, context, options) => streamProxy(...)` closure,
//! proxy.ts:92-98).

use super::options::ProxyStreamOptions;
use super::transport::stream_proxy;
use crate::stream_fn::StreamFn;
use cyrup_core::{EventStream, ModelRef, ModelThinkingLevel, ThinkingLevel};
use cyrup_provider::{Context, ProviderEnv, StreamEvent, StreamOptions};

/// A [`StreamFn`] that routes every model call through a proxy server — the cyrup analog of Pi's
/// example closure (proxy.ts:92-98). Construct with the proxy URL + auth token; the per-request
/// [`StreamOptions`] the agent builds are mapped onto [`ProxyStreamOptions`].
///
/// Every field Pi's closure forwards via its `{...options}` spread is forwarded here, including
/// `thinking_budgets`: the agent loop threads `AgentBuilder::thinking_budgets()` into
/// [`StreamOptions::thinking_budgets`] (`Option<cyrup_provider::ThinkingBudgets>`, stream.rs:165),
/// and [`ProxyStreamFn::options_from`] copies that same-typed field straight onto
/// [`ProxyStreamOptions`], from where `build_proxy_request_options` puts it on the wire body — 1:1
/// with Pi (`buildProxyRequestOptions`, proxy.ts:111). The one field that cannot map 1:1 is the
/// model identity: cyrup's provider-agnostic `StreamFn` seam carries only a [`ModelRef`], not Pi's
/// full `Model` (see [`model_wire`](super::options::model_wire)).
pub struct ProxyStreamFn {
    proxy_url: String,
    auth_token: String,
    env: Option<ProviderEnv>,
}

impl ProxyStreamFn {
    pub fn new(proxy_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self { proxy_url: proxy_url.into(), auth_token: auth_token.into(), env: None }
    }

    /// Attach a provider-scoped environment overlay (Pi `options.env`) used when deciding whether
    /// the hop to the proxy server is itself proxied. See [`ProxyStreamOptions::env`]; without one,
    /// that decision is made purely from the ambient process environment.
    #[must_use]
    pub fn with_env(mut self, env: ProviderEnv) -> Self {
        self.env = Some(env);
        self
    }

    /// Map the agent's provider-level [`StreamOptions`] onto [`ProxyStreamOptions`] — the cyrup
    /// analogue of Pi's `{...options}` spread (proxy.ts:93-97). Every forwardable field, including
    /// `thinking_budgets`, is carried through unchanged; `reasoning` is lowered from the provider
    /// [`ModelThinkingLevel`] to the unified [`ThinkingLevel`] the proxy body carries.
    fn options_from(&self, opts: &StreamOptions) -> ProxyStreamOptions {
        ProxyStreamOptions {
            temperature: opts.temperature,
            // AGENT-026 — carried straight through, pi's `{...options}` spread (proxy.ts:93-97)
            // feeding the `ProxySerializableStreamOptions` Pick (`:59-71` @v0.84.1). What arrives
            // here is already `Model.samplingParams` merged under the per-request map
            // (`simple-options.ts:27-33`), so the proxy server receives the resolved set and its own
            // OpenAI-compatible adapter applies it.
            sampling_params: opts.sampling_params.clone(),
            max_tokens: opts.max_tokens,
            reasoning: model_thinking_to_unified(opts.reasoning),
            cache_retention: opts.cache_retention,
            session_id: opts.session_id.clone(),
            headers: opts.headers.clone(),
            metadata: opts.metadata.clone(),
            transport: opts.transport,
            // Copy the per-level budgets straight through — this is the cyrup analogue of Pi's
            // `{...options}` spread (proxy.ts:93-97), which carries `thinkingBudgets` into
            // `ProxyStreamOptions`; `build_proxy_request_options` then forwards it onto the wire body
            // (Pi `buildProxyRequestOptions`, proxy.ts:111). Both fields are the SAME
            // `Option<cyrup_provider::ThinkingBudgets>` (stream.rs:165 / proxy/options.rs field decl), so no
            // conversion is needed here — the private `ProxyThinkingBudgets` wire mirror is applied
            // one stage later at `build_proxy_request_options`.
            thinking_budgets: opts.thinking_budgets,
            max_retry_delay_ms: opts.max_retry_delay_ms,
            cancel: opts.cancel.clone(),
            auth_token: self.auth_token.clone(),
            proxy_url: self.proxy_url.clone(),
            env: self.env.clone(),
        }
    }
}

impl StreamFn for ProxyStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        stream_proxy(model.clone(), ctx.clone(), self.options_from(opts))
    }
}

/// Lower a provider-level [`ModelThinkingLevel`] to the unified [`ThinkingLevel`] the proxy body
/// carries: `off` → `None` (reasoning disabled), every on-level maps across.
fn model_thinking_to_unified(level: ModelThinkingLevel) -> Option<ThinkingLevel> {
    match level {
        ModelThinkingLevel::Off => None,
        ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
        ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
        ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
        ModelThinkingLevel::High => Some(ThinkingLevel::High),
        ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
        ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::proxy::options::build_proxy_request_options;
    use cyrup_provider::ThinkingBudgets;
    use serde_json::{Map, Value};

    #[test]
    fn model_thinking_lowers_to_unified() {
        assert_eq!(model_thinking_to_unified(ModelThinkingLevel::Off), None);
        assert_eq!(model_thinking_to_unified(ModelThinkingLevel::High), Some(ThinkingLevel::High));
        assert_eq!(model_thinking_to_unified(ModelThinkingLevel::Xhigh), Some(ThinkingLevel::Xhigh));
    }

    // --- StreamFn adapter threads thinking_budgets end-to-end -----------------
    // Pi's proxy closure spreads `...options` (which carries `thinkingBudgets` from `AgentLoopConfig`
    // — agent.ts:441, reaching `options` via agent-loop.ts:304-308) straight into
    // `ProxyStreamOptions` (proxy.ts:92-98), and `buildProxyRequestOptions` forwards
    // `options.thinkingBudgets` unchanged onto the wire body (proxy.ts:111). The cyrup analogue of
    // that spread is `ProxyStreamFn::options_from`; it must COPY `StreamOptions.thinking_budgets`, not
    // drop it. (Before the fix this dropped it — `options_from` hardcoded `thinking_budgets: None`.)
    #[test]
    fn proxy_stream_fn_threads_thinking_budgets_into_wire_body() {
        let budgets =
            ThinkingBudgets { medium: Some(4096), high: Some(8192), ..ThinkingBudgets::default() };
        let stream_opts =
            StreamOptions { thinking_budgets: Some(budgets), ..StreamOptions::default() };

        let proxy_fn = ProxyStreamFn::new("https://proxy.example", "secret");
        // The transform output carries the budgets (Pi `{...options}` spread, proxy.ts:93-97).
        let proxy_opts = proxy_fn.options_from(&stream_opts);
        assert_eq!(
            proxy_opts.thinking_budgets,
            Some(budgets),
            "options_from must thread StreamOptions.thinking_budgets through, not drop it"
        );

        // And they reach the OUTGOING request/wire body (Pi buildProxyRequestOptions, proxy.ts:111).
        let body = serde_json::to_value(build_proxy_request_options(&proxy_opts)).unwrap();
        assert_eq!(body["thinkingBudgets"]["medium"], serde_json::json!(4096));
        assert_eq!(body["thinkingBudgets"]["high"], serde_json::json!(8192));
        // A `None` budgets stays absent on the wire (Pi drops undefined).
        let none_body = serde_json::to_value(build_proxy_request_options(
            &proxy_fn.options_from(&StreamOptions::default()),
        ))
        .unwrap();
        assert!(none_body.get("thinkingBudgets").is_none());
    }

    /// AGENT-026 — the same spread must carry `samplingParams`, the entire v0.83.0→v0.84.1 diff of
    /// `proxy.ts` (`ProxySerializableStreamOptions` Pick at `:59-71`, `buildProxyRequestOptions` at
    /// `:102-114`).
    ///
    /// This closes the OTHER half of the frame: `ProxyStreamOptions` and the wire struct already
    /// carried the field, but `options_from` hardcoded `None`, so nothing upstream of the proxy
    /// could populate it and the field was reachable only by hand-constructing
    /// `ProxyStreamOptions`. The provider half (`StreamOptions.sampling_params` and the merge over
    /// `Model.sampling_params`) is pinned separately in
    /// `cyrup-provider/src/tests/sampling_params.rs`.
    #[test]
    fn agent026_proxy_stream_fn_threads_sampling_params_into_wire_body() {
        let mut params = Map::new();
        params.insert("top_p".to_string(), Value::from(0.9));
        params.insert("repetition_penalty".to_string(), Value::from(1.05));
        let stream_opts =
            StreamOptions { sampling_params: Some(params.clone()), ..StreamOptions::default() };

        let proxy_fn = ProxyStreamFn::new("https://proxy.example", "secret");
        let proxy_opts = proxy_fn.options_from(&stream_opts);
        assert_eq!(
            proxy_opts.sampling_params.as_ref(),
            Some(&params),
            "options_from must thread StreamOptions.sampling_params through, not drop it"
        );

        let body = serde_json::to_value(build_proxy_request_options(&proxy_opts)).unwrap();
        assert_eq!(body["samplingParams"]["top_p"], serde_json::json!(0.9));
        assert_eq!(body["samplingParams"]["repetition_penalty"], serde_json::json!(1.05));

        // Unset stays absent on the wire (Pi's `JSON.stringify` drops undefined).
        let none_body = serde_json::to_value(build_proxy_request_options(
            &proxy_fn.options_from(&StreamOptions::default()),
        ))
        .unwrap();
        assert!(none_body.get("samplingParams").is_none());
    }
}
