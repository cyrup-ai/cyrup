---
stage: qa
status: completed
updated: 2026-09-14 01:00
---

## ⟦AUG 2026-09-14⟧ Re-verification against HEAD — READ THIS FIRST

This spec was augmented at **2026-09-13 02:52**, BEFORE commit `68facb9` ("workflows 16,17")
landed 361 lines across `executor/workflow.rs`, `executor/workflow_steering.rs` and
`tool/routing.rs`. Every citation below was re-opened at HEAD. **Nothing in §§0-4 has been
removed or softened; this section corrects line numbers, resolves the spec's two open
questions, and adds the upstream truth the original augmentation did not have.**

**Verdict: NOT implemented at HEAD.** `register_stop_child` is still `None`
(`crates/cyrup-ext-subagents/src/extension/tool/routing.rs:671`), there is no
`workflow_child_stops.rs`, and `grep -rn 'clear_workflow_child_stop|stop_workflow_child|WorkflowChildStops'`
over `crates/cyrup-ext-subagents/src/` returns **zero** hits outside the prose note at
`workflow_controllers.rs:8`. The whole task is live.

### A. Corrected citations (spec line → HEAD line)

| spec says | HEAD truth | file (absolute) |
|---|---|---|
| `engine.rs:201-203` `WorkflowStopChild` | ✅ **still `:201-203`** (doc `:201-202`, type `:203`) | `/home/user/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/engine.rs` |
| `engine.rs:224-225` `WorkflowStopChildRegistrar` | ✅ **still `:224-225`** | same |
| (field) | `pub register_stop_child: Option<WorkflowStopChildRegistrar>` at **`engine.rs:251`** | same |
| `engine.rs:2625-2671` "the closure is already written" | ❌ **now `engine.rs:2695-2742`** — `if let Some(register) = &options.register_stop_child` at `:2696`, `let stop: WorkflowStopChild = Arc::new(...)` at `:2698`, `register(Some(stop));` at **`:2741`** | same |
| `engine.rs:2633-2635` (the unknown/settled gate) | ❌ **now `engine.rs:2703-2705`**: `if !inner.launches.contains_key(key) \|\| inner.children.contains_key(key) { return false; }` | same |
| `engine.rs:2799-2801` `register(None)` | ❌ **now `engine.rs:2869-2871`** (`register(None);` on **`:2870`**) | same |
| `engine.rs:2796-2798` the leak comment | ❌ **now `engine.rs:2864-2868`**, verbatim at HEAD: *"§6 DL-15: release the stop closure BEFORE anything that can block. It captures the run state, so a slow drain would otherwise keep the whole partial reachable from the embedder — and the child token is already cancelled, so a late `stop()` has nothing left to stop."* | same |
| `stopped_child_result` | **`engine.rs:663-672`** (`ok: false, stopped: true, output = error = message`) | same |
| `routing.rs` `register_stop_child: None` | **`routing.rs:670-671`** — comment `// No external stop channel in a foreground tool call.` on `:670`, the field on **`:671`** | `/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/tool/routing.rs` |
| `routing.rs:652` `settle_workflow_controller` | ❌ **now `routing.rs:698`** (comment `:690-697`) | same |
| `workflow_steering.rs:84-127` `active_workflow_error` | ❌ **now `workflow_steering.rs:103-141`** (`async fn active_workflow_error` at `:103`, body ends `:141`; doc `:96-102`) | `/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow_steering.rs` |
| `foreground_actions/steer.rs:115` workflow branch, block `:109-125` | ✅ **both still exact** — `if let Some(workflow_run_id) = self.live_workflow_run_id_for(id)` on `:115`, `steer_workflow_foreground(...)` call `:116-124` | `/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/executor/foreground_actions/steer.rs` |
| `workflow_controllers.rs:8-11` header quote | ✅ exact | `/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow_controllers.rs` |
| `workflow_controllers.rs:79-180` the API to mirror | ✅ exact (`impl SubagentExecutor` spans `:79-180`) | same |
| `workflow_controllers.rs:158` `abort_and_clear_workflow_controllers` | ✅ exact | same |
| `session_state.rs:282` teardown call site | ✅ exact — `self.abort_and_clear_workflow_controllers();` inside `teardown_session` | `/home/user/cyrup/crates/cyrup-ext-subagents/src/extension/executor/session_state.rs` |
| DoD `cd /home/d0m17bw/workspace/cyrup` | ❌ stale path. This tree is **`/home/user/cyrup`** | — |

