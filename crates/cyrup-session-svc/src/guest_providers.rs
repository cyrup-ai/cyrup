//! The shared model-registry sink for guest-registered providers (arch-08 §5.6; Pi `bindCore` +
//! `ModelRegistry.registerProvider`, model-registry.ts:828-960 / runner.ts:308-380).
//!
//! ROOT CAUSE this closes (L4 gap #4): a guest `pi.registerProvider()` routed through the extension
//! host's [`cyrup_ext::ProviderHub`] but nothing consumed the registrations, so the model never
//! became selectable. Pi's `runner.bindCore` flushes each queued registration into the ONE
//! `ModelRegistry` that `getAvailable()` / `find()` / `setModel` all read (model-registry.ts:917-940
//! folds the registered models straight into `this.models`).
//!
//! cyrup's session streams through a concrete [`Provider`] per provider id, so this registry realizes
//! each guest registration as a provider and holds it behind an `Arc`: under an id no base owns, a
//! `ConfigProvider` ([`cyrup_ext::ProviderRegistration::build_provider`]); under a built-in id, the
//! registration composed over that base ([`cyrup_ext::ProviderRegistration::compose_over`], pi
//! `composeProvider`, PROV-162). The [`AgentSession`](crate::AgentSession) then UNIONs these providers'
//! catalogs into `full_model_registry()` / `available_model_catalog()` and installs the owning provider
//! into the [`crate::ProviderSwap`] on a matching `set_model`, so the registered model is both
//! SELECTABLE and STREAMABLE in the assembled run.
//!
//! # Catalog refresh (EXT-022 native flavour; EXT-M07)
//!
//! The registry is also the engine behind `ModelRegistry.refresh({ providers, allowNetwork, signal })`
//! for the providers it holds (pi `ModelsImpl.refresh`, `packages/ai/src/models.ts:546-606`
//! @v0.99.2-17): see [`GuestProviderRegistry::refresh`]. Nothing drove
//! [`cyrup_provider::Provider::refresh_models`] on an extension-registered provider before this.
//! The per-provider refresh state ([`RefreshInner`]) mirrors pi's `refreshGenerations`,
//! `refreshControllers` and `publicationChains` (`models.ts:389-391`).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use cyrup_core::{CancelToken, ProviderId};
use cyrup_ext::host::services::{ProviderRefreshRequest, ProviderRefresher};
use cyrup_ext::provider::{ComposeOptions, ModelRegistrySink, ProviderRegistration, ProviderWrite};
use cyrup_provider::{
    AuthContext, Credential, CredentialStore, EnvAuthContext, InMemoryCredentialStore,
    InMemoryModelsStore, Model, ModelsPersist, ModelsPublication, ModelsPublisher,
    ModelsRefreshResult, ModelsStore, ModelsStoreOperationOptions, Provider, ProviderError,
    RefreshModelsContext,
};

use crate::provider_swap::ProviderSwap;

/// The bound sink + shared lookup for guest-registered providers. Cheaply shareable via `Arc`
/// (interior `Mutex`); the SAME `Arc` is handed to [`cyrup_ext::ExtensionRegistry::bind_model_registry`]
/// (as the sink) and to the session (as the read view). Built only behind its `Arc`
/// ([`GuestProviderRegistry::new`]), because what it composes holds a weak handle back to it.
pub struct GuestProviderRegistry {
    /// This registry, weakly: what a composed provider's refresh publishes as its `update` re-composes
    /// its registration HERE ([`GuestProviderRegistry::compose_options`], PROV-164).
    this: Weak<GuestProviderRegistry>,
    /// Realized providers keyed by provider id, in insertion order (BTreeMap for a stable catalog).
    providers: Mutex<BTreeMap<String, Arc<dyn Provider>>>,
    /// Monotonic mutation counter — the CACHE KEY for the composed-registry snapshot
    /// (`crate::session::model_runtime::RegistrySnapshot`, CFG-020).
    ///
    /// The snapshot cannot key on this map by identity the way it keys on the installed provider
    /// and the catalog overlay (both `Arc`s whose replacement IS the mutation): a guest
    /// registration mutates this registry IN PLACE, behind the same `Arc` the session holds for its
    /// whole life. A counter is the identity that in-place mutation does have. `upsert_provider`,
    /// `upsert_live_provider` and `remove_provider` — the `ModelRegistrySink` impl below, and the
    /// ONLY three writers of `providers` — each bump it, so "the generation is unchanged" means "no
    /// guest catalog has changed" with no way for a write to slip past. A live provider replaced by
    /// a new `Arc` bumps it too, even when the catalogs happen to be equal: the `Arc` is what the
    /// session streams through.
    generation: AtomicU64,
    /// Per-provider catalog-refresh state (pi `refreshGenerations` / `refreshControllers` /
    /// `publicationChains`), behind an `Arc` because a refresh's background work owns a handle.
    refresh: Arc<RefreshInner>,
    /// The session's installed-provider slot, once [`GuestProviderRegistry::follow_installed`]
    /// bound it. A provider replaced under the id the slot holds is stored into it too, so the
    /// session streams through, lists and filters by the replacement, not by the `Arc` it was
    /// installed as (pi keeps ONE registry, so there is no second copy to go stale).
    installed: Mutex<Option<Weak<ProviderSwap>>>,
    /// Whether the startup restore ([`GuestProviderRegistry::restore_cached`]) has begun. Only
    /// after it does a provider registered under a new id restore its own cached catalog (see
    /// [`GuestProviderRegistry::begin_late_restore`]): everything registered before is covered by
    /// the startup restore, which would only supersede a second one.
    restore_started: AtomicBool,
    /// The session's virtual-model registry and the catalog its registrations validate against,
    /// once [`GuestProviderRegistry::attach_virtual_models`] bound them.
    ///
    /// This slot exists because `ModelRegistrySink` is ONE trait with ONE bind — upstream carries
    /// all five provider actions in one `providerActions` bag (`extensions/runner.ts:413-418`) — so
    /// the sink that answers `upsert_provider` is also the sink that answers
    /// `upsert_virtual_model`. The registry is pi's `ModelRuntime.virtualModels`
    /// (`model-runtime.ts:179`); the catalog is the two reads pi's `registerVirtualModel` takes off
    /// `this` (`:957`, `:962-968`).
    virtual_models: Mutex<Option<VirtualModelSink>>,
    /// SEAM-146 — the runtime the startup restore ran on, recorded by
    /// [`GuestProviderRegistry::restore_cached`] BEFORE it sets `restore_started`, so "the startup
    /// restore has begun" implies "there is a runtime to spawn a restore on". A registration made
    /// from a thread with no tokio runtime current spawns its restore here (see
    /// [`GuestProviderRegistry::runtime_handle`]).
    restore_runtime: Mutex<Option<tokio::runtime::Handle>>,
    /// PROV-162 — what a registration under an id is composed OVER: pi's `base =
    /// nativeExtensionProviders.get(id) ?? builtins.get(id)` (`core/model-runtime.ts:301-310`
    /// @f1b2e77f5), with the session's `models.json` layer applied where it has one (pi composes
    /// built-in → `models.json` → extension in one provider, `provider-composer.ts:555-560`). The
    /// session attaches one ([`GuestProviderRegistry::attach_base_resolver`]); unattached, the
    /// compiled-in built-in over the refresh store.
    base_resolver: Mutex<Option<BaseResolver>>,
    /// The ids whose provider is a registration composed over a base WITHOUT a key of its own: such
    /// an id is configured only through the composed auth (pi `checkAuth` over the composed provider,
    /// `model-runtime.ts:343`), not by being registered. See
    /// [`GuestProviderRegistry::carries_own_credentials`].
    keyless_composed: Mutex<BTreeSet<String>>,
    /// PROV-164 — the session's `models.json`, whose `modelOverrides` for an id apply over the rows a
    /// registration under it declares (pi's topmost user-config layer,
    /// `provider-composer.ts:553-575` @f1b2e77f5). See
    /// [`GuestProviderRegistry::attach_model_config`].
    model_config: Mutex<Option<Arc<cyrup_config::ModelFile>>>,
    /// PROV-165 — the session's pi.dev catalog overlay slot: the ONE input of the base that changes
    /// during a session (see [`BaseResolver`]). Attached with the resolver; unattached, the base
    /// never changes.
    catalog_overlay: Mutex<Option<Arc<cyrup_provider::CatalogOverlaySlot>>>,
    /// PROV-165 — every id whose provider is a registration COMPOSED over a base: the registration
    /// and the overlay its base was resolved with. Read on every read of this registry
    /// ([`Self::revalidate`]); written only with `providers` held, through
    /// [`Self::lock_unless_stale`].
    composed: Mutex<BTreeMap<String, Composition>>,
    /// Stamps each landed registration, so a re-composition of an id writes only while the
    /// registration it composed is still the id's ([`Composition::stamp`]).
    next_stamp: AtomicU64,
    /// SEAM-168 — when the installed slot holds the session's OWN built-in for a non-guest id: the
    /// id, the provider that was made and the overlay it was made over. Every read rebuilds it
    /// when the overlay moved ([`Self::follow_installed_builtin`]); a slot that no longer holds
    /// `made` voids it. Written with `providers` held (lock order: providers -> this -> the swap).
    installed_builtin: Mutex<Option<InstalledBuiltin>>,
    /// SEAM-167 — told after each provider action lands (pi re-reads its current model after each
    /// of the five, `core/agent-session.ts:3470-3490` @f1b2e77f5). Called with no lock of this
    /// registry held.
    action_listener: Mutex<Option<ProviderActionListener>>,
}

/// What [`GuestProviderRegistry::on_provider_action`] installs (SEAM-167).
pub(crate) type ProviderActionListener = Arc<dyn Fn() + Send + Sync>;

/// The overlay a provider in the installed slot was made over (SEAM-168).
#[derive(Clone)]
enum MadeOver {
    /// Made outside the session (the binary's launch provider): rebuilt on the next read.
    Unknown,
    /// Made by the session over this overlay.
    Overlay(Option<Arc<cyrup_provider::CatalogOverlay>>),
}

impl MadeOver {
    fn is(&self, now: Option<&Arc<cyrup_provider::CatalogOverlay>>) -> bool {
        match self {
            Self::Unknown => false,
            Self::Overlay(over) => same_overlay(over.as_ref(), now),
        }
    }
}

/// SEAM-171 — where the provider in the session's installed slot came from, which decides what the
/// session lists for its id (`crate::session::compose_physical_registry`).
///
/// pi composes one provider per id from `base = nativeExtensionProviders.get(id) ??
/// builtins.get(id)` (`core/model-runtime.ts:302` @f1b2e77f5), so whose provider holds the slot
/// is whose rows the id has:
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InstalledProvenance {
    /// A guest registration (JSON or live) under the id: its rows replace every other row of the
    /// id (PROV-162).
    Guest,
    /// The session's OWN built-in ([`InstalledBuiltin`], SEAM-168): the compiled-in rows over the
    /// overlay the slot holds, `models.json` over them — the same rows the compiled-in catalog
    /// lists for the id.
    SessionBuiltin,
    /// An embedder's provider — injected at build, or returned by a resolver that does not
    /// resolve the built-ins — never replaced (round 4). pi's native provider: its rows ARE the
    /// id's rows, and the compiled-in rows of the id are not listed beside them. It reaches the
    /// slot with the session's `models.json` block for its id composed over it (SEAM-173), so
    /// the provider here is pi's `composeModelProvider(id, native, config)`; with no block, the
    /// provider as given.
    Embedder,
}

