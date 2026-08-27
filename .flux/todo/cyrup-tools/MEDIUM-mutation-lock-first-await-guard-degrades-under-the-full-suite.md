---
stage: new
status: pending
priority: MEDIUM
tool: all
source: exec follow-up — self-reported by the mutation-lock executor
updated: 2026-08-27 15:45
---

# The `guard()`-is-first-`.await` guard degrades instead of failing

## What is wrong

`crates/cyrup-tools/src/tests/mutation_lock_is_first_await.rs` exists to fail if
anyone inserts an `.await` above `guard()` in `write.rs` / `edit.rs`. Its
assertion (3) calls `lock::registration_is_held()`, which is a `try_lock` on the
**process-global** `MUTATION_REGISTRATION` mutex.

The sibling test `edit_takes_the_mutation_lock_before_any_other_await` *holds*
that same chain while parked on the hogged blocking thread. So the observer can
read "held" for a reason that has nothing to do with the code under test.

Measured by the executor, with a `yield_now()` inserted above `write.rs`'s
`guard()` as the RED lever:

| condition | detection |
|---|---|
| test run alone | 20/20 |
| run with its sibling | 28/30 |
| full `cyrup-tools` lib suite — **the merge-gate condition** | **2/3** |

So under the condition that actually matters, the guard **degrades** rather than
fails. The brief's own coverage-map criterion was "fails — not degrade", and the
brief had already flagged exactly this `try_lock`-against-a-process-global hazard
for a different step, then used it here anyway.

**GREEN is unaffected** — the test passes 20/20 deterministically. This is purely
about how reliably it catches the regression it exists to catch.

Assertion (2) (`calls == 0`) does not compensate: a bare `yield_now()` makes no
`FsOps` call either, so it is invisible to the counter.

## Parity action

Pick ONE.

1. **Give the assertion a non-global observer.** Preferred — it removes the
   coupling instead of scheduling around it.
2. Serialize the two tests behind a module-local mutex, so the sibling cannot be
   parked in the chain while this one samples it.

Do not weaken the assertion or delete it; the property it guards is real and is
the one thing standing between a future refactor and a silent reintroduction of
the original bug.

## Related

This is the same class of defect as
`MEDIUM-permission-system-extension-tests-flake-under-parallel-execution.md`:
process-global state sampled by a test that does not control every writer. Worth
fixing with the same eye.

## Definition of done

1. With a `yield_now()` inserted above `write.rs`'s `guard()`, the test fails
   **20/20** under the full `cyrup-tools` lib suite, not just in isolation.
2. With that lever reverted, it passes 20/20 under the same conditions.
3. No production behaviour changes.
