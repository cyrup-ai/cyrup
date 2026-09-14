---
stage: aug
status: done
updated: 2026-09-13 09:52
---

# SCOPE_18 — the foreground child steer handle (`ForegroundChildControl.steer`)

> Created 2026-09-12 during [`WORKFLOW_7`](WORKFLOW_7.md)'s augment, which found this unowned.
> It is the **second orphan** in this family, the same shape as the async-workflow orphan
> [`WORKFLOW_1`](WORKFLOW_1.md) §4 records — and it was found by that file's own §5 rule #3
> (*"does every 'deferred to X' row have an X that ACCEPTS it?"*).
>
> **Re-augmented 2026-09-13 against the live tree @ `f8bec9ee`.** §0.5 below lists the five
> corrections that pass made. Two of them change what gets built, and one of those two is a
> **dead-drop defect**: the delivery arm as previously specified would have written the steer
> request into a queue that **nobody drains on the foreground path**.

OBJECTIVE: make a live foreground child steerable — port pi's `ForegroundChildControl.steer`
(`shared/types.ts:2127`), the two propagation assignments
[`WORKFLOW_6`](WORKFLOW_6.md) silently dropped, and the delivery arm
[`WORKFLOW_7`](WORKFLOW_7.md) §1.6 deliberately left on upstream's refusal.

**Depends on WORKFLOW_6** (`ForegroundChildEntry`, `active_children` — **LANDED**, verified
in-tree) and **WORKFLOW_7** (the resolver and the routing that reach this handle — **NOT LANDED**;
`extension/executor/workflow_steering.rs` does not exist and `control_is_live_in_workflow` has no
definition anywhere in the crate).

---

## §0 — Why this task exists, and why it is smaller than it looks

### 0.1 Nobody owned it

Three tasks look like the owner. None is:

| task | what its "steer" actually is | evidence |
|---|---|---|
| [`WORKFLOW_8`](WORKFLOW_8.md) SUBTASK2 | `async-steering-action.ts` — a **background** run | `asyncDir`-gated at [`:45`/`:52`](../../../workspace/pi-subagents/src/runs/foreground/async-steering-action.ts), `reconcileAsyncRun` at `:71`, `requestAsyncSteer` at `:149`. A run with no directory cannot enter it. |
| [`WORKFLOW_13`](WORKFLOW_13.md) §3.2 | `WorkflowScriptHost::steer` on a **second, async** host | *"flip `supports_steer()` and fill `steer`"* — on the async host only. cyrup's FOREGROUND `WorkflowRunHost` ([`workflow.rs:228`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs)) implements exactly two methods — `launch` (`:229`) and `status` (`:388`) — so `engine.rs:160-173`'s default refusal stays in force after W13 ships. |
| [`WORKFLOW_12`](WORKFLOW_12.md) `:113` | `op_runs_steer` — the V8 op binding | the isolate seam, not the delivery. |

⚠ **The naming trap:** WORKFLOW_8's subtask file is literally `foreground_actions/steer.rs`.
`foreground_actions/` names pi's **source directory** (`src/runs/foreground/`), not foreground runs.
Add a one-line note to that effect in WORKFLOW_8 so the next reader does not re-lose a week here.

### 0.2 How the orphan formed — two dropped lines

WORKFLOW_6 ported [`foreground-control.ts`](../../../workspace/pi-subagents/src/runs/foreground/foreground-control.ts)
`:39-60` (`syncCurrentChild`) and `:101-137` (`beginForegroundChild`). Both `steer` assignments sit
**inside** those exact ranges:

```ts
	control.steer = child.steer;                 // :60  — last line of syncCurrentChild
	if (input.steer) child.steer = input.steer;  // :128 — inside beginForegroundChild
```

WORKFLOW_6 §1.2 enumerated what it deliberately omitted — *"`inputTokens`/`outputTokens`/`window`/
`windowPeak`/`model`/`thinking`/`lastActivityAt`/`currentToolStartedAt`/`detach`"* — and **`steer`
is not in that list.** It was dropped silently, not by decision. That is the whole defect. The
omission list is still on the live `sync_current_child` doc comment at
[`foreground_control.rs:48-51`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs);
`steer` must be added to the ported body, not to that list.

### 0.3 The transport already exists end to end — with ONE hop missing

A foreground child is **a spawned `cyrup` process going through the same code as a background
one**:

```
drive_foreground_run_sync (paths.rs:300)
  └─> crate::exec::run_sync                         ← the SAME entry the background runner uses
        ├─ exec/mod.rs:1117-1135  create_dir_all(steer_inbox_dir | steer_ack_dir | capability parent)
        ├─ spawn_plan.rs:1300-1340 env[CYRUP_SUBAGENT_STEER_INBOX / _ACK_DIR / _CAPABILITY]
        └─ child: prompt_runtime.rs:2336-2349 reads the three env vars
             └─ SteeringInbox → injects each message into its live model turn (`:583`)
                             → writes SteerAck (`:326`) and SteerCapability (`:334`)
```

Every hop above is generic. The child side has **no** foreground/background distinction — it reads
three env vars and attaches a watcher. `SteerAck`/`SteerCapability`/`SteerAckState`
(`control.rs:1476`, `:1506`, `:1448`) are equally generic.

The reason the feature is dead on the foreground path is **three hard-coded `None`s**:

```rust
// foreground.rs:865-872 — the justification is CIRCULAR
// G90: a FOREGROUND single run has no async run directory and therefore no steer
// inbox — pi supplies `steerInboxDir` only from the background runner, and
// `control_steer` refuses a foreground run outright for exactly this reason.
steer_inbox_dir: None,
// SUBA-049: same reason, for the return half — a foreground run has no run directory,
// so there is nowhere to write an acknowledgment or a capability record.
steer_ack_dir: None,
steer_capability_path: None,
```

The refusal is justified by the absence and the absence by the refusal. Neither premise is a
mechanism constraint: *"a foreground run has no run directory"* is true of the **async** run tree
only, and cyrup already gives every run's `cwd` a scratch root
([`attempt_scratch_dir`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/artifact_roots.rs),
`artifact_roots.rs:325-337`), created by `exec::run_sync` at `exec/mod.rs:1101`.

### 0.4 ⚠ THE MISSING HOP — this is the defect the previous augment did not catch

**On the background path, the parent does NOT write into the child's inbox. A RUNNER routes it
there.** Two directories, two hops:

