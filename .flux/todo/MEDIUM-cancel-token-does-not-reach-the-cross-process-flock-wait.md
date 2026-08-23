---
title: Cancel Token Does Not Reach The Cross Process Flock Wait
priority: MEDIUM
stage: aug
status: done
updated: 2026-08-23 03:36
---

> ## ⚠️ QUEUE RESOLUTION — READ BEFORE EXECUTING
>
> **This spec is VOID as written. Do NOT implement the design below.** It is superseded by
> [`HIGH-dropped-acquire-future-detaches-blocking-flock-task.md`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md),
> which must be executed **first**, and which satisfies this finding in full.
>
> This file's own "Relationship to HIGH" section pre-declared the condition: *"If the HIGH task is
> instead executed with its primary (polling) suggestion, this spec is void."* That is what
> happened. The condition fired; this is the recorded outcome, not a new decision overriding the
> author.
>
> **Why the HIGH's design won.** Both specs agree this is one defect with one fix and differ only
> on which. This file keeps the blocking `flock` and moves the `KeyedGuard` into the closure; the
> HIGH replaces the blocking `flock` with a non-blocking attempt retried from async land. The
> deciding fact is tokio's own documentation of `spawn_blocking`
> (`tokio-1.52.3/src/task/blocking.rs:106-120`), quoted verbatim:
>
> > *"runtime shutdown will wait indefinitely for all started `spawn_blocking` to finish running."*
>
> Under this file's design, a contended acquire whose caller has gone away leaves a pool thread
> parked inside `flock(2)`, so **process exit hangs** until a foreign peer releases the lock — a
> Ctrl-C that never returns. No amount of guard placement fixes that, because a started
> `spawn_blocking` task cannot be aborted at all.
>
> **On the branch spec's anti-polling rule.** [`CONFIG_LOCK_CONTENTION.md`](../done/2026-08-23-00-08/CONFIG_LOCK_CONTENTION.md)
> rejected polling because *"proper-lockfile polls because its lock is a `mkdir` with no readiness
> signal and JS has no better primitive. Rust does."* For `flock(2)` that premise is false: there is
> no timeout variant, no epoll/`AsyncFd` readiness, no io_uring lock opcode, and no inotify on
> release. Rust has no better primitive here either. The rule stands everywhere it was true; it does
> not bind this syscall.
>
> **What remains for this task.** After the HIGH lands, this becomes a **verification-only** close:
> confirm (1) the `CancelToken` is consumed by the `biased select!` inside `acquire`,
> (2) `ConfigError::Cancelled` is reachable from layer-2 contention, and (3) `acquire`'s doc states
> which layers the token governs — this file's explicit residual ask. Change nothing if all three
> hold. **Do not** clone the token into a blocking closure: with the wait in async land there is no
> long-lived closure to hold it.

# Make the token bound both layers of `FileLock::acquire`, by racing it against the layer-2 task and moving layer 1 into that task

## Verdict

The current state is **not** merely mis-documented, and the fix is **not** the poll loop the finding
suggests.

`cancel` today governs only the wait that is already short. That is not a documentation defect,
because there is no honest sentence that makes the parameter worth having: layer 1 is held for the
duration of one local read-modify-write, so "this token can shorten a sub-millisecond wait" is a
description of a no-op. The parameter exists because `models_store` threads
`ModelsStoreOperationOptions::signal` into it so an aborted operation stops waiting; the only wait
it can be waiting on is layer 2.

But the finding's own remedy — poll `try_lock` on a backoff inside the blocking closure — reverses a
decision that was taken deliberately and recorded
([`CONFIG_LOCK_CONTENTION.md`](../done/2026-08-23-00-08/CONFIG_LOCK_CONTENTION.md)), and it is not
needed to get the property it is after. There is a smaller change that bounds *the caller's* wait at
both layers, keeps the kernel wake, introduces no sleeping and no tunables, and — as a side effect —
turns the unbounded blocking-pool accumulation described in the sibling HIGH finding into a hard cap
of one thread per contended path.

