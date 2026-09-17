---
stage: qa
status: completed
updated: 2026-09-15 12:00
---

# SCOPE_10 — async status snapshot + S6 live workflow controls

> Renamed 2026-09-09 from `SCOPE_10.md` — position 11 of 12 in the WORKFLOW_1|WORKFLOW_12 dependency-ordered sequence.

> ## ⚠ NOT A WORKFLOW TASK — read [`WORKFLOW_1`](WORKFLOW_1.md) §2 before scheduling
>
> Renamed into the `WORKFLOW_*` namespace on 2026-09-09, but this file **never mentions
> `workflowScript`**. It belongs to the **multi-instance session-partitioning** programme
> (with `SCOPE_11`…`SCOPE_17`), whose thesis is *"two cyrup instances sharing a directory must
> not consume each other's runs."*
>
> **It does not block `workflowScript` and `workflowScript` does not block it.** Shipping
> workflows requires [`WORKFLOW_2`](WORKFLOW_2.md) → `3` → `4` → `5`, not this file. Do not read
> the numbering as a runway.
>
> ⚠ If you are here because [`WORKFLOW_2`](WORKFLOW_2.md) or
> [`WORKFLOW_13`](WORKFLOW_13.md) deferred async-workflow work to this file: **it is not owned
> here.** That orphan is [`WORKFLOW_13`](WORKFLOW_13.md)'s — see [`WORKFLOW_1`](WORKFLOW_1.md) §4.

OBJECTIVE: close the last two session gates in the status/reporting surface — the async status
snapshot widget (`async-status-snapshot.ts`, 48 LOC) and S6's live workflow controls
(`run-status.ts:609-612`). Both are small; both were blocked on state that WORKFLOW_6 and WORKFLOW_10 build.

---

# [AUG] AUGMENTATION — 2026-09-15

