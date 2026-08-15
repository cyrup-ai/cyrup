//! The `Provider` abstraction (arch-01 §6 / func-01 §6).

use crate::auth::{Credential, ProviderAuth};
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::simple_options::{SimpleStreamOptions, build_base_options};
use cyrup_core::{CancelToken, EventStream, ProviderId};

/// The per-refresh context a dynamic provider's [`Provider::refresh_models`] receives — pi
/// `RefreshModelsContext` (`packages/ai/src/models.ts:34-44` @v0.83.0), threaded from
/// [`crate::collection::Models::refresh_with`] exactly as pi threads it from `Models.refresh`
/// (`models.ts:297-303`). PROV-S05.
///
/// `[CYRUP-DELTA]` **two of pi's five members are absent, and both by construction.** pi carries
/// `credential` (`:36`) and `store` (`:38`) because `Models.refresh` resolves the effective
/// credential (`resolveRefreshCredential`, `models.ts:330-354`) and builds a provider-scoped
/// `ProviderModelsStore` (`:287-291`) before calling in. In cyrup the persisting fetcher is
/// [`crate::remote_catalog::RemoteCatalog`], which owns its own [`crate::models_store::ModelsStore`]
/// and its own auth context, and the configured-provider restriction lives at the trigger site
/// (`crates/cyrup/src/provider.rs`) — so neither value has anywhere useful to arrive here. The three
/// that DO change a provider's behaviour per call are all present.
#[derive(Clone, Debug)]
pub struct RefreshModelsContext {
    /// `false` during offline / cache-only initialization (pi `:40`). A provider MUST restore its
    /// persisted catalog and perform no network I/O.
    pub allow_network: bool,
    /// Bypass provider freshness checks and fetch immediately when network access is allowed
    /// (pi `:42`).
    pub force: bool,
    /// pi's `signal?: AbortSignal` (`:43`). **This is not advisory** — an implementation that can
    /// block MUST select on [`RefreshModelsContext::cancelled`] or check
    /// [`RefreshModelsContext::is_aborted`], because that is the only thing that makes
    /// [`crate::collection::ModelsRefreshResult::aborted`] mean anything. `Models::refresh_with`
    /// additionally skips any provider whose turn has not started when the token fires (pi's
    /// `if (options.signal?.aborted) return;`, `models.ts:286`).
    pub cancel: CancelToken,
}

impl Default for RefreshModelsContext {
    /// pi's defaults for a bare `refresh()`: `allowNetwork = options.allowNetwork ?? true`
    /// (`models.ts:277`), `force` undefined ⇒ falsy, no signal.
    fn default() -> Self {
        Self {
            allow_network: true,
            force: false,
            cancel: CancelToken::new(),
        }
    }
}

impl RefreshModelsContext {
    /// The offline posture: restore the persisted catalog, touch no network. This is both pi's
    /// startup call (`agent-session-services.ts:180`, `refresh({ allowNetwork: false })`) and the
    /// shape of its post-failure cache restore (`models.ts:314-319`).
    pub fn cache_only() -> Self {
        Self {
            allow_network: false,
            ..Self::default()
        }
    }

    /// Whether the caller has already aborted (pi `options.signal?.aborted`).
    pub fn is_aborted(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Resolves when the caller aborts. Implementations that block on I/O should race this —
    /// `tokio::select! { biased; () = ctx.cancelled() => …, r = fetch => … }`.
    pub async fn cancelled(&self) {
        self.cancel.cancelled().await;
    }
}

/// A runtime unit owning a model catalog, auth, and stream behavior (func-01 §6).
///
/// Slice: `stream` + catalog reads + `stream_simple` lowering + dynamic `refresh_models`. Auth
/// resolution rides on [`Provider::provider_auth`].
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &ProviderId;

    /// Human display name (Pi `Provider.name`, `models.ts:77` @v0.83.0). Provider pickers and
    /// status output show this rather than the machine id. Defaults to the id so an existing
    /// implementation is unchanged (PROV-017).
    fn name(&self) -> &str {
        self.id().as_str()
    }

    /// Provider-level default base URL (Pi `Provider.baseUrl?`, `models.ts:79` @v0.83.0).
    /// `None` = the provider has none and every model carries its own (PROV-017).
    fn base_url(&self) -> Option<&str> {
        None
    }

    /// Provider-level default headers (Pi `Provider.headers?: ProviderHeaders`, `models.ts:80`
    /// @v0.83.0), merged beneath the per-model and per-request overlays (PROV-017).
    fn headers(&self) -> Option<&crate::HeaderMap> {
        None
    }

