---
title: Same-path mutation lock is not granted in dispatch order
priority: MEDIUM
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: exec
status: done
updated: 2026-08-27 18:50
---

# Same-path mutation lock is not granted in dispatch order

QA reviewed the landed implementation. **The mechanism is correct and complete** — all three
ordering points are closed, `KeyedLockMap`'s invariants are intact, the mutex was (correctly) not
touched, and `write.rs`/`edit.rs` still keep `guard()` as their first `.await`. Do not revisit the
mechanism; the *Verified as correct* section at the bottom is the record of that.

What is missing is **verification**, one **documentation clause**, and two **stale cross-references**.
Six steps below, in order. Every one of them is prescriptive: there is one required shape, given
inline. Do not substitute sleeps, `yield_now` spins, or "retry N times" for any of the sequencing
devices — each test below is deterministic-GREEN by construction and says so.

---

## The coverage map this task must end with

Four independently silent-failure-prone mechanisms carry DoD 1/2/3. Each row must end pinned by a
named test that fails — not degrades — when that row is reverted.

| Mechanism | Anchor | Silent revert | Pinned by (this task) |
|---|---|---|---|
| `oneshot` start chain | [`exec.rs:106,:114-115,:127-129,:177-181`](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) — `prev_started` / `wait_turn` / `started_tx` | delete `wait_turn`, or move `started_tx.send(())` above the `poll_fn` | **Step 4** `a_02_2b_parallel_bodies_start_in_source_order` |
| `MUTATION_REGISTRATION` held across key resolution **and** the claim | [`lock.rs:41-42,:205-219`](../../../crates/cyrup-tools/src/lock.rs) — `MUTATION_REGISTRATION`, `FileMutationLocks::guard` | move `drop(registration)` above `Self::key(..).await` | **Step 2a** `the_registration_chain_spans_key_resolution`, **Step 2b** `two_spellings_of_one_path_are_granted_in_call_order` |
| `enqueue` never yields | [`keyed_lock.rs:164-190`](../../../crates/cyrup-core/src/keyed_lock.rs) — `KeyedLocks::enqueue` | any `.await` inside `enqueue`; or dropping the `poll_fn` and letting `wait` do the first poll | **Step 1a** `enqueue_resolves_on_its_first_poll`, **Step 1b** `places_are_granted_in_enqueue_order_not_in_wait_order` |
| `guard()` is the first `.await` of `execute` | [`write.rs:108`](../../../crates/cyrup-tools/src/tools/write.rs), [`edit.rs:273`](../../../crates/cyrup-tools/src/tools/edit.rs) | insert ANY `.await` above it — both files were edited by concurrent work AFTER this fix landed, with nothing to catch it | **Step 3** `write_takes_the_mutation_lock_before_any_other_await`, `edit_takes_the_mutation_lock_before_any_other_await` |
| all four together, on the path a user hits | `execute_parallel` → real `write`/`edit` → real `FileMutationLocks` | any of the above | **Step 5** three tests in `cyrup-session-svc` |

Existing coverage that does **not** reach any of these: `a_02_2_parallel_completion_vs_source_order`
([agent_loop.rs:256](../../../crates/cyrup-agent/src/tests/agent_loop.rs)) asserts concurrency and
event order, `agent_002_parallel_defers_execution_until_whole_batch_is_prepared` ([:1148](../../../crates/cyrup-agent/src/tests/agent_loop.rs))
asserts the prepare/start split — neither observes which *body* starts first; `lock.rs`'s module
tests cover exclusion, eviction and keying only; `cross_registry_mutation_lock.rs` covers the map
being process-global.

### The one shared device: `poll_once`

Every deterministic assertion below rests on polling a future **exactly once** with a no-op waker.
That is the only way to state "this future resolved without suspending" and "this waiter has not
been granted yet" without a wall clock. `Waker::noop()` is stable since 1.85; the workspace MSRV is
1.96 ([Cargo.toml:89](../../../Cargo.toml)). Declare it locally in each test module — it is four
lines and must not become a cross-crate dependency:

```rust
/// Poll `f` exactly once with a no-op waker. No wall clock, no scheduler involvement.
fn poll_once<F: std::future::Future>(f: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    f.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
}
```

### The other shared device: a saturated blocking pool

`FileMutationLocks::key` ([lock.rs:168-176](../../../crates/cyrup-tools/src/lock.rs)) calls
`tokio::fs::canonicalize`, which is `spawn_blocking` under the hood. Build the test runtime with
`max_blocking_threads(1)` and occupy that one thread, and the `realpath` job **provably cannot
complete**: tokio's blocking pool is a FIFO queue drained by its threads, so with one thread the
jobs run in submission order. That turns "the first poll of `guard()` parks inside key resolution"
from an overwhelmingly likely scheduling outcome into a certainty — which is what makes Steps 2a and
3 deterministic rather than merely probable. Note that `#[tokio::test]` cannot express
`max_blocking_threads`, so those tests are plain `#[test]` with a hand-built runtime.

---

## Step 1 — `cyrup-core`: pin `enqueue`, and record the tokio caveat

`cyrup-core/src/keyed_lock.rs` carries no tests today, and `enqueue`/`KeyedAcquire` are new public
API (re-exported at [lib.rs:34](../../../crates/cyrup-core/src/lib.rs)). Add `mod tests` at the end
of the file, and rewrite the `EXACTLY ONE POLL` comment.

