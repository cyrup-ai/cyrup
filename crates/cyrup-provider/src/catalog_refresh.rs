//! The coordinated, whole-catalog refresh (XAI_3) — the port of pi's
//! `ModelCatalogRefreshCoordinator` (`modes/interactive/model-catalog-refresh.ts`, whole file).
//!
//! # What this is NOT
//!
//! It is not the thing that stops two refreshes hitting the network twice. `RemoteCatalog` already
//! does that per provider ([`crate::utils::refresh::RefreshDedup`], `remote_catalog.rs:549-560`),
//! and `RefreshOptions::network()`'s 4h freshness window stops it again. This layer exists for the
//! properties that live ABOVE one provider's fetch: one shared `load_overlay`+install pass, one
//! shared typed result for every joiner, per-caller deadlines that do not cancel each other, and an
//! abort that fires only when the LAST waiter leaves.
//!
//! # The one place this deliberately diverges from `RefreshDedup`
//!
//! `RefreshDedup` does not spawn: its shared future only advances while a caller polls it. That is
//! right for a fetch a caller is always driving and WRONG here. pi's semantics are a JS promise's —
//! the operation runs on its own, and "everybody's deadline fired" ABORTS it. A non-spawned shared
//! future in that situation is neither finished nor cancelled; it is parked mid-request, and the
//! next joiner resumes a half-finished HTTP exchange. So the operation is `tokio::spawn`ed and the
//! coordinator hands out a [`futures::future::Shared`] over its completion, with a [`CancelToken`]
//! for the waiters-reached-zero abort.

use crate::remote_catalog::{CatalogOverlay, RefreshOptions, RemoteCatalog};
use cyrup_core::CancelToken;
use futures::future::{BoxFuture, FutureExt, Shared};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

/// The LIVE pi.dev overlay: one slot every registry read consults and every completed refresh
/// installs into.
///
/// Before XAI_3 the overlay had two independent homes — a `static` in the binary
/// (`cyrup/src/provider.rs`) that the refresh wrote, and a by-value field on `AgentSessionServices`
/// captured once at build time that `/model` read. A completed refresh updated the first and was
/// invisible to the second, so a newly released model could never reach the picker without a
/// restart. This is the single slot both now share.
///
/// Poison-safe (`PoisonError::into_inner`) because a panic in an unrelated task must never wedge
/// model selection (R-00-009), and degrading to "no overlay" rather than erroring because the
/// embedded catalogs are the FLOOR: a refresh failure must never be observable as fewer models.
#[derive(Debug, Default)]
pub struct CatalogOverlaySlot {
    current: RwLock<Option<Arc<CatalogOverlay>>>,
}

impl CatalogOverlaySlot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed a slot with an already-loaded overlay (the disk-only startup restore).
    #[must_use]
    pub fn with_overlay(overlay: Option<Arc<CatalogOverlay>>) -> Self {
        Self {
            current: RwLock::new(overlay),
        }
    }

    /// The active overlay, or `None` for "embedded catalogs only". Cheap: one `Arc` clone.
    pub fn load(&self) -> Option<Arc<CatalogOverlay>> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Install a freshly loaded overlay. An EMPTY overlay installs as `None` so "empty" and
    /// "absent" stay indistinguishable (`CatalogOverlay::from_entries`'s own rule).
    pub fn install(&self, overlay: CatalogOverlay) {
        let next = (!overlay.is_empty()).then(|| Arc::new(overlay));
        *self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = next;
    }
}

/// One coordinated refresh's outcome — pi `ModelsRefreshResult` (`packages/ai/src/models.ts:73-76`)
/// plus the `timedOut` flag pi's call sites keep beside it (`interactive-mode.ts:4871-4886`).
///
/// Three fields, not two, because the picker distinguishes three cases: MY deadline fired
/// ([`Self::timed_out`]), the SHARED operation aborted ([`Self::aborted`]), and some providers
/// failed ([`Self::errors`]). pi's fourth case — the promise REJECTING — has no counterpart:
/// `RemoteCatalog::refresh_providers` collects per-provider failures and never returns `Err`
/// (`remote_catalog.rs:562-583`), and `load_overlay` is infallible (`:518-541`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogRefreshResult {
    /// THIS caller's `CancelToken` fired before the shared operation settled. The operation keeps
    /// running for anyone else still waiting — pi's per-caller
    /// `raceWithAbortSignal(active.promise, signal)`.
    pub timed_out: bool,
    /// The SHARED operation reported an abort (its own token fired — the last waiter left).
    pub aborted: bool,
    /// provider id → rendered error, in id order so a caller's `join(", ")` is deterministic.
    pub errors: BTreeMap<String, String>,
}

