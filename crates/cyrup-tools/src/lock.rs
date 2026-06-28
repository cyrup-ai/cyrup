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

/// A map of per-path async mutexes keyed by a canonicalized path.
#[derive(Default)]
pub struct FileMutationLocks {
    map: DashMap<PathBuf, Arc<Mutex<()>>>,
}

impl FileMutationLocks {
    pub fn new() -> Self {
        Self { map: DashMap::new() }
    }

    /// Canonical key: canonicalize the parent dir (it usually exists even for new files) and rejoin
    /// the file name, so different spellings of the same path share a lock. Falls back to the path.
    fn key(path: &Path) -> PathBuf {
        match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) => match std::fs::canonicalize(parent) {
                Ok(canon) => canon.join(name),
                Err(_) => path.to_path_buf(),
            },
            _ => path.to_path_buf(),
        }
    }

    /// Acquire the lock for `path` for the whole read-modify-write. Cancel-aware: returns
    /// `Err(aborted)` if cancelled before acquisition.
    pub async fn guard(
        &self,
        path: &Path,
        cancel: &CancelToken,
    ) -> Result<OwnedMutexGuard<()>, ToolError> {
        let key = Self::key(path);
        let lock = self.map.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        tokio::select! {
            _ = cancel.cancelled() => Err(error::aborted()),
            g = lock.lock_owned() => Ok(g),
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
