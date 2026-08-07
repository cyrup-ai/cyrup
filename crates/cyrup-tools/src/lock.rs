//! Per-file mutation locking (R-03-007, arch-03 §5).
//!
//! `write`/`edit` serialize per canonical path so concurrent mutations (parallel tool calls,
//! R-02-016) of the same file cannot interleave/corrupt, while different files proceed
//! concurrently. Lock acquisition is itself cancel-aware.
//!
//! Ports `pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts`. The map is
//! **process-global**, matching that file's module-scope `fileMutationQueues` — see
//! `FILE_MUTATION_LOCKS` below.

use crate::error;
use cyrup_core::{CancelToken, ToolError};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use tokio::sync::{Mutex, OwnedMutexGuard};

/// The one lock map for the whole process.
///
/// Pi declares `const fileMutationQueues = new Map<string, Promise<void>>()` at **module scope**
/// (file-mutation-queue.ts:4) and exports a single free function `withFileMutationQueue` (:32), so
/// every `write`/`edit` in a Node process — however many sessions or tool sets exist — contends on
/// one map. A per-owner map would only serialize mutators that happen to share an owner: two
/// `ToolRegistry`s (`cyrup-session-svc`'s builder constructs one per `AgentSession`) would mutate
/// the same file with no exclusion at all. That is not a theoretical loss of atomicity —
/// [`crate::ops::FsOps::write_in_place`] truncates at `open` and then writes, so two unserialized
/// mutators interleave their chunks and leave a file matching NEITHER payload, with no error to
/// either caller. Hence: one map, process-wide, exactly like Pi's.
static FILE_MUTATION_LOCKS: LazyLock<Arc<DashMap<PathBuf, Arc<Mutex<()>>>>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// A handle onto the process-global map of per-path async mutexes, keyed by a fully-resolved
/// (realpath) path.
///
/// Every instance — however constructed — shares the one `FILE_MUTATION_LOCKS` map. Constructing a
/// second `FileMutationLocks` does NOT create a second lock domain; there is deliberately no way to
/// obtain an isolated one, because an isolated one is precisely the bug this type exists to
/// prevent. Tests stay independent by keying on distinct (temp-dir) paths, which is what Pi's
/// single-map design forces on its own tests too.
pub struct FileMutationLocks {
    map: Arc<DashMap<PathBuf, Arc<Mutex<()>>>>,
}

impl Default for FileMutationLocks {
    /// Hand-written, NOT derived: a derived `Default` builds `Arc::<DashMap<_, _>>::default()`,
    /// i.e. a fresh empty map, silently re-creating the per-owner lock domain this type exists to
    /// eliminate. It must be an alias for [`FileMutationLocks::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for a per-file mutation lock. On drop it releases the mutex and evicts the map entry
/// once no other holder/waiter references it (Pi deletes the queue entry when it drains,
/// file-mutation-queue.ts:57-59), so the lock map cannot grow without bound.
pub struct MutationGuard {
    inner: Option<OwnedMutexGuard<()>>,
    lock: Option<Arc<Mutex<()>>>,
    map: Arc<DashMap<PathBuf, Arc<Mutex<()>>>>,
    key: PathBuf,
}

impl Drop for MutationGuard {
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

impl FileMutationLocks {
    /// Attach to the process-global lock map (Pi's module-scope `fileMutationQueues`). This is a
    /// cheap `Arc` clone, not a fresh map — see the type docs.
    pub fn new() -> Self {
        Self { map: Arc::clone(&FILE_MUTATION_LOCKS) }
    }

