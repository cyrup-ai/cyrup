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
//! `has_configured_auth` from the live credential store). An EXTENSION's registration goes through
//! the registry sink instead, at the bind flush and after it, and is checked against
//! [`LiveVirtualCatalog`]: the same physical registry, composed at the check (SEAM-169).

use std::sync::{Arc, Weak};

use cyrup_core::{CancelToken, ProviderId};
use cyrup_provider::{Model, VirtualModelCatalog, VirtualModelListener, is_virtual_model};

use crate::guest_providers::GuestProviderRegistry;

/// SEAM-169 — the catalog an EXTENSION's `registerVirtualModel` (the registry sink's path, at the
/// bind-time flush and for every registration after it) is validated against: the session's
/// physical registry AS IT IS AT THE CHECK. pi checks against its live runtime,
/// `this.models.getModel(providerId, id)` (`core/model-runtime.ts:960` @f1b2e77f5), so a physical
/// id that only the pi.dev overlay lists is a conflict there and must be here.
///
/// It composes [`crate::session::compose_physical_registry`] — the same function, over the same
/// inputs, as the session's own registry — from the installed provider (the slot once one is
/// bound, which follows the overlay; before that the provider the session is being built with),
/// the guest registrations, the overlay the slot holds now and `models.json`. Composed once per
/// check and never kept; checks are rare (a registration). Never the virtual layer: the registry
/// calls this while holding its own lock, which that layer reads.
///
/// [`StartupVirtualCatalog`] was this catalog until SEAM-169: a list composed ONCE at build, with
/// no overlay.
pub(crate) struct LiveVirtualCatalog {
    guest: Weak<GuestProviderRegistry>,
    building: Arc<dyn cyrup_provider::Provider>,
    overlay: Arc<cyrup_provider::CatalogOverlaySlot>,
    model_config: Arc<cyrup_config::ModelFile>,
}

impl LiveVirtualCatalog {
    pub(crate) fn new(
        guest: &Arc<GuestProviderRegistry>,
        building: Arc<dyn cyrup_provider::Provider>,
        overlay: Arc<cyrup_provider::CatalogOverlaySlot>,
        model_config: Arc<cyrup_config::ModelFile>,
    ) -> Self {
        Self {
            guest: Arc::downgrade(guest),
            building,
            overlay,
            model_config,
        }
    }

    /// The physical registry now.
    fn now(&self) -> Vec<Model> {
        let Some(guest) = self.guest.upgrade() else {
            return Vec::new();
        };
        let current = guest
            .installed_provider()
            .unwrap_or_else(|| Arc::clone(&self.building));
        crate::session::compose_physical_registry(
            &current,
            &guest,
            self.overlay.load().as_ref(),
            &self.model_config,
        )
    }
}

impl VirtualModelCatalog for LiveVirtualCatalog {
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        self.now()
            .into_iter()
            .find(|m| m.provider.as_str() == provider && m.id.as_str() == model_id)
    }

    fn models(&self) -> Vec<Model> {
        self.now()
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
    /// `SessionVirtualCatalog` (`AgentSession::register_virtual_model`) does answer it.
    fn has_configured_auth(&self, _provider: &str) -> bool {
        false
    }

    fn has_physical_provider(&self, provider: &str) -> bool {
        self.now()
            .iter()
            .any(|m| m.provider.as_str() == provider && !is_virtual_model(m))
    }
}

/// A FIXED catalog — a list taken once. Until SEAM-169 this was what the builder attached for the
/// extensions' virtual-model check (composed at build, with no overlay); that check now reads
/// [`LiveVirtualCatalog`]. Kept for tests that need a catalog of exactly the rows they give.
#[cfg(test)]
pub(crate) struct StartupVirtualCatalog {
    models: Vec<Model>,
}

#[cfg(test)]
impl StartupVirtualCatalog {
    pub(crate) fn new(models: Vec<Model>) -> Self {
        Self { models }
    }
}

#[cfg(test)]
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
