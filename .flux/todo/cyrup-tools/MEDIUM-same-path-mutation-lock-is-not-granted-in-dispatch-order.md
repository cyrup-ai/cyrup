---
title: Same-path mutation lock is not granted in dispatch order
priority: MEDIUM
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: aug
status: done
updated: 2026-08-27
---

# Same-path mutation lock is not granted in dispatch order

## Core objective

When one assistant turn emits two mutations for the **same** file — `write A` then `write B`, or
`write` then `edit` — the surviving file content must be the payload of the mutation the model
issued **last**, deterministically, exactly as pi produces. Today cyrup usually produces the
**first** one's payload, and both calls report success.

This is an *ordering* task, not an *exclusion* task. Mutual exclusion is already correct and stays
correct: one process-global map, one mutex per realpath key
([lock.rs](../../../crates/cyrup-tools/src/lock.rs), [keyed_lock.rs](../../../crates/cyrup-core/src/keyed_lock.rs)).
Nothing below weakens it.

---

## What pi guarantees, and by what mechanism — verified

[pi file-mutation-queue.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/file-mutation-queue.ts)
(pi v0.84.3):

- `:4` `const fileMutationQueues = new Map<string, Promise<void>>()` — module scope.
- `:5` `let registrationQueue = Promise.resolve()` — one module-scope chain.
- `:7-14` `isMissingPathError` — `ENOENT`/`ENOTDIR` only.
- `:16-26` `getMutationQueueKey` — `resolve()` then `await realpath()`, rethrowing anything else at `:24`.
- `:33` `const registration = registrationQueue.then(async () => { … })` — **synchronous** `.then`,
  so the slot in the chain is claimed at the moment `withFileMutationQueue` is *called*.
- `:34-42` inside that serialized body: resolve the key, read `fileMutationQueues.get(key)`, mint
  `nextQueue`, `chainedQueue = currentQueue.then(() => nextQueue)`, `fileMutationQueues.set(key, chainedQueue)`.
  **The per-key link is made inside the serialized region.**
- `:46-49` `registrationQueue = registration.then(() => undefined, () => undefined)` — the chain
  advances on failure too.
- `:52` `await currentQueue` — the *wait* happens outside the serialized region.
- `:57-59` `finally` deletes the key when the queue drains.

The chain is reached in dispatch order because every hop is synchronous:

- [agent-loop.ts](../../../tmp/pi/packages/agent/src/agent-loop.ts) `:540-542`
  `Promise.all(finalizedCalls.map((entry) => entry()))` — `map` invokes the closures in source order.
- `:523` each closure's first statement is `await executePreparedToolCall(preparation, …)`;
  the async function body (`:670`) runs synchronously up to `:679` `await prepared.tool.execute(…)`,
  and the argument `prepared.tool.execute(…)` is itself evaluated synchronously.
- [write.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts) `:209-210` and
  [edit.ts](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts) `:334-336`: `resolveToCwd`
  is synchronous and `withFileMutationQueue` is the **first** thing awaited. No `await` precedes it.

So: batch order → closure invocation order → `execute` body start order → `registrationQueue.then`
order → key resolution order → per-key link order → grant order. One unbroken synchronous prefix.

The harness twin
([pi harness file-mutation-queue.ts](../../../tmp/pi/packages/agent/src/harness/tools/file-mutation-queue.ts))
is the same algorithm with the state held per `ExecutionEnv` in a `WeakMap` (`:9-18`), chain field at
`:6`, registration at `:31`, reassignment at `:43-46`. cyrup ports the coding-agent variant, so
cyrup's chain is process-global.

---

## What cyrup does — verified, with two corrections to the original finding

### 1. `tokio::sync::Mutex` is already strictly FIFO. The mutex is not the defect.

Workspace resolves tokio **1.52.3** (`Cargo.lock`). `tokio-1.52.3/src/sync/mutex.rs`:

- `:20-22` — *"Tokio's Mutex operates on a guaranteed FIFO basis. This means that the order in which
  tasks call the `lock` method is the exact order in which they will acquire the lock."*