    /// Full-symlink-resolved key (Pi `realpath(resolve(filePath))`, file-mutation-queue.ts:16-26).
    /// Falls back to the (already absolute) path when it does not exist yet — e.g. a `write` to a
    /// brand-new file — mirroring Pi's ENOENT/ENOTDIR fallback.
    fn key(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// Acquire the lock for `path` for the whole read-modify-write. Cancel-aware: returns
    /// `Err(aborted)` if cancelled before acquisition.
    pub async fn guard(
        &self,
        path: &Path,
        cancel: &CancelToken,
    ) -> Result<MutationGuard, ToolError> {
        let key = Self::key(path);
        let lock = self.map.entry(key.clone()).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        tokio::select! {
            _ = cancel.cancelled() => Err(error::aborted()),
            g = lock.clone().lock_owned() => Ok(MutationGuard {
                inner: Some(g),
                lock: Some(lock),
                map: Arc::clone(&self.map),
                key,
            }),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A path unique to the calling test. The map is process-global, so a literal like
    /// `/tmp/cyrup-lock-test-file` shared between two tests would make them contend under
    /// `cargo test`'s parallel harness. Distinct keys keep them independent — which is exactly the
    /// discipline Pi's single module-scope map imposes on its own tests.
    fn unique_path(tag: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("cyrup-lock-test-{tag}-{}-{n}", std::process::id()))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_path_serializes() {
        let locks = Arc::new(FileMutationLocks::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let path = unique_path("serialize");
        let cancel = CancelToken::new();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let locks = locks.clone();
            let counter = counter.clone();
            let max = max.clone();
            let path = path.clone();
            let cancel = cancel.clone();
            handles.push(tokio::spawn(async move {
                let _g = locks.guard(&path, &cancel).await.unwrap();
                let now = counter.fetch_add(1, Ordering::SeqCst) + 1;
                max.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                counter.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
        // Never more than one holder concurrently.
        assert_eq!(max.load(Ordering::SeqCst), 1);
    }

    /// The defining property: independently constructed handles are the SAME lock domain, so two
    /// owners (two `ToolRegistry`s, two `AgentSession`s) contend on one mutex per path. Pi gets
    /// this from a module-scope `Map`; we get it from a `LazyLock` static.
    ///
    /// `Default` is checked alongside `new()` on purpose — a *derived* `Default` would hand back a
    /// fresh empty map and silently reintroduce per-owner lock domains, and nothing else in the
    /// suite would notice.
    #[tokio::test]
    async fn independent_handles_share_one_lock_per_path() {
        let a = FileMutationLocks::new();
        let b = FileMutationLocks::new();
        let c = FileMutationLocks::default();
        let path = unique_path("shared-domain");
        let cancel = CancelToken::new();

        let key = FileMutationLocks::key(&path);
        let ga = a.guard(&path, &cancel).await.unwrap();

        // Same map object behind every handle...
        assert!(Arc::ptr_eq(&a.map, &b.map));
        assert!(Arc::ptr_eq(&a.map, &c.map));
        // ...and therefore the same mutex for the same path.
        let via_b = b.map.get(&key).map(|e| Arc::clone(e.value())).unwrap();
        let via_c = c.map.get(&key).map(|e| Arc::clone(e.value())).unwrap();
        assert!(Arc::ptr_eq(&via_b, &via_c));
        // Held by `a`, so a second owner genuinely cannot enter.
        assert!(via_b.try_lock().is_err());

        drop(ga);
        drop(via_b);
        drop(via_c);
    }

    /// Eviction still drains the entry (Pi deletes the queue key when it drains,
    /// file-mutation-queue.ts:57-59) — now verified against the global map, keyed on a path no
    /// other test uses, so it neither observes nor is observed by concurrent tests.
    #[tokio::test]
    async fn guard_evicts_its_entry_on_drop() {
        let locks = FileMutationLocks::new();
        let path = unique_path("evict");
        let key = FileMutationLocks::key(&path);
        let cancel = CancelToken::new();

        let g = locks.guard(&path, &cancel).await.unwrap();
        assert!(locks.map.contains_key(&key));
        drop(g);
        assert!(!locks.map.contains_key(&key), "drained entry must be evicted, not leaked");
    }

    /// Different paths must NOT serialize against each other even though they now share one map
    /// (Pi: "Operations for different files still run in parallel", file-mutation-queue.ts:30).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn distinct_paths_do_not_serialize() {
        let locks = Arc::new(FileMutationLocks::new());
        let cancel = CancelToken::new();
        let held = locks.guard(&unique_path("parallel-a"), &cancel).await.unwrap();
        // Would hang forever if a shared map meant a shared lock.
        let other = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            locks.guard(&unique_path("parallel-b"), &cancel),
        )
        .await
        .expect("a lock on a different path must not wait on this one")
        .unwrap();
        drop(other);
        drop(held);
    }
}
