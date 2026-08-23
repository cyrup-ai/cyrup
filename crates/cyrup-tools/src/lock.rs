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
use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};
use cyrup_core::{CancelToken, ToolError};
use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

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
static FILE_MUTATION_LOCKS: LazyLock<KeyedLockMap<PathBuf>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// Pi `isMissingPathError` (file-mutation-queue.ts:7-14): `error.code === "ENOENT" || error.code
/// === "ENOTDIR"`, and NOTHING else. Every other realpath failure is re-thrown at `:24`.
fn is_missing_path_error(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::NotFound {
        return true;
    }
    #[cfg(unix)]
    {
        // `ENOTDIR` has no stable `std::io::ErrorKind`, so match the raw errno.
        e.raw_os_error() == Some(libc::ENOTDIR)
    }
    #[cfg(not(unix))]
    {
        // Win32's `ERROR_PATH_NOT_FOUND` / `ERROR_DIRECTORY` are what libuv maps to ENOTDIR; both
        // surface as `NotFound` above on this platform, so there is nothing further to test.
        false
    }
}

/// A handle onto the process-global map of per-path async mutexes, keyed by a fully-resolved
/// (realpath) path.
///
/// Every instance — however constructed — shares the one `FILE_MUTATION_LOCKS` map. Constructing a
/// second `FileMutationLocks` does NOT create a second lock domain; there is deliberately no way to
/// obtain an isolated one, because an isolated one is precisely the bug this type exists to
/// prevent. Tests stay independent by keying on distinct (temp-dir) paths, which is what Pi's
/// single-map design forces on its own tests too.
pub struct FileMutationLocks {
    /// The same map `inner` is built over. Kept as a field, rather than reached through `inner`,
    /// because the tests in this file assert on map membership directly — which is what lets the
    /// move of the registry mechanics into `cyrup-core` be proven with no test changes at all.
    /// Hence the gate: outside `cfg(test)` nothing reads it, and that is the intended state.
    #[cfg_attr(not(test), allow(dead_code))]
    map: KeyedLockMap<PathBuf>,
    /// The registry mechanics (RAII guard, entry eviction, the drop-gap handling and the `biased`
    /// cancel race) live in [`cyrup_core::keyed_lock`]; `cyrup-config` locks its own key domain
    /// over the same code. What stays here is this crate's keying and error vocabulary.
    inner: KeyedLocks<PathBuf>,
}

impl Default for FileMutationLocks {
    /// Hand-written, NOT derived: a derived `Default` builds `Arc::<DashMap<_, _>>::default()`,
    /// i.e. a fresh empty map, silently re-creating the per-owner lock domain this type exists to
    /// eliminate. It must be an alias for [`FileMutationLocks::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for a per-file mutation lock — [`cyrup_core::keyed_lock::KeyedGuard`] keyed by the
/// resolved path. On drop it releases the mutex and evicts the map entry once no other
/// holder/waiter references it (Pi deletes the queue entry when it drains,
/// file-mutation-queue.ts:57-59), so the lock map cannot grow without bound.
pub type MutationGuard = KeyedGuard<PathBuf>;

impl FileMutationLocks {
    /// Attach to the process-global lock map (Pi's module-scope `fileMutationQueues`). This is a
    /// cheap `Arc` clone, not a fresh map — see the type docs.
    pub fn new() -> Self {
        let map = Arc::clone(&FILE_MUTATION_LOCKS);
        Self { inner: KeyedLocks::new(Arc::clone(&map)), map }
    }

