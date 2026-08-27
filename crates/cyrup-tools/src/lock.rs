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
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

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
static FILE_MUTATION_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(KeyedLockMap::new);

/// Pi's module-scope `registrationQueue` (file-mutation-queue.ts:5).
///
/// Pi funnels EVERY registration — the `realpath` key resolution at `:34` and the queue link at
/// `:42` — through one chain, so registrations happen one at a time and in call order. This is the
/// Rust equivalent: `tokio::sync::Mutex` is strictly FIFO (tokio 1.52.3 src/sync/mutex.rs:20-22),
/// so entering it in dispatch order is sufficient to leave it in dispatch order — this is the
/// documented case (tasks calling `lock`), unlike `KeyedLocks::enqueue`'s first-poll claim, which
/// carries its own caveat.
///
/// It is deliberately GLOBAL, not per-path: a slow `realpath` on one mount delays registrations for
/// every other path too, exactly as pi's single chain does. That is the upstream behaviour, and it
/// is bounded — the chain is released as soon as the key is resolved and the per-key place is
/// claimed, never across the mutation itself.
static MUTATION_REGISTRATION: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Test-only observer on the registration chain, so the tests below can assert on its STATE
/// instead of on a sleep. `pub(crate)` because `crate::tests::mutation_lock_is_first_await` needs
/// it too; `cfg(test)` because nothing outside the suite may ever reach the static.
#[cfg(test)]
pub(crate) fn registration_is_held() -> bool {
    MUTATION_REGISTRATION.try_lock().is_err()
}

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
    /// A second handle on the map `inner` is built over, held **per instance** so the tests below
    /// can assert on map membership and on map *identity*. Four tests read it —
    /// `independent_handles_share_one_lock_per_path`, `guard_evicts_its_entry_on_drop`,
    /// `a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it` and
    /// `a_forfeited_queue_place_evicts_its_entry`; `the_registration_chain_spans_key_resolution`
    /// sits beside them, pinning the `MUTATION_REGISTRATION` chain that puts those entries in call
    /// order rather than the map itself — [`cyrup_core::keyed_lock`] now carries its own tests of
    /// the enqueue/wait split and of eviction on a forfeited place; what stays here is the same
    /// coverage over THIS crate's keying and error vocabulary, plus map IDENTITY, which only a
    /// per-instance handle can express.
    ///
    /// Two cleanups look available here and are not:
    ///
    /// - *Reach through `inner`.* There is no route. [`KeyedLocks`] keeps its map private and
    ///   exposes only `new`, `enqueue` and `guard` — no accessor, no `Deref`. Adding one would
    ///   also be the
    ///   wrong direction: the consumer already owns the map. `FILE_MUTATION_LOCKS` is declared
    ///   here and a clone of it is handed to `KeyedLocks::new`, so an accessor would be
    ///   `cyrup-core` re-exporting state its caller supplied. Nor would the observers it hands
    ///   back be free of consequence — `KeyedLockMap::mutex_for` returns an `Arc` clone that
    ///   defers eviction of the entry for as long as it is held.
    /// - *Use the `FILE_MUTATION_LOCKS` static.* It is in scope for the tests and would serve the
    ///   three eviction tests, but it is one object by construction, so it cannot express
    ///   `a.map.ptr_eq(&b.map)`: the check that a separately constructed
    ///   `FileMutationLocks` joins this lock domain instead of silently getting an isolated one —
    ///   precisely the bug this type exists to prevent, and the reason `Default` below is
    ///   hand-written.
    ///
    /// Holding the map per instance is also why no test needed a behavioural change when the
    /// registry mechanics moved into [`cyrup_core::keyed_lock`].
    ///
    /// Outside `cfg(test)` nothing reads it, and that is the intended state; hence the gate.
    #[cfg_attr(not(test), allow(dead_code))]
    map: KeyedLockMap<PathBuf>,
    /// The registry mechanics (RAII guard, entry eviction, the drop-gap handling and the `biased`
    /// cancel race) live in [`cyrup_core::keyed_lock`]; `cyrup-config` locks its own key domain
    /// over the same code. What stays here is this crate's keying and error vocabulary.
    inner: KeyedLocks<PathBuf>,
    /// Test-only, PER-INSTANCE witness that [`Self::guard`]'s body has begun executing.
    ///
    /// [`registration_is_held`] above reads `MUTATION_REGISTRATION`, which is process-global by
    /// design (pi parity, see its docs) and is taken by EVERY `write`/`edit` in the lib test
    /// binary — including `the_registration_chain_spans_key_resolution` below, which parks inside
    /// `Self::key` holding it. A test that samples that static therefore observes other tests, and
    /// `crate::tests::mutation_lock_is_first_await` degraded to 2/3 detection under the full suite
    /// because of it. This counter is reachable only through the `FileMutationLocks` the observing
    /// test constructed and handed to its own tool, so nothing else in the binary can move it.
    #[cfg(test)]
    guard_entries: std::sync::atomic::AtomicUsize,
}

