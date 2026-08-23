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
use std::hash::Hash;
use std::sync::Arc;
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

    /// Acquire the lock for `key`, holding it until the returned guard drops.
    ///
    /// Cancel-aware: returns `Err(Cancelled)` if the token is cancelled before acquisition —
    /// *always*, not just when the mutex happens to be contended (see the `biased` note below).
    pub async fn guard(&self, key: K, cancel: &CancelToken) -> Result<KeyedGuard<K>, Cancelled> {
        // Declared before `lock` so it drops LAST — see `PendingEntry`.
        let _pending = PendingEntry {
            map: self.map.clone(),
            key: key.clone(),
        };
        let lock = self.map.get_or_insert(key.clone());
        tokio::select! {
            biased;
            // `biased` is REQUIRED, not a micro-optimisation. An unbiased `select!` polls its ready
            // arms in a RANDOM order, so when the token is already cancelled AND the mutex is
            // uncontended both arms are ready on the first poll and the outcome is a coin flip:
            // roughly half of all pre-cancelled acquisitions on an idle key would return a guard
            // and let the caller proceed. Polling the cancel arm first makes it deterministic.
            _ = cancel.cancelled() => Err(Cancelled),
            g = lock.clone().lock_owned() => Ok(KeyedGuard {
                inner: Some(g),
                lock: Some(lock),
                map: self.map.clone(),
                key,
            }),
        }
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
/// `.await`, and [`KeyedLocks::guard`] inserts the map entry BEFORE awaiting the mutex. Both
/// non-guard exits therefore leak the entry: the `cancel.cancelled()` arm of the `select!`, and an
/// outright drop of the `guard()` future itself. The map is typically a process-global `static`, so
/// nothing ever collects those entries; only a LATER successful lock on the identical key could,
/// via [`KeyedGuard::drop`].
///
/// Declared BEFORE the `lock` local in [`KeyedLocks::guard`] so that drop order (reverse
/// declaration) releases the local `Arc` clone FIRST — otherwise the `strong_count == 1` predicate
/// would always see that clone and never evict.
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
