---
title: Same-path mutation lock is not granted in dispatch order
priority: MEDIUM
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: needs-rework
updated: 2026-08-27 20:10
---


> **DELEGATION NOTE (orchestrator, post-QA).** QA was right that this task owns the
> degraded Step-3 guard rather than a follow-up. The executable brief for that fix now
> lives in `MEDIUM-mutation-lock-first-await-guard-degrades-under-the-full-suite.md`,
> which was augmented in parallel and carries the research: a `#[cfg(test)]` per-instance
> `guard_entries: AtomicUsize` on `FileMutationLocks`, incremented as the first statement
> of `guard`'s body.
>
> **Run that task, not this item — they target the same twenty lines and must not both
> execute.** This task's remaining independent work is the stale cross-references,
> including the third one QA found at `cyrup-config/src/lock.rs:26-31`.
>
> That augmentation also disproved QA's own argument 4: the module-local-mutex fallback
> cannot reach 20/20, because ~118 test fns across 7 files contend on
> `MUTATION_REGISTRATION` — which is why detection fell from 28/30 with the sibling alone
> to 2/3 under the full suite. The real fix does need a (cfg-gated) production edit.

# Same-path mutation lock is not granted in dispatch order

QA re-reviewed the rework. **The mechanism was already correct and remains so; the verification
work is very nearly complete.** Eight new tests across four crates, all deterministic by
construction, no sleeps, no spins, no retry loops. The tokio caveat is present, accurate and
honest. The vacuous eviction test is gone from both crates and every eviction test now drops `held`
FIRST. The two named stale cross-references are repointed.

**One DoD clause is unmet, and it is the one on the highest-risk row of this task's own coverage
map.** Two items below. Do not revisit anything else — everything not listed here is verified
correct and closed.

---

## 1 — Step 3's pin DEGRADES instead of failing (the coverage map's own criterion)

This task's coverage map states the bar for every row:

> Each row must end pinned by a named test that **fails — not degrades** — when that row is
> reverted.

The row `guard()` is the first `.await` of `execute` does not meet it.

`crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs:121-126` — the load-bearing
assertion, the one that fires for ANY inserted `.await` — is:

```rust
assert!(registration_is_held(), ...);
```

`registration_is_held()` (`crates/cyrup-tools/src/lock.rs:49-52`) is a `try_lock` on the
**process-global** `MUTATION_REGISTRATION`. Two siblings in the same lib-test binary legitimately
hold that chain across their own `canonicalize`:
`the_registration_chain_spans_key_resolution` (`lock.rs:277`) briefly, and
`two_spellings_of_one_path_are_granted_in_call_order` (`lock.rs:344`) for 24 rounds × two
30-hop `realpath` resolutions. Either can make the observer read "held" for a reason that has
nothing to do with the future under test.

Measured with `tokio::task::yield_now().await;` inserted above `write.rs:108` — DoD 9's own RED
control for this row:

| condition | detection |
|---|---|
| alone | 20/20 |
| with its sibling | 28/30 |
| full `cyrup-tools` lib suite — **the merge-gate condition** | **2/3** |

So DoD 9's clause *"insert `tokio::task::yield_now().await;` above `write.rs:108` ⇒ Step 3 and
Step 5 fail"* does not hold as written. Assertion (2), `calls == 0`, cannot compensate: a bare
`yield_now()` makes no `FsOps` call either. Step 5 does not compensate either — under that lever
all bodies are released at the yield and the surviving payload becomes a race, so Step 5 detects
probabilistically as well. This row's only intended deterministic pin is the one that degrades, and
it is the row this task singled out as *"both files were edited by concurrent work AFTER this fix
landed, with nothing to catch it."*

**GREEN is unaffected** (20/20 deterministic), so this is not a merge hazard — it is an unbuilt pin.

Note the same `try_lock`-a-process-global shape appears in
`the_registration_chain_spans_key_resolution`, and there it is self-cancelling: under ITS RED lever
(`drop(registration)` above `Self::key(..).await`) the wide-window sibling stops holding the chain
across `canonicalize` too, so the contamination disappears exactly when the control runs. That is
why only Step 3 is listed here.

