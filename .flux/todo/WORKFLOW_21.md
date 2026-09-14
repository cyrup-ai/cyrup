---
stage: aug
status: done
updated: 2026-09-13 02:52
---

# WORKFLOW_21 — live `emit()` forwarding: the engine delta

> **Created 2026-09-13.** The last undelivered callback. `routing.rs` passes `on_emit: None` and
> explains why in detail — the reason is a real engine defect, not a wiring omission, which is why
> this is its own task and why it is last.

OBJECTIVE: let a long-running workflow stream its `emit()` values to the caller as they happen,
instead of only in the final result. Requires an **engine-side change**; no host wiring alone can
fix it.

**Alignment with pi-subagents core definition of done** (VISION.md "Evidence closes work" +
"Background work stays visible"):
Live `emit` values are concrete progress evidence that must be visible *during* the run, not
only in the final `WorkflowScriptResult::emits`. The per-value `(Value, usize)` delta makes
each emit idempotent evidence (index prevents quadratic re-forward or gaps after rollback);
the forwarder that never returns `Err` protects the run while still surfacing the evidence.
Without the delta the snapshot shape hides live work and re-forwards everything — exactly the
optimistic-failure / hidden-work mode the artisan contract forbids.

Stack: **Rust**. One crate: `cyrup-ext-subagents`.

**Hard prerequisite: [`WORKFLOW_13`](WORKFLOW_13.md).** Everything else in this family is
independent of it.

---

## 0. Why `on_emit` is not simply wired up

`routing.rs` states both reasons, and both are correct:

> *`None`, for TWO reasons. (a) `emit` hands the callback the WHOLE accumulated snapshot every
> time, so a per-value forwarder re-forwards everything on every emit, quadratically — and
> `WorkflowScriptResult::emits` already carries the final list. (b) STRONGER: this is the ONE
> callback whose failure ABORTS the run, so a forwarder that errors kills the workflow. Live emit
> forwarding is a real feature, but it needs a delta the engine does not offer.*

The signature is the problem:

```rust
// workflows/scripted/engine.rs:~228
pub type WorkflowEmitCallback = Arc<
    dyn Fn(Vec<Value>) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync,
>;
```

`Vec<Value>` is the **entire accumulated** emit list, re-handed on every call. Forwarding the last
element only is not a fix — the callback cannot know whether an earlier element was already
forwarded after a retry, and the engine's own `emits.pop()` rollback (`engine.rs:~550`) means the
two sides can disagree about what was persisted.

⚠ And `emit` is uniquely dangerous: **its failure aborts the run.** Every other callback
(`on_trace`, `on_lane_plan`, `on_host_step`) is pure telemetry.

---

## 1. Required change — the engine delta

Change `WorkflowEmitCallback` to deliver **one value and its index**:

```rust
/// Delivered ONE value at a time, with its zero-based index in the accumulated list.
///
/// The previous shape handed the whole accumulated `Vec<Value>` on every call, which made any
/// forwarder quadratic and made "has this value already been forwarded?" unanswerable across the
/// engine's own `emits.pop()` rollback. The index is what makes a forwarder idempotent: a
/// receiver that has already seen index N ignores a repeat.
pub type WorkflowEmitCallback = Arc<
    dyn Fn(Value, usize) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync,
>;
```

Three consequences to handle explicitly, all identified at the call site:

**(a) Append ordering and rollback become a contract, not an implementation detail.** The engine
pushes the value, calls the callback, and `pop()`s on failure (`engine.rs:~550`). With a per-value
callback the contract must be stated in the doc comment: **a failed append was not persisted** —
index N is free again. Without that sentence a receiver cannot reason about a gap.

**(b) The final snapshot is unaffected.** `WorkflowScriptPartial`/result builders at
`engine.rs:2677` and `:2966` already clone `inner.emits` wholesale, so `WorkflowScriptResult::emits`
still carries the complete list on both the success and failure arms. **Do not remove it** — the
stream is additive, not a replacement.

