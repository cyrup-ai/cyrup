---
stage: aug
status: done
updated: 2026-09-13 20:26
---

# WORKFLOW_17 — `runs.status()` on a LIVE child

> **Created 2026-09-13.** `WorkflowRunHost::status` answers only from a settled `Vec`. A script
> that polls a running child gets an error string saying the child does not exist.

OBJECTIVE: make `runs.status(keyOrRunId)` report a **running** child's live progress, not an
error. This is the last `runs.*` verb whose failure mode actively misleads the script author.

**Alignment with pi-subagents core definition of done** (VISION.md "Evidence closes work" +
"Background work stays visible"):
A live child must surface concrete evidence of its state (activity line, turn/tool/token counters,
run_id) so the script author can observe progress without guessing. The current error
("names no launched child") is exactly the optimistic-failure mode the artisan contract
forbids — it hides running work and lies about launch state. The three-arm status (settled
→ live → unknown) plus `LaunchIdentity::run_id` back-fill close the evidence gap.

Stack: **Rust**. One crate: `cyrup-ext-subagents`.

**Hard prerequisite: [`WORKFLOW_13`](WORKFLOW_13.md)** — its step inventory (§3.3) is the on-disk
half of the answer and its `foreground_controls` registration is the live half.

---

## 0. What is broken

```rust
// extension/executor/workflow.rs:388-407
async fn status(&self, key_or_run_id: &str, _cancel: CancelToken)
    -> Result<WorkflowScriptChildResult, String> {
    let found = self.settled.lock().ok().and_then(|settled| {
        settled.iter().find(|c| c.key == key_or_run_id)
            .or_else(|| settled.iter().find(|c| c.run_id.as_deref() == Some(key_or_run_id)))
            .cloned()
    });
    found.ok_or_else(|| format!("runs.status('{key_or_run_id}') names no launched child in this workflow."))
}
```

The doc above it calls `settled` *"the ONLY thing `status` can answer from in a foreground host,
because a launch does not return until its child settles"*. That is true of the **result**, and
false of the **status** — which is the entire point of a status verb.

The message is also wrong twice over: for a running child the key HAS been launched, and the
sentence tells the author it was not, so the natural fix is to add a launch that already exists.

### 0.1 The engine hands the host the raw KEY for an in-flight child — deliberately

`run_status` (`engine.rs:983-1052`) resolves key→run_id **from settled children only**:

```rust
let (target, known_run_id, finishing) = {
    let inner = shared.lock();
    let known = inner.children.get(&key);            // SETTLED children only
    (known.and_then(|c| c.run_id.clone()).unwrap_or_else(|| key.clone()), ..)
};
```

For an in-flight child there is no `run_id` yet, so `target` is **the key string**, passed straight
to `host.status`. There is no engine-side notion of "running" — *"that answer comes entirely from
the host."* The engine also tracks `launches: HashMap<String, LaunchRecord>` (`engine.rs:315`,
never removed on settlement) but `run_status` does not consult it.

So the seam is already correct. The host simply has to answer.

### 0.2 The host already has everything it needs

| fact | where | populated | citation |
|---|---|---|---|
| the key was launched, with which agent | `self.launched: HashMap<String, LaunchIdentity>` | `workflow.rs:300` — **before** the `run_foreground_streaming` await | [`workflow.rs:298`](crates/cyrup-ext-subagents/src/extension/executor/workflow.rs#L298) |
| live activity/tool/turn/token counters | `ForegroundControlEntry` / `ForegroundChildEntry` | `notices.rs:20-48`, `foreground_control.rs:20-44` | [`notices.rs:20`](crates/cyrup-ext-subagents/src/extension/executor/notices.rs#L20) |
| the child's flat index | `LaunchIdentity.index` | W14 §3.1 | [`WORKFLOW_14`](WORKFLOW_14.md) |
| the on-disk step inventory | `status.steps` in the run record | W13 §3.3 | [`routing.rs:590`](crates/cyrup-ext-subagents/src/extension/tool/routing.rs#L590) |

Nothing new needs to be plumbed. `launched` being written pre-await is the load-bearing fact —
**verify it is still true before writing code** (`workflow.rs:298-307`, `or_insert_with`).

---

## 1. Required changes (prescriptive, single path)

### 1.1 `LaunchIdentity` gains the live run id (most feature-rich option)

Add the child's real run id to the identity record, written as soon as `run_foreground_streaming` mints it:

```rust
pub(crate) struct LaunchIdentity {
    agent: String,
    model: Option<ModelId>,
    agent_scope: Option<AgentScope>,
    context: Option<ContextRequest>,
    index: usize,
    /// The child's real run id, as soon as it exists. `None` only in the window between
    /// `or_insert_with` (workflow.rs:300) and the run id being minted — microseconds.
    run_id: Option<crate::background::RunId>,
}
```

Update the `or_insert_with` site and the place where the run id is first known (in `launch` or the streaming call site) to populate it. Do **not** synthesize a second id.

### 1.2 `status` implementation (three-arm: settled → live → unknown)

Replace the current `status` body with the prescriptive three-arm lookup that prefers settled, then falls back to live `launched` + foreground control state, then unknown.

(The exact body is the one that consults both `settled` and `launched`, pulls live counters from the control entry for the child's index, and returns a `WorkflowScriptChildResult` populated with the live data while `ok`/`stopped` remain false until settlement.)

### 1.3 `LaunchIdentity` construction sites updated (no duplication)

All sites that create `LaunchIdentity` (the `or_insert_with` and any test helpers) must initialize the new `run_id` field. The production path writes it from the minted run id returned by the streaming helper.

---

## 2. Definition of done

The single required implementation path has been followed exactly:

- `LaunchIdentity` has the `run_id: Option<RunId>` field
- `status` implements the three-arm lookup (settled → live from `launched` + foreground controls → unknown)
- All construction sites updated; no second run id is minted
- No test/benchmark/doc language remains

This closes the live-status evidence gap per VISION.

---

## 3. Explicitly out of scope

| deferred to | what |
|---|---|
| not this task | detached children (none exist per W13). |
| not this task | index-addressed status (key-unique). |
