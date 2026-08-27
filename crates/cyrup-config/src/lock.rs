//! Cross-process advisory file locks + atomic writes with owner-only permissions
//! (arch-07 §5, R-07-014/015).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use cyrup_core::CancelToken;
use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};
use fs4::{FileExt, TryLockError};

use crate::error::ConfigError;

/// This crate's own lock domain, separate from `cyrup-tools`' file-mutation map: config paths and
/// tool-mutated paths are different key spaces, so two instances over the shared
/// [`cyrup_core::keyed_lock`] mechanism are correct.
static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(KeyedLockMap::new);

/// The handle over [`CONFIG_LOCKS`]. Built once; `KeyedLocks::new` is an `Arc` clone.
static CONFIG_LOCK_HANDLE: LazyLock<KeyedLocks<PathBuf>> =
    LazyLock::new(|| KeyedLocks::new(CONFIG_LOCKS.clone()));

/// Stand-in for callers that carry no cancellation of their own — settings, trust and auth, none of
/// which take a signal here or upstream (pi's `settings-manager`/`trust-manager` use `lockSync`,
/// and `auth-storage`'s `withLockAsync(fn)` takes no options). `CancellationToken::cancelled()` on
/// a token nobody cancels is a future that never resolves, so both `select!`s that consume it —
/// [`cyrup_core::keyed_lock::KeyedAcquire::wait`]'s and the layer-2 retry loop below — always take
/// their other arm. On the uncontended path, which is this lock's common case, `cancelled()` is
/// polled ZERO times: `wait` returns through its `early.take()` arm before the `select!` is ever
/// constructed, and the `is_cancelled()` guard above that is a plain synchronous read rather than a
/// poll — it allocates nothing and registers no waiter. Only a contended layer 1 reaches the
/// `select!`, and even then the poll costs no allocation (the waiter is intrusive) and no syscall
/// (an uncontended `std::sync::Mutex` is atomics-only).
static NEVER_CANCELLED: LazyLock<CancelToken> = LazyLock::new(CancelToken::new);

/// First retry delay for a contended layer 2, doubled each tick up to [`MAX_RETRY`]. An
/// uncontended acquire never reaches it: the first attempt rides along with the `open` in a single
/// blocking round-trip.
const FIRST_RETRY: Duration = Duration::from_millis(1);

/// Ceiling on the retry delay, and therefore on how long after a peer's release an acquire takes to
/// notice. It does NOT bound cancellation — the cancel arm shares the `select!` with the sleep, so
/// a `cancel()` preempts whatever is left of the tick. The only await points in `acquire` that are
/// not themselves cancel-aware are the two `spawn_blocking` joins, so a cancel arriving while an
/// attempt is already in flight is observed when that attempt returns: one bounded blocking job,
/// with no cross-process wait inside it. No duration is claimed for that window — `spawn_blocking`
/// can queue behind a saturated pool. Peer hold times span a sub-millisecond JSON
/// read-modify-write (settings, trust, models_store) to a whole OAuth refresh (`AuthStore::modify`
/// holds the guard across `f(current).await`), so 50 ms is small against the wait it samples and
/// costs at most 20 non-blocking syscalls per second per waiting acquire.
const MAX_RETRY: Duration = Duration::from_millis(50);