impl CatalogRefreshResult {
    /// Nothing timed out, nothing aborted, every provider refreshed.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        !self.timed_out && !self.aborted && self.errors.is_empty()
    }

    fn aborted() -> Self {
        Self {
            aborted: true,
            ..Self::default()
        }
    }

    fn timed_out() -> Self {
        Self {
            timed_out: true,
            ..Self::default()
        }
    }
}

/// Refresh the given providers' catalogs, then reload and install the overlay.
///
/// This is `spawn_model_catalog_refresh_with`'s body, lifted so both the fire-and-forget trigger and
/// the coordinated one run the SAME sequence (SUBTASK 3 rewires the binary onto it).
///
/// The two provider lists are deliberately different and must not be collapsed. `fetch` is the
/// credential-gated set MINUS `radius` (pi wraps every built-in except radius in
/// `withRemoteCatalog`, `model-runtime.ts:183-189` — the pi.dev route 404s for radius and the
/// staleness guard would then discard the gateway's own catalog), while `overlay` is EVERY provider
/// id, because reading the radius entry back is the restore half of `radius.ts:36-48`.
pub async fn refresh_and_install(
    catalog: &Arc<RemoteCatalog>,
    fetch: &[String],
    overlay: &[String],
    options: RefreshOptions,
    slot: &CatalogOverlaySlot,
) -> BTreeMap<String, String> {
    let fetch_refs: Vec<&str> = fetch.iter().map(String::as_str).collect();
    let errors: BTreeMap<String, String> = catalog
        .refresh_providers(&fetch_refs, options)
        .await
        .into_iter()
        .map(|(id, err)| (id, err.to_string()))
        .collect();
    let overlay_refs: Vec<&str> = overlay.iter().map(String::as_str).collect();
    slot.install(catalog.load_overlay(&overlay_refs).await);
    errors
}

/// Monotonic publication counter — the generation half of pi's `active === created` identity check,
/// which a Rust `Shared` cannot express by pointer equality.
static NEXT_GENERATION: AtomicU64 = AtomicU64::new(0);

type SharedFut = Shared<BoxFuture<'static, Arc<CatalogRefreshResult>>>;

struct Inflight {
    shared: SharedFut,
    /// The operation's OWN token — pi's internal `AbortController`. Fired only at zero waiters.
    cancel: CancelToken,
    /// pi's `active.waiters`.
    waiters: Arc<AtomicUsize>,
    generation: u64,
}

type Slot = Arc<Mutex<Option<Inflight>>>;

/// Decrements the waiter count on leave and fires the operation's token at zero — pi's
/// `finally { active.waiters--; if (waiters === 0 && still current) controller.abort(); }`.
///
/// A `Drop` guard rather than straight-line code after the await, because a Rust future can be
/// dropped at ANY await point: a caller whose whole task is cancelled must still be counted out, or
/// the operation is pinned alive by a waiter that no longer exists. The `Weak` avoids the `Arc`
/// cycle (slot → Inflight → … → slot).
struct WaiterGuard {
    slot: Weak<Mutex<Option<Inflight>>>,
    waiters: Arc<AtomicUsize>,
    cancel: CancelToken,
    generation: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        if self.waiters.fetch_sub(1, Ordering::AcqRel) != 1 {
            return; // somebody else is still waiting — pi aborts only at ZERO
        }
        let Some(slot) = self.slot.upgrade() else {
            return;
        };
        let still_current = match slot.lock() {
            Ok(guard) => guard.as_ref().is_some_and(|i| i.generation == self.generation),
            Err(_) => false,
        };
        if still_current {
            self.cancel.cancel();
        }
    }
}

/// Clears the memo when the operation settles OR is dropped un-settled — pi's
/// `promise.finally(() => { if (current === created) delete })`, generation-stamped for the same
/// reason `utils::refresh::MemoClear` is. Read that type's doc: its re-entrancy hazard (take the
/// value out UNDER the lock, drop it OUTSIDE) applies identically here.
struct MemoClear {
    slot: Weak<Mutex<Option<Inflight>>>,
    generation: u64,
}

impl Drop for MemoClear {
    fn drop(&mut self) {
        let Some(slot) = self.slot.upgrade() else {
            return;
        };
        let taken = match slot.lock() {
            Ok(mut current) => match current.as_ref() {
                Some(i) if i.generation == self.generation => current.take(),
                _ => None,
            },
            Err(_) => None,
        };
        drop(taken);
    }
}

/// The publish-race decision, computed under ONE lock acquisition and acted on after the guard
/// drops (a `Mutex` is not reentrant, and the "adopt" arm below needs to run more code than the
/// scrutinee's borrow would allow to survive).
enum Publish {
    /// Someone else already published between our fast-path check and now; adopt their in-flight
    /// operation instead of racing it.
    Adopt {
        shared: SharedFut,
        cancel: CancelToken,
        waiters: Arc<AtomicUsize>,
        generation: u64,
    },
    /// The slot was empty (or poisoned): our candidate is now the published operation.
    Mine,
}