impl Default for FileMutationLocks {
    /// Hand-written, NOT derived: `Default` here must mean "attach to the process-global map",
    /// never "a fresh empty one" — a per-owner lock domain is precisely the bug this type exists
    /// to eliminate. It must be an alias for [`FileMutationLocks::new`].
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for a per-file mutation lock — a newtype over
/// [`cyrup_core::keyed_lock::KeyedGuard`] keyed by the resolved path. On drop it releases the
/// mutex and evicts the map entry once no other holder/waiter references it (Pi deletes the queue
/// entry when it drains, file-mutation-queue.ts:57-59), so the lock map cannot grow without bound.
///
/// A newtype and NOT a `pub type` alias, deliberately. `cyrup-config` instantiates the same
/// generic over the same key type for its own, deliberately separate map — its `CONFIG_LOCKS`
/// static and the `FileLock::_in_process` field — so an alias would make a guard proving
/// exclusion over config paths and one proving it over tool-mutated paths literally the same Rust
/// type, and `fn commit(_: MutationGuard)` would accept either. Nothing passes a guard as a value
/// today; the wrapper is what keeps the day one does from type-checking against the wrong domain.
/// It costs nothing: `KeyedGuard` has no public operations to forward, and drop order, drop
/// behaviour and auto-trait membership are exactly the field's. It also re-opens
/// `impl MutationGuard` in this crate, which E0116 forbids on the aliased foreign type.
pub struct MutationGuard(#[expect(dead_code, reason = "held for its Drop")] KeyedGuard<PathBuf>);

impl FileMutationLocks {
    /// Attach to the process-global lock map (Pi's module-scope `fileMutationQueues`). This is a
    /// cheap `Arc` clone, not a fresh map — see the type docs.
    pub fn new() -> Self {
        let map = FILE_MUTATION_LOCKS.clone();
        Self {
            inner: KeyedLocks::new(map.clone()),
            map,
            #[cfg(test)]
            guard_entries: std::sync::atomic::AtomicUsize::new(0),
        }
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

    /// How many times [`Self::guard`]'s body has begun executing on THIS instance.
    ///
    /// `1` after a single poll of a `write`/`edit` body means the caller reached `guard()` without
    /// suspending first — which is exactly the property
    /// `crate::tests::mutation_lock_is_first_await` exists to pin. `0` means an `.await` above
    /// `guard()` suspended and took the `execute_parallel` handoff with it.
    #[cfg(test)]
    pub(crate) fn guard_entries(&self) -> usize {
        self.guard_entries.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Acquire the lock for `path` for the whole read-modify-write. Cancel-aware: returns
    /// `Err(aborted)` if cancelled before acquisition — *always*, not just when the mutex happens
    /// to be contended.
    ///
    /// The registry mechanics are [`cyrup_core::keyed_lock::KeyedLocks::enqueue`] plus
    /// [`cyrup_core::keyed_lock::KeyedAcquire::wait`]: the entry is inserted before the mutex is
    /// awaited and evicted on every exit including a dropped future, and cancellation is tested
    /// before the wait so a pre-cancelled token is deterministic rather than a coin flip. What is
    /// this crate's own is the realpath keying above, the `MUTATION_REGISTRATION` chain that puts
    /// key resolution and the queue claim in call order, and the mapping of `Cancelled` onto pi's
    /// "Operation aborted" — the abort check is the FIRST statement inside pi's queue body
    /// (`throwIfAborted()`, write.ts:218 / edit.ts:327, defined at write.ts:213-215), so an
    /// already-aborted `write`/`edit` must throw before any filesystem call.
    ///
    /// **Ordering.** Two mutations of the same file must be granted in the order the model issued
    /// them, so the payload of the LAST one survives (pi: `registrationQueue`,
    /// file-mutation-queue.ts:5/:33/:46-49). That requires the caller to REACH this function in
    /// dispatch order — `write::execute`/`edit::execute` keep `guard()` as their first `.await`
    /// for exactly that reason, and `cyrup-agent`'s parallel batch starts its spawned bodies in
    /// source order. Introducing an `.await` before either `guard()` call reopens the gap.
    pub async fn guard(
        &self,
        path: &Path,
        cancel: &CancelToken,
    ) -> Result<MutationGuard, ToolError> {
        // Test-only witness, deliberately ABOVE every `.await` in this body: an async fn's body
        // does not run until its future is polled, so reaching this line at all means the caller
        // got here without suspending. Compiles to nothing outside `cfg(test)`.
        #[cfg(test)]
        self.guard_entries
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Pi `:33`: the registration slot is claimed in call order and the body below runs
        // serialized.
        let registration = MUTATION_REGISTRATION.lock().await;

        // Pi `:34`: `await getMutationQueueKey(filePath)` — INSIDE the chain, so two spellings of
        // the same file resolve in call order instead of racing on the blocking pool. On the `?`
        // path the guard drops here and the chain advances, matching pi `:46-49`, which advances
        // the chain on rejection as well as on fulfilment.
        let key = Self::key(path).await?;

        // Pi `:35-42`: link into this key's queue. `enqueue` never yields, so the place is taken
        // while the registration is still held.
        let acquire = self.inner.enqueue(key).await;

        // Pi `:51`: registration is complete; the chain advances. Everything after this point is
        // pi's `await currentQueue`, which happens outside the chain.
        drop(registration);

        acquire
            .wait(cancel)
            .await
            .map(MutationGuard)
            .map_err(|_| error::aborted())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
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

    /// Poll `f` exactly once with a no-op waker. No wall clock, no scheduler involvement.
    fn poll_once<F: std::future::Future>(f: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
        f.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    }

    /// Pi funnels key resolution (`file-mutation-queue.ts:34`) AND the queue link (`:42`) through
    /// ONE registration chain (`:33`). Moving `drop(registration)` above `Self::key(..).await` is a
    /// one-line revert that returns same-path ordering to a blocking-pool coin flip with no error
    /// to anyone. This is the assertion standing in its way.
    ///
    /// Deterministic, no wall clock: ONE blocking thread, and it is occupied, so
    /// `tokio::fs::canonicalize`'s job provably cannot run. The first poll of `guard()` therefore
    /// parks inside `Self::key` as a certainty. `#[test]`, not `#[tokio::test]`, because the test
    /// macro cannot express `max_blocking_threads`.
    ///
    /// The chain is process-global, so this test holds it for the length of its own body and any
    /// sibling test calling `guard()` waits. That window is a handful of statements with no sleeps
    /// in it — keep it that way.
    #[test]
    fn the_registration_chain_spans_key_resolution() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let (release, hold) = std::sync::mpsc::channel::<()>();
            let hog = tokio::task::spawn_blocking(move || {
                let _ = hold.recv();
            });

            let locks = FileMutationLocks::new();
            let path = unique_path("registration-span");
            let cancel = CancelToken::new();

            let mut acquiring = std::pin::pin!(locks.guard(&path, &cancel));
            assert!(
                poll_once(acquiring.as_mut()).is_pending(),
                "the only blocking thread is occupied, so `guard()` must park inside `Self::key`"
            );
            assert!(
                registration_is_held(),
                "pi resolves the key INSIDE the registration chain (file-mutation-queue.ts:34). \
                 The chain is not held across key resolution — `drop(registration)` has moved above \
                 `Self::key(..).await`, and two spellings of one path now race on the blocking pool"
            );

            let _ = release.send(());
            hog.await.unwrap();
            let guard = acquiring.await.expect("the lock must be granted");

            // …and released BEFORE the wait (pi `:51`), never held across the mutation itself.
            // Stated as behaviour rather than as `try_lock`, because a sibling test in this binary
            // may legitimately hold the global chain for the length of its own `canonicalize`.
            let other = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                locks.guard(&unique_path("registration-span-other"), &cancel),
            )
            .await
            .expect("the registration chain must not be held across the mutation body")
            .unwrap();
            drop(other);
            drop(guard);
        });
    }

    /// DoD 2/3 — two SPELLINGS of one realpath, `guard()`ed in call order, must be GRANTED in call
    /// order. This is the case `MUTATION_REGISTRATION` exists for: without it the two
    /// `canonicalize` calls race on the blocking pool and the shorter spelling wins the lock even
    /// though it was issued second.
    ///
    /// GREEN is structural, not timed. Task A is driven to its FIRST suspension point — inside
    /// `guard()`, chain held — and only then is B released, which is exactly what
    /// `execute_parallel` does to real tool bodies
    /// (`cyrup-agent/src/agent/run/tools/exec.rs:177-181`). From there the chain's FIFO does the
    /// rest. There is no sleep in this test and none may be added.
    ///
    /// The symlink depth is the RED lever ONLY: it widens the window in which a fix-less build
    /// inverts, it is not what makes the fixed build pass. Linux caps a single path resolution at
    /// 40 link traversals, so 30 hops is the usable ceiling; the rounds compensate for the
    /// remaining probability. To reproduce RED, move `drop(registration)` above
    /// `Self::key(..).await` and run this test — it inverts within a few rounds.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_spellings_of_one_path_are_granted_in_call_order() {
        use std::future::{poll_fn, Future};
        use std::sync::Mutex;
        use std::task::Poll;

        const HOPS: usize = 30;
        const ROUNDS: usize = 24;

        let dir = tempfile::tempdir().unwrap();
        for round in 0..ROUNDS {
            let root = dir.path().join(format!("round-{round}"));
            std::fs::create_dir_all(&root).unwrap();
            let real = root.join("real.txt");
            std::fs::write(&real, b"x").unwrap();
            let mut target = real.clone();
            for hop in 0..HOPS {
                let link = root.join(format!("hop-{hop}"));
                std::os::unix::fs::symlink(&target, &link).unwrap();
                target = link;
            }
            let slow = target;
            let fast = real.clone();

            assert_eq!(
                FileMutationLocks::key(&slow).await.unwrap(),
                FileMutationLocks::key(&fast).await.unwrap(),
                "round {round}: the two spellings must resolve to ONE key or this proves nothing"
            );

            let locks = Arc::new(FileMutationLocks::new());
            let cancel = CancelToken::new();
            let order: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
            let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();

            let a = tokio::spawn({
                let (locks, order, cancel) = (locks.clone(), order.clone(), cancel.clone());
                async move {
                    let mut body = std::pin::pin!(async {
                        let g = locks.guard(&slow, &cancel).await.unwrap();
                        order.lock().unwrap().push("A");
                        drop(g);
                    });
                    // `exec.rs:177-181` verbatim: drive to the first suspension, then hand on.
                    let first = poll_fn(|cx| Poll::Ready(body.as_mut().poll(cx))).await;
                    let _ = started_tx.send(());
                    if first.is_pending() {
                        body.await;
                    }
                }
            });
            let b = tokio::spawn({
                let (locks, order, cancel) = (locks.clone(), order.clone(), cancel.clone());
                async move {
                    let _ = started_rx.await;
                    let g = locks.guard(&fast, &cancel).await.unwrap();
                    order.lock().unwrap().push("B");
                    drop(g);
                }
            });
            a.await.unwrap();
            b.await.unwrap();

            assert_eq!(
                *order.lock().unwrap(),
                vec!["A", "B"],
                "round {round}: the SLOW spelling was dispatched first and must be granted first. \
                 Inverted ⇒ key resolution escaped the registration chain and the two \
                 `canonicalize` calls raced on the blocking pool (pi keeps `:34` inside the chain)"
            );
        }
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
        assert!(a.map.ptr_eq(&b.map));
        assert!(a.map.ptr_eq(&c.map));
        // ...and therefore the same mutex for the same path.
        let via_b = b.map.mutex_for(&key).unwrap();
        let via_c = c.map.mutex_for(&key).unwrap();
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

    /// The dropped-acquisition leak (DoD 6), reached honestly.
    ///
    /// The previous body claimed to drop the acquisition "after inserting its map entry and then
    /// parking on the held mutex" and did neither: `guard()`'s first suspension is
    /// `tokio::fs::canonicalize`, which is BEFORE `enqueue` inserts anything, so the `select!`
    /// dropped a future that had inserted nothing and the final assertion passed vacuously.
    /// `KeyedLocks::enqueue` is reachable from here (`inner` is private to the module, not to its
    /// children) and claims a real place behind `held`.
    ///
    /// DROP ORDER IS THE TEST. `held` goes first, so the entry survives on the forfeited place's
    /// own reference and only `KeyedAcquire::drop` + `PendingEntry::drop` can evict it. Dropping
    /// the acquisition first would let `held`'s own eviction do the work and the test would pass
    /// against a build with `PendingEntry` deleted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_forfeited_queue_place_evicts_its_entry() {
        let locks = FileMutationLocks::new();
        let path = unique_path("drop-evict");
        let key = FileMutationLocks::key(&path).await.unwrap();
        let cancel = CancelToken::new();

        let held = locks.guard(&path, &cancel).await.unwrap();
        assert!(locks.map.contains_key(&key));

        let queued = locks.inner.enqueue(key.clone()).await;

        drop(held);
        assert!(
            locks.map.contains_key(&key),
            "a live waiter must keep its entry — otherwise the assertion below is vacuous"
        );
        drop(queued);
        assert!(
            !locks.map.contains_key(&key),
            "a forfeited place must not leak its entry into a process-global static"
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
