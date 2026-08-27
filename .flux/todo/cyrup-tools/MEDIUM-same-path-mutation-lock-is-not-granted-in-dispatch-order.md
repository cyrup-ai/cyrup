---
title: Same-path mutation lock is not granted in dispatch order
priority: MEDIUM
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: needs-rework
updated: 2026-08-27 14:11
---

# Same-path mutation lock is not granted in dispatch order

QA reviewed the landed implementation. **The mechanism is correct and complete** — all three
ordering points are closed, `KeyedLockMap`'s invariants are intact, the mutex was (correctly) not
touched, and `write.rs`/`edit.rs` still keep `guard()` as their first `.await`. What is missing is
verification and one documentation clause. Only the items below remain.

---

## 1. The task's central behaviour has no automated coverage

DoD clauses 1, 2 and 3 — "a turn emitting `write(p, A)` then `write(p, B)` leaves `p` containing
**B**" — are the entire point of the task and nothing in the workspace asserts them. Three separate,
individually silent-failure-prone mechanisms now carry that guarantee and none is pinned:

| Mechanism | Where | What silently reverts it |
|---|---|---|
| `oneshot` start chain | `crates/cyrup-agent/src/agent/run/tools/exec.rs`:106, :114-115, :127-129, :177-181 | deleting `wait_turn`/`started_tx`, or moving the `started_tx.send(())` before the first `poll_fn` |
| `MUTATION_REGISTRATION` held across key resolution + claim | `crates/cyrup-tools/src/lock.rs`:41-42, :205-219 | moving `drop(registration)` above `Self::key(..).await`, or dropping the static |
| `enqueue` never yields | `crates/cyrup-core/src/keyed_lock.rs`:164-190 | adding any `.await` inside `enqueue` — the registration lock would then be released before the claim |
| `guard()` is the first `.await` of `execute` | `write.rs`:108, `edit.rs`:273 | inserting any `.await` above it (both files were edited by concurrent work AFTER this fix landed, with nothing to catch it) |

`crates/cyrup-agent/src/tests/agent_loop.rs` has no start-order test
(`a_02_2_parallel_completion_vs_source_order` asserts concurrency, `agent_002_parallel_defers_…`
asserts the prepare/start split — neither observes which body starts first), and
`crates/cyrup-tools/src/lock.rs`'s module tests cover exclusion, eviction and keying only.

Add, at minimum:

1. **End-to-end, `crates/cyrup-agent/src/tests/`** (multi-thread runtime, `worker_threads >= 2`): one
   assistant message emitting `write(p, "A")` then `write(p, "B")` through `execute_parallel`;
   assert the file contains `B`. Repeat with `write` then `edit`, and with a batch of 3+ so the LIFO
   inversion is unambiguous. This is the only clause-1 test and it is RED without Step 3.
2. **Path-spelling, `crates/cyrup-tools/src/lock.rs`** (DoD 2/3): two `guard()` calls issued in order
   on two *different spellings* of one realpath — e.g. absolute vs. a symlink to it — where the
   first spelling's `canonicalize` is the slower one. Assert grant order matches call order. Without
   `MUTATION_REGISTRATION` this is a blocking-pool coin flip.
3. **Unit, `crates/cyrup-core/src/keyed_lock.rs`** (the never-yield property): `enqueue` two
   acquisitions for one key in order, assert the second is not granted until the first's guard drops
   and that they are granted in `enqueue` order. Pins the one-poll claim directly rather than
   through two crates. `keyed_lock` currently carries no tests of its own — see the note at
   `crates/cyrup-tools/src/lock.rs`:74-78 — and `enqueue`/`KeyedAcquire` are new public API.

## 2. The load-bearing uncertainty is asserted as fact, not flagged

`crates/cyrup-core/src/keyed_lock.rs`:173-177 states first-poll queue placement as settled, with
`tokio 1.52.3 src/sync/mutex.rs:20-22, :598-600` cited. Those lines say *"the order in which tasks
call the `lock` method"* — they do **not** say "the order acquire futures are first polled". The two
coincide, but only by reading `batch_semaphore`, which is what this task's own *Genuinely uncertain*
section singled out as the single property resting on implementation-reading rather than contract.