    /// Full-symlink-resolved key — a 1:1 port of Pi's `getMutationQueueKey`
    /// (file-mutation-queue.ts:16-26):
    /// ```ts
    /// async function getMutationQueueKey(filePath: string): Promise<string> {
    ///   const resolvedPath = resolve(filePath);
    ///   try { return await realpath(resolvedPath); }
    ///   catch (error) { if (isMissingPathError(error)) return resolvedPath; throw error; }
    /// }
    /// ```
    ///
    /// Two properties are load-bearing and were both wrong before.
    ///
    /// 1. **Non-blocking.** Pi's helper is `async` and `await realpath(...)`. `std::fs::canonicalize`
    ///    is a BLOCKING `realpath(2)` on a tokio worker thread, and [`Self::guard`] is awaited by
    ///    both mutators on every call, so an NFS/SSHFS/FUSE mount or a deep symlink chain stalled
    ///    the whole runtime — unrelated in-flight tool calls and the provider stream included.
    ///    `tokio::fs::canonicalize` runs it on the blocking pool instead.
    /// 2. **A narrow catch.** Pi's `isMissingPathError` (file-mutation-queue.ts:7-14) tests ONLY
    ///    `ENOENT` and `ENOTDIR` — the "writing a brand-new file" case — and `:24` re-`throw`s
    ///    everything else, which propagates out of `withFileMutationQueue` (`:34`) BEFORE `fn()`
    ///    runs, so the write/edit fails with the realpath errno rather than the later `open(2)`
    ///    one. `unwrap_or_else(|_| …)` swallowed every kind, including EACCES on a parent's search
    ///    bit, ELOOP and ENAMETOOLONG.
    async fn key(path: &Path) -> Result<PathBuf, ToolError> {
        match tokio::fs::canonicalize(path).await {
            Ok(resolved) => Ok(resolved),
            // Pi's `isMissingPathError`: ENOENT / ENOTDIR only. `ErrorKind::NotFound` is ENOENT;
            // ENOTDIR has no stable `ErrorKind` on stable Rust, so it is matched on the raw errno.
            Err(e) if is_missing_path_error(&e) => Ok(path.to_path_buf()),
            Err(e) => Err(error::io_errno(&error::show(path), &e)),
        }
    }

    /// Acquire the lock for `path` for the whole read-modify-write. Cancel-aware: returns
    /// `Err(aborted)` if cancelled before acquisition — *always*, not just when the mutex happens
    /// to be contended.
    ///
    /// The registry mechanics are [`cyrup_core::keyed_lock::KeyedLocks::guard`]: the entry is
    /// inserted before the mutex is awaited and evicted on every exit including a dropped future,
    /// and the cancel arm is polled `biased` so a pre-cancelled token is deterministic rather than
    /// a coin flip. What is this crate's own is the realpath keying above and the mapping of
    /// `Cancelled` onto pi's "Operation aborted" — the abort check is the FIRST statement inside
    /// pi's queue body (`throwIfAborted()`, write.ts:218 / edit.ts:327, defined at
    /// write.ts:213-215), so an already-aborted `write`/`edit` must throw before any filesystem
    /// call.
    pub async fn guard(
        &self,
        path: &Path,
        cancel: &CancelToken,
    ) -> Result<MutationGuard, ToolError> {
        let key = Self::key(path).await?;
        self.inner.guard(key, cancel).await.map_err(|_| error::aborted())
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

        let key = FileMutationLocks::key(&path).await.unwrap();
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
        let key = FileMutationLocks::key(&path).await.unwrap();
        let cancel = CancelToken::new();

        let g = locks.guard(&path, &cancel).await.unwrap();
        assert!(locks.map.contains_key(&key));
        drop(g);
        assert!(
            !locks.map.contains_key(&key),
            "drained entry must be evicted, not leaked"
        );
    }

    /// A cancelled acquisition must evict its map entry too. Pi's `finally`
    /// (file-mutation-queue.ts:57-59) always runs because a JS `async` function always settles;
    /// cyrup's `guard()` inserted the entry and then `select!`ed, so the cancel arm returned
    /// `Err(aborted)` leaving a permanently unreferenced entry in a process-global static.
    ///
    /// Structural, not timing-based: the lock is HELD by `held`, so the second acquisition is
    /// genuinely parked on the mutex — the exact state an Esc lands in — and the cancel arm is the
    /// only way out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it() {
        let locks = FileMutationLocks::new();
        let path = unique_path("cancel-evict");
        let key = FileMutationLocks::key(&path).await.unwrap();

        let held = locks.guard(&path, &CancelToken::new()).await.unwrap();
        // PRESENCE FIRST, so the absence assertion below cannot pass vacuously.
        assert!(locks.map.contains_key(&key));

        let cancel = CancelToken::new();
        cancel.cancel();
        let err = locks.guard(&path, &cancel).await;
        assert!(err.is_err(), "a pre-cancelled acquisition must abort");
        // Still held by `held`, so the entry legitimately survives this drop.
        assert!(
            locks.map.contains_key(&key),
            "an entry with a live holder must not be evicted"
        );

        drop(held);
        assert!(!locks.map.contains_key(&key));