### 1a/1b/1c — the tests

Append to [`crates/cyrup-core/src/keyed_lock.rs`](../../../crates/cyrup-core/src/keyed_lock.rs):

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    /// Poll `f` exactly once with a no-op waker. No wall clock, no scheduler involvement.
    fn poll_once<F: Future>(f: Pin<&mut F>) -> Poll<F::Output> {
        f.poll(&mut Context::from_waker(Waker::noop()))
    }

    /// The never-yield property, asserted directly rather than inferred from the source.
    ///
    /// `FileMutationLocks::guard` (`cyrup-tools/src/lock.rs:215`) holds `MUTATION_REGISTRATION`
    /// across `enqueue(..).await`. A suspension there releases the registration chain BEFORE the
    /// queue place is claimed, which is the whole defect this task closed — and it would be
    /// silent: nothing panics, ordering just becomes a blocking-pool coin flip again. The HELD
    /// case is the one that matters; the free case is here so the test cannot pass by never
    /// exercising a queue at all.
    #[tokio::test]
    async fn enqueue_resolves_on_its_first_poll() {
        let locks = KeyedLocks::new(KeyedLockMap::new());
        let cancel = CancelToken::new();

        let mut free = pin!(locks.enqueue("k"));
        let Poll::Ready(claimed) = poll_once(free.as_mut()) else {
            panic!("`enqueue` must resolve on its FIRST poll — it has an `.await` in it now");
        };
        drop(claimed);

        let held = locks.guard("k", &cancel).await.unwrap();
        let mut contended = pin!(locks.enqueue("k"));
        let Poll::Ready(queued) = poll_once(contended.as_mut()) else {
            panic!(
                "`enqueue` suspended on a HELD lock. It must claim the place and return: the \
                 caller's registration lock is held across this call (cyrup-tools/src/lock.rs:215)"
            );
        };
        drop(queued);
        drop(held);
    }

    /// THE property this task exists for, reduced to one crate and zero clocks: the queue place is
    /// taken inside `enqueue`, so grant order follows ENQUEUE order — not the order in which the
    /// halves are later `wait`ed on, and not poll order.
    ///
    /// Deterministic in both directions. Releasing a `tokio::sync::Mutex` does not put the permit
    /// back on the counter for whoever polls first: `add_permits_locked` walks the wait queue from
    /// the tail and ASSIGNS the permit into the waiter's own node
    /// (tokio 1.52.3 src/sync/batch_semaphore.rs:306-330, `assign_permits` + `queue.pop_back()`),
    /// so `second`'s later poll finds nothing and `first`'s finds a permit already banked.
    /// RED the moment `enqueue`'s `poll_fn` stops taking the place: the waiter node is pushed on
    /// the first poll that finds no permit (batch_semaphore.rs:496-517), so a first poll deferred
    /// into `wait` would enqueue `second` ahead of `first` and invert both assertions.
    #[tokio::test]
    async fn places_are_granted_in_enqueue_order_not_in_wait_order() {
        let locks = KeyedLocks::new(KeyedLockMap::new());
        let cancel = CancelToken::new();
        let held = locks.guard("k", &cancel).await.unwrap();

        // Two places claimed in this order, with NOTHING awaited in between — the exact shape
        // `FileMutationLocks::guard` produces while it holds the registration chain.
        let first = locks.enqueue("k").await;
        let second = locks.enqueue("k").await;

        // …waited on in the OPPOSITE order.
        let mut wait_second = pin!(second.wait(&cancel));
        let mut wait_first = pin!(first.wait(&cancel));
        assert!(poll_once(wait_second.as_mut()).is_pending(), "the lock is held");
        assert!(poll_once(wait_first.as_mut()).is_pending(), "the lock is held");

        drop(held);
        assert!(
            poll_once(wait_second.as_mut()).is_pending(),
            "the SECOND place jumped the first — the queue place is no longer taken in `enqueue`"
        );
        assert!(
            poll_once(wait_first.as_mut()).is_ready(),
            "the FIRST place must be granted first, however late its `wait` is polled"
        );
    }

    /// DoD 6's dropped-acquisition half, non-vacuously. Note the DROP ORDER: `held` goes first, so
    /// the entry survives on `queued`'s reference alone and only `KeyedAcquire::drop` +
    /// `PendingEntry::drop` can evict it. Reverse the two drops and the test passes with
    /// `PendingEntry` deleted, which is exactly how the `cyrup-tools` version of this test came to
    /// pass for the wrong reason (Step 2c).
    #[tokio::test]
    async fn a_forfeited_place_is_released_and_its_entry_evicted() {
        let map = KeyedLockMap::new();
        let locks = KeyedLocks::new(map.clone());
        let cancel = CancelToken::new();

        let held = locks.guard("k", &cancel).await.unwrap();
        let queued = locks.enqueue("k").await;
        assert!(map.contains_key(&"k"));

        drop(held);
        assert!(map.contains_key(&"k"), "a live waiter must keep its entry alive");
        drop(queued);
        assert!(
            !map.contains_key(&"k"),
            "a forfeited place must evict its entry — the map is a process-global static, so \
             nothing else ever will (tokio's `lock_owned` cancel-safety note, src/sync/mutex.rs:598-600)"
        );
    }
}
```

`cyrup-core`'s `[dev-dependencies]` already carries `tokio` with `macros` + `rt-multi-thread`
([Cargo.toml:23](../../../Cargo.toml)); no manifest change.

### 1d — the caveat, replacing `keyed_lock.rs:173-177`

The current block states first-poll queue placement as settled and cites
`tokio 1.52.3 src/sync/mutex.rs:20-22, :598-600`. Those lines say *"the order in which tasks call
the `lock` method"* and *"the order they were requested"* — neither is about **polls**. The
equivalence is real but comes from reading `batch_semaphore`, and there are **two** ways a tokio bump
can break it silently. Replace the comment inside `KeyedLocks::enqueue` (anchor: the `EXACTLY ONE
POLL` line above the `poll_fn`) with exactly this:

```rust
        // EXACTLY ONE POLL — and this is the ONE property in this module resting on tokio's
        // IMPLEMENTATION rather than on its contract. Re-verify it on every tokio bump; the two
        // `cfg(test)` tests named at the bottom of this block fail rather than degrade.
        //
        // What tokio DOCUMENTS is weaker than what this line needs. The `Mutex` type doc says
        // "the order in which tasks call the `lock` method is the exact order in which they will
        // acquire the lock" (tokio 1.52.3 src/sync/mutex.rs:20-22) and `lock_owned`'s cancel-safety
        // note says the queue distributes locks "in the order they were requested" (:598-600).
        // Neither sentence is about POLLS. `enqueue` does not call `lock_owned` in dispatch order —
        // its CALLER does that — it claims the place by polling an already-constructed acquire
        // future exactly once, and an unpolled future has done nothing at all.
        //
        // The equivalence holds in 1.52.3 because the waiter node is pushed on the FIRST poll that
        // finds no free permit: `Semaphore::poll_acquire` registers the waker and runs
        // `waiters.queue.push_front(node)` under its `!queued` branch before returning `Pending`
        // (src/sync/batch_semaphore.rs:496-517); `Acquire::poll` latches `queued = true` (:600-604)
        // so later polls reuse that place; and a release ASSIGNS the permit into the tail waiter's
        // own node rather than returning it to the counter (`add_permits_locked`, :306-330, via
        // `assign_permits` + `queue.pop_back()`). push_front + pop_back = FIFO by first-poll order.
        //
        // TWO caveats, both of which a bump can change with no compile error:
        //
        // 1. `Acquire::poll` runs `ready!(coop::poll_proceed(cx))` BEFORE `poll_acquire`
        //    (batch_semaphore.rs:598). A task whose cooperative budget is spent gets `Pending`
        //    there with NO place claimed, and the place is taken later — at the first poll inside
        //    `KeyedAcquire::wait`, i.e. OUTSIDE the caller's registration lock. The budget is 128
        //    units per task poll (src/task/coop/mod.rs:115-116) and LIFO-slot tasks inherit the
        //    parent's remainder (src/runtime/scheduler/multi_thread/worker.rs:675), so it is not a
        //    per-`.await` allowance. This is unreachable on the `write`/`edit` path —
        //    `FileMutationLocks::guard` is at most three budget units into a fresh task poll — and
        //    it degrades ordering only, never correctness: exclusion, eviction and cancellation are
        //    unaffected. A future caller that reaches `enqueue` deep inside one poll would lose the
        //    guarantee with no diagnostic.
        // 2. If tokio ever moves queue placement off the first poll, this `poll_fn` becomes a no-op
        //    and ordering silently reverts to "whoever polls `wait` first".
        //
        // Pinned by `enqueue_resolves_on_its_first_poll` and
        // `places_are_granted_in_enqueue_order_not_in_wait_order` in this file's `mod tests`.
        //
        // `poll_fn` returns `Ready` on its own first poll, so this `.await` cannot yield and the
        // caller's registration lock is still held when `enqueue` returns.
