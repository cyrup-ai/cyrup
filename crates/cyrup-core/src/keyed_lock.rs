//! A registry of per-key async mutexes, with RAII guards and self-evicting entries.
//!
//! Callers own the map — declare a `static LazyLock<KeyedLockMap<K>>` per lock domain and hand a
//! clone to [`KeyedLocks::new`]. There is deliberately no shared default: two domains that key on
//! unrelated things must not contend, and a domain that needs process-wide exclusion must not be
//! able to accidentally obtain an isolated map.
//!
//! Keying is the caller's job too, and it is the caller's whole job: this module hashes the key it
//! is handed and nothing else, so two keys naming one entity are two locks. A path-keyed domain
//! must therefore reduce every spelling it can be handed to a single form BEFORE calling
//! [`KeyedLocks::guard`] — and it must choose WHICH form, because this module never touches the
//! filesystem (see the crate docs: no I/O) and so cannot choose for it. The two in-tree domains
//! choose differently, on purpose: `cyrup-tools`' `FileMutationLocks` realpaths, because its
//! upstream `getMutationQueueKey` does; `cyrup-config`'s `FileLock` resolves lexically, because its
//! upstream calls `proper-lockfile` with `realpath: false` and makes the path absolute at
//! construction instead. "Resolved" is the domain's definition, not this module's.

use crate::CancelToken;
use dashmap::DashMap;
use std::future::{poll_fn, Future};
use std::hash::Hash;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// The map one lock domain is built over.
///
/// Opaque on purpose. This module's whole safety argument is "for any live key there is exactly
/// one `Arc<Mutex<()>>`, minted only by [`KeyedLocks::guard`] and evicted only while the map holds
/// the last reference to it". A bare `Arc<DashMap<..>>` alias hands every owner `insert`,
/// `remove`, `clear`, `alter` and `entry`, each of which can put a second live mutex behind one
/// key and void mutual exclusion with no error to either party. The wrapper keeps every mutating
/// operation inside this module — `get_or_insert` and `evict_if_unreferenced` below are the only
/// two, and both are private — and leaves callers three read-only observers. It also keeps
/// `dashmap` out of consumers' manifests: nothing about a lock domain names a `dashmap` type.
pub struct KeyedLockMap<K: Eq + Hash + Clone>(Arc<DashMap<K, Arc<Mutex<()>>>>);

impl<K: Eq + Hash + Clone> KeyedLockMap<K> {
    /// A fresh, empty lock domain.
    ///
    /// Not `const fn`, and it does not need to be: `LazyLock::new` stores the initializer and
    /// runs it on first deref, so a `static M: LazyLock<KeyedLockMap<K>>` built with
    /// `LazyLock::new(KeyedLockMap::new)` is exactly as valid as the closure it replaces —
    /// `DashMap::new` is not `const fn` either.
    //
    // Deliberately NOT `Default`. `Default` on a lock domain means "a fresh, isolated one", which
    // is the bug `FileMutationLocks::default` is hand-written to avoid; with the alias, a
    // `#[derive(Default)]` on any struct holding this field compiled and silently produced that
    // second domain. Without the impl the derive is a compile error instead.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(Arc::new(DashMap::new()))
    }

    /// Whether `key` currently has an entry. Observing the map cannot break the
    /// one-live-mutex-per-key invariant; only mutating it can, which is why the observers are
    /// public and the mutators are not.
    pub fn contains_key(&self, key: &K) -> bool {
        self.0.contains_key(key)
    }

    /// The mutex currently registered for `key`, if any — an `Arc` clone, never a map reference,
    /// so no `dashmap` type crosses the boundary. Holding the returned `Arc` defers eviction of
    /// the entry until it drops (the `strong_count == 1` predicate below sees it); it cannot mint
    /// a second mutex for the key.
    pub fn mutex_for(&self, key: &K) -> Option<Arc<Mutex<()>>> {
        self.0.get(key).map(|e| Arc::clone(e.value()))
    }

    /// Whether two handles name the SAME map, i.e. the same lock domain. This is the property
    /// consumers assert on: two independently constructed owners must contend.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// The mutex for `key`, minting one if the key is unregistered. The ONLY insertion path.
    fn get_or_insert(&self, key: K) -> Arc<Mutex<()>> {
        self.0
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Evict `key`'s entry iff the map holds the last reference to its mutex. The ONLY removal
    /// path, and the one predicate both drop paths share.
    ///
    /// `remove_if` runs the predicate while holding the shard lock, so a concurrent
    /// [`KeyedLocks::guard`] that has just cloned the `Arc` is observed (`strong_count > 1`) and
    /// the entry is kept.
    fn evict_if_unreferenced(&self, key: &K) {
        self.0.remove_if(key, |_, v| Arc::strong_count(v) == 1);
    }
}

