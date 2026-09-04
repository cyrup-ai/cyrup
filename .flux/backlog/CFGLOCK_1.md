---
stage: exec
status: done
updated: 2026-08-23 00:47
---

# CFGLOCK_1 — Promote The Keyed Async Lock Registry Into cyrup-core

**Part 1 of 3** (`CFGLOCK_1` → `CFGLOCK_2` → `CFGLOCK_3`). This one stands alone: pure refactor, no
behaviour change, no public API change. It compiles and passes on its own and is safe to land
independently.

OBJECTIVE: extract the per-key async mutex registry that `cyrup-tools::FileMutationLocks` already
implements into a generic `cyrup-core::keyed_lock`, so `cyrup-config` can reuse the mechanism in
CFGLOCK_2 instead of duplicating ~110 lines of subtle concurrency code.

## Why this is a move, not a rewrite

[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) is a mature, in-production
implementation of exactly the needed primitive. `cyrup-config` cannot depend on `cyrup-tools` —
the dependency runs the other way, and `crates/cyrup-tools/Cargo.toml:51` records that the crate
deliberately "has no runtime dependency beyond cyrup-core". Both already depend on `cyrup-core`,
which owns the shared [`CancelToken`](../../crates/cyrup-core/src/cancel.rs).

Two registry **instances** stay correct — config paths and tool-mutated paths are different key
spaces. Only the mechanism is shared, so the process-global `static` remains each domain's own.

`cyrup-core/src/lib.rs:6` states the crate has "No I/O, no tokio tasks of its own." A keyed mutex
registry honours that — which is precisely why the realpath keying (below) stays behind.

## The five behaviours that must survive the lift

Each is load-bearing and each is easy to drop in a refactor. Sources are
`crates/cyrup-tools/src/lock.rs`.

1. **Guard drop releases *then* evicts** (`:83-92`). `inner.take()` and `lock.take()` run *before*
   the eviction check, so the predicate sees only the map's reference plus genuinely active waiters.
2. **The eviction predicate** (`:89-91`): `map.remove_if(&key, |_, v| Arc::strong_count(v) == 1)`.
   `remove_if` runs the predicate while holding the shard lock, so a concurrent acquirer that has
   just cloned the `Arc` is observed and its entry kept.
3. **`PendingLockEntry`** (`:113-127`) and its **declaration-order** requirement. The entry is
   inserted *before* awaiting the mutex, so both non-guard exits — cancellation, and an outright
   drop of the acquire future — would otherwise leak an entry into a process-global static forever.
   It must be declared **before** the `lock` local so reverse-declaration drop order releases the
   local `Arc` clone first; otherwise `strong_count == 1` always sees this struct's own clone and
   never evicts.
4. **Cancel-aware acquire** racing `CancelToken::cancelled()` against the mutex (`:175-192`).
5. **`biased;` on the `select!`** (`:176`). **Required, not an optimisation.** An unbiased `select!`
   polls ready arms in random order, so when the token is already cancelled *and* the mutex is
   uncontended, both arms are ready on the first poll and the outcome is a coin flip — roughly half
   of all pre-cancelled acquisitions on an idle path returned a guard and let the mutation proceed.
   Keep the arm order (cancel first) and keep the comment explaining it.

## What stays behind in cyrup-tools

- `FileMutationLocks::key` (`:157-165`) — `tokio::fs::canonicalize` is I/O, which `cyrup-core`
  does not do, and the fallback semantics are a pi port.
- `is_missing_path_error` (`:34-49`) — needs `libc::ENOTDIR`; `cyrup-core` has no `libc` dep.
- Every `file-mutation-queue.ts:*` citation and the process-global-map rationale (`:19-29`).
- `ToolError` as the error type — see SUBTASK2's `Cancelled`.

## SUBTASK1 — add `dashmap` to cyrup-core

**Where:** `crates/cyrup-core/Cargo.toml`, `[dependencies]`
**What:** `dashmap = { workspace = true }`. Workspace pins `6.2.1` (root `Cargo.toml:142`);
`cyrup-core` does not currently list it. `tokio` is already there.