/// At most one in-flight whole-catalog refresh, joinable, with independent per-caller deadlines.
#[derive(Default)]
pub struct CatalogRefreshCoordinator {
    inflight: Slot,
}

impl CatalogRefreshCoordinator {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Start, or join, the single in-flight refresh; return when it settles or when `caller` fires.
    ///
    /// `start` receives the OPERATION's token, not the caller's — pi passes its internal
    /// `controller.signal` into `modelRuntime.refresh`, which is precisely what keeps one caller's
    /// deadline from cancelling another caller's wait.
    pub async fn refresh<F, Fut>(&self, caller: CancelToken, start: F) -> CatalogRefreshResult
    where
        F: FnOnce(CancelToken) -> Fut,
        Fut: std::future::Future<Output = BTreeMap<String, String>> + Send + 'static,
    {
        // pi `signal.throwIfAborted()` — an already-cancelled caller starts nothing and joins nothing.
        if caller.is_cancelled() {
            return CatalogRefreshResult::timed_out();
        }

        // Fast path: adopt an in-flight operation. The guard drops inside this expression, so no
        // await is ever held under the lock.
        let joined = match self.inflight.lock() {
            Ok(slot) => slot.as_ref().map(|i| {
                i.waiters.fetch_add(1, Ordering::AcqRel);
                (
                    i.shared.clone(),
                    WaiterGuard {
                        slot: Arc::downgrade(&self.inflight),
                        waiters: Arc::clone(&i.waiters),
                        cancel: i.cancel.clone(),
                        generation: i.generation,
                    },
                )
            }),
            Err(_) => None,
        };

        let (shared, guard) = match joined {
            Some(pair) => pair,
            None => self.start_and_publish(start),
        };

        // pi `raceWithAbortSignal(active.promise, signal)` — race the SHARED promise against MY
        // token. `guard` drops on BOTH arms, which is pi's `finally`.
        let out = tokio::select! {
            biased;
            settled = shared => Some(settled),
            () = caller.cancelled() => None,
        };
        drop(guard);
        match out {
            Some(result) => (*result).clone(),
            None => CatalogRefreshResult::timed_out(),
        }
    }

    fn start_and_publish<F, Fut>(&self, start: F) -> (SharedFut, WaiterGuard)
    where
        F: FnOnce(CancelToken) -> Fut,
        Fut: std::future::Future<Output = BTreeMap<String, String>> + Send + 'static,
    {
        let generation = NEXT_GENERATION.fetch_add(1, Ordering::Relaxed);
        let cancel = CancelToken::new();
        let waiters = Arc::new(AtomicUsize::new(0));
        let memo = MemoClear {
            slot: Arc::downgrade(&self.inflight),
            generation,
        };

        // SPAWNED (D3): the operation must progress without a poller and must be genuinely
        // cancellable. `run_until_cancelled` is this workspace's `raceWithAbortSignal`.
        let operation = start(cancel.clone());
        let op_cancel = cancel.clone();
        let handle = tokio::spawn(async move {
            let _clear_on_settle = memo; // pi's `finally`, plus the dropped-un-settled case.
            match op_cancel.run_until_cancelled(operation).await {
                Some(errors) => CatalogRefreshResult {
                    errors,
                    ..CatalogRefreshResult::default()
                },
                None => CatalogRefreshResult::aborted(),
            }
        });
        // A panicked task degrades to `aborted`; NO-PANIC policy.
        let candidate: SharedFut = async move {
            handle.await.unwrap_or_else(|_| CatalogRefreshResult::aborted())
        }
        .map(Arc::new)
        .boxed()
        .shared();

        // Publish, re-checking under the SAME lock acquisition — `RefreshDedup`'s publish-race
        // lesson, load-bearing here too: the loser must ADOPT the winner. Unlike `RefreshDedup` the
        // work is already spawned, so a loser must ALSO cancel its now-orphaned operation rather
        // than leave it running.
        let decision = match self.inflight.lock() {
            Ok(mut slot) => match slot.as_ref() {
                Some(winner) => Publish::Adopt {
                    shared: winner.shared.clone(),
                    cancel: winner.cancel.clone(),
                    waiters: Arc::clone(&winner.waiters),
                    generation: winner.generation,
                },
                None => {
                    *slot = Some(Inflight {
                        shared: candidate.clone(),
                        cancel: cancel.clone(),
                        waiters: Arc::clone(&waiters),
                        generation,
                    });
                    Publish::Mine
                }
            },
            // A poisoned mutex runs the operation un-deduplicated rather than panicking — the same
            // "never wedge model selection" posture as `RefreshDedup` and `GuestProviderRegistry`.
            Err(_) => Publish::Mine,
        };

        let (shared, waiters, cancel, generation) = match decision {
            Publish::Adopt {
                shared,
                cancel: winner_cancel,
                waiters: winner_waiters,
                generation: winner_generation,
            } => {
                // MY operation is already spawned and running (D3) — losing the race orphans it,
                // so it must be cancelled here rather than merely dropped, or it keeps running
                // (e.g. keeps hitting the network) for a result nobody will ever read.
                cancel.cancel();
                (shared, winner_waiters, winner_cancel, winner_generation)
            }
            Publish::Mine => (candidate, waiters, cancel, generation),
        };

        waiters.fetch_add(1, Ordering::AcqRel);
        let guard = WaiterGuard {
            slot: Arc::downgrade(&self.inflight),
            waiters,
            cancel,
            generation,
        };
        (shared, guard)
    }
}

