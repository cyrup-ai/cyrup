---
stage: aug
status: done
updated: 2026-09-13 02:52
---

# WORKFLOW_18 — the child-stop registry: `register_stop_child` and `workflowChildStops`

> **Created 2026-09-13.** Named as out-of-scope by WORKFLOW_6 (`workflow_controllers.rs:8-11`:
> *"cyrup already has its seam — `register_stop_child`, currently passed `None` at the
> `route_workflow_mode` call site. See WORKFLOW_1 §4 for why it is a different task's."*)
> WORKFLOW_1 §4 assigned it to no one.

OBJECTIVE: let an operator stop **one child** of a live workflow without aborting the whole
workflow. Today the only granularity is the `WorkflowController`'s all-or-nothing abort.

**Alignment with pi-subagents core definition of done** (VISION.md "Evidence closes work" +
"Background work stays visible" + "Authority comes from clarity"):
A per-child stop must produce concrete evidence (`stopped: true` result, `Stopped` trace entry,
engine's `stopped_child_result`) rather than a silent or leaked state. The registrar's `None` arm
MUST drop the handle (evidence of clean release, not tombstone) or the run leaks — exactly the
optimistic-success failure the artisan contract forbids. Precise one-child authority (vs whole-
workflow abort) keeps the operator's intent visible and the evidence truthful.

Stack: **Rust**. One crate: `cyrup-ext-subagents`.

**Hard prerequisite: [`WORKFLOW_13`](WORKFLOW_13.md).** The stop surface routes by run id through
the same gates steering does, and those gates read the run record W13 writes.

---

## 0. What is dead

```rust
// extension/tool/routing.rs — inside the RunWorkflowScriptOptions literal
// No external stop channel in a foreground tool call.
register_stop_child: None,
```

The engine builds a complete, correct stop closure and hands it to a registrar that nobody
supplies:

```rust
// workflows/scripted/engine.rs:201-203
/// A stop-child handle — returns `true` when the key named a live launch that was stopped.
pub type WorkflowStopChild = Arc<dyn Fn(&str, Option<&str>) -> bool + Send + Sync>;
// engine.rs:224-225
pub type WorkflowStopChildRegistrar = Arc<dyn Fn(Option<WorkflowStopChild>) + Send + Sync>;
```

**The closure is already written** — `engine.rs:2625-2671`. Under the run lock it:

1. returns `false` if `!inner.launches.contains_key(key) || inner.children.contains_key(key)`
   (unknown key, or already settled);
2. inserts into `stopped_launches` and puts a `stopped_child_result(key, &message)` into
   `children`;
3. cancels `child_stop_tokens[key]`;
4. pushes a `Stopped` trace entry, copying agent/phase/label/`generated_lane_key` from the last
   `Started` entry;
5. `trace_changed()` + `bump()`, returns `true`.

Default message when `None`: `"Workflow child '{key}' stopped by user."`

So this task writes **no stop logic**. It stores a handle and calls it.

---

## 1. ⚠ The lifetime contract — get this wrong and you leak the whole run

`register(Some(stop))` is called once before the run (`engine.rs:2625-2671`); `register(None)` is
called at settlement (`engine.rs:2799-2801`) **deliberately before anything that can block**, with
this comment at `:2796-2798`:

> the closure captures run state; keeping it alive would keep the whole partial reachable.

**The registrar MUST drop the stored handle on `None`.** A registry that only inserts leaks the
run's `Arc<RunShared>` — every `WorkflowScriptChildResult`, every trace entry, every console line,
for the life of the process.

Two more constraints from the same seam:

* The callback is **synchronous** and must not block — it runs under the run mutex path, called
  from arbitrary host threads. No `.await`, no file I/O, no lock ordering against
  `foreground_controls`.
* The return value is **advisory**: `false` means "nothing live under that key", not an error.

---

## 2. Required changes

### 2.1 `extension/executor/workflow_child_stops.rs` — the sibling registry (new file)

Model it on `workflow_controllers.rs`, which is the same shape and already proven:

```rust
//! pi `state.workflowChildStops: Map<string, WorkflowStopChild>` — the SIBLING of
//! `state.workflowControllers` (`workflow_controllers.rs`), and deliberately a separate map for
//! the reason upstream keeps them separate: a controller aborts a WORKFLOW, a stop handle stops
//! ONE CHILD of one. Conflating them makes "stop child b" abort the run.

pub(crate) struct WorkflowChildStops {
    inner: Arc<Mutex<HashMap<RunId, WorkflowStopChild>>>,
}
```

on `SubagentExecutor`, mirroring `workflow_controllers.rs:79-180`'s API exactly:

```rust
pub(crate) fn register_workflow_child_stop(&self, run_id: &RunId, stop: WorkflowStopChild);
/// §1 — the `None` arm of the registrar. MUST remove, not merely mark.
pub(crate) fn clear_workflow_child_stop(&self, run_id: &RunId);
pub(crate) fn stop_workflow_child(&self, run_id: &RunId, key: &str, reason: Option<&str>) -> bool;
```

And extend the existing teardown: `abort_and_clear_workflow_controllers`
(`workflow_controllers.rs:158`, called from `session_state.rs:282`) must clear this map too, or a
session teardown leaves the same leak §1 describes.

### 2.2 `extension/tool/routing.rs` — supply the registrar

Replace `register_stop_child: None` with a closure over the executor handle and this run's id:

```rust
// The registrar is called with `Some(stop)` before the run and `None` at settlement
// (`engine.rs:2625-2671`, `:2799-2801`). BOTH arms are load-bearing: the `None` arm is what
// releases the engine's captured run state (§1).
register_stop_child: Some({
    let executor = std::sync::Arc::clone(&self.executor);
    let run_id = workflow_run_id.clone();
    std::sync::Arc::new(move |stop: Option<crate::workflows::scripted::WorkflowStopChild>| {
        match stop {
            Some(stop) => executor.register_workflow_child_stop(&run_id, stop),
            None => executor.clear_workflow_child_stop(&run_id),
        }
    })
}),
```

⚠ The existing unconditional `settle_workflow_controller(&workflow_run_id)` (`routing.rs:652`) is
**not** a substitute for the `None` arm — it clears a different map. Add a matching
`clear_workflow_child_stop` there as a belt-and-braces idempotent cleanup, because the engine's
`register(None)` is skipped on a panic path.

### 2.3 `extension/executor/foreground_actions/stop.rs` — route a workflow child

`control_stop` currently has no workflow branch. Add one **in the same position**
`control_steer` puts its workflow branch (`foreground_actions/steer.rs:115`), for the same reason
and with the same exact-match rule:

```rust
// Mirrors `control_steer`'s `:109-125` routing exactly. Exact-match only
// (`live_workflow_run_id_for`'s own doc) — a prefix match risks stopping a child of the
// wrong workflow.
if let Some(workflow_run_id) = self.live_workflow_run_id_for(id) {
    return self.stop_workflow_child_action(&workflow_run_id, key, reason, &async_root).await;
}
```

`stop_workflow_child_action` runs the SAME four gates `active_workflow_error` applies
(`workflow_steering.rs:84-127`) before calling `stop_workflow_child`. **Reuse
`active_workflow_error` — do not re-implement the gates.** Stopping another session's child is
exactly the cross-session hazard `SessionGate::Strict` exists to prevent.

Return the engine's own truth:

```rust
match self.stop_workflow_child(workflow_run_id, key, reason) {
    true  => Ok(format!("Stopped workflow {workflow_run_id} child '{key}'.")),
    // `false` = unknown key OR already settled (`engine.rs:2633-2635`). One message, because
    // from the caller's side they are one fact — the same collapse `no_live_child` makes.
    false => Err(format!("Workflow '{workflow_run_id}' has no live child '{key}'.")),
}
```

### 2.4 The tool surface needs a child selector

`subagent action:"stop"` takes `id`/`childId`. `childId` (`SubagentToolParams`) is the natural
carrier for the workflow **key**. Confirm it reaches `control_stop`; if it does not, thread it
rather than adding a new parameter — the schema already advertises it.

---

## 3. Definition of done

**Evidence closes the stop** (pi-subagents VISION): `stop_workflow_child` returning `true` +
`stopped: true` result + `Stopped` trace entry is the concrete proof the child was stopped
without leaking the run. The `clear_workflow_child_stop` (`remove`, not tombstone) on the
`None` registrar arm is the evidence that authority was released cleanly. The refusal message
for settled/unknown keys is the truthful boundary.

```bash
cd /home/d0m17bw/workspace/cyrup
grep -n 'register_stop_child' crates/cyrup-ext-subagents/src/extension/tool/routing.rs   # -> Some(...)
grep -rn 'clear_workflow_child_stop' crates/cyrup-ext-subagents/src/                     # >= 3 (registrar None arm, settle tail, session teardown)
cargo clippy --workspace --all-targets --features test-fixtures                          # exits 0
```

**End to end — a workflow stops its own child and keeps running.** The engine's closure is
reachable from the host side only; drive it through the tool surface from a second turn against a
live workflow, or assert it directly in a unit test over the registrar. The observable contract:

* `stop_workflow_child(run, "a", Some("not needed"))` on a LIVE key ⇒ `true`, and the workflow's
  own result for key `a` becomes the engine's `stopped_child_result` with
  `stopped: true` — **the workflow itself still completes**.
* the same call on a SETTLED key ⇒ `false` (engine gate `:2633-2635`), and the refusal reaches the
  caller as §2.3's message.
* an unknown key ⇒ `false`, same message.
* after settlement, `clear_workflow_child_stop` has run: the map has no entry for that run id.

**The leak check is part of done**, not a nicety:

```bash
grep -n 'fn clear_workflow_child_stop' -A6 crates/cyrup-ext-subagents/src/extension/executor/workflow_child_stops.rs
# MUST `remove(run_id)`, not insert a tombstone or set a flag.
```

---

## 4. Explicitly out of scope

| deferred to | what |
|---|---|
| already landed | aborting a WHOLE workflow — `WorkflowController::abort` (WORKFLOW_6), a different verb on a different map |
| not this task | a guest-facing `runs.stop(key)`. Upstream exposes no such verb; `runs.*` is a launch/observe surface and stop is an operator action. |
| [`WORKFLOW_14`](WORKFLOW_14.md) | steering the same child — same gates, different transport |