    /// Last-known catalog; synchronous and non-throwing (func-01 R-01-001).
    fn models(&self) -> &[Model];

    /// Optional provider policy for credential-specific model availability (Pi
    /// `Provider.filterModels?`, `models.ts:111` @v0.83.0, documented at `:105-110`).
    ///
    /// [`Provider::models`] remains the complete synchronous catalog; this is applied by
    /// [`crate::collection::Models::get_available`] **after** confirming that provider auth is
    /// configured — pi's exact position, `models.ts:407`. The default returns the catalog unchanged,
    /// matching pi's optional member being absent (PROV-032).
    fn filter_models(&self, models: &[Model], _credential: Option<&Credential>) -> Vec<Model> {
        models.to_vec()
    }

    /// The provider's auth strategy (Pi `Provider.auth`). Exposed so a [`crate::collection::Models`]
    /// can resolve request auth against the collection's own credential store + auth context
    /// (Pi `models.ts:getAuth`/`applyAuth`). Default `None` for providers that fully encapsulate
    /// their own auth (the collection then delegates `stream()` without re-resolution). Additive.
    fn provider_auth(&self) -> Option<&ProviderAuth> {
        None
    }

    fn get_model(&self, id: &str) -> Option<&Model> {
        self.models().iter().find(|m| m.id.as_str() == id)
    }

    /// Dynamic providers only: re-fetch and update the model list (Pi `Provider.refreshModels?`,
    /// `models.ts:104` @v0.83.0, documented at `:99-103`; PROV-041 corrected `:63`, which is
    /// `ModelsApiStreamOptions`). Side-effect-free discovery (no loading/downloading). Returns:
    ///
    /// - `None` for a static provider (no dynamic model source) — `Models::refresh` treats this as a
    ///   no-op, exactly as Pi's optional `refreshModels?` being `undefined`.
    /// - `Some(Ok(()))` when the refresh succeeded and the catalog was updated.
    /// - `Some(Err(_))` when the fetch failed; the list stays at its last-known state and a later
    ///   call retries (`Models::refresh(provider)` re-wraps it as a `model_source` error).
    ///
    /// Concurrent calls MUST share one in-flight fetch — an override builds that with
    /// [`crate::utils::refresh::RefreshDedup`]. The default is `None` (static provider). Additive.
    ///
    /// PROV-S05: `ctx` is pi's `RefreshModelsContext` argument (`models.ts:104`, constructed at
    /// `:297-303`). Before this the method took nothing, so `allowNetwork`, `force` and the abort
    /// signal could not reach an implementation at all. An implementation that performs network I/O
    /// **must** honour all three; see [`RefreshModelsContext`].
    async fn refresh_models(&self, _ctx: &RefreshModelsContext) -> Option<Result<(), ProviderError>> {
        None
    }

