//! Per-file mutation locking (R-03-007, arch-03 §5).
//!
//! `write`/`edit` serialize per canonical path so concurrent mutations (parallel tool calls,
//! R-02-016) of the same file cannot interleave/corrupt, while different files proceed
//! concurrently. Lock acquisition is itself cancel-aware.

use crate::error;
use cyrup_core::{CancelToken, ToolError};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// A map of per-path async mutexes keyed by a fully-resolved (realpath) path.
#[derive(Default)]
pub struct FileMutationLocks {
    map: Arc<DashMap<PathBuf, Arc<Mutex<()>>>>,
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
    pub fn new() -> Self {
        Self { map: Arc::new(DashMap::new()) }
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn same_path_serializes() {
        let locks = Arc::new(FileMutationLocks::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let path = PathBuf::from("/tmp/cyrup-lock-test-file");
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
}