- `:112-114` — *"works in a simple FIFO (first in, first out) style … In that way the Mutex is
  'fair' and predictable"*.
- `:598-600` (`lock_owned`) — *"uses a queue to fairly distribute locks in the order they were
  requested. Cancelling a call to `lock_owned` makes you lose your place in the queue."*

It is implemented on `batch_semaphore`, whose module docs state the waiter list is popped from the
front. **The waiter's place is taken when its acquire future is first polled** — an unpolled future
has done nothing, so "the order in which tasks call `lock`" can only mean first-poll order. That
fact is the lever the fix below uses.

Consequence: `KeyedLocks::guard` ([keyed_lock.rs](../../../crates/cyrup-core/src/keyed_lock.rs):155-177)
grants in arrival order, and arrival order is the only thing that is wrong.

### 2. The guard is requested **after** an `await`, so arrival order is scrambled.

[lock.rs](../../../crates/cyrup-tools/src/lock.rs):175-186:

```rust
pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError> {
    let key = Self::key(path).await?;   // tokio::fs::canonicalize -> spawn_blocking round trip
    self.inner                          // ^ suspension point BEFORE any queue position exists
        .guard(key, cancel)
        .await
        .map(MutationGuard)
        .map_err(|_| error::aborted())
}
```

`Self::key` ([lock.rs](../../../crates/cyrup-tools/src/lock.rs):153-161) is `tokio::fs::canonicalize`,
which dispatches to the blocking pool. Two calls dispatched in order A→B complete in whatever order
the blocking pool returns them, and *that* is the order they reach the mutex. There is no analogue
of `registrationQueue` anywhere in the workspace.

### 3. The dominant cause is upstream of the lock entirely: `tokio::spawn` starts the batch backwards.

[exec.rs](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs):100-144 spawns each prepared call
onto a `JoinSet` in source order. tokio's multi-thread scheduler does **not** poll them in that order:

- `tokio-1.52.3/src/runtime/scheduler/multi_thread/worker.rs:1353-1377` `schedule_local` — a task
  spawned from a worker thread with `is_yield == false` goes into the worker's **LIFO slot**, and the
  slot's previous occupant is pushed to the *back* of the run queue.
- `:707` — when the worker next picks work it takes the **LIFO slot first**.

So for a two-call batch, `spawn(A)` puts A in the slot; `spawn(B)` evicts A to the queue's back and
puts B in the slot; the worker then polls **B before A**. This is a systematic inversion, not a
coin flip: cyrup preferentially runs the *later* mutation first, so the *earlier* payload survives.

No change confined to `guard()` can fix this, because the registration chain is entered on first
poll, and first-poll order is what is inverted.

### 4. `write` and `edit` themselves are already correct.

[write.rs](../../../crates/cyrup-tools/src/tools/write.rs):102 and
[edit.rs](../../../crates/cyrup-tools/src/tools/edit.rs):223 call `guard()` as the **first** `.await`
of `execute`; everything before it (`serde_json::from_value`, `normalize_args`,
`path::resolve_to_cwd` at [path.rs](../../../crates/cyrup-tools/src/path.rs):248) is synchronous.
`Tool` is `#[async_trait]` ([tool.rs](../../../crates/cyrup-core/src/tool.rs):88-89, `:227`), so that
synchronous prefix runs on the boxed future's first poll — i.e. on the spawned task's first poll.
**These two files do not change, and that property must not be broken.**

---

## The three ordering points

| # | Ordering point | pi | cyrup today |
|---|---|---|---|
| 1 | Tool body reaches its lock call in dispatch order | `map(entry => entry())`, synchronous (agent-loop.ts:540-542) | `joinset.spawn`, LIFO-inverted (exec.rs:107) |
| 2 | Key resolution runs in that same order | `registrationQueue` (file-mutation-queue.ts:5/:33/:46-49) | independent `canonicalize` per call (lock.rs:180) |
| 3 | Per-key queue position claimed before the serialized region is released | `fileMutationQueues.set(key, chainedQueue)` (:42) inside the chain | position taken by racing to the mutex, outside anything |

