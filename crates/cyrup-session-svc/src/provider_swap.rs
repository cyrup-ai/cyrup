//! Live provider swapping (Pi model+provider switch, model-selector.ts:328-332 + agent-session.ts
//! `setModel`). The `/model` selector may pick a model whose provider differs from the one the
//! session currently streams against; Pi swaps BOTH the model and the owning provider in place.
//!
//! cyrup's agent loop streams through a fixed [`cyrup_agent::StreamFn`], so to swap the provider
//! without rebuilding the whole agent (which would discard conversation state) the loop is handed a
//! [`ProviderSwap`] instead of a bare [`cyrup_agent::ProviderStreamFn`]. `ProviderSwap` holds the
//! *current* provider behind a lock and streams against whatever is installed; the session mutates
//! that inner provider on a cross-provider select. A [`ProviderResolver`] seam (the bin's
//! `select_provider`) rebuilds the owning provider — installing its env-backed credentials — for the
//! target provider id. The seam is additive: absent a resolver, only same-provider changes apply.

use std::sync::{Arc, Mutex, OnceLock};

use cyrup_agent::{Agent, Context, ProviderStreamFn, StreamEvent, StreamFn, StreamOptions};
use cyrup_core::{EventStream, ModelRef, SessionId};
use cyrup_provider::Provider;

use crate::cache_warmer::{CacheWarmRequest, CacheWarmer, ProviderWarmStream};

/// Resolves the owning [`Provider`] for a provider id, installing its credentials (Pi
/// `resolveProvider`). Implemented by the binary over `cyrup::provider::select_provider` (the
/// Pi-faithful built-in registry + env/`--api-key` credential install). Returns a human-readable
/// error string on failure (unknown provider, missing credentials) — never panics.
pub trait ProviderResolver: Send + Sync {
    /// Resolve the provider that owns `provider_id`, with its credentials installed.
    fn resolve(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String>;
}

/// The swappable stream source shared between the [`crate::AgentSession`] and its agent loop.
///
/// The agent streams through this (`impl StreamFn`); swapping the inner provider on a cross-provider
/// `/model` select makes the SAME loop stream against the new provider — 1:1 with Pi switching
/// model+provider live, without rebuilding the agent.
pub struct ProviderSwap {
    /// The currently-installed provider (the offline faux default, or a resolved real provider).
    inner: Mutex<Arc<dyn Provider>>,
    /// The bin's provider resolver seam (`select_provider`). `None` in contexts that never swap
    /// providers (e.g. tests / one-shot builds without the seam wired) — a cross-provider select
    /// then surfaces a clear error instead of silently streaming against the wrong provider.
    resolver: Option<Arc<dyn ProviderResolver>>,
    /// Prompt-cache warming (SEAM-131). This is the structural counterpart of pi's `streamFn`
    /// closure, which holds `cacheWarmer.start(...)` (`core/sdk.ts:403-412` @v1.0.4): the ONE
    /// session request site. `None` in contexts that do not warm: every test path that builds a
    /// bare swap, and an embedder-supplied custom `StreamFn` (`Cyrup::builder().stream_fn(...)`),
    /// which the builder installs INSTEAD of this type and which therefore has no warming seam.
    ///
    /// [CYRUP-DELTA] That second case is a cyrup-only limitation, NOT upstream's scope — this
    /// comment used to claim it was. pi builds its `Agent` with its own `streamFn` closure, and
    /// that closure always holds `cacheWarmer.start(...)` whatever transport it delegates to
    /// (`core/sdk.ts:403-412` @v1.0.4), so every session request is warmed regardless. A session
    /// on a custom `StreamFn` here instead gets `cache_warmer() == None`: nothing is warmed and
    /// `/session` reports `Inactive (cache warming unavailable)`, which is at least honest about
    /// it. Closing the gap means decorating the supplied `StreamFn` so `maybe_start_warming` still
    /// runs before it delegates; it is left open deliberately rather than silently, because a
    /// proxy transport's cache behaviour is not knowable from here and warming it would be the
    /// very assumption `apply_prompt_cache_metadata` refuses to make for a gateway.
    /// The second element is the live session's id, for pi's
    /// `options?.sessionId === sessionManager.getSessionId()` guard.
    ///
    /// Filled AFTER construction (`attach_cache_warming`) because the warmer needs the session
    /// tree and the seam fan-out, both of which the builder assembles after it has handed this
    /// swap to the agent as its `StreamFn`. Nothing can stream through the swap before then, so
    /// the window is not observable.
    warming: OnceLock<(Arc<CacheWarmer>, SessionId)>,
    /// The agent whose `(model, messages)` pair pi's `cacheContextIsCurrent` reads
    /// (`sdk.ts:348-362`). `Weak`, and filled AFTER construction, because the agent is built WITH
    /// this swap as its `StreamFn` — a strong handle would be a cycle and would keep a disposed
    /// session's agent alive. A gone agent answers "not current", which also covers teardown.
    agent: OnceLock<std::sync::Weak<Agent>>,
}

impl ProviderSwap {
    /// Wrap `initial` as the currently-installed provider, with an optional swap `resolver`.
    pub fn new(initial: Arc<dyn Provider>, resolver: Option<Arc<dyn ProviderResolver>>) -> Self {
        Self {
            inner: Mutex::new(initial),
            resolver,
            warming: OnceLock::new(),
            agent: OnceLock::new(),
        }
    }