```
parent:  request_async_steer_with_mode(run_dir, …)        control.rs:1258
             └─ writes → steer_requests_dir(run_dir)      control.rs:1165  = <run_dir>/control/steer-requests/
RUNNER:  route_steer_requests(…)                          runner_main/control_watcher.rs:334
             ├─ consume_steer_requests(run_dir)           control.rs:1354   (drains the run-level queue)
             └─ enqueue_step_steer(run_dir, index, req)   control.rs:1293  → step_steer_inbox_dir(run_dir, index)
CHILD:   SteeringInbox::flush → consume_steer_requests_from_dir(self.dir)   prompt_runtime.rs:583
```

`control_watcher.rs:762-769` states the agreement as a named invariant: *"`route_steer_requests`
writes an accepted request into `control::step_steer_inbox_dir(run_dir, index)`; `run_single` hands
the child `steer_inbox_for(index)`. If those two ever diverge … the feature is silently dead again,
with no test failing."*

**A foreground run has no runner process.** `drive_foreground_run_sync` spawns the child directly;
nothing in this process drains `<fg_control_dir>/control/steer-requests/`. So reusing
`control_steer`'s `request_async_steer_with_mode` (as §4 previously implied) writes the request into
a **dead drop**: the file lands on disk, no one moves it, the child never sees it,
`await_steer_ack` times out, and the tool honestly reports `queued` forever. That is a silent
regression of exactly the shape `control_watcher.rs:762` warns about.

**The fix is one new `pub` helper (§1.2): the parent writes DIRECTLY into the per-child inbox**,
because on this path the parent IS the router. `enqueue_step_steer` already performs that write —
it just needs a minted id and a request to carry, and `next_steer_request_id` is private
(`control.rs:1217`).

### 0.5 Corrections this re-augment makes to the previous text

| # | previous claim | truth | impact |
|---|---|---|---|
| 1 | *"reuse `control_steer`'s existing ack wait and request write"* | the request write targets a queue **no foreground process drains** (§0.4) | **behavioural — the feature would ship dead** |
| 2 | `control::steer_ack_dir(&fg_control_dir, 0)` | **no such function.** It is `control::steer_acks_dir` (plural, `control.rs:1536`). Only the `RunOptions` FIELD is singular (`agent_config.rs:599`). | **compile error as written** |
| 3 | §5: *"reusing WORKFLOW_7's `control_is_live_in_workflow`"* | upstream's `steerWorkflowChildByKey` uses a **different** predicate (`subagent-executor.ts:4491-4493`): `parentWorkflowRunId && workflowKey && activeChildren.size`. `controlIsLiveInWorkflow` (`workflow-foreground-steering.ts:32-36`) compares `sessionId`, **not** `workflowKey` — it cannot select a lane. | **behavioural — wrong child, or none** |
| 4 | §4/§6: *"pi's three receipts … never a fourth state"* | true of the **tool** receipt. `WorkflowSteerState` (`scripted/types.rs:99-108`) has **four**: `Queued`/`Delivered`/`Missed`/`Failed`, and `Missed` is load-bearing in §5 (`:4512`, `:4531`, `:4534`, `:4537`). | §5 was underspecified |
| 5 | §1: *"It does have a scratch dir"* (per-run) | `attempt_scratch_dir(cwd)` is **per-`cwd`, not per-run** (`artifact_roots.rs:333-337`) — shared by every run in that directory. The `fg-<run_id>/` leaf IS the per-run partition, and that is precisely why it must be created and removed by this task. | clarification |

Also verified-still-true: every `None` at `foreground.rs:868/871/872`, the circular comment at
`:865-867`, the stale docs at `agent_config.rs:583-586` and `prompt_runtime.rs:1409-1412`, the
*"Absent on the foreground path"* comment at `spawn_plan.rs:1304-1305`, and
`ForegroundChildEntry`'s single `interrupt` handle at `foreground_control.rs:43`.

### 0.6 Scope boundary — what stays refused

A **plain** (non-workflow) foreground run addressed by `action:"steer"` stays refused with
[`STEER_FOREGROUND_RUN_REFUSAL`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/tool/text.rs)
(`text.rs:116`, fired at `control.rs:574`). That is upstream-faithful: pi refuses the same case at
`subagent-executor.ts:3217`, and the **only** routes to `child.steer` in pi are
`resolveWorkflowForegroundSteeringTarget` (requires `parentWorkflowRunId`,
`workflow-foreground-steering.ts:47`) and `steerWorkflowChildByKey` (requires a `workflowRunId` +
lane key, `:4491`). Do not widen it.

pi's third consumer — the FleetView TUI's `actions.steer` (`tui/fleet.ts:1395`) — is **out of
scope**; cyrup's fleet TUI has no steer action to wire it to yet.

---

## §1 SUBTASK1 — a control tree for a foreground run, and a direct-drop request writer

### 1.1 `foreground_run_control_dir`

**File:** [`background/artifact_roots.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/artifact_roots.rs),
beside `attempt_scratch_dir` (`:325`).

```rust
/// `<temp_root_dir>/scratch/<cwd_key>/fg-<run_id>/` — the run-directory-shaped root a FOREGROUND
/// run's steer control tree hangs off. The `control/…` leaves below it are appended by
/// [`crate::background::control::step_steer_inbox_dir`] and friends, which is why this returns the
/// run root and NOT the `control/` directory itself.
///
/// Why not the async run tree: `resume_tracking` (`extension/executor/status.rs:26-32`) does a
/// `read_dir` over the whole async root and treats every entry as a run to reconcile, so a
/// foreground run with a directory there would surface as a status-less async run. Why the
/// per-`cwd` scratch root: it is already `run_sync`'s own working area (`exec/mod.rs:1101`),
/// already keyed by [`cwd_key`], already outside the project tree, and already disposable.
///
/// ⚠ [`attempt_scratch_dir`] is per-`cwd`, NOT per-run — every run in a directory shares it. The
/// `fg-<run_id>` leaf IS the per-run partition, which is exactly why §1.4's teardown must remove
/// this path and never its parent.
///
/// Layout mirrors the background tree byte for byte so `step_steer_inbox_dir`/`steer_acks_dir`/
/// `steer_capability_path` (`background/control.rs:1174`, `:1536`, `:1530`) are reused UNCHANGED —
/// each takes a `run_dir` and appends `control/…` via `control_inbox_dir` (`:618`), and none of
/// them knows or cares that this one is not under the async root.
#[must_use]
pub fn foreground_run_control_dir(cwd: &Path, run_id: &RunId) -> PathBuf {
    attempt_scratch_dir(cwd).join(format!("fg-{}", run_id.as_str()))
}
```

Resulting paths, for `index = 0`:

```
<temp>/scratch/<cwd_key>/fg-<run_id>/control/steer-targets/0/        ← inbox   (the child polls this)
<temp>/scratch/<cwd_key>/fg-<run_id>/control/steer-acks/0/           ← acks    (the child writes here)
<temp>/scratch/<cwd_key>/fg-<run_id>/control/steer-capabilities/0.json ← capability
```

Index is always `0`: a foreground SINGLE run has exactly one child at flat index 0, which is the
same fact `register_foreground_controls` already encodes (`foreground.rs:949`,
`begin_foreground_child` with `index: 0`) and the same default the child itself falls back to
(`prompt_runtime.rs:2347-2349`, `CHILD_INDEX_ENV` → `unwrap_or(0)` — and `foreground.rs:832` does
set `child_index: Some(0)`, so the two agree explicitly rather than by luck).

### 1.2 `request_direct_steer` — the parent-as-router write (§0.4)

**File:** [`background/control.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs),
immediately after `enqueue_step_steer` (`:1293`), where both halves it fuses already live.

