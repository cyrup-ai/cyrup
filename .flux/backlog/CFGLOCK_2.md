---
stage: exec
status: done
updated: 2026-08-23 01:34
---

# CFGLOCK_2 — Two-Layer FileLock In cyrup-config, And Make Its Call Sites Async

**Part 2 of 3.** Requires `CFGLOCK_1` — landed as `65b032b`, so `cyrup_core::keyed_lock` with
`KeyedLocks`, `KeyedGuard`, `KeyedLockMap` and `Cancelled` is available now.

**This task ends with the workspace not building.** Turning `cyrup-config`'s public API async breaks
`cyrup` and `cyrup-session-svc`; `CFGLOCK_3` repairs them. Scope here ends at
`cargo check -p cyrup-config --all-targets`. Run `CFGLOCK_3` immediately after, same branch, before
pushing.

OBJECTIVE: replace the indefinitely-blocking `flock` with a per-path async mutex in front of the
cross-process flock, so config contention is bounded, cancel-aware, and never parks a tokio worker.

## The defect

[`crates/cyrup-config/src/lock.rs:31`](../../crates/cyrup-config/src/lock.rs):

```rust
FileExt::lock(&file).map_err(|_| ConfigError::Lock { path: lock_path })?;
```

`fs4` 1.1.0 documents `FileExt::lock` as "Acquires an exclusive lock on the file, **blocking until
the lock can be acquired**" (`fs4-1.1.0/src/lib.rs:294-303`). A second cyrup process touching the
same config file freezes with no output and no timeout. Three call sites are inside `async fn`, so
today they park a tokio worker — `auth.rs:272` does it while also holding an async mutex. And that
`map_err` is nearly unreachable: it guards a call that cannot return `WouldBlock`.

`models_store.rs:295` already documents the consequence: "under this crate's `FileLock` an abort
after acquisition would additionally block every other process for the duration of a write nobody
wants."

## The design (settled — do not re-litigate)

Upstream polls (`proper-lockfile@4.1.2`) because a `mkdir` lock has no readiness signal and JS has
no better primitive. **Do not port that retry loop.** Instead:

- **Layer 1, in-process:** a per-path async mutex from `cyrup_core::keyed_lock`. Fair, cancel-aware,
  zero syscalls, zero polling. This is where virtually all real contention lives.
- **Layer 2, cross-process:** layer 1 admits at most one task per path per process, so the flock
  wait has exactly one waiter and the *blocking* `FileExt::lock` belongs on `spawn_blocking`. The
  kernel wakes it the instant the peer releases — better latency than upstream's 20 ms polling.
- **Bounded by cancellation,** not a retry count.

## Where the `CancelToken` comes from — researched, and it is not uniform

`KeyedLocks::guard` needs a `&CancelToken`. Only **one** of the four config stores carries one
today, and that matches upstream's own placement:

| Store | Token available? | Upstream |
| --- | --- | --- |
| `models_store` | **Yes** — `ModelsStoreOperationOptions { signal: Option<CancelToken> }` (`cyrup-provider/src/models_store.rs:64-69`), already threaded through `read`/`write`/`delete` and checked by `throw_if_aborted` before the lock | pi passes `options` into `withLockAsync` (`models-store.ts:132`) |
| `auth` | No — `modify(provider, f)` takes no signal | pi's `withLockAsync(fn)` takes none either |
| `settings` | No | pi uses `lockSync`, no signal |
| `trust` | No | pi uses `lockSync`, no signal |

So **do not invent token parameters** for settings/trust/auth. `acquire` takes
`Option<&CancelToken>`; pass `Some` only where the crate already has one. `CancelToken` is
`tokio_util::sync::CancellationToken` (`cyrup-core/src/cancel.rs:9`), whose `cancelled()` on a
never-cancelled token is a future that never resolves — so a process-static uncancelled token is a
correct, zero-cost stand-in for `None`.

## SUBTASK1 — rewrite `FileLock` as two layers

**Where:** `crates/cyrup-config/src/lock.rs`