**Hard prerequisite WORKFLOW_13: SATISFIED.** No `WORKFLOW_13.md` (or `WORKFLOW_14.md`) exists
anywhere under `.flux` — both landed. Proof in tree: `workflow_steering.rs:14` says *"WORKFLOW_14
completed this file"*, the workflow owns a real run dir (`routing.rs:586-593`
`ensure_accessible_dir(&run_dir)`), and `executor/workflow.rs:515` publishes
`workflow_key: crate::workflows::WorkflowKey::parse(key).ok()` onto each `StepStatus`. Do not
block on W13.

### B. ⚠ Two compile-blockers the spec does not mention

**B1. `active_workflow_error` is PRIVATE and unreachable from `stop.rs`.**
`workflow_steering.rs:103` declares it `async fn active_workflow_error(...)` with **no `pub`**.
It therefore resolves only inside module `extension::executor::workflow_steering`.
`extension::executor::foreground_actions::stop` is a *sibling*, not a descendant — §2.3's
"Reuse `active_workflow_error`" cannot compile as written.

Two legal shapes; **prefer the second**, it is the established precedent:
* raise `active_workflow_error` to `pub(crate)` and call it from `stop.rs`; or
* **put `stop_workflow_child_action` in `workflow_steering.rs` as `pub(crate)`, exactly like
  `steer_workflow_foreground` (`workflow_steering.rs:247`, `pub(crate) async fn`, called from
  `steer.rs:116`).** `stop.rs` then only carries the three-line routing branch, mirroring
  `steer.rs:115` one-for-one. This keeps the private gate private and puts the workflow-scoped
  action next to the other workflow-scoped action.

**B2. `WorkflowStopChildRegistrar` is NOT re-exported from `crate::workflows::scripted`.**
`workflows/scripted/mod.rs:121-128`'s `pub use engine::{...}` list exports `WorkflowStopChild`
(`mod.rs:126`) but **not** `WorkflowStopChildRegistrar`. §2.2's snippet compiles anyway — an
`Arc<{closure}>` unsize-coerces to the field's `Option<WorkflowStopChildRegistrar>` at the struct
literal — but if you want to name the type, add `WorkflowStopChildRegistrar` to that `pub use`
list (alphabetically it sits between `WorkflowResumeInput` and `WorkflowRunCall`… in practice
append next to `WorkflowStopChild` on `mod.rs:126`).

### C. §2.4 — RESOLVED: `childId` already reaches `control_stop`. Thread nothing.

The spec asks to "confirm it reaches `control_stop`". It does, landed by SUBA-087:

* `SubagentToolParams::child_id` — `extension/tool/params.rs:110` (`pub(crate) child_id: Option<String>`, doc `:101-109`)
* dispatch — `extension/tool/routing.rs:1922-1927`:
  `"stop" => { let target = p.run_id.as_deref().or(p.id.as_deref()); self.executor.control_stop(cwd, target, p.dir.as_deref(), p.child_id.as_deref()).await }`
* receiver — `foreground_actions/stop.rs:94-100`:
  `pub async fn control_stop(&self, cwd: &Path, target: Option<&str>, dir: Option<&str>, child_id: Option<&str>) -> Result<String, String>`

`control_stop` performs **no validation of `child_id` before** the target block — `validate_stop_child_id`
runs deep inside `background/control.rs:1096` (`request_async_stop`), which the workflow branch
returns before ever reaching. So a workflow launch key passes through untouched.

**There is no `reason` parameter on `control_stop`, and upstream does not have one either** (see
§D). Do **not** invent a signature change: call the handle with pi's own constant message.

### D. Upstream truth — `subagent-executor.ts` @HEAD of `/home/user/cyrup/tmp/pi-subagents`

The original augmentation cited `subagent-executor.ts:6122-6155` for this branch. At the checked-out
upstream the real sites are:

* `shared/types.ts:2315-2316` — `/** Live in-process workflow child stoppers keyed by parent workflow run id. */ workflowChildStops?: Map<string, (childId: string, message?: string) => boolean>;`
* `subagent-executor.ts:4958` — `workflowChildStops: new Map(),` (state construction)
* `subagent-executor.ts:5226` — `deps.state.workflowChildStops ??= new Map();` beside `workflowControllers` at `:5225`
* `subagent-executor.ts:5691-5694` — **the registrar, both arms**:
  `registerStopChild: (stop) => { if (stop) deps.state.workflowChildStops?.set(workflowRunId, stop); else deps.state.workflowChildStops?.delete(workflowRunId); }`
  — `delete`, confirming §1's "MUST remove, not tombstone".
* `subagent-executor.ts:5926-5927` — the settlement tail deletes from **both** maps:
  `deps.state.workflowControllers?.delete(workflowRunId); deps.state.workflowChildStops?.delete(workflowRunId);`
  This is the exact precedent for §2.2's "belt-and-braces" `clear_workflow_child_stop` next to
  `routing.rs:698` — upstream does it unconditionally, not defensively.
* `extension/index.ts:1038-1042` — teardown: abort every controller, `state.workflowControllers?.clear();`
  **then `state.workflowChildStops?.clear();`** — the exact precedent for extending
  `abort_and_clear_workflow_controllers` (`workflow_controllers.rs:158-179`).
* `subagent-executor.ts:6668-6704` — **the `action === "stop"` child branch** (this is the code
  §2.3 ports; it is NOT at `:6122-6155`).
* `extension/rpc.ts:623-637` — a second, RPC-side consumer of the same map, same refusal strings.

**What upstream's `:6668-6704` actually does, in order** — this is more precise than §2.3 and
should be honoured where it does not conflict with §2.3's stated scope:

1. `const workflowController = deps.state.workflowControllers?.get(targetRunId)` — the registry
   probe that routes into this branch at all (cyrup's equivalent is
   `self.live_workflow_run_id_for(id)`, `workflow_controllers.rs:144`).
2. `const stopChild = deps.state.workflowChildStops?.get(targetRunId)`.
3. `if (params.childId !== undefined)` — **the child branch. With no `childId` it falls through
   to whole-workflow abort, which §4 keeps out of scope.**
4. Read the workflow's `status.json`; missing/unreadable ⇒
   `Status file not found for async workflow '${workflowRunId}'.`
5. `resolveAsyncStatusChild(status, params.childId)` — **cyrup already has this**:
   [`crate::background::child_identity::resolve_async_status_child`]
   (`background/child_identity.rs:156`) returning
   [`AsyncStatusChildResolution`](`:63`) `Resolved | NotFound | Ambiguous`, each with upstream's
   exact sentence. It matches on `workflow_key → run_id → positional` (`child_identity.rs:117`),
   and the workflow host stamps `workflow_key` at `executor/workflow.rs:515` — **so the launch
   key IS the resolvable child id.** A failed resolution answers with the resolver's own sentence.
6. `isStoppableAsyncStatusStep(resolution.child.step)` — cyrup's
   `crate::background::child_identity::is_stoppable_async_status_step` (`child_identity.rs:211`);
   refusal is the sentence `control_stop` already spells at `stop.rs:199-204`:
   `Child '{childId}' in async run '{runId}' is {status}; stop only supports pending or running children.`
7. `if (!stopChild)` ⇒ `Workflow ${targetRunId} child stop is unavailable in this extension runtime.`
   — **a distinct refusal for "registry has no handle for this run"**, which the spec collapses away.
8. **The message is a CONSTANT, not a caller-supplied reason**:
   `stopChild(resolution.child.id, \`Workflow child '${resolution.child.id}' stopped.\`)`
   (rpc.ts:626 uses `… stopped by RPC.`). Note it passes **`resolution.child.id`**, the RESOLVED
   identity, not the raw `childId`.
9. `false` ⇒ `Child '${params.childId}' in workflow ${workflowRunId} is not available to stop.`
   — upstream's own string for the collapsed unknown/settled fact. **§2.3's proposed
   `Workflow '{id}' has no live child '{key}'.` is cyrup-original**; prefer upstream's sentence
   unless the spec author insists, and keep the collapse either way.