All three must be closed. Closing any subset leaves the inversion.

---

## Required change — three files, in this order

`cyrup-core` **must land first**; `cyrup-tools` depends on the new API, and `cyrup-agent` is
independent of both but pointless without them.

### Step 1 — `crates/cyrup-core/src/keyed_lock.rs`

Split acquisition into *claim* and *wait*, so a caller can hold a registration lock across the claim.

**CURRENT** ([keyed_lock.rs](../../../crates/cyrup-core/src/keyed_lock.rs):155-177):

```rust
pub async fn guard(&self, key: K, cancel: &CancelToken) -> Result<KeyedGuard<K>, Cancelled> {
    let _pending = PendingEntry { map: self.map.clone(), key: key.clone() };
    let lock = self.map.get_or_insert(key.clone());
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Cancelled),
        g = lock.clone().lock_owned() => Ok(KeyedGuard {
            inner: Some(g), lock: Some(lock), map: self.map.clone(), key,
        }),
    }
}
```

**REPLACEMENT** — add `use std::future::{poll_fn, Future}; use std::pin::Pin; use std::task::Poll;`
at the top of the module, then:

```rust
impl<K: Eq + Hash + Clone> KeyedLocks<K> {
    /// Take this task's place in `key`'s FIFO queue WITHOUT waiting for it.
    ///
    /// The returned future resolves on its FIRST poll and can therefore never suspend, so a caller
    /// may hold a registration lock across `enqueue(..).await` and release it on the very next
    /// line with its queue position already claimed. This is the Rust shape of pi's
    /// `fileMutationQueues.set(key, chainedQueue)` (file-mutation-queue.ts:42): the LINK is made
    /// inside the serialized registration, only the WAIT happens outside it.
    pub async fn enqueue(&self, key: K) -> KeyedAcquire<K> {
        let pending = PendingEntry { map: self.map.clone(), key: key.clone() };
        let lock = self.map.get_or_insert(key.clone());
        let mut acquire: Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send>> =
            Box::pin(Arc::clone(&lock).lock_owned());
        // EXACTLY ONE POLL. `tokio::sync::Mutex` is strictly FIFO (tokio 1.52.3
        // src/sync/mutex.rs:20-22, :598-600) and a task's place in that queue is taken when its
        // acquire future is first polled — an unpolled future has done nothing. `poll_fn` returns
        // `Ready` on its own first poll, so this `.await` cannot yield and the caller's
        // registration lock is still held when `enqueue` returns.
        let early = match poll_fn(|cx| Poll::Ready(acquire.as_mut().poll(cx))).await {
            Poll::Ready(g) => Some(g),
            Poll::Pending => None,
        };
        KeyedAcquire {
            acquire: Some(acquire),
            early,
            lock: Some(lock),
            map: self.map.clone(),
            key,
            _pending: pending,
        }
    }

    /// Unchanged behaviour, now expressed over the two halves. `cyrup-config` keeps using this.
    pub async fn guard(&self, key: K, cancel: &CancelToken) -> Result<KeyedGuard<K>, Cancelled> {
        self.enqueue(key).await.wait(cancel).await
    }
}

/// A claimed-but-not-yet-granted place in one key's FIFO queue.
pub struct KeyedAcquire<K: Eq + Hash + Clone> {
    acquire: Option<Pin<Box<dyn Future<Output = OwnedMutexGuard<()>> + Send>>>,
    early: Option<OwnedMutexGuard<()>>,
    lock: Option<Arc<Mutex<()>>>,
    map: KeyedLockMap<K>,
    key: K,
    /// Declared LAST so it drops AFTER `Drop::drop` below has released the guard, the acquire
    /// future and this handle's `Arc` clone — otherwise the `strong_count == 1` predicate would
    /// always see them and the entry would never be evicted.
    _pending: PendingEntry<K>,
}

impl<K: Eq + Hash + Clone> KeyedAcquire<K> {
    /// Wait for the place claimed by [`KeyedLocks::enqueue`] to come up.
    pub async fn wait(mut self, cancel: &CancelToken) -> Result<KeyedGuard<K>, Cancelled> {
        // Carries over what `biased` bought the old `select!`: an already-cancelled token loses
        // even when the lock is free. Now deterministic by construction rather than by poll order.
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        let granted = match self.early.take() {
            Some(g) => g,
            None => {
                let acquire = match self.acquire.as_mut() {
                    Some(a) => a,
                    None => return Err(Cancelled),
                };
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return Err(Cancelled),
                    g = acquire.as_mut() => g,
                }
            }
        };
        Ok(KeyedGuard {
            inner: Some(granted),
            lock: self.lock.take(),
            map: self.map.clone(),
            key: self.key.clone(),
        })
    }
}

impl<K: Eq + Hash + Clone> Drop for KeyedAcquire<K> {
    /// Release in the same order `KeyedGuard::drop` does, so `_pending` — which drops after this
    /// body returns — sees only the map's own reference. Covers both non-guard exits: a cancelled
    /// wait and an outright drop of the `wait()` future.
    fn drop(&mut self) {
        self.early.take();
        self.acquire.take();
        self.lock.take();
    }
}
```

