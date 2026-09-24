---
stage: augment
status: ready
updated: 2026-09-22 00:00
---

# UW-3 — the parent never reads a child's watchdog status events

> Branch `claude/subagents-delegation`, HEAD `521beaa`. Read-only research pass: no code changed.
> Ledger row: `docs/gap-analysis/PARITY-GAPS.md:2022` (UW-3, *medium*).
> Upstream is read pinned: `git -C tmp/pi-subagents show <tag>:<path>`. `child_status.rs` and
> `register_child.rs` are ports of **v0.43.0** (`child_status.rs:1-2`, `register_child.rs:1-2`), so
> v0.43.0 is the reference tag. v0.68.0 numbers are added beside it and do not replace it.

## Verdict

**The row is still open, and the observable in it is right. It is now proven by a trace, where
before it was only inferred.** It is also **incomplete in a way that changes the size of the fix.**

1. **Parent (the row's claim): proven.** cyrup has one drive loop for both foreground and background
   children (`exec/drive_attempt.rs:552`). Only one thing ever disarms its final-drain timer:
   `CancelDrain`, which comes from `agent_end{willRetry:true}` (`drive_attempt.rs:340-342`). No
   other code holds the timer. A `subagent.watchdog.status` line parses to
   `SubagentEvent::Unknown` (`exec/ndjson.rs:245-246`) and nothing folds it. So an armed child that
   gives its final answer and then starts its watchdog review is sent SIGINT→SIGTERM→SIGKILL
   1000 ms later (`drive_attempt.rs:69`, `:343-348`, `:647-654`). The run is then **coerced to
   success** (`exec/attempt_runner.rs:515-517`, `:537-538`). The result is exit 0, the answer from
   before the review, no warning and no error.
2. **Is there a second holding mechanism? Not on the parent side.** On the **child** side there is a
   second **truncating** mechanism that nobody has recorded. The child's review is awaited inside an
   `AgentEnd` extension handler (`prompt_runtime.rs:2017`). The extension dispatcher drops every
   handler future after `DEFAULT_INVOKE_BUDGET` = **5 s** (`cyrup-ext/src/dispatch.rs:30`,
   `:530-533`; the only production `Dispatcher` is `facade.rs:294`). The review's own bound is
   `agentEndTimeoutMs` = **30 s** (`settings.ts:76`; cyrup `settings.rs:83`). Today the parent's 1 s
   kill comes first, so the 5 s budget cannot be seen. **If only the parent fold is ported, any
   child review longer than 5 s is still cut short, now by the child itself.** The parent's snapshot
   would then be stuck at `reviewing`. Upstream's runner has no per-handler budget
   (`runner.ts:805-811`, cited in-tree at `native_impl.rs:593-596`).
3. **The framing in the brief needs correcting.** The protocol has no "I am blocked" and no
   "progress". At v0.43.0 the child emits exactly `idle` (session start and shutdown), `reviewing`
   (at `agent_end`), then `failed`, `stale` or `idle` (`register-child.ts:89-115`).
   `autofollow`/`settling` are declared but never emitted, and `followUpPending` is always `false`.
   These events are not a general liveness heartbeat. The parent uses them for one thing: a child
   that has finished its turn but is still reviewing must not be drained. In cyrup the ignored line
   still counts as activity (`control.observe_event` → `note_activity`, `exec/control.rs:1321`).
   So the child is **not treated as idle**. It is **force-drained as finished**.

## 1. Citation audit

### Upstream — v0.43.0 (pinned; every one re-read at the tag this pass, all exact)

| Row cites | At v0.43.0 | OK |
|---|---|---|
| `execution.ts:846` | `if (isChildWatchdogStatusEvent(evt)) {` | ✔ |
| `execution.ts:848` | `const next = acceptChildWatchdogEvent({` | ✔ |
| `execution.ts:857` | `if (childWatchdogIsActive(next)) {` | ✔ |
| `execution.ts:585` | `if (childWatchdogIsActive(childWatchdogState)) {` (inside `startFinalDrain`) | ✔ |
| `subagent-runner.ts:626` / `:628` / `:640` | the runner's stdout fold, same three calls | ✔ |
| `subagent-runner.ts:831` | `startFinalDrain`'s active check | ✔ |
| `subagent-runner.ts:2711-2712` | `updateStepFromChildEvent` → `status.json` `step.watchdog` | ✔ |
| `child-status.ts:167` / `:181` / `:186` | `isChildWatchdogStatusEvent` / `childWatchdogIsActive` / `acceptChildWatchdogEvent` | ✔ |

### Upstream — v0.68.0 (the sweep's coordinates, all verified)

`child-status.ts:159/:182/:187` ✔; `execution.ts:692/:1015/:1017/:1082` ✔ (`:1082` is the new
`applyChildWatchdogMessage`); `subagent-runner.ts:3016/:3017/:3033` ✔.
**Add to the row:** at v0.68.0 the background drain-hold fold moved to
`src/runs/background/run-child-session.ts:315` (`startFinalDrain`'s check), `:430`/`:432`/`:444`
(fold). `subagent-runner.ts:3016-3033` is only the `status.json` half.
**Also add:** at v0.68.0 children run in-process. `registerChildWatchdog(pi, childConfig,
writeStatus)` receives a sink from `ChildRuntimeConfig.watchdogStatus` and throws if it is missing
(`register-child.ts` @v0.68.0). The transport is no longer NDJSON on stdout. The fold is the same
shape, and `childWatchdogIsActive` shrank to `phase === "reviewing"` (`:182-185`). cyrup's
subprocess model matches v0.43.0's transport, so v0.43.0 stays the reference.

### cyrup — at `521beaa`

| Row claim | Now | Note |
|---|---|---|
| `child_status.rs:489`, `:512`, `:531` | exact | zero callers outside the module; every in-module call is in `#[cfg(test)] mod tests` (starts `:561`; calls at `:732-864`) |
| `spawn_plan.rs:1045-1055` resolves+encodes | exact | production |
| `spawn_plan.rs:2333-2341` "decodes it" | exact lines, **but this is test code** | inside `#[cfg(test)]` (`:1429`), test `:2295`. The production decode is the child's: `prompt_runtime.rs:2382` → `register_child.rs:247` |
| `exec/ndjson.rs` has no watchdog ref | ✔ (0 hits; same for `drive_attempt.rs`, `exec/mod.rs`) | |
| `background/` "three" refs, all `PermissionRules` | **four**, all `permission_arbiter` | `recovery_descriptor.rs:1032` added, test-only (`#[cfg(test)]` at `:1018`) |
| emit side `prompt_runtime.rs:2394` / `:2404` | exact | |
| `child_status.rs:471-485` in-tree statement | exact | |

**In-tree doc drift. Not part of the row; fix it opportunistically when the code is touched:**
- `child_status.rs:16-19` and `:470-490` cite `child-status.ts:168-181 / :183-186 / :188-205 /
  :194-198 / :199`. At v0.43.0 those are `:167-179 / :181-184 / :186-205 / :193-196 / :197`.
- `register_child.rs:314-316` cites `prompt_runtime.rs:1701`, `:1687-1688`, `:1698-1700`. They are
  now `:2394`, `:2382`, `:2392-2393`.

## 2. Upstream, quoted (v0.43.0)

Child side, `src/watchdog/register-child.ts:102-110`:
```ts
onRuntimeEvent("agent_end", async (event, ctx) => {
	rememberContext(ctx);
	emitStatus("reviewing");
	await runtime.handleAgentEnd(event, ctx);
	const snapshot = runtime.getSnapshot(ctx.cwd);
	if (snapshot.status === "failed") emitStatus("failed", false, snapshot.lastError);
	else if (snapshot.status === "stale") emitStatus("stale", false, "review stale");
	else emitStatus("idle");
});
```
`writeStatus` (`:48-54`) is `process.stdout.write(JSON.stringify(event)+"\n")`, and errors are swallowed.

Parent fold, `src/runs/foreground/execution.ts:846-864`:
```ts
if (isChildWatchdogStatusEvent(evt)) {
	if (!childWatchdog) return;
	const next = acceptChildWatchdogEvent({ current: childWatchdogState, event: evt,
		runId: options.runId, agent: agent.name, childIndex: options.index ?? 0 });
	if (!next) return;
	updateChildWatchdogState(next);           // result.watchdog = progress.watchdog = snapshot (:563-567)
	if (childWatchdogIsActive(next)) {
		clearFinalDrainTimers();
		armWatchdogTail();
	} else {
		clearWatchdogTailTimer();
		if (cleanTerminalAssistantStopReceived || agentSettledReceived) startFinalDrain();
	}
	fireUpdate();
	return;                                    // BEFORE progress.lastActivityAt = now (:866-869)
}
```
The drain gate, `:584-588`: `startFinalDrain` first checks `if (childWatchdogIsActive(childWatchdogState)) { armWatchdogTail(); return; }`.
Tail, `:606-622`: the tail is armed only after a clean terminal stop or `agent_settled`. It lasts
`childWatchdog?.watchdogTailTimeoutMs ?? 120_000`. When it fires, the snapshot becomes
`{phase:"stale", seq:+1, reason:"child watchdog tail timeout", timedOut:true}` and it calls
`startFinalDrain()`.
Lifecycle, `:623-630`: `cancel-drain` clears **both** the drain timers and the tail timer.
Ordering: `transcriptWriter.writeChildEvent(evt)` (`:842`) and `applyChildLifecycle(...)` (`:844`)
run **before** the watchdog branch.
The runner (`subagent-runner.ts:572`, `:626-652`, `:830-834`, `:858-871`) has the same fold. It
reads its config from the child env (`decodeChildWatchdogConfig(env?.[CHILD_WATCHDOG_CONFIG_ENV])`)
and filters identity against `childEventContext` (runId / agent / stepIndex). Its `status.json` half
(`:2711-2722`) stores `step.watchdog` and **does** bump `lastActivityAt`.
Consequence of not reading the events: the 1000 ms `FINAL_STOP_GRACE_MS` window (`:552`) opened by
the terminal stop fires. The child is SIGTERMed mid-review, and `forcedDrainAfterFinalSuccess`
(`:1080`, `:1098`) coerces the run to exit 0.

Defaults: `enabled:false` and `children.enabled:false` (`settings.ts:71-121`; cyrup
`settings.rs:78`, `:106`). **The whole path is opt-in on both sides.**

## 3. cyrup, greped and traced

**Producer: wired.** `session_launch.rs:130` → `prompt_runtime_extension_for_env` →
`prompt_runtime.rs:2382` reads `CYRUP_SUBAGENT_WATCHDOG_CHILD_CONFIG` →
`register_child_watchdog(..., stdout_status_sink())` (`:2394`, `:2404`).
`ChildWatchdog::handle_agent_end` (`register_child.rs:206`) emits `reviewing`, awaits the runtime,
then emits `failed`/`stale`/`idle`. It is dispatched from `prompt_runtime.rs:2017`.
`stdout_status_sink` writes `std::io::stdout()` directly (`register_child.rs:114-125`). This is
**not** routed through `output_guard::emit_stray_line` (`crates/cyrup/src/output_guard.rs`), so the
line really does reach fd 1. The review runs whenever git reports changes: `review_changes_only:
true` (`register_child.rs:290`), and with `repo_change_signature: None` it defaults to
`GitRepoChangeSource` (`watchdog/runtime.rs:543-545`). **A child that edits files triggers a real
review, which is a model call after up to 3 s of LSP diagnostics.**

**Parse side: none.** `drive_attempt.rs:309` `parse_line` → `Unknown`. The line is then projected
(`:335`, `None`), written to the transcript (`:390-392`), counted as activity (`:396`) and recorded
(`:403`). No field anywhere holds a child watchdog snapshot: `SingleResult`, `AgentProgress` and
`StepStatus` have no `watchdog` field (grep `pub watchdog` finds only
`workflows/checklist.rs:200`, which is unrelated). The runner's `status.json` fold
(`background/telemetry.rs:214-272`) sends it to `_ => {}` and bumps `last_activity_at`. That matches
upstream's activity bump, but `step.watchdog` is never stored.

**Zero-caller claim: proven.**
`git grep -n 'is_child_watchdog_status_event\|child_watchdog_is_active\|accept_child_watchdog_event\|ChildWatchdogStateSnapshot\|ChildWatchdogIdentity' -- crates ':!…/child_status.rs'`
returns nothing. Only the event **type** is used elsewhere, by the producer (`register_child.rs`)
and its tests (`tests/watchdog_wiring.rs:35-733`).

**Drain machinery: a single loop, no second hold.** `drive_attempt` is called once
(`exec/attempt_runner.rs:150`). The background runner reaches it through `run_sync` with a
`live_events` tee (`background/runner_main/executor.rs:711`, `:872`; `drive_attempt.rs:302-304`).
`spawn/parallel.rs`'s four `wait_final_drain` calls are all under `#[cfg(test)]` (`:632`). Every
write to `DriveState::final_drain_at`:
- `:223` init `None`
- `:341` `CancelDrain` → `None` (the **only** disarm)
- `:344-347` `StartDrain` → `now + 1000ms`, set if unset

Nothing else reads or clears it. The select arm at `:647-654` calls `child.terminate(&cancel)` and
returns `Settled::ForcedDrain`.

### The trace (armed child that edited a file, bound session, `--mode json`)

1. The child's agent emits the final assistant `message_end{stopReason:"stop"}`. The agent awaits
   each subscriber before it emits the next event (`cyrup-ext/src/subscriber.rs:1-4`). So this
   line is already on the json fan-out (`cyrup-modes/src/json.rs:108-114`) **before** `AgentEnd` is
   dispatched.
2. Parent: `is_terminal_assistant_stop` → `StartDrain` → `final_drain_at = t0 + 1000ms`
   (`drive_attempt.rs:315-348`).
3. Child: the `AgentEnd` dispatch → `prompt_runtime.rs:2017` → `emit_status(Reviewing)` goes to
   stdout, then `runtime.handle_agent_end` → `agent_end_body` → LSP (≤3 s) → `review_delta(delta,
   30_000, …)` (`watchdog/runtime.rs:1092`), which is a model call.
4. Parent: the `reviewing` line becomes `Unknown` and nothing changes. At t0+1000ms the
   `final_drain_arm` fires and the parent sends SIGINT→SIGTERM→SIGKILL to the child mid-review.
   `agent_settled` has not been emitted yet: a bound session emits it only after the whole run,
   including every `AgentEnd` subscriber, returns (`cyrup-session-svc/src/session/run.rs:336`,
   `:354-362`).
5. `forced_termination && clean_terminal_stop && error.is_none()` → exit 0
   (`attempt_runner.rs:515-538`). Stderr is not drained on this path (`:526`).

**Observable:** the parent reports success with the answer from before the review. The child's
blocker and concern warnings never exist, because they would have been injected as a custom
`subagent-watchdog-warning` message by `display_warning` → `services.inject_message`
(`register_child.rs:273-286`). No error, no signal and no stale marker is surfaced.

### The second, unrecorded mechanism (child side)

`Dispatcher::invoke_contained` (`cyrup-ext/src/dispatch.rs:521-535`) wraps each handler in
`tokio::time::timeout(self.budget, call)`, and `budget = DEFAULT_INVOKE_BUDGET = 5s` (`:30`, `:77`).
The only exception is `invoke_with_human_wait_forgiveness`, which forgives only while
`HumanWaitGate::is_waiting()`. That gate is "**permission-only by construction — no other handler
obtains a guard**" (`cyrup-ext/src/native.rs:83-86`). The crate already documents the failure mode
for a different handler (`extension/host/native_impl.rs:19-29`: the dispatcher "enforces that
budget by DROPPING the handler future … every disposal below it … was silently dropped").

For the child's `AgentEnd` arm this means:
- the review (30 s bound) is dropped at 5 s: `reviewing` was emitted, but no terminal phase follows;
- the child's `drainOutstandingWork` block that comes **after** the watchdog in the same `on_event`
  (`prompt_runtime.rs:2023-2043…`) never runs either, for an armed child whose review exceeds 5 s.

This does **not** change today's observable, because the 1 s parent kill always comes first. It
**does** change what fixing the parent alone achieves.

**Adjacent, not in this row's scope. Reported, not decided.** The main watchdog's boundary review is
awaited in the same kind of handler (`extension/host/native_impl.rs:604`) under the same 5 s budget.
UW-4's integration test (`crates/cyrup-it/tests/subagents/watchdog_model_turn_integration.rs`) uses
an instant scripted provider, so it cannot see this. It needs its own row.

## 4. Rust shape (real file:line)

**Plumb the armed config. Do not re-derive it.** `exec/spawn_plan.rs:1045-1058` already holds
`child_config`. Add `pub child_watchdog: Option<ChildWatchdogConfig>` to `AttemptSpawnPlan`
(`spawn_plan.rs:36-55`), following the same "returned alongside the spec rather than re-derived
from the overlay" rule that `tool_diagnostic_path` states at `:42-45`. Carry it through
`PreparedAttempt` (`attempt_runner.rs:302`) into `drive_attempt` (`drive_attempt.rs:552`; the one
call site is `attempt_runner.rs:150`).

**Identity must come from that config**, i.e. `ChildWatchdogIdentity { run_id: cfg.run_id,
agent: cfg.agent, child_index: cfg.child_index }`, and **not** from upstream's literal
`options.index ?? 0`. cyrup encodes `opts.child_index.and_then(...)` (`spawn_plan.rs:1049`), so a
run with no index emits no `childIndex`. A parent that required `Some(0)` would reject every event
(`child_status.rs:542-545`).

**`DriveState`** (`drive_attempt.rs:167-189`) gets three new fields:
`child_watchdog: Option<ChildWatchdogConfig>`, `watchdog_state:
Option<ChildWatchdogStateSnapshot>` and `watchdog_tail_at: Option<Instant>`.

**`handle_child_line`** (`:285`) inserts the fold **after** the transcript write (`:392`, which
matches upstream `:842` coming first) and **before** `control.observe_event` (`:396`, which matches
upstream's `return` before `lastActivityAt`). The raw `line` is re-read as `Value`, because
`SubagentEvent` has flattened it to `Unknown`. Only lines that already parsed to
`SubagentEvent::Unknown` need a second `serde_json::from_str`, so the one-parse rule at `:305-308`
is still true for every known event. The fold: `is_child_watchdog_status_event` → if not armed,
`return Continue` → `serde_json::from_value::<ChildWatchdogStatusEvent>` →
`accept_child_watchdog_event` → store the snapshot →
- if active: `final_drain_at = None` and arm the tail
- otherwise: `watchdog_tail_at = None`, and restart the drain if `clean_terminal_stop ||
  agent_settled`

**`StartDrain` arm** (`:343-348`): if the snapshot is active, arm the tail instead (upstream
`:585-588`). **`CancelDrain`** (`:340-342`) also clears `watchdog_tail_at` (upstream `:624-627`).
The tail is armed only if `clean_terminal_stop || agent_settled` and it is not already armed
(upstream `:607`).

**New `select!` arm** beside `final_drain_arm` (`:647`): when `watchdog_tail_at` fires, set the
snapshot to `Stale`, `seq+1`, `reason: "child watchdog tail timeout"`, `timed_out: Some(true)`,
then run `StartDrain` (upstream `:608-619`). The duration is `cfg.watchdog_tail_timeout_ms`
(default 120 000, `settings.rs:109`).

**Result surface** (upstream `result.watchdog`/`progress.watchdog`, `execution.ts:563-567`;
`step.watchdog`, `subagent-runner.ts:2720`, `:3508`): add `watchdog: Option<ChildWatchdogStateSnapshot>`
to `DriveOutcome` (`:19`) → `SingleResult` (`exec/run_result.rs:25`), plus `StepStatus`
(`background/records.rs:24`) via `apply_child_event_to_step` (`background/telemetry.rs:214`). No
v0.43.0 renderer reads `watchdog.phase`, so this is data only, but it is what the tests assert on.

**Child budget (see Blockers).** The shape is not decided here.

## 5. Call sites touched

- `exec/spawn_plan.rs:36-55`, `:1045-1058`: carry the config out
- `exec/attempt_runner.rs:150`, `:302`: hand it to the loop; fill `SingleResult.watchdog`
- `exec/drive_attempt.rs:19`, `:167-233`, `:285-418`, `:552-685`: the fold, the gate and the tail arm
- `exec/run_result.rs:25`: the field
- `background/telemetry.rs:214-272`, `background/records.rs:24`: `step.watchdog` in `status.json`
- `watchdog/child_status.rs:470-490`: delete the "No non-test caller yet" paragraph
- `PARITY-GAPS.md:2022`: the row

## 6. Tests, each with the mutation it kills

Use the real-subprocess harness already in place:
`crates/cyrup-it/tests/subagents/child_protocol_stream_integration.rs` (G76 at `:270-355` is the
exact template) with the `cyrup-subagent-fixture` script (`emit` / `sleep_ms`). Arm the child the
way `spawn_plan.rs:2295-2312` does, by writing
`<tmp>/.cyrup/settings.json = {"subagents":{"watchdog":{"enabled":true,"children":{"enabled":true,"watchdogTailTimeoutMs":1500}}}}`.
**Assert the env var is present before relying on it.** The unit test at `:2333-2339` silently
`return`s when the layer is not found, and that must not happen in these tests.

| # | Script | Assert | Kills |
|---|---|---|---|
| T1 | terminal-stop `message_end` → status `reviewing` (seq 1, identity = encoded config) → `sleep_ms 2500` → status `idle` (seq 2) → `agent_settled` → exit 0 | `result.watchdog == Some{phase: Idle, seq: 2}`; `process_signal.is_none()`; elapsed ≥ 2.5 s | deleting the fold (today's code: drained at 1 s, `watchdog` is `None`); `child_watchdog_is_active` → `false`; not clearing `final_drain_at` on an active event |
| T2 | terminal stop → `reviewing` → `sleep_ms 30000` | elapsed < 10 s; `watchdog == {phase: Stale, timed_out: Some(true), reason: "child watchdog tail timeout", seq: 2}` | removing the tail arm (the test hangs for 30 s); a tail that never calls `StartDrain` |
| T3 | as T1, but `runId: "someone-else"` | drained at about 1 s; `watchdog.is_none()` | dropping the identity filter |
| T4 | as T1, **unarmed** (no settings) | drained at about 1 s | dropping the `if (!childWatchdog) return` gate |
| T5 | terminal stop → `reviewing` → `agent_end{willRetry:true}` → `sleep 2000` → terminal stop → `agent_settled` | not force-drained; final output is the post-retry text | `CancelDrain` not clearing `watchdog_tail_at` (a stale tail fires mid-retry) |
| T6 | `reviewing` seq 2, then a replayed `idle` seq 1, then sleep 2500 | still held (a replay cannot walk the phase back) | removing the `seq <= current.seq` check at the call site |
| U1 | unit test: a `subagent.watchdog.status` line through `handle_child_line` does **not** call `control.note_activity` | `ControlMonitor` last-activity unchanged | moving the fold after `:396` (upstream returns before `lastActivityAt`) |

T1 is the row's observable. Run it against today's code and it must **fail**. That run is the
evidence that the test pins the gap and not the fixture.

A child-side test for the 5 s budget belongs to the blocker's decision, not to this list.

## 7. Honest size

- **Parent fold and plumbing: small-to-medium.** About 120 lines in `drive_attempt.rs`, about 30 of
  plumbing across `spawn_plan.rs` and `attempt_runner.rs`, about 40 for the result and status
  fields, and about 300 lines of tests. One loop covers both upstream sites (foreground and
  runner).
- **The 5 s child budget: a design decision. It is not code-sized until it is decided.** It touches
  `cyrup-ext`'s dispatch contract, which is shared by every extension. It is the hardest part of
  this row.
- **Priority:** keep it at *medium*. The feature is opt-in and off by default on both sides.
  **When a user turns it on and the child edits files, it fails every time and says nothing.**
  "Watched" children are never actually reviewed, and the run reports success.

## 8. Blockers, with proof

**B1 — the child's `AgentEnd` handler runs under a 5 s drop-budget that upstream does not have.**
The proof is `cyrup-ext/src/dispatch.rs:30`, `:521-535`; `facade.rs:294` (the only production
construction); `native.rs:83-86` (the gate is permission-only by construction);
`prompt_runtime.rs:2017` (the awaited review); `watchdog/runtime.rs:1091-1092` (a 30 s bound
inside it); `native_impl.rs:19-29` (the crate's own record of this failure mode).
Options, for the human to choose:
- (a) a sanctioned "long extension wait" guard, a generalized `HumanWaitGate`, which the watchdog
  holds for the review. The same guard fixes the main watchdog.
- (b) exempt `AgentEnd` notify dispatch from the budget and rely on the handler's own bound.
- (c) spawn the review off the handler and emit its terminal phase later. This **breaks** upstream's
  order (settle comes after review) and the auto-drain order at `:2043`, so it is not recommended.

Without B1 the parent fold is still correct and worth landing, since it fixes every review that
finishes within 1–5 s and makes the tail timer honest. But the observable is only half-fixed, and
the ledger must say so.

**B2 — none on identity**, provided the config is carried out of `spawn_plan` as in §4. If someone
"simplifies" to upstream's `options.index ?? 0`, every unindexed run breaks silently (T3 and T4 do
not catch that; T1 does).

## 9. Corrected row text

> **UW-3 · Child-watchdog status events are never read by the parent, so an armed child is killed mid-review** — *medium* · **STILL OPEN at `521beaa`; observable PROVEN by trace 2026-09-22**
> - **Re-greped 2026-09-22 at `521beaa`:** `is_child_watchdog_status_event` (`watchdog/child_status.rs:489`), `child_watchdog_is_active` (`:512`), `accept_child_watchdog_event` (`:531`) and `ChildWatchdogStateSnapshot`/`ChildWatchdogIdentity` have **zero** callers outside their module's `#[cfg(test)]` block (`:561`). The CONFIG half is wired: `exec/spawn_plan.rs:1045-1058` encodes `CHILD_WATCHDOG_CONFIG_ENV` into the child env. The child decodes it at `prompt_runtime.rs:2382` → `register_child.rs:247` (`spawn_plan.rs:2333-2341` is a *test*). The EMIT half is wired: `prompt_runtime.rs:2394`, `stdout_status_sink()` at `:2404`, which writes real fd 1 and not the output guard.
> - upstream: `pi-subagents/src/runs/foreground/execution.ts:846`, `:848`, `:857`, `:585`; `runs/background/subagent-runner.ts:626`, `:628`, `:640`, `:831`, `:2711-2712`; definitions `src/watchdog/child-status.ts:167`, `:181`, `:186`; emitter `src/watchdog/register-child.ts:102-110`. **All @v0.43.0, the tag `child_status.rs` is pinned to, and all re-verified 2026-09-22.** At v0.68.0 they are `child-status.ts:159`/`:182`/`:187`, `execution.ts:692`/`:1015`/`:1017`/`:1082`, `subagent-runner.ts:3016`/`:3017`/`:3033` (status.json half) and `runs/background/run-child-session.ts:315`/`:430`/`:432`/`:444` (drain half). At v0.68.0 the child is in-process and the events arrive through a `writeStatus` sink rather than stdout, and the fold is the same shape. **These are pinned coordinates: add, never renumber.**
> - cyrup: one drive loop serves foreground and background (`exec/drive_attempt.rs:552`, sole caller `attempt_runner.rs:150`). A status line parses to `SubagentEvent::Unknown` (`exec/ndjson.rs:245-246`) and is folded nowhere. `final_drain_at` is disarmed **only** by `agent_end{willRetry:true}` (`drive_attempt.rs:340-342`), so **no second holding mechanism exists**.
> - observable (**traced, not inferred**): for an armed child that edited files, its final `message_end{stop}` arms the 1000 ms window (`drive_attempt.rs:343-348`) before `AgentEnd` is dispatched. The child emits `reviewing` and starts a model review (`register_child.rs:206`, `watchdog/runtime.rs:1092`). At +1 s the parent sends SIGINT→SIGTERM→SIGKILL (`:647-654`), and `forced_drain_after_final_success` reports **exit 0** (`attempt_runner.rs:515-538`). The review, and every blocker/concern warning it would have injected, is lost without a trace. The child is not "treated as idle": the line still counts as activity (`exec/control.rs:1321`). It is drained as finished. Opt-in: `watchdog.enabled` and `children.enabled` both default `false`.
> - **Second defect behind it, not previously recorded:** the child's review is awaited inside an `AgentEnd` extension handler (`prompt_runtime.rs:2017`) that the dispatcher drops after `DEFAULT_INVOKE_BUDGET` = 5 s (`cyrup-ext/src/dispatch.rs:30`, `:530-533`). The review's own bound is 30 s. Porting the parent fold alone therefore fixes only reviews that finish in 1–5 s. See `.flux/todo/CHILD_STATUS_EVENTS.md` §8 B1. The main watchdog (`extension/host/native_impl.rs:604`) sits under the same budget and needs its own row.

## Out-of-scope notes, found and recorded

- `child_status.rs` doc cites `child-status.ts` ranges that are off by 1–2 from v0.43.0 (see §1).
- `register_child.rs:314-316` cites dead `prompt_runtime.rs:1687-1701` coordinates.
- The ledger's "three" `background/` watchdog references are four now (one is a test).
