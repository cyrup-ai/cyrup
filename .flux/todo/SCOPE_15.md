---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_15 — scheduled runs, part A: store and schedule model

OBJECTIVE: build the persistence half of `background/scheduled-runs.ts` (979 LOC) — the schedule
record, its on-disk store, and the capability-ceiling gate that governs whether a schedule may be
persisted at all. **Unwired**: no trigger loop, no tool surface. SCOPE_16 adds those.

## SUBTASK1 — the store path, and its deliberate non-session keying

**Where:** new module `crates/cyrup-ext-subagents/src/background/scheduled_runs/`

```ts
export function scheduledRunStorePath(cwd: string, _sessionId?: string, root?: string): string;  // :98
```

The `_sessionId` parameter is **accepted and unused** — underscore-prefixed upstream. The store is
per-**cwd** by design: a schedule outlives the session that created it, which is the whole point of
scheduling.

**Port that, do not "fix" it.** Keep the parameter and the underscore, and document why: a reader
who has just absorbed this programme's session-scoping work will otherwise "correct" it into a
session-keyed store and silently break schedule persistence across sessions.

This is the counter-example that proves the rule is *scoping*, not *session-keying everything*.

## SUBTASK2 — the schedule record and store

The persisted schedule: what to run, when, with which agent/options. Round-trips through the store
with a version field, following the `ResultIndexEntry` pattern — a future version deserializes to
`None`, never a panic.

**Layout:** `scheduled_runs/{mod,store,schedule}.rs` for this task; SCOPE_16 adds `trigger.rs` and
`tool.rs`.

## SUBTASK3 — the capability-ceiling gate

```ts
resolveCapabilityCeiling?: (sessionId: string) => ResolvedSubagentCapabilityCeiling | undefined;  // :86
const sessionId = ctx.sessionManager.getSessionId() ?? "unknown";                                  // :619
if (this.deps.resolveCapabilityCeiling?.(sessionId))                                               // :620
    return textResult("Cannot persist a schedule while a capability ceiling is active.", …, true);
```

**This is where session enters the file** — not in the store, but in the *authority to write to it*.
A session operating under a capability ceiling cannot persist a schedule, because a schedule would
outlive the ceiling and escape it.

Note `:619`'s `?? "unknown"` — upstream substitutes a literal string rather than refusing. Port
that fallback; do not turn it into an `Option`, or a headless host with no session gains the
ability to persist schedules that a ceiling-bound one lacks.

cyrup already has `exec/capability_ceiling.rs` (14 `session_id` references) — wire to it, do not
build a second ceiling notion.

The refusal string is contract: `Cannot persist a schedule while a capability ceiling is active.`

## Tests

| test | pins |
|---|---|
| `the_store_path_is_keyed_by_cwd_not_session` | SUBTASK1 — two sessions, one cwd, one store |
| `a_schedule_round_trips_through_the_store` | persistence |
| `a_future_schedule_version_is_ignored_not_a_panic` | version discipline |
| `a_schedule_cannot_be_persisted_under_an_active_capability_ceiling` | `:620` + exact string |
| `a_schedule_persists_normally_with_no_ceiling` | the negative |
| `a_host_with_no_session_uses_the_unknown_key` | `:619`'s `?? "unknown"` — asserted explicitly, because the instinct is to make it `Option` |
| `the_store_survives_a_session_change` | the reason it is cwd-keyed |

## Benchmarks

None. Schedule persistence is a user-triggered write of a small record.

## Definition of done

- `scheduled_run_store_path` is cwd-keyed, accepts an unused session parameter, and its doc says
  why that is correct rather than an oversight.
- Schedules round-trip with version tolerance.
- The capability-ceiling gate refuses persistence with upstream's exact string, wired to
  `exec/capability_ceiling.rs`.
- The `"unknown"` session fallback is ported verbatim.
- The module is **unwired** — no trigger, no tool registration yet.
- Workspace `--no-fail-fast` 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/scheduled-runs.ts:86`, `:98`, `:466-470`, `:619-620`
  (HEAD `7fe9dee1`).
* Existing ceiling: `crates/cyrup-ext-subagents/src/exec/capability_ceiling.rs`.
* Version-tolerance pattern to copy: `background/result_index/entry.rs`'s `IndexVersion`
  (deserializes only from `1`; anything else is `None`, not a panic).
* Sweep item **D20**: `exec/capability_ceiling.rs`'s docs gain a note that the ceiling is consulted
  before a schedule may be persisted.