`KeyedLockMap` is untouched: its one-live-mutex-per-key invariant, its private `get_or_insert` /
`evict_if_unreferenced`, and the three observers `contains_key` / `mutex_for` / `ptr_eq` all stay
exactly as they are. `PendingEntry` stays and keeps its single eviction predicate.

Also in **`crates/cyrup-core/src/lib.rs`** line 33, widen the re-export:

```rust
pub use keyed_lock::{Cancelled, KeyedAcquire, KeyedGuard, KeyedLockMap, KeyedLocks};
```

### Step 2 — `crates/cyrup-tools/src/lock.rs`

Add the process-global registration chain and hold it across key resolution *and* the claim.

**CURRENT** ([lock.rs](../../../crates/cyrup-tools/src/lock.rs):175-186):

```rust
pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError> {
    let key = Self::key(path).await?;
    self.inner
        .guard(key, cancel)
        .await
        .map(MutationGuard)
        .map_err(|_| error::aborted())
}
```

**REPLACEMENT** — beside `FILE_MUTATION_LOCKS` (line 28), add:

```rust
/// Pi's module-scope `registrationQueue` (file-mutation-queue.ts:5).
///
/// Pi funnels EVERY registration — the `realpath` key resolution at `:34` and the queue link at
/// `:42` — through one chain, so registrations happen one at a time and in call order. This is the
/// Rust equivalent: `tokio::sync::Mutex` is strictly FIFO (tokio 1.52.3 src/sync/mutex.rs:20-22),
/// so entering it in dispatch order is sufficient to leave it in dispatch order.
///
/// It is deliberately GLOBAL, not per-path: a slow `realpath` on one mount delays registrations for
/// every other path too, exactly as pi's single chain does. That is the upstream behaviour, and it
/// is bounded — the chain is released as soon as the key is resolved and the per-key place is
/// claimed, never across the mutation itself.
static MUTATION_REGISTRATION: LazyLock<tokio::sync::Mutex<()>> =
    LazyLock::new(|| tokio::sync::Mutex::new(()));
```

and rewrite `guard`:

```rust
pub async fn guard(&self, path: &Path, cancel: &CancelToken) -> Result<MutationGuard, ToolError> {
    // Pi `:33`: the registration slot is claimed in call order and the body below runs serialized.
    let registration = MUTATION_REGISTRATION.lock().await;

    // Pi `:34`: `await getMutationQueueKey(filePath)` — INSIDE the chain, so two spellings of the
    // same file resolve in call order instead of racing on the blocking pool. On the `?` path the
    // guard drops here and the chain advances, matching pi `:46-49`, which advances the chain on
    // rejection as well as on fulfilment.
    let key = Self::key(path).await?;

    // Pi `:35-42`: link into this key's queue. `enqueue` never yields, so the place is taken while
    // the registration is still held.
    let acquire = self.inner.enqueue(key).await;

    // Pi `:51`: registration is complete; the chain advances. Everything after this point is pi's
    // `await currentQueue`, which happens outside the chain.
    drop(registration);

    acquire
        .wait(cancel)
        .await
        .map(MutationGuard)
        .map_err(|_| error::aborted())
}
```