/// The installed slot's provider when it is the session's own built-in (SEAM-168).
struct InstalledBuiltin {
    id: String,
    made: Arc<dyn Provider>,
    over: MadeOver,
}

/// Whether two provider handles are the same provider (by address; the vtable half of a `dyn`
/// pointer is not an identity).
fn same_provider(a: &Arc<dyn Provider>, b: &Arc<dyn Provider>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

/// Resolves the base provider a registration under `id` composes over, from the catalog overlay
/// the caller hands it (see [`GuestProviderRegistry::attach_base_resolver`]). `None`: no base owns
/// the id.
///
/// PROV-165 — the overlay is a PARAMETER, not something the resolver reads for itself, because it
/// is the one input of the base that changes during a session (`models.json` is read once, the
/// credential store and auth context are live objects whose reads happen at request time, and the
/// compiled-in catalogs do not change): the registry records the overlay each composition was
/// resolved with and composes again when the slot holds another.
pub type BaseResolver = Arc<
    dyn Fn(&str, Option<&Arc<cyrup_provider::CatalogOverlay>>) -> Option<Arc<dyn Provider>>
        + Send
        + Sync,
>;

/// What a composed id's provider was built from (PROV-165): the registration, the overlay its base
/// was resolved with, and the registration's stamp.
struct Composition {
    registration: ProviderRegistration,
    over: Option<Arc<cyrup_provider::CatalogOverlay>>,
    /// Bumped by every registration that lands for the id; a re-composition (a read that found
    /// the overlay moved, or the composed provider's own refresh) carries the stamp it read and
    /// writes nothing if a registration landed since.
    stamp: u64,
}

/// Why [`GuestProviderRegistry::compose_and_write`] composes: a registration landing (pi
/// `registerProvider` → `recomposeProvider`, which supersedes the id's refresh like every
/// `setProvider`), or the SAME registration composed again over the base as it is now (pi has no
/// such write: its composed `getAllModels` reads the base on every call, so nothing is superseded).
#[derive(Clone, Copy)]
enum ComposeCause {
    Registration,
    Recompose { stamp: u64 },
}

/// Whether two overlay reads are the same overlay: the slot replaces its `Arc` on every install, so
/// identity is the change. Held `Arc`s, so an address cannot be reused while it is compared.
fn same_overlay(
    a: Option<&Arc<cyrup_provider::CatalogOverlay>>,
    b: Option<&Arc<cyrup_provider::CatalogOverlay>>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        _ => false,
    }
}

/// What [`GuestProviderRegistry::attach_virtual_models`] binds: the registry a registration lands
/// in, and the catalog its two validations read.
struct VirtualModelSink {
    registry: Arc<cyrup_provider::VirtualModelRegistry>,
    catalog: Arc<dyn cyrup_provider::VirtualModelCatalog>,
}

/// The attached catalog with the LIVE guest-provider catalog layered over it: a guest-registered
/// id answers from its provider alone. (Since SEAM-169 the attached catalog is itself composed at
/// the check, guest rows included, so this layer changes no answer the production catalog gives;
/// it is what a fixed test catalog is layered under.)
struct LiveGuestCatalog<'a> {
    guest: &'a GuestProviderRegistry,
    snapshot: &'a dyn cyrup_provider::VirtualModelCatalog,
}

impl cyrup_provider::VirtualModelCatalog for LiveGuestCatalog<'_> {
    /// A guest-registered id answers from its LIVE provider alone: the snapshot was taken before
    /// any registration, and a registration over a built-in REPLACES the id's rows (pi validates
    /// against `this.models.getModel(providerId, id)`, the composed id, `model-runtime.ts:960`
    /// @f1b2e77f5), so a built-in row the registration no longer lists is not a conflict.
    fn get_model(&self, provider: &str, model_id: &str) -> Option<Model> {
        if let Some(guest) = self.guest.provider(provider) {
            return guest.get_model(model_id).cloned();
        }
        self.snapshot.get_model(provider, model_id)
    }

    fn models(&self) -> Vec<Model> {
        let guest_ids = self.guest.ids();
        let mut models = self.snapshot.models();
        models.retain(|m| !guest_ids.iter().any(|id| id == m.provider.as_str()));
        for m in self.guest.models() {
            if !models
                .iter()
                .any(|e| e.provider == m.provider && e.id == m.id)
            {
                models.push(m);
            }
        }
        models
    }

    fn has_configured_auth(&self, provider: &str) -> bool {
        self.snapshot.has_configured_auth(provider)
    }

    fn has_physical_provider(&self, provider: &str) -> bool {
        self.snapshot.has_physical_provider(provider)
            || self
                .guest
                .models()
                .iter()
                .any(|m| m.provider.as_str() == provider && !cyrup_provider::is_virtual_model(m))
    }
}

