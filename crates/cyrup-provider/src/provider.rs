//! The `Provider` abstraction (arch-01 §6 / func-01 §6).

use crate::auth::ProviderAuth;
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::error::ProviderError;
use crate::model::Model;
use crate::stream::{StreamEvent, StreamOptions};
use crate::utils::simple_options::{build_base_options, SimpleStreamOptions};
use cyrup_core::{EventStream, ProviderId};

/// A runtime unit owning a model catalog, auth, and stream behavior (func-01 §6).
///
/// Slice: `stream` + catalog reads + `stream_simple` lowering + dynamic `refresh_models`. Auth
/// resolution rides on [`Provider::provider_auth`].
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> &ProviderId;

    /// Last-known catalog; synchronous and non-throwing (func-01 R-01-001).
    fn models(&self) -> &[Model];

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
    /// models.ts:63). Side-effect-free discovery (no loading/downloading). Returns:
    ///
    /// - `None` for a static provider (no dynamic model source) — `Models::refresh` treats this as a
    ///   no-op, exactly as Pi's optional `refreshModels?` being `undefined`.
    /// - `Some(Ok(()))` when the refresh succeeded and the catalog was updated.
    /// - `Some(Err(_))` when the fetch failed; the list stays at its last-known state and a later
    ///   call retries (`Models::refresh(provider)` re-wraps it as a `model_source` error).
    ///
    /// Concurrent calls MUST share one in-flight fetch — an override builds that with
    /// [`crate::utils::refresh::RefreshDedup`]. The default is `None` (static provider). Additive.
    async fn refresh_models(&self) -> Option<Result<(), ProviderError>> {
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

    /// Stream with the unified "simple" option surface (Pi `Provider.streamSimple`, models.ts:71).
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
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
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    fn ctx() -> Context {
        Context {
            system_prompt: None,
            messages: vec![Message::User {
                content: vec![cyrup_core::Content::Text { text: "hi".into(), text_signature: None }],
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