impl<K: Eq + Hash + Clone> Clone for KeyedLockMap<K> {
    /// An `Arc` clone of the same map — never a second lock domain. Hand-written rather than
    /// derived so that guarantee is stated where a reader looks for it.
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// Acquisition was cancelled before the lock was taken.
///
/// Deliberately not a variant of any crate's error enum: callers map it into their own vocabulary
/// at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("lock acquisition cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// A handle onto a caller-owned map of per-key async mutexes.
///
/// Cloning a handle, or constructing a second one over the same map, does NOT create a second lock
/// domain — every handle over one map contends on that map. Prefer the clone when a second owner
/// or a spawned task needs a handle: it is one `Arc` bump and, unlike [`KeyedLocks::new`], it
/// cannot be pointed at a different map.
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
}

impl<K: Eq + Hash + Clone> Clone for KeyedLocks<K> {
    /// Hand-written rather than derived so the reason cloning is safe lives on the impl: a clone
    /// is one `Arc` bump on the caller's map (it delegates to [`KeyedLockMap`]'s own hand-written
    /// `Clone`), so it is by construction the SAME lock domain — the only handle-producing
    /// operation that cannot name a different map. It is also invisible to eviction, which gates
    /// `remove_if` on the strong count of the per-key `Arc<Mutex<()>>` *value* (see
    /// `KeyedLockMap::evict_if_unreferenced`), never on references to the map itself.
    ///
    /// There is deliberately no `Default` beside it, and none is derivable: [`KeyedLockMap`] has
    /// no `Default` on purpose, so the only `Default` that could exist here would be a
    /// hand-written one minting a fresh map — the isolated domain this module exists to make
    /// unobtainable.
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
        }
    }
}

impl<K: Eq + Hash + Clone> KeyedLocks<K> {
    /// Attach to a caller-owned map. This is a cheap `Arc` clone, not a fresh map.
    pub fn new(map: KeyedLockMap<K>) -> Self {
        Self { map }
    }