**Required path: race the token against the layer-2 join, and move the layer-1 `KeyedGuard` into the
blocking closure so an abandoned acquisition carries layer 1 with it.**

## What is actually true today

[`crates/cyrup-config/src/lock.rs:57-69`](../../crates/cyrup-config/src/lock.rs) — the token is
consumed by [`KeyedLocks::guard`](../../crates/cyrup-core/src/keyed_lock.rs) (`:54-77`) and then
never referenced again; `spawn_blocking(open_and_lock)` is awaited unconditionally.

Two corrections to the finding as filed, both of which narrow it and neither of which dissolves it:

1. **Layer 1 is not always the fast layer.** A local task that reaches `flock` holds layer 1 for the
   whole cross-process wait, so under peer contention a queue forms *at layer 1* — and that queue
   **is** cancel-aware. So the 2nd..Nth local waiter can already abandon. What cannot abandon is the
   one task per path that has reached `open_and_lock`. The defect is real but is exactly one task
   wide per path.
2. **The suggested code would not compile.** `fs4` 1.1.0 has no `FileExt::try_lock_exclusive`; the
   non-blocking exclusive form is `FileExt::try_lock`, and it returns
   `Result<(), fs4::TryLockError>` (`TryLockError::WouldBlock`), not an `io::Error` whose `kind()`
   is `WouldBlock` (`fs4-1.1.0/src/lib.rs:303`, `:318`, `src/try_lock_error.rs:10-16`). The same
   mistake appears in the sibling HIGH finding's sketch. Anyone reaching for the polling design must
   know it costs a new error-shape conversion as well as the sleep.

## Relationship to [`HIGH-dropped-acquire-future-detaches-blocking-flock-task`](./HIGH-dropped-acquire-future-detaches-blocking-flock-task.md)

**These are one defect with one fix, seen from two sides.** Both ask: *what governs the inside of
`FileLock::acquire` when the caller stops waiting?* This task is the explicit-token case; the HIGH
task is the dropped-future case. Rust makes them the same event — a cancelled caller drops the
`acquire` future — so two local patches would be two mechanisms for one transition.

The change specified below is the HIGH task's **stated alternative** ("keep the blocking `flock` but
hold layer 1 for the whole detached lifetime … move the `KeyedGuard` into the blocking closure"),
plus the cancel race this task needs. It resolves both:

| HIGH task's consequence | After this change |
| --- | --- |
| (a) process holds an *unowned* cross-process lock | Narrowed to the interval between the kernel granting the lock and tokio dropping the detached output — a handoff, not a wait. Never a *wait* undertaken on behalf of a dead caller that another live caller is blocked behind. |
| (b) "exactly one waiter per path per process" violated | **Restored unconditionally.** The zombie still holds layer 1, so no successor can start a second `open_and_lock` for that path. |
| pinned blocking-pool threads, unbounded | Hard cap of **one per contended path**, because of (b). Pool exhaustion is no longer reachable. |

Whoever executes first should implement the whole thing and mark the other task as subsumed. If the
HIGH task is instead executed with its *primary* (polling) suggestion, this spec is void and the two
must be reconciled before either lands — do not apply both.

## Why not polling (against the recorded rationale)

[`CONFIG_LOCK_CONTENTION.md`](../done/2026-08-23-00-08/CONFIG_LOCK_CONTENTION.md) rejected polling on
two grounds and accepted one cost:

> **Do not port that retry loop.** proper-lockfile polls because its lock is a `mkdir` with no
> readiness signal and JS has no better primitive. Rust does.

> A timed-out / cancelled `spawn_blocking` cannot un-park the thread already inside `flock(2)`. …
> Layer 1 caps this at **one parked blocking thread per contended path per process**, which is
> acceptable; do not try to cancel it.

The second quote is the design this branch was supposed to build and did not: it presumes the async
side *races the token* and lets the thread finish on its own. The branch implemented the parked
thread without the race. So the corrective is to finish the recorded design, not to overturn it.