## SUBTASK2 — create `crates/cyrup-core/src/keyed_lock.rs`

Write it as below. The map is **handed in**, never created here, so each domain keeps its own
`static`.

```rust
//! A registry of per-key async mutexes, with RAII guards and self-evicting entries.
//!
//! Callers own the map — declare a `static LazyLock<KeyedLockMap<K>>` per lock domain and hand a
//! clone to [`KeyedLocks::new`]. There is deliberately no shared default: two domains that key on
//! unrelated things must not contend.

use crate::CancelToken;
use dashmap::DashMap;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// The map one lock domain is built over.
pub type KeyedLockMap<K> = Arc<DashMap<K, Arc<Mutex<()>>>>;

/// Acquisition was cancelled before the lock was taken. Callers map this into their own error
/// vocabulary — `cyrup-tools` to `error::aborted()`, `cyrup-config` to `ConfigError::Lock`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("lock acquisition cancelled")
    }
}
impl std::error::Error for Cancelled {}

pub struct KeyedLocks<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
}

impl<K: Eq + Hash + Clone> KeyedLocks<K> {
    /// Attach to a caller-owned map. Cheap `Arc` clone, not a fresh map.
    pub fn new(map: KeyedLockMap<K>) -> Self {
        Self { map }
    }

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
            // `biased` is REQUIRED. An unbiased `select!` polls ready arms in RANDOM order, so an
            // already-cancelled token racing an uncontended mutex is a coin flip — half of all
            // pre-cancelled acquisitions would hand back a guard. Polling cancel first makes the
            // outcome deterministic.
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

/// RAII guard. On drop it releases the mutex and evicts the map entry once no other holder or
/// waiter references it, so the map cannot grow without bound.
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
/// A Rust future can be dropped at ANY `.await`, and [`KeyedLocks::guard`] inserts the map entry
/// BEFORE awaiting the mutex. Both non-guard exits therefore leak the entry: the cancel arm of the
/// `select!`, and an outright drop of the `guard()` future itself. The map is typically a
/// process-global `static`, so nothing else ever collects those entries.
///
/// Declared BEFORE the `lock` local in `guard` so drop order (reverse declaration) releases the
/// local `Arc` clone FIRST — otherwise the `strong_count == 1` predicate would always see this
/// struct's own clone and never evict.
struct PendingEntry<K: Eq + Hash + Clone> {
    map: KeyedLockMap<K>,
    key: K,
}

impl<K: Eq + Hash + Clone> Drop for PendingEntry<K> {
    fn drop(&mut self) {
        // Identical predicate to `KeyedGuard::drop`. On the SUCCESS path the returned guard holds
        // a clone, so this is a no-op and the guard's own drop does the eviction.
        self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
    }
}
```

Add `Send + Sync + 'static` to the `K` bound only where the compiler demands it (holding a guard
across `.await` in a spawned task). Do not pre-emptively over-constrain.

## SUBTASK3 — export from cyrup-core

**Where:** `crates/cyrup-core/src/lib.rs`
**What:** `pub mod keyed_lock;` in the module list (alphabetical: after `event_stream`, before
`message`), and `pub use keyed_lock::{Cancelled, KeyedGuard, KeyedLockMap, KeyedLocks};` in the
re-export block, matching the file's existing style.

## SUBTASK4 — reduce `FileMutationLocks` to a wrapper

**Where:** `crates/cyrup-tools/src/lock.rs`

**Keep the `map` field.** This is the crucial detail: the in-file test module reads `locks.map`
directly (`:299`, `:301`, `:322`, and the other eviction tests) and calls the private
`FileMutationLocks::key` (`:295`, `:319`). Keeping both means **zero test edits**, which is the
proof that the extraction is faithful. Do not "simplify" the field away.