`FILE_MUTATION_LOCKS`, `FileMutationLocks`, `MutationGuard`, `Default`, `is_missing_path_error` and
`Self::key` are all unchanged.

### Step 3 — `crates/cyrup-agent/src/agent/run/tools/exec.rs`

Start the batch's calls in source order, each one released as soon as the previous has reached its
first suspension point. This is the Rust shape of pi's `map((entry) => entry())`.

**CURRENT** ([exec.rs](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs):100-144):

```rust
let mut remaining = deferred.len();
for Deferred { source_index, tool, args, call_id, tool_name } in deferred {
    let accepting = Arc::new(AtomicBool::new(true));
    // … acc2 / utx / ftx / cid / child unchanged …
    joinset.spawn(async move {
        // … on_update sink, catch_unwind around tool.execute, ftx.send(Finished) — unchanged …
    });
}
```

**REPLACEMENT** — add `use std::future::{poll_fn, Future}; use std::task::Poll; use tokio::sync::oneshot;`
to the imports, then:

```rust
let mut remaining = deferred.len();
// The batch's start order. Each call releases the next as soon as its own body has been driven to
// its first suspension point — which for `write`/`edit` is inside `FileMutationLocks::guard`, so
// the mutation registrations line up in source order.
let mut prev_started: Option<oneshot::Receiver<()>> = None;
for Deferred { source_index, tool, args, call_id, tool_name } in deferred {
    let accepting = Arc::new(AtomicBool::new(true));
    let acc2 = accepting.clone();
    let utx = tx.clone();
    let ftx = tx.clone();
    let cid = call_id;
    let child = self.cancel.child();
    let (started_tx, started_rx) = oneshot::channel::<()>();
    let wait_turn = prev_started.replace(started_rx);
    joinset.spawn(async move {
        // Pi invokes every prepared call from `finalizedCalls.map((entry) => entry())`
        // (agent-loop.ts:540-542): `map` walks the array in source order and each async body runs
        // synchronously to its FIRST suspension point before the next closure is invoked.
        // `tokio::spawn` INVERTS that. `schedule_local` puts each newly spawned task in the
        // worker's LIFO slot and pushes the slot's previous occupant to the back of the run queue
        // (tokio 1.52.3 runtime/scheduler/multi_thread/worker.rs:1353-1377), and the worker polls
        // the LIFO slot first (:707) — so an unordered batch starts its LAST call first. An `Err`
        // here means the previous call was aborted before it ran; proceed rather than stall.
        if let Some(turn) = wait_turn {
            let _ = turn.await;
        }

        let mut body = std::pin::pin!(async move {
            // … on_update sink, catch_unwind around tool.execute, ftx.send(Finished) —
            //     byte-for-byte the existing body, unchanged …
        });

        // Drive this call to its first suspension point — pi's `entry()` — then hand the batch on.
        let first = poll_fn(|cx| Poll::Ready(body.as_mut().poll(cx))).await;
        let _ = started_tx.send(());
        if first.is_pending() {
            body.await;
        }
    });
}
```

Nothing else in `exec.rs` changes: `remaining`, `drop(tx)`, the `rx.recv()` loop, the source-indexed
`finalized` slots, `join_next`, and `execute_sequential` are all untouched. The handoff costs one
`oneshot` per call and is released at each body's first `.await`, so a long-running `bash` in the
same batch delays nobody.

### Step 4 — files that must NOT change, and the invariant they carry

[write.rs](../../../crates/cyrup-tools/src/tools/write.rs) and
[edit.rs](../../../crates/cyrup-tools/src/tools/edit.rs) are already correct: `guard()` is the first
`.await` in both `execute` bodies (`write.rs:102`, `edit.rs:223`). **No `.await` may be introduced
before those `guard()` calls** — an await there would suspend before the registration chain is
entered and reopen the whole gap. Neither may declare `execution_mode` (`write.rs:68`,
`edit.rs:141`); pi's mutators inherit the parallel default and are serialized only by the queue.

