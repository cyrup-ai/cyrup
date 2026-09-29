//! The composed model-registry SNAPSHOT — pi `ModelRuntime`'s cached `snapshot` field
//! (`packages/coding-agent/src/core/model-runtime.ts:59-65` + `:142-148` @v0.87.1). CFG-020.
//!
//! # What upstream does, and what cyrup did
//!
//! pi composes ONCE per invalidation and reads a field thereafter: `rebuildProviders()` (`:268-273`)
//! recomposes every provider and ends in `updateModelSnapshot()` (`:277-284`), which stores
//! `{all, available}`; `getAvailableSnapshot()` (`:423-425`) is then a bare field read, and
//! `hasConfiguredAuth()` (`:467-469`) is a `Set` lookup on `snapshot.configuredProviders`.
//!
//! cyrup's [`AgentSession::full_model_registry`](crate::AgentSession) composed the SAME registry on
//! every call — re-parsing every embedded catalog (`cyrup_provider::default_models` constructs all
//! built-in providers fresh, and each one runs `catalog::load_catalog` over its embedded JSON:
//! `providers/all.rs:362`, `catalog.rs:37`), then deduping the union with a nested
//! `base.iter().any(..)`. That is upstream's `rebuildProviders` cost paid per READ, and the reads
//! are hot: the `/model` picker, the argument-completion source, `set_model`'s resolver, the ACP
//! config options, the rpc `get_available_models`.
//!
//! # The three keys, and why they are exhaustive
//!
//! This snapshot holds the composed `Arc<Vec<Model>>` beside the three inputs that can change it.
//! A read recomputes them and compares; equal means the composition would be identical, so the
//! cached `Arc` is handed back.
//!
//! 1. **The installed provider** — `ProviderSwap::current()`. Compared by `Arc` IDENTITY.
//!    [`crate::ProviderSwap::store`] (`provider_swap.rs:60`) is its only writer, and it *replaces*
//!    the `Arc`, so a different provider is always a different pointer.
//! 2. **The guest-provider registry** — mutated in place behind one long-lived `Arc`, so identity
//!    says nothing; it carries its own monotonic counter instead
//!    ([`crate::guest_providers::GuestProviderRegistry::generation`]), bumped by the only two
//!    writers, `upsert_provider` and `remove_provider`.
//! 3. **The live pi.dev catalog overlay** — `CatalogOverlaySlot::load()`. Compared by `Arc`
//!    identity for the same reason as (1): `CatalogOverlaySlot::install`
//!    (`cyrup-provider/src/catalog_refresh.rs:70`) is its only writer and it replaces the `Arc`.
//!
//! Keying (1) and (3) on identity is sound because a `Provider`'s catalog is FIXED for the
//! lifetime of the instance: `Provider::models` is `fn models(&self) -> &[Model]`
//! (`cyrup-provider/src/provider.rs:105`), a borrow of `&self`, so no implementation can swap its
//! catalog behind a shared reference — a dynamic provider's `refresh_models` writes a
//! [`cyrup_provider::models_store::ModelsStore`] and reaches the registry through a NEW provider
//! instance or through the overlay slot, both of which are key changes. Same for `CatalogOverlay`,
//! which is immutable once built (`remote_catalog.rs`, `from_entries` and read-only accessors).
//!
//! A fourth input exists and is deliberately absent: `services.model_config`, the `models.json`
//! snapshot composed LAST. It is an immutable `Arc` field on [`AgentSessionServices`], fixed when
//! the session is built (`services.rs:119`) — there is no writer at all, so it cannot invalidate
//! anything. That immutability is load-bearing for this cache; if `models.json` ever becomes
//! live-reloadable, it becomes key four.
//!
//! [`AgentSessionServices`]: crate::AgentSessionServices
//!
//! # Why a stale hit is not reachable
//!
//! The snapshot keeps its OWN `Arc` clones of the provider and the overlay for as long as it is
//! cached. An `Arc` that is still held cannot be freed, so its address cannot be reused by a
//! different value underneath the comparison — the classic ABA hazard of pointer-keyed caches is
//! ruled out by construction rather than by argument.
//!
//! # What is NOT cached
//!
//! The auth-derived half. [`AgentSession::available_model_catalog`](crate::AgentSession) filters
//! this registry through a configured-provider set that is rebuilt on EVERY call, so a mid-session
//! `/login` widens the picker on the very next read exactly as it does today. pi caches that set
//! and refreshes it explicitly (`runAvailabilityRefresh`, `:285-312`); cyrup recomputes it, which
//! is strictly fresher and is what keeps the mid-session-login contract
//! (`cyrup-tui/src/tests/login_flow.rs:941-952`) green. What this module removes from that path is
//! the per-MODEL evaluation, not the freshness: see [`AvailabilityFilter`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use cyrup_core::{ModelId, ProviderId};
use cyrup_provider::{CatalogOverlay, Model, Provider};

