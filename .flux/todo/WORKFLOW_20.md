---
stage: aug
status: done
updated: 2026-09-13 02:52
---

# WORKFLOW_20 — workflow `state.get` / `state.set`: bind the mission scratchpad

> **Created 2026-09-13.** `missions/workflow_state.rs` is a complete 1:1 port with its own tests
> and **zero callers**, because — in its own words — *"cyrup has no `workflowScript` runtime at
> all"*. That stopped being true at commit `f8bec9ee`.

OBJECTIVE: make `await state.get(k)` / `await state.set(k, v)` work inside a `workflowScript` that
is bound to a mission, by adapting the already-ported `MissionWorkflowState` to the engine's
`WorkflowStateStore` trait.

**Alignment with pi-subagents core definition of done** (VISION.md "Evidence closes work"):
State operations must emit concrete evidence (persistent JSON writes that survive processes,
`get` returning prior values, refusal messages for unbound/invalid keys, 256 KiB/key-grammar
enforcement) so a workflow can close on proven scratchpad state rather than a ReferenceError or
undefined. The single `state_enabled` derivation (mission resolved first) and the adapter that
delegates without re-checking invariants are the evidence surface; without them the guest sees
inconsistent refusal or the state silently disappears — exactly the optimistic-failure mode the
artisan contract forbids.

Stack: **Rust**. One crate: `cyrup-ext-subagents`.

**Hard prerequisite: [`WORKFLOW_13`](WORKFLOW_13.md)** — a workflow with durable state needs a
durable run record; without it the state outlives a run that was never recorded.

---

## 0. What exists

`crates/cyrup-ext-subagents/src/missions/workflow_state.rs` — a port of
`pi-subagents/src/missions/workflow-state.ts`, with all three invariants already enforced:

| piece | site |
|---|---|
| `MISSION_STATE_MAX_BYTES` (256 KiB, checked on read AND before write) | `:53` |
| `mission_state_path(...)` | `:64` — already has two production callers (`actions.rs` `mission.show`, `goal_driver.rs`) |
| `assert_workflow_json_value` (the finiteness check; the other four upstream checks are unrepresentable in `serde_json::Value`) | `:93` |
| `MissionWorkflowState::get/set` — lazy once-only load, key grammar `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`, map updated only after the write succeeds | `:203` / `:216` |
| `create_mission_workflow_state(...)` | `:246` — **zero callers** |

Its own module doc states the position precisely:

> *"[`create_mission_workflow_state`] has exactly ONE caller upstream —
> `runs/foreground/subagent-executor.ts:4139`, inside the `workflowScript` branch — and cyrup has
> no `workflowScript` runtime at all … It is ported here, in full and with its own tests, so that
> the `workflowScript` port is a call-site change rather than a second port of this file."*

**Take it at its word. This task is a call-site change.** Do not re-port, do not re-validate, do
not add a second key grammar.

## 0.1 What is missing

```rust
// extension/tool/routing.rs — inside RunWorkflowScriptOptions
// No mission state -> the verbatim "Workflow state is unavailable without a mission." refusal at
// the ops, AND `globalThis.state` is never installed in the guest realm.
state: None,
```

And the trait that `None` fails to satisfy:

```rust
// workflows/scripted/engine.rs:192-200
#[async_trait::async_trait]
pub trait WorkflowStateStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Value>, String>;
    async fn set(&self, key: &str, value: Value) -> Result<(), String>;
}
```

---

## 1. The two-layer gate — understand it before touching it

`state_enabled` is derived ONCE from `options.state.is_some()` (`engine.rs:2693`) and then flows to
**two independent places**, both of which must agree:

1. **the analyzer**, `engine.rs:2480`/`:2500` → `analyze_workflow_script(script, AnalyzerOptions {
   state_enabled })`, whose refusal text lives at `scripted/analyzer.rs:148-180`:
   `"Workflow state is unavailable without a mission."`
2. **the guest realm**, `engine.rs:2388`/`:2403` → `__cyrupWorkflowInstall({ stateEnabled, … })` →
   `prelude.js:578` `if (stateEnabled) globalThis.state = state;`

With `state_enabled` false, `state` is an **undeclared global** in the guest — a bare
`ReferenceError`, which is exactly why the analyzer must refuse first and why its message is the
one a script author actually sees.

⚠⚠ **`route_workflow_mode` passes `state_enabled: false` to `validate_workflow_script` by hand**
(`routing.rs:553`), with this comment:

> *"`state_enabled: false` matches this build's `state: None` below; passing `true` would ACCEPT a
> `state.get(...)` the run then refuses at the op, which is the worst of both."*

That comment is correct and becomes a **trap the moment `state` is conditionally `Some`**: the
validation call happens *before* the mission is resolved in the current ordering. Both must be
derived from the SAME value. Resolve the mission binding first, compute `state_enabled` once, and
pass that one value to both `validate_workflow_script` and `RunWorkflowScriptOptions::state`.

A third refusal exists as defence in depth — `engine.rs:2686+` `state_get`/`state_set` refuse
verbatim when no store is present. Leave it.

---

## 2. Required changes

### 2.1 `missions/workflow_state.rs` — the adapter

`MissionWorkflowState::get`/`set` take `&mut self` (lazy load mutates the cached map) and return
`MissionResult<_>`; the trait needs `&self` and `Result<_, String>`. One adapter reconciles both:

```rust
/// `MissionWorkflowState` behind the engine's trait.
///
/// `Mutex`, not `RwLock`: `get` takes `&mut self` because the lazy once-only load mutates the
/// cached map, so a read is a write on the first call and a reader/writer split would buy nothing.
/// `tokio::sync::Mutex` because both methods are `async` and the guard is held across the
/// blocking file I/O below.
pub struct MissionWorkflowStateStore {
    inner: tokio::sync::Mutex<MissionWorkflowState>,
}

#[async_trait::async_trait]
impl crate::workflows::scripted::WorkflowStateStore for MissionWorkflowStateStore {
    async fn get(&self, key: &str) -> Result<Option<Value>, String> {
        let mut guard = self.inner.lock().await;
        // `MissionWorkflowState` is BLOCKING std::fs. The whole-file read is bounded by
        // MISSION_STATE_MAX_BYTES (256 KiB) and happens at most once per run, so it is taken
        // inline rather than through `spawn_blocking` — which would need the guard to cross a
        // thread boundary.
        guard.get(key).map_err(|e| e.to_string())
    }
    async fn set(&self, key: &str, value: Value) -> Result<(), String> {
        let mut guard = self.inner.lock().await;
        guard.set(key, value).map_err(|e| e.to_string())
    }
}
```

⚠ Do **not** re-check the key grammar or the 256 KiB ceiling in the adapter. `get`/`set` enforce
both (`workflow_state.rs:203`, `:216`), and a second check is a second place to get it wrong.

⚠ Do **not** call `assert_workflow_json_value` in the adapter either — confirm whether `set`
already calls it; if it does, calling it again double-reports. If it does **not**, add the call
inside `set`, not in the adapter, so every caller of `MissionWorkflowState` gets it.

### 2.2 `extension/tool/routing.rs` — resolve the mission, then derive once

```rust
// pi `subagent-executor.ts:4139` — the workflowScript branch's ONE caller of
// `createMissionWorkflowState`. A workflow bound to a mission gets a durable scratchpad; an
// unbound one gets no `state` global at all, and the analyzer says why.
let state: Option<std::sync::Arc<dyn crate::workflows::scripted::WorkflowStateStore>> =
    match p.mission_id.as_deref() {
        Some(mission_id) => Some(std::sync::Arc::new(
            crate::missions::MissionWorkflowStateStore::create(&cfg.roots, cwd, mission_id)
                .map_err(|e| ToolError::new(e.to_string()))?,
        )),
        None => None,
    };
// §1 — ONE value, both consumers. Never two literals.
let state_enabled = state.is_some();
```

then `validate_workflow_script(script, state_enabled)` at `:553` and `state,` in the options
literal. Replace the `state: None` comment with the new contract.

⚠ Check how `mission` binds on `SubagentToolParams` — the tool takes both `missionId` and a
`mission` object (`mission: false` means "no mission"). Honour a `mission: false` as explicitly
unbound, do not treat it as "resolve from ambient context".

---

## 3. Definition of done

**Evidence closes the state surface** (pi-subagents VISION): a successful `state.set` + later
`state.get` returning the written value (even across processes) + refusal messages for unbound
or invalid keys is the concrete proof the scratchpad is durable and bounded. The single
`state_enabled` derivation ensures the analyzer and guest agree; the adapter delegates without
re-checking so every enforcement point stays in the ported `workflow_state.rs`.

```bash
cd /home/d0m17bw/workspace/cyrup
grep -n 'state_enabled' crates/cyrup-ext-subagents/src/extension/tool/routing.rs
# -> ONE binding, used twice. No `false` literal at the validate call.
cargo clippy --workspace --all-targets --features test-fixtures    # exits 0
```

**End to end — state survives across two runs of the same mission.** Create a mission, then:

```bash
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with missionId:"<id>" and workflowScript: "await state.set(\"seen\", { n: 1 }); return await state.get(\"seen\");"'
# -> { n: 1 }

CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with missionId:"<id>" and workflowScript: "const prior = await state.get(\"seen\"); await state.set(\"seen\", { n: (prior ? prior.n : 0) + 1 }); return await state.get(\"seen\");"'
# -> { n: 2 }   — a SECOND process read the first process's write

cat <missionDir>/<id>/state.json      # { "seen": { "n": 2 } }
```

**Unbound must still refuse, at the analyzer and with the real sentence:**

```bash
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with workflowScript: "return await state.get(\"seen\");"'
# -> "Workflow state is unavailable without a mission."
# NOT a ReferenceError, and NOT a successful run returning undefined.
```

**And the invariants still bite:**

```bash
# key grammar
... workflowScript: "await state.set(\"bad key!\", 1);"     # -> refused, key-pattern message
# 256 KiB ceiling
... workflowScript: "await state.set(\"big\", \"x\".repeat(300000));"   # -> refused, not truncated
```

---

## 4. Explicitly out of scope

| deferred to | what |
|---|---|
| not this task | a workflow-scoped (non-mission) state store. Upstream offers none, and inventing one gives the crate two scratchpads with different lifetimes. |
| not this task | `mission.state.get/set` as a tool action — a different surface on the same file |
| already ported | every validation rule in `workflow_state.rs`. §2.1 adapts, it does not re-check. |