10. Best-effort append of a `subagent.child-status` line (`status: "stopping"`,
    `reason: "subagent-action"`, `stepIndex`, `agent`, optional `childRunId`/`workflowKey`/`phase`/`label`)
    into the workflow's `events.jsonl`, wrapped in `try/catch`. cyrup's analogue helper is
    `append_steering_notice` (`steer.rs:332-359`) — same best-effort discipline.
11. Success ⇒ `Stop requested for child ${resolution.child.id} in async workflow ${workflowRunId}.`

**Deviation to hold deliberately:** upstream does **not** run `activeWorkflowError` on this path —
it gates on registry membership alone. §2.3's "run the SAME four gates" is a cyrup-original
hardening and the spec requires it. Keep it, but know that `active_workflow_error`'s refusals are
worded for steering — `"Workflow steering requires an active parent session."`
(`workflow_steering.rs:110`) and `no_live_child` ⇒ `"Workflow '{id}' has no live foreground child."`
(`workflow_steering.rs:67-69`) — so a stop refused by those gates will say "steering"/"foreground
child". Either accept that, or factor the gate so the caller supplies its own noun. **Do not
weaken the `SessionGate::Strict` check at `workflow_steering.rs:139`** — that is the whole reason
the spec demands reuse.

### E. Current shape at each seam you will touch

**E1 — the engine closure you are storing (`engine.rs:2695-2742`), verbatim structure:**

```rust
// engine.rs:2695  — pi `registerStopChild` (`:1793-1816`).
if let Some(register) = &options.register_stop_child {          // :2696
    let stop_shared = shared.clone();                            // :2697
    let stop: WorkflowStopChild = Arc::new(move |key: &str, message: Option<&str>| { // :2698
        let message = message.map(ToString::to_string)
            .unwrap_or_else(|| format!("Workflow child '{key}' stopped by user."));   // :2699-2701
        let mut inner = stop_shared.lock();                       // :2702
        if !inner.launches.contains_key(key) || inner.children.contains_key(key) {
            return false;                                         // :2703-2705
        }
        inner.stopped_launches.insert(key.to_string());           // :2706
        inner.children.insert(key.to_string(), stopped_child_result(key, &message)); // :2707-2709
        if let Some(token) = inner.child_stop_tokens.get(key) { token.cancel(); }     // :2710-2712
        /* copy agent/phase/label/generated_lane_key off the last `Started` Run entry */ // :2713-2733
        entry.error = Some(message);                              // :2734
        inner.trace.push(entry);                                  // :2735
        drop(inner);                                              // :2736
        stop_shared.trace_changed();                              // :2737
        stop_shared.bump();                                       // :2738
        true                                                      // :2739
    });
    register(Some(stop));                                         // :2741
}
```

`RunInner`'s relevant fields (`engine.rs:314-332`): `launches: HashMap<String, LaunchRecord>` (`:320`),
`children` (`:324`-ish), `stopped_launches: HashSet<String>` (`:329`),
`child_stop_tokens: HashMap<String, CancelToken>` (`:330`). The `stopped_launches` entry is what
the launch path re-reads at `engine.rs:1868-1884` and `:1985-1997` to short-circuit a not-yet-started
child, which is why a stop lands even before the child spawns.

`stopped_child_result` (`engine.rs:663-672`) sets `ok: false, stopped: true, output = message,
error = Some(message)` — that is the `stopped: true` evidence §3 demands.

**E2 — the executor field to add (`executor/mod.rs`):**
Module list is `executor/mod.rs:8-26` (add `pub(crate) mod workflow_child_stops;` after
`workflow_controllers` on `:25`). The sibling field is declared at **`mod.rs:151`**:

```rust
workflow_controllers: Arc<std::sync::Mutex<HashMap<crate::background::RunId, WorkflowController>>>,
```

with its field doc at `:138-150` (read it — it explains why the key is typed `RunId` rather than
`String`: nothing prefix-matches a workflow id, unlike `foreground_controls` at `:137`). It is
initialised at **`mod.rs:255`**: `workflow_controllers: Arc::new(std::sync::Mutex::new(HashMap::new())),`.
Add `workflow_child_stops` beside both, same `std::sync::Mutex` choice for the same reason (short
synchronous insert/remove, no `.await` in the critical section), and lock with the crate's standing
idiom `.lock().unwrap_or_else(std::sync::PoisonError::into_inner)`.