```rust
use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};
use cyrup_core::CancelToken;
use dashmap::DashMap;                 // add `dashmap = { workspace = true }` to Cargo.toml
use std::sync::LazyLock;

/// This crate's own lock domain, separate from `cyrup-tools`' file-mutation map: config paths and
/// tool-mutated paths are different key spaces, so two instances are correct even though the
/// mechanism is shared.
static CONFIG_LOCKS: LazyLock<KeyedLockMap<PathBuf>> = LazyLock::new(|| Arc::new(DashMap::new()));

/// Stand-in for callers with no cancellation of their own. `CancellationToken::cancelled()` on a
/// token nobody cancels never resolves, so the `select!` inside `KeyedLocks::guard` simply always
/// takes the lock arm — no allocation per acquire, no polling.
static NEVER_CANCELLED: LazyLock<CancelToken> = LazyLock::new(CancelToken::new);

pub struct FileLock {
    /// Layer 1. Declared FIRST so reverse-declaration drop order releases the flock BEFORE this —
    /// otherwise a same-process successor wakes out of layer 1 into a still-held flock and pays a
    /// pointless trip to the kernel.
    _in_process: KeyedGuard<PathBuf>,
    /// Layer 2, released by `Drop` below.
    file: File,
}

impl FileLock {
    pub async fn acquire(
        target: &Path,
        cancel: Option<&CancelToken>,
    ) -> Result<Self, ConfigError> {
        let lock_path = lock_path_for(target);
        let token = cancel.unwrap_or(&NEVER_CANCELLED);
        let in_process = CONFIG_LOCKS_HANDLE
            .guard(lock_path.clone(), token)
            .await
            .map_err(|_| ConfigError::Cancelled)?;
        let target_owned = target.to_path_buf();
        let file = tokio::task::spawn_blocking(move || open_and_lock(&target_owned, &lock_path))
            .await
            .map_err(|_| ConfigError::Lock { path: target.to_path_buf() })??;
        Ok(Self { _in_process: in_process, file })
    }
}

/// Everything that blocks: `ensure_dir`, `open`, and the blocking `flock`. Unchanged behaviour,
/// relocated onto the blocking pool.
fn open_and_lock(target: &Path, lock_path: &Path) -> Result<File, ConfigError> {
    if let Some(parent) = target.parent() {
        ensure_dir(parent)?;
    }
    let file = OpenOptions::new()
        .create(true).read(true).write(true).truncate(false)
        .open(lock_path)
        .map_err(|e| io_err(lock_path, e))?;
    FileExt::lock(&file).map_err(|_| ConfigError::Lock { path: target.to_path_buf() })?;
    Ok(file)
}
```

Build the `KeyedLocks<PathBuf>` handle once (a second `LazyLock` over
`KeyedLocks::new(Arc::clone(&CONFIG_LOCKS))`, or construct per call — it is an `Arc` clone either
way). Name it consistently with whichever you choose.

**Two error mappings, both using variants that already exist** in `error.rs`:

- layer-1 cancellation → `ConfigError::Cancelled` (`error.rs:55-56`, `#[error("cancelled")]`)
- flock failure or `spawn_blocking` join failure → `ConfigError::Lock { path }`

Note the `Lock` path argument changes from the `.lock` sidecar to the **target** file: the sidecar
is an implementation detail no operator opens. Keep the message *format* exactly as it is — see
"Relationship to CONFIG_ERROR_VARIANT_ACCURACY" below for why that matters.

## SUBTASK2 — `Drop` keeps unlocking, and gains the sidecar comment

Keep `FileExt::unlock`. Add a comment recording **why the `<path>.lock` sidecar is never unlinked**:
upstream's `.lock` is a *directory* created by `mkdir` and removed by `rmdir`
([`tmp/pi`](../../tmp/pi) → `proper-lockfile/lib/lockfile.js:28-29`, `:88-90`) — for that primitive
removal *is* the release. cyrup locks a regular file with `flock`, where unlinking is the classic
advisory-lock race: unlink, recreate, and two processes hold locks on different inodes at the same
path. A previous task proposed deleting the `FileLock::path` field that would have enabled this
cleanup; without the comment someone re-adds it to "finish" the job.

Do **not** port `stale: 30000` or `onCompromised` — they detect locks left by crashed processes,
which the kernel handles for `flock`.

## SUBTASK3 — make the eight call sites async

| Where | Change | Token |
| --- | --- | --- |
| `models_store.rs:238` `read_latest` | → `async fn`, take `Option<&ModelsStoreOperationOptions>` | thread from `read` (`:267-275`), which already has it |
| `models_store.rs:299` `write` | already async — `.await` | `options` |
| `models_store.rs:317` `delete` | already async — `.await` | `options` |
| `settings/store.rs:64` `with_lock` | → `async fn`; its `f: &mut dyn FnMut(Option<&str>) -> Option<String>` stays sync | `None` |
| `settings/manager.rs:232,292,338,426` | `set`, `set_nested`, `persist_nested`, `set_enable_analytics` → `async fn` | `None` |
| `trust.rs:150` `nearest`, `:174` `set_many` | → `async fn` | `None` |
| `auth.rs:272` | already inside `pub async fn modify` — `.await` | `None` |

For the `models_store` sites pass `options.and_then(|o| o.signal.as_ref())`. The existing
`throw_if_aborted(options)?` calls before each acquire stay exactly where they are — they are the
port of pi's pre-lock abort check and are not superseded by layer 1's cancel race.

## SUBTASK4 — decide the `auth.rs` `provider_lock` question