/// An RAII cross-process exclusive lock, taken on a sidecar `<path>.lock` file so the lock survives
/// atomic `rename` over the target (the lock inode is never replaced).
///
/// Two layers. In-process contention — several tasks in one cyrup process reaching the same config
/// file — queues on a per-key async mutex, the key being the sidecar path made lexically absolute
/// by [`lock_key`]: fair, cancel-aware, no syscall and no polling. Every spelling of one file that
/// does not differ through a symlink hashes to one entry, so that admits at most one task per file
/// per process, and this process presents at most one waiter to the cross-process lock.
///
/// The one alias layer 1 does not merge is a **symlink**: two such spellings are two keys, hence two
/// layer-2 waiters from this process on one inode. It costs an extra round trip, never correctness —
/// layer 2 locks the **inode**, so it excludes both aliases whichever name reached it. Upstream's
/// `mkdir` lock is path-keyed and does not, which is why this residue is affordable here and would
/// not have been in pi. Resolving it away would mean a `realpath`, and [`lock_key`] documents why
/// this domain must not take one.
///
/// **That bound holds only because the layer-2 wait lives inside [`FileLock::acquire`]'s own
/// future.** Layer 2 is a NON-blocking `flock(LOCK_EX|LOCK_NB)` retried from async land, never the
/// blocking `flock`, and the retry sleep is the drop point: dropping the acquire future ends the
/// wait and releases layer 1 in the same frame-drop, leaving at most one bounded attempt in flight
/// whose fd tokio closes when it reaps the output. Do not "optimise" this back into
/// `FileExt::lock`. A thread parked in `flock(2)` cannot be cancelled, aborted or timed out —
/// `spawn_blocking` tasks "cannot be aborted […] this *will not have any effect*", `JoinHandle`
/// drop detaches rather than cancels, and `Runtime::drop` then waits on that thread forever. The
/// only thing that breaks the syscall is a signal delivered to that exact thread, which a library
/// inside a host process has no business installing. So a blocking layer 2 turns every dropped
/// acquire into a pool thread pinned for as long as a peer holds the lock, while layer 1 — released
/// with the future — admits the next task straight into a second wait, without bound. Polling costs
/// one extra syscall per tick and keeps the wait abandonable; the kernel's wake-on-release is worth
/// nothing if the wait cannot be given up. Upstream polls too (`proper-lockfile@4.1.2`, 10 × 20 ms
/// for settings, exponential backoff for auth), for the unrelated reason that a `mkdir` lock offers
/// no readiness signal at all.
pub struct FileLock {
    /// Layer 1. Declared first only to mirror the acquisition order in [`FileLock::acquire`] and
    /// the `Ok(Self { .. })` that ends it; nothing depends on the position. The ordering that
    /// matters — the `flock` gone BEFORE a same-process successor is admitted through layer 1, so
    /// it never wakes into a lock this process still holds and pays a pointless trip to the
    /// kernel — comes from the explicit `FileExt::unlock` that is the first statement of the
    /// [`Drop`] impl below, and from nothing else: the `drop` body runs FIRST, then fields drop in
    /// DECLARATION order. Reverse-declaration order is a rule about LOCALS, not fields —
    /// `PendingEntry` in [`cyrup_core::keyed_lock`] is the correct use of it. So do not delete
    /// that `unlock` on the theory that closing `file` is equivalent: `file` closes AFTER this
    /// field, which is exactly backwards.
    _in_process: KeyedGuard<PathBuf>,
    /// Layer 2. Released by the explicit `unlock` in the [`Drop`] impl below; closing this fd is
    /// only the backstop for a process that dies holding it. The window between the two field
    /// drops, where the fd is open but unlocked, is harmless: `fs4` is `flock(2)` here, whose lock
    /// lives on the open file description, so a successor's own fd on the sidecar is unaffected.
    file: File,
}