```

Also extend `enqueue`'s doc comment (the sentence at `:155-157` beginning "The returned future
resolves on its FIRST poll") with one clause: `— see the EXACTLY ONE POLL block in the body for the
tokio property that rests on, and the re-verify step a tokio bump requires.`

`cyrup-tools/src/lock.rs:33-35` cites the *same* `mutex.rs:20-22` sentence for `MUTATION_REGISTRATION`
and is **correct** there — that IS the documented case, tasks calling `lock` in dispatch order.
Append one clause so a reader cannot conflate the two: `— this is the documented case (tasks calling`
`lock`), unlike `KeyedLocks::enqueue`'s first-poll claim, which carries its own caveat.`

---

## Step 2 — `cyrup-tools/src/lock.rs`: pin the registration chain

### 2a — a `cfg(test)` observer on the chain

`MUTATION_REGISTRATION` is private and must stay private. Add the narrowest possible window onto it,
immediately after the static ([lock.rs:41-42](../../../crates/cyrup-tools/src/lock.rs)):

```rust
/// Test-only observer on the registration chain, so the tests below can assert on its STATE
/// instead of on a sleep. `pub(crate)` because `crate::tests::mutation_lock_is_first_await` needs
/// it too; `cfg(test)` because nothing outside the suite may ever reach the static.
#[cfg(test)]
pub(crate) fn registration_is_held() -> bool {
    MUTATION_REGISTRATION.try_lock().is_err()
}
```

Then add to `mod tests` in the same file (it already has `unique_path`, which every new test must
use — the map is process-global):

```rust
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
```

`poll_once` goes in this `mod tests` too (the four-line form above).

### 2b — DoD 2/3: two spellings, one realpath, granted in call order

```rust
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
```

`tempfile` is already a `cyrup-tools` dev-dependency (`cross_registry_mutation_lock.rs` uses it).

### 2c — the vacuous test: **fix it here, not in a follow-up**

`dropping_the_acquisition_future_evicts_its_entry`
([lock.rs:374-400](../../../crates/cyrup-tools/src/lock.rs)) documents itself as dropping the
acquisition "after inserting its map entry and then parking on the held mutex". It does not:
`guard()`'s first suspension is `tokio::fs::canonicalize`, reached **before** any map entry exists,
so the `biased` `select!` drops a future that inserted nothing. Its final `!contains_key` also
asserts after `held` is already dropped, which passes even with `PendingEntry` deleted.

This belongs in **this** task, not a separate one, for three reasons: the false statement is a doc
comment describing *this task's own* mechanism; `KeyedAcquire` — the type that makes an honest
version writable at all — is new public API from this task; and a follow-up would reopen the same
twenty lines. Replace the whole test:

```rust
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
```

### 2d — repoint the stale note on `FileMutationLocks::map`

[lock.rs:74-78](../../../crates/cyrup-tools/src/lock.rs) says "*they are the only coverage of entry
eviction, the dropped-future gap and the `biased` cancel race in the workspace, because
`cyrup_core::keyed_lock` carries no tests of its own*". Step 1 ends that. Replace the trailing clause
with: `— `cyrup_core::keyed_lock` now carries its own tests of the enqueue/wait split and of
eviction on a forfeited place; what stays here is the same coverage over THIS crate's keying and
error vocabulary, plus map IDENTITY, which only a per-instance handle can express.` Update the test
list in that same doc to name `a_forfeited_queue_place_evicts_its_entry` and
`the_registration_chain_spans_key_resolution`.

---

## Step 3 — `cyrup-tools`: pin "`guard()` is the first `.await`" at RUNTIME

This is the row a future edit to `write.rs`/`edit.rs` breaks silently, and source inspection will not
hold it — both files were already edited by concurrent work after this fix landed. It **can** be
asserted at runtime: with the blocking pool saturated, poll `execute` exactly once and prove that

1. it is suspended, and
2. it made **zero** `FsOps` calls, and
3. the registration chain is **held** — i.e. the suspension is inside `FileMutationLocks::guard`.

(3) is the load-bearing assertion: it fails for ANY inserted `.await`, whether or not that `.await`
touches the seam. (2) names the likeliest culprit in the failure message.

New file `crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs`, plus
`mod mutation_lock_is_first_await;` in [tests/mod.rs](../../../crates/cyrup-tools/src/tests/mod.rs)
(alphabetical, after `isolation`).

```rust
//! `write::execute` and `edit::execute` must take the per-path mutation lock as their FIRST
//! `.await` (`write.rs:108`, `edit.rs:273`).
//!
//! Ordering is only preserved if the callers REACH `FileMutationLocks::guard` in dispatch order.
//! `cyrup-agent`'s `execute_parallel` hands each body on once it has been driven to its first
//! suspension point (`exec.rs:177-181`), so an `.await` inserted ABOVE `guard()` moves the handoff
//! to that earlier point and same-path mutations are once again granted in whatever order the
//! blocking pool finishes them. Nothing else in the workspace would notice: both writes succeed,
//! both tool calls report success, and the file simply holds the wrong payload.
//!
//! Asserted at runtime rather than by reading the source, because both files are edited by other
//! work. With the runtime's ONE blocking thread occupied, `tokio::fs::canonicalize` inside
//! `FileMutationLocks::key` provably cannot complete, so the first poll of `execute` is pinned
//! inside the lock — or it is not, and this file says so.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::config::{EditOpts, WriteOpts};
use crate::lock::{FileMutationLocks, registration_is_held};
use crate::ops::local::LocalFs;
use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use crate::tools::{EditTool, WriteTool};
use cyrup_core::{
    CancelToken, EventStream, Tool, ToolCallId, ToolError, ToolUpdate, ToolUpdateSink,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Poll `f` exactly once with a no-op waker.
fn poll_once<F: std::future::Future>(f: std::pin::Pin<&mut F>) -> std::task::Poll<F::Output> {
    f.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
}

/// `LocalFs` that counts every seam call. Zero is the assertion.
struct CountingFs {
    inner: Arc<dyn FsOps>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl FsOps for CountingFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.read(path).await
    }
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.write_in_place(path, bytes).await
    }
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.access(path, mode).await
    }
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.metadata(path).await
    }
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.read_dir(path).await
    }
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.walk(root, opts)
    }
}

/// One blocking thread, occupied; poll `tool.execute` once; assert it parked inside the lock.
fn assert_first_await_is_the_mutation_lock(
    build: impl FnOnce(Arc<dyn FsOps>, Arc<FileMutationLocks>, PathBuf) -> (Arc<dyn Tool>, serde_json::Value),
    seed: impl FnOnce(&Path),
) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path());

        let calls = Arc::new(AtomicUsize::new(0));
        let fs: Arc<dyn FsOps> = Arc::new(CountingFs {
            inner: Arc::new(LocalFs),
            calls: calls.clone(),
        });
        let (tool, args) = build(fs, Arc::new(FileMutationLocks::new()), dir.path().to_path_buf());

        let (release, hold) = std::sync::mpsc::channel::<()>();
        let hog = tokio::task::spawn_blocking(move || {
            let _ = hold.recv();
        });

        let sink: ToolUpdateSink = Box::new(|_u: ToolUpdate| {});
        let mut body = std::pin::pin!(tool.execute(
            ToolCallId::from("tc-first-await"),
            args,
            CancelToken::new(),
            sink,
        ));

        assert!(
            poll_once(body.as_mut()).is_pending(),
            "the only blocking thread is occupied, so the first poll must park inside \
             `FileMutationLocks::key`'s `canonicalize`"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "an `FsOps` call was made BEFORE the mutation lock was taken. `guard()` must be the \
             first `.await` of `execute` (write.rs:108 / edit.rs:273): `execute_parallel` hands the \
             batch on at the first suspension point (exec.rs:177-181), so anything awaited above \
             `guard()` moves the handoff and same-path mutations lose dispatch order"
        );
        assert!(
            registration_is_held(),
            "the first `.await` of `execute` is NOT `FileMutationLocks::guard` — some other await \
             was inserted above it. Same-path mutations are no longer granted in the order the \
             model issued them (DoD 1/2/3); nothing else in the suite observes this"
        );

        let _ = release.send(());
        hog.await.unwrap();
        let _ = body.await;
    });
}

#[test]
fn write_takes_the_mutation_lock_before_any_other_await() {
    assert_first_await_is_the_mutation_lock(
        |fs, locks, cwd| {
            let tool: Arc<dyn Tool> = Arc::new(WriteTool::new(fs, locks, cwd, WriteOpts));
            (tool, serde_json::json!({ "path": "f.txt", "content": "hello" }))
        },
        |_dir| {},
    );
}

#[test]
fn edit_takes_the_mutation_lock_before_any_other_await() {
    assert_first_await_is_the_mutation_lock(
        |fs, locks, cwd| {
            let tool: Arc<dyn Tool> = Arc::new(EditTool::new(fs, locks, cwd, EditOpts));
            (
                tool,
                serde_json::json!({
                    "path": "f.txt",
                    "edits": [{ "oldText": "SEED", "newText": "DONE" }],
                }),
            )
        },
        |dir| std::fs::write(dir.join("f.txt"), b"SEED\n").unwrap(),
    );
}
```

`WriteOpts` and `EditOpts` are unit structs
([config.rs:202-205](../../../crates/cyrup-tools/src/config.rs)); pass them by value as shown.

---

## Step 4 — `cyrup-agent`: pin the start chain on its own

`cyrup-agent` does not depend on `cyrup-tools` and must not start to. The chain is observable
without it: a tool that records its name **before its first `.await`** observes exactly the instant
`exec.rs:177-181` orders. Add to
[`crates/cyrup-agent/src/tests/agent_loop.rs`](../../../crates/cyrup-agent/src/tests/agent_loop.rs),
immediately after `a_02_2_parallel_completion_vs_source_order` (`:256-...`), reusing that file's
existing imports:

```rust
/// R-02-016 — the parallel batch must START its bodies in SOURCE order, not just emit its events
/// in source order. `a_02_2` above pins the events; nothing pinned the bodies, and the bodies are
/// what reach `FileMutationLocks::guard` in `cyrup-tools`. Three calls, not two, so a LIFO
/// inversion is unambiguous rather than a swap.
struct StartOrderTool {
    name: String,
    params: Value,
    starts: Arc<Mutex<Vec<String>>>,
    all_started: Arc<tokio::sync::Barrier>,
}

#[async_trait::async_trait]
impl Tool for StartOrderTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // BEFORE the first `.await`: this is the instant the oneshot chain orders. `execute_parallel`
        // drives each body to its first suspension point and only then releases the next
        // (exec.rs:177-181), so this push happens under that guarantee or not at all.
        self.starts.lock().unwrap().push(self.name.clone());
        // …and the ordering must not have cost concurrency: nobody leaves until all three have
        // started. A serialized batch deadlocks here and the timeout below reports it.
        self.all_started.wait().await;
        Ok(ToolResult {
            content: vec![Content::text("ok")],
            details: None,
            terminate: false,
            ..Default::default()
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_02_2b_parallel_bodies_start_in_source_order() {
    let starts = Arc::new(Mutex::new(Vec::<String>::new()));
    let all_started = Arc::new(tokio::sync::Barrier::new(3));
    let tools: Vec<Arc<dyn Tool>> = (0..3)
        .map(|i| {
            Arc::new(StartOrderTool {
                name: format!("t{i}"),
                params: obj_schema(),
                starts: starts.clone(),
                all_started: all_started.clone(),
            }) as Arc<dyn Tool>
        })
        .collect();

    let (_faux, sf) = faux_stream_fn(vec![
        faux_assistant_message(
            vec![
                faux_tool_call("t0", json!({})),
                faux_tool_call("t1", json!({})),
                faux_tool_call("t2", json!({})),
            ],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(tools).build();

    tokio::time::timeout(Duration::from_secs(10), async {
        agent.prompt("go").await.unwrap();
        agent.wait_for_idle().await;
    })
    .await
    .expect("three concurrent bodies settle; a serialized batch deadlocks on the barrier");

    assert_eq!(
        *starts.lock().unwrap(),
        vec!["t0", "t1", "t2"],
        "parallel bodies must START in source order. RED without the oneshot chain: `tokio::spawn` \
         puts each new task in the worker's LIFO slot and pushes the slot's previous occupant to \
         the back of the run queue (tokio 1.52.3 \
         runtime/scheduler/multi_thread/worker.rs:1353-1377, polled first at :707), so an \
         unordered batch starts its LAST call first"
    );
}
```

Match the neighbours' helper names exactly (`obj_schema`, `model_ref`, `faux_stream_fn`,
`faux_assistant_message`, `faux_tool_call`, `faux_text`) — they are already in scope in that file.