**Where:** `crates/cyrup-config/src/auth.rs:269-272`. `modify` takes
`self.provider_lock(provider).lock().await` immediately before the `FileLock`.

The keys **do not coincide**: `provider_lock` is per-`ProviderId`, layer 1 is per-file, and all
providers share one auth file. So layer 1 is the *coarser* lock and subsumes `provider_lock`'s
mutual exclusion entirely — but only if it is taken first. **Keep both**, and add a one-line comment
stating that the per-provider mutex is now the finer-grained inner lock and that the acquisition
order (provider mutex, then file lock) must not be inverted, since two locks taken in opposite
orders by different callers is the textbook deadlock. Do not remove it without evidence.

## Relationship to CONFIG_ERROR_VARIANT_ACCURACY.md — corrected

The pre-augmentation draft said to delete a "`ConfigError::Lock` section" from that task. **There is
no such section, and deleting anything there would be wrong.** Its four sections are: models_store
dropping writes, `auth.rs` mislabelling I/O as `AuthError::Lock`, `ConfigError::Trust` as a
catch-all, and `ConfigError::Io` not naming its path. Its only mention of `Lock` is at line 70,
where it holds `#[error("lock contention on {path}")] Lock { path: PathBuf }` up as **the exemplar**
the `Io` variant should imitate.

So: **leave that file alone**, and keep `Lock`'s message format byte-identical. Changing only which
path is passed is compatible with — indeed reinforces — what that task is asking for.

Two of its observations are already stale, worth knowing but not to be fixed here: it cites
`FileLock::acquire(&self.path).ok()` at `models_store.rs:226/:287/:305`, but those sites are now
`?` and `.map_err(store_err)?` at `:238/:299/:317`.

## Expected test churn — mechanical, but not small

Making the setters async forces the tests that drive them to become async. Measured:

| File | Call sites needing `.await` | Sync `#[test]` fns needing `#[tokio::test]` |
| --- | --- | --- |
| `src/settings/tests/write_refusal.rs` | 21 | 13 |
| `src/settings/tests/merge_and_scope.rs` | 10 | 16 |
| `src/settings/tests/getters.rs` | 5 | — |
| `src/trust.rs` (in-file tests) | 6 | — |

Neither settings test file uses `#[tokio::test]` today. This churn is in scope **only** as the
mechanical `.await` + attribute change; if any test needs a logic change to pass, stop and flag it
rather than rewriting it.

## Definition of done

- [ ] `FileLock::acquire` is `async`, takes `Option<&CancelToken>`, and holds the layer-1 guard for
      the flock's entire lifetime
- [ ] `_in_process` is declared before `file`, with the comment explaining the drop order
- [ ] The blocking `FileExt::lock` and `ensure_dir` run only inside `spawn_blocking`
- [ ] No `std::thread::sleep`, retry counter, backoff or jitter anywhere in the lock path
- [ ] Layer-1 cancellation maps to `ConfigError::Cancelled`; flock/join failure to
      `ConfigError::Lock { path }` naming the **target**, with the message format unchanged
- [ ] `dashmap` added to `crates/cyrup-config/Cargo.toml`
- [ ] The sidecar is still never unlinked, reasoning in a `Drop` comment
- [ ] All eight call sites `.await`; `models_store`'s three pass the real `options` signal and keep
      their existing `throw_if_aborted` calls
- [ ] `provider_lock` kept, with the lock-ordering comment
- [ ] `CONFIG_ERROR_VARIANT_ACCURACY.md` is **not** modified
- [ ] `cargo check -p cyrup-config --all-targets` clean
- [ ] `cargo test -p cyrup-config` passes
- [ ] `cargo clippy -p cyrup-config --all-targets` adds no new warnings

`cargo check --workspace` will fail at the end of this task. Expected — `CFGLOCK_3` fixes it.

## Research notes

- Layer-1 primitive: `crates/cyrup-core/src/keyed_lock.rs` (landed in `65b032b`)
- `CancelToken` is `tokio_util::sync::CancellationToken` — `cyrup-core/src/cancel.rs:9`
- `ModelsStoreOperationOptions`: `crates/cyrup-provider/src/models_store.rs:64-74`
- `ConfigError`: `crates/cyrup-config/src/error.rs` — `Lock { path }` at `:54`, `Cancelled` at `:55`
- Upstream clone: `tmp/pi` at `v0.83.0`;
  `packages/coding-agent/src/core/{settings-manager,auth-storage,trust-manager}.ts`
- Do **not** run `cargo fmt`: no crate in this workspace is rustfmt-clean at HEAD, so it reformats
  whole packages and buries the diff

## No tests

Another team owns tests. **Do not write any new test code.** The `.await` and `#[tokio::test]`
migration quantified above is mechanical and in scope; anything beyond mechanical is not — flag it.

## No benchmarks

Another team owns benchmarks. **Do not write any benchmark code.**
