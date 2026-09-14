---
stage: new
status: done
updated: 2026-09-06 03:29
---

# SCOPE_11 — durable wait subscriptions

OBJECTIVE: port `background/wait-subscriptions.ts` (348 LOC) — `{id, nonBlocking: true}` wake
subscriptions that survive across turns. cyrup already has the in-process `CompletionBus` wired
into `wait.rs`, so this adds the **durable, cross-turn** half, not the wake mechanism.

**Depends on SCOPE_3** (`wait_completions`) and **SCOPE_4** (replay), which supply what a woken
subscription reads.

## SUBTASK1 — `background/wait_subscriptions/`

**Ports:** `pi-subagents/src/runs/background/wait-subscriptions.ts`

Surface:

```ts
interface ArmWaitSubscriptionInput          // :39
interface WaitSubscriptionManager           // :46
formatWaitSubscriptions(state, now): string | undefined   // :89
createWaitSubscriptionManager(…)            // :99
```

Five session gates:

```ts
|| typeof record.sessionId !== "string"                       // :72   invalid record — no session
if (record.sessionId === state.currentSessionId) continue;    // :157  skip own-session
if (record.sessionId !== state.currentSessionId) return;      // :206  act only on own-session
if (!run || run.sessionId !== record.sessionId) { … }         // :215  run/record agreement
if (record.sessionId === state.currentSessionId) { … }        // :327
```

`:157` and `:206` are **opposite** comparisons on the same field in the same file — one skips the
current session, the other requires it. Read both in context before implementing; they are
different phases (enumeration vs delivery), not a contradiction to be "simplified".

`:215` re-verifies the run's session against the record's, the same contents-decide discipline the
result index uses for hashed addresses.

**Layout:** `wait_subscriptions/{mod,record,manager,format}.rs` — the persisted record, the
manager's lifecycle, and the rendering are three jobs.

## SUBTASK2 — arm from the wait tool's non-blocking form

**Where:** `background/wait.rs` and the wait tool surface

**Change:** `{id, nonBlocking: true}` arms a subscription and returns immediately instead of
blocking. The subscription fires when the run completes — including in a later turn, and including
when the payload has already been cleaned up (resolving through SCOPE_4's replay record).

**Why:** `PARITY-GAPS.md` records this as part of the `subagent_wait` gap: *"there is no
`{id, nonBlocking:true}` wake subscription"*.

## SUBTASK3 — render armed subscriptions

**Where:** the status/fleet reporting surface

**Change:** `formatWaitSubscriptions` (`:89`) renders what is currently armed. Session-scoped by
construction — a subscription belonging to another session is never rendered here.

## Tests

| test | pins |
|---|---|
| `a_non_blocking_wait_arms_a_subscription_and_returns_immediately` | the feature |
| `an_armed_subscription_fires_when_the_run_completes` | the wake |
| `an_armed_subscription_fires_after_cleanup_via_the_replay_record` | the SCOPE_4 dependency — the hard case |
| `a_record_with_no_session_id_is_invalid` | `:72` |
| `enumeration_skips_the_current_session` | `:157` |
| `delivery_requires_the_current_session` | `:206` — the opposite comparison, asserted separately |
| `a_record_whose_run_reports_a_different_session_is_dropped` | `:215` contents-decide |
| `format_renders_only_this_sessions_subscriptions` | `:89` |
| `a_subscription_survives_a_turn_boundary` | the durability claim, not just the in-memory bus |

The third and last are the two that would silently pass against the existing in-memory bus if the
durable half were skipped — write them so they fail without it.

## Benchmarks

None. Subscriptions are armed per user request and read on completion events.

## Definition of done

- `{id, nonBlocking: true}` arms a durable subscription and returns immediately.
- An armed subscription fires across a turn boundary and after payload cleanup.
- All five session gates ported with their upstream comparisons intact — including the two that
  point opposite ways.
- Tests pass; workspace 0 failed; clippy exit 0.

## Research notes

* Upstream: `pi-subagents/src/runs/background/wait-subscriptions.ts` (HEAD `7fe9dee1`).
* Existing wake mechanism to build on, not replace: `background/wait.rs:206` (`WaitDeps::completion_bus`),
  `background/watch/observer.rs` (`CompletionBus`, already a `CompletionObserver` returning `bool`).
* `wait.rs:30-50`'s module docs explain the two-part wake shape and the 500 ms latency floor; extend
  rather than contradict them.
* `PARITY-GAPS.md` `SUBA-034` (event-bus wake) is already closed; this is the subscription half.