---

## Step 5 — `cyrup-session-svc`: the end-to-end clause-1 proof

This is the only test that exercises all four mechanisms on the path a user hits, and it is the only
crate positioned to: `cyrup-session-svc` depends on both `cyrup-agent` and `cyrup-tools`
([Cargo.toml](../../../crates/cyrup-session-svc/Cargo.toml)) and already carries `cyrup-provider`
with `faux`. It must **not** go in `cyrup-it`: that crate is seam tests only (spawned binaries, live
wasm, real sockets) and its targets are gated OFF by `required-features = ["it"]`, so a test placed
there would not run in the merge gate at all. It must not go in `cyrup-agent` either — that would
require a new `cyrup-tools` dev-edge on a crate that deliberately has none.

New file `crates/cyrup-session-svc/src/tests/same_path_mutation_order.rs`, plus
`mod same_path_mutation_order;` in [tests/mod.rs](../../../crates/cyrup-session-svc/src/tests/mod.rs).
Model the harness on
[`read_image_auto_resize.rs`](../../../crates/cyrup-session-svc/src/tests/read_image_auto_resize.rs)
— `TempDir` cwd + agent dir, `SessionConfig::new(cwd, agent_dir)` with
`trust_override = Some(true)` and `no_extensions = true`, a two-step `FauxProvider` (the tool turn,
then a terminal `StopReason::Stop` turn), `session.prompt(..)` then `session.wait_for_idle()`.

