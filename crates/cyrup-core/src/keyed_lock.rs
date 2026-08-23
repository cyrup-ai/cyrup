//! A registry of per-key async mutexes, with RAII guards and self-evicting entries.
//!
//! Callers own the map — declare a `static LazyLock<KeyedLockMap<K>>` per lock domain and hand a
//! clone to [`KeyedLocks::new`]. There is deliberately no shared default: two domains that key on
//! unrelated things must not contend, and a domain that needs process-wide exclusion must not be
//! able to accidentally obtain an isolated map.
//!
//! Keying is the caller's job too. This module never touches the filesystem (see the crate docs:
//! no I/O), so a domain that keys on resolved paths resolves them itself before calling
//! [`KeyedLocks::guard`].

use crate::CancelToken;
use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// The map one lock domain is built over.
pub type KeyedLockMap<K> = Arc<DashMap<K, Arc<Mutex<()>>>>;

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
/// domain — every handle over one map contends on that map.
pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
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
        let _pending = PendingEntry { map: Arc::clone(&self.map), key: key.clone() };
        let lock = self
            .map
            .entry(key.clone())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
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
                map: Arc::clone(&self.map),
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
        // `remove_if` runs the predicate while holding the shard lock, so a concurrent `guard()`
        // that has just cloned the Arc is observed (strong_count > 1) and the entry is kept.
        self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
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
        // Identical predicate to `KeyedGuard::drop`: evict only when the map holds the last
        // reference, so a concurrent waiter that has already cloned the `Arc` keeps its entry. On
        // the SUCCESS path the returned `KeyedGuard` holds a clone, so this is a no-op and the
        // guard's own drop does the eviction.
        self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
    }
}