    /// Construct the response stream. Returns immediately; setup happens behind the stream and
    /// failures are delivered as a terminal `StreamEvent::Error` (func-01 R-01-009/045) — this
    /// method never returns `Err`.
    fn stream(
        &self,
        model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent>;

    /// Stream with the unified "simple" option surface (Pi `Provider.streamSimple`,
    /// `models.ts:119` @v0.83.0; PROV-041 corrected `:71`, a prose line in the `Provider` docblock).
    ///
    /// Lowers a [`SimpleStreamOptions`] to a concrete [`StreamOptions`] and delegates to
    /// [`Provider::stream`]. The default mirrors Pi's per-API `streamSimple` for the
    /// `openai-completions` family (the only wire protocol present), `api/openai-completions.ts:478`:
    ///
    /// 1. `build_base_options` clamps `max_tokens` to the remaining context window and threads every
    ///    transport-level field (`buildBaseOptions`, simple-options.ts:21).
    /// 2. The unified `reasoning` on-level is clamped to one the model supports via
    ///    [`clamp_thinking_level`] (`clampThinkingLevel(model, options.reasoning)`,
    ///    openai-completions.ts:486); a clamp result of `off` disables reasoning
    ///    (`clampedReasoning === "off" ? undefined`, openai-completions.ts:487).
    ///
    /// Token-budget providers (anthropic-messages / google-generative-ai) override this to split the
    /// budget via `adjust_max_tokens_for_thinking`; they land with their wire protocols. Like
    /// [`Provider::stream`], this never returns `Err` — failures arrive as a terminal
    /// [`StreamEvent::Error`].
    fn stream_simple(
        &self,
        model: &Model,
        context: &Context,
        options: &SimpleStreamOptions,
    ) -> EventStream<StreamEvent> {
        // `apiKey || options?.apiKey` — buildBaseOptions already applies this precedence, so pass the
        // option key through (no separate request-key here at the provider edge).
        let mut lowered =
            build_base_options(model, context, options, options.base.api_key.as_deref());
        // `options?.reasoning ? clampThinkingLevel(...) : undefined`; `off` collapses to "disabled".
        if let Some(level) = options.reasoning {
            lowered.reasoning = clamp_thinking_level(model, level.into());
        }
        self.stream(model, context, &lowered)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::model::{Modality, ModelCost};
    use crate::stream::collect_message;
    use crate::utils::simple_options::SimpleStreamOptions;
    use cyrup_core::{AssistantMessage, Message, ModelThinkingLevel, StopReason, ThinkingLevel};
    use std::sync::{Arc, Mutex};
    use tokio_stream::wrappers::ReceiverStream;

    /// A `Provider` whose `stream()` records the exact [`StreamOptions`] it was handed, then yields a
    /// single terminal `Done`. Lets a `stream_simple` test assert the lowering applied to the options.
    struct RecordingProvider {
        id: ProviderId,
        models: Vec<Model>,
        seen: Arc<Mutex<Option<StreamOptions>>>,
    }

    impl Provider for RecordingProvider {
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
            if let Ok(mut g) = self.seen.lock() {
                *g = Some(options.clone());
            }
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            let msg = AssistantMessage::errored(
                model.provider.clone(),
                model.id.as_str(),
                Some(model.api.clone()),
                StopReason::Stop,
                "",
            );
            tokio::spawn(async move {
                let _ = tx.send(StreamEvent::terminal(msg)).await;
            });
            Box::pin(ReceiverStream::new(rx))
        }
    }

    fn reasoning_model(context_window: u64, max_tokens: u64) -> Model {
        Model {
            id: "m1".into(),
            name: "M1".into(),
            api: "openai-completions".into(),
            provider: "p".into(),
            base_url: String::new(),
            reasoning: true,
            input: vec![Modality::Text],
            cost: ModelCost::default(),
            context_window,
            max_tokens,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn ctx() -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![cyrup_core::Content::Text {
                    text: "hi".into(),
                    text_signature: None,
                }],
                timestamp: 0,
            }],
            tools: Vec::new(),
        }
    }

    fn recorder(model: Model) -> (RecordingProvider, Arc<Mutex<Option<StreamOptions>>>) {
        let seen = Arc::new(Mutex::new(None));
        (
            RecordingProvider {
                id: ProviderId::from("p"),
                models: vec![model],
                seen: seen.clone(),
            },
            seen,
        )
    }

    /// Pi `streamSimple` (openai-completions.ts:486-487): the unified on-level is clamped to a level
    /// the model supports. `xhigh` is unsupported without an explicit map entry, so it walks down to
    /// `high`; `max_tokens` defaults to the model cap then is clamped to fit the window.
    #[tokio::test]
    async fn stream_simple_lowers_reasoning_and_clamps_max_tokens() {
        let model = reasoning_model(100_000, 8_000);
        let (provider, seen) = recorder(model.clone());
        let opts = SimpleStreamOptions {
            reasoning: Some(ThinkingLevel::Xhigh),
            ..Default::default()
        };
        let _ = collect_message(provider.stream_simple(&model, &ctx(), &opts)).await;
        let lowered = seen.lock().unwrap().clone().expect("stream() saw options");
        // xhigh (no map entry) clamps to high.
        assert_eq!(lowered.reasoning, ModelThinkingLevel::High);
        // defaulted to the model cap, then clamped to fit the (large) window.
        assert_eq!(lowered.max_tokens, Some(8_000));
    }

    /// No unified `reasoning` → the lowered options keep the default `off` (Pi: `reasoning`
    /// `undefined`), and an explicit `max_tokens` is threaded then clamped.
    #[tokio::test]
    async fn stream_simple_without_reasoning_keeps_off_and_threads_max_tokens() {
        let model = reasoning_model(100_000, 8_000);
        let (provider, seen) = recorder(model.clone());
        let mut opts = SimpleStreamOptions::default();
        opts.base.max_tokens = Some(2_000);
        let _ = collect_message(provider.stream_simple(&model, &ctx(), &opts)).await;
        let lowered = seen.lock().unwrap().clone().expect("stream() saw options");
        assert_eq!(lowered.reasoning, ModelThinkingLevel::Off);
        assert_eq!(lowered.max_tokens, Some(2_000));
    }
}