```rust
//! DoD 1 — a turn emitting `write(p, A)` then `write(p, B)` must leave `p` containing **B**.
//!
//! The whole point of the task, on the whole path: a real `SessionBuilder::build()`, the faux
//! provider issuing both calls in ONE assistant message, `execute_parallel` spawning both bodies,
//! the real `write`/`edit` tools, and the real process-global `FileMutationLocks`. Four separate
//! mechanisms have to hold for this to pass — the oneshot start chain (`exec.rs:177-181`), the
//! `MUTATION_REGISTRATION` chain (`cyrup-tools/src/lock.rs:205-219`), `enqueue`'s never-yield
//! property (`cyrup-core/src/keyed_lock.rs:164-190`), and `guard()` being the first `.await` of
//! both tool bodies — and each is pinned individually elsewhere. This is where they are pinned
//! TOGETHER, because their composition is the user-visible guarantee and nothing else asserts it.
//!
//! No sleeps and no retries: given all four, the outcome is determined, not likely.
```

Three tests, all `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`:

1. `write_then_write_to_one_path_leaves_the_second_payload` — one assistant message with
   `write("out.txt", "A")` then `write("out.txt", "B")`. Assert **both** tool results have
   `is_error == false` (a denied or failed write must not let the content assertion pass by
   accident), then `read_to_string(cwd/"out.txt") == "B"`.