impl GuestProviderRegistry {
    /// An empty registry, behind the `Arc` every holder shares (the extension host's sink and the
    /// session's read view are the same one).
    pub fn new() -> Arc<Self> {
        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            providers: Mutex::default(),
            generation: AtomicU64::default(),
            refresh: Arc::default(),
            installed: Mutex::default(),
            restore_started: AtomicBool::default(),
            virtual_models: Mutex::default(),
            restore_runtime: Mutex::default(),
            base_resolver: Mutex::default(),
            keyless_composed: Mutex::default(),
            model_config: Mutex::default(),
            catalog_overlay: Mutex::default(),
            composed: Mutex::default(),
            next_stamp: AtomicU64::default(),
            installed_builtin: Mutex::default(),
            action_listener: Mutex::default(),
        })
    }

    /// The provider realized for `id`, if a guest registered one (cheap `Arc` clone). Poison-safe.
    ///
    /// Every public read of this registry first [`Self::revalidate`]s, so what it answers is the
    /// registrations composed over their bases AS THE BASES ARE NOW (PROV-165).
    pub fn provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.revalidate();
        self.lock().get(id).cloned()
    }

    /// Whether a guest provider with this id is registered.
    pub fn has_provider(&self, id: &str) -> bool {
        self.revalidate();
        self.lock().contains_key(id)
    }

    /// Every guest-registered provider's model catalog, unioned (Pi folds registered models into the
    /// shared `ModelRegistry.models`, model-registry.ts:917-940). Stable order by provider id.
    pub fn models(&self) -> Vec<Model> {
        self.revalidate();
        self.lock()
            .values()
            .flat_map(|p| p.models().to_vec())
            .collect()
    }

    /// PROV-165 — pi's composed provider reads its base on EVERY `getAllModels` call
    /// (`provider-composer.ts:555-575` @f1b2e77f5), so a pi.dev catalog reload of a built-in
    /// reaches a registration over it at once. A cyrup provider's rows are an immutable borrow, so
    /// the same contract is kept one level up: every read of this registry compares the overlay
    /// slot with the overlay each composition's base was resolved with, and composes again — the
    /// same registration, over the base as it is now — when they differ. A read that finds nothing
    /// moved costs one `RwLock` read and a pointer comparison per composed id.
    ///
    /// The base's other inputs do not change during a session (see [`BaseResolver`]), so "the
    /// overlay is the one the composition was resolved with" is "the composition is the one a
    /// per-read resolve would produce". The re-composition writes through
    /// [`Self::compose_and_write`], so through the one gate ([`Self::lock_unless_stale`]), and
    /// bumps the generation, which is what the session's composed-registry snapshot keys on.
    ///
    /// SEAM-168 — the same contract for the installed slot when it holds the session's OWN
    /// built-in for a non-guest id ([`Self::follow_installed_builtin`]): pi's provider for such an
    /// id is the `withRemoteCatalog`-wrapped built-in, whose rows read the overlay on every call
    /// (`core/model-runtime.ts:228-233` @f1b2e77f5), so the provider is rebuilt over the overlay
    /// the slot holds now before anything reads it.
    fn revalidate(&self) {
        let Some(slot) = poison_safe(&self.catalog_overlay).clone() else {
            return;
        };
        let now = slot.load();
        let moved: Vec<(ProviderRegistration, u64)> = poison_safe(&self.composed)
            .values()
            .filter(|c| !same_overlay(c.over.as_ref(), now.as_ref()))
            .map(|c| (c.registration.clone(), c.stamp))
            .collect();
        for (registration, stamp) in moved {
            let _ = self.compose_and_write(&registration, ComposeCause::Recompose { stamp });
        }
        self.follow_installed_builtin(now.as_ref());
    }

    /// SEAM-168 — rebuild the installed built-in over `now` when it was made over another overlay.
    ///
    /// The rebuild is the base resolver's answer for the id (the session's composition of the
    /// compiled-in catalog, `now` and `models.json` over the session's credentials — what a fresh
    /// `select_provider` would hand back), made OUTSIDE every lock. It is stored only if, under
    /// the provider map's lock (the lock every writer of the slot holds), the slot still holds the
    /// provider the record names and no guest has registered the id since: a writer that got there
    /// first wins, and a slot something else replaced voids the record.
    fn follow_installed_builtin(&self, now: Option<&Arc<cyrup_provider::CatalogOverlay>>) {
        let (id, made) = {
            let record = poison_safe(&self.installed_builtin);
            match record.as_ref() {
                Some(record) if !record.over.is(now) => {
                    (record.id.clone(), Arc::clone(&record.made))
                }
                _ => return,
            }
        };
        let Some(swap) = self.installed_swap() else {
            return;
        };
        let rebuilt = self.base_provider(&id, now);
        let providers = self.lock();
        let mut record = poison_safe(&self.installed_builtin);
        let still_ours = record
            .as_ref()
            .is_some_and(|r| same_provider(&r.made, &made))
            && same_provider(&swap.current_raw(), &made)
            && !providers.contains_key(&id);
        if !still_ours {
            if record
                .as_ref()
                .is_some_and(|r| same_provider(&r.made, &made))
            {
                *record = None;
            }
            return;
        }
        match rebuilt {
            Some(provider) => {
                swap.store(Arc::clone(&provider));
                *record = Some(InstalledBuiltin {
                    id,
                    made: provider,
                    over: MadeOver::Overlay(now.cloned()),
                });
            }
            // No built-in or `models.json` provider owns the id (a faux, an unconfigured
            // placeholder): the overlay can never reach it, so there is nothing to follow.
            None => *record = None,
        }
    }

    /// The session's installed-provider slot, if one is bound and alive.
    fn installed_swap(&self) -> Option<Arc<ProviderSwap>> {
        poison_safe(&self.installed)
            .as_ref()
            .and_then(Weak::upgrade)
    }

    /// The provider the installed slot holds now, brought up to date with the overlay first; `None`
    /// before a slot is bound. What the extensions' virtual-model check reads as the session's own
    /// provider (SEAM-169).
    pub(crate) fn installed_provider(&self) -> Option<Arc<dyn Provider>> {
        self.installed_swap().map(|swap| swap.current())
    }

    /// SEAM-171 — the provenance of `provider`, the provider the installed slot holds (read through
    /// [`ProviderSwap::current`], so already brought up to date with the overlay): a guest's when
    /// a registration owns its id, the session's own built-in when the [`InstalledBuiltin`] record
    /// names it, otherwise an embedder's.
    ///
    /// A provider the session made for an id no built-in owns (a faux, the unconfigured
    /// placeholder) has no record and answers `Embedder`; nothing compiled-in shares such an id,
    /// so the answer lists the same rows either way.
    pub(crate) fn provenance_of(&self, provider: &Arc<dyn Provider>) -> InstalledProvenance {
        let providers = self.lock();
        if providers.contains_key(provider.id().as_str()) {
            return InstalledProvenance::Guest;
        }
        let record = poison_safe(&self.installed_builtin);
        if record
            .as_ref()
            .is_some_and(|r| same_provider(&r.made, provider))
        {
            InstalledProvenance::SessionBuiltin
        } else {
            InstalledProvenance::Embedder
        }
    }

    /// SEAM-168 — record that the installed slot holds the session's built-in for its id, made
    /// OUTSIDE the session (the binary's launch provider, `select_provider` over the overlay it
    /// read then), and bring it up to date at once: the overlay it was made over is not known, so
    /// it is rebuilt over the one the slot holds. A no-op for a guest-registered id (its provider
    /// is the composition, PROV-165) and for an id no built-in owns.
    pub(crate) fn adopt_installed_builtin(&self) {
        let Some(swap) = self.installed_swap() else {
            return;
        };
        {
            let providers = self.lock();
            let made = swap.current_raw();
            let id = made.id().as_str().to_string();
            if providers.contains_key(&id) {
                return;
            }
            *poison_safe(&self.installed_builtin) = Some(InstalledBuiltin {
                id,
                made,
                over: MadeOver::Unknown,
            });
        }
        self.revalidate();
    }

    /// SEAM-168 — install the session's own built-in for `id` (a cross-provider `/model` select of
    /// a non-guest id, when the swap's resolver resolves the built-ins): made over the overlay the
    /// slot holds now and recorded, so it follows the next overlay. `false` when no built-in or
    /// `models.json` provider owns the id; the caller then asks the swap's resolver.
    pub(crate) fn install_builtin(&self, id: &str) -> bool {
        let Some(swap) = self.installed_swap() else {
            return false;
        };
        let over = self.overlay_now();
        let Some(provider) = self.base_provider(id, over.as_ref()) else {
            return false;
        };
        let providers = self.lock();
        if providers.contains_key(id) {
            // A guest registered the id meanwhile: its provider is the id's.
            swap.store(Arc::clone(providers.get(id).unwrap_or(&provider)));
            return true;
        }
        swap.store(Arc::clone(&provider));
        *poison_safe(&self.installed_builtin) = Some(InstalledBuiltin {
            id: id.to_string(),
            made: provider,
            over: MadeOver::Overlay(over),
        });
        true
    }

    /// SEAM-167 — call `listener` after each provider action lands: a registration, a live
    /// registration, an unregistration, a virtual model's registration or removal. Replaces any
    /// listener installed before.
    pub(crate) fn on_provider_action(&self, listener: ProviderActionListener) {
        *poison_safe(&self.action_listener) = Some(listener);
    }

    /// Tell the listener a provider action landed (no lock of this registry is held here).
    fn provider_action_landed(&self) {
        let listener = poison_safe(&self.action_listener).clone();
        if let Some(listener) = listener {
            listener();
        }
    }

    /// The mutation generation — bumped by every `upsert_provider` / `upsert_live_provider` /
    /// `remove_provider`.
    ///
    /// Read as a cache key, never as a count: a caller compares it with the value it last saw and
    /// recomposes when it differs. `Relaxed` is the right ordering for exactly that use — the
    /// recompose that follows a difference takes the registry's own `Mutex`, which is what
    /// publishes the new catalogs.
    pub fn generation(&self) -> u64 {
        // A moved overlay is composed in BEFORE the counter is read, so the caller keying a cache
        // on it sees the re-composition's bump (PROV-165).
        self.revalidate();
        self.generation.load(Ordering::Relaxed)
    }

    /// The registered provider ids (diagnostics).
    pub fn ids(&self) -> Vec<String> {
        self.revalidate();
        self.lock().keys().cloned().collect()
    }

    /// Bind the session's installed-provider slot: from now on a provider this registry replaces
    /// under the id the slot currently holds is stored into the slot as well, in the same
    /// operation. Without it the slot keeps the `Arc` it was installed as — a refresh's or
    /// `/llama`'s replacement reached the registry but never the provider the session streams
    /// through, which kept shadowing the replacement's catalog (the composed registry starts from
    /// the installed provider's models).
    /// Bind the virtual-model registry an extension registration lands in, plus the catalog its
    /// two validations read — pi's `registerVirtualModel` reads `this.models.getModel(providerId,
    /// id)` for the physical-conflict check (`model-runtime.ts:957`) and
    /// `recomposeProvider`/`configuredProviders` for the virtual-only marking (`:962-968`).
    ///
    /// Called by [`crate::SessionBuilder`] BEFORE
    /// [`cyrup_ext::ExtensionRegistry::bind_model_registry`], so the bind-time flush of the
    /// extensions' queued registrations has somewhere to land — which is upstream's ordering:
    /// `createAgentSessionServices` drains `pendingVirtualModelRegistrations` into a `modelRuntime`
    /// that already holds the catalog (`agent-session-services.ts:182-194`).
    pub fn attach_virtual_models(
        &self,
        registry: Arc<cyrup_provider::VirtualModelRegistry>,
        catalog: Arc<dyn cyrup_provider::VirtualModelCatalog>,
    ) {
        *poison_safe(&self.virtual_models) = Some(VirtualModelSink { registry, catalog });
    }

    ///
    /// PROV-162 — a provider ALREADY registered under the slot's id is stored into it at once: the
    /// builder flushes the extensions' startup registrations into this registry before it builds
    /// the slot, so a startup `registerProvider` over the installed provider's id (a proxy
    /// `baseUrl` over the built-in the session started on) would otherwise leave the session
    /// streaming the uncomposed built-in. pi has one registry; the composed provider is the only
    /// one under the id.
    ///
    /// SEAM-168 — the slot also asks this registry, before every read, to bring what it holds up
    /// to date with the catalog overlay ([`crate::provider_swap::InstalledFollower`]).
    ///
    /// SEAM-173 — otherwise the slot holds the provider the session was BUILT with. When the swap's
    /// resolver does not resolve the built-ins that is an embedder's provider, and it is composed
    /// here with the session's `models.json` block for its id, by the composer a live registration
    /// goes through ([`Self::with_models_json_over_native`], PROV-167): pi's native provider is
    /// the `base` the config layer composes over (`core/model-runtime.ts:301-315` @f1b2e77f5), so
    /// a `models.json` model under its id streams as itself and an override reaches the provider
    /// that streams. The session's own launch provider (a resolver that does) is
    /// `select_provider`'s, composed over `models.json` already; the provider a resolver returns
    /// later is composed in [`ProviderSwap`] (the same composer).
    pub fn follow_installed(&self, swap: &Arc<ProviderSwap>) {
        *poison_safe(&self.installed) = Some(Arc::downgrade(swap));
        let follower: Weak<dyn crate::provider_swap::InstalledFollower> = self.this.clone();
        swap.attach_follower(follower);
        let built_with = swap.current_raw();
        let id = built_with.id().as_str().to_string();
        if let Some(provider) = self.provider(&id) {
            swap.store(provider);
        } else if !swap.resolves_builtins() {
            let composed = self.with_models_json_over_native(&id, Arc::clone(&built_with));
            if !same_provider(&composed, &built_with) {
                swap.store(composed);
            }
        }
    }

    /// PROV-162 — install what a registration under an id composes over (see
    /// [`BaseResolver`]). Called by [`crate::SessionBuilder`] BEFORE the bind flush, so the
    /// extensions' startup registrations compose over the session's own built-ins and
    /// `models.json`.
    ///
    /// PROV-165 — `overlay` is the session's catalog overlay slot, which the resolver is handed on
    /// every resolve and which every read of this registry compares against
    /// ([`Self::revalidate`]).
    pub fn attach_base_resolver(
        &self,
        resolver: BaseResolver,
        overlay: Arc<cyrup_provider::CatalogOverlaySlot>,
    ) {
        *poison_safe(&self.base_resolver) = Some(resolver);
        *poison_safe(&self.catalog_overlay) = Some(overlay);
    }

    /// SEAM-166 — `file` without the blocks of the ids a guest registered: those ids' rows already
    /// carry their `models.json` layer UNDER the registration (pi's order built-in → `models.json`
    /// → extension, `provider-composer.ts:555-560` @f1b2e77f5), so composing the block over them
    /// again would put a `models.json` `baseUrl` back over the registration's. The ONE place that
    /// rule is spelled; every composition of `models.json` over rows that may include a guest
    /// provider's goes through it (`AgentSession::compose_model_registry`, the builder's step 3b).
    pub fn without_guest_blocks<'a>(
        &self,
        file: &'a cyrup_config::ModelFile,
    ) -> std::borrow::Cow<'a, cyrup_config::ModelFile> {
        let guest = self.ids();
        if !guest.iter().any(|id| file.providers.contains_key(id)) {
            return std::borrow::Cow::Borrowed(file);
        }
        let mut stripped = file.clone();
        stripped
            .providers
            .retain(|id, _| !guest.iter().any(|guest_id| guest_id == id));
        std::borrow::Cow::Owned(stripped)
    }

    /// PROV-164 — install the session's `models.json`, whose `modelOverrides` for an id apply over
    /// the rows a registration under it declares. Called by [`crate::SessionBuilder`] beside
    /// [`Self::attach_base_resolver`], BEFORE the bind flush. Unattached, no override applies.
    pub fn attach_model_config(&self, model_config: Arc<cyrup_config::ModelFile>) {
        *poison_safe(&self.model_config) = Some(model_config);
    }

    /// What composing `reg` over its base takes from this session (PROV-164): the id's
    /// `models.json` `modelOverrides`, and the `update` the composed provider publishes after its
    /// base refreshed — this registration composed again, over the base as it stands then, which is
    /// how a refreshed base reaches the composed rows (pi's composed `getAllModels` reads its base
    /// live, `provider-composer.ts:555-560` @f1b2e77f5; a cyrup provider's rows are an immutable
    /// borrow, so the provider is replaced instead). The update runs inside the publication, so
    /// its write goes through the same gate as every other ([`Self::lock_unless_stale`]).
    fn compose_options(&self, reg: &ProviderRegistration) -> ComposeOptions {
        let model_overrides = poison_safe(&self.model_config)
            .as_ref()
            .and_then(|file| file.providers.get(&reg.id))
            .map(|block| block.model_overrides.clone())
            .unwrap_or_default();
        let this = self.this.clone();
        let id = reg.id.clone();
        ComposeOptions {
            model_overrides,
            // The id's CURRENT registration, composed again — not a copy of the one this provider
            // was built from, which a re-registration may have replaced (PROV-165).
            refreshed: Some(Arc::new(move || {
                if let Some(registry) = this.upgrade() {
                    registry.recompose(&id);
                }
            })),
        }
    }

    /// Compose the id's current registration again over its base as it is now, if the id is
    /// composed over one (PROV-164's refresh `update`).
    fn recompose(&self, id: &str) {
        let current = poison_safe(&self.composed)
            .get(id)
            .map(|c| (c.registration.clone(), c.stamp));
        if let Some((registration, stamp)) = current {
            let _ = self.compose_and_write(&registration, ComposeCause::Recompose { stamp });
        }
    }

    /// PROV-167 — `native` with the session's `models.json` block for `id` composed over it. pi's
    /// `composeProvider` takes the native extension's provider as the BASE of the config layer
    /// (`base = nativeExtensionProviders.get(id) ?? builtins.get(id)`, then
    /// `composeModelProvider(id, base, config, extension)`, `core/model-runtime.ts:301-310`
    /// @f1b2e77f5), so a `baseUrl` / `modelOverrides` block for a native's id applies over its
    /// rows. (A JSON `registerProvider` under the id never composes over the native: pi deletes the
    /// native first, `:925`, and so does the hub.) The same composer `models.json` uses for the
    /// built-ins (`ModelFile::compose_providers`), over a collection holding only the native. A
    /// block it rejects leaves the native, as pi's `catch` returns `base` (`:312-315`).
    ///
    /// Re-run on every live registration, so a native's catalog publication (which re-registers)
    /// is composed again. The listing does not apply the block a second time: a guest id's block
    /// is stripped there ([`Self::without_guest_blocks`]).
    fn with_models_json_over_native(
        &self,
        id: &str,
        native: Arc<dyn Provider>,
    ) -> Arc<dyn Provider> {
        let block = poison_safe(&self.model_config)
            .as_ref()
            .and_then(|file| file.providers.get(id).cloned());
        let Some(block) = block else {
            return native;
        };
        let file = cyrup_config::ModelFile {
            providers: std::iter::once((id.to_string(), block)).collect(),
        };
        let mut models =
            cyrup_provider::create_models(cyrup_provider::CreateModelsOptions::default());
        models.set_provider(Arc::clone(&native));
        let (store, ctx) = self.refresh.auth();
        let errors = file.compose_providers(
            &mut models,
            store,
            Arc::new(cyrup_provider::builtin_registry()),
            Some(ctx),
        );
        if let Some(message) = errors.first() {
            tracing::warn!(provider = id, "{message}");
            return native;
        }
        models.get_provider(id).unwrap_or(native)
    }

    /// The overlay the base is resolved with now (`None` when no slot is attached).
    fn overlay_now(&self) -> Option<Arc<cyrup_provider::CatalogOverlay>> {
        poison_safe(&self.catalog_overlay)
            .as_ref()
            .and_then(|slot| slot.load())
    }

    /// The base a registration under `id` composes over, resolved with `overlay`: the attached
    /// resolver's answer, else the compiled-in built-in over the refresh engine's credential store.
    fn base_provider(
        &self,
        id: &str,
        overlay: Option<&Arc<cyrup_provider::CatalogOverlay>>,
    ) -> Option<Arc<dyn Provider>> {
        let resolver = poison_safe(&self.base_resolver).clone();
        match resolver {
            Some(resolver) => resolver(id, overlay),
            None => cyrup_provider::builtin_provider(
                id,
                self.refresh.auth().0,
                Arc::new(cyrup_provider::builtin_registry()),
                overlay.map(AsRef::as_ref),
            ),
        }
    }

    /// Whether the guest provider under `id` brings its own credentials, so being registered is
    /// enough to count as configured: a registration under a fresh id or a live provider (cyrup's
    /// standing rule, pi `hasConfiguredAuth` with a provider-request key), or a registration over a
    /// base that carries a key (pi `composeApiKeyAuth`'s `check`, `provider-composer.ts:445-451`).
    /// A KEY-LESS registration over a base is configured only through the composed auth's
    /// inherited check (`:453-455`), which the caller asks the base's own predicates. PROV-162.
    pub fn carries_own_credentials(&self, id: &str) -> bool {
        self.has_provider(id) && !poison_safe(&self.keyless_composed).contains(id)
    }

    /// The registered providers, by id (a snapshot of the map).
    pub fn providers(&self) -> Vec<(String, Arc<dyn Provider>)> {
        self.revalidate();
        self.lock()
            .iter()
            .map(|(id, provider)| (id.clone(), Arc::clone(provider)))
            .collect()
    }

    /// The id the bound installed-provider slot currently holds, if one is bound.
    fn installed_id(&self) -> Option<String> {
        self.installed_swap()
            .map(|swap| swap.current_raw().id().as_str().to_string())
    }

    /// Realize `reg` and write it: composed over the id's base when one owns the id (PROV-162),
    /// else the registration alone. Every write a registration makes, and every re-composition of
    /// one, is made here, in ONE hold of the provider map taken through [`Self::lock_unless_stale`]
    /// (SEAM-145, SEAM-151): the stale check, the supersede, the map, the composition record, the
    /// key-less set, the installed slot and the generation.
    ///
    /// Composed BEFORE the gate: composing writes nothing, and no lock is held across provider
    /// code. The base is resolved with the overlay read once here, and that overlay is what the
    /// record keeps, so a composition is never recorded as newer than the base it was made over
    /// (PROV-165).
    ///
    /// PROV-166 — a composition that fails leaves the BASE serving the id: pi's `composeProvider`
    /// records the error and returns `base` (`model-runtime.ts:312-315` @f1b2e77f5), which
    /// `recomposeProvider` registers in place of whatever the id held. So the id's composed
    /// provider and its record go, and the installed slot, when it holds the id, is handed the
    /// base — under the same hold as the other writes. The hub still records the registration
    /// (pi stores it, `:933`), which is why this answers [`ProviderWrite::Landed`].
    fn compose_and_write(&self, reg: &ProviderRegistration, cause: ComposeCause) -> ProviderWrite {
        let over = self.overlay_now();
        let composed = self.base_provider(&reg.id, over.as_ref()).map(|base| {
            let (store, ctx) = self.refresh.auth();
            let result = reg.compose_over(
                Arc::clone(&base),
                store,
                Some(ctx),
                self.compose_options(reg),
            );
            (base, result)
        });
        let Some(mut providers) = self.lock_unless_stale(&reg.id) else {
            return ProviderWrite::Superseded;
        };
        let mut records = poison_safe(&self.composed);
        let stamp = match cause {
            ComposeCause::Registration => {
                // pi `registerProvider` → `setProvider` / `deleteProvider`, both of which
                // supersede the id's refresh (`models.ts:405-413`).
                self.supersede_refresh(&reg.id);
                self.next_stamp.fetch_add(1, Ordering::Relaxed)
            }
            // A registration landed since this one was read: it owns the id, and it was composed
            // over a base no older than this one.
            ComposeCause::Recompose { stamp }
                if records.get(&reg.id).map(|c| c.stamp) != Some(stamp) =>
            {
                return ProviderWrite::Superseded;
            }
            ComposeCause::Recompose { stamp } => stamp,
        };
        let (provider, keyless) = match composed {
            Some((_, Ok(provider))) => {
                records.insert(
                    reg.id.clone(),
                    Composition {
                        registration: reg.clone(),
                        over,
                        stamp,
                    },
                );
                (
                    provider,
                    reg.resolved_api_key.as_deref().is_none_or(str::is_empty),
                )
            }
            Some((base, Err(message))) => {
                tracing::warn!(provider = %reg.id, "{message}");
                providers.remove(&reg.id);
                records.remove(&reg.id);
                poison_safe(&self.keyless_composed).remove(&reg.id);
                self.replace_installed(&reg.id, &base, Some(over));
                self.generation.fetch_add(1, Ordering::Relaxed);
                return ProviderWrite::Landed;
            }
            None => {
                records.remove(&reg.id);
                (reg.build_provider(), false)
            }
        };
        drop(records);
        providers.insert(reg.id.clone(), Arc::clone(&provider));
        {
            let mut keyless_composed = poison_safe(&self.keyless_composed);
            if keyless {
                keyless_composed.insert(reg.id.clone());
            } else {
                keyless_composed.remove(&reg.id);
            }
        }
        self.replace_installed(&reg.id, &provider, None);
        self.generation.fetch_add(1, Ordering::Relaxed);
        ProviderWrite::Landed
    }

    /// Store `provider` into the bound installed-provider slot when that holds the same id. Every
    /// caller holds the provider map's lock.
    ///
    /// SEAM-168 — `base_over` says what is stored: `None` a guest's provider (a composition or a
    /// live provider, which [`Self::revalidate`] keeps current through its own record), `Some` the
    /// id's BASE handed back by an unregistration or a failed composition, made over that overlay
    /// — the session's built-in, recorded so it follows the next overlay too.
    fn replace_installed(
        &self,
        id: &str,
        provider: &Arc<dyn Provider>,
        base_over: Option<Option<Arc<cyrup_provider::CatalogOverlay>>>,
    ) {
        if let Some(swap) = self.installed_swap()
            && swap.current_raw().id().as_str() == id
        {
            swap.store(Arc::clone(provider));
            *poison_safe(&self.installed_builtin) = base_over.map(|over| InstalledBuiltin {
                id: id.to_string(),
                made: Arc::clone(provider),
                over: MadeOver::Overlay(over),
            });
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<dyn Provider>>> {
        // Poison-safe: a panic elsewhere must not wedge model selection (R-00-009).
        self.providers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// ---- catalog refresh -----------------------------------------------------------------------

thread_local! {
    /// The provider id whose `ModelsPublication::update` is running on this thread, if any.
    ///
    /// A cyrup provider's catalog is an immutable borrow, so an `update` swaps the catalog by
    /// re-registering a new provider value under the same id — the registry's own replace path,
    /// which supersedes a refresh like every other `setProvider` does (pi `setProvider`,
    /// `models.ts:399-402`). The refresh that is PUBLISHING that update must not be superseded by
    /// it, or the second publish of a restore-then-network refresh could never land. The update
    /// closure is synchronous, so a thread-local names it exactly.
    ///
    /// It carries the publication's refresh GENERATION as well (SEAM-145), so the registry write the
    /// update makes can re-check, under the provider-map lock, that the publication is still the
    /// current one (see [`GuestProviderRegistry::stale_publication`]).
    static PUBLISHING: RefCell<Option<(String, u64)>> = const { RefCell::new(None) };
}

/// Whether the update running on this thread is publishing for `id` (any generation).
fn publishing_id_is(id: &str) -> bool {
    PUBLISHING.with(|slot| slot.borrow().as_ref().is_some_and(|(p, _)| p == id))
}

/// Restores [`PUBLISHING`] to what it was, even if the update unwinds.
struct PublishingGuard(Option<(String, u64)>);

impl PublishingGuard {
    fn enter(id: &str, generation: u64) -> Self {
        Self(PUBLISHING.with(|slot| slot.borrow_mut().replace((id.to_string(), generation))))
    }
}

impl Drop for PublishingGuard {
    fn drop(&mut self) {
        let previous = self.0.take();
        PUBLISHING.with(|slot| *slot.borrow_mut() = previous);
    }
}

/// One provider's refresh bookkeeping (pi `refreshGenerations[id]` + `refreshControllers[id]`).
#[derive(Default)]
struct RefreshSlot {
    /// Bumped by every [`RefreshInner::supersede`]; a publication carries the value it began with.
    generation: u64,
    /// The in-flight refresh's token, cancelled when it is superseded.
    controller: Option<CancelToken>,
}

/// The refresh engine's shared state (pi `ModelsImpl`'s refresh fields, `models.ts:389-391`).
struct RefreshInner {
    /// pi `modelNetworkEnabled` (`core/model-runtime.ts:182`): what a request that does not say
    /// `allowNetwork` gets. An offline run turns it off.
    network_enabled: AtomicBool,
    /// Generation and controller together under ONE lock, so superseding (bump + abort the previous
    /// controller) and beginning (bump + install the new one) are each atomic.
    slots: Mutex<HashMap<String, RefreshSlot>>,
    /// pi `publicationChains` (`models.ts:391`): publications for one provider apply one at a time,
    /// in arrival order (a `tokio` mutex queues fairly).
    chains: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// pi `modelsStore`. Process-local until the session attaches its file-backed store.
    store: Mutex<Arc<dyn ModelsStore>>,
    /// pi `credentials` + `authContext`, which `resolveRefreshCredential` reads.
    auth: Mutex<(Arc<dyn CredentialStore>, Arc<dyn AuthContext>)>,
    /// The detached tasks a refresh has spawned and that have not ended: the per-provider
    /// operations and the publications (see [`TaskTicket`]).
    in_flight: AtomicUsize,
}

/// One detached refresh task, counted while it lives. A refresh stops WAITING for its operation and
/// publications when it is aborted, but they run to completion on their own (pi's
/// `raceWithAbortSignal` does not cancel the promise either); this is what lets a caller, a test or
/// a shutdown, know when the last of them has ended.
struct TaskTicket(Arc<RefreshInner>);

impl TaskTicket {
    fn new(inner: &Arc<RefreshInner>) -> Self {
        inner.in_flight.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(inner))
    }
}

impl Drop for TaskTicket {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

/// What ends one provider's [`RefreshInner::refresh_one`], however it ends.
///
/// The operation is its own task, held only through a `JoinHandle` in the caller's `select!`, and
/// it is cancelled only through `signal`, a child of the caller's token. When the caller's future
/// is DROPPED (a task abort, a command future dropped on session teardown) nothing would ever
/// cancel that token: the operation would keep its network I/O going, and its publication would
/// still pass the `signal.is_cancelled()` gate, persisting and applying a catalog for a refresh
/// nobody is waiting for. Dropping this guard before [`Self::settled`] cancels the signal, which is
/// what that gate reads. Either way the provider's controller slot is released (pi's `finally`,
/// `models.ts:591-595`).
struct RefreshSettle {
    inner: Arc<RefreshInner>,
    id: String,
    generation: u64,
    signal: CancelToken,
    settled: bool,
}

impl RefreshSettle {
    /// The refresh ran to its end (or its caller aborted it): nothing to cancel on the way out.
    fn settled(&mut self) {
        self.settled = true;
        self.inner.finish(&self.id, self.generation);
    }
}

impl Drop for RefreshSettle {
    fn drop(&mut self) {
        if !self.settled {
            self.signal.cancel();
            self.inner.finish(&self.id, self.generation);
        }
    }
}

impl Default for RefreshInner {
    fn default() -> Self {
        Self {
            network_enabled: AtomicBool::new(true),
            slots: Mutex::new(HashMap::new()),
            chains: Mutex::new(HashMap::new()),
            store: Mutex::new(Arc::new(InMemoryModelsStore::new())),
            auth: Mutex::new((
                Arc::new(InMemoryCredentialStore::new()),
                Arc::new(EnvAuthContext),
            )),
            in_flight: AtomicUsize::new(0),
        }
    }
}

fn poison_safe<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl RefreshInner {
    /// pi `supersedeProviderRefresh` (`models.ts:480-489`): invalidate every publication the
    /// provider's in-flight refresh could still make, and abort it.
    fn supersede(&self, id: &str) -> u64 {
        let mut slots = poison_safe(&self.slots);
        let slot = slots.entry(id.to_string()).or_default();
        slot.generation += 1;
        if let Some(previous) = slot.controller.take() {
            previous.cancel();
        }
        slot.generation
    }

    /// pi `beginProviderRefresh` (`models.ts:491-496`). The new controller is a CHILD of the
    /// caller's token, so one token carries both pi's `callerSignal` and its `controller.signal`
    /// (`AbortSignal.any`, `models.ts:560`).
    fn begin(&self, id: &str, caller: &CancelToken) -> (u64, CancelToken) {
        let mut slots = poison_safe(&self.slots);
        let slot = slots.entry(id.to_string()).or_default();
        slot.generation += 1;
        if let Some(previous) = slot.controller.take() {
            previous.cancel();
        }
        let controller = caller.child_token();
        slot.controller = Some(controller.clone());
        (slot.generation, controller)
    }

    /// The `finally` of pi's per-provider body (`models.ts:591-595`): drop the controller if it is
    /// still this refresh's.
    fn finish(&self, id: &str, generation: u64) {
        if let Some(slot) = poison_safe(&self.slots).get_mut(id)
            && slot.generation == generation
        {
            slot.controller = None;
        }
    }

    fn is_current(&self, id: &str, generation: u64) -> bool {
        poison_safe(&self.slots)
            .get(id)
            .is_some_and(|slot| slot.generation == generation)
    }

    fn store(&self) -> Arc<dyn ModelsStore> {
        Arc::clone(&poison_safe(&self.store))
    }

    fn auth(&self) -> (Arc<dyn CredentialStore>, Arc<dyn AuthContext>) {
        let guard = poison_safe(&self.auth);
        (Arc::clone(&guard.0), Arc::clone(&guard.1))
    }

    fn chain(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            poison_safe(&self.chains)
                .entry(id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    /// pi `publishProviderModels`' queued body (`models.ts:504-518`): wait for the provider's
    /// earlier publications, refuse if aborted or superseded, persist, re-check, then run `update`.
    async fn publish_queued(
        &self,
        id: &str,
        generation: u64,
        signal: &CancelToken,
        publication: ModelsPublication,
    ) -> Result<bool, ProviderError> {
        let chain = self.chain(id);
        let _turn = chain.lock().await;
        if signal.is_cancelled() || !self.is_current(id, generation) {
            return Ok(false);
        }

        let store = self.store();
        let options = ModelsStoreOperationOptions {
            signal: Some(signal.clone()),
        };
        match publication.persist {
            Some(ModelsPersist::Delete) => store.delete(id, Some(&options)).await?,
            // The chat entry and the classifier models are pi's one `models` array, written by one
            // `write` (`extensions/llama/provider.ts:251-253`): ONE store operation, so the file
            // never holds new chat models beside old classifier models.
            Some(ModelsPersist::Write { entry, classifiers }) => {
                store
                    .write_with_classifiers(id, entry, classifiers, Some(&options))
                    .await?;
            }
            None => {}
        }

        // pi re-checks here and runs `update` in the same synchronous turn (`models.ts:521-522`
        // @f1b2e77f5), so nothing can supersede in between. Here another thread can, so this check
        // is only the cheap early-out: the DECISIVE one is made again by the registry write the
        // update performs, under the provider-map lock that every registry writer bumps the
        // generation under ([`GuestProviderRegistry::stale_publication`], SEAM-145).
        if signal.is_cancelled() || !self.is_current(id, generation) {
            return Ok(false);
        }
        if let Some(update) = publication.update {
            let _publishing = PublishingGuard::enter(id, generation);
            update();
        }
        // This `Ok(true)` does NOT reach a provider whose update was superseded mid-call: every
        // generation bump (`RefreshInner::supersede` / `begin`) cancels the superseded refresh's
        // controller in the same `slots` critical section, so by the time `stale_publication`
        // can see a stale generation the phase signal is already cancelled, and
        // `PhasePublisher::publish`'s `biased` select answers `Err(ProviderError::Aborted)`
        // (pinned by the SEAM-145 tests).
        Ok(true)
    }

    /// One phase of pi's `runProviderRefreshPhase` (`models.ts:527-544`). `Ok(false)` is a provider
    /// that has no `refreshModels` (pi's optional member being `undefined`). `force` is pi's
    /// `force: allowNetwork ? force : undefined` (`models.ts:541`): the caller passes `false` for the
    /// cache-only phase.
    #[allow(clippy::too_many_arguments)]
    async fn run_phase(
        self: &Arc<Self>,
        id: &str,
        provider: &Arc<dyn Provider>,
        credential: Option<Credential>,
        allow_network: bool,
        force: bool,
        generation: u64,
        signal: &CancelToken,
    ) -> Result<bool, ProviderError> {
        let store = self.store();
        let options = ModelsStoreOperationOptions {
            signal: Some(signal.clone()),
        };
        let stored = store.read(id, Some(&options)).await?;
        let stored_classifiers = store.read_classifier_models(id, Some(&options)).await?;
        // Everything the provider needs is ON the argument, so it survives a `tokio::spawn` inside
        // `refresh_models` (the provider hands the task a clone). PROV-111.
        let context = RefreshModelsContext {
            credential,
            stored,
            stored_classifiers,
            publisher: Some(Arc::new(PhasePublisher {
                inner: Arc::clone(self),
                id: id.to_string(),
                generation,
                signal: signal.clone(),
            })),
            allow_network,
            force,
            cancel: signal.clone(),
        };
        match provider.refresh_models(&context).await {
            None => Ok(false),
            Some(result) => result.map(|()| true),
        }
    }

    /// pi `resolveRefreshCredential` (`models.ts:608-635`): the credential the network phase runs
    /// with, or `None` when the provider cannot authenticate (the phase is then skipped).
    async fn resolve_credential(
        &self,
        provider: &Arc<dyn Provider>,
        stored: Option<&Credential>,
        signal: &CancelToken,
    ) -> Result<Option<Credential>, ProviderError> {
        let Some(auth) = provider.provider_auth() else {
            return Ok(None);
        };
        let (credentials, ctx) = self.auth();
        let probe = cyrup_config::login::auth_probe_model(provider.id());
        if let Some(Credential::Oauth { expires, .. }) = stored {
            if auth.oauth.is_none() {
                return Ok(None);
            }
            if cyrup_provider::auth::oauth::now_ms() < *expires {
                return Ok(stored.cloned());
            }
            if signal.is_cancelled() {
                return Ok(None);
            }
            // The refresh runs UNDER the credential store's lock inside `resolve_provider_auth`;
            // the post-refresh credential is then read back, as pi returns `modify`'s result.
            cyrup_provider::resolve_provider_auth(
                provider.id(),
                auth,
                &probe,
                credentials.as_ref(),
                ctx.as_ref(),
                cyrup_provider::AuthOverrides::default(),
            )
            .await?;
            return Ok(credentials
                .read(provider.id())
                .await?
                .filter(|credential| matches!(credential, Credential::Oauth { .. })));
        }

        let Some(api_key) = auth.api_key.as_ref() else {
            return Ok(None);
        };
        let credential =
            stored.filter(|credential| matches!(credential, Credential::ApiKey { .. }));
        let Some(result) = api_key.resolve(&probe, ctx.as_ref(), credential).await? else {
            return Ok(None);
        };
        Ok(Some(Credential::ApiKey {
            key: result.auth.api_key,
            env: result.env,
        }))
    }

    /// The per-provider body of pi's `refresh` (`models.ts:559-596`), minus the abort race.
    #[allow(clippy::too_many_arguments)]
    async fn operate(
        self: Arc<Self>,
        id: String,
        provider: Arc<dyn Provider>,
        generation: u64,
        signal: CancelToken,
        allow_network: bool,
        force: bool,
    ) -> Result<(), ProviderError> {
        let (credentials, _) = self.auth();
        let (stored_credential, credential_error) =
            match credentials.read(&ProviderId::from(id.as_str())).await {
                Ok(credential) => (credential, None),
                Err(error) => (None, Some(ProviderError::from(error))),
            };

        // Restore cached provider state before auth resolution or network access (`:570-571`).
        let refreshable = self
            .run_phase(
                &id,
                &provider,
                stored_credential.clone(),
                false,
                false,
                generation,
                &signal,
            )
            .await?;
        if let Some(error) = credential_error {
            return Err(error);
        }
        if !refreshable || !allow_network || signal.is_cancelled() {
            return Ok(());
        }

        let Some(credential) = self
            .resolve_credential(&provider, stored_credential.as_ref(), &signal)
            .await?
        else {
            return Ok(());
        };
        self.run_phase(
            &id,
            &provider,
            Some(credential),
            true,
            force,
            generation,
            &signal,
        )
        .await?;
        Ok(())
    }

    /// One provider's refresh from `begin` to `finish` (`models.ts:559-596`): the operation runs as
    /// its own task and the caller stops WAITING for it when the signal fires, which is pi's
    /// `raceWithAbortSignal` (the underlying promise is not cancelled either). What keeps a
    /// stranded operation harmless is the publication gate, not the race.
    async fn refresh_one(
        self: Arc<Self>,
        id: String,
        provider: Arc<dyn Provider>,
        allow_network: bool,
        force: bool,
        caller: CancelToken,
    ) -> Option<(String, ProviderError)> {
        let begun = self.begin(&id, &caller);
        self.drive(id, provider, begun, allow_network, force).await
    }

    /// [`Self::refresh_one`] after its `begin`. Split out so a caller can begin SYNCHRONOUSLY and
    /// drive later: whatever begins afterwards supersedes this refresh, never the other way round.
    async fn drive(
        self: Arc<Self>,
        id: String,
        provider: Arc<dyn Provider>,
        (generation, signal): (u64, CancelToken),
        allow_network: bool,
        force: bool,
    ) -> Option<(String, ProviderError)> {
        let mut settle = RefreshSettle {
            inner: Arc::clone(&self),
            id: id.clone(),
            generation,
            signal: signal.clone(),
            settled: false,
        };
        let ticket = TaskTicket::new(&self);
        let operation = tokio::spawn({
            let work = Arc::clone(&self).operate(
                id.clone(),
                provider,
                generation,
                signal.clone(),
                allow_network,
                force,
            );
            async move {
                let _ticket = ticket;
                work.await
            }
        });
        let settled = tokio::select! {
            biased;
            () = signal.cancelled() => None,
            joined = operation => Some(joined),
        };
        settle.settled();

        // pi records an error only `if (!signal.aborted)` (`models.ts:583`): an aborted or
        // superseded refresh is a cancellation, not a provider failure.
        let failure = match settled? {
            Ok(Ok(())) => return None,
            Ok(Err(error)) => error,
            Err(joined) => ProviderError::ModelSource(
                format!("refresh task for {id} ended abnormally: {joined}").into(),
            ),
        };
        // **EXT-099d — the `false` branch of this guard cannot be observed by a test, and here
        // is why.** Suppressing the error needs a cancel that is INVISIBLE at the `tokio::select!`
        // above (`:585-589`) and visible here. That select is `biased` with `signal.cancelled()`
        // first and yields `None` on that arm, which `settled?` turns into an early return — so
        // any cancellation already visible there never reaches this line. Between the select
        // resolving `Some(joined)` and this `then_some` there is no await point (`settle.settled()`
        // is synchronous), so no other task can be scheduled in the gap: on a current-thread
        // runtime the window is empty, and on a multi-threaded one it is a few instructions wide
        // and cannot be forced.
        //
        // The TRUE branch — an uncancelled refresh reporting its failure — is covered end to end
        // by `an_abort_during_the_network_phase_is_reported_and_is_not_a_provider_error`
        // (`tests/provider_refresh.rs:1240`), which drives a provider that answers an abort with
        // `ProviderError::Aborted` and asserts `result.errors.is_empty()`. That test passes through
        // the select's `None` arm, not through this guard, which is precisely why the guard itself
        // survives mutation.
        //
        // The guard stays because upstream has it (pi's `if (!signal.aborted)`, `models.ts:583`)
        // and because that instruction-wide window is real: a cancel landing in it must not be
        // reported as a provider failure. Unreachable by construction in a test, NOT untested
        // behaviour nobody looked at. Same disposition, same reasoning, as `EXT-099c`
        // (`cyrup-llama/src/provider.rs`'s post-fan-out abort guard).
        (!signal.is_cancelled()).then_some((id, failure))
    }
}

/// The publisher a provider's refresh phase hands its `refresh_models` (pi's `publish` closure,
/// `models.ts:539`): bound to the phase's provider, generation and signal.
struct PhasePublisher {
    inner: Arc<RefreshInner>,
    id: String,
    generation: u64,
    signal: CancelToken,
}

#[async_trait::async_trait]
impl ModelsPublisher for PhasePublisher {
    async fn publish(&self, publication: ModelsPublication) -> Result<bool, ProviderError> {
        // pi `raceWithAbortSignal(queued, signal)` (`models.ts:524`): the publication itself runs
        // to completion as its own task, so a publication that began is never abandoned half way
        // (its store write is one operation, `write_with_classifiers`); only the wait is
        // abortable.
        let inner = Arc::clone(&self.inner);
        let id = self.id.clone();
        let generation = self.generation;
        let signal = self.signal.clone();
        let ticket = TaskTicket::new(&self.inner);
        let queued = tokio::spawn(async move {
            let _ticket = ticket;
            inner
                .publish_queued(&id, generation, &signal, publication)
                .await
        });
        tokio::select! {
            biased;
            () = self.signal.cancelled() => Err(ProviderError::Aborted),
            joined = queued => joined.unwrap_or_else(|error| {
                Err(ProviderError::ModelSource(
                    format!("catalog publication for {} ended abnormally: {error}", self.id).into(),
                ))
            }),
        }
    }
}

/// Cache-only refreshes, BEGUN (generation bumped, controller installed, ticket taken) but not yet
/// running: a provider registered after startup (SEAM-141), or every provider left after a removal
/// (SEAM-144). Begun synchronously on the registering thread and spawned afterwards, so whatever
/// begins later supersedes these, never the other way round.
struct CachedRestore {
    inner: Arc<RefreshInner>,
    drives: Vec<CachedDrive>,
    runtime: tokio::runtime::Handle,
}

/// One provider's begun cache-only refresh inside a [`CachedRestore`].
struct CachedDrive {
    id: String,
    provider: Arc<dyn Provider>,
    begun: (u64, CancelToken),
    ticket: TaskTicket,
}

impl CachedRestore {
    /// Run them detached, as pi's `void this.refresh({ allowNetwork: false })` does
    /// (`core/model-runtime.ts:895`, `:941`, `:949` @f1b2e77f5): the outcome is nobody's to read,
    /// and a failing restore leaves the provider as it was.
    fn spawn(self) {
        let Self {
            inner,
            drives,
            runtime,
        } = self;
        for CachedDrive {
            id,
            provider,
            begun,
            ticket,
        } in drives
        {
            let inner = Arc::clone(&inner);
            runtime.spawn(async move {
                let _ticket = ticket;
                let _ = inner.drive(id, provider, begun, false, false).await;
            });
        }
    }
}

impl GuestProviderRegistry {
    /// SEAM-146 — the runtime a registration's detached restore is spawned on: the caller's own
    /// when it has one, else the one the startup restore ran on.
    ///
    /// pi has one event loop, so its `void this.refresh(...)` (`core/model-runtime.ts:895`, `:941`,
    /// `:949`, `:975`, `:984` @f1b2e77f5) is always scheduled. Here a guest can register from any
    /// thread (`ExtensionRegistry::register_provider` / `register_virtual_model` are synchronous),
    /// and `Handle::try_current()` alone silently skipped the restore for a thread with no runtime.
    /// `None` only when neither exists, which no restore path reaches after the startup restore has
    /// begun (it records its runtime first); the callers report that case rather than drop it.
    ///
    /// One corner stays: a recorded handle whose runtime has since shut down accepts the spawn and
    /// drops the task (its [`TaskTicket`] is released on drop). There is then no session left to
    /// list the catalog in.
    pub(crate) fn runtime_handle(&self) -> Option<tokio::runtime::Handle> {
        tokio::runtime::Handle::try_current()
            .ok()
            .or_else(|| poison_safe(&self.restore_runtime).clone())
    }

    /// pi's registration-time restore (SEAM-141): `registerNativeProvider` / `registerProvider`
    /// end in `void this.refresh({ allowNetwork: false })` (`core/model-runtime.ts:895`, `:941`
    /// @f1b2e77f5), so a provider that registers after startup lists its cached catalog at once.
    /// `None` when nothing is to be restored.
    ///
    /// `[CYRUP-DELTA]` pi restores on EVERY registration, because a pi provider changes its own
    /// catalog in place and no re-registration is involved. Here a catalog change IS a
    /// re-registration (the provider's `update` swaps a new provider value in, and `/llama`'s
    /// `set_catalog` does the same), so restoring on every registration would replace a
    /// just-published or just-set catalog with the stored one, and would supersede the very
    /// refresh that is publishing it. It restores only for an id that had NO provider (the caller
    /// checks), which is the only registration that has no catalog of its own to protect, and never
    /// for the engine's own re-registration of an id it is publishing ([`PUBLISHING`]). The
    /// functionality is the same: a provider registered late with a stored catalog lists it.
    ///
    /// Also skipped before the startup restore has begun (it covers everything registered until
    /// then) and for a static provider (pi's `refreshModels === undefined` filter). A registration
    /// from a thread with no tokio runtime runs on the startup restore's runtime (SEAM-146, see
    /// [`Self::runtime_handle`]).
    fn begin_late_restore(&self, id: &str, provider: &Arc<dyn Provider>) -> Option<CachedRestore> {
        if !self.restore_started.load(Ordering::SeqCst)
            || !provider.has_refresh_models()
            || publishing_id_is(id)
        {
            return None;
        }
        let Some(runtime) = self.runtime_handle() else {
            // SEAM-146's "reports" arm: unreachable once `restore_started` is set (the startup
            // restore recorded its runtime first), kept so a skip can never again be silent.
            tracing::warn!(
                provider = id,
                "no tokio runtime to restore the cached catalog of a late-registered provider on"
            );
            return None;
        };
        let inner = Arc::clone(&self.refresh);
        // The caller token is one nobody cancels: there is no caller to abort it.
        let begun = inner.begin(id, &CancelToken::new());
        let ticket = TaskTicket::new(&inner);
        Some(CachedRestore {
            inner,
            drives: vec![CachedDrive {
                id: id.to_string(),
                provider: Arc::clone(provider),
                begun,
                ticket,
            }],
            runtime,
        })
    }

    /// SEAM-144 — pi's `unregisterProvider` ends in `void this.refresh({ allowNetwork: false })`
    /// (`core/model-runtime.ts:944-950`, the call at `:949` @f1b2e77f5), a WHOLE-registry refresh:
    /// `models.refresh` with no `providers` begins a cache-only refresh of every refreshable
    /// provider (`packages/ai/src/models.ts:558-565`, `:577`). This begins the same over `remaining`
    /// (the map after the removal, still locked by the caller), so a removal re-publishes the other
    /// providers' cached catalogs.
    ///
    /// pi-parity, stated: like pi's `beginProviderRefresh` (`models.ts:565`) this supersedes each
    /// other provider's in-flight refresh and republishes its STORED catalog — the clobber
    /// SEAM-141 refused for a same-id RE-registration. It is accepted here because it is what pi
    /// does on a removal and what the virtual-model listener
    /// ([`crate::virtual_models::CachedRestoreOnVirtualChange`]) already does on
    /// `registerVirtualModel` / `unregisterVirtualModel` (`:975`, `:984`).
    ///
    /// `[CYRUP-DELTA]` skipped before the startup restore has begun (pi fires it regardless): the
    /// startup restore, which runs over every provider still registered, covers that case. The id
    /// an update is publishing on this thread is skipped, so a removal made from inside an `update`
    /// does not supersede the refresh publishing it.
    fn begin_removal_restore(
        &self,
        remaining: &BTreeMap<String, Arc<dyn Provider>>,
    ) -> Option<CachedRestore> {
        if !self.restore_started.load(Ordering::SeqCst) {
            return None;
        }
        let targets: Vec<(&String, &Arc<dyn Provider>)> = remaining
            .iter()
            .filter(|(id, provider)| provider.has_refresh_models() && !publishing_id_is(id))
            .collect();
        if targets.is_empty() {
            return None;
        }
        let Some(runtime) = self.runtime_handle() else {
            tracing::warn!(
                providers = targets.len(),
                "no tokio runtime to restore the cached catalogs on after a provider removal"
            );
            return None;
        };
        let inner = Arc::clone(&self.refresh);
        let drives = targets
            .into_iter()
            .map(|(id, provider)| CachedDrive {
                id: id.clone(),
                provider: Arc::clone(provider),
                begun: inner.begin(id, &CancelToken::new()),
                ticket: TaskTicket::new(&inner),
            })
            .collect();
        Some(CachedRestore {
            inner,
            drives,
            runtime,
        })
    }

    /// Spawn the whole-registry cache-only restore ([`Self::restore_cached`]) detached — the
    /// virtual-model listener's `void this.refresh({ allowNetwork: false })`
    /// (`core/model-runtime.ts:975`, `:984` @f1b2e77f5). Its [`TaskTicket`] is taken here, on the
    /// calling thread, so [`Self::refresh_tasks_in_flight`] counts it before it starts. SEAM-146:
    /// spawned on [`Self::runtime_handle`], and reported when there is none.
    pub(crate) fn spawn_restore_cached(self: &Arc<Self>, cancel: CancelToken) {
        let Some(runtime) = self.runtime_handle() else {
            tracing::warn!(
                "no tokio runtime to restore the cached catalogs on after a virtual-model change"
            );
            return;
        };
        let ticket = TaskTicket::new(&self.refresh);
        let registry = Arc::clone(self);
        runtime.spawn(async move {
            let _ticket = ticket;
            registry.restore_cached(cancel).await;
        });
    }

    /// Invalidate the in-flight refresh of `id` (pi `setProvider` / `deleteProvider`, which both
    /// start with `supersedeProviderRefresh`, `models.ts:405-413`) — unless this IS the update a
    /// refresh is publishing for `id`, see [`PUBLISHING`]. Every caller holds the provider-map lock
    /// (SEAM-145).
    fn supersede_refresh(&self, id: &str) {
        if publishing_id_is(id) {
            return;
        }
        self.refresh.supersede(id);
    }

    /// SEAM-145 — whether the registry write about to be made for `id` is a STALE publication's:
    /// an `update` running on this thread for `(id, generation)` whose refresh has been superseded
    /// since `publish_queued`'s last `is_current` check.
    ///
    /// THE WINDOW THIS CLOSES: from that check (`publish_queued`) to the update's registry write.
    /// pi has none — its re-check and `publication.update?.()` are back to back in one synchronous
    /// turn (`packages/ai/src/models.ts:521-522` @f1b2e77f5). Here another thread could re-register
    /// (or remove) the id in between, and the stale update then overwrote the newer provider (or
    /// resurrected the removed one), because its own write does not supersede. The caller makes
    /// this check while HOLDING the provider-map lock, and every registry write of an id
    /// (`upsert_provider`, `upsert_live_provider`, `remove_provider`), and every `begin` made from
    /// one (a late or removal restore), bumps the refresh generation under that same lock. So
    /// either the newer writer's bump came first and the stale write is dropped here, or the stale
    /// write fully precedes the newer writer, which then overwrites it: the newer writer always
    /// wins. A plain refresh's own `begin` (`RefreshInner::refresh_one`, used by `refresh`,
    /// `refresh_all` and `restore_cached`) bumps the generation OUTSIDE the lock; that is harmless,
    /// because that `begin` writes nothing to the map: if it lands before this locked check the
    /// stale write is dropped, and if after, the newer refresh publishes later and overwrites it.
    /// No lock is held across the provider's `update` code.
    ///
    /// SEAM-151 — this is the ONLY staleness check on the publication path, and everything else
    /// the update writes obeys it: each sink write answers [`ProviderWrite`], and the extension
    /// host's hub and owner map and the provider's own copy (`cyrup-llama`'s `current`) record a
    /// change only on [`ProviderWrite::Landed`]. So a stale update's re-registration, which reaches
    /// here last, changes nothing anywhere. The provider is also told: a superseded refresh's signal
    /// is cancelled with the bump, so its `publish` returns `Err(ProviderError::Aborted)`, never
    /// `Ok(true)`.
    fn stale_publication(&self, id: &str) -> bool {
        PUBLISHING.with(|slot| {
            slot.borrow()
                .as_ref()
                .is_some_and(|(p, generation)| p == id && !self.refresh.is_current(id, *generation))
        })
    }

    /// The provider map, locked, unless the write about to be made for `id` is a superseded
    /// publication's ([`Self::stale_publication`]): then `None`, and the writer writes nothing and
    /// answers [`ProviderWrite::Superseded`]. The three writers of the map take it only through
    /// here, so the check is made once, in one place, under the same hold of the lock as the write
    /// it guards (SEAM-145, SEAM-151).
    fn lock_unless_stale(
        &self,
        id: &str,
    ) -> Option<std::sync::MutexGuard<'_, BTreeMap<String, Arc<dyn Provider>>>> {
        let providers = self.lock();
        (!self.stale_publication(id)).then_some(providers)
    }

    /// Back catalog persistence with `store` — the session's `<agent_dir>/models-store.json`
    /// (pi `modelsStore`, `core/model-runtime.ts`). Until attached, a process-local store.
    pub fn attach_models_store(&self, store: Arc<dyn ModelsStore>) {
        *poison_safe(&self.refresh.store) = store;
    }

    /// Back credential reads and provider-auth resolution with the session's own store and ambient
    /// context (pi `credentials` / `authContext`). Until attached, an empty store and the process
    /// environment.
    pub fn attach_refresh_auth(
        &self,
        credentials: Arc<dyn CredentialStore>,
        ctx: Arc<dyn AuthContext>,
    ) {
        *poison_safe(&self.refresh.auth) = (credentials, ctx);
    }

    /// How many detached refresh tasks (per-provider operations and publications) are still
    /// running. A refresh whose caller aborted returns at once while these finish on their own;
    /// this is how a test or a shutdown sees the last of them end.
    pub fn refresh_tasks_in_flight(&self) -> usize {
        self.refresh.in_flight.load(Ordering::SeqCst)
    }

    /// pi `modelNetworkEnabled` (`core/model-runtime.ts:182`): whether a refresh that does not name
    /// `allow_network` may use the network — the interactive startup refresh and the post-`/login`
    /// refresh both leave it unnamed. The session builder sets it from
    /// [`crate::SessionConfig::model_network_enabled`], which the binary derives from the offline
    /// switch (`--offline` / `CYRUP_OFFLINE`); an explicit `allow_network` on a request overrides
    /// it in both directions.
    pub fn set_network_enabled(&self, enabled: bool) {
        self.refresh
            .network_enabled
            .store(enabled, Ordering::Relaxed);
    }

    /// pi `ModelsImpl.refresh(options)` (`packages/ai/src/models.ts:546-606` @v0.99.2-17), for the
    /// providers this registry holds.
    ///
    /// Every selected provider (all when `request.providers` is `None`; unknown ids and static
    /// providers are ignored) is refreshed concurrently:
    ///
    /// 1. its stored catalog and classifier models are read and handed to
    ///    [`cyrup_provider::Provider::refresh_models`] with `allow_network: false` — the cache-only
    ///    restore, which runs even when no network is allowed (`:570-571`);
    /// 2. when the request allows the network, nobody aborted, and the provider's auth resolves to
    ///    a credential (`:573-576`), `refresh_models` runs again with `allow_network: true`.
    ///
    /// A provider reads its credential and stored catalog from, and publishes `{ persist, update }`
    /// through, the [`RefreshModelsContext`] it is handed; a publication of a refresh that was aborted
    /// or superseded by a newer one (a later `refresh`, or a re-registration of the provider)
    /// writes nothing and runs no update — pi 0.84.0's "stale catalog refreshes could publish
    /// after a newer refresh" fix (`coding-agent` CHANGELOG:760).
    ///
    /// Failures are returned, never thrown (`:584-590`): `errors` holds one entry per provider
    /// whose refresh failed, and an aborted provider contributes none. `aborted` is the caller's
    /// token at return time (`:605`).
    pub async fn refresh(&self, request: ProviderRefreshRequest) -> ModelsRefreshResult {
        let mut result = ModelsRefreshResult::default();
        // `if (callerSignal.aborted) return { aborted: true, errors }` (`:550`).
        if request.cancel.is_cancelled() {
            result.aborted = true;
            return result;
        }
        // pi `options.allowNetwork ?? this.modelNetworkEnabled` (`core/model-runtime.ts:850`).
        let allow_network = request
            .allow_network
            .unwrap_or_else(|| self.refresh.network_enabled.load(Ordering::Relaxed));
        let selected: Option<BTreeSet<String>> = request
            .providers
            .map(|ids| ids.into_iter().collect::<BTreeSet<_>>());
        let targets: Vec<(String, Arc<dyn Provider>)> = self
            .lock()
            .iter()
            // pi `provider.refreshModels !== undefined && (!selected || selected.has(id))`
            // (`models.ts:553-554`) — BEFORE `beginProviderRefresh`, so a static provider never
            // supersedes an in-flight refresh of its id (SEAM-142).
            .filter(|(id, provider)| {
                provider.has_refresh_models()
                    && selected
                        .as_ref()
                        .is_none_or(|ids| ids.contains(id.as_str()))
            })
            .map(|(id, provider)| (id.clone(), Arc::clone(provider)))
            .collect();

        let refreshes = targets.into_iter().map(|(id, provider)| {
            Arc::clone(&self.refresh).refresh_one(
                id,
                provider,
                allow_network,
                request.force,
                request.cancel.clone(),
            )
        });
        result.errors = futures::future::join_all(refreshes)
            .await
            .into_iter()
            .flatten()
            .collect();
        result.aborted = request.cancel.is_cancelled();
        result
    }

    /// pi's startup `modelRuntime.refresh({ signal })` over every provider the registry holds, with
    /// the request's `allowNetwork` left to the registry's own switch
    /// ([`Self::set_network_enabled`], pi `modelNetworkEnabled`) — the call rpc mode detaches after
    /// the runtime exists (`main.ts:931-936` @v0.99.2-17) and the interactive host makes once its UI
    /// is up (`interactive-mode.ts:1115-1126`). A provider with no resolvable credential is skipped
    /// by the engine itself (`models.ts:573-576`), so nothing is fetched for one that is not set up.
    pub async fn refresh_all(&self, cancel: CancelToken) -> ModelsRefreshResult {
        self.refresh(ProviderRefreshRequest {
            providers: None,
            allow_network: None,
            force: false,
            cancel,
        })
        .await
    }

    /// The startup restore: every provider's stored catalog is handed to its `refresh_models` with
    /// no network (pi `modelRuntime.refresh({ allowNetwork: false })` after the extensions'
    /// providers are registered, `core/agent-session-services.ts:190-206` @v0.99.2-17).
    pub async fn restore_cached(&self, cancel: CancelToken) -> ModelsRefreshResult {
        // SEAM-146: record the runtime BEFORE `restore_started`, so a restore that sees the flag
        // always has a runtime to spawn on, even when its registration comes from a thread with
        // none. (This is an `async fn`, polled on a tokio runtime.)
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            *poison_safe(&self.restore_runtime) = Some(runtime);
        }
        // Set BEFORE the targets are snapshotted: a provider registered from here on either is in
        // the snapshot or restores itself, and one registered a moment earlier is in the snapshot.
        self.restore_started.store(true, Ordering::SeqCst);
        self.refresh(ProviderRefreshRequest {
            providers: None,
            allow_network: Some(false),
            force: false,
            cancel,
        })
        .await
    }
}

#[async_trait::async_trait]
impl ProviderRefresher for GuestProviderRegistry {
    async fn refresh(&self, request: ProviderRefreshRequest) -> ModelsRefreshResult {
        GuestProviderRegistry::refresh(self, request).await
    }
}

impl crate::provider_swap::InstalledFollower for GuestProviderRegistry {
    fn follow_overlay(&self) {
        self.revalidate();
    }

    fn compose_native(&self, provider: Arc<dyn Provider>) -> Arc<dyn Provider> {
        let id = provider.id().as_str().to_string();
        self.with_models_json_over_native(&id, provider)
    }
}

impl ModelRegistrySink for GuestProviderRegistry {
    fn upsert_provider(&self, reg: &ProviderRegistration) -> ProviderWrite {
        // Full replacement for this provider id (Pi "replaces all models", model-registry.ts:919).
        let write = self.compose_and_write(reg, ComposeCause::Registration);
        if write.landed() {
            self.provider_action_landed();
        }
        write
    }

    fn upsert_live_provider(&self, id: &str, provider: Arc<dyn Provider>) -> ProviderWrite {
        // PROV-167 — the session's `models.json` block for the id, composed over the native's
        // provider (composed BEFORE the gate: no lock is held across provider code).
        let provider = self.with_models_json_over_native(id, provider);
        // A native's own provider, stored as given — no `ConfigProvider` is rebuilt. Replacing the
        // id swaps the `Arc` (Pi "replaces all models", model-registry.ts:919), and the bump is what
        // makes `full_model_registry()` recompose so the replacement's catalog is what `/model` lists.
        let late_restore = {
            // The stale check, supersede, replace, (for a new id) the restore's begin, the
            // installed slot and the bump under ONE hold of the map, so two concurrent
            // registrations of an id cannot interleave them, and a publication superseded after
            // `publish_queued`'s check cannot land over the newer provider (SEAM-145, see
            // `stale_publication`). Lock order: providers -> refresh slots -> installed -> swap.
            let Some(mut providers) = self.lock_unless_stale(id) else {
                return ProviderWrite::Superseded;
            };
            self.supersede_refresh(id);
            let is_new = providers
                .insert(id.to_string(), Arc::clone(&provider))
                .is_none();
            poison_safe(&self.keyless_composed).remove(id);
            // A live provider is not a composition: nothing re-composes it (PROV-165).
            poison_safe(&self.composed).remove(id);
            let late_restore = if is_new {
                self.begin_late_restore(id, &provider)
            } else {
                None
            };
            self.replace_installed(id, &provider, None);
            self.generation.fetch_add(1, Ordering::Relaxed);
            late_restore
        };
        // Spawned LAST: the restore's own `update` re-registers the provider, and must land after
        // this registration has finished installing, not before it.
        if let Some(restore) = late_restore {
            restore.spawn();
        }
        self.provider_action_landed();
        ProviderWrite::Landed
    }

    fn remove_provider(&self, id: &str) -> ProviderWrite {
        // pi `unregisterProvider` recomposes the id, which leaves its BASE registered
        // (`model-runtime.ts:944-950` → `composeProvider`, `:301-310` @f1b2e77f5). The map entry
        // going is that for every reader of the map; the installed slot is a COPY of the entry, so
        // when it holds this id it is handed the base (resolved here, outside the lock, and only
        // then). An id no base owns leaves the slot as it was.
        let base = self
            .installed_id()
            .is_some_and(|installed| installed == id)
            .then(|| {
                let over = self.overlay_now();
                self.base_provider(id, over.as_ref())
                    .map(|base| (base, over))
            })
            .flatten();
        let restore = {
            // Same single hold as the upserts (SEAM-145): a stale publication's removal is
            // dropped, and the supersede happens under the lock its check reads under.
            let Some(mut providers) = self.lock_unless_stale(id) else {
                return ProviderWrite::Superseded;
            };
            self.supersede_refresh(id);
            providers.remove(id);
            poison_safe(&self.keyless_composed).remove(id);
            poison_safe(&self.composed).remove(id);
            if let Some((base, over)) = base {
                self.replace_installed(id, &base, Some(over));
            }
            // SEAM-144: begun under the same hold, so a registration that follows supersedes
            // this restore, never the reverse.
            let restore = self.begin_removal_restore(&providers);
            self.generation.fetch_add(1, Ordering::Relaxed);
            restore
        };
        if let Some(restore) = restore {
            restore.spawn();
        }
        self.provider_action_landed();
        ProviderWrite::Landed
    }

    fn upsert_virtual_model(
        &self,
        definition: &cyrup_provider::VirtualModelDefinition,
    ) -> Result<(), String> {
        // Not `self.generation`: the virtual registry carries its OWN monotonic generation, and
        // that counter is already the fourth key of `RegistrySnapshot` (step 2's contract), so a
        // registration invalidates the composed-catalog cache through it. Bumping this counter too
        // would invalidate the guest-provider half for no reason.
        let Some(sink) = poison_safe(&self.virtual_models)
            .as_ref()
            .map(|s| VirtualModelSink {
                registry: Arc::clone(&s.registry),
                catalog: Arc::clone(&s.catalog),
            })
        else {
            // No registry bound: nothing in this process can route, so a registration cannot be
            // honoured. Refused rather than dropped, because the hub turns the refusal into pi's
            // `Extension "{path}" error: …` startup diagnostic — a silent drop is how an extension
            // author debugs a router that "registered" and never appears in `/model`.
            return Err("virtual-model registry is not bound to this session".to_string());
        };
        // The guard is RELEASED before `register`, which takes the registry's own lock and reads
        // the catalog under it. Holding this `Mutex` across that call would nest two locks on a
        // path an extension can drive from a background task.
        //
        // SEAM-169 — the attached catalog is the session's physical registry composed AT THIS
        // CHECK (`virtual_models::LiveVirtualCatalog`), overlay included: pi checks against its
        // live runtime (`this.models.getModel(providerId, id)`, `core/model-runtime.ts:960`
        // @f1b2e77f5), so a physical id only a reloaded pi.dev catalog lists is a conflict. (A
        // catalog refresh AFTER the registration is upstream's other case: "If a catalog refresh
        // later adds a physical model with the same ID, the virtual model hides it",
        // `docs/virtual-models.md` — a later arrival is hidden, never a conflict.) The live guest
        // layer over it answers a guest-registered id from its provider alone, as before.
        let catalog = LiveGuestCatalog {
            guest: self,
            snapshot: sink.catalog.as_ref(),
        };
        let registered = sink
            .registry
            .register(definition.clone(), &catalog)
            .map_err(|e| e.to_string());
        if registered.is_ok() {
            self.provider_action_landed();
        }
        registered
    }

    fn remove_virtual_model(&self, provider: &str, id: &str) {
        let registry = poison_safe(&self.virtual_models)
            .as_ref()
            .map(|s| Arc::clone(&s.registry));
        // `false` is pi's no-op arm (`model-runtime.ts:977`): an absent pair recomposes nothing
        // and fires no refresh. `unregister` enforces that itself.
        if let Some(registry) = registry
            && registry.unregister(provider, id)
        {
            self.provider_action_landed();
        }
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
    use cyrup_core::ExtensionId;
    use cyrup_ext::registry::ExtensionRegistry;
    use serde_json::json;

    fn config() -> serde_json::Value {
        json!({
            "name": "Acme",
            "baseUrl": "https://acme.test/v1",
            "api": "openai-completions",
            "apiKey": "sk-acme-123",
            "models": [{ "id": "acme-fast", "name": "Acme Fast", "contextWindow": 64000, "maxTokens": 4096 }],
        })
    }

    /// Binding the registry flushes a queued guest registration into it (Pi `bindCore` pending flush).
    #[test]
    fn bind_flushes_pending_registration() {
        let ext = ExtensionRegistry::new();
        ext.register_provider(ExtensionId::from("acme-ext"), "acme", config())
            .unwrap();
        let sink: Arc<GuestProviderRegistry> = GuestProviderRegistry::new();
        ext.bind_model_registry(sink.clone()).unwrap();

        assert!(sink.has_provider("acme"));
        let models = sink.models();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id.as_str(), "acme-fast");
        assert_eq!(models[0].provider.as_str(), "acme");
        assert_eq!(models[0].context_window, 64000);
    }

    /// A registration made AFTER bind upserts immediately (Pi post-`bindCore` live registration).
    #[test]
    fn live_registration_after_bind_upserts_immediately() {
        let ext = ExtensionRegistry::new();
        let sink: Arc<GuestProviderRegistry> = GuestProviderRegistry::new();
        ext.bind_model_registry(sink.clone()).unwrap();
        assert!(!sink.has_provider("acme"));

        ext.register_provider(ExtensionId::from("acme-ext"), "acme", config())
            .unwrap();
        assert!(sink.has_provider("acme"));
        assert!(
            sink.provider("acme")
                .unwrap()
                .get_model("acme-fast")
                .is_some()
        );

        ext.unregister_provider("acme").unwrap();
        assert!(!sink.has_provider("acme"));
    }
}