### What to do — pick ONE, then re-measure

1. **Preferred, and it needs no production change:** serialize the registration-chain-sampling tests
   behind a module-local `static std::sync::Mutex<()>` held for the sampling window, so no sibling
   can be parked in the chain while this test reads it. Both `lock.rs` tests that hold the chain
   across `canonicalize` and both tests in `mutation_lock_is_first_await.rs` take it.
2. **A non-global observer.** Give `FileMutationLocks` a `#[cfg(test)]` per-instance entry counter
   bumped immediately after `MUTATION_REGISTRATION.lock().await` in `guard()`; the test owns the
   instance it hands to the tool, snapshots the counter, polls once, and asserts it moved by exactly
   1. Sibling-immune by construction. Costs a `#[cfg(test)]` field and one `fetch_add`.

Do not weaken or delete the assertion. Do not add a sleep, a spin or a retry.

**Done when:** with `yield_now()` above `write.rs:108`, both
`write_takes_the_mutation_lock_before_any_other_await` and
`edit_takes_the_mutation_lock_before_any_other_await` fail **20/20 under the full `cyrup-tools`
lib suite**, not just in isolation; with the lever reverted, both pass 20/20 under the same
conditions; and the RED/GREEN counts are recorded in the exec notes (DoD 9 asked for that and
nothing in the tree records it).

`/home/user/cyrup/.flux/todo/cyrup-tools/MEDIUM-mutation-lock-first-await-guard-degrades-under-the-full-suite.md`
was filed for this and is an accurate write-up; as of this review it has already moved to
`stage: aug`. It is **the same twenty lines this task created**, and this task's own Step 2c argued
that a defect in this task's own new test belongs in this task rather than a follow-up "because a
follow-up would reopen the same twenty lines" — which is why it is booked here as an unmet clause.
Either fold it back in and retire that file, or let it run to completion and close this task on its
landing; do not do both independently against the same twenty lines.

## 2 — A THIRD stale cross-reference in `cyrup-config/src/lock.rs`

Step 6 named two (`:108`, `:169`); both are correctly repointed. A third, of the same class, was
missed and is now factually wrong on two counts — `crates/cyrup-config/src/lock.rs:26-31`, the
`NEVER_CANCELLED` doc:

```rust
/// ... so both `select!`s that consume it —
/// [`KeyedLocks::guard`]'s and the layer-2 retry loop below — always take their other arm. It is
/// polled once per acquire, in `guard`'s biased first branch, and not again on the uncontended
/// path because the retry loop is skipped.
```

1. The `select!` moved to `KeyedAcquire::wait` (`cyrup-core/src/keyed_lock.rs:277-281`), same as
   the two already repointed.
2. "polled once per acquire" is no longer true at all. `wait` now returns through
   `self.early.take() => Some(g)` on the uncontended path — which is the config-lock common case —
   without ever constructing or polling `cancelled()`. The `is_cancelled()` pre-check above it
   (`keyed_lock.rs:267`) is a plain sync read (`cyrup-core/src/cancel.rs:38`), not a poll, and
   registers no waiter.

Rewrite the clause to name `KeyedAcquire::wait` and to state the true cost: the `cancelled()`
future is polled only when layer 1 is contended, and the `is_cancelled()` pre-check is a sync read
that allocates and registers nothing.

---

## Verified as correct — do not revisit

- **DoD 1** `cyrup-core/src/keyed_lock.rs:351-458` — `mod tests` with all three named tests.
  `poll_once` over `Waker::noop()`; no sleep, no `yield_now`, no retry loop.
  `places_are_granted_in_enqueue_order_not_in_wait_order` enqueues in one order and waits in the
  reverse, and its post-release "second is still Pending / first is Ready" pair is a decision
  already taken (`assign_permits` banks the permit in the tail node), not a race.
  `a_forfeited_place_is_released_and_its_entry_evicted` drops `held` FIRST, so the surviving entry
  rests on the forfeited place's own reference and only `KeyedAcquire::drop` + `PendingEntry::drop`
  can evict it — non-vacuous.
