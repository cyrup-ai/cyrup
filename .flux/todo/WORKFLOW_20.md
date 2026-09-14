---
stage: qa
status: completed
updated: 2026-09-14 01:00
---

# WORKFLOW_20 — workflow `state.get` / `state.set`: bind the mission scratchpad

> **Created 2026-09-13.** `missions/workflow_state.rs` is a complete 1:1 port with its own tests
> and **zero callers**, because — in its own words — *"cyrup has no `workflowScript` runtime at
> all"*. That stopped being true at commit `f8bec9ee`.

---

## ⟡ RE-AUGMENTATION @ HEAD (2026-09-14) — READ THIS FIRST

This spec was first augmented at **2026-09-13 02:52**. Since then commit **`68facb9`
("workflows 16,17")** landed 361 lines across `executor/workflow.rs`,
`executor/workflow_steering.rs` and `extension/tool/routing.rs`. Every factual claim below has
been re-verified against the working tree at HEAD. Verdict:

**NOT implemented at HEAD. The work is still real.** `routing.rs:669` is still literally
`state: None`; `routing.rs:553` still passes a hard `false`; `create_mission_workflow_state` still
has **zero production callers** (the only two hits outside its own file are inside
`missions/goal_driver.rs`'s `mod tests` at `:587`/`:997` — test-only, and `mod.rs:73`, a
re-export).

But **four claims are stale or wrong**, and one of them changes the design. In order of severity:

| # | claim in this spec | reality at HEAD |
|---|---|---|
| **S1** | §2.2: *"resolve the mission … `MissionWorkflowStateStore::create(&cfg.roots, cwd, mission_id)`"* | **The mission is ALREADY resolved, one frame up.** `extension/tool/mod.rs:379-389` runs `prepare_mission_binding_for_dispatch` **before** the mode dispatch at `:397`, for all four modes including WORKFLOW. Resolving again inside `route_workflow_mode` would be a **second, side-effecting** resolution. See §2.2-AUG. |
| **S2** | §1 item 1 + §3: the analyzer's refusal text is *"Workflow state is unavailable without a mission."* | **False — that sentence is not in `analyzer.rs` at all.** It exists only at `engine.rs:561` and `:570`. The analyzer emits a different sentence. See §1-AUG. |
| **S3** | engine.rs line numbers `:192-200`, `:2388`/`:2403`, `:2480`/`:2500`, `:2686+`, `:2693` | All shifted. Corrected table in §1-AUG. `:2686+` is off by ~2100 lines — `state_get`/`state_set` are at `:559`/`:567`. |
| **S4** | §3 DoD: *"grep `state_enabled` … no `false` literal at the validate call"* | There are now **TWO** `validate_workflow_script(script, false)` call sites in `routing.rs` — `:553` (in scope) and `:1493` (the `action:"validate"` arm, which did not figure in this spec). The grep as written cannot pass without a decision. See §3-AUG. |

Also resolved: **§2.1's open question** (*"confirm whether `set` already calls
`assert_workflow_json_value`"*) — **it does**, `workflow_state.rs:218`. Add nothing.

And the **hard prerequisite `WORKFLOW_13` is satisfied**: no `WORKFLOW_13.md` remains anywhere in
`.flux/`, and `executor/workflow.rs:140` documents its output as already present —
*"(W13 §3.1 already resolves it once per `route_workflow_mode`)"*. `route_workflow_mode` writes a
real `status.json` run record at `routing.rs:617-620` before the engine runs. Unblocked.

---

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

**[AUG @HEAD] §0 table verified — every line number in it is still exact.**
`missions/workflow_state.rs` is 411 lines and was **not touched** by `68facb9`:
`MISSION_STATE_MAX_BYTES` `:53`, `mission_state_path` `:64`, `assert_workflow_json_value` `:93`,
`MissionWorkflowState::get` `:203`, `::set` `:216`, `create_mission_workflow_state` `:246`. ✅

Two details the table does not spell out, both load-bearing for §2:

* **The real constructor signature** is
  `create_mission_workflow_state(location: &MissionStoreLocation, mission_id: &str) -> MissionResult<MissionWorkflowState>`
  (`:246-253`). It takes a **`&MissionStoreLocation`**, *not* a roots/cwd pair — this is what
  makes §2.2's snippet uncompilable as written (see S1/§2.2-AUG).
* **`MissionStoreLocation`** (`missions/types.rs:607-618`) is
  `{ project_root, mission_dir, global_index_dir, write_global_index, retain_terminal }`, and
  `mission_state_path` (`:64-70`) only ever reads `location.mission_dir`, producing
  `<mission_dir>/<missionId>/state.json` after running the id through
  `store::validate_mission_id_str` (the traversal guard — do not bypass it).
* `create_mission_workflow_state` / `MissionWorkflowState` / `MISSION_STATE_MAX_BYTES` /
  `mission_state_path` are re-exported at `missions/mod.rs:72-75`;
  **`assert_workflow_json_value` is NOT re-exported** — reach it as
  `crate::missions::workflow_state::assert_workflow_json_value` if ever needed (you will not need
  it; see §2.1-AUG).
* The key grammar is **not duplicated**: `validate_state_key` (`workflow_state.rs:75-85`)
  delegates to `crate::workflows::WorkflowKey::parse` and only owns the wording. That is the
  crate's ONE declaration of the grammar (SCOPE_3 §A.3).

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

### §1-AUG — corrected citations, and the refusal text §1 gets WRONG

**[AUG @HEAD] Corrected engine.rs map** (file is now 3597 lines):

| what | spec said | **HEAD** |
|---|---|---|
| `pub trait WorkflowStateStore` | `:192-200` | **`:194-199`** (doc comment `:191-192`) |
| `pub state: Option<Arc<dyn WorkflowStateStore>>` on `RunWorkflowScriptOptions` | — | **`:248`** |
| `RunShared::state_store` field | — | **`:340`** |
| `RunShared::state_get` (verbatim refusal at `:561`) | `:2686+` | **`:559-565`** |
| `RunShared::state_set` (verbatim refusal at `:570`) | `:2686+` | **`:567-583`** |
| `WorkflowOpsBridge` forwarders `state_get`/`state_set` | — | **`:653`/`:657`** |
| `let state_enabled = options.state.is_some();` — **THE single derivation** | `:2693` | **`:2763`** |
| `IsolateThread::spawn(shared, script, state_enabled)` | — | **`:2764`** (→ `:2201`, `:2207`) |
| `install_sandbox(&mut runtime, state_enabled, host_enabled)` | `:2388`/`:2403` | **`:2352`**, fn at **`:2441-2465`**, the install script at **`:2457`** |
| `analyze_workflow_script(script, AnalyzerOptions { state_enabled })` inside `validate_workflow_script` | `:2480`/`:2500` | **`:2554`**; `pub fn validate_workflow_script(script, state_enabled)` at **`:2532-2559`** |

`analyzer.rs` is unchanged: `unavailable_global_message` **`:148-173`**, `AnalyzerOptions`
**`:176-181`**, the report site **`:330`**, `is_allowed`'s `state` branch **`:339-342`**. ✅

`prelude.js:578` is correct — but it is in a **different crate**:
`crates/cyrup-workflow-runtime/src/js/prelude.js`, `:572`
`globalThis.__cyrupWorkflowInstall = ({ stateEnabled, hostEnabled }) => {`, `:578`
`if (stateEnabled) globalThis.state = state;`. ✅

#### ⚠⚠ S2 — §1's item 1 attributes the WRONG SENTENCE to the analyzer

`"Workflow state is unavailable without a mission."` appears **exactly twice in the whole crate**,
both in `engine.rs` (`:561`, `:570`) — i.e. **only** at the op-level defence in depth §1 tells you
to leave alone. `analyzer.rs` does **not** contain that string. What the analyzer actually emits
for a `state` reference when `state_enabled == false` is built by
`unavailable_global_message("state", false)` (`analyzer.rs:148-173`) and reads, verbatim:

```text
workflowScript referenced an unavailable global 'state'. Available globals are runs, emit, console, and standard ECMAScript built-ins only. state.get/state.set require a mission; this run was started with mission:false.
```

(and when `state_enabled == true` the globals list becomes `runs, emit, console, state` and the
`state` arm is not taken at all — `:161-166`, `:167-169`.)

§1's *architecture* is right and unchanged — the analyzer is the refusal a script author actually
sees, the op refusal is unreachable-in-practice defence in depth. Only the quoted sentence was
wrong. **This propagates into §3's acceptance script — see §3-AUG.**

`engine.rs`'s own test proves the point today
(`engine.rs:3552-3564`, `state_without_a_mission_uses_the_verbatim_refusal`): with
`state_enabled = false` the guest sees a **`ReferenceError`**, never the engine sentence, because
`globalThis.state` was never installed. The engine sentence is only reachable by a store-less
`RunShared` whose realm nonetheless had `state` installed — which the single derivation at
`:2763` makes impossible. Leave it, as §1 says; just do not write an acceptance test against it.

#### [AUG @HEAD] What the engine already checks before your store is called

Do not re-do these in the adapter or the call site:

* **Key grammar, twice.** `state_get`/`state_set` both call
  `validate_key(Some(&Value::String(key)), "state")` (`engine.rs:563`, `:571`) → `validate_key`
  at **`engine.rs:712-720`**, the same `WorkflowKey::parse`, refusing with
  `"state key must be 1-128 characters using letters, numbers, '.', '_' or '-', and start with a
  letter or number."` So an invalid key from the guest is refused by the **engine**, with the
  engine's wording, and your store never sees it. `MissionWorkflowState::get`/`set` re-check
  anyway (they are a public API) — that is fine and intended, not a duplicate to remove.
* **Value finiteness.** `state_set` calls `assert_workflow_json_value(&value, "state.set('{key}') value")`
  at **`engine.rs:582`** (that is `scripted::json_value::assert_workflow_json_value`, imported at
  `:58` — a *different* function from the missions one, `Result<_, String>` not `MissionResult`).
* **Recovery barrier.** `state_set` refuses inside a recovery barrier (`engine.rs:573-579`) before
  touching the store.

What the engine does **NOT** check, and therefore what your store is solely responsible for:
**the 256 KiB ceiling** and **persistence**. There is no size check anywhere in `engine.rs`.

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

#### §2.1-AUG — verified, plus the answer to the open question

**[AUG @HEAD] The two ⚠ in §2.1 are both settled:**

1. *"Do not re-check the key grammar or the 256 KiB ceiling."* — **correct, keep it.**
   `get` (`workflow_state.rs:203-206`) and `set` (`:216-241`) enforce both. Confirmed unchanged.
2. *"confirm whether `set` already calls `assert_workflow_json_value`"* — **IT DOES**, at
   **`workflow_state.rs:218`**:
   `assert_workflow_json_value(&value, &format!("state.set('{valid_key}') value"))?;`
   → so the §2.1 branch *"If it does not, add the call inside `set`"* is **dead. Add nothing.**
   (Note the value is therefore validated twice on the guest path: once by
   `scripted::json_value::assert_workflow_json_value` at `engine.rs:582` and once by the missions
   copy at `workflow_state.rs:218`. Both are pure, produce the same verdict for the same
   `serde_json::Value`, and neither is yours to remove — the missions one guards non-guest callers,
   the scripted one guards non-mission stores.)

**[AUG @HEAD] Exact types the adapter must reconcile** (all confirmed at HEAD):