    /// Take this task's place in `key`'s FIFO queue WITHOUT waiting for it.
    ///
    /// The returned future resolves on its FIRST poll and can therefore never suspend, so a caller
    /// may hold a registration lock across `enqueue(..).await` and release it on the very next
    /// line with its queue position already claimed — see the EXACTLY ONE POLL block in the body
    /// for the tokio property that rests on, and the re-verify step a tokio bump requires. This is
    /// the Rust shape of pi's
    /// `fileMutationQueues.set(key, chainedQueue)` (file-mutation-queue.ts:42): the LINK is made
    /// inside the serialized registration, only the WAIT happens outside it.
    ///
    /// Pair it with [`KeyedAcquire::wait`]; [`KeyedLocks::guard`] below is the two halves back to
    /// back, for callers with no registration order to preserve.
    pub async fn enqueue(&self, key: K) -> KeyedAcquire<K> {
        // Declared before `lock` so it drops LAST — see `PendingEntry`.
        let pending = PendingEntry {
            map: self.map.clone(),
            key: key.clone(),
        };
        let lock = self.map.get_or_insert(key.clone());
        let mut acquire: Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send>> =
            Box::pin(Arc::clone(&lock).lock_owned());
        // EXACTLY ONE POLL — and this is the ONE property in this module resting on tokio's
        // IMPLEMENTATION rather than on its contract. Re-verify it on every tokio bump; the two
        // `cfg(test)` tests named at the bottom of this block fail rather than degrade.
        //
        // What tokio DOCUMENTS is weaker than what this line needs. The `Mutex` type doc says
        // "the order in which tasks call the `lock` method is the exact order in which they will
        // acquire the lock" (tokio 1.52.3 src/sync/mutex.rs:20-22) and `lock_owned`'s cancel-safety
        // note says the queue distributes locks "in the order they were requested" (:598-600).
        // Neither sentence is about POLLS. `enqueue` does not call `lock_owned` in dispatch order —
        // its CALLER does that — it claims the place by polling an already-constructed acquire
        // future exactly once, and an unpolled future has done nothing at all.
        //
        // The equivalence holds in 1.52.3 because the waiter node is pushed on the FIRST poll that
        // finds no free permit: `Semaphore::poll_acquire` registers the waker and runs
        // `waiters.queue.push_front(node)` under its `!queued` branch before returning `Pending`
        // (src/sync/batch_semaphore.rs:496-517); `Acquire::poll` latches `queued = true` (:600-604)
        // so later polls reuse that place; and a release ASSIGNS the permit into the tail waiter's
        // own node rather than returning it to the counter (`add_permits_locked`, :306-330, via
        // `assign_permits` + `queue.pop_back()`). push_front + pop_back = FIFO by first-poll order.
        //
        // TWO caveats, both of which a bump can change with no compile error:
        //
        // 1. `Acquire::poll` runs `ready!(coop::poll_proceed(cx))` BEFORE `poll_acquire`
        //    (batch_semaphore.rs:598). A task whose cooperative budget is spent gets `Pending`
        //    there with NO place claimed, and the place is taken later — at the first poll inside
        //    `KeyedAcquire::wait`, i.e. OUTSIDE the caller's registration lock. The budget is 128
        //    units per task poll (src/task/coop/mod.rs:115-116) and LIFO-slot tasks inherit the
        //    parent's remainder (src/runtime/scheduler/multi_thread/worker.rs:675), so it is not a
        //    per-`.await` allowance. This is unreachable on the `write`/`edit` path —
        //    `FileMutationLocks::guard` is at most three budget units into a fresh task poll — and
        //    it degrades ordering only, never correctness: exclusion, eviction and cancellation are
        //    unaffected. A future caller that reaches `enqueue` deep inside one poll would lose the
        //    guarantee with no diagnostic.
        // 2. If tokio ever moves queue placement off the first poll, this `poll_fn` becomes a no-op
        //    and ordering silently reverts to "whoever polls `wait` first".
        //
        // Pinned by `enqueue_resolves_on_its_first_poll` and
        // `places_are_granted_in_enqueue_order_not_in_wait_order` in this file's `mod tests`.
        //
        // `poll_fn` returns `Ready` on its own first poll, so this `.await` cannot yield and the
        // caller's registration lock is still held when `enqueue` returns.
        let early = match poll_fn(|cx| Poll::Ready(acquire.as_mut().poll(cx))).await {
            Poll::Ready(g) => Some(g),
            Poll::Pending => None,
        };
        KeyedAcquire {
            acquire: Some(acquire),
            early,
            lock: Some(lock),
            map: self.map.clone(),
            key,
            _pending: pending,
        }
    }

    /// Acquire the lock for `key`, holding it until the returned guard drops.
    ///
    /// Cancel-aware: returns `Err(Cancelled)` if the token is cancelled before acquisition —
    /// *always*, not just when the mutex happens to be contended (see [`KeyedAcquire::wait`]).
    ///
    /// Unchanged behaviour, now expressed over the two halves: the claim is taken on this future's
    /// first poll (it cannot suspend before that, since [`KeyedLocks::enqueue`] never yields) and
    /// the wait happens after it.
    pub async fn guard(&self, key: K, cancel: &CancelToken) -> Result<KeyedGuard<K>, Cancelled> {
        self.enqueue(key).await.wait(cancel).await
    }
}

/// A claimed-but-not-yet-granted place in one key's FIFO queue.
///
/// Minted by [`KeyedLocks::enqueue`] and consumed by [`KeyedAcquire::wait`]. Dropping it instead
/// forfeits the place and runs the same eviction check every other exit path runs.
pub struct KeyedAcquire<K: Eq + Hash + Clone> {
    acquire: Option<Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send>>>,
    early: Option<OwnedMutexGuard<()>>,
    lock: Option<Arc<Mutex<()>>>,
    map: KeyedLockMap<K>,
    key: K,
    /// Declared LAST so it drops AFTER `Drop::drop` below has released the guard, the acquire
    /// future and this handle's `Arc` clone — otherwise the `strong_count == 1` predicate would
    /// always see them and the entry would never be evicted.
    _pending: PendingEntry<K>,
}

