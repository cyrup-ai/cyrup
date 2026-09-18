---
stage: qa
status: completed
updated: 2026-09-14 01:00
---

## ⟦AUG 2026-09-14⟧ Re-verification against HEAD — READ THIS FIRST

This spec was first augmented at **2026-09-13 02:52**, BEFORE commit `68facb9` ("workflows 16,17")
landed 361 lines across `extension/executor/workflow.rs`, `extension/executor/workflow_steering.rs`
and `extension/tool/routing.rs`. Every factual claim in §§0-4 has been re-opened against the
working tree at HEAD. **Nothing below removes or softens scope.** This section corrects the stale
line numbers, kills one snippet that references a variable which does not exist, names the missing
accessor the implementor must add, and supplies the upstream + runtime facts the original
augmentation did not have.

### Verdict: **NOT implemented at HEAD. The whole task is live.**

```
grep -n 'type WorkflowEmitCallback' -A3 crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs
#  -> 214:  pub type WorkflowEmitCallback = Arc<
#     215:      dyn Fn(
#     216:              Vec<Value>,          <-- STILL the accumulated snapshot. §1 not done.
grep -n 'on_emit' crates/cyrup-ext-subagents/src/extension/tool/routing.rs
#  -> 686:                on_emit: None,    <-- STILL None. §2 not done.
grep -rn 'CYRUP-DELTA' crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs
#  -> 0 hits.                                <-- §1c not done.
```

`grep -rn 'WorkflowEmitCallback' crates/` returns exactly **4** hits, all inside
`cyrup-ext-subagents`, so the blast radius of the signature change is fully enumerated below.
No consumer outside the crate exists.

---

### A. Corrected citation table (spec claim → HEAD truth)

| §    | spec said | HEAD truth |
|---|---|---|
| §1   | `engine.rs:~228` for `WorkflowEmitCallback` | **`engine.rs:214-221`** (doc line **`:213`**) |
| §1a  | `engine.rs:~550` for `emits.pop()` | **`engine.rs:550`** — *still exactly right* |
| §1b  | builders at `engine.rs:2677` and `:2966` | **STALE.** The `partial` closure is **`engine.rs:2743-2755`** (`emits: inner.emits.clone()` at **`:2747`**); the success arm is **`engine.rs:3033-3040`** (`emits: partial.emits` at **`:3036`**). `engine.rs:2678` is now `on_emit: options.on_emit.clone()` inside the `RunShared` constructor (`:2672-…`), not a result builder. |
| §2   | `WorkflowRunHost`'s `Arc<Mutex<ToolUpdateSink>>`, **`workflow.rs:53`** | **STALE.** `workflow.rs:53-54` is now `WORKFLOW_CHILD_ENV`. The alias is **`workflow.rs:64`** (`type SharedUpdateSink = Arc<Mutex<ToolUpdateSink>>`, doc `:56-63`); the field is **`workflow.rs:110`**; the wrap happens in `WorkflowRunHost::new` at **`workflow.rs:170`**. |
| §0   | the verbatim TWO-reason comment in `routing.rs` | **ACCURATE, unchanged.** It is at **`routing.rs:681-685`**, immediately above `on_emit: None` at **`:686`**. |
| §3   | `cd /home/d0m17bw/workspace/cyrup` | **Foreign path.** Use this repo's root. |

Everything else in §§0-4 — the two reasons, the abort semantics, the additive-not-replacement
rule, the "swallow and return `Ok`" requirement, and the out-of-scope table — was re-checked and
is **correct as written**.

---

### B. The seam, verbatim at HEAD

**`crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:213-221`** — the alias to change:

```rust
/// pi `onEmit` (`:1126`) — fallible: a persistence failure aborts the run.
pub type WorkflowEmitCallback = Arc<
    dyn Fn(
            Vec<Value>,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>
        + Send
        + Sync,
>;
```

**`crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs:531-556`** — the one call site
(method on `RunShared`):

```rust
    /// `emit(value)` — upstream `:1929-1946`. The one callback whose failure aborts the run, so the
    /// emit is rolled back before the fatal is recorded and the wording is upstream's.
    pub(crate) async fn emit(&self, value: Value) -> Result<(), String> {
        if let Err(error) = assert_workflow_json_value(&value, "emit") {
            let message = format!("Workflow emit could not be persisted: {error}");
            self.record_fatal(message.clone(), None);
            return Err(message);
        }
        let snapshot = {
            let mut inner = self.lock();
            inner.emits.push(value);              // :541  — `value` is MOVED here
            inner.emits.clone()                   // :542  — the quadratic clone §0 names
        };
        let Some(on_emit) = &self.on_emit else {
            return Ok(());
        };
        // Awaited, not blocked on: §5.5. A sync callback here would force the embedder to block
        // the isolate thread inside a running op.
        if let Err(error) = on_emit(snapshot).await {          // :549
            self.lock().emits.pop();                           // :550  — the rollback §1a is about
            let message = format!("Workflow emit could not be persisted: {error}");
            self.record_fatal(message.clone(), None);
            return Err(message);
        }
        Ok(())
    }
```

