//! Cross-process advisory file locks + atomic writes with owner-only permissions
//! (arch-07 §5, R-07-014/015).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use cyrup_core::CancelToken;
use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};
use dashmap::DashMap;
use fs4::{FileExt, TryLockError};

use crate::error::ConfigError;

/// This crate's own lock domain, separate from `cyrup-tools`' file-mutation map: config paths and
/// tool-mutated paths are different key spaces, so two instances over the shared
/// [`cyrup_core::keyed_lock`] mechanism are correct.
static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(|| Arc::new(DashMap::new()));

/// The handle over [`CONFIG_LOCKS`]. Built once; `KeyedLocks::new` is an `Arc` clone.
static CONFIG_LOCK_HANDLE: LazyLock<KeyedLocks<PathBuf>> =
    LazyLock::new(|| KeyedLocks::new(Arc::clone(&CONFIG_LOCKS)));

/// Stand-in for callers that carry no cancellation of their own — settings, trust and auth, none of
/// which take a signal here or upstream (pi's `settings-manager`/`trust-manager` use `lockSync`,
/// and `auth-storage`'s `withLockAsync(fn)` takes no options). `CancellationToken::cancelled()` on
/// a token nobody cancels is a future that never resolves, so both `select!`s that consume it —
/// [`KeyedLocks::guard`]'s and the layer-2 retry loop below — always take their other arm. It is
/// polled once per acquire, in `guard`'s biased first branch, and not again on the uncontended
/// path because the retry loop is skipped. That poll costs no allocation (the waiter is intrusive)
/// and no syscall (an uncontended `std::sync::Mutex` is atomics-only).
static NEVER_CANCELLED: LazyLock<CancelToken> = LazyLock::new(CancelToken::new);

/// First retry delay for a contended layer 2, doubled each tick up to [`MAX_RETRY`]. An
/// uncontended acquire never reaches it: the first attempt rides along with the `open` in a single
/// blocking round-trip.
const FIRST_RETRY: Duration = Duration::from_millis(1);

/// Ceiling on the retry delay, and therefore on how long after a peer's release an acquire takes to
/// notice. It does NOT bound cancellation — the cancel arm shares the `select!` with the sleep, so
/// a `cancel()` preempts whatever is left of the tick; only a cancel arriving while an attempt is
/// already in flight waits at all, for the microseconds that attempt runs. Peer hold times span a
/// sub-millisecond JSON read-modify-write (settings, trust, models_store) to a whole OAuth refresh
/// (`auth.rs:316-324` holds the guard across `f(current).await`), so 50 ms is small against the
/// wait it samples and costs at most 20 non-blocking syscalls per second per waiting acquire.
const MAX_RETRY: Duration = Duration::from_millis(50);

/// An RAII cross-process exclusive lock, taken on a sidecar `<path>.lock` file so the lock survives
/// atomic `rename` over the target (the lock inode is never replaced).
///
/// Two layers. In-process contention — several tasks in one cyrup process reaching the same config
/// file — queues on a per-path async mutex: fair, cancel-aware, no syscall and no polling. That
/// admits at most one task per path per process, so this process presents at most one waiter to the
/// cross-process lock.
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
    /// Layer 1. Declared FIRST so reverse-declaration drop order releases the `flock` BEFORE this —
    /// otherwise a same-process successor wakes out of layer 1 into a still-held `flock` and pays a
    /// pointless trip to the kernel.
    _in_process: KeyedGuard<PathBuf>,
    /// Layer 2, released by [`Drop`] below.
    file: File,
}

impl FileLock {
    /// Acquire both layers for `target`, holding them until the returned guard drops.
    ///
    /// `cancel` governs BOTH layers: layer 1 through the `biased` cancel arm inside
    /// [`KeyedLocks::guard`], layer 2 between retry ticks — so a cancelled acquire returns
    /// [`ConfigError::Cancelled`] within [`MAX_RETRY`] instead of waiting out a peer process.
    /// Dropping this future is bounded the same way: the whole layer-2 wait is in this frame, so
    /// the drop ends the wait and releases layer 1 together. At most one bounded blocking job is
    /// then in flight, and tokio discards its output — closing the fd, which releases the lock if
    /// that attempt had just won it. Contended acquires sleep between ticks, so this requires a
    /// runtime with a time driver enabled; an uncontended acquire never reaches the sleep.
    ///
    /// A token cancelled while an attempt is already in flight still yields a granted lock: there
    /// is no re-check after winning, because whether that matters is the caller's contract
    /// (`models_store.rs:311` re-checks after the read; `write`/`delete` deliberately check only
    /// before, matching pi's placement). Dropping the returned guard releases both layers at once.
    ///
    /// `cancel` is `Some` only where the caller already has a token — today that is `models_store`,
    /// whose `ModelsStoreOperationOptions::signal` mirrors pi's `options` on the models-store path
    /// (`models-store.ts:132`). Everything else passes `None`; see [`NEVER_CANCELLED`].
    pub async fn acquire(target: &Path, cancel: Option<&CancelToken>) -> Result<Self, ConfigError> {
        let lock_path = lock_path_for(target);
        let token = cancel.unwrap_or(&NEVER_CANCELLED);
        let in_process = CONFIG_LOCK_HANDLE
            .guard(lock_path.clone(), token)
            .await
            .map_err(|_| ConfigError::Cancelled)?;

        // One blocking round-trip covers the uncontended case: `ensure_dir` + `open` + the first
        // attempt — the same syscalls the blocking version issued, on the same pool. The loop below
        // is entered only when a peer process actually holds the lock.
        let owned_target = target.to_path_buf();
        let (mut file, mut held) =
            tokio::task::spawn_blocking(move || open_and_try_lock(&owned_target, &lock_path))
                .await
                .map_err(|_| ConfigError::Lock {
                    path: target.to_path_buf(),
                })??;

        // Enrolled ONCE, outside the loop: `CancellationToken::cancelled()` registers a waiter on
        // first poll, and rebuilding the future every tick would churn that registration.
        let cancelled = token.cancelled();
        tokio::pin!(cancelled);
        let mut backoff = FIRST_RETRY;
        while !held {
            tokio::select! {
                biased;
                // `biased` for the reason spelled out in `KeyedLocks::guard`: with both arms ready
                // the unbiased poll order is random, and a caller that has given up must not be
                // handed the lock on a coin flip.
                () = &mut cancelled => return Err(ConfigError::Cancelled),
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = backoff.saturating_mul(2).min(MAX_RETRY);
            let owned_target = target.to_path_buf();
            let (f, h) = tokio::task::spawn_blocking(move || try_lock(file, &owned_target))
                .await
                .map_err(|_| ConfigError::Lock {
                    path: target.to_path_buf(),
                })??;
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