```rust
/// Write one steer request STRAIGHT into a child's own inbox, minting the id the caller needs to
/// wait for the acknowledgment.
///
/// This is [`request_async_steer_with_mode`] and [`enqueue_step_steer`] fused, with the runner hop
/// removed — because on the FOREGROUND path there is no runner. `drive_foreground_run_sync` spawns
/// the child in-process (`extension/executor/paths.rs:300`); nothing drains
/// [`steer_requests_dir`], so a request written there is a dead drop that the child never sees and
/// the parent's ack wait can only ever time out on. The parent IS the router here, so it writes
/// where the router would have written: [`step_steer_inbox_dir`], the exact directory
/// `run_sync` handed the child as `CYRUP_SUBAGENT_STEER_INBOX` (`exec/spawn_plan.rs:1306`).
///
/// `target_index` is PINNED to `index`, exactly as [`enqueue_step_steer`] pins it: a request
/// sitting in a per-child inbox is addressed, and the child's own validator reads the field back.
///
/// [CYRUP-DELTA: a second producer for an existing directory, not a second channel. The request
/// shape, the file name, the ordering rule and the inbox path are all pi's — only the hop that
/// would have moved the file is elided, because on this path the two hops collapse to one.]
///
/// # Errors
///
/// [`SubagentError::Management`] for an empty message (upstream's `steer message must not be
/// empty.`), or [`SubagentError::Spawn`] for an I/O failure.
pub async fn request_direct_steer(
    run_dir: &Path,
    index: usize,
    message: &str,
    mode: Option<SteerDeliveryMode>,
    source: Option<&str>,
) -> Result<(PathBuf, String), SubagentError> {
    let message = message.trim();
    if message.is_empty() {
        return Err(SubagentError::Management(
            "steer message must not be empty.".to_string(),
        ));
    }
    let request = SteerRequest {
        kind: "steer".to_string(),
        id: next_steer_request_id(),
        ts: crate::time::now_epoch_millis(),
        message: message.to_string(),
        // `Steer` is normalised OFF the wire, matching upstream's conditional spread — see
        // `SteerRequest::mode`'s own doc (`:1140-1149`).
        mode: mode.filter(|m| *m != SteerDeliveryMode::Steer),
        target_index: Some(index),
        source: source.map(str::to_string),
    };
    let id = request.id.clone();
    let path = write_steer_request_to_dir(&step_steer_inbox_dir(run_dir, index), &request).await?;
    Ok((path, id))
}
```

**Do not** widen `next_steer_request_id`'s visibility and **do not** rebuild a request in
`workflow_steering.rs`: the monotonic-sequence tiebreak documented at `control.rs:1128-1138` is the
only thing that makes same-millisecond guidance deterministically ordered, and a second minting site
is a second place for it to drift.

### 1.3 Wiring it into `run_foreground_impl`

The path must be computed **once**, before `build_foreground_run_options`, and reach three places:
the `RunOptions` (§2), the control entry (§3), and teardown (§1.4). The natural seam is
[`resolve_run_channels`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs)
(`:612-710`), which already mints `run_id` and already returns a `RunChannels` bundle every later
stage reads — add `fg_control_dir: PathBuf` to it and compute it beside `session_dir` (`:695-697`).
`ForegroundRunOptionsInput` (`:103-140`) then gains a borrowed `fg_control_dir: &'a Path`, matching
its existing owned/borrowed convention (borrowed = the caller still needs it, which is true here
because §1.4 tears it down).

Creation is **not** this task's job: `exec::run_sync` already does `create_dir_all` for all three
paths (`exec/mod.rs:1117-1135`), unconditionally and best-effort, on every path including this one.
Do not add a fourth `create_dir_all`.

### 1.4 Teardown

Remove the tree in [`settle_foreground_run`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs)
(`:1003-1040`), after the control entry is dropped.

⚠ `settle_foreground_run(&self, run_id, control_notifier)` does **not** take a `cwd` — and it must
not grow one. The entry it removes at `:1018` already carries it: `ForegroundControlEntry::cwd`
(`notices.rs:83-84`), populated from `ForegroundControlIdentity` at `foreground.rs:945`. Derive the
path from the entry being dropped:

```rust
            if let Some(entry) = controls.remove(run_id.as_str()) {
                tracing::debug!( /* … unchanged … */ );
                // WORKFLOW_14 §1.4 — the steer control tree is per-RUN (`fg-<run_id>/` under the
                // per-`cwd` scratch root), so it is removed with the run and never with its
                // parent, which every other run in this directory shares. Derived from the entry's
                // OWN `cwd` rather than a new parameter: that field exists for exactly this kind
                // of reader (`notices.rs:83-84`).
                //
                // Best-effort by construction. The tree is disposable — it holds consumed request
                // files, drained acks and one capability record — and a removal failure must never
                // turn a settled run into a failed one. The scratch root is scoped-temp, so the
                // worst case of a leak is an empty directory the OS reaps.
                if let Some(cwd) = entry.cwd.as_deref() {
                    let dir = crate::background::foreground_run_control_dir(cwd, run_id);
                    let _ = tokio::fs::remove_dir_all(&dir).await;
                }
            }
```

⚠ The block at `:1013-1031` is a `std::sync::Mutex` critical section. `remove_dir_all` is `.await`,
so it must be issued **after** the lock guard is dropped — bind the `cwd` inside the block, close
the scope, then remove. Holding a `std::sync::Mutex` across an `.await` is exactly what the
`foreground_controls` doc (`mod.rs:145-147`) says never happens.