Everything below the `[AUG]` marker was produced by opening every file named and by reading the
upstream at a **pinned commit**, never from a working tree and never by line number off an
unpinned HEAD. **Nothing in the original body has been softened, narrowed or removed** — the
original body is reproduced verbatim in [§9](#9--original-body-verbatim-nothing-removed), and every
place where the draft's mechanism cannot work is recorded *next to* the requirement it belongs to,
not in place of it.

**Headline: the draft is not wrong about WHAT to do, it is wrong about HOW BIG it is.**
Three findings dominate everything else:

1. **`async-status-snapshot.ts` is a 48-line RE-EXPORT SHIM.** `buildAsyncStatusSnapshot` is one
   line delegating to `projectAsyncStatusSnapshot` in `runs/shared/async-status-projection.ts`
   (565 LOC). The real port is ~330 LOC of that file plus the 48-line facade. See [§3](#3--subtask1-augmented).
2. **`format_status` has no `deps`.** cyrup's renderer is
   `async fn format_status(status: &RunStatus, paths: &RunPaths) -> String`
   (`background/run_status.rs:265`). There is no `state`, no foreground registry, no workflow
   controller registry and no current session in scope. S6 is a **signature change threaded through
   four functions**, not a one-expression insert. See [§4](#4--subtask2-augmented-s6).
3. **`:249` and `:368` are two halves of ONE unported feature**, not two loose gates. `:249` lives
   inside `formatLiveForegroundTranscript` (upstream `:248-292`) which has **no cyrup port at all**,
   and `:368` is the filter inside the no-id transcript branch (`:366-372`) whose only purpose is to
   pick the control that `:249` is then applied to. Porting either gate alone lands a predicate with
   nothing to predicate over. See [§5](#5--subtask3-augmented).

`alreadyImplemented = false`. Nothing here is satisfied in the tree; the verification is in
[§1.4](#14--what-is-definitively-not-in-the-tree).

---

## [AUG] §0 — The upstream pin

| | |
|---|---|
| **repo** | `/home/user/cyrup/tmp/pi-subagents` |
| **pin** | `7fe9dee1` — `git describe --tags 7fe9dee1` → **`v0.65.1-50-g7fe9dee1`** |
| **why this pin** | Same pin SCOPE_11 landed against (#139). It is the ONLY tag/commit in the clone at which **both** of this task's upstream facts hold simultaneously. |

Verification, by `git -C /home/user/cyrup/tmp/pi-subagents show <rev>:<path>` only:

```
rev        async-status-snapshot.ts   run-status.ts   :609 is `const liveWorkflowControls`
v0.41.0    (absent)                   503             no
v0.60.0    48                         694             no (at :608)
v0.65.1    48                         713             no
7fe9dee1   48                         783             YES   <-- the pin
v0.67.0    46                         786             YES (identical expression, :609-613)
```

The file does not exist at `v0.41.0` at all — do not chase it there. At `v0.67.0` the *expression*
is byte-identical but the snapshot shim has been trimmed to 46 lines; the draft says "48 LOC", so
`7fe9dee1` is the shape the draft was written against and is the shape to port.

### [AUG] §0.1 — Every upstream citation in the draft, re-checked line-for-line at `7fe9dee1`

| draft citation | verdict at `7fe9dee1` |
|---|---|
| `async-status-snapshot.ts` exists, 48 LOC | ✅ exists, exactly 48 lines |
| `:24` `ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX = "PI_SUBAGENT_ASYNC_JSON:"` | ✅ verbatim |
| `:26` `buildAsyncStatusSnapshot(jobs, options)` | ✅ |
| `:30` `asyncStatusSnapshotJobsForState(state, sessionId)` | ✅ |
| `:31` the STRICT state gate | ✅ verbatim |
| `:34` the per-job session filter | ✅ — **but see §0.2, the draft stops one loop short** |
| `:42` `buildAsyncStatusSnapshotForState` | ✅ |
| `:46` `encodeAsyncStatusSnapshotWidget` | ✅ |
| `run-status.ts:249` the throwing STRICT gate | ✅ verbatim, inside `formatLiveForegroundTranscript` (`:248`) |
| `run-status.ts:368` the session-filtered control list | ✅ verbatim |
| `run-status.ts:609-612` `liveWorkflowControls` | ⚠️ **`:609-613`** — the draft's range omits the `: [];` alternative arm at `:613`. The quoted TS in the draft *does* include it; only the range label is short. |
| `:487`/`:514` "adjacent capacity reads" | ⚠️ off by one: they are **`:486`** and **`:513`** (`inspectActiveAsyncCapacityOwner`). Semantically correct and **out of scope here** — they are SCOPE_9's (`active-async-capacity.ts`). |
| `:494`/`:521` async transcript gate "landed already" | ✅ upstream lines correct; ✅ cyrup port confirmed at `extension/executor/status.rs:444-456` |

### [AUG] §0.2 — The draft's "two filters, not one" is **three**, not two

The draft's TS excerpt of `asyncStatusSnapshotJobsForState` shows one `for` loop. The real function
has **two**, and the second carries a dedup rule:

```ts
// async-status-snapshot.ts:30-40 @7fe9dee1   (VERBATIM)
export function asyncStatusSnapshotJobsForState(state: SubagentState | undefined, sessionId: string | null | undefined): AsyncJobState[] {
	if (!state || !sessionId || state.currentSessionId !== sessionId) return [];   // :31 STRICT
	const jobs = new Map<string, AsyncJobState>();
	for (const job of state.asyncJobs.values()) {
		if (job.sessionId === sessionId) jobs.set(job.asyncId, job);               // :34
	}
	for (const job of state.fleetJobs?.values() ?? []) {                           // :36  <-- MISSING FROM THE DRAFT
		if (job.sessionId === sessionId && !jobs.has(job.asyncId)) jobs.set(job.asyncId, job);  // :37 first-writer-wins
	}
	return [...jobs.values()];
}
```

`state.fleetJobs` is a **separate, bounded** map (`shared/types.ts:2244`, written at
`async-job-tracker.ts:55-60` with a `MAX_RECENT_FLEET_JOBS` eviction and at
`subagent-executor.ts:5213-5214` for the synthetic *workflow* job). `asyncJobs` wins on collision.

**cyrup has no `fleetJobs` analogue.** `tui/fleet_state.rs:487-489` documents its single
`tracked_jobs` field as *"pi `state.fleetJobs ?? state.asyncJobs`"* — the two upstream maps are
already collapsed into one on this side. So the second loop and its dedup **collapse away**, and
that collapse must be written down as a `[CYRUP-DELTA]` at the seam. Do NOT invent a second map to
make the loop count match.

---

## [AUG] §1 — The seams, as the code stands on `claude/subagents-scope-3` @ `b2fdc7e`

Every line number below was read out of the file on this branch. None is carried forward from the
draft.

### [AUG] §1.1 — `ForegroundControlEntry` — the source of every `control.*` term

`crates/cyrup-ext-subagents/src/extension/executor/notices.rs:20-99`

```rust
#[derive(Clone)]                                             // :19  — Clone, NOT Debug (holds a CancelToken)
pub(crate) struct ForegroundControlEntry {                   // :20
    pub(crate) interrupt: CancelToken,                       // :23
    pub(crate) current_agent: Option<String>,                // :26
    pub(crate) current_index: Option<usize>,                 // :28
    pub(crate) current_activity_state: Option<crate::background::ActivityState>,
    pub(crate) mode: crate::background::RunMode,
    pub(crate) description: Option<String>,
    pub(crate) current_tool: Option<String>,
    pub(crate) current_path: Option<String>,
    pub(crate) turn_count: Option<u64>,
    pub(crate) tool_count: Option<u64>,
    pub(crate) tokens: Option<u64>,
    pub(crate) started_at: i64,
    pub(crate) updated_at: i64,                              // :62
    pub(crate) session_id: Option<crate::identity::SessionId>,          // :68
    pub(crate) parent_workflow_run_id: Option<RunId>,                   // :73
    pub(crate) workflow_key: Option<crate::workflows::WorkflowKey>,
    pub(crate) cwd: Option<std::path::PathBuf>,                         // :83
    pub(crate) session_name: Option<String>,                            // :87
    pub(crate) active_children: std::collections::BTreeMap<             // :95-98
        usize,
        crate::extension::executor::foreground_control::ForegroundChildEntry,
    >,
}                                                            // :99
```

Invariants the surrounding code relies on, all load-bearing here:

* **The entry carries no `run_id`.** Upstream's `ForegroundRunControl.runId` is a field; cyrup's is
  the **map key**. `workflow_steering.rs:55-58` states this explicitly and carries
  `control_run_id: String` alongside the cloned entry for exactly that reason. Every rendered line
  in S6 interpolates the run id, so the projection MUST carry the key.
* **`active_children` is a `BTreeMap`, deliberately** (`notices.rs:89-94`): every upstream reader
  sorts the keys (`run-status.ts:687`), and an ordered map makes upstream's
  `[...keys()].sort((l,r) => l-r)` a **no-op**. Iterate `.keys()` directly; do **not** re-sort.
* **`session_id` is `Option<SessionId>`, never `Option<String>`** (`notices.rs:63-68`). `SessionId`
  is `#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]`
  (`identity/session_id.rs:39`) with `parse`/`parse_opt`/`as_str` (`:47`/`:59`/`:66`).
* **The registry is `Arc<Mutex<HashMap<String, ForegroundControlEntry>>>`**
  (`extension/executor/mod.rs:157`), a **`std::sync::Mutex`**. The crate's rule (stated at
  `mod.rs:168-169` and again at `workflow_child_stops.rs:36`) is: *no `.await` inside the critical
  section.* `format_status` is `async`. ⇒ **the projection must be built by the executor and handed
  down**; the renderer must never hold this lock.
* Poison handling in this crate is always `.unwrap_or_else(std::sync::PoisonError::into_inner)`
  (`status.rs:530-533`, `workflow_controllers.rs:93`), never `.unwrap()` — `lib.rs:20-24` has
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`.

### [AUG] §1.2 — The workflow-controller registry (the `workflowControllers.has` arm)

`crates/cyrup-ext-subagents/src/extension/executor/workflow_controllers.rs`

```rust
pub(crate) struct WorkflowController { abort: CancelToken, session_id: Option<SessionId>, started_at: i64 }  // :40-53
impl SubagentExecutor {
    pub(crate) fn register_workflow_controller(&self, run_id: &RunId, abort: CancelToken) -> WorkflowController; // :85
    pub(crate) fn settle_workflow_controller(&self, run_id: &RunId) -> Option<WorkflowController>;              // :115
    pub(crate) fn live_workflow_run_ids(&self) -> HashSet<RunId>;                                               // :125
    pub(crate) fn has_workflow_controller(&self, run_id: &RunId) -> bool;                                       // :137
}
```

`has_workflow_controller` (`:137`) **is** pi's `state.workflowControllers?.has(runId)` and is already
`pub(crate)`. It was ported for WORKFLOW_7; S6 is its second caller. Field is
`Arc<Mutex<HashMap<RunId, WorkflowController>>>` (`mod.rs:171-172`), keyed by **typed `RunId`**, not
`String`, and **never prefix-matched** (`mod.rs:161-166`).

Existing tests to sit beside: `live_workflow_run_ids_tracks_registration_and_settlement` (`:241`),
`has_workflow_controller_tracks_registration_and_settlement` (`:289`),
`register_workflow_controller_carries_the_current_session_id` (`:325`).

### [AUG] §1.3 — The three-term predicate already exists, with ONE wrong bound

`crates/cyrup-ext-subagents/src/extension/executor/workflow_steering.rs:86-96`

```rust
fn control_is_live_in_workflow(
    control: &ForegroundControlEntry,
    workflow_run_id: &RunId,
    session_id: &SessionId,            // <-- &SessionId, i.e. a KNOWN-Some session
) -> bool {
    control.parent_workflow_run_id.as_ref() == Some(workflow_run_id)
        && control.session_id.as_ref() == Some(session_id)
        && !control.active_children.is_empty()
}
```

This is *exactly* S6's inner filter (`run-status.ts:610-612`) — it is pi's
`controlIsLiveInWorkflow` (`workflow-foreground-steering.ts:32-36`), and upstream reuses the same
three terms in both places. **But its session parameter is `&SessionId`, not `Option<&SessionId>`**,
because its caller `active_workflow_error` (`:111`) hoists `!state.currentSessionId` into a hard
refusal first (`:122-124`). S6 has **no such hoist** — see §4.2. Widening this function's parameter
in place would silently relax WORKFLOW_7's gate.

### [AUG] §1.4 — What is definitively NOT in the tree

Each of these was grepped across `crates/` and came back empty or negative:

| thing | evidence |
|---|---|
| any async-status-snapshot port | `rg 'async_status_snapshot\|AsyncStatusSnapshot\|PI_SUBAGENT_ASYNC_JSON\|project_async_status'` over `crates/**/*.rs` → **0 hits** |
| any `Steer live foreground child:` / `Steer: unavailable; no live foreground route` line | `rg` over `crates/**/*.rs` → **0 hits** |
| any port of `formatLiveForegroundTranscript` | `rg 'is not owned by the current session'` → **0 hits** |
| `state.lastForegroundControlId` | `rg 'last_foreground'` → **0 hits** (upstream writes it at `extension/index.ts:472,954`, `subagent-executor.ts:545,551`) |
| `state.fleetJobs` as a distinct map | collapsed into `FleetState::tracked_jobs`, `tui/fleet_state.rs:487-489` |
| `RunStatus::workflow_graph` / `host_steps` / `nested_children` | `records.rs:210-352` field list — **none of the three exist** |

⚠️ **`AsyncJobSnapshot` (`tui/events.rs:750`) is NOT this.** It is the C21 *render row* — a
`Vec<Line>` feed for `render_async_jobs_widget` (`tui/events.rs:865`). The task's snapshot is a
JSON wire document. Same English word, unrelated types. Do not extend `AsyncJobSnapshot`.

### [AUG] §1.5 — Corrected internal citations from the draft's own Research notes

| draft | actual, on this branch |
|---|---|
| `list_active_runs`'s session filter at `run_status.rs:647` | **`run_status.rs:719-723`** (`if let Some(wanted) = session_id && status.session_id.as_ref().map(SessionId::as_str) != Some(wanted)`) |
| "its doc at `:586-595`" explaining the drop-unattributed rule | **`run_status.rs:650-664`** — the `# Session scoping (SUBA-031)` block. The rule is still stated exactly as the draft describes it, and it is still the model to follow. |
| "Already landed in `extension/executor/status.rs`: the async transcript gate (`:494`/`:521`)" | ✅ **`status.rs:444-456`**, `SessionGate::Permissive.admits(...)` + upstream's verbatim refusal string |
| "Requires WORKFLOW_6's `ForegroundControlEntry::{session_id, parent_workflow_run_id, active_children}`" | ✅ all three present, `notices.rs:68`/`:73`/`:95`. **Dependency satisfied.** |

---

## [AUG] §2 — ⚠ THE MECHANISM PROBLEM: `SessionGate` is the WRONG primitive for S6

This is the finding worth the most. An implementor reading this crate will reach for
`crate::background::delivery::SessionGate` — it is the established primitive, its own doc
(`background/delivery/gate.rs:1-40`) literally names `run-status.ts:249`, `:494`, `:521`, and it is
what `status.rs:444` already uses. **For `:249` that is correct. For S6's two comparisons it is
wrong, and wrong in a way that compiles and passes a happy-path test.**

`SessionGate::admits` (`gate.rs:52-58`):

```rust
pub fn admits(self, current: Option<&SessionId>, record: Option<&SessionId>) -> bool {
    match current {
        None => self == Self::Permissive,
        Some(current) => record == Some(current),
    }
}
```

S6's two comparisons are **raw JS `===` between two `string | undefined`**, which is
`Option<SessionId>` **structural equality**, not either gate class:

| `current` | `record` | JS `a === b` (S6) | `Strict.admits` | `Permissive.admits` |
|---|---|---|---|---|
| `None` | `None` | **true** | false ❌ | true |
| `None` | `Some(x)` | **false** | false | true ❌ |
| `Some(x)` | `None` | false | false | false |
| `Some(x)` | `Some(x)` | true | true | true |
| `Some(x)` | `Some(y)` | false | false | false |

Both classes disagree with S6 in exactly one row, and they disagree in *different* rows.
The row that bites in practice is **`None`/`None`**: a headless / SDK-embedder orchestrator with no
session identity running a workflow whose `status.json` records no session. Upstream renders its
steer hints; `SessionGate::Strict` would silently suppress them and the run would report
`Steer: unavailable; no live foreground route is registered in the active session.` forever.

**The mechanism that DOES work:**

```rust
// gate 1 — pi `deps.state?.currentSessionId === status.sessionId` (run-status.ts:609)
current_session.as_ref() == status.session_id.as_ref()      // Option<&SessionId> == Option<&SessionId>

// gate 2 — pi `control.sessionId === status.sessionId` (run-status.ts:611)
control.session_id.as_ref() == status.session_id.as_ref()
```

`SessionId` derives `PartialEq` (`identity/session_id.rs:39`), so `Option<&SessionId>` equality is
free and total. **Write a doc comment at both sites saying why `SessionGate` is not used**, or the
next reader will "fix" it back.

`:249` is the opposite: it **is** `SessionGate::Strict` — `gate.rs:33-35` names it by line number —
and it must stay `Strict`.

---

## [AUG] §3 — SUBTASK1 augmented

### 3.1 — The real port is two files, not one

`git -C tmp/pi-subagents show 7fe9dee1:src/runs/background/async-status-snapshot.ts` is 48 lines of
which **22 are re-export statements**. The body is:

```ts
:26  buildAsyncStatusSnapshot(jobs, options) { return projectAsyncStatusSnapshot(jobs, options); }
```

`projectAsyncStatusSnapshot` lives in **`src/runs/shared/async-status-projection.ts` (565 LOC)** at
`:546-565`. The transitive closure reachable from it is:

| upstream lines | what | port? |
|---|---|---|
| `:8-9` | `ASYNC_STATUS_SNAPSHOT_KIND = "pi-subagents.async-status-snapshot"`, `VERSION = 1` | **yes — wire constants** |
| `:11-15` | the five `DEFAULT_MAX_*` caps (`20`, `8`, `3`, `160`, `32*1024`) | **yes** |
| `:17-33` | `AsyncStatusSnapshotState` / `Kind` + `isAsyncStatusSnapshotState` | **yes** (Rust `enum` + serde) |
| `:35-100` | the six snapshot structs + `AsyncStatusSnapshotOptions` | **yes** |
| `:143-151` | `resolveCaps` (`max(0, floor(..))`, `maxSerializedBytes` floored at **256**) | **yes** |
| `:153-171` | `publicText`/`publicOptionalText`/`publicTime`/`publicCount` | **yes** |
| `:173-193` | `normalizeState` / `terminalState` / `kindForMode` / `labelForAgents` | **yes** |
| `:195-223` | `activityFor`, `appendBoundedChildren` | **yes** |
| `:225-250` | `projectStep` | **yes** |
| `:252-288` | `projectWorkflowGraphNode`, `projectNestedRun` | partial — see §3.3 |
| `:290-323` | `hostStepSnapshotState`, `projectHostStep` | see §3.3 |
| `:325-363` | `projectRun` | **yes** |
| `:365-385` | `snapshotBytes` + `enforceByteLimit` (**binary search**, not a truncate) | **yes** |
| `:102-123`, `:387-543` | `AsyncStatusWorkflowRow` + `projectAsyncWorkflowRows` + its helpers | **NO — not reachable from `projectAsyncStatusSnapshot`; out of scope.** `workflowGraphStepStatus` (`:449-458`) IS reachable via `projectWorkflowGraphNode` and is the one exception. |

So: **≈330 LOC of projection + the 48-line facade**, not 48 LOC. The draft's `(48 LOC)` is the
shim's size and must not be read as the task's size.

### 3.2 — The behaviours inside the projection that a re-derivation will get wrong

These are the reason this is worth writing down rather than re-deriving:

* **`enforceByteLimit` (`:369-385`) is a BINARY SEARCH over retained runs**, not a truncate. It sets
  `omitted.byteLimitExceeded = true` *before* searching, mutates `snapshot.runs`/`snapshot.omitted.runs`
  in place at each probe, and settles on `lower`. Naively slicing gives a different (smaller)
  retained set for the same input.
* **`normalizeState` (`:173-178`) maps `"completed" -> "complete"` and `"pending" -> "queued"`**, and
  **anything unrecognised becomes `"partial"`, never an error.**
* **`projectRun`'s child budget is not FIFO (`:340-355`).** Host steps are reserved FIRST
  (`retainedHostSteps = hostStepChildren.slice(0, maxChildrenPerNode)`), then ordinary children take
  what is left, but the emitted order is `[...ordinary, ...hostSteps]`. Reserve-then-emit-reversed.
* **`endedAt` is only emitted for a `terminalState`** (`:238`, `:275`, `:337`) — a running node with
  an `endedAt` must not carry one.
* **`publicText`'s pre-slice is `maxLength * 4`** (`:155`) before sanitizing, so a string of control
  sequences cannot make sanitisation quadratic.
* **Sorting (`:549-553`) is `updatedAt ?? startedAt ?? 0` DESC, tie-broken by `asyncId` ascending** —
  `localeCompare`, i.e. cyrup `str::cmp` (ASCII run ids; no locale collation to reproduce).
* `omitted.runs` is incremented by `max(0, sorted.length - caps.maxRuns)` **before** slicing (`:554`),
  and `enforceByteLimit` adds to that same counter rather than resetting it.

### 3.3 — `[CYRUP-DELTA]`s, each to be recorded AT its seam

The projection's input is upstream's in-memory `AsyncJobState` (`shared/types.ts:1968-2028`).
cyrup's nearest thing is **`crate::tui::fleet_state::AsyncRunView`** (`tui/fleet_state.rs:393-411`):
`{ paths: RunPaths, status: RunStatus, session_id: Option<String>, description: Option<String>, context: Option<ContextMode>, nested_children: Vec<NestedRunView> }`.
Map through it; do not invent a new job record.

| upstream field | cyrup source | delta |
|---|---|---|
| `asyncId` | `status.run_id` (`records.rs:212`) | none |
| `sessionId` | `status.session_id: Option<SessionId>` (`records.rs:230`) | ⚠ `AsyncRunView::session_id` is `Option<String>`; **filter on `status.session_id`, the typed one** |
| `status` | `status.state: RunState` (`records.rs:243`) + `run_state_label` (`run_status.rs:45`) | none |
| `mode` | `status.mode: RunMode` (`records.rs:241`) | none |
| `agents` | **absent** — cyrup has `status.steps[].agent` (`records.rs:26`) | `labelForAgents` must fold the step agents; state it |
| `steps` | `status.steps: Vec<StepStatus>` | `StepStatus` carries **no `label`/`phase`** — the same delta `fleet_view.rs:36-41` and `run_status.rs` already record; `projectStep`'s `label` collapses to `agent` |
| `nestedChildren` | `AsyncRunView::nested_children: Vec<NestedRunView>` (`fleet_state.rs:70-100`) | present; one level only |
| `workflowGraph` | **unrepresentable.** `RunStatus` has no such field, and `workflow_graph_from_run` (`background/workflow_graph.rs:718`) needs a `&[RunnerStep]` plan the status does not carry. | `projectWorkflowGraphNode` + the `graphChildren`/`graphCount` arms of `projectRun` (`:343-346`, `:358-359`) collapse to empty. **Record it; do not fake a graph.** |
| `hostSteps` | **unrepresentable on `RunStatus`.** `HostStepNode` exists (`workflows/scripted/engine.rs:71`) but lives on the live engine, not on the persisted status. | `validHostStepList`/`projectHostStep` have no input ⇒ `hostStepChildren` is always empty. Port `hostStepSnapshotState` anyway or omit — **open question, see §8**. |
| `fleetJobs` | collapsed (§0.2) | the `:36-38` loop has no second source |

Helpers that already exist and MUST be reused rather than re-written:

* `crate::workflows::display_text::sanitize_display_text` (`workflows/display_text.rs:82`) — pi
  `sanitizeDisplayText`. ⚠ **NOT** `tui/fleet_transcript.rs:152`'s `safe_display_text`, which is
  `safeTerminalText` and *escapes* rather than *strips*; `display_text.rs:1-9` warns about exactly
  this confusion.
* `crate::workflows::display_text::truncate_display` (`:153`) — pi `truncateDisplayText`
  (`shared/display-text.ts:84-93`): hard cut, **no ellipsis**, `max <= 0 -> ""`. Byte-for-byte the
  same contract; the only stated delta is astral-character boundary behaviour (`display_text.rs:147-151`).
* `crate::time::now_epoch_millis()` — pi `Date.now()` for `generatedAt`. **Thread ONE clock read**,
  the discipline `control_inspect` already states (`status.rs:509`).

### 3.4 — Layout, and the constants that are the wire shape

One concern, one file — but the concern is two: the projection and the facade. Follow
`background/inspect_rpc/`'s own layout (`inspect_rpc/mod.rs:22-28`), which is the nearest sibling
and landed in #139:

```text
background/async_status_snapshot/
  mod.rs        facade + the wire constants + the narrative (mirrors inspect_rpc/mod.rs:70-79)
  types.rs      AsyncStatusSnapshot{,Node,Activity,Caps,Omitted,State,Kind,Options} + resolve_caps
  project.rs    project_async_status_snapshot + every private projector + enforce_byte_limit
  state.rs      async_status_snapshot_jobs_for_state + build_async_status_snapshot_for_state
                (the :31/:34 filters — the ONLY session-aware file)
```

The wire constants, verbatim, all three:

```rust
/// pi `ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX` (`async-status-snapshot.ts:24`).
pub const ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX: &str = "PI_SUBAGENT_ASYNC_JSON:";
/// pi `ASYNC_STATUS_SNAPSHOT_KIND` (`async-status-projection.ts:8`).
pub const ASYNC_STATUS_SNAPSHOT_KIND: &str = "pi-subagents.async-status-snapshot";
/// pi `ASYNC_STATUS_SNAPSHOT_VERSION` (`async-status-projection.ts:9`).
pub const ASYNC_STATUS_SNAPSHOT_VERSION: u32 = 1;
```

**The rebrand does not apply to any of the three.** The draft says so for the prefix; it is equally
true of `KIND`, which is serialized *into* the JSON the prefix introduces. Precedent, landed in
#139: `inspect_rpc/mod.rs:71-79` keeps `INSPECT_REPLY_KIND = "pi-subagents.inspect-reply"` and
`INSPECT_WIDGET_PREFIX = "PI_SUBAGENT_INSPECT_JSON:"` verbatim for the identical reason.

Serde: `#[serde(rename_all = "camelCase")]` on every snapshot struct and
`#[serde(skip_serializing_if = "Option::is_none")]` on every optional field — upstream builds its
objects with `...(x ? {x} : {})` spreads, i.e. **absent, not null**. A `null` in the JSON is a port
defect, not a cosmetic difference.

### 3.5 — ⚠ The consumer problem (the draft does not mention it)

Upstream has exactly two callers (`git grep` at `7fe9dee1`):

* `extension/rpc.ts:725,749` — the subagent RPC bridge's `status` method, returning `asyncSnapshot`
  alongside `fleet`. **cyrup has no such bridge.**
* `tui/render.ts:2863` — `ctx.ui.setWidget(WIDGET_KEY, encodeAsyncStatusSnapshotWidget(jobs))`,
  reached only when `ctx.mode === "rpc"`. **cyrup's extension surface has no `set_widget`
  capability** — `tui/events.rs:44-52` states this at length for the C21 widget and says the
  function's only caller is its own test *"until `cyrup-ext` grows a `set_widget` capability — a
  change outside this crate."*

So `encode_async_status_snapshot_widget` **will have no production caller**. That is acceptable —
`background/` is a `pub mod` (`background/mod.rs:36-79`), so `pub fn`s are public API and do not
trip `dead_code` — but it must be a **decision that is written down**, exactly as `tui/events.rs:44`
writes down C21's. See §8 for the choice the implementor must not make silently.

---

## [AUG] §4 — SUBTASK2 augmented (S6)

**Where:** `crates/cyrup-ext-subagents/src/background/run_status.rs`
**Ports:** `pi-subagents/src/runs/background/run-status.ts:609-613` (predicate) **and `:684-691`
(the consumer)** — the draft names only the predicate. A predicate with no consumer renders nothing.

### 4.1 — The consumer, verbatim at `7fe9dee1` (`:684-691`)

```ts
if (status.mode === "workflow" && status.state === "running") {                 // :684
    if (liveWorkflowControls.length === 0) lines.push("Steer: unavailable; no live foreground route is registered in the active session.");   // :685
    else for (const control of liveWorkflowControls) {                          // :686
        for (const index of [...control.activeChildren!.keys()].sort((left, right) => left - right)) {   // :687
            lines.push(`Steer live foreground child: subagent({ action: "steer", id: "${control.runId}", index: ${index}, message: "..." })`); // :688
        }
    }
}
```

Both strings are wire text; reproduce them byte-for-byte. Position: **after** the unattached-nested
lines (`:683`) and **before** `Warning:` / `Workflow receipt:` / `Session:` (`:690-693`). In cyrup's
`format_status` that is: after the per-step loop (`run_status.rs:311-357`) and **immediately before**
the `Workflow receipt:` push at `run_status.rs:363-365`. The relative order *is* the transferable
anchor — the same argument `run_status.rs:359-362` already makes for that line.

### 4.2 — The predicate, and why `format_status` cannot evaluate it

```rust
// crates/cyrup-ext-subagents/src/background/run_status.rs:265
async fn format_status(status: &RunStatus, paths: &RunPaths) -> String
```

No `state`. No registries. No session. The call chain that must be widened, with current signatures:

```rust
run_status.rs:265  async fn format_status(status: &RunStatus, paths: &RunPaths) -> String
run_status.rs:400  async fn inspect_paths(paths: &RunPaths) -> Result<Option<String>, SubagentError>
run_status.rs:520  pub async fn inspect_status_by_id(async_root: &Path, results_dir: &Path, selector: &str)
                       -> Result<Option<String>, SubagentError>
run_status.rs:594  pub async fn inspect_status_by_dir(async_dir: &Path, results_dir: &Path)
                       -> Result<Option<String>, SubagentError>
```

Callers — there are exactly **three** in the whole workspace (`rg` verified):

* `extension/executor/status.rs:424` — `inspect_status_by_id`
* `extension/executor/status.rs:428` — `inspect_status_by_dir`
* `crates/cyrup-it/tests/subagents/background_runner_main_integration.rs:2207` — `inspect_status_by_id`

⇒ the blast radius of the signature change is **three call sites**, one of them a test. Small, but it
is a public-API change to two `pub async fn`s and must be stated as such.

**The shape to thread.** Because `foreground_controls` is a `std::sync::Mutex` and `format_status` is
`async`, the renderer must receive an already-materialised, lock-free value:

```rust
/// pi's `liveWorkflowControls` element, reduced to the two facts `run-status.ts:688`
/// interpolates. `run_id` is the `foreground_controls` MAP KEY (`notices.rs` carries no
/// `run_id` field — `workflow_steering.rs:55-58`).
#[derive(Clone, Debug, Default)]
pub struct LiveWorkflowControl {
    pub run_id: String,
    /// `control.activeChildren.keys()`, ALREADY ascending — `active_children` is a `BTreeMap`
    /// (`notices.rs:89-98`), so pi's `.sort((l,r) => l-r)` at `:687` is a no-op here.
    pub child_indexes: Vec<usize>,
}

/// pi's `deps` subset `formatAsyncRunStatus` reads. An empty/`Default` value is upstream's
/// `deps.state === undefined`: every optional-chain short-circuits and `liveWorkflowControls`
/// is `[]` (`run-status.ts:613`).
#[derive(Clone, Debug, Default)]
pub struct RunStatusRenderDeps {
    pub current_session: Option<crate::identity::SessionId>,
    /// `true` iff `has_workflow_controller(status.run_id)` — evaluated by the EXECUTOR, which
    /// owns the registry (`workflow_controllers.rs:137`).
    pub workflow_controller_live: bool,
    /// Every live foreground control, pre-projected. Filtering happens here, not in the executor,
    /// so gate 2 is visible next to gate 1.
    pub foreground_controls: Vec<LiveWorkflowControlCandidate>,
}
```

…where `LiveWorkflowControlCandidate` additionally carries
`session_id: Option<SessionId>` and `parent_workflow_run_id: Option<RunId>`.

**Alternative, if a new pub struct on `background::run_status` is judged too wide:** have the
executor compute the final `Vec<LiveWorkflowControl>` and pass only that plus nothing else. This
moves gate 1 and gate 2 into `status.rs`, next to the registry they read, and keeps `run_status.rs`
a pure renderer. **This is an open question (§8, Q2), not a decision to make silently** — it changes
which file the two session comparisons live in, and therefore which file the tests live in.

⚠ **`ForegroundFleetEntry` (`fleet_view.rs:374-390`) is NOT sufficient** and must not be reused:
it carries `run_id`/`current_agent`/`current_index`/`activity_state`/`session_id`/
`parent_workflow_run_id` but **no `active_children`**, so the third term of the filter cannot be
evaluated from it. Its own doc (`fleet_view.rs:383-384`) says it deliberately projects only what the
TEXT fleet renderer plus "later gates" need. Either extend it with the child indexes — which changes
`foreground_fleet_entries` (`status.rs:528-553`) and every fleet test — or add the new projection
above. Prefer the new projection; `fleet_view`'s entry is a *render* shape.

### 4.3 — The predicate, written out with the §2 correction applied

```rust
// pi `run-status.ts:609-613`. FOUR terms; all four are required and none is redundant.
let live_workflow_controls: Vec<LiveWorkflowControl> =
    if status.mode == RunMode::Workflow
        // gate 1 — `deps.state?.currentSessionId === status.sessionId`.
        // ⚠ Option EQUALITY, not `SessionGate`. See §2: both gate classes disagree with `===`
        // in exactly one row, and `None`/`None` (a headless host running an unattributed
        // workflow) is the row that bites.
        && deps.current_session.as_ref() == status.session_id.as_ref()
        // `deps.state?.workflowControllers?.has(status.runId)` — `workflow_controllers.rs:137`.
        && deps.workflow_controller_live
    {
        deps.foreground_controls
            .iter()
            .filter(|c| {
                c.parent_workflow_run_id.as_ref() == Some(&status.run_id)   // :610
                    && c.session_id.as_ref() == status.session_id.as_ref()  // :611  ⚠ Option eq again
                    && !c.child_indexes.is_empty()                          // :612  (activeChildren.size > 0)
            })
            .cloned()
            .collect()
    } else {
        Vec::new()                                                          // :613
    };
```

**The draft's "two session comparisons in one expression" is exactly right and is the thing most
likely to be collapsed.** They are against two *different* objects — the state's current session vs
the status's, and each control's session vs the status's — and a control can carry a stale session
after a rotation. Keep both, with the comment.

`deps.workflow_controller_live` is evaluated **before** the disk-bound work either way in cyrup
(`format_status` runs after reconciliation), so upstream's "checked before any disk read" ordering
argument (`workflow_controllers.rs:133-136`) does not transfer — say so rather than implying it does.

### 4.4 — This is reachable: a workflow run DOES have a `status.json`

Worth stating because `RunMode::Workflow`'s own doc (`background/state.rs:25-30`) says the
background runner never emits it, which reads like "there is nothing on disk to inspect".
There is: `extension/tool/routing.rs:618-656` mints the workflow run id, creates
`<async_root>/<run_dir>/`, builds `RunStatus::queued(.., RunMode::Workflow, Some(pid))`, sets
`status.session_id` from the live session (`:640-641`), advances to `Running` (`:649-651`) and
writes `status.json` (`:653-655`) — *"THE FIRST PRODUCTION `RunMode::Workflow` IN THE CRATE"*
(`:632`). `inspect_status_by_id` resolves it like any other run.

---

## [AUG] §5 — SUBTASK3 augmented

**Where:** `background/run_status.rs`, `extension/executor/status.rs` — both, as the draft says.

### 5.1 — ⚠ `:249` and `:368` are one feature, and its host function is unported

`:249` is **the first statement of `formatLiveForegroundTranscript`** (upstream `:247-292`):

```ts
/** Request-only snapshot of the same artifact used by Fleet; no session or output fallback. */
function formatLiveForegroundTranscript(control: ForegroundRunControl, state: SubagentState, options: { index?: number; lines?: number }): string {   // :248
	if (!state.currentSessionId || control.sessionId !== state.currentSessionId) {                                    // :249  STRICT — THROWS
		throw new Error(`Foreground run '${control.runId}' is not owned by the current session.`);                     // :250
	}
	if (options.index !== undefined && (!Number.isInteger(options.index) || options.index < 0))
		throw new Error("Transcript index must be a non-negative integer.");                                           // :252-254
	const children = control.activeChildren ? [...control.activeChildren.values()]
		: control.currentAgent ? [{ index: control.currentIndex ?? 0, agent: control.currentAgent, sessionName: control.sessionName }] : [];  // :255-257
	const header = [`Run: ${control.runId}`, "State: live foreground"];                                                // :258
	if (children.length === 0) return `${header…}\nTranscript unavailable: no active foreground child.`;               // :259
	if (options.index === undefined && children.length > 1)
		throw new Error(`Transcript view requires index for foreground run '${control.runId}'. Active child indexes: ${…}.`);  // :260-262
	const child = options.index === undefined ? children[0]! : children.find((c) => c.index === options.index);        // :263
	if (!child) throw new Error(`Transcript index ${options.index} is not an active foreground child of '${control.runId}'.`); // :264
	const root = getArtifactsDir(state.parentSessionFile ?? null, control.cwd ?? state.baseCwd, state.artifactDirPreference); // :265
	const transcriptPath = getArtifactPaths(root, control.runId, child.agent, child.index).transcriptPath;             // :266
	const transcript = readFleetTranscript(transcriptPath, { trustedRoots: [root] });                                  // :267
	const lineLimit = Number.isFinite(options.lines) ? Math.max(1, Math.min(500, Math.trunc(options.lines!))) : 80;    // :268
	…flatten events → slice(-lineLimit) → truncated flags → 32 KiB byte tail on a UTF-8 boundary…                      // :269-290
	header.push(`Child: ${child.index} (${child.sessionName?.trim() || child.agent})`, `Transcript: ${transcriptPath}`);// :288
	if (transcript.warning) header.push(`Transcript warning: ${transcript.warning}`);                                   // :289
	header.push(`Live transcript tail${truncated ? " (tail truncated)" : ""}:`);                                        // :290
	if (!body) header.push("Transcript unavailable: no readable activity in the bounded artifact tail yet.");           // :291
	return [...header.map(safeTerminalText), body].filter(Boolean).join("\n");                                          // :292
}
```

`:368` is the **selector that feeds it** (upstream `:366-372`):

```ts
if (!params.id && !params.runId && !params.dir) {                                                       // :366
	if (params.view === "transcript" && deps.state?.currentSessionId) {                                  // :367
		const controls = [...deps.state.foregroundControls.values()].filter((c) => c.sessionId === currentSessionId);  // :368
		const foreground = controls.find((c) => c.runId === deps.state?.lastForegroundControlId)
			?? controls.sort((l, r) => r.updatedAt - l.updatedAt)[0];                                      // :369-370
		if (foreground) return inspectSubagentStatus({ ...params, id: foreground.runId }, deps);          // :371
	}
```

and the by-id arm that re-enters it (`:412-418`):

```ts
const resolved = resolveSubagentRunId(requestedId, { asyncDirRoot, resultsDir, state: deps.state, nested: deps.nested });
if (resolved?.kind === "foreground") {                                                                   // :412
	const control = deps.state?.foregroundControls.get(resolved.id);                                       // :413
	if (control && deps.state && params.view === "transcript")                                             // :414
		return { content: [{ type: "text", text: formatLiveForegroundTranscript(control, deps.state, params) }], … };  // :416
```

**Consequence for scope.** The draft's *"port both the class and the shape"* for `:249` cannot be
satisfied by inserting a `SessionGate::Strict` call: there is no function for it to guard. To land
`:249` as a live, reachable gate the implementor must also land:

* the **foreground arm of run-id resolution** — cyrup already has the primitive:
  `SubagentExecutor::is_live_foreground_run(&self, selector: &str) -> bool`
  (`extension/executor/nested_control.rs:200-210`), which does exact-then-unique-prefix matching over
  the `foreground_controls` keys, i.e. pi's `resolved?.kind === "foreground"`. cyrup's
  `run_status::resolve_run_id` (`run_status.rs:446`) scans **only async run directories**, so a
  foreground id currently resolves to `Err("Async run not found. Provide id or dir.")`
  (`status.rs:427-431`);
* a **cyrup port of `formatLiveForegroundTranscript`**, for which every dependency exists:
  * `crate::artifacts::resolve_artifacts_dir(session_file, project_cwd, temp_cwd, preference)`
    (`artifacts.rs:235-257`) — pi `getArtifactsDir`;
  * `crate::artifacts::artifact_paths(dir, run_id, agent, index) -> ArtifactPaths`
    (`artifacts.rs:295-312`), whose `.transcript_path` is `<base>_transcript.jsonl` (`:310`) —
    pi `getArtifactPaths(...).transcriptPath`;
  * `crate::tui::fleet_transcript::read_fleet_transcript(&Path, &FleetTranscriptReadOptions) -> FleetTranscript`
    (`tui/fleet_transcript.rs:1140-1183`), with
    `FleetTranscriptReadOptions { trusted_roots: Vec<PathBuf>, max_records: Option<usize>, max_bytes: Option<u64> }`
    (`:346-355`) and
    `FleetTranscript { path, events: Vec<FleetTranscriptEvent>, truncated: bool, warning: Option<String> }` (`:333-343`).
    ⚠ **`trusted_roots: []` REFUSES the read outright** (`:348-350`) — there is no
    "no roots means anything goes" mode. Pass `vec![root]`, exactly as upstream's `{ trustedRoots: [root] }`;
  * `crate::tui::fleet_transcript::safe_display_text` (`:152`) for the header lines — this one **is**
    `safeTerminalText`, and it is the correct function here (contrast §3.3, where it is the wrong one);
  * `crate::workflows::display_text::truncate_to_bytes` (`display_text.rs:175`) for the 32 KiB tail
    — ⚠ it appends `"..."` and cuts from the **front** of the budget, whereas upstream keeps the
    **tail** and skips UTF-8 continuation bytes (`:283-287`). **Not interchangeable**; write the
    tail-keeping form locally and say why.
* a decision on **`state.lastForegroundControlId`**, which has **no cyrup port** (§1.4). Without it,
  `:369`'s first disjunct is unrepresentable and the selection degenerates to `:370`'s
  `updatedAt` DESC alone (`ForegroundControlEntry::updated_at`, `notices.rs:62`, is present and is
  bumped on every control-event transition). That degradation is **acceptable and must be recorded**;
  porting `lastForegroundControlId` is a separate task (its writers are
  `extension/index.ts:472,954`, `subagent-executor.ts:545,551`, `async-job-tracker.ts:759`).

### 5.2 — `:249` — the class IS `SessionGate::Strict`, and the shape is an `Err`

`crate::background::delivery::SessionGate::Strict` (`background/delivery/gate.rs:29-35`) names
`run-status.ts:249` in its own doc and is *"the one gate upstream implements as a throw"*. Both of
the draft's arms fall out of `admits` directly:

```rust
// pi `:249-251`. STRICT: no current session REFUSES (gate.rs:19, :52-57).
if !crate::background::delivery::SessionGate::Strict
    .admits(current_session.as_ref(), control.session_id.as_ref())
{
    return Err(format!(
        "Foreground run '{control_run_id}' is not owned by the current session."
    ));
}
```

**Shape:** cyrup's control surface has no `isError` flag; the crate's convention
(`extension/executor/status.rs:126-129`) is *"`Err` is the user-facing failure message the tool
surface turns into a `ToolError`"*. A `throw` upstream is an `Err(String)` here — that **is** porting
"the shape", and it is what `status.rs:450-455` already does for the sibling `:494`/`:521` gate.
**Do not `panic!`**: `lib.rs:20-24` denies it crate-wide.

### 5.3 — `:368` — the filter, and where it goes

cyrup's no-id transcript branch is `status.rs:407-418`:

```rust
match runs.as_slice() {
    [only] => resolved_id = Some(only.status.run_id.as_str().to_string()),
    []     => return Err("No active async run transcript is available.".to_string()),
    many   => return Err(format!("Transcript view requires an id when {} active async runs exist. …", many.len())),
}
```

Upstream runs the **foreground** selection (`:367-371`) **ahead of** the `listAsyncRuns` fallback
(`:381`). So `:368` is inserted **before** `status.rs:407`, not in place of it:

```rust
// pi `run-status.ts:367-371`. Runs BEFORE the async-run fallback below: a live foreground run
// in this session is the answer to a bare `view: "transcript"`, and only if there is none does
// the async listing get a turn.
if transcript && let Some(current) = SessionId::parse_opt(self.current_session_id().as_deref()) {
    // `:368` — the control list is session-filtered. Note this is `=== currentSessionId` with a
    // KNOWN-Some current session (the `if` above is pi's `deps.state?.currentSessionId` guard at
    // `:367`), so unlike S6 (§2) this one genuinely collapses onto `Some(&current)` equality.
    …filter, then `:369-370`'s selection, then render via the §5.1 port…
}
```

Note the asymmetry with §2 and record it: `:367`'s truthiness guard makes `currentSessionId`
known-`Some` before `:368` compares, which is why `:368` **is** expressible as
`c.session_id.as_ref() == Some(&current)` while S6's `:611` is not.

---

## [AUG] §6 — Tests

Every name below is a `#[test]`/`#[tokio::test]`. **Fail-before** is stated as the concrete symptom
on today's tree, so each test is a regression proof and not a tautology.

### Placement

* §3 tests → in-module `#[cfg(test)]` in `background/async_status_snapshot/*.rs`, following
  `background/inspect_rpc/respond.rs`'s in-module convention.
* §4 tests → in-module in `background/run_status.rs` (beside
  `list_active_runs_scopes_to_the_requested_session`, `run_status.rs:1127`) for the renderer, and in
  `extension/executor/status.rs`'s test module for the executor projection.
* §5 tests → `extension/executor/status.rs`'s test module. Reuse the helpers that already exist in
  `extension/executor/workflow_steering.rs`'s test module — `control_entry` (`:400-425`),
  `one_active_child` (`:429-453`), `with_session` (`:455-460`), `write_running_workflow_status`
  (`:473-495`), `must_err` (`:462-469`). ⚠ `ForegroundControlEntry` derives **no `Debug`**
  (`notices.rs:19`), which is why `must_err` exists; any new assertion over a `Result` carrying one
  needs the same treatment.

| test | pins | fail-before, today |
|---|---|---|
| `the_snapshot_is_empty_when_the_state_session_does_not_match` | `:31` STRICT, first filter | no module exists |
| `the_snapshot_is_empty_when_there_is_no_requested_session` | `:31`'s `!sessionId` arm — **the arm the draft's one-line summary hides**; STRICT means a `None` session yields `[]`, it does **not** mean "no filter" | no module exists |
| `the_snapshot_excludes_jobs_from_other_sessions` | `:34`, second filter | no module exists |
| `the_snapshot_widget_prefix_is_unchanged` | `PI_SUBAGENT_ASYNC_JSON:` verbatim | no module exists |
| `the_snapshot_kind_and_version_are_unchanged` | `"pi-subagents.async-status-snapshot"` / `1` (`projection:8-9`) — same wire-constant argument, and **not** in the draft | no module exists |
| `the_snapshot_omits_absent_optionals_rather_than_serializing_null` | the `...(x ? {x} : {})` spread contract (§3.4) | no module exists |
| `the_snapshot_byte_limit_binary_searches_the_retained_runs` | `projection:369-385` — assert `omitted.byteLimitExceeded` **and** that the retained count is the maximum that fits, not merely fewer | no module exists |
| `the_snapshot_sorts_by_updated_at_desc_then_async_id` | `projection:549-553` | no module exists |
| `the_snapshot_normalizes_completed_and_pending_and_falls_back_to_partial` | `projection:173-178` | no module exists |
| `live_workflow_controls_require_both_session_comparisons` | S6 — a control whose session differs from the status's is excluded **even when the registry has the run and gate 1 passes** | `format_status` renders no steer lines at all |
| `live_workflow_controls_are_empty_without_a_registry_entry` | the `workflowControllers.has` arm (`:609`) | as above |
| `live_workflow_controls_exclude_children_with_no_active_children` | `activeChildren.size > 0` (`:612`) | as above |
| `live_workflow_controls_are_empty_when_the_state_session_differs_from_the_status` | gate 1 (`:609`) **on its own** — registry live, control matching, only gate 1 failing | as above |
| `live_workflow_controls_admit_an_unattributed_run_in_a_sessionless_host` | **§2's correction.** `current = None`, `status.session_id = None`, `control.session_id = None` ⇒ the hints ARE rendered. This is the test that fails if anyone reaches for `SessionGate::Strict`. | as above |
| `a_running_workflow_with_no_live_route_renders_the_unavailable_sentence` | `:685`'s exact string | as above |
| `live_workflow_child_steer_hints_are_emitted_in_ascending_index_order` | `:687-688`, one line per active-child index | as above |
| `a_non_running_workflow_renders_no_steer_lines` | `:684`'s `state === "running"` conjunct | as above |
| `a_foreground_transcript_of_a_foreign_run_is_refused` | `:249` STRICT | there is no live-foreground transcript path; the id resolves to `Err("Async run not found. Provide id or dir.")` |
| `a_foreground_transcript_with_no_session_is_refused` | `:249`'s `!currentSessionId` arm — the arm that distinguishes `Strict` from `Permissive` | as above |
| `the_transcript_control_list_is_session_filtered` | `:368` | the no-id transcript branch never consults `foreground_controls` (`status.rs:407-418`) |
| `the_no_id_transcript_prefers_the_most_recently_updated_live_control` | `:370`'s `updatedAt` DESC selection (with the `lastForegroundControlId` degradation of §5.1 recorded) | as above |
| `a_live_foreground_transcript_with_no_active_child_reports_it_rather_than_failing` | `:259`'s `Transcript unavailable: no active foreground child.` | as above |
| `a_live_foreground_transcript_requires_an_index_when_several_children_are_active` | `:260-262`, including the `Active child indexes: …` enumeration | as above |

### Benchmarks

None. Both are report-rendering paths. *(Unchanged from the draft, and re-confirmed: the byte-limit
binary search is bounded by `caps.maxRuns = 20`.)*

---

## [AUG] §7 — Definition of done

Checkable by reading code and running named tests.

1. **`background/async_status_snapshot/`** exists with the layout of §3.4 and exports
   `ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX`, `ASYNC_STATUS_SNAPSHOT_KIND`,
   `ASYNC_STATUS_SNAPSHOT_VERSION`, `build_async_status_snapshot`,
   `async_status_snapshot_jobs_for_state`, `build_async_status_snapshot_for_state`,
   `encode_async_status_snapshot_widget`. All three constants are byte-identical to upstream.
2. **Both `:31` and `:34` filters are present and separately tested.** `:31` is STRICT in all three
   of its arms (`!state`, `!sessionId`, mismatch). The absent `fleetJobs` loop is recorded as a
   `[CYRUP-DELTA]` **at the seam**, citing `tui/fleet_state.rs:487-489`.
3. **`enforce_byte_limit` is a binary search** and `omitted.byteLimitExceeded` is set before it runs.
4. **S6's four terms are all present in one expression** in `format_status`, with a doc comment
   stating why the two session comparisons use `Option` equality and **not** `SessionGate`
   (§2). Grep proof: `rg 'SessionGate' crates/cyrup-ext-subagents/src/background/run_status.rs`
   returns nothing.
5. **S6's consumer renders**: `Steer live foreground child: subagent({ action: "steer", id: "…",
   index: N, message: "..." })` one line per ascending active-child index, or
   `Steer: unavailable; no live foreground route is registered in the active session.` when the list
   is empty — gated on `mode == Workflow && state == Running`, positioned immediately before the
   `Workflow receipt:` push (`run_status.rs:363`).
6. **`:249` refuses via `SessionGate::Strict`** with upstream's verbatim sentence, returned as
   `Err(String)`; **`:368` filters** the control list and the no-id transcript branch consults it
   **before** the `list_active_runs` fallback.
7. Every unrepresentable upstream arm (`workflowGraph`, `hostSteps`, `fleetJobs`,
   `lastForegroundControlId`, `step.label`/`phase`) is recorded as a `[CYRUP-DELTA]` at its own
   seam, in the house style (`inspect_rpc/mod.rs:31-57` is the model), **not** silently dropped.
8. `cargo nextest run -p cyrup-ext-subagents` — every named test in §6 passes; workspace **0 failed**;
   `cargo clippy --workspace --all-targets` exit 0; `cargo fmt --check` clean.

---

## [AUG] §8 — Open questions the implementor must NOT silently decide

* **Q1 — the snapshot's consumer.** `encode_async_status_snapshot_widget` has no reachable caller in
  cyrup (§3.5). Options: (a) land the `pub` API with a module-doc paragraph in the style of
  `tui/events.rs:44-52`, naming `set_widget` as the missing capability; (b) additionally add
  `SubagentExecutor::async_status_snapshot(&self, cwd) -> AsyncStatusSnapshot` so the host has a
  real entry point. **Neither is obviously right; ask before choosing.** Do NOT append the encoded
  widget line to `control_status`'s text output — upstream emits it through `setWidget`, never into
  the status report, and doing so would corrupt the report for every reader.
* **Q2 — where S6's two session comparisons live.** Renderer (`run_status.rs`, needs a new `pub`
  deps struct on a public module) vs executor (`status.rs`, keeps `run_status.rs` a pure renderer
  but splits the four-term predicate across two files, which is exactly what the draft warns against
  with *"two session comparisons in one expression"*). §4.2 leans renderer; confirm.
* **Q3 — `hostStepSnapshotState`/`projectHostStep`.** `HostStepNode` exists
  (`workflows/scripted/engine.rs:71`) but is not on `RunStatus`, so the projector has no input today
  (§3.3). Port it unreachable-but-correct, or omit it with a recorded delta? The crate has precedent
  both ways (`inspect_rpc/mod.rs:57` ports a provably-dead branch; `workflow_controllers.rs:16-19`
  refuses to port `topLevelResume` until it has a caller).
* **Q4 — does SUBTASK3 include the `formatLiveForegroundTranscript` port, or only its gate?** §5.1
  shows the gate is unreachable without the function. If the answer is "gate only", the task must
  say what the gate guards, and the `a_foreground_transcript_*` tests cannot be written as
  behavioural tests. **This materially changes the size of the task** (S ↔ M).
* **Q5 — `lastForegroundControlId`.** Degrade `:369` to `:370`'s `updatedAt` sort (recommended, §5.1)
  or port the field and its five writers as part of this task?
* **Q6 — the public-API break.** `inspect_status_by_id`/`inspect_status_by_dir` are `pub async fn`
  on a `pub mod`. Adding a deps parameter breaks them for any out-of-crate caller. Add the parameter
  to the existing names (3 call sites to fix, one of them
  `cyrup-it/tests/subagents/background_runner_main_integration.rs:2207`), or add
  `*_with_deps` siblings and leave the originals delegating with `Default::default()`?

---

## [AUG] §9 — ORIGINAL BODY, VERBATIM (nothing removed)

Everything from here down is the pre-augmentation text, unchanged. Where §§0-8 contradict a line
below, §§0-8 record *why*; they do not supersede the requirement.

> **Depends on WORKFLOW_6** (`workflow_controllers`, `ForegroundControlEntry` fields). WORKFLOW_10 is *not*
> required for S6 itself, only for the adjacent `:487`/`:514` capacity reads.
>
> ## SUBTASK1 — `background/async_status_snapshot.rs`
>
> **Ports:** `pi-subagents/src/runs/background/async-status-snapshot.ts` (48 LOC)
>
> ```ts
> ASYNC_STATUS_SNAPSHOT_WIDGET_PREFIX = "PI_SUBAGENT_ASYNC_JSON:"                       // :24
> buildAsyncStatusSnapshot(jobs, options)                                                // :26
> asyncStatusSnapshotJobsForState(state, sessionId) {                                    // :30
>     if (!state || !sessionId || state.currentSessionId !== sessionId) return [];       // :31  STRICT
>     for (const job of …) if (job.sessionId === sessionId) jobs.set(job.asyncId, job);  // :34
> }
> buildAsyncStatusSnapshotForState(state, sessionId, options)                            // :42
> encodeAsyncStatusSnapshotWidget(jobs, options): string[]                               // :46
> ```
>
> **Two filters, not one:** `:31` gates on the *state* matching the requested session (STRICT — a
> mismatch or a missing session yields an empty snapshot), then `:34` filters *each job*. Port both.
>
> The widget prefix is a wire constant — cyrup's rebrand does **not** apply. Keep
> `PI_SUBAGENT_ASYNC_JSON:` verbatim unless a consumer in this repo demands otherwise; changing it
> silently breaks any reader keyed on it.
>
> **Layout:** one concern, one file.
>
> ## SUBTASK2 — S6, live workflow controls in the status report
>
> **Where:** `crates/cyrup-ext-subagents/src/background/run_status.rs`
> **Ports:** `pi-subagents/src/runs/background/run-status.ts:609-612`
>
> ```ts
> const liveWorkflowControls =
>     status.mode === "workflow"
>     && deps.state?.currentSessionId === status.sessionId          // gate 1
>     && deps.state?.workflowControllers?.has(status.runId)         // ← WORKFLOW_6 registry
>         ? [...deps.state.foregroundControls.values()].filter((control) =>
>               control.parentWorkflowRunId === status.runId        // ← WORKFLOW_6 field
>            && control.sessionId === status.sessionId              // gate 2 ← WORKFLOW_6 field
>            && (control.activeChildren?.size ?? 0) > 0)            // ← WORKFLOW_6 field
>         : [];
> ```
>
> **Two session comparisons in one expression**, against two different objects: the *state's* current
> session vs the status's, and each *control's* session vs the status's. They are not redundant — a
> control could carry a stale session after a rotation.
>
> ## SUBTASK3 — the remaining `run-status.ts` transcript gates
>
> **Where:** `background/run_status.rs`, `extension/executor/status.rs`
>
> The async transcript gate (`:494`/`:521`) landed already. Two remain:
>
> ```ts
> if (!state.currentSessionId || control.sessionId !== state.currentSessionId)  // :249  STRICT — THROWS
>     throw new Error(`Foreground run '${control.runId}' is not owned by the current session.`);
> const controls = [...deps.state.foregroundControls.values()]
>     .filter((control) => control.sessionId === currentSessionId);             // :368
> ```
>
> `:249` is the one gate upstream implements as a **throw** rather than a returned refusal, and it is
> **STRICT**. Port both the class and the shape.
>
> ## Tests
>
> | test | pins |
> |---|---|
> | `the_snapshot_is_empty_when_the_state_session_does_not_match` | `:31` STRICT, first filter |
> | `the_snapshot_excludes_jobs_from_other_sessions` | `:34`, second filter |
> | `the_snapshot_widget_prefix_is_unchanged` | the wire constant |
> | `live_workflow_controls_require_both_session_comparisons` | S6 — construct a control whose session differs from the status's and assert it is excluded even when the registry has the run |
> | `live_workflow_controls_are_empty_without_a_registry_entry` | the `workflowControllers.has` arm |
> | `live_workflow_controls_exclude_children_with_no_active_children` | `activeChildren.size > 0` |
> | `a_foreground_transcript_of_a_foreign_run_is_refused` | `:249` STRICT |
> | `a_foreground_transcript_with_no_session_is_refused` | `:249`'s `!currentSessionId` arm |
> | `the_transcript_control_list_is_session_filtered` | `:368` |
>
> ## Benchmarks
>
> None. Both are report-rendering paths.
>
> ## Definition of done
>
> - The async status snapshot applies both filters and keeps its wire prefix.
> - S6's live workflow controls apply **both** session comparisons plus the registry and
>   `activeChildren` predicates.
> - `:249` refuses (STRICT) and `:368` filters.
> - Tests pass; workspace 0 failed; clippy exit 0.
>
> ## Research notes
>
> * Upstream: `pi-subagents/src/runs/background/async-status-snapshot.ts`,
>   `pi-subagents/src/runs/background/run-status.ts:249`, `:368`, `:609-612`.
> * Already landed in `run_status.rs`: `list_active_runs`'s session filter (`:647`) — the model to
>   follow; its doc at `:586-595` already explains the drop-unattributed rule correctly.
> * Already landed in `extension/executor/status.rs`: the async transcript gate (`:494`/`:521`).
> * Requires WORKFLOW_6's `ForegroundControlEntry::{session_id, parent_workflow_run_id, active_children}`.