The comment must record that, so a tokio upgrade is not a silent correctness change. Add to the
`EXACTLY ONE POLL` block: that the documented sentence is about calling `lock`, that the equivalence
comes from `batch_semaphore` pushing the waiter on the first `poll_acquire`, and that a tokio bump
must re-verify it. Pairing this with test 3 above is what turns the caveat into something a CI run
can catch.

## 3. Stale cross-references left behind in `cyrup-config`

The `biased`-select! rationale moved out of `KeyedLocks::guard` into `KeyedAcquire::wait`
(`keyed_lock.rs`:223-231), where it is now a `cancel.is_cancelled()` pre-check plus a `biased`
`select!`. Two doc comments still point at the old home:

- `crates/cyrup-config/src/lock.rs`:108 — "`KeyedLocks::guard` races the token against the mutex in
  a `biased` `select!`" — now one hop removed and no longer the whole story (the pre-check makes the
  already-cancelled case deterministic *before* any select).
- `crates/cyrup-config/src/lock.rs`:169 — "`biased` for the reason spelled out in
  `KeyedLocks::guard`" — that reason is now spelled out in `KeyedAcquire::wait`.

Repoint both.

## 4. Observation — a pre-existing test is now vacuous (not caused by this task)

`crates/cyrup-tools/src/lock.rs`:377-400
`dropping_the_acquisition_future_evicts_its_entry` documents itself as dropping the acquisition
"after inserting its map entry and then parking on the held mutex". It does not: `guard()`'s first
suspension is `tokio::fs::canonicalize` (via `spawn_blocking`), which is reached **before** any map
entry exists, so the `biased` select drops the future with nothing inserted and the final
`!contains_key` assertion passes for the wrong reason. This was equally true of the pre-change
`guard()`, so it is not a regression — but the dropped-future half of DoD 6 is therefore untested.
Drive the drop from a point after `enqueue` has returned (e.g. hold a `KeyedAcquire` from
`KeyedLocks::enqueue` directly and drop it) to restore the intent.

---

## Verified as correct — do not revisit

- **Step 1** `crates/cyrup-core/src/keyed_lock.rs`:164-264 — `enqueue`/`KeyedAcquire::wait` split,
  single `poll_fn` claim, `guard` re-expressed as `enqueue().await.wait().await`, `KeyedAcquire`'s
  `Drop` releasing `early`→`acquire`→`lock` with `_pending` as the LAST field, `PendingEntry`
  declared before `lock` in `enqueue`. `KeyedLockMap` and its two private mutators are byte-identical.
- **Step 1b** `crates/cyrup-core/src/lib.rs`:34 — `KeyedAcquire` re-exported.
- **Step 2** `crates/cyrup-tools/src/lock.rs`:41-42, :198-226 — `MUTATION_REGISTRATION` held across
  `Self::key(..).await?` *and* `enqueue`, dropped before `wait`; the `?` path drops it too, matching
  pi `:46-49`. `FILE_MUTATION_LOCKS`, `Self::key`, `is_missing_path_error`, `MutationGuard` and
  `Default` untouched. No deadlock is reachable: `enqueue` cannot block, so the global chain is only
  ever held across `canonicalize`.
- **Step 3** `crates/cyrup-agent/src/agent/run/tools/exec.rs`:102-183 — start chain correct; a
  dropped `started_tx` (panic or abort) resolves the receiver as `Err` and the batch proceeds rather
  than stalling. `remaining`, `drop(tx)`, the `rx.recv()` loop, the source-indexed slots,
  `join_next` and `execute_sequential` are unchanged.
- **Step 4** `crates/cyrup-tools/src/tools/write.rs`:108 and `edit.rs`:273 — `guard()` is still the
  first `.await`; everything above it is synchronous. Neither declares `execution_mode`
  (`write.rs`:68, `edit.rs`:184 keep the explanatory non-override comment).
- The tokio `Mutex` was not modified anywhere — correct, it was never the defect.