        // And with NO other holder at all, the cancelled acquisition must leave nothing behind.
        // This is also the case that pins `guard()`'s `biased` ordering: with the mutex free, BOTH
        // `select!` arms are ready on the first poll, and an unbiased `select!` chose between them
        // at random — this `is_err()` failed roughly half the time.
        let path2 = unique_path("cancel-evict-solo");
        let key2 = FileMutationLocks::key(&path2).await.unwrap();
        let cancel2 = CancelToken::new();
        cancel2.cancel();
        assert!(locks.guard(&path2, &cancel2).await.is_err());
        assert!(
            !locks.map.contains_key(&key2),
            "RED before the fix: the entry stayed in the process-global map forever"
        );
    }

    /// The same leak reached by DROPPING the `guard()` future rather than by cancelling it — the
    /// shape a `tokio::select!`/`timeout` at a call site produces, which no `CancelToken` observes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_acquisition_future_evicts_its_entry() {
        let locks = FileMutationLocks::new();
        let path = unique_path("drop-evict");
        let key = FileMutationLocks::key(&path).await.unwrap();
        let cancel = CancelToken::new();

        let held = locks.guard(&path, &cancel).await.unwrap();
        assert!(locks.map.contains_key(&key));

        // Drop the acquisition mid-`.await`, deterministically and with no wall clock: `biased`
        // polls the arms in order, so the acquisition IS polled (inserting its map entry and then
        // parking on the held mutex) before the already-ready arm wins and `select!` drops it.
        tokio::select! {
            biased;
            _ = locks.guard(&path, &cancel) => unreachable!("`held` owns the lock, so this cannot resolve"),
            () = std::future::ready(()) => {}
        }

        drop(held);
        assert!(
            !locks.map.contains_key(&key),
            "the dropped acquisition must not keep the entry alive"
        );
    }

    /// Pi's key helper catches ONLY `ENOENT`/`ENOTDIR` (`isMissingPathError`,
    /// file-mutation-queue.ts:7-14) and returns the resolved path for them. A `write` to a file
    /// that does not exist yet is exactly that case and must still acquire a lock.
    #[tokio::test]
    async fn missing_path_falls_back_to_the_resolved_path() {
        let path = unique_path("enoent");
        assert!(!path.exists());
        let key = FileMutationLocks::key(&path)
            .await
            .expect("ENOENT is Pi's fallback, not an error");
        assert_eq!(key, path);
        // A path whose PARENT is a regular file is ENOTDIR, Pi's other caught kind.
        let file = unique_path("enotdir-parent");
        std::fs::write(&file, b"x").unwrap();
        let under = file.join("child.txt");
        let key = FileMutationLocks::key(&under)
            .await
            .expect("ENOTDIR is Pi's other fallback");
        assert_eq!(key, under);
        let _ = std::fs::remove_file(&file);
    }

    /// The other half of `isMissingPathError`: every NON-missing realpath failure is re-`throw`n
    /// at file-mutation-queue.ts:24, propagating out of `withFileMutationQueue` (`:34`) BEFORE the
    /// mutation body runs. `unwrap_or_else(|_| path.to_path_buf())` swallowed all of them, so
    /// cyrup took a differently-keyed lock and only failed later at `open(2)` with a different
    /// errno. RED before the fix (`guard` returned `Ok`), GREEN after.
    #[cfg(unix)]
    #[tokio::test]
    async fn non_missing_realpath_failure_propagates_instead_of_being_swallowed() {
        use std::os::unix::fs::PermissionsExt;
        let dir = unique_path("eacces-parent");
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("f.txt");
        std::fs::write(&target, b"x").unwrap();
        // Drop the parent's search bit: `realpath(2)` on anything inside now fails EACCES.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        // root ignores the mode bits — same as Node for root, so skip rather than assert.
        if std::fs::canonicalize(&target).is_ok() {
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }

        let locks = FileMutationLocks::new();
        let err = locks
            .guard(&target, &CancelToken::new())
            .await
            .err()
            .expect("a non-ENOENT realpath failure must propagate (file-mutation-queue.ts:24)");
        let msg = err.to_string();

        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            msg.starts_with("EACCES: "),
            "must name the realpath errno, got: {msg}"
        );
        assert!(
            msg.contains(&*target.to_string_lossy()),
            "must name the path, got: {msg}"
        );
    }

    /// Different paths must NOT serialize against each other even though they now share one map
    /// (Pi: "Operations for different files still run in parallel", file-mutation-queue.ts:30).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn distinct_paths_do_not_serialize() {
        let locks = Arc::new(FileMutationLocks::new());
        let cancel = CancelToken::new();
        let held = locks
            .guard(&unique_path("parallel-a"), &cancel)
            .await
            .unwrap();
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
