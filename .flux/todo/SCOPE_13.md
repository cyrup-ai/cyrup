---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_13 — async retention, part A: policy, scanning, tombstones

OBJECTIVE: build the first half of the async-root reaper — the data model, the retention policy,
the tree scan, and tombstones. **Unwired**: this task lands a complete, tested module that nothing
calls yet. SCOPE_14 turns it on.

Upstream `background/async-retention.ts` is 912 LOC; splitting it in two keeps each half landable
green in one session, with part A having no behavioural effect at all.

## Why this is in scope despite having no session dimension

`async-retention.ts` contains **zero** `sessionId` references — it is the one item in this
programme that is not session-scoped. It is included because the directory it reaps is **shared**:
`<temp_root>/async/<cwd_key>` is resolved identically by every cyrup instance in a directory
(`background/artifact_roots.rs:281-284`), and orphaned run trees accumulate there without bound.
Diagnosis measured **340** per-cwd result directories. Say this in the module docs — do not imply a
session justification the file does not have.

Distinct from `result_index::cleanup_result_indexes`, which sweeps only `result-index/` and never
touches a payload or a run tree.

## SUBTASK1 — constants and options

**Where:** new module `crates/cyrup-ext-subagents/src/background/async_retention/`

```ts
ASYNC_RETENTION_DAYS = 30                    // :14
ASYNC_RETENTION_BATCH_SIZE = 100             // :15
ASYNC_RETENTION_DELAY_MS = 60_000            // :16
ASYNC_RETENTION_TOMBSTONE_GRACE_MS = 24h     // :17
interface AsyncRetentionOptions              // :81
interface AsyncRetentionResult               // :103
```

The batch size and delay are **load-shedding**, not tuning: the reaper must never stall a live
session. The 24 h tombstone grace exists so a run being reconciled concurrently is not reaped
mid-flight. Both are correctness properties; port the numbers and say why in the docs.

## SUBTASK2 — `policy.rs`, the retention decision

A pure function: given a run directory's facts (age, terminal state, tombstone presence, whether it
is still tracked), decide reap / keep / tombstone. **No I/O** — same functional-core discipline as
`delivery::DeliveryDisposition`, and for the same reason: this is the decision most likely to be
wrong, and it should be exhaustively testable without a filesystem.

Model the outcome as an enum, not a bool. A two-state answer here would lose the tombstone case,
which is exactly the mistake `DeliveryDisposition` documents.

## SUBTASK3 — `scan.rs`, the batched tree walk

Walks the async root in batches of `ASYNC_RETENTION_BATCH_SIZE`, yielding candidates with the facts
`policy.rs` needs. Reuses `result_index::errno`'s classification — missing/permission-denied
directories are skipped, a genuine fault propagates.

## SUBTASK4 — `tombstone.rs`

Writing, reading and expiring tombstones on the `ASYNC_RETENTION_TOMBSTONE_GRACE_MS` ladder.

**Layout so far:** `async_retention/{mod,policy,scan,tombstone}.rs`. SCOPE_14 adds `sweep.rs` and
`report.rs`.

## Tests

All of `policy.rs`'s tests are pure — no tempdir, no tokio:

| test | pins |
|---|---|
| `a_run_younger_than_the_window_is_kept` | 30-day policy |
| `an_orphaned_terminal_run_past_the_window_is_reaped` | the happy path |
| `a_still_tracked_run_is_never_reaped_regardless_of_age` | the liveness guard |
| `a_non_terminal_run_is_never_reaped` | the state guard |
| `a_tombstoned_run_inside_the_grace_is_kept` | 24 h grace — the concurrent-reconciliation case |
| `a_tombstoned_run_past_the_grace_is_reaped` | grace expiry |
| `the_policy_has_three_outcomes_not_two` | tombstone is not collapsible into keep/reap |

Filesystem-backed:

| test | pins |
|---|---|
| `the_scan_yields_at_most_a_batch_at_a_time` | `ASYNC_RETENTION_BATCH_SIZE` |
| `the_scan_skips_an_unreadable_directory` | `errno` reuse |
| `the_scan_propagates_a_genuine_io_fault` | the other half of `errno`'s policy |
| `a_tombstone_round_trips` | `tombstone.rs` |

## Benchmarks

None **in this task**. The reaper is explicitly load-shed by batch size and delay rather than
optimised for throughput; if a performance question arises it belongs with SCOPE_14, which owns the
sweep that actually does I/O at scale.

## Definition of done

- `policy.rs` is pure, returns a three-outcome enum, and has the full table above passing with no
  filesystem.
- `scan.rs` batches and reuses `result_index::errno`.
- `tombstone.rs` round-trips and honours the grace.
- The module compiles and is fully tested but is **called by nothing** — `grep -rn async_retention
  crates/ --include=*.rs | grep -v async_retention/` returns only the `mod` declaration.
- Module docs state the no-session-dimension justification (shared directory, unbounded growth,
  340 dirs measured) rather than implying session scoping.
- Workspace `--no-fail-fast` 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/async-retention.ts` (912 LOC, HEAD `7fe9dee1`).
* The shared-root fact: `background/artifact_roots.rs:281-284` plus its doc note added by the
  session-scoping work.
* Functional-core precedent to copy: `background/delivery/disposition.rs` (pure classifier, enum
  outcome, exhaustive table with no I/O).
* Error-policy module to reuse: `background/result_index/errno.rs` (named `io_class.rs` until
  SCOPE_1 renames it — check which is present).
* Do **not** confuse with `result_index::cleanup_result_indexes` (24 h, `result-index/` only).