```rust
// missions/workflow_state.rs:203, :216 — &mut self, MissionResult
pub fn get(&mut self, key: &str) -> MissionResult<Option<Value>>
pub fn set(&mut self, key: &str, value: Value) -> MissionResult<()>
// where MissionResult<T> = Result<T, MissionError>, and
// MissionError (missions/mod.rs:86-109) is #[derive(thiserror::Error)] with
//   NotFound { mission_id, mission_dir }  -> "Mission '{id}' was not found in {dir}"
//   Invalid(String)                       -> "{0}"   <- BARE, no prefix, deliberately verbatim
//   Io { path, source }                   -> "{path}: {source}"
// so `.map_err(|e| e.to_string())` preserves upstream's character-for-character refusals. ✅

// workflows/scripted/engine.rs:194-199 — &self, Result<_, String>
#[async_trait::async_trait]
pub trait WorkflowStateStore: Send + Sync {
    async fn get(&self, key: &str) -> Result<Option<Value>, String>;
    async fn set(&self, key: &str, value: Value) -> Result<(), String>;
}
```

Re-export path for the trait: `crate::workflows::scripted::WorkflowStateStore`
(`workflows/scripted/mod.rs:126`) — the §2.1 snippet's path is correct. ✅
`async-trait` (`Cargo.toml:81`) and `tokio` with the `sync` feature (`Cargo.toml:71`) are both
already workspace deps of `cyrup-ext-subagents` — no manifest change. ✅

`MissionWorkflowState` is `#[derive(Debug)]` and holds only `PathBuf` + `Option<BTreeMap<String,
Value>>` (`:124-135`), so it is `Send + Sync`; wrapped in `tokio::sync::Mutex` the store satisfies
the trait's `Send + Sync` bound with no extra work.

**Reference implementation of the trait shape** already in-tree:
`engine.rs:3568-3577` (`struct MemoryState` in the `mission_state_round_trips_through_the_store`
test) — the exact `#[async_trait::async_trait] impl WorkflowStateStore` skeleton to copy, and the
test at `engine.rs:3566-3596` is the closest existing proof that `opts.state = Some(store)` makes
`await state.set/get` work end to end.

**Constructor to provide.** §2.1's snippet declares the struct but not how it is built. The
adapter needs a constructor that takes what `create_mission_workflow_state` takes — i.e. a
`&MissionStoreLocation` and a `&str` mission id — NOT roots/cwd:

```rust
impl MissionWorkflowStateStore {
    /// # Errors
    /// [`MissionError::Invalid`] when `mission_id` is not a valid mission id.
    pub fn create(
        location: &MissionStoreLocation,
        mission_id: &str,
    ) -> MissionResult<Self> {
        Ok(Self {
            inner: tokio::sync::Mutex::new(create_mission_workflow_state(location, mission_id)?),
        })
    }
}
```

Export it alongside the rest at `missions/mod.rs:72-75`.

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

### §2.2-AUG — ⚠⚠ S1: the mission is ALREADY resolved, one frame up

**[AUG @HEAD] §2.2's code snippet does not compile and its plan double-resolves the mission.**
The *intent* of §2.2 is right and unchanged — **one `state_enabled`, both consumers, mission
resolved first**. The mechanism is wrong, because §2.2 was written as if `route_workflow_mode`
had to resolve the mission itself. It does not: **`extension/tool/mod.rs` already did, for every
mode, before the dispatch**.

```rust
// extension/tool/mod.rs:379-389  — runs BEFORE the mode arm at :392-414
let explicit_mission = parsed.mission_id.is_some() || parsed.mission.is_some();
let mission_config = cfg.missions.clone();
let (mission_binding, mission_warning) = prepare_mission_binding_for_dispatch(
    &parsed.mission_launch_params(),
    &effective_cwd,
    mission_config.as_ref(),
    self.executor.host_services().and_then(|s| s.session_id()).as_deref(),
    explicit_mission,
)?;
// ...
let outcome = if has_workflow {
    self.route_workflow_mode(&call_id, parsed, &effective_cwd, on_update, cancel).await   // :397
} else if ...
// :416-421
attach_mission_to_tool_outcome(outcome, mission_binding.as_ref(), mission_warning, explicit_mission)
```

`mission_binding: Option<MissionLaunchBinding>` is **exactly the two values the store needs**
(`missions/lifecycle.rs:78-88`):

```rust
pub struct MissionLaunchBinding {
    pub mission_id: String,                 // validated, existent, marked Active
    pub location: MissionStoreLocation,     // <- feeds create_mission_workflow_state directly
    pub auto_created: bool,
    pub announce_in_content: bool,
}
```

