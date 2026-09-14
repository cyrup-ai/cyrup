---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_16 — scheduled runs, part B: trigger loop and tool surface

OBJECTIVE: complete `scheduled-runs.ts` — fire schedules when due, and expose the tool actions that
create, list and cancel them. This turns SCOPE_15's store on.

**Depends on SCOPE_15**, and on **WORKFLOW_10** (a fired schedule spawns an async run, which must claim
a per-session capacity slot).

## SUBTASK1 — `scheduled_runs/trigger.rs`

The loop that finds due schedules and fires them. A fired schedule spawns a background run through
the existing async spawn path (`extension/executor/background.rs`), which means:

* it acquires a **per-session capacity slot** (WORKFLOW_10) — a schedule cannot exceed its session's cap;
* the spawned run carries `session_id` and `completion_owner_id` like any other, so its result is
  delivered to the right instance and no other.

**Which session does a fired schedule belong to?** The session that is live when it fires, not the
one that created it — the store is cwd-keyed precisely because schedules outlive sessions. Make
this explicit in the docs; it is the question a reader will have, and getting it wrong either
strands results (attributing to a dead session) or misdelivers them.

## SUBTASK2 — the session-proxy shape

```ts
const sessionId = source.getSessionId();                    // :466
if (property === "getSessionId") return () => sessionId;    // :470
```

Upstream wraps the session source in a proxy that pins `getSessionId` to a captured value, so a
schedule firing mid-session-change sees one consistent identity for its whole execution. Port the
*property* (a pinned identity for the duration of a fire), not the JS proxy mechanism — in Rust
this is capturing the `SessionId` up front and passing it down, which is what
`OwnershipSnapshot` already does for the drain loop.

## SUBTASK3 — `scheduled_runs/tool.rs`

The tool actions: create / list / cancel. Listing is session-scoped for *display* even though the
store is not — a user asks "what have I scheduled", and the answer is filtered by the session that
created each entry where one is recorded.

## SUBTASK4 — register and wire

**Where:** `extension/tool/routing.rs` and the extension's action schema.

Register the actions and start the trigger loop at install, alongside the existing detached
schedules. Use SCOPE_14's shared staggered scheduler rather than a fourth ad-hoc `tokio::spawn`.

## Tests

| test | pins |
|---|---|
| `a_due_schedule_fires` | the loop |
| `a_not_yet_due_schedule_does_not_fire` | the bound |
| `a_fired_schedule_claims_a_capacity_slot_for_the_live_session` | WORKFLOW_10 integration |
| `a_fired_schedule_is_refused_when_the_session_is_at_its_cap` | the cap actually binds |
| `a_fired_runs_result_is_delivered_to_the_live_session_only` | end-to-end: the whole programme's property, reached via a schedule |
| `the_session_identity_is_pinned_for_the_duration_of_a_fire` | `:466-470` — change the session mid-fire, assert one identity |
| `a_schedule_created_in_one_session_fires_in_a_later_one` | the cwd-keyed store's reason to exist |
| `listing_schedules_is_filtered_for_display` | SUBTASK3 |
| `cancelling_a_schedule_removes_it_from_the_store` | lifecycle |

The fifth is the acceptance test for this task: it exercises schedule → spawn → capacity → delivery
and proves the session chain holds across all of it.

## Benchmarks

None. The trigger loop wakes on an interval and does nothing when no schedule is due; SCOPE_14 owns
the one performance-scoped item in this programme.

## Definition of done

- Due schedules fire; not-yet-due ones do not.
- A fired schedule claims a per-session capacity slot and is refused at the cap.
- A fired run's result is delivered only to the live session — verified end to end.
- Session identity is pinned for the duration of a fire.
- A schedule created in one session fires in a later one.
- Tool actions registered; the trigger loop uses the shared staggered scheduler.
- Workspace `--no-fail-fast` 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/scheduled-runs.ts` (HEAD `7fe9dee1`), especially
  `:466-470` (the pinned-session proxy) and the trigger body.
* Spawn path a fired schedule uses: `extension/executor/background.rs` — already stamps
  `session_id` and `completion_owner_id` onto `RunnerConfig`.
* Capacity: WORKFLOW_10's `active_async_capacity`.
* Pinned-identity precedent: `background/delivery/ownership.rs`'s `OwnershipSnapshot`, captured once
  before a drain loop for exactly this reason (a session change mid-loop must not split the batch).
* Shared scheduler: SCOPE_14 SUBTASK3.