impl<K: Eq + Hash + Clone> KeyedAcquire<K> {
    /// Wait for the place claimed by [`KeyedLocks::enqueue`] to come up.
    pub async fn wait(mut self, cancel: &CancelToken) -> Result<KeyedGuard<K>, Cancelled> {
        // Carries over what `biased` bought the old `select!`: an already-cancelled token loses
        // even when the lock is free. An unbiased `select!` polls its ready arms in a RANDOM
        // order, so when the token is already cancelled AND the mutex is uncontended both arms
        // were ready on the first poll and the outcome was a coin flip. Now deterministic by
        // construction rather than by poll order.
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        let granted = match self.early.take() {
            Some(g) => g,
            None => {
                let acquire = match self.acquire.as_mut() {
                    Some(a) => a,
                    None => return Err(Cancelled),
                };
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(Cancelled),
                    g = acquire.as_mut() => g,
                }
            }
        };
        Ok(KeyedGuard {
            inner: Some(granted),
            lock: self.lock.take(),
            map: self.map.clone(),
            key: self.key.clone(),
        })
    }
}

impl<K: Eq + Hash + Clone> Drop for KeyedAcquire<K> {
    /// Release in the same order `KeyedGuard::drop` does, so `_pending` — which drops after this
    /// body returns — sees only the map's own reference. Covers both non-guard exits: a cancelled
    /// wait and an outright drop of the `wait()` future.
    fn drop(&mut self) {
        self.early.take();
        self.acquire.take();
        self.lock.take();
    }
}

/// RAII guard for a per-key lock. On drop it releases the mutex and evicts the map entry once no
/// other holder/waiter references it, so the map cannot grow without bound.
pub struct KeyedGuard<K: Eq + Hash + Clone> {
    inner: Option<OwnedMutexGuard<()>>,
    lock: Option<Arc<Mutex<()>>>,
    map: KeyedLockMap<K>,
    key: K,
}

impl<K: Eq + Hash + Clone> Drop for KeyedGuard<K> {
    fn drop(&mut self) {
        // Release the mutex and drop our clone of the Arc *before* the eviction check, so the only
        // remaining strong refs are the map's plus any genuinely active holders/waiters.
        self.inner.take();
        self.lock.take();
        self.map.evict_if_unreferenced(&self.key);
    }
}

/// Runs [`KeyedGuard`]'s eviction check for an acquisition that never became a guard.
///
/// **This closes a mechanism gap, not a tidiness one.** A Rust future can be dropped at ANY
/// `.await`, and [`KeyedLocks::enqueue`] inserts the map entry BEFORE the mutex is awaited. Both
/// non-guard exits therefore leak the entry: the `cancel.cancelled()` arm of
/// [`KeyedAcquire::wait`], and an outright drop of the `wait()` future — or of the [`KeyedAcquire`]
/// itself, which is the same thing. The map is typically a process-global `static`, so nothing ever
/// collects those entries; only a LATER successful lock on the identical key could, via
/// [`KeyedGuard::drop`].
///
/// Declared BEFORE the `lock` local in [`KeyedLocks::enqueue`], and held as [`KeyedAcquire`]'s LAST
/// field, so that drop order (reverse declaration for locals, declaration order for fields)
/// releases every local `Arc` clone FIRST — otherwise the `strong_count == 1` predicate would
/// always see those clones and never evict.
struct PendingEntry<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
    key: K,
}