**Why resolving a second time inside `route_workflow_mode` would be a bug, not just waste:**
`prepare_mission_launch` (`missions/lifecycle.rs:165-...`) is **side-effecting** — for a
`missionId` it does `read_mission` **and** `update_mission(status: Active)` (`:190-200`), and for a
`mission` object it **creates a record** (`:223+`). Calling it twice per tool call writes twice.

**Therefore the real change is: thread the binding down, do not re-resolve.**

```rust
// extension/tool/mod.rs — at the call site, :397
self.route_workflow_mode(
    &call_id, parsed, &effective_cwd, mission_binding.as_ref(), on_update, cancel,
).await
// `mission_binding.as_ref()` is a shared borrow that ends when the call returns; :416's
// second `.as_ref()` is unaffected.

// extension/tool/routing.rs — new parameter on route_workflow_mode (:521-528)
mission: Option<&crate::missions::MissionLaunchBinding>,
```

and then, inside `route_workflow_mode`, **above** the validation at `:549`:

```rust
// pi `subagent-executor.ts:4139` — the workflowScript branch's ONE caller of
// `createMissionWorkflowState`. A workflow bound to a mission gets a durable scratchpad; an
// unbound one gets no `state` global at all, and the analyzer says why.
//
// The binding is NOT resolved here: `tool/mod.rs:381` already did it once for the whole
// dispatch, and `prepare_mission_launch` is side-effecting (it marks an attached mission
// Active, and creates one for a `mission:{...}` object) — resolving twice would write twice.
let state: Option<std::sync::Arc<dyn crate::workflows::scripted::WorkflowStateStore>> =
    match mission {
        Some(binding) => Some(std::sync::Arc::new(
            crate::missions::MissionWorkflowStateStore::create(
                &binding.location,
                &binding.mission_id,
            )
            .map_err(|e| ToolError::new(e.to_string()))?,
        )),
        None => None,
    };
// §1 — ONE value, both consumers. Never two literals.
let state_enabled = state.is_some();
```

then `validate_workflow_script(script, state_enabled)` at **`:553`** and `state,` at **`:669`**,
replacing the `state: None` comment at `:665-668` with the new contract.

**[AUG @HEAD] §2.2's ⚠ about `mission: false` is satisfied for free — verified.** Routing through
`prepare_mission_launch` gets every case right without a single extra branch
(`missions/lifecycle.rs:171-184`):

| call shape | `prepare_mission_launch` | `state_enabled` | correct? |
|---|---|---|---|
| `missionId: "x"` | `Some(binding{ x })` after `read_mission` + `Active` | **true** | ✅ bound |
| `mission: false` | `Ok(None)` at **`:175-177`**, explicitly | **false** | ✅ honoured as explicitly unbound, exactly as §2.2 demands |
| `mission: { title: … }` | creates a record → `Some(binding)` | **true** | ✅ |
| both `missionId` **and** `mission` | `Err("Use missionId or mission, not both")` at **`:172-174`**, fatal because `explicit_mission` | — | ✅ |
| **neither**, pure `workflowScript` | `Ok(None)` at **`:182-184`** | **false** | ✅ — and this is the important one: `workflow_objective` (**`:96-130`**) reads only `task` / `tasks[].task` / `chain[].task` / `chain[].parallel[].task`. **It never reads `workflowScript`.** So a bare workflowScript yields `objective = None`, `should_create = false`, and **no mission is auto-created**. Unbound stays unbound. |
| a bogus `missionId` | `MissionNotFoundError` → `ToolError` (fatal, `tool/mod.rs:389`'s `?`) | — | ✅ refused before the isolate spawns |

Note the error discipline is already correct and is **not yours to change**: an explicit
`mission`/`missionId` failure is fatal; an automatic binding degrades to
`details.missionWarning` (`tool/mission.rs:74-96`). Since a bare `workflowScript` can never
auto-create, the degraded path is unreachable for WORKFLOW mode — a mission-bound workflow either
gets its store or fails the call, never silently loses its scratchpad.

**One judgement call the implementor must make explicitly:** a store is constructed even when the
script never touches `state`. That is correct and cheap — `MissionWorkflowState` is lazy
(`values: None` until first access, `workflow_state.rs:150-153` / `:246-252`), so constructing it
does **zero** I/O; only `mission_state_path`'s id validation runs. Do not try to defer it, because
`state_enabled` must be known before `validate_workflow_script` at `:553`.

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

### §3-AUG — the DoD, corrected against HEAD

**[AUG @HEAD] ⚠⚠ S4 — the `state_enabled` grep cannot pass as written.** `routing.rs` now has
**two** `validate_workflow_script(script, false)` call sites, not one:

* **`:553`** — inside `route_workflow_mode`, with the *"`state_enabled: false` matches this build's
  `state: None` below"* comment at `:551-552`. **This is the one §2.2 changes.**
* **`:1493`** — inside `route_action`'s `"validate"` arm (`action: "validate"` +
  `workflowScript`, the synchronous no-child preflight), carrying its own comment at
  **`:1482-1483`**: *"`state_enabled` is `false`: this build grants no mission state, and the flag
  exists precisely because `state` is only a legal global when the run has a mission."*
  **This spec never mentions it** — it was not in view when §3 was written.