    /// Install prompt-cache warming on this swap (SEAM-131). Builder-only, called once; a second
    /// call is ignored.
    pub fn attach_cache_warming(&self, warmer: Arc<CacheWarmer>, session_id: SessionId) {
        let _ = self.warming.set((warmer, session_id));
    }

    /// Hand the swap the agent whose live model + transcript decide whether a warming run is still
    /// current. Called once, right after the agent is built. A second call is ignored.
    pub fn attach_agent(&self, agent: &Arc<Agent>) {
        let _ = self.agent.set(Arc::downgrade(agent));
    }

    /// The warmer this swap feeds, if any — the session's accessor for `/session` and
    /// `/settings`.
    pub fn cache_warmer(&self) -> Option<&Arc<CacheWarmer>> {
        self.warming.get().map(|(w, _)| w)
    }

    /// pi `cacheContextIsCurrent(requestModel)` (`core/sdk.ts:350-362` @v1.0.4): the request's
    /// model is still selected AND the live transcript still EXTENDS the request's prefix.
    ///
    /// Upstream compares messages by `===` identity and says why in its own comment — agent state
    /// shallow-copies the array and refreshes the model object, so top-level object identity is not
    /// a valid key, but the ELEMENTS are stable. cyrup has no stable element identity (every
    /// snapshot clones), so the comparison is by VALUE
    /// ([`cyrup_agent::AgentMessage`] is `PartialEq`). That is exactly the cache-prefix condition
    /// identity was standing in for, and it is strictly stronger: two distinct-but-equal messages
    /// produce the same prompt bytes, so the cache entry really is still the one the warm refreshes.
    fn cache_context_is_current(&self, request_model: &ModelRef) -> crate::cache_warmer::IsCurrent {
        let agent = self.agent.get().cloned();
        let request_model = request_model.clone();
        let captured = agent
            .as_ref()
            .and_then(std::sync::Weak::upgrade)
            .map(|a| a.current_model_and_messages().1)
            .unwrap_or_default();
        Arc::new(move || {
            let Some(agent) = agent.as_ref().and_then(std::sync::Weak::upgrade) else {
                return false;
            };
            let (current_model, current_messages) = agent.current_model_and_messages();
            let Some(current_model) = current_model else {
                return false;
            };
            current_model.provider == request_model.provider
                && current_model.model == request_model.model
                && captured.len() <= current_messages.len()
                && captured
                    .iter()
                    .zip(current_messages.iter())
                    .all(|(a, b)| a == b)
        })
    }

    /// pi's `if (options?.sessionId === sessionManager.getSessionId()) cacheWarmer.start(...)`
    /// (`core/sdk.ts:409-411` @v1.0.4).
    ///
    /// The session-id guard is structurally free here — compaction, turn-prefix and branch
    /// summaries call `provider.stream(...)` straight from `complete_summarization`
    /// (`cyrup-session/src/compaction/summarize.rs`, through `DynSummarizer`) and never reach a
    /// `StreamFn` at all, and that function deliberately stamps a FRESH session id. It is ported
    /// anyway, as upstream's own reasoning, and because it keeps the seam correct if `ProviderSwap`
    /// is ever reused for a non-session request.
    fn maybe_start_warming(&self, model: &ModelRef, ctx: &Context, opts: &StreamOptions) {
        let Some((warmer, session_id)) = self.warming.get() else {
            return;
        };
        if opts.session_id.as_ref() != Some(session_id) {
            return;
        }
        let provider = self.current();
        // A virtual model never reaches a provider (`ProviderStreamFn` refuses it), so there is no
        // cache entry to keep warm; and the catalog fallback `ProviderStreamFn` applies for a
        // missing id would warm an arbitrary model. Both cases drop the previous run rather than
        // warm the wrong request — pi reaches the same place through `cacheContextIsCurrent`, which
        // is false for a routed or redirected selection.
        let is_virtual = model
            .api
            .as_ref()
            .is_some_and(|api| api.as_str() == cyrup_core::VIRTUAL_MODEL_API);
        let resolved = (!is_virtual)
            .then(|| provider.get_model(model.model.as_str()))
            .flatten()
            .cloned();
        let Some(resolved) = resolved else {
            // NOT `cancel()`: its `"inactive"` is upstream's session-teardown reason and says
            // nothing a user can act on. `start` on a model without a `promptCache` answers
            // `"cache lifetime unavailable"` upstream, so that is the reason carried here.
            warmer.stop_unwarmable_model();
            return;
        };
        warmer.start(
            CacheWarmRequest {
                stream: Arc::new(ProviderWarmStream(provider)),
                model: resolved,
                context: ctx.clone(),
                options: opts.clone(),
            },
            self.cache_context_is_current(model),
        );
    }