Note §2.1's sketch wraps the map in its own `WorkflowChildStops` struct; `workflow_controllers`
does **not** — it is a bare field on `SubagentExecutor` with free functions in an `impl
SubagentExecutor` block. Mirroring `workflow_controllers.rs` exactly (as §2.1 instructs) means the
bare field; the struct wrapper is optional and unused by the proven sibling.

**E3 — teardown (`workflow_controllers.rs:158-179`).** The function takes the
`workflow_controllers` lock at `:159-162`, aborts each entry in the loop `:164-177`, then
`controllers.clear()` at `:178`. Clear the child-stop map **after** that lock is released (it is
scoped to the function body, so take the second lock on a separate statement — `std::sync::Mutex`
is not reentrant and the file's own comment at `:93-96` makes exactly this point about nested
acquisition).

**E4 — the routing branch to add in `stop.rs`.** `control_stop` computes `async_root` at
`stop.rs:105` and `results_dir` at `:106`, then opens the id-addressed block at **`:113-115`**:

```rust
let mut resolved_async_id: Option<String> = None;   // :112
if let Some(id) = target
    && dir.is_none()
{                                                   // :113-115
    // <-- INSERT THE WORKFLOW BRANCH HERE, as the FIRST statement in this block,
    //     ahead of `resolves_to_nested_run` (:118) and `is_live_foreground_run` (:123).
    //     This is positionally identical to steer.rs:115, which sits ahead of its own
    //     `is_live_foreground_run` at steer.rs:132.
```

Both `async_root` and `child_id` are already in scope at that point. `dir.is_none()` is part of
the guard, which matches upstream: the `dir` form never reaches the workflow branch there either
(`subagent-executor.ts:6713`).

**E5 — `routing.rs:671`, the field to replace.** The surrounding literal is
`RunWorkflowScriptOptions { … }` starting `routing.rs:568`; `routing.rs:564-566` warns *"Field order
mirrors `RunWorkflowScriptOptions`' own declaration order… The struct has no `Default`, so all
thirteen are named."* Keep that ordering. In scope at `:671`: `self.executor:
Arc<SubagentExecutor>` (`extension/tool/mod.rs:49`), `workflow_run_id: crate::background::RunId`
(minted `routing.rs:583`), `cancel: CancelToken`. `RunId: Clone`, so §2.2's snippet is sound.

### F. Confirmed-unchanged facts (no action needed)

* `steer.rs:109-125`'s comment names the pattern to copy verbatim, including *"Exact-match only
  (`live_workflow_run_id_for`'s own doc) — a prefix match here would risk routing at the wrong
  workflow."* `live_workflow_run_id_for` (`workflow_controllers.rs:144-151`) still does
  `keys().find(|run_id| run_id.as_str() == id)` and is pinned by
  `live_workflow_run_id_for_is_exact_match_only` (`workflow_controllers.rs:281-297`).
* `workflow_controllers.rs`'s five unit tests (`:195-314`) are the exact template for the
  child-stop registry's tests, including the idempotent-double-remove assertion at `:218`.
* `SubagentExecutor::new()` has no session id by default; `register_workflow_controller`
  (`:82-103`) reads `self.current_session_id()` at call time. If the child-stop registry needs a
  session it should do the same — but the gates come from `active_workflow_error`, so it likely
  needs no session field at all.

### G. Corrected Definition-of-Done commands

```bash
cd /home/user/cyrup                                                                      # NOT /home/d0m17bw/workspace/cyrup
grep -n 'register_stop_child' crates/cyrup-ext-subagents/src/extension/tool/routing.rs   # -> Some(...) , currently `None` at :671
grep -rn 'clear_workflow_child_stop' crates/cyrup-ext-subagents/src/                     # >= 3 (registrar None arm, settle tail near routing.rs:698, session teardown via workflow_controllers.rs:158)
grep -n 'fn clear_workflow_child_stop' -A6 crates/cyrup-ext-subagents/src/extension/executor/workflow_child_stops.rs  # MUST `remove(run_id)`
cargo clippy --workspace --all-targets --features test-fixtures                          # exits 0
```

⚠ `cargo fmt --all -- --check` is RED repo-wide (~80 files, pre-existing toolchain bump). Keep
only the files you touch well-formatted; do not run a repo-wide format.

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