/// One composed registry plus the three inputs it was composed from.
pub(super) struct RegistrySnapshot {
    /// The installed provider this was composed against (held to pin its address; see module docs).
    pub(super) provider: Arc<dyn Provider>,
    /// [`crate::guest_providers::GuestProviderRegistry::generation`] at composition time.
    pub(super) guest_gen: u64,
    /// The catalog overlay this was composed against (held to pin its address).
    pub(super) overlay: Option<Arc<CatalogOverlay>>,
    /// The composed, deduped, `models.json`-overlaid registry — pi's `snapshot.all`.
    pub(super) models: Arc<Vec<Model>>,
}

impl RegistrySnapshot {
    /// Whether this snapshot was composed from exactly these inputs.
    pub(super) fn matches(
        &self,
        provider: &Arc<dyn Provider>,
        guest_gen: u64,
        overlay: Option<&Arc<CatalogOverlay>>,
    ) -> bool {
        self.guest_gen == guest_gen
            && Arc::ptr_eq(&self.provider, provider)
            && match (self.overlay.as_ref(), overlay) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            }
    }
}

/// The availability predicate, hoisted out of the per-model loop — pi's
/// `snapshot.configuredProviders` (`model-runtime.ts:302-311`, built from one `checkAuth` per
/// PROVIDER) plus the two per-model accommodations cyrup's `has_configured_auth` adds on top.
///
/// Built once per [`AgentSession::available_model_catalog`](crate::AgentSession) call and then
/// consulted with two hash lookups per model. It answers, for every model, exactly what
/// `has_configured_auth` answers for it — the three arms are the same three arms, in the same
/// order — but the expensive ones are evaluated per distinct PROVIDER (~40) instead of per model
/// (~1100):
///
/// * `configured` — `cyrup_config::provider_is_configured`, which reaches the runtime-key map, an
///   `RwLock` over the credential store and the process environment (`cyrup-config/src/auth.rs`),
///   plus the `models.json` presence check (`cyrup-config/src/model/compose.rs:107-123`). Also
///   folds in the guest-provider arm, which is per-provider too.
/// * `current_catalog` — the offline-faux accommodation: a model the CURRENTLY installed provider
///   exposes is always selectable. This was a linear scan of that provider's catalog per model;
///   it is now one set built from that catalog.
pub(super) struct AvailabilityFilter {
    /// Provider ids with configured auth, or registered by a guest extension.
    configured: HashSet<ProviderId>,
    /// Every model the installed provider exposes, as provider → ids. A map rather than a set of
    /// pairs so a lookup borrows both halves instead of cloning them per model.
    current_catalog: HashMap<ProviderId, HashSet<ModelId>>,
}

impl AvailabilityFilter {
    /// Build from an already-evaluated configured-provider set and the installed provider's catalog.
    pub(super) fn new(configured: HashSet<ProviderId>, current: &[Model]) -> Self {
        let mut current_catalog: HashMap<ProviderId, HashSet<ModelId>> = HashMap::new();
        for model in current {
            current_catalog
                .entry(model.provider.clone())
                .or_default()
                .insert(model.id.clone());
        }
        Self {
            configured,
            current_catalog,
        }
    }

    /// Whether `model` is offered by the `/model` selector.
    pub(super) fn allows(&self, model: &Model) -> bool {
        self.configured.contains(&model.provider)
            || self
                .current_catalog
                .get(&model.provider)
                .is_some_and(|ids| ids.contains(&model.id))
    }
}