    /// The currently-installed provider (cheap `Arc` clone). Poison-safe (no panic).
    pub fn current(&self) -> Arc<dyn Provider> {
        match self.inner.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Install `provider` as the current one. Poison-safe (no panic).
    pub fn store(&self, provider: Arc<dyn Provider>) {
        match self.inner.lock() {
            Ok(mut guard) => *guard = provider,
            Err(poisoned) => *poisoned.into_inner() = provider,
        }
    }

    /// Whether a swap resolver is wired (cross-provider selects are only possible when it is).
    pub fn has_resolver(&self) -> bool {
        self.resolver.is_some()
    }

    /// Resolve the provider owning `provider_id` (installing its credentials) and install it as the
    /// current provider. Errors when no resolver is wired or the resolver fails.
    pub fn resolve_and_store(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        let resolver = self.resolver.as_ref().ok_or_else(|| {
            format!(
                "cannot switch to provider '{provider_id}': no provider resolver is configured for this session"
            )
        })?;
        let provider = resolver.resolve(provider_id)?;
        self.store(provider.clone());
        Ok(provider)
    }
}

impl StreamFn for ProviderSwap {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        // Stream against whatever provider is currently installed. `ProviderStreamFn` already
        // resolves the concrete `Model` from the `ModelRef` and delivers a terminal error event when
        // the model is absent from the catalog — reuse it verbatim so behaviour matches the fixed
        // (non-swappable) path exactly.
        self.maybe_start_warming(model, ctx, opts);
        ProviderStreamFn::new(self.current()).stream(model, ctx, opts)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use cyrup_provider::faux::FauxProvider;

    struct StubResolver(Arc<dyn Provider>);
    impl ProviderResolver for StubResolver {
        fn resolve(&self, _provider_id: &str) -> Result<Arc<dyn Provider>, String> {
            Ok(self.0.clone())
        }
    }

    #[test]
    fn current_returns_installed_provider() {
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let swap = ProviderSwap::new(faux.clone(), None);
        assert_eq!(swap.current().id().as_str(), "faux");
        assert!(!swap.has_resolver());
    }

    #[test]
    fn resolve_and_store_swaps_the_current_provider() {
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let target: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let swap = ProviderSwap::new(
            faux,
            Some(Arc::new(StubResolver(target.clone())) as Arc<dyn ProviderResolver>),
        );
        let installed = swap
            .resolve_and_store("whatever")
            .expect("stub resolver succeeds");
        assert!(Arc::ptr_eq(&installed, &swap.current()));
    }

    /// SEAM-131 — the warming seam is the SESSION request site and nothing else: pi's
    /// `if (options?.sessionId === sessionManager.getSessionId()) cacheWarmer.start(...)`
    /// (`core/sdk.ts:409-411` @v1.0.4).
    ///
    /// The discriminator is the warmer's own status: a swap that never calls `start` leaves it at
    /// its constructed `waiting for first request`, while a `start` on a faux model with no
    /// prompt-cache lifetime moves it to `cache lifetime unavailable`. So the test distinguishes
    /// "the seam fired" from "the seam declined" without needing a priced catalog.
    ///
    /// **Red-proved** by deleting the `opts.session_id.as_ref() != Some(session_id)` guard: the
    /// foreign-session and no-session calls both moved the status, i.e. a compaction or branch
    /// summary would have replaced the live run's warming schedule with its own.
    #[tokio::test]
    async fn cache_warming_starts_only_for_the_live_session_request() {
        use crate::cache_warmer::{CacheWarmer, CacheWarmingHost, PiDecision};
        use cyrup_core::{ModelId, ProviderId, SessionId, Usage};
        use futures::future::BoxFuture;

        struct NoopHost;
        impl CacheWarmingHost for NoopHost {
            fn last_prompt_tokens(&self) -> BoxFuture<'_, u64> {
                Box::pin(std::future::ready(0))
            }
            fn record_warm(
                &self,
                _provider: ProviderId,
                _model: ModelId,
                _usage: Usage,
                _note: Option<String>,
            ) -> BoxFuture<'_, ()> {
                Box::pin(std::future::ready(()))
            }
        }

        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let swap = ProviderSwap::new(faux, None);
        let warmer = CacheWarmer::new(
            Arc::new(NoopHost),
            cyrup_config::CacheWarmingMode::Streaming,
            Arc::new(PiDecision),
            cyrup_core::CancelToken::new(),
        );
        swap.attach_cache_warming(warmer.clone(), SessionId::from("live"));