Do **not** extend this to the scratch root itself — `run_foreground_impl:365-380` carries a long
comment explaining why an earlier revision's `remove_dir_all` of the whole scratch dir was a defect
(it discarded the per-attempt stdout tee that `cyrup-it`'s integration tests read back). This
removal is strictly the `fg-<run_id>/` leaf.

## §2 SUBTASK2 — populate the three `RunOptions` fields

**File:** [`foreground.rs:865-872`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs),
in `build_foreground_run_options`.

```rust
            // WORKFLOW_14 — the pre-task comment here was CIRCULAR: it justified the absence with
            // `control_steer`'s refusal and the refusal with the absence. Neither premise is a
            // mechanism constraint. `run_sync` creates all three of these
            // (`exec/mod.rs:1117-1135`) and `spawn_plan` hands them to the child as
            // `CYRUP_SUBAGENT_STEER_INBOX`/`_ACK_DIR`/`_CAPABILITY` (`spawn_plan.rs:1300-1340`) on
            // EVERY path — a foreground child is a spawned `cyrup` process exactly like a
            // background one, and the child-side `SteeringInbox` has no foreground/background
            // distinction at all (`prompt_runtime.rs:2336-2349`).
            //
            // The tree is NOT under the async run root: `resume_tracking` reads that root
            // directory-wise (`status.rs:26-32`) and would surface a foreground run there as a
            // status-less async run. See `foreground_run_control_dir` (`artifact_roots.rs`).
            //
            // Index 0: a foreground SINGLE run has exactly one child, the same fact
            // `register_foreground_controls` encodes with `begin_foreground_child { index: 0 }`
            // (`:949`) and `child_index: Some(0)` (`:832`) hands the child.
            steer_inbox_dir: Some(control::step_steer_inbox_dir(input.fg_control_dir, 0)),
            steer_ack_dir: Some(control::steer_acks_dir(input.fg_control_dir, 0)),
            steer_capability_path: Some(control::steer_capability_path(input.fg_control_dir, 0)),
```

⚠ `steer_acks_dir` is **plural** (`control.rs:1536`). There is no `steer_ack_dir` function; the
singular name belongs only to the `RunOptions` field (`agent_config.rs:599`) and the env var
(`prompt_runtime.rs:2344`). Correction #2 of §0.5.

**Three doc claims become false with this change and must be corrected in the same commit** — all
three are load-bearing documentation a reader will trust over the code:

1. [`agent_config.rs:583-586`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/agent_config.rs)
   — *"`None` on the foreground path (no async run directory exists), matching upstream's own
   `if (input.steerInboxDir)` guard."* Becomes: `Some` on every path; the guard survives for
   embedders that supply no control tree.
2. [`prompt_runtime.rs:1409-1412`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/prompt_runtime.rs)
   — *"`Some` only when the parent handed this child a `STEER_INBOX_ENV` path — i.e. only for a
   background/async child, which is the only kind that has an async run directory to steer
   through."* The parenthetical is now simply wrong; the first clause stays.
3. [`spawn_plan.rs:1304-1305`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/spawn_plan.rs)
   — *"Absent on the foreground path, so a foreground child is byte-identical to before."* and the
   matching *"a foreground child — which has no run directory and therefore neither path — is
   byte-identical to before"* at `:1319-1320`.

## §3 SUBTASK3 — the handle on `ForegroundChildEntry`

**File:** [`foreground_control.rs:43`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs),
next to `interrupt`.

pi's shape (`shared/types.ts:2127`) is
`steer?: (input: ForegroundSteerInput) => Promise<ForegroundSteerOutcome>` — a closure, because pi's
foreground child is an **in-process `ChildSession`** whose `steer`/`followUp` methods are direct
calls (`runs/shared/child-session.ts:324-325`, closed over at
`subagent-executor.ts:3859-3870`). cyrup's child is a **spawned process**, so its equivalent of that
in-process session handle IS the file protocol it was handed. Data, not a trait object: no `dyn`,
no new implementor, no second transport.

```rust
    /// pi `ForegroundChildControl.steer` (`shared/types.ts:2127`) — this child's live steer
    /// channel. `Some` once the parent gave the run a control tree (WORKFLOW_14 §1); `None` keeps
    /// upstream's own `if (!child.steer)` refusal reachable
    /// (`workflow-foreground-steering.ts:144`) for a child registered without one.
    ///
    /// The three paths ARE the handle. pi closes over a function because its child is an
    /// in-process `ChildSession` (`runs/shared/child-session.ts:324`); cyrup's child is a spawned
    /// `cyrup` process, so the channel is the file protocol it was handed at spawn — the same
    /// three paths `build_foreground_run_options` put on its `RunOptions` and `spawn_plan` put in
    /// its environment.
    pub(crate) steer: Option<ForegroundChildSteer>,
```

```rust
/// pi's `ForegroundSteerInput`/`ForegroundSteerOutcome` pair (`shared/types.ts:2130-2140`),
/// expressed as the addresses the existing protocol already uses.
///
/// Cloned into `ForegroundControlEntry` by `sync_current_child`, so three `PathBuf`s and a `usize`
/// rather than an `Arc`: the clone happens once per control event on a struct that is already
/// `Clone`, and an `Arc` here would buy indirection without removing a single allocation that
/// matters.
#[derive(Clone)]
pub(crate) struct ForegroundChildSteer {
    /// `control/steer-targets/<index>/` — where a request is DROPPED (§1.2); the exact directory
    /// the child was handed as `CYRUP_SUBAGENT_STEER_INBOX`.
    pub(crate) inbox_dir: PathBuf,
    /// `control/steer-acks/<index>/` — where the child ANSWERS (`prompt_runtime.rs:326`).
    pub(crate) ack_dir: PathBuf,
    /// `control/steer-capabilities/<index>.json` — whether the child can be steered AT ALL, and
    /// under which pid. This is what distinguishes "has not booted yet" from "cannot be steered",
    /// which is the whole of §4.3.
    pub(crate) capability_path: PathBuf,
    /// The flat child index — always `0` for a foreground SINGLE run (§1.1).
    pub(crate) index: usize,
    /// The run-directory-shaped root the three paths above were derived from
    /// (`foreground_run_control_dir`). Carried rather than re-derived because
    /// `take_steer_acks`/`read_steer_capability` take a `run_dir`, not a leaf
    /// (`control.rs:1706`, `:1671`).
    pub(crate) run_dir: PathBuf,
}
```

**Restore the two dropped assignments** (§0.2), in the functions that already exist:

```rust
// `sync_current_child` (:50) — pi `foreground-control.ts:60`, the line WORKFLOW_6 dropped.
// Unconditional, exactly as upstream's is: this function's contract is "the live child's state IS
// the run's state", and a conditional here would strand a stale handle after a child swap.
entry.steer = child.steer.clone();
```

```rust
// `begin_foreground_child` (:67) — pi `:128`, `if (input.steer) child.steer = input.steer`.
// Guarded exactly as upstream guards it, so a `None` never clears an existing handle. Since the
// child is MOVED in here, the guard is on the CALLER's side in cyrup: `register_foreground_controls`
// builds `ForegroundChildEntry { steer: Some(..), .. }` and the field simply travels with it.
```

`ForegroundControlEntry` ([`notices.rs:20`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs))
gains the matching `steer: Option<ForegroundChildSteer>` so `sync_current_child` has somewhere to
sync it to — pi's `ForegroundRunControl.steer` (`shared/types.ts:2187`, declared as
`steer?: ForegroundChildControl["steer"]`). Both existing test fixtures that build the struct
literally — `foreground_control.rs`'s `base_entry()`/`child()` (`:132-171`) — need the new field.

