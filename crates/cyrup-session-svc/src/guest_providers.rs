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
//! each guest registration as a `ConfigProvider` (via [`cyrup_ext::ProviderRegistration::build_provider`])
//! and holds it behind an `Arc`. The [`AgentSession`](crate::AgentSession) then UNIONs these providers'
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
use cyrup_ext::provider::{ModelRegistrySink, ProviderRegistration};
use cyrup_provider::{
    AuthContext, Credential, CredentialStore, EnvAuthContext, InMemoryCredentialStore,
    InMemoryModelsStore, Model, ModelsPersist, ModelsPublication, ModelsPublisher,
    ModelsRefreshResult, ModelsStore, ModelsStoreOperationOptions, Provider, ProviderError,
    RefreshModelsContext,
};

use crate::provider_swap::ProviderSwap;

/// The bound sink + shared lookup for guest-registered providers. Cheaply shareable via `Arc`
/// (interior `Mutex`); the SAME `Arc` is handed to [`cyrup_ext::ExtensionRegistry::bind_model_registry`]
/// (as the sink) and to the session (as the read view).
#[derive(Default)]
pub struct GuestProviderRegistry {
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
}

impl GuestProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// The provider realized for `id`, if a guest registered one (cheap `Arc` clone). Poison-safe.
    pub fn provider(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.lock().get(id).cloned()
    }

    /// Whether a guest provider with this id is registered.
    pub fn has_provider(&self, id: &str) -> bool {
        self.lock().contains_key(id)
    }

    /// Every guest-registered provider's model catalog, unioned (Pi folds registered models into the
    /// shared `ModelRegistry.models`, model-registry.ts:917-940). Stable order by provider id.
    pub fn models(&self) -> Vec<Model> {
        self.lock()
            .values()
            .flat_map(|p| p.models().to_vec())
            .collect()
    }

    /// The mutation generation — bumped by every `upsert_provider` / `upsert_live_provider` /
    /// `remove_provider`.
    ///
    /// Read as a cache key, never as a count: a caller compares it with the value it last saw and
    /// recomposes when it differs. `Relaxed` is the right ordering for exactly that use — the
    /// recompose that follows a difference takes the registry's own `Mutex`, which is what
    /// publishes the new catalogs.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// The registered provider ids (diagnostics).
    pub fn ids(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    /// Bind the session's installed-provider slot: from now on a provider this registry replaces
    /// under the id the slot currently holds is stored into the slot as well, in the same
    /// operation. Without it the slot keeps the `Arc` it was installed as — a refresh's or
    /// `/llama`'s replacement reached the registry but never the provider the session streams
    /// through, which kept shadowing the replacement's catalog (the composed registry starts from
    /// the installed provider's models).
    pub fn follow_installed(&self, swap: &Arc<ProviderSwap>) {
        *poison_safe(&self.installed) = Some(Arc::downgrade(swap));
    }

    /// Store `provider` into the bound installed-provider slot when that holds the same id.
    fn replace_installed(&self, id: &str, provider: &Arc<dyn Provider>) {
        let swap = poison_safe(&self.installed)
            .as_ref()
            .and_then(Weak::upgrade);
        if let Some(swap) = swap
            && swap.current().id().as_str() == id
        {
            swap.store(Arc::clone(provider));
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
    static PUBLISHING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Restores [`PUBLISHING`] to what it was, even if the update unwinds.
struct PublishingGuard(Option<String>);

impl PublishingGuard {
    fn enter(id: &str) -> Self {
        Self(PUBLISHING.with(|slot| slot.borrow_mut().replace(id.to_string())))
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

        if signal.is_cancelled() || !self.is_current(id, generation) {
            return Ok(false);
        }
        if let Some(update) = publication.update {
            let _publishing = PublishingGuard::enter(id);
            update();
        }
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

/// A cache-only refresh of a provider registered after startup, begun but not yet running.
struct LateRestore {
    inner: Arc<RefreshInner>,
    id: String,
    provider: Arc<dyn Provider>,
    begun: (u64, CancelToken),
    ticket: TaskTicket,
    runtime: tokio::runtime::Handle,
}

impl LateRestore {
    /// Run it detached, as pi's `void this.refresh({ allowNetwork: false })` does
    /// (`core/model-runtime.ts:893`, `:939`): its outcome is nobody's to read, and a failing
    /// restore leaves the provider as it registered.
    fn spawn(self) {
        let Self {
            inner,
            id,
            provider,
            begun,
            ticket,
            runtime,
        } = self;
        runtime.spawn(async move {
            let _ticket = ticket;
            let _ = inner.drive(id, provider, begun, false, false).await;
        });
    }
}

impl GuestProviderRegistry {
    /// pi's registration-time restore (SEAM-141): `registerNativeProvider` / `registerProvider`
    /// end in `void this.refresh({ allowNetwork: false })` (`core/model-runtime.ts:893`, `:939`
    /// @v0.99.2-17, same lines at v1.0.0-25), so a provider that registers after startup lists its
    /// cached catalog at once. `None` when nothing is to be restored.
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
    /// then), for a static provider (pi's `refreshModels === undefined` filter), and when there is
    /// no tokio runtime to run it on (a registration made from outside one has nothing to spawn on;
    /// such a provider is covered by the startup restore if it registered before it).
    fn begin_late_restore(&self, id: &str, provider: &Arc<dyn Provider>) -> Option<LateRestore> {
        if !self.restore_started.load(Ordering::SeqCst)
            || !provider.has_refresh_models()
            || PUBLISHING.with(|slot| slot.borrow().as_deref() == Some(id))
        {
            return None;
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let inner = Arc::clone(&self.refresh);
        // The caller token is one nobody cancels: there is no caller to abort it.
        let begun = inner.begin(id, &CancelToken::new());
        let ticket = TaskTicket::new(&inner);
        Some(LateRestore {
            inner,
            id: id.to_string(),
            provider: Arc::clone(provider),
            begun,
            ticket,
            runtime,
        })
    }

    /// Invalidate the in-flight refresh of `id` (pi `setProvider` / `deleteProvider`, which both
    /// start with `supersedeProviderRefresh`, `models.ts:399-407`) — unless this IS the update a
    /// refresh is publishing for `id`, see [`PUBLISHING`].
    fn supersede_refresh(&self, id: &str) {
        if PUBLISHING.with(|slot| slot.borrow().as_deref() == Some(id)) {
            return;
        }
        self.refresh.supersede(id);
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

impl ModelRegistrySink for GuestProviderRegistry {
    fn upsert_provider(&self, reg: &ProviderRegistration) {
        // Full replacement for this provider id (Pi "replaces all models", model-registry.ts:919).
        let provider = reg.build_provider();
        self.supersede_refresh(&reg.id);
        self.lock().insert(reg.id.clone(), Arc::clone(&provider));
        self.replace_installed(&reg.id, &provider);
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn upsert_live_provider(&self, id: &str, provider: Arc<dyn Provider>) {
        // A native's own provider, stored as given — no `ConfigProvider` is rebuilt. Replacing the
        // id swaps the `Arc` (Pi "replaces all models", model-registry.ts:919), and the bump is what
        // makes `full_model_registry()` recompose so the replacement's catalog is what `/model` lists.
        let late_restore = {
            // Supersede, replace and (for a new id) begin the restore under ONE hold of the map, so
            // two concurrent registrations of an id cannot interleave the three.
            let mut providers = self.lock();
            self.supersede_refresh(id);
            let is_new = providers
                .insert(id.to_string(), Arc::clone(&provider))
                .is_none();
            if is_new {
                self.begin_late_restore(id, &provider)
            } else {
                None
            }
        };
        self.replace_installed(id, &provider);
        self.generation.fetch_add(1, Ordering::Relaxed);
        // Spawned LAST: the restore's own `update` re-registers the provider, and must land after
        // this registration has finished installing, not before it.
        if let Some(restore) = late_restore {
            restore.spawn();
        }
    }

    fn remove_provider(&self, id: &str) {
        self.supersede_refresh(id);
        self.lock().remove(id);
        self.generation.fetch_add(1, Ordering::Relaxed);
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
        let sink: Arc<GuestProviderRegistry> = Arc::new(GuestProviderRegistry::new());
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
        let sink: Arc<GuestProviderRegistry> = Arc::new(GuestProviderRegistry::new());
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