⚠ **`value` is moved into `push` at `:541`.** A per-value callback therefore needs the value
cloned *before* the push, and the index taken as the **pre-push length** inside the same lock:

```rust
        let (forwarded, index) = {
            let mut inner = self.lock();
            let index = inner.emits.len();   // 0-based index this value will occupy
            inner.emits.push(value.clone());
            (value, index)
        };
```

Take the index as the **pre-push `len()`**, not `len() - 1`: `unwrap_used`/`expect_used` are DENY
workspace-wide (`Cargo.toml:101-104`) and `checked_sub` here would be noise for an arithmetic fact
the pre-push read already states exactly.

---

### C. The four sites the signature change touches — complete list

| file:line | what | action |
|---|---|---|
| `workflows/scripted/engine.rs:213-221` | the alias + its doc | rewrite to `Fn(Value, usize)`, add the `[CYRUP-DELTA]` block (§1c, form in D below) |
| `workflows/scripted/engine.rs:539-554` | `RunShared::emit` | clone-before-push, pass `(value, index)`, keep the `pop()` + `record_fatal` arm byte-for-byte |
| `workflows/scripted/engine.rs:3171` | test `options()` helper, `on_emit: None` | **no change** — `None` is signature-agnostic |
| `workflows/scripted/engine.rs:3532` | second test literal, `on_emit: None` | **no change** |

Plus the public re-export at **`workflows/scripted/mod.rs:124`** (`WorkflowEmitCallback` is listed
beside `WorkflowHostStepCallback, WorkflowLanePlanCallback`) — the name does not change, so the
re-export line does not either.

**No test in the workspace constructs an emit callback.**
`grep -rn 'on_emit\|WorkflowEmitCallback' crates/cyrup-it/` → 0 hits. The only behavioural test
that touches emits at all is `engine.rs:3314/3335`
(`emit({ first: results[0].output });` … `assert_eq!(result.emits, vec![json!({"first": "output of c0"})])`),
which exercises the **final snapshot** and must keep passing untouched — that is §1b's
"do not remove it" made executable.

**The guest/op layer is UNAFFECTED.** `cyrup-workflow-runtime/src/lib.rs:109` is already
per-value:

```rust
    /// `emit(value)` — the one telemetry-shaped call whose failure aborts the run.
    async fn emit(&self, value: serde_json::Value) -> Result<(), String>;
```

and the engine's bridge impl at **`engine.rs:645-647`** is a one-line forward
(`self.0.emit(value).await`). No JS, no `prelude.js`, no deno op signature changes. This task is
**host-facing type + one call site + one wiring**, nothing more.

---

### D. Upstream truth (checked in `tmp/pi-subagents`, this checkout)

```ts
// scripted-workflow.ts:1198
onEmit?: (emits: unknown[]) => void;

// scripted-workflow.ts:2166-2181
if (message.type === "emit") {
    try { assertWorkflowJsonValue(message.value, "emit"); }
    catch (error) { finish({ error: new Error(`Workflow emit could not be persisted: …`) }); return; }
    emits.push(message.value);
    try { options.onEmit?.([...emits]); }
    catch (error) { emits.pop(); finish({ error: new Error(`Workflow emit could not be persisted: …`) }); }
    return;
}
```

So cyrup is **already** a delta on two axes before this task touches it — async where upstream is
synchronous, `Result` where upstream throws. The per-value `(Value, usize)` is the **third**. The
`[CYRUP-DELTA]` block §1c asks for should name all three, in the house form used at
`cyrup-core/src/tool.rs:60-…` and `tui/events.rs:76-86`:

```rust
/// # [CYRUP-DELTA] — one value and its index, where pi re-hands the whole accumulated list
///
/// **What differs.** …per-value + index, async, `Result` instead of throw…
/// **What it costs.** …the receiver must re-accumulate if it wants the list; it already gets the
/// complete list back in `WorkflowScriptResult::emits`…
```

⚠ **DO NOT renumber the pi citations in `engine.rs`.** That file pins an **older** pi revision
than the copy in `tmp/`: it says `onHostStep (:1119) / onTrace (:1124) / onLanePlan (:1125) /
onEmit (:1126)`, while the checked-out `scripted-workflow.ts` has them at
`:1190 / :1196 / :1197 / :1198` — a consistent offset across the whole options interface.
"Fixing" only this one alias's number would break the file's internal consistency and is not in
scope. Add the delta note; leave `(:1126)` alone.