impl<K: Eq + Hash + Clone> Drop for PendingEntry<K> {
    fn drop(&mut self) {
        // Same single eviction path as `KeyedGuard::drop`. On the SUCCESS path the returned
        // `KeyedGuard` holds a clone of the mutex, so this is a no-op and the guard's own drop
        // does the eviction.
        self.map.evict_if_unreferenced(&self.key);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Poll `f` exactly once with a no-op waker. No wall clock, no scheduler involvement.
    fn poll_once<F: Future>(f: Pin<&mut F>) -> Poll<F::Output> {
        f.poll(&mut Context::from_waker(Waker::noop()))
    }

    /// The never-yield property, asserted directly rather than inferred from the source.
    ///
    /// `FileMutationLocks::guard` (`cyrup-tools/src/lock.rs:215`) holds `MUTATION_REGISTRATION`
    /// across `enqueue(..).await`. A suspension there releases the registration chain BEFORE the
    /// queue place is claimed, which is the whole defect this task closed — and it would be
    /// silent: nothing panics, ordering just becomes a blocking-pool coin flip again. The HELD
    /// case is the one that matters; the free case is here so the test cannot pass by never
    /// exercising a queue at all.
    #[tokio::test]
    async fn enqueue_resolves_on_its_first_poll() {
        let locks = KeyedLocks::new(KeyedLockMap::new());
        let cancel = CancelToken::new();

        let mut free = pin!(locks.enqueue("k"));
        let Poll::Ready(claimed) = poll_once(free.as_mut()) else {
            panic!("`enqueue` must resolve on its FIRST poll — it has an `.await` in it now");
        };
        drop(claimed);

        let held = locks.guard("k", &cancel).await.unwrap();
        let mut contended = pin!(locks.enqueue("k"));
        let Poll::Ready(queued) = poll_once(contended.as_mut()) else {
            panic!(
                "`enqueue` suspended on a HELD lock. It must claim the place and return: the \
                 caller's registration lock is held across this call (cyrup-tools/src/lock.rs:215)"
            );
        };
        drop(queued);
        drop(held);
    }

    /// THE property this task exists for, reduced to one crate and zero clocks: the queue place is
    /// taken inside `enqueue`, so grant order follows ENQUEUE order — not the order in which the
    /// halves are later `wait`ed on, and not poll order.
    ///
    /// Deterministic in both directions. Releasing a `tokio::sync::Mutex` does not put the permit
    /// back on the counter for whoever polls first: `add_permits_locked` walks the wait queue from
    /// the tail and ASSIGNS the permit into the waiter's own node
    /// (tokio 1.52.3 src/sync/batch_semaphore.rs:306-330, `assign_permits` + `queue.pop_back()`),
    /// so `second`'s later poll finds nothing and `first`'s finds a permit already banked.
    /// RED the moment `enqueue`'s `poll_fn` stops taking the place: the waiter node is pushed on
    /// the first poll that finds no permit (batch_semaphore.rs:496-517), so a first poll deferred
    /// into `wait` would enqueue `second` ahead of `first` and invert both assertions.
    #[tokio::test]
    async fn places_are_granted_in_enqueue_order_not_in_wait_order() {
        let locks = KeyedLocks::new(KeyedLockMap::new());
        let cancel = CancelToken::new();
        let held = locks.guard("k", &cancel).await.unwrap();

        // Two places claimed in this order, with NOTHING awaited in between — the exact shape
        // `FileMutationLocks::guard` produces while it holds the registration chain.
        let first = locks.enqueue("k").await;
        let second = locks.enqueue("k").await;

        // …waited on in the OPPOSITE order.
        let mut wait_second = pin!(second.wait(&cancel));
        let mut wait_first = pin!(first.wait(&cancel));
        assert!(poll_once(wait_second.as_mut()).is_pending(), "the lock is held");
        assert!(poll_once(wait_first.as_mut()).is_pending(), "the lock is held");

        drop(held);
        assert!(
            poll_once(wait_second.as_mut()).is_pending(),
            "the SECOND place jumped the first — the queue place is no longer taken in `enqueue`"
        );
        assert!(
            poll_once(wait_first.as_mut()).is_ready(),
            "the FIRST place must be granted first, however late its `wait` is polled"
        );
    }

    /// DoD 6's dropped-acquisition half, non-vacuously. Note the DROP ORDER: `held` goes first, so
    /// the entry survives on `queued`'s reference alone and only `KeyedAcquire::drop` +
    /// `PendingEntry::drop` can evict it. Reverse the two drops and the test passes with
    /// `PendingEntry` deleted, which is exactly how the `cyrup-tools` version of this test came to
    /// pass for the wrong reason (Step 2c).
    #[tokio::test]
    async fn a_forfeited_place_is_released_and_its_entry_evicted() {
        let map = KeyedLockMap::new();
        let locks = KeyedLocks::new(map.clone());
        let cancel = CancelToken::new();

        let held = locks.guard("k", &cancel).await.unwrap();
        let queued = locks.enqueue("k").await;
        assert!(map.contains_key(&"k"));

        drop(held);
        assert!(map.contains_key(&"k"), "a live waiter must keep its entry alive");
        drop(queued);
        assert!(
            !map.contains_key(&"k"),
            "a forfeited place must evict its entry — the map is a process-global static, so \
             nothing else ever will (tokio's `lock_owned` cancel-safety note, src/sync/mutex.rs:598-600)"
        );
    }
}
