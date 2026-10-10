//! The two pieces the virtual-model registry needs from the SESSION side at startup: the catalog
//! an extension's registration is validated against, and the cache-only refresh a registration or
//! removal fires (SEAM-144).
//!
//! Both exist because pi's registry IS its `ModelRegistry`: `registerVirtualModel` reads the
//! catalog and the auth snapshot off `this` (`core/model-runtime.ts:957`, `:962-968`) and ends with
//! `void this.refresh({ allowNetwork: false })` (`:973`, `:982`). cyrup's registry is a standalone
//! value in `cyrup-provider` that takes those reads as a
//! [`cyrup_provider::VirtualModelCatalog`] and that tail as a
//! [`cyrup_provider::VirtualModelListener`].
//!
//! Once an [`crate::AgentSession`] exists, `AgentSession::register_virtual_model` supplies its own,
//! richer catalog (`session/virtual_models.rs`'s `SessionVirtualCatalog`, which can answer
//! `has_configured_auth` from the live credential store). [`StartupVirtualCatalog`] is what the
//! BUILDER has instead, at the point where pi has already drained its queue: after the extensions
//! load and before the selection is restored.

use std::sync::{Arc, Weak};

use cyrup_core::{CancelToken, ProviderId};
use cyrup_provider::{Model, VirtualModelCatalog, VirtualModelListener, is_virtual_model};

use crate::guest_providers::GuestProviderRegistry;

/// The catalog the builder's bind-time flush validates virtual-model registrations against — pi's
/// `modelRuntime` at `agent-session-services.ts:182-194`, which by then holds every provider's
/// catalog.
///
/// Built from the same three sources `AgentSession::compose_model_registry` unions — the session's
/// installed provider, the guest-registered providers, and the compiled-in defaults — then composed
/// through `models.json`, so a physical id the user declared there is seen by the conflict check
/// too.
pub(crate) struct StartupVirtualCatalog {
    models: Vec<Model>,
}

impl StartupVirtualCatalog {
    pub(crate) fn new(models: Vec<Model>) -> Self {
        Self { models }
    }
}

impl VirtualModelCatalog for StartupVirtualCatalog {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.models
            .iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
            .cloned()
    }

    fn models(&self) -> Vec<Model> {
        self.models.clone()
    }

    /// Always `false`, and that is not a stub.
    ///
    /// The registry reads this ONLY to suppress the virtual-only marking
    /// (`model-runtime.ts:962-968`: `if (!recomposeProvider(id) && !configuredProviders.has(id))`),
    /// and the first conjunct — [`Self::has_physical_provider`] — already decides every case that
    /// matters. A provider id with physical models is never marked whatever this answers; a
    /// provider id with none is a virtual-only provider, which is exactly what the marking is for.
    /// The only state this loses is "a provider with no models but with a stored credential", where
    /// the marking is redundant rather than wrong, since the provider is configured either way.
    ///
    /// Answering it properly would need the live credential store the builder has not yet handed to
    /// a session; `SessionVirtualCatalog` does answer it, for every registration made after the
    /// session opens.
    fn has_configured_auth(&self, _provider: &str) -> bool {
        false
    }

    fn has_physical_provider(&self, provider: &str) -> bool {
        self.models
            .iter()
            .any(|m| m.provider.as_str() == provider && !is_virtual_model(m))
    }
}

/// SEAM-144 — the cache-only whole-registry refresh pi fires from `registerVirtualModel` and
/// `unregisterVirtualModel` (`core/model-runtime.ts:975`, `:984` @f1b2e77f5: `void this.refresh({ allowNetwork:
/// false })`), plus the one `createAgentSessionServices` fires after draining the pending queue
/// (`agent-session-services.ts:194`).
///
/// It is [`GuestProviderRegistry::restore_cached`] — whole-registry and cache-only — not
/// `begin_late_restore`'s per-new-id form, because upstream's call takes no `providers` argument.
///
/// Why a virtual-model registration refreshes the PHYSICAL catalogs at all: upstream's
/// `registerVirtualModel` recomposes the provider the virtual model is listed under, and a provider
/// composed for the first time (a virtual-only id, or one whose models were never restored) has
/// nothing until a refresh publishes its persisted catalog. Firing it is also what makes a router
/// registered at startup able to route to a model only a cached catalog names.
///
/// **SEAM-146**: the refresh is spawned, and a registration can be made with no Tokio runtime
/// current (`ExtensionRegistry::register_virtual_model` is synchronous). It is spawned through
/// [`GuestProviderRegistry::spawn_restore_cached`], which falls back to the runtime the startup
/// restore ran on and reports (a `tracing` warning) when there is none, instead of silently doing
/// nothing.
pub(crate) struct CachedRestoreOnVirtualChange {
    /// Weak: the registry holds the listener, and the session holds the registry.
    guest_providers: Weak<GuestProviderRegistry>,
    /// The session's cancel scope, so a restore started by a registration dies with the session.
    cancel: CancelToken,
}

impl CachedRestoreOnVirtualChange {
    pub(crate) fn new(guest_providers: &Arc<GuestProviderRegistry>, cancel: CancelToken) -> Self {
        Self {
            guest_providers: Arc::downgrade(guest_providers),
            cancel,
        }
    }
}

impl VirtualModelListener for CachedRestoreOnVirtualChange {
    fn virtual_models_changed(&self, _provider: &ProviderId) {
        let Some(guest_providers) = self.guest_providers.upgrade() else {
            return;
        };
        let cancel = self.cancel.child_token();
        // `void` — upstream does not await it, and this listener runs inside the registry's
        // mutator, which must not block (and from which an `await` is not even available).
        // SEAM-146: spawned on the caller's runtime or, from a thread with none, on the one the
        // startup restore ran on; reported when neither exists.
        guest_providers.spawn_restore_cached(cancel);
    }
}