---

## Citation corrections against the original finding

- `agent-loop.ts` lives at `packages/agent/src/agent-loop.ts` (pi v0.84.3), not under
  `coding-agent`. `executePreparedToolCall` is **declared at `:670`**, not `:678`; its synchronous
  `prepared.tool.execute(…)` is at `:679`; the parallel batch calls it at `:523`; the
  `Promise.all(finalizedCalls.map(…))` is `:540-542`. All were cited as a single `:678`.
- `keyed_lock.rs:145-168` names the `impl KeyedLocks` block (`:145`); `guard` itself is `:155-177`.
- `exec.rs` is `crates/cyrup-agent/src/agent/run/tools/exec.rs` (the `src/agent/` segment was
  missing); the spawn loop is `:100-144` with `joinset.spawn` at `:107`, not `:96-107`.
- The finding's claim that "the tokio Mutex is FIFO only from the moment each task arrives" is
  **correct**, and the mutex is therefore not the defect. The *Parity action* as originally worded
  (a registration mutex around key resolution and entry insertion) is **necessary but not
  sufficient** — it does not address the `tokio::spawn` LIFO inversion in Step 3, which is the
  larger of the two causes and turns a race into a systematic reversal.
- `file-mutation-queue.ts:4,5,7-14,16-26,32,33,35-42,46-49,57-59`, the harness twin at `:6,31,43-46`,
  `write.ts:210`, `edit.ts:336`, `lock.rs:153-161`, `lock.rs:175-186`, `write.rs:68`, `edit.rs:141`:
  all verified correct as written.

---

## Genuinely uncertain

- **First-poll queue placement.** tokio documents FIFO as "the order in which tasks call the `lock`
  method" (mutex.rs:20-22) rather than "the order acquire futures are first polled". The two are the
  same thing — an unpolled future has done nothing — and `batch_semaphore` pushes the waiter on the
  first `poll_acquire`, but this is the one load-bearing property of Step 1 that rests on reading
  the implementation rather than on an explicit contract sentence.
- **Cancellation forfeits a place.** `lock_owned` loses its queue position when cancelled
  (mutex.rs:598-600). A mutator cancelled while waiting therefore vacates its slot; pi has no
  cancellation of `await currentQueue` at all, so there is no upstream behaviour to match here.
- **Registration chain scope.** cyrup ports the coding-agent variant, whose chain is module-global.
  The harness twin scopes both the map and the chain per `ExecutionEnv`. If cyrup ever adopts the
  harness shape, `MUTATION_REGISTRATION` becomes per-env alongside `FILE_MUTATION_LOCKS`.
- **Other dispatch paths.** `execute_parallel` is the only place tool bodies are spawned
  (workspace-wide, `joinset.spawn` appears once). If another concurrent dispatcher is added it will
  need the same start ordering.

---

## Definition of done

Observable behaviour, on the multi-thread runtime:

1. A single assistant turn emitting `write(p, A)` then `write(p, B)` leaves `p` containing **B** —
   every run, for any batch size, and for any mix of `write`/`edit` targeting `p`.
2. The same holds when the two calls spell the path differently (relative vs absolute, via a
   symlink, through `..`) but resolve to the same realpath.
3. The same holds when the first call's path resolution is slower than the second's — a symlink
   chain, a network mount, a cold dentry cache no longer changes which payload survives.
4. Mutations of *different* files still overlap: a mutation holding one path's lock does not delay a
   mutation of another path beyond that second path's own key resolution.
5. Non-mutating calls in the same batch (`read`, `grep`, `bash`) still run concurrently with each
   other and with the mutators; a long-running call does not delay any later call in the batch past
   its own first suspension point.
6. A cancelled turn still aborts a pending mutation before the lock is granted, and after every exit
   path — granted, cancelled, or the acquisition future dropped — the process-global lock map holds
   no entry for that key.
7. `cyrup-config`'s own lock domain behaves exactly as before.
8. No behaviour is added that pi lacks: this is a parity port of `registrationQueue` plus the
   dispatch ordering JavaScript's single-threaded event loop gives pi for free.