**(c) This deviates from upstream's shape.** Every alias in this file pins itself to a pi line
number; add an explicit `[CYRUP-DELTA]` note explaining that the per-value form is what makes live
forwarding possible at all, and that the accumulated form is recoverable by the receiver.

---

## 2. Required change — the host side

`route_workflow_mode` supplies a forwarder that pushes each value into the tool's
`ToolUpdateSink`:

```rust
// The `on_update` sink the workflow already owns, shared the same way children share it
// (`WorkflowRunHost`'s `Arc<Mutex<ToolUpdateSink>>`, workflow.rs:53).
on_emit: Some({
    let sink = std::sync::Arc::clone(&host_sink);
    std::sync::Arc::new(move |value: Value, index: usize| {
        let sink = std::sync::Arc::clone(&sink);
        Box::pin(async move {
            // ⚠ THIS CALLBACK'S FAILURE ABORTS THE RUN (§0). A UI sink that cannot take an
            // update is NOT a reason to kill a workflow — swallow it and return Ok. The only
            // acceptable `Err` from this forwarder is one that means the emitted VALUE itself
            // was rejected, and a progress sink has no such concept.
            if let Ok(mut sink) = sink.lock() {
                sink(/* a progress ToolUpdate carrying `value` and `index` */);
            }
            Ok(())
        })
    })
}),
```

⚠⚠ **`Ok(())` on every sink failure is not laziness, it is the requirement.** Returning `Err`
here turns a dropped UI update into an aborted workflow. Write that reason into the code, because
the next reader's instinct will be to propagate the error.

---

## 3. Definition of done

**Evidence closes the live emit stream** (pi-subagents VISION): each `(Value, usize)` delivered
exactly once (index prevents re-forward or gaps) + the final `emits` snapshot still complete is
the concrete proof live progress was visible during the run. The forwarder that swallows sink
errors (never returns `Err`) protects the evidence surface from killing the workflow.

```bash
cd /home/d0m17bw/workspace/cyrup
grep -n 'type WorkflowEmitCallback' -A3 crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs
# -> Fn(Value, usize), and a [CYRUP-DELTA] note above it
grep -n 'on_emit' crates/cyrup-ext-subagents/src/extension/tool/routing.rs    # -> Some(...)
cargo clippy --workspace --all-targets --features test-fixtures               # exits 0
```

**End to end — values arrive DURING the run, and the final list is still complete:**

```bash
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with workflowScript: "
  emit({ step: 1 });
  await runs.run(\"a\", { agent: \"scout\", task: \"Name one file in crates/cyrup-ext-subagents/src/spawn/\" });
  emit({ step: 2 });
  await runs.run(\"b\", { agent: \"scout\", task: \"Name one file in crates/cyrup-ext-subagents/src/exec/\" });
  emit({ step: 3 });
  return \"done\";
"'
```

* the three emits must be observable as progress **before** the tool call returns — with
  `{step:1}` visible while child `a` is still running, not all three at the end;
* the final result's `Emitted:` line must still list all three (§1b);
* each value must be forwarded **exactly once** — `{step:1}` must not reappear when `{step:2}` is
  emitted. That is the quadratic bug this task exists to fix, and it is the one thing a naive
  port of the old signature would silently keep.

**The abort-safety check is part of done:**

```bash
grep -n 'on_emit' -A20 crates/cyrup-ext-subagents/src/extension/tool/routing.rs | grep -c 'Err('
# -> 0. The forwarder must have NO error path.
```

---

## 4. Explicitly out of scope

| deferred to | what |
|---|---|
| unchanged | `WorkflowScriptResult::emits` — the final list stays, this is additive (§1b) |
| unchanged | `on_trace` / `on_lane_plan`. Both are pure telemetry with no failure semantics; `on_trace` is additionally redundant (the engine returns the trace complete on both arms) and `on_lane_plan` is advisory — `runs.lanes` works without it. Neither is worth a task. |
| not this task | back-pressure on a slow sink. The forwarder swallows and returns `Ok`; bounding the sink is the sink's problem. |