        let model = ModelRef {
            provider: ProviderId::from("faux"),
            api: None,
            model: ModelId::from("faux-1"),
        };
        let ctx = Context::default();
        let untouched = Some("waiting for first request");

        // A compaction / branch summary: its own session id (`complete_summarization` stamps a
        // fresh one) — the warmer is never consulted.
        let foreign = StreamOptions {
            session_id: Some(SessionId::from("summary")),
            ..Default::default()
        };
        let _ = swap.stream(&model, &ctx, &foreign);
        assert_eq!(warmer.status().await.reason.as_deref(), untouched);

        // No session id at all (a bare provider call).
        let _ = swap.stream(&model, &ctx, &StreamOptions::default());
        assert_eq!(warmer.status().await.reason.as_deref(), untouched);

        // The live session's own request DOES reach the warmer.
        let live = StreamOptions {
            session_id: Some(SessionId::from("live")),
            ..Default::default()
        };
        let _ = swap.stream(&model, &ctx, &live);
        assert_eq!(
            warmer.status().await.reason.as_deref(),
            Some("cache lifetime unavailable"),
            "the faux model publishes no prompt-cache lifetime, which is `start` answering"
        );
    }

    /// SEAM-131 — a model the seam cannot resolve stops the run with a reason a USER can act on.
    ///
    /// Upstream has no short-circuit at all: `sdk.ts:409-411` @v1.0.4 hands `cacheWarmer.start`
    /// whatever model the `streamFn` was called with, and a model with no `promptCache` stops with
    /// `"cache lifetime unavailable"` (`cache-warmer.ts:221`). cyrup has to resolve the catalog row
    /// before `start` (the warm replays an owned `Model`), so the unresolvable case answers here —
    /// and it must answer with the SAME reason, not with `cancel()`'s `"inactive"`.
    ///
    /// **Red-proved** by restoring `warmer.cancel()` in `maybe_start_warming`: both cases below
    /// reported `Some("inactive")`, which is upstream's session-TEARDOWN reason, is not one of the
    /// ten the guide documents, and renders as the content-free `/session` row
    /// `warming | Inactive (inactive)`. The docs test could not see it because it proves only that
    /// every DOCUMENTED reason is produced, never the converse — which is why
    /// `seam131_every_reason_a_user_can_see_is_documented` now exists beside it.
    #[tokio::test]
    async fn an_unwarmable_model_stops_with_a_documented_reason() {
        use crate::cache_warmer::{CacheWarmer, CacheWarmingHost, PiDecision};
        use cyrup_core::{ModelId, ProviderId, SessionId, Usage};
        use futures::future::BoxFuture;

        struct NoopHost;
        impl CacheWarmingHost for NoopHost {
            fn last_prompt_tokens(&self) -> BoxFuture<'_, u64> {
                Box::pin(std::future::ready(0))
            }
            fn record_warm(
                &self,
                _provider: ProviderId,
                _model: ModelId,
                _usage: Usage,
                _note: Option<String>,
            ) -> BoxFuture<'_, ()> {
                Box::pin(std::future::ready(()))
            }
        }

        let ctx = Context::default();
        let live = StreamOptions {
            session_id: Some(SessionId::from("live")),
            ..Default::default()
        };

        // A virtual selection (never reaches a provider) and an id no catalog carries: the two
        // cases `maybe_start_warming` cannot hand to `start`.
        for model in [
            ModelRef {
                provider: ProviderId::from("faux"),
                api: Some(cyrup_core::ApiId::from(cyrup_core::VIRTUAL_MODEL_API)),
                model: ModelId::from("my-router"),
            },
            ModelRef {
                provider: ProviderId::from("faux"),
                api: None,
                model: ModelId::from("not-in-the-catalog"),
            },
        ] {
            let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
            let swap = ProviderSwap::new(faux, None);
            let warmer = CacheWarmer::new(
                Arc::new(NoopHost),
                cyrup_config::CacheWarmingMode::Streaming,
                Arc::new(PiDecision),
                cyrup_core::CancelToken::new(),
            );
            swap.attach_cache_warming(warmer.clone(), SessionId::from("live"));
            let _ = swap.stream(&model, &ctx, &live);
            assert_eq!(
                warmer.status().await.reason.as_deref(),
                Some("cache lifetime unavailable"),
                "an unwarmable {model:?} must report upstream's reason for exactly this \
                 situation, never `cancel()`'s teardown literal"
            );
        }
    }

    #[test]
    fn resolve_and_store_without_resolver_errors() {
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let swap = ProviderSwap::new(faux, None);
        assert!(swap.resolve_and_store("openai").is_err());
    }
}
