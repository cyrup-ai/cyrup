---
stage: split
status: complete
updated: 2026-08-23 00:47
---

# Config File Locks Hang Forever Under Contention Instead Of Failing

## Description

`cyrup-config`'s `FileLock` blocks indefinitely when another process holds the lock, and on three
of its eight call sites it does so **on a tokio worker thread**. A second cyrup process touching
the same settings/auth/models/trust file does not error — it freezes, with no output and no
timeout, until the other process exits.

[`crates/cyrup-config/src/lock.rs:31`](../../crates/cyrup-config/src/lock.rs):

```rust
FileExt::lock(&file).map_err(|_| ConfigError::Lock { path: lock_path })?;
```

`fs4` 1.1.0 documents `FileExt::lock` as "Acquires an exclusive lock on the file, **blocking until
the lock can be acquired**" (`fs4-1.1.0/src/lib.rs:294-303`). So the `map_err` almost never fires:
`ConfigError::Lock { path }` is an ELOCKED-shaped variant guarding a call that cannot return
`WouldBlock`.

Upstream (pi-mono v0.83.0, cloned at `tmp/pi`) bounds every wait. All three of its lock sites use
`proper-lockfile@4.1.2` and throw on exhaustion: `settings-manager.ts:199-224` (10 × 20 ms),
`auth-storage.ts:111-124` (`retries: 10, factor: 2, minTimeout: 100, maxTimeout: 10000,
randomize: true`), `trust-manager.ts:138-165` (retry loop → hard error).

**Do not port that retry loop.** proper-lockfile polls because its lock is a `mkdir` with no
readiness signal and JS has no better primitive. Rust does. The required design below gets the same
guarantee — bounded wait, clear error, never a hang — with no polling and no sleeping at all.

## Business Requirements (Clarified)

### Core Behavior

- **Two layers.** A per-path **async mutex** (in-process) in front of **flock** (cross-process).
- **In-process contention never reaches a syscall.** Two tasks in one cyrup process contending for
  `settings.json` queue on the async mutex: fair, FIFO, cancel-aware, zero polling. This is where
  virtually all real contention lives — one agent process, many sessions and tool calls.
- **Cross-process contention blocks in the kernel.** Because layer 1 admits at most one task per
  path per process, the flock wait has exactly one waiter and can use the *blocking*
  `FileExt::lock` on `tokio::task::spawn_blocking`. The kernel wakes it the instant the peer
  releases — strictly better latency than upstream's 20 ms poll granularity.
- **Bounded by cancellation, not by a retry count.** Acquisition takes a `CancelToken` and races it
  against the lock, exactly as `FileMutationLocks::guard` already does. Exhausting a deadline or
  observing cancellation yields `ConfigError::Lock { path }`.
- **All eight call sites become async.** One lock API, async throughout.

### Edge Cases & Error Handling

- A timed-out / cancelled `spawn_blocking` cannot un-park the thread already inside `flock(2)`.
  That thread unblocks when the peer releases and immediately drops the guard. Layer 1 caps this at
  **one parked blocking thread per contended path per process**, which is acceptable; do not try to
  cancel it.
- The async mutex must be released on **every** exit including future-drop. `FileMutationLocks`
  already solves this with `PendingLockEntry` (`cyrup-tools/src/lock.rs:113-127`) — a Rust future
  can be dropped at any `.await`, where pi's `finally` always runs. Carry that mechanism over
  verbatim; it is the single subtlest part of the pattern.
- Map entries must be evicted when the last holder drops, or the process-global map grows without
  bound. Reuse the `Arc::strong_count(v) == 1` predicate under `remove_if`
  (`cyrup-tools/src/lock.rs:84-91`), including its drop-order requirement.

### Constraints & Boundaries — OUT of scope, with reasoning

- **Do not delete the `<path>.lock` sidecar on release.** Upstream's `.lock` is a *directory*
  created by `mkdir` and removed by `rmdir` (`proper-lockfile/lib/lockfile.js:28-29`, `:88-90`) —
  for that primitive removal *is* the release. cyrup locks a regular file with `flock`, where
  unlinking is the classic advisory-lock race: unlink, recreate, and two processes hold locks on
  different inodes at the same path. Persisting sidecars are correct here. Put this reasoning in a
  comment on `Drop` so nobody "finishes" it later.