- **DoD 2** `keyed_lock.rs:175-215` — the caveat is present and honest. It states that what tokio
  DOCUMENTS is the order tasks **call** `lock` (`mutex.rs:20-22`, `:598-600`) and that neither
  sentence is about polls; that first-poll placement rests on `poll_acquire`'s
  `waiters.queue.push_front(node)` under `!queued` (`batch_semaphore.rs:496-517`) plus
  `queued = true` latching (`:600-604`) plus `add_permits_locked`'s `assign_permits` +
  `queue.pop_back()` (`:306-330`); it names BOTH silent-degradation paths (the `coop::poll_proceed`
  budget hole at `:598`, and tokio moving placement off the first poll); it carries the
  re-verify-on-bump instruction and names the two tests that catch it. The `enqueue` doc at
  `:156-159` carries the pointer clause. `cyrup-tools/src/lock.rs:32-37` carries the
  disambiguating "this is the documented case" clause so the two citations cannot be conflated.
- **DoD 3** `cyrup-tools/src/lock.rs` — `registration_is_held()` is `#[cfg(test)] pub(crate)`
  (`:49-52`); `the_registration_chain_spans_key_resolution` (`:277`) is a hand-built runtime with
  `max_blocking_threads(1)` whose one thread is occupied by a `spawn_blocking` hog submitted
  FIRST, so the FIFO blocking queue makes `canonicalize` provably unable to complete — the park
  inside `Self::key` is a certainty, not a likelihood; `two_spellings_of_one_path_are_granted_in_call_order`
  (`:344`) drives A to its first suspension with the `exec.rs:177-181` shape before releasing B, so
  GREEN is structural and the `ROUNDS` loop is a RED-strength lever only, not a retry;
  `dropping_the_acquisition_future_evicts_its_entry` is gone workspace-wide and
  `a_forfeited_queue_place_evicts_its_entry` (`:558`) replaces it with `drop(held)` first. The
  `map` field doc (`:82-91`) no longer claims `keyed_lock` has no tests and names both new tests.
- **DoD 4** `crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs` exists and is declared at
  `tests/mod.rs:11` (alphabetical). Its runtime construction and `CountingFs` seam are correct;
  only assertion (3)'s observer is at issue — item 1 above.
- **DoD 5** `a_02_2b_parallel_bodies_start_in_source_order` (`cyrup-agent/src/tests/agent_loop.rs:387`)
  with `StartOrderTool` (`:348`) recording its name BEFORE its first `.await` and a
  `Barrier::new(3)` proving the ordering cost no concurrency. Three calls, so a LIFO inversion is
  unambiguous. No sleep; the `timeout` is a deadlock reporter, not a sequencing device.
- **DoD 6** `crates/cyrup-session-svc/src/tests/same_path_mutation_order.rs` exists, is declared at
  `tests/mod.rs:43`, and carries all three tests. Each asserts `is_error == false` on every tool
  result BEFORE asserting on file content, so a denied write cannot let the content assertion pass
  by accident. No sleeps, no retries.
- **DoD 7** `cyrup-config/src/lock.rs:108-115` and `:173-179` both name `KeyedAcquire::wait` and
  describe the `is_cancelled()` pre-check plus the `biased` `select!` accurately. (The third,
  unnamed reference is item 2 above.)
- **DoD 8** `cargo check --workspace --all-targets` clean; 3947 tests across 9 crates, zero
  failures.
- `write.rs:108` and `edit.rs:273` — `guard()` is still the first `.await` of `execute`;
  everything above it in both bodies is synchronous.
- The mechanism itself: `KeyedLocks::enqueue`/`KeyedAcquire::wait`, `PendingEntry` drop ordering,
  `MUTATION_REGISTRATION` held across `Self::key(..).await?` AND `enqueue` and dropped before
  `wait`, and the `exec.rs` oneshot start chain. All unchanged and correct.