The honest case *for* polling is that it retires the parked thread outright. Weigh it against what
it costs:

- It flips a shipped DoD line ("No `std::thread::sleep`, no retry counter, no jitter anywhere in the
  lock path") and the type doc's central claim, for a bound that layer 1 already provides at a
  coarser granularity — one thread per path, and only while a peer genuinely holds the lock. Config
  paths per process are few (global settings, project settings, `auth.json`, models, trust), so the
  ceiling is single digits out of tokio's 512-thread default blocking pool.
- It replaces an instant kernel wake with up to a backoff tick of added handoff latency, on the one
  path — genuine cross-process contention — that the two-layer design was built to serve well.
- It adds a tunable (the cap), a sleeping thread, and the `TryLockError` conversion above, for a
  case that only bites when a peer process is *wedged* rather than merely busy.

The one argument that would justify the reversal — "a readiness signal is only worth having if the
wait can also be abandoned" — is answered by the fix below: the wait *is* abandoned, by the caller,
in bounded time. What is not abandoned is one OS thread, and that was priced in.

Not proposed, and deliberately: a deadline or retry ceiling. Upstream's `ELOCKED` is a consequence
of the `mkdir` primitive, not a product decision cyrup shares, and "bounded by cancellation, not by
a retry count" is the recorded contract.

## Required change — [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs)

One file. No signature change, no call site touched, no new dependency.

### 1. New private type, next to `FileLock`

```rust
/// Owns the in-flight layer-2 task so that abandoning an acquisition — the cancel arm of the
/// `select!` below, or an outright drop of the [`FileLock::acquire`] future at its `.await` — is an
/// *abort*, not a bare detach.
///
/// `abort` cannot stop a `spawn_blocking` task that is already inside `flock(2)`; tokio says so in
/// as many words ("this *will not have any effect* … The exception is if the task has not started
/// running yet"). That exception is the case worth having: when the blocking pool is busy the task
/// can sit queued, and aborting it there drops the closure — releasing layer 1 with it — without
/// ever touching the filesystem.
///
/// Safe on a *blocking* task even though `BlockingSchedule::schedule` is `unreachable!()`: a queued
/// task is already `NOTIFIED` from birth, so `transition_to_notified_and_cancel` takes the
/// already-notified branch, returns false, and `remote_abort` never reaches `schedule`. Verified
/// against `tokio-1.53.1` (`runtime/task/harness.rs:118-127`, `runtime/task/state.rs:61`, `:303-333`,
/// `runtime/blocking/schedule.rs`).
struct AcquireTask(tokio::task::JoinHandle<Result<FileLock, ConfigError>>);

impl Drop for AcquireTask {
    fn drop(&mut self) {
        // A no-op once the task has completed, which includes the success path — the guard has
        // already been taken out of the handle by then.
        self.0.abort();
    }
}
```

### 2. Rewrite `FileLock::acquire` (`:57-69`)

```rust
    pub async fn acquire(target: &Path, cancel: Option<&CancelToken>) -> Result<Self, ConfigError> {
        let lock_path = lock_path_for(target);
        let token = cancel.unwrap_or(&NEVER_CANCELLED);
        let in_process = CONFIG_LOCK_HANDLE
            .guard(lock_path.clone(), token)
            .await
            .map_err(|_| ConfigError::Cancelled)?;
        let target_owned = target.to_path_buf();
        // Layer 1 is MOVED into the blocking task, which builds the whole guard. An acquisition
        // this caller walks away from therefore keeps layer 1 until it finishes, and whatever it
        // finally wins is released by `Drop` below — tokio drops a detached task's output.
        let mut task = AcquireTask(tokio::task::spawn_blocking(move || {
            let file = open_and_lock(&target_owned, &lock_path)?;
            Ok::<Self, ConfigError>(Self { _in_process: in_process, file })
        }));
        tokio::select! {
            // `biased` for the same reason as `KeyedLocks::guard`: with a token that is already
            // cancelled and an uncontended `flock`, both arms can be ready on the first poll, and
            // an unbiased `select!` would hand a guard to a cancelled caller about half the time.
            biased;
            _ = token.cancelled() => Err(ConfigError::Cancelled),
            // Unchanged mapping — see the note in the scope fence below before touching it.
            joined = &mut task.0 => {
                joined.map_err(|_| ConfigError::Lock { path: target.to_path_buf() })?
            }
        }
    }
```

Notes for the implementer:

- `KeyedGuard<PathBuf>` is `Send + 'static` — it holds an `OwnedMutexGuard<()>` (whose only field is
  an `Arc<Mutex<()>>`, auto-`Send` because `Mutex<()>: Send + Sync`), an `Arc<DashMap<…>>` and a
  `PathBuf` — so moving it into the closure needs no wrapper and no `unsafe`.
- Releasing it from a blocking-pool thread is fine: `KeyedGuard::drop` is a semaphore release plus a
  `DashMap::remove_if`, neither of which needs a runtime context.
- The failure path inside the closure is unchanged: `open_and_lock` returning `Err` drops
  `in_process` on the way out, exactly as the guard does today.
- Do **not** add a pre-`spawn_blocking` `token.is_cancelled()` check. The biased cancel arm is the
  check, and `AcquireTask::drop` reclaims a task that never started; a third checkpoint is a third
  thing to keep true.

### 3. Doc on `acquire` (`:52-56`) — the sentence the finding asks for

Keep the existing paragraph about who passes `Some`, and add:

```rust
    /// **What the token bounds, layer by layer** — the two are bounded by different mechanisms and
    /// the difference is load-bearing:
    ///
    /// * Layer 1 is cancelled *in place*: [`KeyedLocks::guard`] races the token against the mutex
    ///   and returns having taken nothing.
    /// * Layer 2 cannot be cancelled at all. `flock(2)` on a blocking thread is uninterruptible and
    ///   `JoinHandle::abort` is a documented no-op once a `spawn_blocking` task is running. What is
    ///   bounded here is the CALLER's wait: the acquisition is raced against the token, so an
    ///   aborted operation returns [`ConfigError::Cancelled`] at once instead of queueing behind a
    ///   foreign process that may never release.
    ///
    /// The abandoned acquisition outlives the caller and carries layer 1 with it, which is what
    /// makes the "exactly one waiter" invariant above survive cancellation: a cancelled acquire
    /// cannot free layer 1 while its own `flock` attempt is still queued, so a successor can never
    /// pile a second waiter from this process onto the same `flock`. Whatever the runaway
    /// acquisition wins it releases immediately — the detached output is a `FileLock`, so [`Drop`]
    /// unlocks the `flock` and then frees layer 1. The residue is one parked blocking-pool thread
    /// per contended path for as long as the peer holds the lock; that cost was taken knowingly,
    /// and capping it at one is precisely what layer 1 is for.
```

### 4. Type doc (`:35-41`) — one clause

The claim "admits at most one task per path per process, so the cross-process `flock` has exactly
one waiter" is what licenses the blocking call. Append that it holds **across cancellation and
future-drop, because the layer-1 guard travels with the blocking task**. Keep the addition to that
one clause: [`LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires`](./LOW-config-lock-skips-the-path-resolution-its-own-module-contract-requires.md)
qualifies the *same* sentence for path spellings, and the two edits must compose rather than
overwrite each other.

## Scope fence — do not fold these in

- **`JoinError` mapping.** The `joined.map_err(|_| ConfigError::Lock { … })` arm is reproduced
  verbatim on purpose. Discriminating panic from runtime-shutdown belongs to
  [`LOW-spawn-blocking-join-error-reported-as-lock-contention`](./LOW-spawn-blocking-join-error-reported-as-lock-contention.md);
  this restructure leaves that a one-arm edit.
- **Field-order comment (`:43-45`).** Owned by
  [`MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule`](./MEDIUM-filelock-drop-order-comment-states-a-false-rust-rule.md).
  This change does not alter the struct, its field order, or `Drop`. Note the dependency in the
  other direction: the claim above that a detached `FileLock` "releases the `flock` and then frees
  layer 1" relies on the explicit `FileExt::unlock` in `Drop::drop`, which is exactly what that task
  is protecting.
- **`store_err`.** This change makes `ConfigError::Cancelled` reachable from layer 2 as well, which
  raises the stakes on
  [`MEDIUM-lock-cancellation-reported-as-model-source-not-aborted`](./MEDIUM-lock-cancellation-reported-as-model-source-not-aborted.md)
  — a cancelled cross-process wait would surface as `ProviderError::ModelSource`. Fix it there, in
  `cyrup-config/src/models_store.rs`, not here.
- **Call sites.** `acquire`'s signature is unchanged, so
  [`models_store.rs:248`/`:311`/`:338`](../../crates/cyrup-config/src/models_store.rs),
  [`auth.rs:277`](../../crates/cyrup-config/src/auth.rs),
  [`settings/store.rs:69`](../../crates/cyrup-config/src/settings/store.rs),
  [`trust.rs:150`/`:174`](../../crates/cyrup-config/src/trust.rs) and
  [`settings_write.rs:84`](../../crates/cyrup-ext-subagents/src/discovery/settings_write.rs) are
  untouched. That the five `None` sites still cannot cancel *anything* is a separate question — the
  answer is to give those callers real tokens, not to give this function a deadline.

## Trade-offs accepted

- **One parked blocking-pool thread per contended path**, until the peer releases. Bounded by layer
  1, recorded as acceptable, and strictly better than the branch's current unbounded accumulation.
- **A spurious grant to a dead request.** The zombie eventually takes the `flock` and immediately
  releases it; a peer can be denied for that handoff. Microseconds, and no live waiter is starved.
- **Side effects survive cancellation.** A cancelled acquire can still leave `ensure_dir`'s parent
  directory and an empty `<path>.lock` behind. Already true today, and harmless — the sidecar is
  never unlinked by design (see `Drop`, `:94-104`).
- **A second `cancelled()` registration per acquire.** `WaitForCancellationFuture` is an intrusive
  registration on the token's `Notify` — no allocation — and it is noise beside `open(2)` +
  `flock(2)`. For the `None` callers it lands on the shared [`NEVER_CANCELLED`] token and never
  resolves, so the select always takes the join arm, exactly as `:25-30` already argues for layer 1.

## Definition of done

- [ ] `FileLock::acquire` races `token.cancelled()` against the layer-2 join with `biased`, cancel
      arm first, and returns `ConfigError::Cancelled` on that arm.
- [ ] The layer-1 `KeyedGuard` is owned by the blocking closure, which constructs the `FileLock`.
      No code path releases layer 1 for a path while an `open_and_lock` for that path is in flight.
- [ ] At most one `spawn_blocking` acquisition exists per lock path at any time — cancellations and
      dropped futures included.
- [ ] An abandoned acquisition releases both layers on its own completion, via `FileLock::drop`.
- [ ] The in-flight handle is held in a type whose `Drop` aborts it, so a caller that walks away
      also reclaims a task that has not started yet.
- [ ] `acquire`'s doc states which layer the token bounds and by what mechanism, and the type doc's
      "exactly one waiter" claim says it survives cancellation.
- [ ] `FileExt::lock` is still the only lock call in the file: no `try_lock`, no `std::thread::sleep`,
      no backoff, no retry counter, no deadline, no new tunable.
- [ ] `acquire`'s signature and every call site are unchanged; the `JoinError` arm is byte-identical
      to the pre-change mapping.

Sanity check while implementing (not a deliverable): hold the sidecar from a shell —
`flock <path>.lock -c 'sleep 30'` — then call `acquire` with a live token from a `tokio::test`,
cancel it, and confirm the call returns promptly while a subsequent `acquire` on the same path is
still admitted only after the first task's `flock` completes.