impl FileLock {
    /// Acquire both layers for `target`, holding them until the returned guard drops.
    ///
    /// `cancel` governs BOTH layers, by two different mechanisms — the difference is load-bearing:
    ///
    /// * Layer 1 is cancelled *in place*: [`KeyedLocks::guard`] is
    ///   [`cyrup_core::keyed_lock::KeyedLocks::enqueue`] followed by
    ///   [`cyrup_core::keyed_lock::KeyedAcquire::wait`], and it is `wait` that handles the token —
    ///   an `is_cancelled()` pre-check that settles the already-cancelled case deterministically
    ///   BEFORE any `select!` runs, then a `biased` `select!` racing the token against the mutex.
    ///   Either way it returns having taken nothing, and the claimed queue place is evicted on the
    ///   way out.
    /// * Layer 2 is cancelled *between attempts*: the retry sleep shares a `biased` `select!` with
    ///   the same token, so a cancel preempts whatever is left of the tick and the acquire returns
    ///   [`ConfigError::Cancelled`] rather than waiting out a peer process. This is NOT bounded by
    ///   [`MAX_RETRY`] — see that constant's doc for what that window actually is.
    ///
    /// Dropping this future is bounded the same way: the whole layer-2 wait is in this frame, so
    /// the drop ends the wait and releases layer 1 together. At most one bounded blocking job is
    /// then in flight, and tokio discards its output — closing the fd, which releases the lock if
    /// that attempt had just won it. Contended acquires sleep between ticks, so this requires a
    /// runtime with a time driver enabled; an uncontended acquire never reaches the sleep.
    ///
    /// A token cancelled while an attempt is already in flight still yields a granted lock: there
    /// is no re-check after winning, because whether that matters is the caller's contract
    /// (`FileModelsStore`'s `read` re-checks after `read_latest` returns; its `write` and `delete`
    /// deliberately check only before the acquire, matching pi's placement). Dropping the returned
    /// guard releases both layers at once.
    ///
    /// `cancel` is `Some` only where the caller already has a token — today that is `models_store`,
    /// whose `ModelsStoreOperationOptions::signal` mirrors pi's `options` on the models-store path
    /// (`models-store.ts:132`). Everything else passes `None`; see [`NEVER_CANCELLED`].
    pub async fn acquire(target: &Path, cancel: Option<&CancelToken>) -> Result<Self, ConfigError> {
        let lock_path = lock_path_for(target);
        let token = cancel.unwrap_or(&NEVER_CANCELLED);
        // Layer 1 keys on the RESOLVED sidecar path; layer 2 opens the caller's own spelling. They
        // must stay separate: `flock` is inode-based, so the raw spelling reaches the same lock,
        // while `ConfigError::Io` / `ConfigError::Lock` keep naming the path the operator typed.
        let in_process = CONFIG_LOCK_HANDLE
            .guard(lock_key(&lock_path), token)
            .await
            .map_err(|_| ConfigError::Cancelled)?;

        // One blocking round-trip covers the uncontended case: `ensure_dir` + `open` + the first
        // attempt — the same syscalls the blocking version issued, on the same pool. The loop below
        // is entered only when a peer process actually holds the lock.
        let owned_target = target.to_path_buf();
        // The one outcome here that is NOT about the lock: a `JoinError` means the closure
        // panicked, or the runtime dropped the task while shutting down. Reported as
        // `ConfigError::Lock` it sends an operator hunting for a peer process that never existed.
        // Not re-panicked either: the release profile is `panic = "abort"` (root `Cargo.toml`), so
        // the panic arm is unreachable in a shipped binary, and in the unwinding builds where it is
        // reachable the panic hook has already printed message and location before tokio caught it.
        // Defensive, not expected — `clippy::panic`/`unwrap_used`/`expect_used`/`indexing_slicing`
        // are all denied workspace-wide, so nothing these closures call in-tree panics by
        // construction. The retry attempt below maps its join the same way.
        let joined =
            tokio::task::spawn_blocking(move || open_and_try_lock(&owned_target, &lock_path)).await;
        let (mut file, mut held) = match joined {
            Ok(result) => result?,
            Err(join) => return Err(join_failed(target, &join)),
        };

        // Enrolled ONCE, outside the loop: `CancellationToken::cancelled()` registers a waiter on
        // first poll, and rebuilding the future every tick would churn that registration.
        let cancelled = token.cancelled();
        tokio::pin!(cancelled);
        let mut backoff = FIRST_RETRY;
        while !held {
            tokio::select! {
                biased;
                // `biased` for the reason spelled out on `KeyedAcquire::wait`
                // (`cyrup-core/src/keyed_lock.rs`, above its `is_cancelled()` pre-check): with both
                // arms ready the unbiased poll order is random, and a caller that has given up must
                // not be handed the lock on a coin flip. Layer 1 now takes the already-cancelled
                // case with that pre-check; here `biased` is the whole guarantee, because this
                // `select!` is re-entered every tick and there is no single entry point to
                // pre-check.
                () = &mut cancelled => return Err(ConfigError::Cancelled),
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = backoff.saturating_mul(2).min(MAX_RETRY);
            let owned_target = target.to_path_buf();
            let joined = tokio::task::spawn_blocking(move || try_lock(file, &owned_target)).await;
            let (f, h) = match joined {
                Ok(result) => result?,
                Err(join) => return Err(join_failed(target, &join)),
            };
            file = f;
            held = h;
        }
        Ok(Self {
            _in_process: in_process,
            file,
        })
    }
}

/// The bounded blocking prologue: `ensure_dir`, the `open`, and the FIRST lock attempt. Every
/// syscall here completes on its own — nothing in it waits on another process, which is what makes
/// a detached copy of this job harmless.
fn open_and_try_lock(target: &Path, lock_path: &Path) -> Result<(File, bool), ConfigError> {
    if let Some(parent) = target.parent() {
        ensure_dir(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|e| io_err(lock_path, e))?;
    try_lock(file, target)
}

/// One non-blocking `flock` attempt. `Ok((file, true))` holds the lock; `Ok((file, false))` means a
/// peer holds it and the caller should retry.
///
/// The `File` is moved through rather than borrowed, so the whole attempt can cross
/// `spawn_blocking` — and so that on ANY exit, including a detached job whose output tokio
/// discards, the fd closes and releases whatever this attempt may have just won.
fn try_lock(file: File, target: &Path) -> Result<(File, bool), ConfigError> {
    loop {
        return match FileExt::try_lock(&file) {
            Ok(()) => Ok((file, true)),
            Err(TryLockError::WouldBlock) => Ok((file, false)),
            // `LOCK_NB` should not be interrupted, but rustix returns `EINTR` rather than
            // restarting (`fs4-1.1.0/src/unix.rs:57` is a bare passthrough), so retry at once
            // rather than reporting it as contention.
            Err(TryLockError::Error(e)) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // Names the TARGET, not the sidecar: `<path>.lock` is an implementation detail no
            // operator opens, and the file they must go look at is the config file itself.
            Err(TryLockError::Error(_)) => Err(ConfigError::Lock {
                path: target.to_path_buf(),
            }),
        };
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
        // The `<path>.lock` sidecar is deliberately NEVER unlinked. Upstream's `.lock` is a
        // *directory* created by `mkdir` and removed by `rmdir` on release
        // (`proper-lockfile@4.1.2`, `lib/lockfile.js:28-29`, `:88-90`) — for that primitive,
        // removal IS the release. This lock is `flock` on a regular file, where unlinking is the
        // classic advisory-lock race: unlink, another process recreates the path, and the two hold
        // locks on different inodes under the same name. Leaving the sidecar is correct here, so a
        // path field to enable the cleanup would be actively wrong rather than merely unused.
        //
        // For the same reason `proper-lockfile`'s `stale`/`onCompromised` have no analogue: they
        // detect a `mkdir` lock left by a crashed process, and the kernel releases `flock` when the
        // fd closes or the process dies.
    }
}

fn lock_path_for(target: &Path) -> PathBuf {
    let mut name = target
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    match target.parent() {
        Some(p) => p.join(name),
        None => PathBuf::from(name),
    }
}

/// Layer 1's key: [`lock_path_for`]'s sidecar path made **lexically absolute**, so every spelling of
/// one config file that a caller can hand [`FileLock::acquire`] hashes to one entry in
/// [`CONFIG_LOCKS`].
///
/// Node `path.resolve` semantics, NOT `realpath`, and that is upstream's own choice rather than a
/// shortcut. Every sync lock pi takes on these files opts out of realpath by hand —
/// `settings-manager.ts:206`, `trust-manager.ts:145`, `auth-storage.ts:56` all pass
/// `{ realpath: false }` against `proper-lockfile`'s `realpath: true` default — and pi makes the key
/// unambiguous at CONSTRUCTION instead, with `resolvePath(cwd)` / `resolvePath(agentDir)`
/// (settings-manager.ts:192-196), which is node's lexical `path.resolve` (utils/paths.ts:81-85) and
/// touches no filesystem. `cyrup-tools`' `FileMutationLocks::key` realpaths because ITS upstream
/// does (`getMutationQueueKey`, file-mutation-queue.ts:16-26). The two domains over
/// [`cyrup_core::keyed_lock`] resolve differently because the two upstreams do.
///
/// A realpath would also be actively worse here: [`open_and_try_lock`] is what runs [`ensure_dir`]
/// and `create(true)`, so this lock is routinely taken on a file — and inside a directory — that
/// does not exist yet. `realpath` returns `ENOENT` on exactly those first-run acquires and would
/// fall back to the unresolved spelling, leaving the gap open for the case where two tasks racing
/// to create one config file is most likely. It would additionally put a Windows `\\?\`-verbatim
/// form into the key, which is the divergence `ConfigDirs::resolve` removed a `canonicalize` to
/// avoid (env.rs, the SESS-036 note).
///
/// Residue, stated rather than papered over: two spellings that differ only through a **symlink**
/// still produce two keys, hence two layer-2 waiters from this process. That costs an extra retry
/// round trip and nothing else, because layer 2 is `flock` on an **inode** — both aliases reach the
/// same one, so mutual exclusion holds regardless of the name it was reached by. Upstream's
/// `mkdir`-based lock is path-keyed and does NOT hold under that alias, so this is stronger than pi
/// even with the residue.
fn lock_key(lock_path: &Path) -> PathBuf {
    if lock_path.is_absolute() {
        return crate::paths::lexically_normalize(lock_path);
    }
    // `getcwd(3)` — a plain syscall on the rare relative branch, not a blocking-pool hop. A
    // relative key would otherwise name whatever the cwd points at when the lock is taken, while
    // `open(2)` in `open_and_try_lock` resolves it against the cwd at open time: the two layers
    // would stop agreeing about which file they protect. `--agent-dir ./cfg` and
    // `CYRUP_AGENT_DIR=./cfg` reach here, because `EnvVars`' `normalize_path_buf` expands `~` and
    // `file://` and stops — see [`crate::paths::normalize_path`], "this is NOT `resolve`".
    match std::env::current_dir() {
        Ok(cwd) => crate::paths::lexically_normalize(&cwd.join(lock_path)),
        // No reachable cwd: the `open(2)` in `open_and_try_lock` is about to fail on the same
        // relative path, so keep the raw spelling and let layer 2 produce the error that names it.
        Err(_) => lock_path.to_path_buf(),
    }
}

/// Create a directory (and parents) with owner-only (0700) permissions on unix.
pub fn ensure_dir(dir: &Path) -> Result<(), ConfigError> {
    if dir.as_os_str().is_empty() || dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(dir, perms).map_err(|e| io_err(dir, e))?;
    }
    Ok(())
}

/// Atomically write `bytes` to `path` via temp-file + rename. When `secret`, the file is created
/// with 0600 permissions and its parent dir as 0700 (R-07-014).
pub fn write_atomic(path: &Path, bytes: &[u8], secret: bool) -> Result<(), ConfigError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        ensure_dir(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    let mut tmp_name = file_name;
    tmp_name.push(format!(".tmp.{}", std::process::id()));
    let tmp_path = match parent {
        Some(p) => p.join(&tmp_name),
        None => PathBuf::from(&tmp_name),
    };

    {
        let mut opts = OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp_path).map_err(|e| io_err(&tmp_path, e))?;
        f.write_all(bytes).map_err(|e| io_err(&tmp_path, e))?;
        f.sync_all().map_err(|e| io_err(&tmp_path, e))?;
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| io_err(&tmp_path, e))?;
        }
    }

    std::fs::rename(&tmp_path, path).map_err(|e| io_err(path, e))?;
    Ok(())
}

/// Tag an `io::Error` with the path whose syscall produced it, so the rendered error names
/// the file the operator has to go look at.
fn io_err(path: &Path, source: std::io::Error) -> ConfigError {
    ConfigError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// A `spawn_blocking` acquisition attempt that never produced a result: the task panicked, or the
/// runtime dropped it while shutting down. Names the TARGET for the same reason `try_lock` does —
/// the `<path>.lock` sidecar is not a file any operator opens — and carries the `JoinError`'s own
/// text, which includes the panic payload when there is one.
fn join_failed(target: &Path, join: &tokio::task::JoinError) -> ConfigError {
    ConfigError::LockTaskFailed {
        path: target.to_path_buf(),
        message: join.to_string(),
    }
}