```rust
use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};

// Doc comment at :19-29 stays verbatim — it carries the pi citation and the reasoning for a
// process-global map.
static FILE_MUTATION_LOCKS: LazyLock<KeyedLockMap<PathBuf>> =
    LazyLock::new(|| Arc::new(DashMap::new()));

/// Guard type is unchanged for callers; nothing outside this file names it.
pub type MutationGuard = KeyedGuard<PathBuf>;

pub struct FileMutationLocks {
    /// Same map the `inner` registry holds. Kept as a field because the tests in this file read it
    /// directly; that is what lets the extraction be proven with no test changes.
    map: KeyedLockMap<PathBuf>,
    inner: KeyedLocks<PathBuf>,
}

impl FileMutationLocks {
    pub fn new() -> Self {
        let map = Arc::clone(&FILE_MUTATION_LOCKS);
        Self { inner: KeyedLocks::new(Arc::clone(&map)), map }
    }

    // `key` and `is_missing_path_error` stay exactly as they are, docs included.

    pub async fn guard(
        &self,
        path: &Path,
        cancel: &CancelToken,
    ) -> Result<MutationGuard, ToolError> {
        let key = Self::key(path).await?;
        self.inner.guard(key, cancel).await.map_err(|_| error::aborted())
    }
}
```

`impl Default` (`:63-70`) stays as-is — including its comment about why it is hand-written.

**Delete from this file** (now living in cyrup-core): the `MutationGuard` struct and its `Drop`,
`PendingLockEntry` and its `Drop`. Move their doc comments to cyrup-core per SUBTASK2 rather than
discarding them.

**Public API must not change.** External callers are `registry.rs:56`, `tools/write.rs:20,30,102`,
`tools/edit.rs:30,40,223`, the `crate::FileMutationLocks` re-export at `lib.rs:43`, and eight test
modules — all use only `FileMutationLocks::new()` and `.guard(&path, &cancel).await`. None may need
editing.

## Definition of done

- [ ] `cyrup-core::keyed_lock` exists with `KeyedLocks`, `KeyedGuard`, `KeyedLockMap`, `Cancelled`
- [ ] All five behaviours above are present, including `biased;` with its comment and
      `PendingEntry`'s declaration-order comment
- [ ] `dashmap` added to `cyrup-core/Cargo.toml`; `keyed_lock` exported from `lib.rs`
- [ ] `FileMutationLocks` delegates, keeps its `map` field, its `key`/`is_missing_path_error`, and
      all its existing doc comments
- [ ] **Zero files changed outside `crates/cyrup-core/{Cargo.toml,src/lib.rs,src/keyed_lock.rs}` and
      `crates/cyrup-tools/src/lock.rs`** — including zero test files
- [ ] `cargo check -p cyrup-core -p cyrup-tools --all-targets` clean
- [ ] `cargo test -p cyrup-tools` passes, with these eight unchanged tests green: `same_path_serializes`,
      `independent_handles_share_one_lock_per_path`, `guard_evicts_its_entry_on_drop`,
      `a_cancelled_acquisition_evicts_its_entry_instead_of_leaking_it`,
      `dropping_the_acquisition_future_evicts_its_entry`, `missing_path_falls_back_to_the_resolved_path`,
      `non_missing_realpath_failure_propagates_instead_of_being_swallowed`, `distinct_paths_do_not_serialize`
- [ ] `cargo test -p cyrup-tools cross_registry_mutation_lock` passes
- [ ] `cargo clippy -p cyrup-core -p cyrup-tools --all-targets` adds no new warnings

## Research notes

- Mechanism to lift: `crates/cyrup-tools/src/lock.rs:18-192`; regression tests at `:207-467`
- `CancelToken`: `crates/cyrup-core/src/cancel.rs`
- `cyrup-core` carries `#![forbid(unsafe_code)]` (`lib.rs:7`) — nothing here needs `unsafe`
- Do **not** run `cargo fmt` in this workspace: no crate is rustfmt-clean at HEAD, so it reformats
  whole packages and buries the diff

## No tests

Tests are in scope (the earlier "another team owns tests" line was wrong). Add a test for any behaviour this task changes; the eight existing tests are the
regression net and must pass **unmodified** — if one stops compiling, the extraction changed an API
it should not have. Fix the extraction, never the test.

## No benchmarks

No benchmarks: this task is not performance-scoped.