2. `three_writes_to_one_path_leave_the_last_payload` — the same with `"A"`, `"B"`, `"C"`;
   assert `"C"`. Three calls make a LIFO inversion unambiguous: a two-call batch that inverts looks
   like a swap, a three-call batch that inverts lands on `"A"`.
3. `write_then_edit_of_one_path_applies_the_edit_to_the_write` — `write("notes.txt", "L1\n")` then
   `edit("notes.txt", [{oldText: "L1", newText: "L2"}])`. This one fails LOUDLY under inversion
   rather than quietly: an edit granted first finds no `L1` and returns an error result. Assert the
   `edit` result is not an error, and that the file is `"L2\n"`.

Read the results off the transcript, as `read_image_auto_resize.rs` does:

```rust
fn tool_results(messages: &[Message]) -> Vec<(String, bool)> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::ToolResult { tool_name, is_error, .. } => {
                Some((tool_name.clone(), *is_error))
            }
            _ => None,
        })
        .collect()
}
```

and assert on it before asserting on the file, with a message naming the inversion:

```rust
    let results = tool_results(&session.messages().await);
    assert_eq!(results.len(), 2, "both calls must produce a tool result: {results:?}");
    assert!(
        results.iter().all(|(_, is_error)| !*is_error),
        "a mutation failed, so the content assertion below would prove nothing: {results:?}"
    );
    assert_eq!(
        std::fs::read_to_string(cwd.join("out.txt")).unwrap(),
        "B",
        "the model issued `write(A)` then `write(B)`; the LAST payload must survive \
         (pi file-mutation-queue.ts:5/:33/:46-49). Got `A` ⇒ the two mutations were granted in \
         the wrong order somewhere between `execute_parallel` and `KeyedLocks::enqueue`"
    );
```