---

### E. §2's snippet references a variable that does not exist — the real host wiring

`route_workflow_mode` is declared at **`routing.rs:521-528`**; its sink parameter is
`on_update: ToolUpdateSink` at **`routing.rs:526`**. That sink is **MOVED**, not cloned, into
`WorkflowRunHost::new` at **`routing.rs:637-647`**:

```rust
        let host = std::sync::Arc::new(crate::extension::executor::workflow::WorkflowRunHost::new(
            std::sync::Arc::clone(&self.executor),
            cwd.to_path_buf(),
            // Moved, not cloned — `WorkflowRunHost::new` does the `Arc<Mutex<…>>` wrap that lets
            // each child borrow a forwarding share of this one sink.
            on_update,                                        // routing.rs:642
            …
        ));
```

By the time the `RunWorkflowScriptOptions` literal is built (**`routing.rs:653-690`**) there is
**no `host_sink` in scope, and `on_update` has been moved away.** The `let sink =
std::sync::Arc::clone(&host_sink)` line in §2 will not compile as written.

**What is in scope** is `host: Arc<WorkflowRunHost>`, constructed at `:637` — *before* the options
literal — and already `Arc::clone`d at `:665-666` for the `host:` field. So the wiring is:

1. **Add an accessor on `WorkflowRunHost`.** `SharedUpdateSink` (`workflow.rs:64`) is a
   module-private alias and `on_update` (`workflow.rs:110`) is a private field; `routing.rs` is a
   different module and cannot reach either. Add, next to `child_sink`:

   ```rust
   /// The host's shared sink, for the live `emit` forwarder (`routing.rs`'s `on_emit`).
   pub(crate) fn update_sink(&self) -> Arc<Mutex<ToolUpdateSink>> {
       Arc::clone(&self.on_update)
   }
   ```

   (Returning the spelled-out `Arc<Mutex<ToolUpdateSink>>` rather than raising `SharedUpdateSink`
   to `pub(crate)` keeps the alias private, which is what its doc at `:56-63` is written for.)

2. **Reuse the existing poison-swallow idiom**, do not reinvent it. The exact pattern §2 writes by
   hand already exists at **`workflow.rs:66-79`**:

   ```rust
   fn child_sink(shared: &SharedUpdateSink) -> ToolUpdateSink {
       let shared = Arc::clone(shared);
       Box::new(move |update| {
           // `unwrap_used`/`expect_used` are DENY workspace-wide (`Cargo.toml:101-104`): a poisoned
           // sink drops the update rather than panicking inside a child's progress fold.
           if let Ok(mut sink) = shared.lock() {
               sink(update);
           }
       })
   }
   ```

   `if let Ok(mut sink) = sink.lock()` — **exactly** as §2 already has it — is the sanctioned form,
   and its "drop the update rather than panic" rationale is the same one §2's ⚠⚠ paragraph states
   for the `Ok(())`. Cite `child_sink` so the two do not drift.

3. **Replace `routing.rs:681-686`.** The TWO-reason comment is the *record of why this was
   deferred*; once the engine offers the delta, reason (a) is gone and reason (b) is answered by
   the forwarder's no-error-path. Rewrite the comment to say so — do not leave a comment claiming
   the feature is impossible above a wired callback.

---

### F. Runtime facts the implementor must not discover the hard way

* **The forwarder runs ON THE ISOLATE THREAD, inside a live deno op.** `RunShared::emit` is
  reached from the bridge impl at `engine.rs:645-647`, which does **not** hop to
  `RunShared::main_handle` (`engine.rs:360`, the "BLOCKER-1 / §5.1" field every *other* host call
  forwards through). The future returned by `on_emit` is therefore polled on the isolate's
  `current_thread` runtime. Budget: **one `std::sync::Mutex` lock and one synchronous
  `FnMut(ToolUpdate)` call.** Nothing slow, nothing that `.await`s I/O — a stall here stalls the
  whole workflow, including the children already running.

* **Do NOT route emits through the telemetry channel.** `TelemetryEvent` (`engine.rs:277-298`)
  and its single drain task (`engine.rs:2645-2672`) exist precisely so `on_trace`/`on_lane_plan`/
  `on_host_step` never run on the isolate thread — but they are all infallible. `on_emit` is
  queued nowhere *because it is fallible and its failure aborts the run*: queuing it would silently
  discard the abort semantics that both cyrup (`:549-554`) and upstream (`:2174-2180`) implement.
  Keep it awaited inline.

* **Lock contention is already a designed-for case.** The same host `Mutex<ToolUpdateSink>` is held
  by the per-child forwarding sinks minted at `workflow.rs:525` (`child_sink(&self.on_update)`),
  which run on the **main** multi-thread pool while children stream. Both sides hold it only across
  one synchronous `sink(update)` and never across an `.await`, so the forwarder joins an existing,
  bounded discipline. Do not introduce a `tokio::sync::Mutex` here — that would put an `.await`
  inside the op.

