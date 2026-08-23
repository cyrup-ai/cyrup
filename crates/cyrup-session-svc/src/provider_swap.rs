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

use std::sync::{Arc, Mutex};

use cyrup_agent::{Context, ProviderStreamFn, StreamEvent, StreamFn, StreamOptions};
use cyrup_core::{EventStream, ModelRef};
use cyrup_provider::Provider;

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
pub(crate) struct ProviderSwap {
    /// The currently-installed provider (the offline faux default, or a resolved real provider).
    inner: Mutex<Arc<dyn Provider>>,
    /// The bin's provider resolver seam (`select_provider`). `None` in contexts that never swap
    /// providers (e.g. tests / one-shot builds without the seam wired) — a cross-provider select
    /// then surfaces a clear error instead of silently streaming against the wrong provider.
    resolver: Option<Arc<dyn ProviderResolver>>,
}

impl ProviderSwap {
    /// Wrap `initial` as the currently-installed provider, with an optional swap `resolver`.
    pub(crate) fn new(initial: Arc<dyn Provider>, resolver: Option<Arc<dyn ProviderResolver>>) -> Self {
        Self { inner: Mutex::new(initial), resolver }
    }

    /// The currently-installed provider (cheap `Arc` clone). Poison-safe (no panic).
    pub(crate) fn current(&self) -> Arc<dyn Provider> {
        crate::sync::lock(&self.inner).clone()
    }

    /// Install `provider` as the current one. Poison-safe (no panic).
    pub(crate) fn store(&self, provider: Arc<dyn Provider>) {
        *crate::sync::lock(&self.inner) = provider;
    }

    /// Resolve the provider owning `provider_id` (installing its credentials) and install it as the
    /// current provider. Errors when no resolver is wired or the resolver fails.
    pub(crate) fn resolve_and_store(&self, provider_id: &str) -> Result<Arc<dyn Provider>, String> {
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
        ProviderStreamFn::new(self.current()).stream(model, ctx, opts)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
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
    }

    #[test]
    fn resolve_and_store_swaps_the_current_provider() {
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let target: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let swap =
            ProviderSwap::new(faux, Some(Arc::new(StubResolver(target.clone())) as Arc<dyn ProviderResolver>));
        let installed = swap.resolve_and_store("whatever").expect("stub resolver succeeds");
        assert!(Arc::ptr_eq(&installed, &swap.current()));
    }

    #[test]
    fn resolve_and_store_without_resolver_errors() {
        let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
        let swap = ProviderSwap::new(faux, None);
        assert!(swap.resolve_and_store("openai").is_err());
    }
}