---

## Step 6 — `cyrup-config`: repoint the two stale cross-references

The `biased`-`select!` rationale moved out of `KeyedLocks::guard` into `KeyedAcquire::wait`
([keyed_lock.rs:221-252](../../../crates/cyrup-core/src/keyed_lock.rs)), where it is now an
`is_cancelled()` pre-check **plus** a `biased` `select!`. Two doc comments in
[`crates/cyrup-config/src/lock.rs`](../../../crates/cyrup-config/src/lock.rs) still point at the old
home. `KeyedAcquire` is not imported there ([lock.rs:11](../../../crates/cyrup-config/src/lock.rs)),
so use the full path in the intra-doc links rather than adding an unused import.

**`:108-109`**, in `FileLock::acquire`'s doc. Replace:

```rust
    /// * Layer 1 is cancelled *in place*: [`KeyedLocks::guard`] races the token against the mutex
    ///   in a `biased` `select!` and returns having taken nothing.
```

with:

```rust
    /// * Layer 1 is cancelled *in place*: [`KeyedLocks::guard`] is
    ///   [`cyrup_core::keyed_lock::KeyedLocks::enqueue`] followed by
    ///   [`cyrup_core::keyed_lock::KeyedAcquire::wait`], and it is `wait` that handles the token —
    ///   an `is_cancelled()` pre-check that settles the already-cancelled case deterministically
    ///   BEFORE any `select!` runs, then a `biased` `select!` racing the token against the mutex.
    ///   Either way it returns having taken nothing, and the claimed queue place is evicted on the
    ///   way out.
```

**`:169-171`**, inside the layer-2 retry `select!`. Replace:

```rust
                // `biased` for the reason spelled out in `KeyedLocks::guard`: with both arms ready
                // the unbiased poll order is random, and a caller that has given up must not be
                // handed the lock on a coin flip.
```

with:

```rust
                // `biased` for the reason spelled out on `KeyedAcquire::wait`
                // (`cyrup-core/src/keyed_lock.rs`, above its `is_cancelled()` pre-check): with both
                // arms ready the unbiased poll order is random, and a caller that has given up must
                // not be handed the lock on a coin flip. Layer 1 now takes the already-cancelled
                // case with that pre-check; here `biased` is the whole guarantee, because this
                // `select!` is re-entered every tick and there is no single entry point to
                // pre-check.
```

---

## Definition of done

1. `cyrup-core/src/keyed_lock.rs` has `mod tests` with `enqueue_resolves_on_its_first_poll`,
   `places_are_granted_in_enqueue_order_not_in_wait_order` and
   `a_forfeited_place_is_released_and_its_entry_evicted`, all passing, none using a sleep, a
   `yield_now` spin, or a retry loop.
2. `keyed_lock.rs`'s `EXACTLY ONE POLL` block carries the caveat text of Step 1d verbatim in
   substance: the documented sentence is about calling `lock`; the equivalence comes from
   `batch_semaphore` pushing the waiter on the first `poll_acquire`; the coop-budget hole; the
   re-verify-on-bump instruction naming the two tests.
3. `cyrup-tools/src/lock.rs` has `registration_is_held()` (`#[cfg(test)] pub(crate)`),
   `the_registration_chain_spans_key_resolution`,
   `two_spellings_of_one_path_are_granted_in_call_order`, and
   `dropping_the_acquisition_future_evicts_its_entry` replaced by
   `a_forfeited_queue_place_evicts_its_entry` with the drop order of Step 2c. The `map` field's doc
   at `:74-78` no longer claims `keyed_lock` has no tests.
4. `crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs` exists and is declared in
   `tests/mod.rs`; both its tests pass.
5. `a_02_2b_parallel_bodies_start_in_source_order` passes in `cyrup-agent`.
6. `crates/cyrup-session-svc/src/tests/same_path_mutation_order.rs` exists, is declared, and its
   three tests pass.
7. `cyrup-config/src/lock.rs:108` and `:169` name `KeyedAcquire::wait`.
8. `cargo check --workspace --all-targets` clean; `cargo test --workspace` green with **no new
   flakes**: run the new tests 20× (`cargo test -p cyrup-tools -p cyrup-core -p cyrup-agent
   -p cyrup-session-svc <name> -- --exact` in a loop) before declaring done.
9. **RED controls, run manually and then reverted, results recorded in the exec notes** (do not
   leave them in the tree):
   - move `started_tx.send(())` above the `poll_fn` in `exec.rs` ⇒ Step 4 and Step 5 fail;
   - move `drop(registration)` above `Self::key(..).await` in `cyrup-tools/src/lock.rs` ⇒ Steps 2a
     and 2b fail;
   - replace `enqueue`'s `poll_fn` with a plain store of the unpolled future ⇒ Step 1b fails;
   - insert `tokio::task::yield_now().await;` above `write.rs:108` ⇒ Step 3 and Step 5 fail;
   - delete `PendingEntry` ⇒ Step 1c and Step 2c fail.