So after §2.2 lands, `grep -n 'state_enabled' routing.rs` yields **one binding used twice** *plus*
a surviving `false` literal at `:1493`. Read the DoD grep as **scoped to `route_workflow_mode`**:

```bash
grep -n 'state_enabled\|validate_workflow_script' \
  crates/cyrup-ext-subagents/src/extension/tool/routing.rs
# EXPECT, inside route_workflow_mode (~:521-700):
#   ONE `let state_enabled = state.is_some();`, used at the validate call AND in the options literal.
#   NO `false` literal at the route_workflow_mode validate call.
# The `action:"validate"` arm's `false` at ~:1493 is a SEPARATE surface — see the decision below.
cargo clippy --workspace --all-targets --features test-fixtures    # exits 0
```

> **DECISION POINT — flagged, NOT taken here, and NOT silently in scope.** After §2.2, a preflight
> `action: "validate"` of a script containing `state.get(...)` will **refuse**, while the identical
> script run with `missionId` will **succeed**. The `:1482-1483` comment's premise (*"this build
> grants no mission state"*) becomes false the moment §2.2 lands, so at minimum that comment must
> be corrected. The minimal consistent fix is to derive the validate arm's flag from the params
> **without** resolving anything — `action: "validate"` must stay side-effect-free and must never
> call `prepare_mission_launch` (it would create/activate a mission from a validation call):
> `let state_enabled = p.mission_id.is_some() || (p.mission.is_some() && p.mission != Some(Value::Bool(false)));`
> Get the coordinator's ruling before widening; if it stays out of scope, the stale comment at
> `:1482-1483` must still be updated to say *why* the two surfaces now differ.

**[AUG @HEAD] The "unbound must still refuse" check asserts the WRONG SENTENCE (S2).** Per
§1-AUG, the analyzer does not say *"Workflow state is unavailable without a mission."* Replace
that block's expectation with the sentence the author actually sees:

```bash
CYRUP_SUBAGENTS=1 ./target/debug/cyrup -p 'Use the subagent tool with workflowScript: "return await state.get(\"seen\");"'
# EXPECT, from the analyzer via render_validation_findings (routing.rs:555-559), verbatim:
#   workflowScript referenced an unavailable global 'state'. Available globals are runs, emit,
#   console, and standard ECMAScript built-ins only. state.get/state.set require a mission; this
#   run was started with mission:false.
# NOT a ReferenceError (that is what an UNVALIDATED run would give), and NOT a successful run
# returning undefined. The op-level "Workflow state is unavailable without a mission."
# (engine.rs:561/:570) stays unreachable — that is the invariant, not the assertion.
```

