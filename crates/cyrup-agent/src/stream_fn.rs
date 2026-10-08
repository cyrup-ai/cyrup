//! Transport injection (`StreamFn`) + dynamic key resolution (`ApiKeyResolver`) (arch-02 §3.4 /
//! func-02 §14). The loop is provider-agnostic: it only ever talks to a `StreamFn`.

use cyrup_core::{AssistantMessage, EventStream, ModelRef, ProviderId, StopReason};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};
use std::sync::Arc;

/// A provider-agnostic stream source (func-02 R-02-053). Conforms to the arch-01 stream contract:
/// it MUST NOT return `Err` for request/model/runtime failure — failures arrive as a terminal
/// `StreamEvent::Error` inside the stream.
pub trait StreamFn: Send + Sync {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent>;
}

/// Dynamic key resolution (func-02 R-02-054). MUST NOT error: returns `None` on failure; the result
/// takes precedence over any static configured key.
#[async_trait::async_trait]
pub trait ApiKeyResolver: Send + Sync {
    async fn get_api_key(&self, provider: &ProviderId) -> Option<String>;
}

/// Adapter wrapping a concrete [`cyrup_provider::Provider`] as a [`StreamFn`].
pub struct ProviderStreamFn {
    provider: Arc<dyn Provider>,
}

impl ProviderStreamFn {
    pub fn new(provider: Arc<dyn Provider>) -> Self {
        Self { provider }
    }
}

impl StreamFn for ProviderStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        // A VIRTUAL model must never reach a provider: it is a catalog entry that routes each
        // request to a physical model, and the routing step is what replaces it. Guarded ahead of
        // the resolution below because that resolution FALLS BACK to the provider's first catalog
        // entry — so an unrouted virtual model would otherwise stream against an arbitrary model
        // with no error anywhere, which is silently wrong rather than loudly broken.
        //
        // Defence in depth, matching upstream's shape: pi's `withVirtualModels` decorator is the
        // only thing a virtual model can reach and it answers with this same text
        // (`unroutedStream`, `packages/coding-agent/src/core/virtual-models.ts:189-194` @v1.0.4),
        // because `ModelRuntime.stream` has no virtual arm at all.
        if model
            .api
            .as_ref()
            .is_some_and(|api| api.as_str() == cyrup_core::VIRTUAL_MODEL_API)
        {
            let err = AssistantMessage::errored(
                model.provider.clone(),
                model.model.as_str(),
                model.api.clone(),
                StopReason::Error,
                // One spelling of upstream's asserted text, shared with the extension seam's
                // `complete_standalone` — see `cyrup_provider::unrouted_message`.
                cyrup_provider::unrouted_message(model.provider.as_str(), model.model.as_str()),
            );
            return Box::pin(futures::stream::iter(vec![StreamEvent::terminal(err)]));
        }
        // Resolve the concrete Model from the ModelRef; fall back to the first catalog entry.
        let resolved = self
            .provider
            .get_model(model.model.as_str())
            .or_else(|| self.provider.models().first())
            .cloned();
        match resolved {
            Some(m) => self.provider.stream(&m, ctx, opts),
            None => {
                let err = AssistantMessage::errored(
                    model.provider.clone(),
                    model.model.as_str(),
                    model.api.clone(),
                    StopReason::Error,
                    format!("no model '{}' in provider catalog", model.model),
                );
                // `err.stop_reason` is `Error`, so `terminal` builds the `error` terminal with the
                // matching narrowed `ErrorReason`.
                Box::pin(futures::stream::iter(vec![StreamEvent::terminal(err)]))
            }
        }
    }
}
