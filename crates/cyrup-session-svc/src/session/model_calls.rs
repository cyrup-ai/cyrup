//! Extension model calls through the session's providers — the backend of pi's
//! `ctx.modelRegistry.stream()` / `streamSimple()` / `complete()` (EXT-086).
//!
//! [`cyrup_ext::HostServices::model_stream`] lands here through [`SessionModelCallsHandle`], which
//! [`AgentSession::into_shared`] attaches over a weak self-handle. It answers what only the session
//! knows: which catalog row an address names, which provider serves it, and — for `streamSimple`
//! — where a virtual model routes.
//!
//! # What is and is not the session's request
//!
//! pi runs an extension's call through `ModelRuntime` directly (`core/model-registry.ts` @v1.0.4),
//! never through the agent, so none of the agent's per-request machinery applies to it and none
//! applies here: no `before_provider_request` / `before_provider_headers` /
//! `after_provider_response` hook (those are the agent's `onPayload` / `transformHeaders` /
//! `onResponse`, `core/sdk.ts:296-385`), no attribution headers, no session id, no retry, no
//! compaction, and no entry in the session — so no usage in the session's totals
//! (`getSessionStats` reads entries only, `core/agent-session.ts:4168-4181`).
//!
//! # Providers are resolved, never installed
//!
//! cyrup installs ONE provider at a time into the [`crate::ProviderSwap`] the agent loop streams
//! through, and a `/model` select or a route swaps it. An extension's call must not: a guest asking
//! `anthropic/…` mid-turn would otherwise retarget the user's conversation. So the owning provider
//! is LOOKED UP — the installed one when it owns the model, then a guest-registered one, then the
//! resolver the swap uses — and handed to this one call only. pi needs no counterpart because its
//! `ModelRuntime` keeps every provider live (`prepareRequest`, `model-runtime.ts:658-688`).
//!
//! # Locks
//!
//! Nothing here takes the session manager's lock or the dynamic-tool state's: the catalog is the
//! composed snapshot ([`AgentSession::full_model_registry`], two std `RwLock` reads), routing a
//! `direct` request reads no branch state (upstream: "`direct` requests have no state"), and the
//! virtual registry clones its router out before awaiting it. So a guest handler that the host
//! dispatched while holding any of those locks cannot deadlock against its own model call.

use std::sync::{Arc, Weak};

use cyrup_core::{EventStream, ModelThinkingLevel};
use cyrup_ext::host::model_calls::dispatch;
use cyrup_ext::host::{ModelCall, ModelCallVerb};
use cyrup_provider::{
    Model, ModelRouteReason, Provider, RouteOptions, StreamEvent, is_virtual_model,
    unrouted_message,
};
use futures::StreamExt;

use super::AgentSession;

/// The live [`crate::host_services::SessionModelCalls`] behind every extension's model call.
/// Weak, like the other adapters, so the capability backend the session owns never keeps it alive.
pub(super) struct SessionModelCallsHandle(pub(super) Weak<AgentSession>);

impl crate::host_services::SessionModelCalls for SessionModelCallsHandle {
    fn model_stream(&self, call: ModelCall) -> EventStream<StreamEvent> {
        let session = self.0.clone();
        // pi's `lazyStream`: the work happens when the stream is first polled, and every failure is
        // a terminal event. The session is upgraded only for the setup and released before the
        // provider streams, so an open stream never keeps an ended session alive. Timed from the
        // call, as pi's `lazyStream` outer stream is: a failure before the provider is reached still
        // gets `durationMs`, and a provider that times its own response wins.
        cyrup_provider::timing::timed(Box::pin(
            futures::stream::once(async move {
                match session.upgrade() {
                    Some(session) => session.extension_model_stream(call).await,
                    None => call.errored_stream("the session this extension belongs to has ended"),
                }
            })
            .flatten(),
        ))
    }
}

impl AgentSession {
    /// One extension model call — see the module docs.
    pub(crate) async fn extension_model_stream(
        &self,
        mut call: ModelCall,
    ) -> EventStream<StreamEvent> {
        let catalog = self.full_model_registry();
        let Some(row) = catalog
            .iter()
            .find(|m| m.provider.as_str() == call.provider && m.id.as_str() == call.model_id)
            .cloned()
        else {
            return call.errored_stream(format!(
                "Model {}/{} is not in this session's model catalog",
                call.provider, call.model_id
            ));
        };
        drop(catalog);
        let model = if is_virtual_model(&row) {
            match call.verb {
                // `stream` has no virtual arm upstream: the call reaches `withVirtualModels`'
                // `unroutedStream` (`core/virtual-models.ts:189-194`, `:233-235` @v1.0.4).
                ModelCallVerb::Stream => {
                    return call
                        .errored_stream(unrouted_message(row.provider.as_str(), row.id.as_str()));
                }
                ModelCallVerb::StreamSimple => {
                    match self.route_extension_call(&row, &mut call).await {
                        Ok(model) => model,
                        Err(message) => return call.errored_stream(message),
                    }
                }
            }
        } else {
            row
        };
        match self.provider_for_extension_call(model.provider.as_str()) {
            Ok(provider) => dispatch(provider.as_ref(), &model, &call),
            Err(message) => call.errored_stream(message),
        }
    }

    /// `ModelRuntime.streamSimple`'s virtual arm (`core/model-runtime.ts:717-734` @v1.0.4): route
    /// with reason `direct`, no state, the CALL's own messages and `reasoning ?? "off"`; then cap
    /// `maxTokens` to the routed model, take its thinking level, and drop caller credentials bound
    /// for another provider — of which a guest can send only `headers`.
    async fn route_extension_call(
        &self,
        row: &Model,
        call: &mut ModelCall,
    ) -> Result<Model, String> {
        let level = call.options.reasoning.unwrap_or(ModelThinkingLevel::Off);
        let mut options = RouteOptions::new(ModelRouteReason::Direct, level);
        options.cancel = call.cancel.clone();
        let catalog = self.virtual_catalog();
        let route = self
            .services
            .virtual_models
            .resolve_model(row, &call.context.messages, options, &catalog)
            .await
            .map_err(|e| e.to_string())?;
        drop(catalog);
        let limit = route.model.max_tokens;
        if limit > 0 {
            call.options.max_tokens = call.options.max_tokens.map(|max| max.min(limit));
        }
        call.options.reasoning = Some(route.thinking_level);
        if route.model.provider != row.provider {
            call.options.headers = None;
        }
        Ok(route.model)
    }

    /// The provider that serves `provider_id`, for ONE extension call, WITHOUT installing it — see
    /// the module docs. Same precedence [`Self::install_owning_provider`] uses.
    fn provider_for_extension_call(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        let current = self.provider.current();
        if current.id().as_str() == provider_id {
            return Ok(current);
        }
        if let Some(guest) = self.services.guest_providers.provider(provider_id) {
            return Ok(guest);
        }
        self.provider
            .resolve(provider_id)
            .map_err(|e| format!("Provider is not configured: {provider_id} ({e})"))
    }
}