The same correction applies to the `mission: false` shape, which must produce the identical
sentence (`prepare_mission_launch` → `Ok(None)` → `state_enabled = false`).

**[AUG @HEAD] The key-grammar check refuses one layer earlier than §3 implies.** `state.set("bad
key!", 1)` never reaches `MissionWorkflowState`: the **analyzer passes it** (it is a runtime
string, not a global), and the **engine** refuses at `validate_key` (`engine.rs:571` →
`:712-720`) with
`"state key must be 1-128 characters using letters, numbers, '.', '_' or '-', and start with a
letter or number."` `MissionWorkflowState`'s own copy of that sentence
(`workflow_state.rs:80-83`) is byte-identical, so the observable text is the same either way —
but do not write the test expecting the missions module to be the one that spoke.

**[AUG @HEAD] The 256 KiB check is the one refusal that is genuinely yours.** Nothing in
`engine.rs` bounds the value size; `state.set("big", "x".repeat(300000))` reaches
`MissionWorkflowState::set`, which measures `serde_json::to_vec_pretty(&next).len()` against
`MISSION_STATE_MAX_BYTES` **before** writing (`workflow_state.rs:226-232`) and refuses with
`"Mission state exceeds the 256 KiB limit ({bytes} bytes; maximum 262144 bytes)."` This is the
single strongest piece of evidence that the adapter is delegating rather than re-implementing:
**if that sentence does not appear, the adapter is not wired to the ported file.**

**[AUG @HEAD] Existing tests to extend rather than duplicate:**
* `workflows/scripted/engine.rs:3566-3596` — `mission_state_round_trips_through_the_store`, the
  trait-level round trip (swap `MemoryState` for `MissionWorkflowStateStore` over a tempdir to get
  the cross-process durability proof without a subprocess).
* `workflows/scripted/engine.rs:3552-3564` — `state_without_a_mission_uses_the_verbatim_refusal`
  (its name is now misleading; it asserts `ReferenceError`, which is the correct behaviour for an
  unvalidated store-less run — **leave it alone**, it is not this task's).
* `missions/workflow_state.rs:289+` — the ported unit tests, including
  `set_then_get_round_trips_through_the_file` (`:289`) and the 256 KiB tests (`:354`, `:375`).
* `extension/tool/routing_tests.rs:1154` —
  `an_execution_call_with_an_explicit_mission_binds_before_the_run_and_settles_after`, the
  existing proof of the `tool/mod.rs:381` binding seam §2.2-AUG threads through.
* `extension/tool/routing_tests.rs:1818+` — `route_workflow_mode`'s existing controller-lifecycle
  coverage; the new parameter added in §2.2-AUG must not disturb it.

**[AUG @HEAD] The `cd` path in the DoD block is from another machine.** Use the repo root
(`/home/user/cyrup` in this environment), not `/home/d0m17bw/workspace/cyrup`.

## 4. Explicitly out of scope

| deferred to | what |
|---|---|
| not this task | a workflow-scoped (non-mission) state store. Upstream offers none, and inventing one gives the crate two scratchpads with different lifetimes. |
| not this task | `mission.state.get/set` as a tool action — a different surface on the same file |
| already ported | every validation rule in `workflow_state.rs`. §2.1 adapts, it does not re-check. |
| **[AUG @HEAD] decision needed** | `action: "validate"`'s own `state_enabled: false` at `routing.rs:1493` — see the DECISION POINT in §3-AUG. Not silently in scope; its comment at `:1482-1483` becomes factually stale either way. |
| **[AUG @HEAD] not this task** | `missions/goal_driver.rs`'s reads of `state.json` (`mission_state_path` caller #2) — a bound workflow writing the same file is exactly what that reader was built for, but nothing about the goal driver changes here. |
| **[AUG @HEAD] already satisfied** | `WORKFLOW_13` (the "hard prerequisite"). No `WORKFLOW_13.md` remains in `.flux/`, and `route_workflow_mode` writes a real `status.json` run record at `routing.rs:617-620` before the engine runs; `executor/workflow.rs:140` refers to W13 §3.1 as done. Not a blocker. |
