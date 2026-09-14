---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_10 — async status snapshot + S6 live workflow controls

> Renamed 2026-09-09 from `SCOPE_10.md` — position 11 of 12 in the WORKFLOW_1|WORKFLOW_12 dependency-ordered sequence.

> ## ⚠ NOT A WORKFLOW TASK — read [`WORKFLOW_1`](WORKFLOW_1.md) §2 before scheduling
>
> Renamed into the `WORKFLOW_*` namespace on 2026-09-09, but this file **never mentions
> `workflowScript`**. It belongs to the **multi-instance session-partitioning** programme
> (with `SCOPE_11`…`SCOPE_17`), whose thesis is *"two cyrup instances sharing a directory must
> not consume each other's runs."*
>
> **It does not block `workflowScript` and `workflowScript` does not block it.** Shipping
> workflows requires [`WORKFLOW_2`](WORKFLOW_2.md) → `3` → `4` → `5`, not this file. Do not read
> the numbering as a runway.
>
> ⚠ If you are here because [`WORKFLOW_2`](WORKFLOW_2.md) or
> [`WORKFLOW_13`](WORKFLOW_13.md) deferred async-workflow work to this file: **it is not owned
> here.** That orphan is [`WORKFLOW_13`](WORKFLOW_13.md)'s — see [`WORKFLOW_1`](WORKFLOW_1.md) §4.

OBJECTIVE: close the last two session gates in the status/reporting surface — the async status
snapshot widget (`async-status-snapshot.ts`, 48 LOC) and S6's live workflow controls
(`run-status.ts:609-612`). Both are small; both were blocked on state that WORKFLOW_6 and WORKFLOW_10 build.

**Depends on WORKFLOW_6** (`workflow_controllers`, `ForegroundControlEntry` fields). WORKFLOW_10 is *not*
required for S6 itself, only for the adjacent `:487`/`:514` capacity reads.

## SUBTASK1 — `background/async_status_snapshot.rs`

**Ports:** `pi-subagents/src/runs/background/async-status-snapshot.ts` (48 LOC)

```ts
ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX = "PI_SUBAGENT_ASYNC_JSON:"                       // :24
buildAsyncStatusSnapshot(jobs, options)                                                // :26
asyncStatusSnapshotJobsForState(state, sessionId) {                                    // :30
    if (!state || !sessionId || state.currentSessionId !== sessionId) return [];       // :31  STRICT
    for (const job of …) if (job.sessionId === sessionId) jobs.set(job.asyncId, job);  // :34
}
buildAsyncStatusSnapshotForState(state, sessionId, options)                            // :42
encodeAsyncStatusSnapshotWidget(jobs, options): string[]                               // :46
```

**Two filters, not one:** `:31` gates on the *state* matching the requested session (STRICT — a
mismatch or a missing session yields an empty snapshot), then `:34` filters *each job*. Port both.

The widget prefix is a wire constant — cyrup's rebrand does **not** apply. Keep
`PI_SUBAGENT_ASYNC_JSON:` verbatim unless a consumer in this repo demands otherwise; changing it
silently breaks any reader keyed on it.

**Layout:** one concern, one file.

## SUBTASK2 — S6, live workflow controls in the status report

**Where:** `crates/cyrup-ext-subagents/src/background/run_status.rs`
**Ports:** `pi-subagents/src/runs/background/run-status.ts:609-612`

```ts
const liveWorkflowControls =
    status.mode === "workflow"
    && deps.state?.currentSessionId === status.sessionId          // gate 1
    && deps.state?.workflowControllers?.has(status.runId)         // ← WORKFLOW_6 registry
        ? [...deps.state.foregroundControls.values()].filter((control) =>
              control.parentWorkflowRunId === status.runId        // ← WORKFLOW_6 field
           && control.sessionId === status.sessionId              // gate 2 ← WORKFLOW_6 field
           && (control.activeChildren?.size ?? 0) > 0)            // ← WORKFLOW_6 field
        : [];
```

**Two session comparisons in one expression**, against two different objects: the *state's* current
session vs the status's, and each *control's* session vs the status's. They are not redundant — a
control could carry a stale session after a rotation.

## SUBTASK3 — the remaining `run-status.ts` transcript gates

**Where:** `background/run_status.rs`, `extension/executor/status.rs`

The async transcript gate (`:494`/`:521`) landed already. Two remain:

```ts
if (!state.currentSessionId || control.sessionId !== state.currentSessionId)  // :249  STRICT — THROWS
    throw new Error(`Foreground run '${control.runId}' is not owned by the current session.`);
const controls = [...deps.state.foregroundControls.values()]
    .filter((control) => control.sessionId === currentSessionId);             // :368
```

`:249` is the one gate upstream implements as a **throw** rather than a returned refusal, and it is
**STRICT**. Port both the class and the shape.

## Tests

| test | pins |
|---|---|
| `the_snapshot_is_empty_when_the_state_session_does_not_match` | `:31` STRICT, first filter |
| `the_snapshot_excludes_jobs_from_other_sessions` | `:34`, second filter |
| `the_snapshot_widget_prefix_is_unchanged` | the wire constant |
| `live_workflow_controls_require_both_session_comparisons` | S6 — construct a control whose session differs from the status's and assert it is excluded even when the registry has the run |
| `live_workflow_controls_are_empty_without_a_registry_entry` | the `workflowControllers.has` arm |
| `live_workflow_controls_exclude_children_with_no_active_children` | `activeChildren.size > 0` |
| `a_foreground_transcript_of_a_foreign_run_is_refused` | `:249` STRICT |
| `a_foreground_transcript_with_no_session_is_refused` | `:249`'s `!currentSessionId` arm |
| `the_transcript_control_list_is_session_filtered` | `:368` |

## Benchmarks

None. Both are report-rendering paths.

## Definition of done

- The async status snapshot applies both filters and keeps its wire prefix.
- S6's live workflow controls apply **both** session comparisons plus the registry and
  `activeChildren` predicates.
- `:249` refuses (STRICT) and `:368` filters.
- Tests pass; workspace 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/async-status-snapshot.ts`,
  `pi-subagents/src/runs/background/run-status.ts:249`, `:368`, `:609-612`.
* Already landed in `run_status.rs`: `list_active_runs`'s session filter (`:647`) — the model to
  follow; its doc at `:586-595` already explains the drop-unattributed rule correctly.
* Already landed in `extension/executor/status.rs`: the async transcript gate (`:494`/`:521`).
* Requires WORKFLOW_6's `ForegroundControlEntry::{session_id, parent_workflow_run_id, active_children}`.
