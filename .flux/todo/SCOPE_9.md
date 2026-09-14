---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_9 — per-session active-async capacity

> Renamed 2026-09-09 from `SCOPE_9.md` — position 10 of 12 in the WORKFLOW_1|WORKFLOW_12 dependency-ordered sequence.

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

OBJECTIVE: port `background/active-async-capacity.ts` (516 LOC) so the async concurrency cap is
**per session** rather than global. Without it, one instance's fan-out starves every other instance
sharing the directory.

**Depends on WORKFLOW_6** (`workflow_controllers`, consumed as `liveWorkflowRunIds`).

SCOPE_9 is *not* the 30 `sessionId` references upstream — this file is session-keyed end to end.

## SUBTASK1 — a new root, keyed by SESSION not cwd

**Where:** `crates/cyrup-ext-subagents/src/background/artifact_roots.rs`

```ts
export const ACTIVE_ASYNC_CAPACITY_DIR = path.join(TEMP_ROOT_DIR, "session-active-async-capacity");  // :10
export function activeAsyncCapacitySessionKey(sessionId: string): string;                            // :91
```

**This is the first root in the system keyed by session.** Every existing root
(`async_root`, `results_dir`, scratch, chain-runs) is keyed by [`cwd_key`]. Add it beside them and
**say so in the doc** — sweep item **D22** — because a reader who assumes the cwd-keying convention
will place it wrong.

Use `identity::IndexSegment` for the session key; do not write a second encoder.

## SUBTASK2 — configuration

**Where:** `crates/cyrup-ext-subagents/src/registration/` (`SubagentExtensionConfig`)

```ts
resolveMaxActiveAsyncRunsPerSession(value): number | undefined              // :80
resolveAbandonedSlotReleaseAfterMs(value): number | false                   // :85
DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS = 20 * 60 * 1000                    // :11
MIN_ABANDONED_SLOT_RELEASE_AFTER_MS     =  5 * 60 * 1000                    // :12
MAX_ABANDONED_SLOT_RELEASE_AFTER_MS     = 24 * 60 * 60 * 1000               // :13
```

cyrup has **no** `max_active_async_runs_per_session` today. Add it next to the existing
`global_concurrency_limit`, plus `capacity.abandonedSlotReleaseAfterMs`. Port the clamping
(`resolveAbandonedSlotReleaseAfterMs` returns `false` to mean "never release", which is **not**
the same as `None`/default — model it as a three-state enum, not an `Option<Duration>`).

## SUBTASK3 — claim, inspect, release

```ts
interface ActiveAsyncCapacityOwner       // :15
interface ActiveAsyncCapacityHandle      // :31
interface ActiveAsyncCapacityReleaseEvidence  // :50
interface ActiveAsyncCapacityInspection  // :63
inspectActiveAsyncCapacityOwner(
    { runId, sessionId, asyncDir },
    { rootDir, liveWorkflowRunIds, abandonedSlotReleaseAfterMs }): ActiveAsyncCapacityInspection  // :311
```

`liveWorkflowRunIds` is `new Set(state.workflowControllers?.keys() ?? [])` at every call site
(`run-status.ts:487`, `:514`; `subagent-executor.ts:628`, `:1684`, `:1927`, `:4988`) — a live
workflow's slot is **never** reclaimed as abandoned. This is why WORKFLOW_6 and SCOPE_8 must land
first: a leaked registry entry withholds capacity forever, a missing one reclaims a live run's slot.

**Layout:** `active_async_capacity/{mod,config,key,claim,inspect,sweep}.rs`.

## SUBTASK4 — enforce at the async spawn path

**Where:** `extension/executor/background.rs` (where `global_concurrency_limit` is already read)

**Change:** before spawning an async run, claim a capacity slot for the current session; refuse
with upstream's message when the session is at its cap. Release on terminal.

## Tests

| test | pins |
|---|---|
| `two_sessions_each_get_their_own_cap` | **the core property** — session A at its cap does not block session B |
| `a_slot_is_released_when_the_run_reaches_terminal` | lifecycle |
| `an_abandoned_slot_releases_after_the_configured_delay` | `DEFAULT_ABANDONED_SLOT_RELEASE_AFTER_MS`, injected clock |
| `a_live_workflows_slot_is_never_reclaimed` | `liveWorkflowRunIds` — the WORKFLOW_6 dependency |
| `release_after_ms_clamps_below_the_minimum` | `MIN_…` = 5 min |
| `release_after_ms_clamps_above_the_maximum` | `MAX_…` = 24 h |
| `release_after_false_means_never_release` | the three-state distinction from "unset" |
| `no_configured_cap_means_unlimited` | `resolveMaxActiveAsyncRunsPerSession` → `undefined` |
| `the_capacity_root_is_keyed_by_session_not_cwd` | SUBTASK1 — two cwds, one session, one slot dir |
| `a_session_id_that_is_a_path_produces_one_component` | `IndexSegment` reuse |

## Benchmarks

None. The claim is one small file write per async spawn — negligible beside process spawn itself,
which the existing spawn path already dominates.

## Definition of done

- The async cap is per session: one instance exhausting its cap does not affect another.
- A live workflow's slot is never reclaimed; an abandoned one releases on the clamped ladder.
- `false` (never release) is distinguishable from unset.
- The capacity root is keyed by session, documented as the first such root (D22).
- Config surfaces `maxActiveAsyncRunsPerSession` and `capacity.abandonedSlotReleaseAfterMs`.
- Tests pass; workspace 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/active-async-capacity.ts` (HEAD `7fe9dee1`).
* Call sites for `liveWorkflowRunIds`: `run-status.ts:487`, `:514`; `subagent-executor.ts:628`,
  `:1684`, `:1927`, `:4988`.
* cyrup roots to sit beside: `background/artifact_roots.rs:281-284` (`cwd_key`-keyed) — read the
  doc note there about every instance resolving identical roots before adding a differently-keyed one.
* Existing concurrency config: `SubagentExtensionConfig::global_concurrency_limit`, threaded into
  `RunnerConfig` at `extension/executor/background.rs`.