`register_foreground_controls` (`foreground.rs:903`) fills it at `:949`, from the same
`fg_control_dir` §1.3 threaded in:

```rust
                    index: 0,
                    // WORKFLOW_14 §3 — the same three paths `build_foreground_run_options` put on
                    // this run's `RunOptions`, derived from the ONE `fg_control_dir` resolved in
                    // `resolve_run_channels`. Never re-derived here: two derivations of the same
                    // path is exactly the divergence `control_watcher.rs:762-769` names as the way
                    // this feature dies silently.
                    steer: Some(ForegroundChildSteer {
                        inbox_dir: control::step_steer_inbox_dir(fg_control_dir, 0),
                        ack_dir: control::steer_acks_dir(fg_control_dir, 0),
                        capability_path: control::steer_capability_path(fg_control_dir, 0),
                        index: 0,
                        run_dir: fg_control_dir.to_path_buf(),
                    }),
```

## §4 SUBTASK4 — real delivery, replacing WORKFLOW_7 §1.6's refusal

**File:** `extension/executor/workflow_steering.rs` — **created by WORKFLOW_7**, which has not
landed. This task replaces exactly one line of it: the arm WORKFLOW_7 §1.6 leaves at
`Foreground run '{id}' child {i} does not support steering.`

Port [`steerWorkflowForegroundTarget`'s delivery half, `:146-168`](../../../workspace/pi-subagents/src/runs/foreground/workflow-foreground-steering.ts).

### 4.1 The shape

```rust
    // pi `:142-144` — unchanged from WORKFLOW_7 §1.6, and the `index` defaulting ABOVE this point
    // (`:135-141`) is already specified and landed by that task. Do not rewrite it.
    let Some(child) = target.control.active_children.get(&index) else {
        return Err(format!("Foreground run '{run_id}' child {index} is not live."));
    };
    // pi `:144` — `ForegroundChildControl.steer` is OPTIONAL upstream and stays optional here.
    // This is the branch WORKFLOW_7 shipped as the whole arm; it survives as the fallback.
    let Some(steer) = child.steer.clone() else {
        return Err(format!(
            "Foreground run '{run_id}' child {index} does not support steering."
        ));
    };

    // pi `:146-147`. cyrup mints the id inside `request_direct_steer` rather than ahead of it,
    // because the id is also the ORDERING tiebreak (`control.rs:1128-1138`) and a second minting
    // site is a second place for that to drift.
    let message = message.trim();
    let (_, request_id) = control::request_direct_steer(
        &steer.run_dir,
        steer.index,
        message,
        mode,
        Some("steer-action"),
    )
    .await
    .map_err(|e| e.to_string())?;

    // pi `:149`'s `await child.steer(...)` resolves the outcome synchronously because its child is
    // in-process. cyrup's child is a process, so the outcome arrives as a file: the SAME
    // `await_steer_ack` + 3s `STEER_ACK_TIMEOUT` (`text.rs:177`) `control_steer` already uses
    // (`control.rs:660`, `:690`). Narrowed to this request id, which is what keeps two concurrent
    // steers from consuming each other's answers (`take_steer_acks`'s own `[CYRUP-DELTA]`,
    // `control.rs:1692-1705`).
    let outcome = Self::await_steer_ack(&steer.run_dir, &request_id, Some(steer.index)).await;
```

⚠ `await_steer_ack` is a **private** inherent method on `SubagentExecutor`
([`control.rs:690`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/control.rs)).
`workflow_steering.rs` is a sibling module, so it must become `pub(crate)` (or
`pub(in crate::extension::executor)`). Change the visibility; do **not** copy the function.

### 4.2 The receipt mapping — exact, already

cyrup's [`SteerAckState`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs)
(`control.rs:1448-1458`) has precisely pi's three states:

| pi `outcome.state` (`:150-155`) | cyrup `SteerAckState` | receipt (`:163`/`:166`/`:168`) |
|---|---|---|
| `delivered` | `Delivered` | `Steering delivered for foreground run {id} (request {rid}).` |
| `queued` | `Queued` | `Steering queued for foreground run {id} (request {rid}).` |
| `failed` | `Failed` | `Steering failed for foreground run {id} (request {rid}): {reason}` + `isError` |
| — (no ack inside the budget) | `None` | the **queued** receipt, verbatim |

The no-ack fallback is upstream's stated contract at `workflow-foreground-steering.ts:112-114`:
*"No acknowledgment leaves an honest, unaddressed queued receipt."* `control_steer` already renders
that case as the word `pending` (`control.rs:664-667`) — that is the **async** surface's own text
and is correct there; this surface uses **`queued`**, because pi's foreground receipt has no
`pending` word (`:158`: `deliveryStatus: outcome.state === "delivered" ? "delivered" : "queued"`).
Do not unify the two and do not invent a fourth word.

### 4.3 `CHILD_SESSION_NOT_RUNNING_YET` — the state pi has and this port must reproduce

pi's foreground steer closure has a fourth branch the receipt table hides
([`subagent-executor.ts:3860-3861`](../../../workspace/pi-subagents/src/runs/foreground/subagent-executor.ts)):

```ts
steer: async (input) => {
    if (!childSessionControls) return { state: "failed", reason: CHILD_SESSION_NOT_RUNNING_YET };
```

`CHILD_SESSION_NOT_RUNNING_YET = "Child session is not running yet."` (`:4336`). It exists because
**the control registers before the child session does** — `beginForegroundChild` runs at `:3842`,
`onChildSession` fires at `:3899` — and `steerWorkflowChildByKey` **polls on exactly that reason**
(`:4501-4502`) rather than treating it as a failure.

cyrup's signal for the same fact is the **capability record**, which is precisely what it was built
for (`control.rs:1668-1670`: *"`None` when the child has not reached its runtime yet"*):

```rust
/// pi's `CHILD_SESSION_NOT_RUNNING_YET` (`subagent-executor.ts:4336`), resolved from the child's
/// published capability instead of an in-process session handle.
///
/// Three distinguishable facts, and collapsing any two of them is how this surface lies:
///   None                     → the child has not reached its runtime yet → pi's "not running yet"
///   Some(c) if !c.supported  → its host cannot inject messages at all    → a genuine `failed`
///   Some(c)                  → steerable
///
/// The `pid` on the record (`control.rs:1527`) makes a stale file from a dead process detectable;
/// this surface does not need it, but do not drop it from the read.
const CHILD_SESSION_NOT_RUNNING_YET: &str = "Child session is not running yet.";
```

Consume it **before** the request write, so the poll in §5 has something to poll on:

```rust
    match control::read_steer_capability(&steer.run_dir, steer.index).await {
        // pi `:3861` — the control exists, the child does not yet. NOT a refusal: §5 retries this
        // until the ack deadline, exactly as `steerWorkflowChildByKey` does at `:4502`.
        None => return Err(CHILD_SESSION_NOT_RUNNING_YET.to_string()),
        // pi's `catch` arm at `:3867-3869`, reached here at spawn time rather than call time
        // because cyrup's child publishes the answer instead of throwing it.
        Some(cap) if !cap.supported => {
            return Err(format!(
                "Steering failed for foreground run {run_id} (request -): child {index} cannot be steered."
            ));
        }
        Some(_) => {}
    }
```

⚠ The capability is republished on every `activate`, not only at `start`
(`prompt_runtime.rs:334-345`'s `publish_capability` doc: `set_host_services` is late-bound, so a
single publish at `session_start` would pin `supported: false` on every child). So a `None` read is
genuinely "not yet", never "never".

## §5 SUBTASK5 — `WorkflowRunHost::steer`, the second consumer

**File:** [`workflow.rs:228`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs),
in `impl WorkflowScriptHost for WorkflowRunHost`, beside `launch` (`:229`) and `status` (`:388`).

Upstream is **`steerWorkflowChildByKey`** ([`subagent-executor.ts:4477-4541`](../../../workspace/pi-subagents/src/runs/foreground/subagent-executor.ts)),
wired as the host's `steer` at `:5710` and `:5960`. That is the port target — **not**
`resolveWorkflowForegroundSteeringTarget`.

### 5.1 ⚠ It is a DIFFERENT predicate (correction #3 of §0.5)

```ts
// :4491-4493
const control = [...state.foregroundControls.values()].find((candidate) =>
	candidate.parentWorkflowRunId === input.workflowRunId
	&& candidate.workflowKey === input.key
	&& (candidate.activeChildren?.size ?? 0) > 0);
```

versus WORKFLOW_7's `controlIsLiveInWorkflow` (`workflow-foreground-steering.ts:32-36`):

```ts
control.parentWorkflowRunId === workflowRunId && control.sessionId === sessionId && activeChildren.size > 0
```

The middle term differs: **`workflowKey` here, `sessionId` there.** Reusing WORKFLOW_7's predicate
would select *any* live child of the workflow regardless of lane — wrong child on a multi-lane
script, and `Missed` on none. cyrup's `ForegroundControlEntry` carries both fields
(`notices.rs:73`, `:79`), so write the three-term find upstream writes. The session term is
unnecessary because a `WorkflowRunHost` **is** this session's host: it was constructed for one tool
call, and `workflow_run_id` (`workflow.rs:118`) is the only workflow it can ever name.

### 5.2 ⚠ It must POLL, and that is what makes the method non-vacuous

`WorkflowRunHost::launch` **blocks until its child settles** (`workflow.rs:353` `.await`), so
`runs.steer(key, …)` is only ever reachable from a **concurrent lane** of the guest script — the
engine drives lanes on one event loop and `run_steer` hops to main via `shared.on_main`
(`engine.rs:1118-1124`). The steered lane may therefore be anywhere in its life, including
*registered but not yet spawned*. Upstream's loop (`:4490`, `:4539`) exists for exactly that, and
its comment at `:4501` says so: *"The control registers before its child session exists; keep
polling until the steer can route."*

```rust
    /// pi `options.steer` (`engine.rs:165`), implemented as `steerWorkflowChildByKey`
    /// (`subagent-executor.ts:4477`). The foreground host CAN steer now (WORKFLOW_14 §4);
    /// WORKFLOW_13 answers the same two methods on its own ASYNC host, separately.
    fn supports_steer(&self) -> bool {
        true
    }

    async fn steer(
        &self,
        key: &str,
        message: &str,
        options: WorkflowSteerOptions,
        cancel: CancelToken,
    ) -> Result<WorkflowSteerResult, String> {
        // pi `:4488-4489` — ONE budget for the whole loop, not per attempt.
        let deadline = std::time::Instant::now()
            + options
                .ack_timeout_ms
                .map_or(STEER_ACK_TIMEOUT, std::time::Duration::from_millis);
        loop {
            // pi `:4491-4493`, verbatim three terms. `.clone()`d out of the lock — every access to
            // `foreground_controls` is a short synchronous section with no `.await` inside it
            // (`mod.rs:145-147`).
            let found = { /* scan self.executor.foreground_controls for
                             (parent_workflow_run_id == self.workflow_run_id)
                             && (workflow_key == key)
                             && !active_children.is_empty(), returning (run_id, entry) */ };

            if let Some((run_id, entry)) = found {
                match self.steer_foreground_target(&run_id, &entry, message, &options).await {
                    // pi `:4502` — the ONLY retryable outcome. Anything else is the answer.
                    Err(reason) if reason == CHILD_SESSION_NOT_RUNNING_YET => {}
                    other => return Ok(workflow_steer_receipt(key, other)),
                }
            }

            // pi `:4536-4537`. cyrup has no async status file for a FOREGROUND child, so the
            // status-derived `missed` arms (`:4512`, `:4531`, `:4534`) collapse into this one —
            // the lane has no live steering route inside the budget, which is the same fact those
            // three report by three different routes.
            if cancel.is_cancelled() || std::time::Instant::now() >= deadline {
                return Ok(WorkflowSteerResult {
                    key: key.to_string(),
                    state: WorkflowSteerState::Missed,
                    error: Some(format!("Workflow child '{key}' had no live steering route.")),
                    ..Default::default()
                });
            }
            // pi `:4539` — `Math.min(10, …)`; the same 10 ms, not `STEER_ACK_POLL_INTERVAL`
            // (that one paces the ACK read inside `await_steer_ack`, a different loop).
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
```

⚠ `WorkflowSteerResult` (`scripted/types.rs:126-140`) has no `Default` derive today. Either add one
or construct every field explicitly — do not `unwrap`.

### 5.3 The FOUR-state receipt (correction #4 of §0.5)

Port `workflowSteerReceipt` (`subagent-executor.ts:4319-4334`). `WorkflowSteerState`
(`scripted/types.rs:99-108`) has four variants, and `Missed` is only reachable from §5.2's loop:

| condition | `WorkflowSteerState` | source |
|---|---|---|
| ack `Delivered` | `Delivered` | `:4325` |
| ack `Queued`, or no ack inside the budget | `Queued` | `:4325` |
| ack `Failed`, or the delivery arm errored | `Failed` | `:4323-4324` |
| no live route before the deadline / cancelled | **`Missed`** | `:4537` |

`targets` is a single-element `Vec<WorkflowSteerTarget>` (`types.rs:112-121`) carrying
`{ index, state, reason? }` — pi `:4331`. `request_id` and `delivery_status` come straight off the
ack (`:4329-4330`).

Without this subtask the feature is unreachable by a user: a workflow script's `runs.steer` returns
`"Workflow steering is unavailable in this host."` from `engine.rs:1106-1108`, gated on
`shared.host.supports_steer()` at `:1089`.

---

## §6 Behavioural contract

| # | invariant | source |
|---|---|---|
| 1 | A foreground run is handed a real `steer_inbox_dir`/`steer_ack_dir`/`steer_capability_path`, and its child receives all three env vars. | `spawn_plan.rs:1300-1340` |
| 2 | The control tree lives under the per-`cwd` run **scratch**, in a `fg-<run_id>/` leaf, never under the async root — `resume_tracking`'s `read_dir` must not see it as a status-less run. | `status.rs:26-32` |
| 3 | The `fg-<run_id>/` leaf — never its shared parent — is removed when the run settles; a removal failure never fails the run; the per-attempt stdout tee survives. | §1.4, `foreground.rs:365-380` |
| 4 | **The parent writes the request into `step_steer_inbox_dir`, not `steer_requests_dir`.** There is no runner on this path to perform the routing hop. | §0.4, `control_watcher.rs:334-400` |
| 5 | `sync_current_child` and `begin_foreground_child` propagate the handle, restoring `foreground-control.ts:60` and `:128`. | §0.2 |
| 6 | A child with no handle still refuses with upstream's `child {i} does not support steering.` | `:144` |
| 7 | A child whose capability is unpublished yields `CHILD_SESSION_NOT_RUNNING_YET` — **retryable**, not a refusal; `supported: false` is a genuine failure. | `:3861`, `:4502` |
| 8 | Delivery maps `Delivered`/`Queued`/`Failed` onto pi's three tool receipts, and a missing ack yields the honest **queued** receipt — never `pending`, never a fourth word. | `:112-114`, `:150-158` |
| 9 | `runs.steer(key, …)` selects by `(parent_workflow_run_id, workflow_key, active_children non-empty)` — **not** by session — and polls to the ack deadline before answering `Missed`. | `:4491-4493`, `:4536-4539` |
| 10 | A **plain** foreground run addressed by `action:"steer"` is still refused with `STEER_FOREGROUND_RUN_REFUSAL`. | `:3217`, `control.rs:574`, §0.6 |
| 11 | A steer addressed at a workflow in another session is still refused by WORKFLOW_7's gates — this task adds delivery, never a bypass. | WORKFLOW_7 §4 |

## §7 Definition of done

* `foreground_run_control_dir` exists in `background/artifact_roots.rs` and returns the run-root
  (not the `control/` leaf); `RunChannels`/`ForegroundRunOptionsInput` thread it through once.
* `request_direct_steer` exists in `background/control.rs`, writes into `step_steer_inbox_dir` with
  `target_index` pinned, and mints its id through the existing private `next_steer_request_id`.
* `build_foreground_run_options` populates all three steer paths using `step_steer_inbox_dir` /
  **`steer_acks_dir`** / `steer_capability_path`; the circular comment at `foreground.rs:865-867`
  is replaced with the real reason.
* The three stale doc claims (`agent_config.rs:583-586`, `prompt_runtime.rs:1409-1412`,
  `spawn_plan.rs:1304-1305` + `:1319-1320`) are corrected.
* `ForegroundChildSteer` exists; `ForegroundChildEntry::steer` / `ForegroundControlEntry::steer`
  exist, are filled by `register_foreground_controls`, and are propagated by `sync_current_child`.
  The two struct-literal test fixtures in `foreground_control.rs` compile.
* `await_steer_ack` is `pub(crate)` (not copied) and WORKFLOW_7's delivery arm performs a real
  steer returning one of pi's three receipts; an unpublished capability is retryable; a child with
  no handle still hits upstream's refusal.
* `WorkflowRunHost::supports_steer()` returns `true`; `steer` selects by
  `(parent_workflow_run_id, workflow_key, non-empty active_children)`, polls to the ack deadline,
  and maps onto all four `WorkflowSteerState` variants through a `workflow_steer_receipt` port.
* A plain foreground run is still refused; WORKFLOW_7's four gates are unchanged.
* The `fg-<run_id>/` tree is removed on settle, outside the `foreground_controls` lock guard.
* `cargo clippy --workspace --all-targets` exits 0 with no new `allow`s and no dead-code warnings.

## §8 Research notes & citations

**Upstream** (`/home/d0m17bw/workspace/pi-subagents` @ `57278d82`)

* [`shared/types.ts:2127`](../../../workspace/pi-subagents/src/shared/types.ts) — `ForegroundChildControl.steer`; `:2130-2140` `ForegroundSteerInput`/`Outcome`; `:2187` `ForegroundRunControl.steer`; `:2353-2356` `ForegroundChildSessionControls`
* [`runs/foreground/foreground-control.ts:60`](../../../workspace/pi-subagents/src/runs/foreground/foreground-control.ts) and `:128` — the two assignments WORKFLOW_6 dropped; `:17` the `BeginForegroundChildInput.steer` field
* [`runs/foreground/workflow-foreground-steering.ts:32-36`](../../../workspace/pi-subagents/src/runs/foreground/workflow-foreground-steering.ts) — `controlIsLiveInWorkflow` (the **session** predicate); `:135-141` the `index` defaulting (WORKFLOW_7's, not this task's); `:142-144` the two refusals; `:146-168` the delivery + the three receipts; `:112-114` the no-ack contract
* [`runs/foreground/subagent-executor.ts:3842-3871`](../../../workspace/pi-subagents/src/runs/foreground/subagent-executor.ts) — `beginForegroundChild`'s real `steer` closure and `:3899` the `onChildSession` that arms it; `:4319-4334` `workflowSteerReceipt`; `:4336` `CHILD_SESSION_NOT_RUNNING_YET`; `:4477-4541` `steerWorkflowChildByKey` (the **key** predicate at `:4491-4493`, the poll at `:4501-4502`/`:4539`, the four `missed` arms at `:4512`/`:4531`/`:4534`/`:4537`); `:5710`/`:5960` the host wiring; `:3217` the plain-foreground refusal
* [`runs/shared/child-session.ts:324-325`](../../../workspace/pi-subagents/src/runs/shared/child-session.ts) — why pi's handle is a closure and cyrup's is a path set
* [`runs/foreground/async-steering-action.ts:45,52,71,149`](../../../workspace/pi-subagents/src/runs/foreground/async-steering-action.ts) — the `asyncDir` gating that proves WORKFLOW_8 is a different layer

**cyrup** (`/home/d0m17bw/workspace/cyrup` @ `f8bec9ee`)

* [`extension/executor/foreground.rs:865-872`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground.rs) — the three `None`s and the circular comment; `:612-710` `resolve_run_channels`/`RunChannels`; `:96-140` `ForegroundRunOptionsInput`; `:903-1000` `register_foreground_controls` (`:832` `child_index: Some(0)`, `:949` `index: 0`); `:1003-1040` `settle_foreground_run`; `:365-380` why the scratch root is NOT deleted
* [`extension/executor/foreground_control.rs:20-44`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_control.rs) — `ForegroundChildEntry` (`interrupt` at `:43`); `:48-64` `sync_current_child` + its omission list; `:67-76` `begin_foreground_child`; `:132-171` the test fixtures that must gain the field
* [`extension/executor/notices.rs:20-97`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs) — `ForegroundControlEntry` (`cwd` `:83-84`, `parent_workflow_run_id` `:73`, `workflow_key` `:79`, `active_children` `:95`)
* [`extension/executor/paths.rs:300-313`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/paths.rs) — `drive_foreground_run_sync` → `exec::run_sync`, the shared entry
* [`extension/executor/status.rs:26-32`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/status.rs) — `resume_tracking`'s `read_dir` over the async root: invariant 2's reason
* [`extension/executor/control.rs:529`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/control.rs) `control_steer`; `:574` the plain-foreground refusal; `:640-660` the async request + ack wait; `:690-710` `await_steer_ack` (**private — must become `pub(crate)`**)
* [`extension/executor/workflow.rs:228`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow.rs) — the host impl with only `launch` (`:229`) and `status` (`:388`); `:118` `workflow_run_id`; `:353` the blocking launch that makes §5.2's poll necessary
* [`extension/executor/mod.rs:134`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/mod.rs) — `foreground_controls: HashMap<String, ForegroundControlEntry>` and `:145-147` the no-`.await`-under-lock rule
* [`exec/mod.rs:1101-1135`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs) — scratch + all three steer `create_dir_all`s before spawn
* [`exec/spawn_plan.rs:1300-1340`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/spawn_plan.rs) — the env overlay; its two *"Absent on the foreground path"* comments are the lines this task falsifies
* [`exec/agent_config.rs:583-611`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/agent_config.rs) — the three `RunOptions` fields and the stale `None on the foreground path` doc
* [`prompt_runtime.rs:2336-2349`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/prompt_runtime.rs) — the child-side read, with no foreground/background distinction; `:1409-1412` the stale doc; `:326` the ack write; `:334-345` `publish_capability`'s republish cadence; `:583` the inbox drain
* [`background/control.rs:618`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs) `control_inbox_dir`, `:1128-1138` the id-ordering contract, `:1165` `steer_requests_dir`, `:1174` `step_steer_inbox_dir`, `:1196` `write_steer_request_to_dir`, `:1217` `next_steer_request_id` (private), `:1258` `request_async_steer_with_mode`, `:1293` `enqueue_step_steer`, `:1354` `consume_steer_requests`, `:1376` `SteerDeliveryMode`, `:1430` `MAX_STEER_QUEUE_SIZE`, `:1448-1458` `SteerAckState`, `:1476` `SteerAck`, `:1506` `SteerCapability`, `:1530` `steer_capability_path`, **`:1536` `steer_acks_dir`**, `:1671` `read_steer_capability`, `:1706` `take_steer_acks`
* [`background/runner_main/control_watcher.rs:334-400`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/control_watcher.rs) — `route_steer_requests`, the runner hop a foreground run does not have; `:762-769` the "silently dead again" invariant
* [`background/artifact_roots.rs:246`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/artifact_roots.rs) `cwd_key`, `:325-337` `attempt_scratch_dir`/`_in` (per-`cwd`, not per-run)
* [`extension/tool/text.rs:116`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/tool/text.rs) `STEER_FOREGROUND_RUN_REFUSAL`, `:177` `STEER_ACK_TIMEOUT` (3 s)
* [`workflows/scripted/engine.rs:122`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs) the `WorkflowScriptHost` trait, `:160-173` the `supports_steer`/`steer` defaults, `:1062-1130` `run_steer` and the `:1089` capability gate
* [`workflows/scripted/types.rs:71-140`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/types.rs) — `WorkflowSteerMode`/`Options`/**`State` (four variants, `:99-108`)**/`Target`/`Result`
* [`tests/steer_delivery_integration.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/tests/steer_delivery_integration.rs) — the existing end-to-end conventions for this protocol (`:150`, `:290`, `:376`, `:769-794`)

No third-party clone was required: upstream is checked out at
`/home/d0m17bw/workspace/pi-subagents`, so nothing was written to `./tmp`.

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_18.md