## Out of scope

- The coop-budget hole in caveat (1) of Step 1d. It is documented, not fixed: it is unreachable on
  the `write`/`edit` path, it degrades ordering only, and closing it would mean re-polling `enqueue`
  under a budget guard — a mechanism change on a mechanism QA passed. If a caller ever appears that
  reaches `enqueue` deep inside one task poll, that is a new task.
- Anything in the *Verified as correct* list below.

---

## Verified as correct — do not revisit

- **Step 1** [`keyed_lock.rs:164-264`](../../../crates/cyrup-core/src/keyed_lock.rs) —
  `enqueue`/`KeyedAcquire::wait` split, single `poll_fn` claim, `guard` re-expressed as
  `enqueue().await.wait().await`, `KeyedAcquire`'s `Drop` releasing `early`→`acquire`→`lock` with
  `_pending` as the LAST field, `PendingEntry` declared before `lock` in `enqueue`. `KeyedLockMap`
  and its two private mutators are byte-identical.
- **Step 1b** [`cyrup-core/src/lib.rs:34`](../../../crates/cyrup-core/src/lib.rs) — `KeyedAcquire`
  re-exported.
- **Step 2** [`cyrup-tools/src/lock.rs:41-42,:198-226`](../../../crates/cyrup-tools/src/lock.rs) —
  `MUTATION_REGISTRATION` held across `Self::key(..).await?` *and* `enqueue`, dropped before `wait`;
  the `?` path drops it too, matching pi `:46-49`. `FILE_MUTATION_LOCKS`, `Self::key`,
  `is_missing_path_error`, `MutationGuard` and `Default` untouched. No deadlock is reachable:
  `enqueue` cannot block, so the global chain is only ever held across `canonicalize`.
- **Step 3** [`exec.rs:102-183`](../../../crates/cyrup-agent/src/agent/run/tools/exec.rs) — start
  chain correct; a dropped `started_tx` (panic or abort) resolves the receiver as `Err` and the batch
  proceeds rather than stalling. `remaining`, `drop(tx)`, the `rx.recv()` loop, the source-indexed
  slots, `join_next` and `execute_sequential` are unchanged.
- **Step 4** [`write.rs:108`](../../../crates/cyrup-tools/src/tools/write.rs) and
  [`edit.rs:273`](../../../crates/cyrup-tools/src/tools/edit.rs) — `guard()` is still the first
  `.await`; everything above it is synchronous. Neither declares `execution_mode` (`write.rs:68`,
  `edit.rs:184` keep the explanatory non-override comment).
- The tokio `Mutex` was not modified anywhere — correct, it was never the defect.

## Research notes — tokio 1.52.3, read for this augmentation

Registry root: `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tokio-1.52.3/`.

- `src/sync/mutex.rs:20-22` — the FIFO paragraph. Says *"the order in which tasks call the `lock`
  method"*. Correct citation for `MUTATION_REGISTRATION`; **not** a statement about polls.
- `src/sync/mutex.rs:598-600` — `lock_owned`'s cancel-safety note: *"a queue to fairly distribute
  locks in the order they were requested"* and *"cancelling a call to `lock_owned` makes you lose
  your place in the queue"* — the second half is the contract `KeyedAcquire::drop` relies on.
- `src/sync/batch_semaphore.rs:496-517` — `poll_acquire` registers the waker and, under `if !queued`,
  runs `waiters.queue.push_front(node)` before returning `Pending`. **This is what makes first-poll
  placement true.**
- `src/sync/batch_semaphore.rs:598` — `let coop = ready!(crate::task::coop::poll_proceed(cx));`,
  ahead of `poll_acquire` at `:600`. The budget hole.
- `src/sync/batch_semaphore.rs:600-604` — `Poll::Pending => { *queued = true; ... }`, so a re-poll
  keeps the place rather than taking a second one.
- `src/sync/batch_semaphore.rs:306-330` — `add_permits_locked`: `waiters.queue.last()` →
  `assign_permits` → `queue.pop_back()`. push_front + pop_back = FIFO, and the permit is ASSIGNED
  into the waiter's node, which is why Step 1b's "poll the second, it is still Pending" assertion is
  a decision already taken rather than a race.
- `src/sync/batch_semaphore.rs:551-571` (`Waiter::assign_permits`) — returns
  `next == 0`, so an assigned waiter's later `poll_acquire` computes `needed == 0` and returns
  `Ready` immediately.
- `src/sync/batch_semaphore.rs:686-709` — `Drop for Acquire`: removes the node from the list and
  returns any assigned permits. Step 1c/2c depend on it.
- `src/task/coop/mod.rs:115-116` — `Budget::initial()` is `128`.
- `src/task/coop/mod.rs:343-362` — `poll_proceed` returns `Pending` (registering the waker) when the
  budget is spent.
- `src/runtime/scheduler/multi_thread/worker.rs:675` — `coop::budget(|| { task.run(); … })`, with the
  comment that LIFO-slot tasks polled inside the scope *"inherit the 'parent''s limits"*. This is why
  the budget is per task-poll-chain, not per `.await`.
- `src/runtime/scheduler/multi_thread/worker.rs:1353-1377` and `:707` — the LIFO slot, already cited
  in `exec.rs:120-126`; re-checked and still accurate at 1.52.3.
