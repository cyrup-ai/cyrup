---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_14 — async retention, part B: sweep, report, wiring

OBJECTIVE: complete the async-root reaper — execute the sweep, report its outcome, and schedule it.
This is the task that turns SCOPE_13's module on.

**Depends on SCOPE_13.**

## SUBTASK1 — `async_retention/sweep.rs`

Drives `scan.rs` → `policy.rs` → deletion, in batches of `ASYNC_RETENTION_BATCH_SIZE` separated by
`ASYNC_RETENTION_DELAY_MS`. The delay is between **batches**, not between files — a 60 s pause per
file would never finish; a 60 s pause per 100-run batch is the load-shedding upstream intends.

**Deletion order matters.** A run tree's own `status.json` is the last thing removed, so a crash
mid-delete leaves a directory that still reads as a run and gets re-evaluated next sweep, rather
than a half-deleted tree that reads as corrupt.

## SUBTASK2 — `async_retention/report.rs`

`AsyncRetentionResult` (`async-retention.ts:103`) — what was scanned, kept, tombstoned, reaped, and
why. Surfaced through the existing diagnostics/doctor path (`registration/doctor.rs`) rather than a
new user-facing surface.

## SUBTASK3 — schedule it

**Where:** `extension/executor/notices.rs`, beside the existing post-install detached schedules
(`cleanup_result_indexes`, and SCOPE_4's `cleanup_completion_replay_if_due`).

**Change:** schedule the sweep detached, after install, on the retention delay. Three cleanup jobs
now share that seam — factor them into one scheduling helper rather than a third ad-hoc `tokio::spawn`.

**Why a shared helper:** three independent detached tasks racing the same roots at session start is
exactly the I/O storm that `CLAUDE.md` records as perturbing file watchers and socket timeouts. One
scheduler, staggered.

## SUBTASK4 — never reap a live run

The sweep consults the live `JobTracker` (`background/tracker.rs`) before reaping. A run this
process is tracking is never a candidate regardless of age or on-disk state.

**Why:** `policy.rs` has the liveness input but SCOPE_13 left it unwired; this is where the real
tracker is threaded in.

## Tests

| test | pins |
|---|---|
| `the_sweep_reaps_only_what_the_policy_selects` | integration of 13's parts |
| `the_sweep_processes_in_batches` | `ASYNC_RETENTION_BATCH_SIZE` |
| `the_sweep_pauses_between_batches_not_between_files` | the load-shedding shape — an injected clock, asserting pause count |
| `a_run_tracked_by_the_live_tracker_is_never_reaped` | SUBTASK4 — the dangerous case |
| `status_json_is_removed_last` | SUBTASK1's crash-safety ordering |
| `a_crash_mid_sweep_leaves_a_re_evaluable_tree` | drop the sweep partway, re-run, assert convergence |
| `the_report_accounts_for_every_candidate` | scanned == kept + tombstoned + reaped |
| `the_sweep_never_touches_the_results_dir` | it reaps async run trees, not payloads |
| `the_three_cleanup_jobs_are_staggered` | SUBTASK3 — no simultaneous start |

## Benchmarks

**One, and it is the reason this task owns it.** A `criterion` benchmark over a synthetic async
root of 1,000 run directories, asserting the sweep's *per-batch* wall time stays bounded — the
property that matters is that a large root does not stall a live session, not raw throughput.

Place it under `crates/cyrup-ext-subagents/benches/`. If the workspace has no `benches/` convention
yet, state that in the task's completion notes rather than inventing one silently.

## Definition of done

- The sweep reaps orphaned run trees on the 30-day policy, batched and paused between batches.
- A live-tracked run is never reaped.
- `status.json` is removed last; an interrupted sweep converges on re-run.
- The report accounts for every candidate.
- Three cleanup jobs share one staggered scheduler.
- The 340 leaked directories measured at diagnosis are reapable — verify against a synthetic root
  of that shape, not against the real one.
- Benchmark exists and its per-batch bound holds.
- Workspace `--no-fail-fast` 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/async-retention.ts:103` (`AsyncRetentionResult`) and
  the sweep body.
* SCOPE_13 built `policy.rs`, `scan.rs`, `tombstone.rs`; this task adds `sweep.rs`, `report.rs`.
* Scheduling seam: `extension/executor/notices.rs` — the post-install detached schedule that already
  drives `result_index::cleanup_result_indexes`.
* Live-run source: `background/tracker.rs` (`JobTracker::snapshot` / `jobs.keys()`).
* `CLAUDE.md`'s note on concurrent I/O perturbing watchers and socket timeouts is the reason for
  staggering rather than firing three cleanups at once.