/// The one object the startup trigger and the picker trigger share (pi keys its coordinator by
/// runtime instance; this is cyrup's counterpart).
pub struct ModelCatalogService {
    catalog: Arc<RemoteCatalog>,
    overlay: Arc<CatalogOverlaySlot>,
    coordinator: CatalogRefreshCoordinator,
    /// Every registered provider id — the OVERLAY list, which is the static registration roster and
    /// so is safe to hold.
    overlay_providers: Vec<String>,
    /// The network posture, decided by the BIN (D5). `CACHE_ONLY` makes every refresh a disk reload
    /// with no request, which is what an `--offline` run must do.
    options: RefreshOptions,
}

impl ModelCatalogService {
    /// Start a service over `catalog` and the `overlay` slot it installs into. Defaults to
    /// `CACHE_ONLY` and an empty overlay-provider list; see [`Self::with_options`] and
    /// [`Self::with_overlay_providers`].
    #[must_use]
    pub fn new(catalog: Arc<RemoteCatalog>, overlay: Arc<CatalogOverlaySlot>) -> Self {
        Self {
            catalog,
            overlay,
            coordinator: CatalogRefreshCoordinator::new(),
            overlay_providers: Vec::new(),
            options: RefreshOptions::CACHE_ONLY,
        }
    }

    /// Every registered provider id — the list [`refresh_and_install`]'s `overlay` argument reads
    /// back after a fetch (FINDING 4's fetch/overlay split; see [`refresh_and_install`]'s doc).
    #[must_use]
    pub fn with_overlay_providers(mut self, ids: Vec<String>) -> Self {
        self.overlay_providers = ids;
        self
    }

    /// The network posture this service's refreshes run with (D5) — decided once by the bin, never
    /// per call.
    #[must_use]
    pub fn with_options(mut self, options: RefreshOptions) -> Self {
        self.options = options;
        self
    }

    /// The live overlay slot — handed to every registry builder.
    #[must_use]
    pub fn overlay(&self) -> Arc<CatalogOverlaySlot> {
        Arc::clone(&self.overlay)
    }

    /// Trigger-or-join a bounded, coalesced whole-catalog refresh (pi `refreshModelCatalogs`).
    ///
    /// `fetch_providers` is a PER-CALL argument, not service state — FINDING 4. pi resolves each
    /// provider's credential inside the refresh (`models.ts:420-421`), so a `/login` that lands
    /// mid-session is picked up by the very next call. Freezing the list at construction would mean
    /// a user who logs in and then opens `/model` gets no refresh for the provider they just
    /// configured, and a fresh install would never fetch anything for the whole session.
    ///
    /// An EMPTY list is not an error: nothing is fetched, the overlay is still reloaded from disk,
    /// and the caller gets a clean result. That is pi's `if (!credential) return;` per provider,
    /// hoisted.
    pub async fn refresh(
        &self,
        caller: CancelToken,
        fetch_providers: Vec<String>,
    ) -> CatalogRefreshResult {
        let catalog = Arc::clone(&self.catalog);
        let overlay = Arc::clone(&self.overlay);
        let all = self.overlay_providers.clone();
        let options = self.options;
        self.coordinator
            .refresh(caller, move |_op_cancel| async move {
                refresh_and_install(&catalog, &fetch_providers, &all, options, &overlay).await
            })
            .await
    }
}

// `_op_cancel` is intentionally unused by the production closure above: `refresh_providers` has no
// signal argument, so cancellation is delivered structurally — `run_until_cancelled` drops the
// operation at its next await point, exactly as `tokio::time::timeout` already cancels
// `refresh_model_catalogs_with`. The parameter stays: it is the seam a future signal-aware
// `refresh_providers` plugs into, and what a substituted closure (as `/flux/tests` will write) uses
// to observe the abort.
