---
title: Same-path mutation lock is not granted in dispatch order
priority: MEDIUM
tool: write
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: qa
status: completed
updated: 2026-08-27 21:56
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
| --- | --- |
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

## 2 — A THIRD stale cross-reference in `cyrup-config/src/lock.rs`

Step 6 named two (`:108`, `:169`); both are correctly repointed. A third, of the same class, was
missed and is now factually wrong on two counts — `crates/cyrup-config/src/lock.rs:26-31`, the
`NEVER_CANCELLED` doc:

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

## RESOLUTION (both items closed)

Item 1 was delegated and is fixed: the witness is now a `#[cfg(test)]` per-instance
`guard_entries: AtomicUsize` on `FileMutationLocks`, incremented as the FIRST statement of
`guard`'s body — above `MUTATION_REGISTRATION.lock().await`, which matters, because bumping it
after that await (as option 2 above proposed) would have parked the poll with the counter still at
zero and re-broken the guard. Measured under the full lib suite at default parallelism with plain
`cargo test`: RED 20/20 with `yield_now()` above `write.rs`'s `guard()`, RED 20/20 above
`edit.rs`'s, GREEN 20/20 with both reverted.

Item 2 is fixed: the `NEVER_CANCELLED` doc now names
`cyrup_core::keyed_lock::KeyedAcquire::wait` and states that `cancelled()` is polled ZERO times on
the uncontended path, because `wait` returns through its `early.take()` arm before the `select!` is
constructed, and that the `is_cancelled()` guard is a synchronous read registering no waiter.

The retained `[`KeyedLocks::guard`]` reference at `cyrup-config/src/lock.rs:111` was reviewed and
kept deliberately: that bullet attributes token handling to `wait` and names `guard` only as the
composed entry point, which is the function this crate actually calls.

## Verified as correct — do not revisit

- **DoD 1-8** all verified across `cyrup-core`, `cyrup-tools`, `cyrup-agent`, `cyrup-config` and
  `cyrup-session-svc`: eight new tests, deterministic by construction via `poll_once` over
  `Waker::noop()` and a runtime whose single blocking thread is occupied; the tokio caveat naming
  `poll_acquire`'s `push_front` and both silent-degradation paths; the vacuous eviction test gone
  workspace-wide with every eviction test dropping `held` first.
- `write.rs` and `edit.rs` — `guard()` is still the first `.await` of `execute`; everything above
  it in both bodies is synchronous.
- The mechanism itself: `KeyedLocks::enqueue`/`KeyedAcquire::wait`, `PendingEntry` drop ordering,
  `MUTATION_REGISTRATION` held across `Self::key(..).await?` AND `enqueue` and dropped before
  `wait`, and the `exec.rs` oneshot start chain. All unchanged and correct.