* **Both result arms already carry the complete list (§1b holds at HEAD).** Success:
  `engine.rs:3033-3040` reads `partial.emits`. Failure: `finish_error`
  (`engine.rs:2890-2895`) rebuilds `partial(&shared)` (`engine.rs:2743-2755`) for every terminal
  arm — `Fatal`, `TimedOut`, `Cancelled`. The one builder that does **not** carry emits is `fail`
  (`engine.rs:2620-2624`, `WorkflowScriptPartial::default()`), and it is reachable only from the
  three pre-run validation guards at `:2625-2639`, before any `emit` can have happened. Nothing to
  change on either arm.

---

### G. What `ToolUpdate` to push — the shapes that exist, and the one that is in scope

`cyrup_core::ToolUpdate` is `{ content: Vec<Content>, details: Option<serde_json::Value>,
terminate: TerminateHint }` (`crates/cyrup-core/src/tool.rs:51-58`), and
`ToolUpdateSink = Box<dyn FnMut(ToolUpdate) + Send + 'static>` (`cyrup-core/src/tool.rs:123`).

* The host sink **already** carries `SubagentUpdatePayload` updates during a workflow run —
  forwarded per child by `child_sink` from `drive_foreground_run_sync`
  (`extension/executor/paths.rs:300-393`), built with
  `SubagentUpdatePayload::single_live(...).into_tool_update(text)` (`tui/events.rs:617`, `into_tool_update` at `:669`).
* `SubagentUpdatePayload` has **no** workflow-emit variant, and `RunMode::Workflow`
  (`background/state.rs:37`) has no constructor on it. **Inventing one is scope creep** — §4 keeps
  this task to the delta plus the forwarder.
* The renderer degrades safely on a `details` it cannot parse: `render_subagent_result`
  (`extension/host/native_impl.rs:736`) tries `from_value::<SubagentUpdatePayload>` and,
  failing that or finding no settled `results`, falls back to the first text content block
  ("(no output)" when absent). So **`details: None` + one `Content::text(..)` block is legal and
  renders**.
* The in-scope, consistent choice is therefore a plain text update whose wording matches the
  settled `Emitted:` line the same function already produces at **`routing.rs:709-718`**, using the
  same formatter:

  ```rust
  crate::workflows::scripted::format_workflow_json_preview(&value, 200)
      .unwrap_or_else(|| "undefined".to_string())
  ```

  (`workflows/scripted/json_value.rs:75-79`; `routing.rs:714` is the existing caller with the same
  200-char bound.) Carry `index` in the text so a receiver can dedupe, as §1 requires.
* **Not the target:** `workflows/chat_progress.rs` (the pi live-card projection) has **zero**
  production callers — its only caller is a test at `preflight.rs:1295`. Do not wire emits into it.

---

### H. Corrected acceptance commands

Run from the repo root (**not** `/home/d0m17bw/workspace/cyrup`):

```bash
grep -n 'type WorkflowEmitCallback' -A6 crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs
# -> `dyn Fn(Value, usize)`, with a [CYRUP-DELTA] block above it
grep -n 'CYRUP-DELTA' crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs   # -> >= 1 hit
grep -n 'on_emit' crates/cyrup-ext-subagents/src/extension/tool/routing.rs          # -> Some(...)
grep -n 'fn update_sink' crates/cyrup-ext-subagents/src/extension/executor/workflow.rs  # -> 1 hit
cargo clippy --workspace --all-targets --features test-fixtures                     # exits 0
```

The §3 abort-safety grep still stands and is the sharpest check in the spec — keep the forwarder
literally free of the token `Err(`:

```bash
grep -n 'on_emit' -A20 crates/cyrup-ext-subagents/src/extension/tool/routing.rs | grep -c 'Err('
# -> 0
```

⚠ `cargo fmt --all -- --check` is **repo-wide RED** on ~80 pre-existing files from a toolchain
bump. That is not this task's. Keep only the files you touch well-formatted.

The end-to-end script in §3 is unchanged and still the right proof; the three assertions
(live before return / final list still complete / each value exactly once) are the definition of
done.

---

### I. Prerequisite status

**`WORKFLOW_13` has landed.** It is no longer in `.flux/todo/`, and the tree shows its output:
`WorkflowRunHost::resolve_resume` at `workflow.rs:581`, the `async_root` field whose doc reads
*"W13 §3.1 already resolves it once per `route_workflow_mode`"* (`workflow.rs:138-141`), and
`async_root.clone()` threaded at `routing.rs:646`. The hard prerequisite in the header is
satisfied; nothing blocks this task.

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