- **Do not port `stale: 30000` or `onCompromised`.** They exist because a crashed process leaves a
  `mkdir` lock behind forever. The kernel releases `flock` when the fd closes or the process dies,
  so neither has anything to detect.
- **Do not add a retry/backoff/jitter policy.** Superseded by the design above.

## Implementation Research

### Existing code to leverage

[`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) is this exact mechanism,
already written, documented and in production: a process-global
`LazyLock<Arc<DashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>>`, realpath-keyed, with an RAII
`MutationGuard`, entry eviction on drop, `PendingLockEntry` for the future-drop gap, and
cancel-aware acquisition. **Do not reimplement it and do not copy it.**

`cyrup-config` cannot depend on `cyrup-tools` — dependency direction runs the other way, and
`cyrup-tools/Cargo.toml:51` records that it deliberately "has no runtime dependency beyond
cyrup-core". So: **promote the mechanism into `cyrup-core`**, which both crates already depend on
and which already owns the shared concurrency primitive
[`cyrup-core/src/cancel.rs`](../../crates/cyrup-core/src/cancel.rs).

The two lock domains stay separate — config paths and tool-mutated paths are different key spaces,
so two registry *instances* are correct. What must not be duplicated is the *mechanism*.

### Files that need changes

| File | Change |
| --- | --- |
| `crates/cyrup-core/src/keyed_lock.rs` *(new)* | Generic `KeyedLocks` + `KeyedGuard`, lifted from `cyrup-tools/src/lock.rs:18-127` with the pi-specific realpath keying and doc citations left behind |
| `crates/cyrup-core/src/lib.rs` | `pub mod keyed_lock;` + re-export |
| `crates/cyrup-core/Cargo.toml` | add `dashmap` (workspace dep; not currently present) |
| `crates/cyrup-tools/src/lock.rs` | `FileMutationLocks` becomes a thin wrapper over `KeyedLocks`, keeping its realpath keying and its doc comments |
| `crates/cyrup-config/src/lock.rs` | `FileLock::acquire` → `async fn acquire(target, cancel)`; two-layer body; `Drop` comment on why the sidecar persists |
| `crates/cyrup-config/src/error.rs:54` | `ConfigError::Lock { path }` — folded in from `CONFIG_ERROR_VARIANT_ACCURACY.md` per that decision; confirm the message names the *target* file, not the `.lock` sidecar, since the sidecar is an implementation detail the operator never edits |
| `crates/cyrup-config/src/models_store.rs:238,299,317` | `read_latest` → async (no external callers); `write`/`delete` already async |
| `crates/cyrup-config/src/settings/store.rs:64` | `with_lock` → async |
| `crates/cyrup-config/src/settings/manager.rs:232,292,338,426` | `set`, `set_nested`, `persist_nested`, `set_enable_analytics` → async |
| `crates/cyrup-config/src/auth.rs:272` | already in `pub async fn modify`; drop the now-redundant `provider_lock` if `KeyedLocks` subsumes it |
| `crates/cyrup-config/src/trust.rs:150,174` | `nearest`, `set_many` → async |

### Measured downstream ripple

Making the five sync sites async **leaves cyrup-config and changes sync public API in two other
crates**. This was measured, not assumed:

| Caller | Enclosing fn | Note |
| --- | --- | --- |
| `cyrup/src/subcommands.rs:441` | `fn saved_trusted(dirs) -> bool` | sync, returns a plain bool |
| `cyrup/src/startup_ui.rs:391` | `fn persist_trust_choice(...)` | sync |
| `cyrup-session-svc/src/session/accessors.rs:105` | `pub fn saved_trust_decision(&self) -> Option<…>` | sync public API |
| `cyrup-session-svc/src/session/accessors.rs:121` | `pub fn write_project_trust(…)` | sync public API |
| `cyrup-session-svc/src/builder.rs:623` | `pub async fn build(…)` | already async, but uses `.and_then(\|store\| store.nearest(&cwd).ok().flatten())` — a sync combinator chain that must be restructured |
| `cyrup-config/src/settings/manager.rs` ×4 | `pub fn set*` | in-crate, but each has its own downstream callers to chase |

`models_store::read_latest` has **no** external callers — zero ripple there.

### Code patterns to follow

- Registry + guard + eviction: `cyrup-tools/src/lock.rs:18-127`.
- Cancel-aware acquire racing a `CancelToken`: same file, `FileMutationLocks::guard`.
- Do **not** invent a jitter/rng helper — nothing in the new design sleeps.

## Implementation Plan

### Step 1 — `cyrup-core::keyed_lock`

Lift the mechanism out of `cyrup-tools/src/lock.rs` into a generic, citation-free
`KeyedLocks<K>`/`KeyedGuard<K>`: process-global map is the *caller's* choice (a `LazyLock` static
they own), so `cyrup-tools` and `cyrup-config` each keep their own domain. Preserve the eviction
predicate, the `PendingLockEntry` drop-gap handling and its declaration-order requirement, and the
cancel-aware acquire. Add `dashmap` to `cyrup-core/Cargo.toml`.

### Step 2 — rewrite `cyrup-config::FileLock`

```rust
pub struct FileLock {
    _in_process: KeyedGuard<PathBuf>, // layer 1 — released on drop, before the flock
    file: File,                       // layer 2 — flock released by Drop below
}

impl FileLock {
    pub async fn acquire(target: &Path, cancel: &CancelToken) -> Result<Self, ConfigError> {
        let lock_path = lock_path_for(target);
        // Layer 1: in-process. Fair, async, cancel-aware, no syscall.
        let in_process = CONFIG_LOCKS.guard(lock_path.clone(), cancel).await?;
        // Layer 2: cross-process. Exactly one waiter per path per process, so the blocking
        // flock belongs on the blocking pool — the kernel wakes it the moment the peer releases.
        let file = tokio::task::spawn_blocking(move || open_and_lock(&lock_path))
            .await
            .map_err(|_| ConfigError::Lock { path: /* … */ })??;
        Ok(Self { _in_process: in_process, file })
    }
}
```

Field order matters: `_in_process` is declared first so reverse-declaration drop releases the flock
before the in-process mutex, keeping a same-process successor from waking into a still-held flock.

`Drop` keeps `FileExt::unlock` and gains the comment on why the sidecar is never unlinked.

### Step 3 — make the eight call sites async, then chase the ripple

Work outward in the order of the table above: `cyrup-config` internals, then
`settings/manager.rs`'s public setters, then `cyrup-session-svc`, then `cyrup`. Restructure
`builder.rs:623`'s `.and_then(…)` chain rather than forcing a block-on.

### Step 4 — fold in the `ConfigError::Lock` work

Per the decision on overlap, this task owns the variant end to end. Whoever executes must also
delete the `ConfigError::Lock` section from `CONFIG_ERROR_VARIANT_ACCURACY.md` so the two do not
collide.

### Definition of Done

- [ ] `cyrup-core::keyed_lock` exists; `cyrup-tools::FileMutationLocks` is a wrapper over it with its behaviour and docs intact
- [ ] `cyrup-config::FileLock::acquire` is async, takes a `CancelToken`, and holds the in-process guard for the flock's whole lifetime
- [ ] No `std::thread::sleep`, no retry counter, no jitter anywhere in the lock path
- [ ] The blocking flock runs only under `spawn_blocking`, never on a runtime worker
- [ ] `ConfigError::Lock { path }` is genuinely reachable and names the target file, not the sidecar
- [ ] All eight call sites and every downstream caller in `cyrup` / `cyrup-session-svc` compile async-clean; no `block_on` introduced
- [ ] The `.lock` sidecar is still never unlinked, with the reasoning in a `Drop` comment
- [ ] Two concurrent processes contending for one config file: the loser gets an error or cancels cleanly; neither hangs
- [ ] `cargo check --workspace --all-targets` and `cargo test -p cyrup-core -p cyrup-tools -p cyrup-config` pass
- [ ] The `ConfigError::Lock` section is removed from `CONFIG_ERROR_VARIANT_ACCURACY.md`

## Recommended next step

This is no longer one session. Step 1 (a new cyrup-core primitive + refactoring a battle-tested
cyrup-tools type), Step 2 (the lock rewrite) and Step 3 (an async ripple across three crates and
several sync public APIs) are independently reviewable and independently riskable. Run `/split`
before `/exec`.
