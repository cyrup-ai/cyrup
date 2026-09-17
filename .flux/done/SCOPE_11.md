---
stage: qa
status: completed
updated: 2026-09-14 20:00
---

# SCOPE_11 — durable wait subscriptions

OBJECTIVE: port `background/wait-subscriptions.ts` (348 LOC) — `{id, nonBlocking: true}` wake
subscriptions that survive across turns. cyrup already has the in-process `CompletionBus` wired
into `wait.rs`, so this adds the **durable, cross-turn** half, not the wake mechanism.

**Depends on SCOPE_3** (`wait_completions`) and **SCOPE_4** (replay), which supply what a woken
subscription reads.

> **[AUG] Both dependencies LANDED in SCOPE batch 1 (PR #137).** The ground under this task is new
> and was read for this augmentation, not assumed:
> * `background/wait_completions/` — `mod.rs` (60), `collect.rs` (412), `project.rs` (711),
>   `record.rs` (549). The session-index rung is `collect.rs:56-83`.
> * `background/completion_replay/` — `mod.rs` (163), `record.rs` (301), `store.rs` (419),
>   `archive.rs` (548), `retention.rs` (376).
>
> Anything below marked **[AUG]** was verified by opening the file named. Nothing in the original
> body has been softened or removed.

---

## [AUG] §0 — Upstream pin, and resolving the 348-vs-253 discrepancy

The task's *Research notes* name HEAD `7fe9dee1`. That commit **exists** in the clone at
`/home/user/cyrup/tmp/pi-subagents` and is the correct pin:

```
git -C /home/user/cyrup/tmp/pi-subagents describe --tags 7fe9dee1   -> v0.65.1-50-g7fe9dee1
git -C /home/user/cyrup/tmp/pi-subagents show 7fe9dee1:src/runs/background/wait-subscriptions.ts | wc -l  -> 348
git -C /home/user/cyrup/tmp/pi-subagents show v0.41.0:src/runs/background/wait-subscriptions.ts  | wc -l  -> 253
```

`PARITY-GAPS.md` records BOTH figures because they are two different shapes of the same file at two
different times:

| figure | where in `PARITY-GAPS.md` | what it is |
|---|---|---|
| **253** | `:1218` (row **VL-S8**, tagged *"(v0.35.0/v0.41.0)"*) | the file **as it stood at `v0.41.0`**, the tag that refresh was written against |
| **348** | `:55` (the SUBA-031 "still open" residual list, SECOND REFRESH 2026-09-04) | the file **as it stands now** |

**Resolution: port the 348-LOC shape at `7fe9dee1`.** It is stable across the whole tag range
`v0.60.0 … v0.67.0` (each `git show <tag>:…| wc -l` = 348 — checked), so "348" is the current
format, not one commit's accident. The 253-LOC v0.41.0 shape predates `FOREIGN_SWEEP_*`, the
`unresolvedRestoredForegroundTokens` set and the replay read at `:251`; **do not port it.**

Every `:NN` in the ORIGINAL body below was re-verified line-for-line against `7fe9dee1` and **all
nine are correct**. They are quoted in §1.

`git -C … show <pin>:<path>` was the only read used. No working tree, no unpinned HEAD, no
line numbers from a floating ref.

---

## SUBTASK1 — `background/wait_subscriptions/`

**Ports:** `pi-subagents/src/runs/background/wait-subscriptions.ts`

Surface:

```ts
interface ArmWaitSubscriptionInput          // :39
interface WaitSubscriptionManager           // :46
formatWaitSubscriptions(state, now): string | undefined   // :89
createWaitSubscriptionManager(…)            // :99
```

Five session gates:

```ts
|| typeof record.sessionId !== "string"                       // :72   invalid record — no session
if (record.sessionId === state.currentSessionId) continue;    // :157  skip own-session
if (record.sessionId !== state.currentSessionId) return;      // :206  act only on own-session
if (!run || run.sessionId !== record.sessionId) { … }         // :215  run/record agreement
if (record.sessionId === state.currentSessionId) { … }        // :327
```

`:157` and `:206` are **opposite** comparisons on the same field in the same file — one skips the
current session, the other requires it. Read both in context before implementing; they are
different phases (enumeration vs delivery), not a contradiction to be "simplified".

`:215` re-verifies the run's session against the record's, the same contents-decide discipline the
result index uses for hashed addresses.

**Layout:** `wait_subscriptions/{mod,record,manager,format}.rs` — the persisted record, the
manager's lifecycle, and the rendering are three jobs.

### [AUG] §1 — the five gates, quoted from `7fe9dee1`, each with the phase it lives in

The draft's one-line labels are correct but under-specify **which function** each gate is in. An
implementor who ports them without that will put `:157`'s `continue` in the wrong loop. Verbatim:

| # | line | enclosing function | exact source |
|---|---|---|---|
| 1 | `:72` | `parseRecord(value)` (`:66-79`) | `\|\| typeof record.sessionId !== "string"` — one disjunct of an **eight-term** rejection chain (`:69-77`) |
| 2 | `:157` | `sweepExpiredForeignSubscriptions(force)` (`:134-168`) | `if (record.sessionId === state.currentSessionId) continue;` |
| 3 | `:206` | `reconcileRecord(record)` (`:205-259`) | `if (record.sessionId !== state.currentSessionId) return;` — the FIRST statement |
| 4 | `:215` | `reconcileRecord`, **foreground branch only** (`:211-229`) | `if (!run \|\| run.sessionId !== record.sessionId) { settle(record, "could not be reconciled", …); return; }` |
| 5 | `:327` | `restore()` (`:309-336`) | `if (record.sessionId === state.currentSessionId) { subscriptions.set(record.token, record); … }` |

**[AUG] The draft's gloss on `:157` is imprecise and the test name inherits the imprecision.**
`:157` is **not** general "enumeration". It is the FOREIGN-RECORD SWEEP, and its own upstream
comment (`:155-156`) says exactly why it skips the current session:

> *"The owning session keeps its own expired records so `reconcileRecord()` can still settle them
> with the 'timed out' notice callers expect."*

So the rule is: **the sweep deletes only OTHER sessions' expired records, and only
`FOREIGN_SWEEP_GRACE_MS` past expiry** (`:158`). The test `enumeration_skips_the_current_session`
stays in the table (it is not being dropped), but it must be WRITTEN as *"the foreign sweep does
not unlink a record belonging to the current session, even when that record is expired"* — see §7.

**[AUG] `settle` carries a SIXTH session comparison the draft does not list.** `:181`:

```ts
if (disposed || state.currentSessionId !== record.sessionId) return;
```

It is redundant with `:206` on the `reconcileRecord` path but is NOT redundant overall: `settle` is
also reachable from `reconcile`'s catch arm (`:272`), which runs after a `reconcileRecord` **throw**
— i.e. after a point where the session could have been swapped. Port it. Do not collapse it into
`:206` on the grounds that the callers already checked; upstream has both, deliberately.

### [AUG] §2 — the constants, and the two that are format

```ts
const SUBSCRIPTION_VERSION      = 1;              // :22  ON-DISK FORMAT
const RECONCILE_INTERVAL_MS     = 1_000;          // :23
const FOREIGN_SWEEP_INTERVAL_MS = 60_000;         // :29  throttle, not a timer
const FOREIGN_SWEEP_GRACE_MS    = 24*60*60*1000;  // :37  one day
```

`:24-28` and `:30-36` are load-bearing doc comments; carry their reasoning into the Rust doc
comments the way `completion_replay/mod.rs:67-80` carries `completion-replay.ts:7-12`.

`SUBSCRIPTION_VERSION` is a **format** constant. Spell it as a unit type, not a `u32` field — this
crate already has two precedents and they agree:
* `background/completion_replay/record.rs:56` — `pub struct ReplayVersion;`
* `extension/executor/foreground_history/record.rs:41` — `pub(crate) struct HistoryVersion;` with
  `Serialize`/`Deserialize` hand-impls at `:48-66` that make a version-2 file a **parse failure**.

Copy the `HistoryVersion` shape: a record carrying any other value must fail to deserialize rather
than require every reader to remember a check.

### [AUG] §3 — the record type, field-for-field

Upstream `WaitSubscriptionRecord` (`shared/types.ts:2189-2198` @ `7fe9dee1`):

```ts
export interface WaitSubscriptionRecord {
	version: 1;
	token: string;
	sessionId: string;
	targetKind: "async" | "foreground";
	runId: string;
	requestedId: string;
	createdAt: number;
	expiresAt: number;
}
```

The cyrup shape, with the **real** local types (each verified):

```rust
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitSubscriptionRecord {
    pub version: SubscriptionVersion,        // unit type; see §2
    pub token: SubscriptionToken,            // newtype; see below
    pub session_id: crate::identity::SessionId,   // identity/session_id.rs:41 — `SessionId(Arc<str>)`
    pub target_kind: WaitTargetKind,         // new enum { Async, Foreground }
    pub run_id: crate::background::RunId,    // background/run_id.rs:38-59 — `RunId(Arc<str>)`
    pub requested_id: String,                // the RAW prefix the caller typed; never a RunId
    pub created_at: i64,                     // crate::time::now_epoch_millis (time.rs:18)
    pub expires_at: i64,
}
```

Four typing decisions the implementor must not rediscover:

1. **`session_id: SessionId`, required, NOT `Option<String>`.** This is gate `:72` enforced by the
   TYPE — a record with no session cannot be constructed and a JSON file with a null/empty session
   fails to deserialize. Exactly the argument
   `foreground_history/record.rs:128-134` already makes for `ForegroundHistoryRun::session_id`:
   *"A run with no session cannot be CONSTRUCTED, so it cannot be persisted."* `SessionId::parse`
   (`identity/session_id.rs:47-53`) rejects the empty string and nothing else, which IS pi's
   `typeof … === "string"` plus its falsy check.
2. **`requested_id: String`, not `RunId`.** Upstream keeps the caller's prefix separately from the
   resolved exact id (`:42` `requestedId` vs `:41` `runId`) and only ever renders it. Collapsing
   them loses the audit of what the user actually asked for.
3. **`token: SubscriptionToken`** — a newtype over a **hyphenated** UUIDv4. Upstream validates the
   token in `parseRecord` (`:71`) against
   `/^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i` and mints it with
   `randomUUID()` (`:296`). ⚠ **`RunId::new` is NOT the right minter**: `background/run_id.rs:39`
   uses `uuid::Uuid::new_v4().as_simple()` — 32 hex chars, **no hyphens** — which that regex
   rejects. Mint with `uuid::Uuid::new_v4().hyphenated()`. The `uuid` crate is already a dependency
   of `cyrup-ext-subagents` (`run_id.rs:39` proves it) and `hyphenated()` is inherent, no feature.
   Make `parse` the only constructor and have it apply the regex's equivalent, so a hand-edited
   file cannot smuggle a token that does not match its own filename.
4. **`target_kind: WaitTargetKind`**, a two-variant enum with
   `#[serde(rename_all = "lowercase")]` so the wire words stay `"async"` / `"foreground"` — pi's
   `:73` disjunct becomes an unrepresentable state instead of a check.

### [AUG] §4 — where the records live on disk, and why the draft's obvious port is WRONG

Upstream (`:106`):

```ts
const subscriptionsDir = options.subscriptionsDir ?? path.join(path.dirname(asyncDirRoot), "wait-subscriptions");
```

For pi that resolves to `<TEMP_ROOT_DIR>/wait-subscriptions`, because pi's async root is
`ASYNC_DIR = path.join(TEMP_ROOT_DIR, "async-subagent-runs")` (`shared/types.ts:2733`) — one flat,
non-cwd-keyed directory, so `dirname` is the temp root.

**cyrup's layout is different and a literal `dirname(async_root)` is a bug.**
`background/artifact_roots.rs:301-308`:

```rust
pub fn run_artifact_roots_in(roots: &crate::paths::Roots, cwd: &Path) -> RunArtifactRoots {
    let scratch = roots.run_scratch();
    let key = cwd_key(cwd);
    RunArtifactRoots {
        async_root:  scratch.join(ASYNC_SUBDIR).join(&key),   // ASYNC_SUBDIR   = "async"   (:27)
        results_dir: scratch.join(RESULTS_SUBDIR).join(&key), // RESULTS_SUBDIR = "results" (:35)
    }
}
```

So `dirname(async_root)` is `<run_scratch>/async` — **shared by every cwd**, and one level too
high. Two mechanisms actually work; pick one and write the choice into the module doc:

* **(A) a fourth sibling, cwd-keyed** — `<run_scratch>/wait-subscriptions/<cwd_key>`, mirroring
  `ASYNC_SUBDIR`/`RESULTS_SUBDIR`/`SCRATCH_SUBDIR` (`artifact_roots.rs:27,35,47`). This preserves
  cyrup's stated layering, which `background/wait.rs:87-99` states outright: *"The cwd partition
  … is still the outer one; the session filter is the inner one, exactly as upstream layers them."*
* **(B) under the results dir** — `<results_dir>/wait-subscriptions/`, the way
  `completion_replay` puts `completion-replay/` and `output-archives/` under `results_dir`
  (`completion_replay/mod.rs:21-23, 100-112`).

(A) is the closer structural analog of upstream's *sibling-of-async-root*; (B) needs no new
`Roots` plumbing and reuses the dir the manager already holds for the replay read. **Recorded in
`unresolvedQuestions`** — do not decide it silently.

Per-file naming: `<token>.json`, upstream `subscriptionFile` (`:85-87`). Upstream then **re-checks
the name against the parsed token** at BOTH read sites (`:154` and `:326`):
`if (!record || file !== \`${record.token}.json\`) continue;`. Port that check; it is the cheapest
possible defence against a record being addressed by a name it does not claim.

Write with `crate::background::atomic::write_atomic_json` (`background/atomic.rs:75-100`) — upstream
uses `writeAtomicJson` (`:305`) and this is its direct analog (temp file in the same parent, then
`rename`, with bounded backoff).

### [AUG] §5 — the manager: the Rust surface SCOPE_8/9/10 and the wait tool will compile against

⚠ **Layering constraint, discovered by opening the files.** `background/` sits **below**
`extension/executor/`. `ForegroundControlEntry` is `pub(crate)` and defined in
`extension/executor/notices.rs:20-99`; `ForegroundHistoryRun` is `pub(crate)` in
`extension/executor/foreground_history/record.rs:124-141`. A `background/wait_subscriptions/`
module **cannot name either type**. Upstream reaches straight into `state.foregroundRuns`
(`:212`) because pi has one flat `SubagentState`; cyrup does not. The manager therefore takes
**injected dependencies**, exactly as `WaitDeps` already does for the bus, the store and the ledger
(`background/wait.rs:242,259,285`).

Proposed public surface — this is the contract, state it in the module doc:

```rust
// background/wait_subscriptions/mod.rs
pub use format::format_wait_subscriptions;
pub use manager::{
    ForegroundSubscriptionProbe, ForegroundTargetState, SubscriptionNotifier,
    SubscriptionOutcome, WaitSubscriptionDeps, WaitSubscriptionManager,
};
pub use record::{
    ArmWaitSubscriptionInput, SubscriptionToken, SubscriptionVersion, WaitSubscriptionRecord,
    WaitTargetKind,
};

pub const SUBSCRIPTION_VERSION: u32 = 1;                       // pi :22
pub const RECONCILE_INTERVAL: Duration = Duration::from_secs(1);        // pi :23
pub const FOREIGN_SWEEP_INTERVAL: Duration = Duration::from_secs(60);   // pi :29
pub const FOREIGN_SWEEP_GRACE_MS: i64 = 24 * 60 * 60 * 1000;            // pi :37
```

```rust
/// pi `ArmWaitSubscriptionInput` (:39-44).
pub struct ArmWaitSubscriptionInput {
    pub target_kind: WaitTargetKind,
    pub run_id: RunId,
    pub requested_id: String,
    pub timeout_ms: u64,
}

/// pi `WaitSubscriptionManager` (:46-51). `arm` is the only method the wait tool needs — pass it
/// as `Pick<WaitSubscriptionManager, "arm">` does upstream (`wait-tool.ts:12`).
impl WaitSubscriptionManager {
    /// pi `arm` (:290-308). Errors when there is no live session — pi's
    /// `throw new Error("A wait subscription requires an active session identity.")` (:292).
    pub async fn arm(&self, input: ArmWaitSubscriptionInput)
        -> Result<WaitSubscriptionRecord, SubagentError>;
    /// pi `restore` (:309-336). Clears, force-sweeps, re-reads own-session records, reconciles.
    pub async fn restore(&self);
    /// pi `reconcile` (:261-275). Never propagates: every failure settles or is logged.
    pub async fn reconcile(&self);
    /// pi `dispose` (:338-346).
    pub fn dispose(&self);
    /// The current-session records, for `format_wait_subscriptions`. Session-scoped BY
    /// CONSTRUCTION — `restore`'s :327 gate is the only writer besides `arm`.
    pub fn armed(&self) -> Vec<WaitSubscriptionRecord>;
}
```

The two injected seams, and the exact upstream they stand in for:

```rust
/// Stands in for pi's `pi.sendMessage({...}, { triggerTurn: true })` (:189-199).
#[async_trait::async_trait]
pub trait SubscriptionNotifier: Send + Sync {
    async fn notify(&self, record: &WaitSubscriptionRecord, outcome: SubscriptionOutcome,
                    detail: &str, completion: Option<&WaitCompletion>) -> bool;
}

/// Stands in for pi's `state.foregroundRuns?.get(record.runId)` (:212) — see §6.3 for why this
/// cannot be a plain map lookup in cyrup.
pub trait ForegroundSubscriptionProbe: Send + Sync {
    fn probe(&self, run_id: &RunId) -> Option<ForegroundTargetState>;
}

/// The ONLY facts :211-228 actually reads off a foreground run, lifted out of the two
/// executor-private types that hold them.
pub struct ForegroundTargetState {
    pub session_id: Option<SessionId>,
    /// One entry per child. `status`/`activity_state`/`current_tool` are pi's `child.status`,
    /// `child.activityState`, `child.currentTool`.
    pub children: Vec<ForegroundTargetChild>,
}
```

`SubscriptionOutcome` is an enum, not a `&str`, for the same reason `WaitVerdict`
(`background/wait.rs:538-575`) is: every one of upstream's six outcome strings is a distinct
`settle` site and a reader's `match` must be forced to decide.

| upstream string | line | meaning |
|---|---|---|
| `"timed out"` | `:208` | `now() >= record.expiresAt` |
| `"could not be reconciled"` | `:216`, `:241` | run/record disagreement (fg) or exact run gone (async) |
| `"needs attention"` | `:221`, `:245` | supervisor request / `needsAttention(run)` |
| `"completed"` / `"failed"` | `:226`, `:257` | the two terminal foreground arms |
| `run.state` verbatim | `:257` | async non-`complete` terminal states (`failed`/`stopped`/`paused`) |
| `"reconciliation failed"` | `:272` | `reconcileRecord` threw |

### [AUG] §6 — what CANNOT work as drafted, and the mechanism that can

Each item is recorded here, **next to** the requirement it qualifies. None of them removes scope.

#### 6.1 The async branch cannot use `list_active_runs` — it is hard-filtered to active states

Upstream `:231-243`:

```ts
const runs = listAsyncRuns(asyncDirRoot, { sessionId: record.sessionId, runId: record.runId,
                                           exactRunId: true, resultsDir, kill: options.kill, now });
const run = runs.find((candidate) => candidate.id === record.runId);
if (!run) { settle(record, "could not be reconciled", …); return; }
```

Note there is **no `states:` option** — upstream deliberately wants the run back even when it is
terminal, because `:248` (`run.state !== "queued" && run.state !== "running"`) is the whole point.

cyrup's `list_active_runs` (`background/run_status.rs:669-738`) **cannot** serve this. `:724`:

```rust
if matches!(status.state, RunState::Queued | RunState::Running) {
    runs.push(ActiveRun { dir: paths.run_dir.clone(), status });
}
```

A terminal run is simply absent, so a naive port would settle **every** completed run as
`"could not be reconciled"` — the exact opposite of the feature.

**The mechanism that works:** `run_status::reconcile_by_id` (`run_status.rs:540-549`):

```rust
pub async fn reconcile_by_id(async_root: &Path, results_dir: &Path, selector: &str)
    -> Result<Option<(RunStatus, RunPaths)>, SubagentError>
```

It performs the same R-SA-079 reconciliation gate, returns the full `RunStatus` in **any** state,
and maps "neither file exists" to `Ok(None)` (`reconcile_paths`, `:577-583`) — which is upstream's
`if (!run)`. Two follow-through obligations:

* **`exactRunId: true` must be honoured.** `reconcile_by_id` resolves a **prefix** through
  `resolve_run_id` (`:497-506`, which returns `SubagentError::AmbiguousRunId` for >1 match).
  The record stores an exact `runId`, so after resolution assert
  `status.run_id == record.run_id` and settle `"could not be reconciled"` otherwise. A prefix that
  now matches a *different* run must never satisfy this subscription.
* **`sessionId` must be re-applied.** `reconcile_by_id` applies **no** session filter;
  `list_active_runs` applies it at `:719-723`. Upstream gets the filter for free by passing
  `sessionId` into `listAsyncRuns`. Port it explicitly:
  `if status.session_id.as_ref() != Some(&record.session_id) { settle("could not be reconciled") }`.
  **This is the async-branch twin of gate `:215`** and it is why the draft's phrasing
  ("`:215` re-verifies the run's session against the record's") is right in spirit for both
  branches even though upstream only spells it out for the foreground one.

#### 6.2 `needsAttention` has a step-level disjunct that cyrup's existing helper drops

Upstream `:81-83`:

```ts
function needsAttention(run: AsyncRunSummary): boolean {
	return run.activityState === "needs_attention"
	    || run.steps.some((step) => step.activityState === "needs_attention");
}
```

cyrup's existing helper, `background/wait.rs:407-409`, is **run-level only**:

```rust
fn needs_attention(run: &ActiveRun) -> bool {
    run.status.telemetry.activity_state == Some(ActivityState::NeedsAttention)
}
```

The step disjunct **is** portable — `StepTelemetry::activity_state` exists at
`background/telemetry.rs:119` (`pub activity_state: Option<ActivityState>`, doc'd as
*"pi `step.activityState`"*), reachable as `status.steps[i].telemetry.activity_state`
(`records.rs:142` gives `StepStatus::telemetry: StepTelemetry`).

And it is **not redundant**: `RunStatus::sync_top_level_telemetry` (`records.rs:398-425`) rolls up
`current_tool`, `tool_count`, `turn_count`, `total_tokens` and `last_activity_at` — and **not**
`activity_state`. A step can be `NeedsAttention` while `RunTelemetry::activity_state`
(`telemetry.rs:164`) is `None`.

**Do not reuse `wait.rs:407`.** Write the two-disjunct predicate in `wait_subscriptions/` and cite
`:81-83`. Whether `wait.rs:407` should also gain the disjunct is a SEPARATE defect, **out of scope
here** — recorded in `unresolvedQuestions`, not fixed.

#### 6.3 The foreground branch: pi's one map is two disjoint maps in cyrup, and one of them lacks the fields

Upstream `:211-228` reads, off `state.foregroundRuns.get(record.runId)`:
`run.sessionId`, `run.children[].status` (`"detached"` / `"failed"`),
`child.activityState === "needs_attention"`, `child.currentTool === "contact_supervisor"`.

cyrup has **two** maps and they are **disjoint by construction**
(`extension/executor/mod.rs:188-192`: *"an entry here is created when a run SETTLES, and
`foreground_controls`' entry for the SAME id is removed at that exact point"*):

| cyrup map | type | has `session_id`? | has `current_activity_state` / `current_tool`? |
|---|---|---|---|
| `foreground_controls: HashMap<String, ForegroundControlEntry>` (`mod.rs:146`) | `notices.rs:20-99` — **LIVE** runs | yes, `Option<SessionId>` (`:68`) | **yes** — `:34` and `:44`; per child in `active_children: BTreeMap<usize, ForegroundChildEntry>` (`:95-98`) → `foreground_control.rs:202-203` |
| `foreground_runs: HashMap<RunId, ForegroundHistoryRun>` (`mod.rs:197-204`) — pi's literal `state.foregroundRuns` | `foreground_history/record.rs:124-141` — **SETTLED** runs | yes, `SessionId` **required** (`:134`) | **NO** — `ForegroundHistoryChild` (`:80-119`) carries `status`, `updated_at`, `context`, `model`, `thinking`, `session_file`, `transcript_path`, `saved_output_path`, `artifact_output_path`, `error`, `output_save_error`, `transcript_error`, `final_output`, `tokens`, `tool_count` — **no `activity_state`, no `current_tool`** |

So the literal port ("read `state.foregroundRuns`") gives a map that **cannot express `:220`'s
`contact_supervisor` test at all**, and reading only `foreground_controls` gives a map that a
SETTLED run has already left — which is `:224`'s `detached.length === 0` case, i.e. the case that
must settle `"completed"`/`"failed"`.

**The mechanism that works:** the injected `ForegroundSubscriptionProbe` (§5) is implemented **in
`extension/executor/`**, where both maps are in scope, and consults them in this order:

1. `foreground_controls` first — a LIVE run. Its `active_children` supply `current_activity_state`
   and `current_tool`, so `:220`'s supervisor test is real. A live run has at least one
   non-settled child, which is upstream's `detached.length > 0` state.
2. `foreground_runs` second — a SETTLED run. Its `ForegroundHistoryChild::status` supplies
   `"detached"` / `"failed"` / `"completed"` / `"stopped"` (minted by
   `foreground_history_child_status`, `record.rs:167-177`), which is exactly what `:224-227` needs.
   `activity_state`/`current_tool` are `None` here, so `:220` simply cannot fire — correct, because
   a settled run has no tool in flight.
3. Neither → `probe` returns `None` → `:215`'s `!run` arm → `"could not be reconciled"`.

**[CYRUP-DELTA] to write into the module doc:** a subscription armed against a foreground run whose
supervisor request arrives *after* the run settles is not detectable, because the field that would
report it is not persisted. Upstream has the same hole for a different reason (`RESTORABLE`,
`foreground_history/record.rs:27-31`, deliberately never persists `"detached"`), so this is a
narrowing of an existing upstream boundary, not a new one.

#### 6.4 `unresolvedRestoredForegroundTokens` — the one-shot grace the draft never mentions

`:110`, `:213-214`, `:311`, `:329`. On `restore()`, a foreground record whose run is **not** in the
map is added to `unresolvedRestoredForegroundTokens` (`:329`). `reconcileRecord` then **returns
early without settling** while the run is still missing (`:213`) and drops the token the moment the
run appears (`:214`).

Without it, every foreground subscription restored before the foreground registry is repopulated
immediately settles `"could not be reconciled"` — i.e. `a_subscription_survives_a_turn_boundary`
fails for foreground targets. **Port it.** cyrup's ordering makes it necessary for the same reason:
`restore_foreground_run_history_for` runs at `native_impl.rs:362-364`, and whichever order the
manager's `restore()` is placed in relative to it, this grace is what makes the ordering
non-load-bearing.

#### 6.5 `settle` REMOVES before it notifies — so the notifier must be ACKED

Upstream `:180-203`: `remove(record)` (`:183`, which unlinks the file and drops the map entry),
and only then `pi.sendMessage(…, { triggerTurn: true })` (`:189`). If the send fails it logs
*"Failed to deliver wait subscription '…' after clearing it"* (`:201`) — pi accepts the loss
because its `sendMessage` is a synchronous in-process call that essentially cannot fail.

cyrup's send is **not** in-process. The seam is `cyrup_ext::host::HostServices`
(`crates/cyrup-ext/src/host/services.rs`):

```rust
fn inject_message(&self, content: &str, custom_type: Option<&str>, display: bool,
                  details: Option<&Value>, trigger_turn: bool) -> Result<(), String>;      // :499
fn inject_message_ack(&self, /* same five */ )
    -> Result<tokio::sync::oneshot::Receiver<InjectOutcome>, String>;                       // :530
```

`InjectOutcome` (`:35-48`) is `Accepted` | `SessionUnavailable`, and `Accepted`'s own doc says it
is *"the ONLY licence for the caller to destroy its own copy of the data."*

**Use `inject_message_ack`, not `inject_message`.** `inject_message`'s own doc warns *"the host
took it" is not "the session has it"*, and `HostServicesCompletionSink`
(`background/watch/sink.rs:61-97`) already took the ack path for precisely this reason. Then either:

* **(a)** reorder — await `Accepted` first, and remove only after (deviates from `:183`-before-`:189`
  but preserves the user-visible guarantee), or
* **(b)** keep upstream's order and, on `SessionUnavailable`, **re-arm** the record by rewriting the
  file, so the next `reconcile` re-delivers.

**Recorded in `unresolvedQuestions`.** Do not pick silently; whichever is chosen must be written
into the module doc with this paragraph's reasoning.

Message shape, from `:190-198` — port these strings verbatim (modulo the tool rename):

```
customType : "subagent-wait-subscription"
content    : `Wait subscription ${token} fired for run ${runId}: ${outcome}. ${detail}`
display    : true
details    : { token, runId, outcome, ...(completion ? { completions: [completion] } : {}) }
triggerTurn: true
```

#### 6.6 `formatResumeFirstFailedRunDetail` is OWED BY THIS TASK — the crate says so

`:255`:

```ts
const detail = formatResumeFirstFailedRunDetail(run) ?? "Inspect the run status for its final output.";
```

`background/resume_guidance.rs:23-29` records the debt explicitly:

> *"`formatResumeFirstFailedRunDetail` → `wait-subscriptions.ts:183` ONLY. That module … is unported
> in cyrup and tracked as PARITY-GAPS **VL-S8**; it is the *only* call site upstream. Porting the
> formatter now would add a `pub fn` with no caller in this crate … **It is therefore owed BY
> VL-S8** — whoever lands wait subscriptions writes it there, over
> [`format_async_revive_command`], which is the only part of it that carries real logic."*

(The `:183` in that note is the v0.41.0 line; at `7fe9dee1` the call is at `:255`. Same call, one
site, unchanged.) Build it over `format_async_revive_command` (`resume_guidance.rs:63`), which is
already ported. Its sibling `format_resume_first_failed_runs_note` (`:99`) is the plural form and is
already wired into `wait` — do not confuse them.

#### 6.7 The reconcile timer: no `setInterval().unref()` in Rust

`:286-287` — `const interval = setInterval(reconcile, 1000); interval.unref?.();`. cyrup's
equivalent shape already exists twice and the implementation should follow it:

* `background/watch/install.rs:29-55` — `CompletionWatcherHandle` with an `impl Drop` (`:37`) that
  aborts the drain task. `notices.rs:480-484` documents why re-install-overwrites-and-drops is
  cyrup's structural equivalent of pi's `deliveryEpoch` lease.
* `extension/executor/notices.rs:633-637` — `spawn_retention_sweep(...) -> tokio::task::JoinHandle<()>`,
  whose caller aborts the previous handle (`:496-498`).

So: `dispose()` sets the disposed flag, aborts the `JoinHandle`, and clears the map (`:338-346`);
`Drop` on the handle does the same so a forgotten `dispose` cannot leak the task. `.unref()` has no
analog and needs none — a `tokio` task does not hold the runtime open the way a libuv timer holds
the loop.

#### 6.8 `sweepExpiredForeignSubscriptions` must NEVER throw (`:132`)

Every `fs` call in `:134-168` is wrapped: `readdirSync` → log-unless-ENOENT and `return` (`:141-144`),
`readFileSync`/`JSON.parse` → log and `continue` (`:150-153`), `unlinkSync` → log-unless-ENOENT
(`:161-166`, with the explicit comment that losing the race to another session's sweep is harmless).
The crate already has the ENOENT predicate: `background::result_index::errno::is_absent`
(used at `completion_replay/store.rs:142`, `wait_completions/collect.rs:101`). Use it; do not
hand-roll `ErrorKind::NotFound` comparisons.

---

## SUBTASK2 — arm from the wait tool's non-blocking form

**Where:** `background/wait.rs` and the wait tool surface

**Change:** `{id, nonBlocking: true}` arms a subscription and returns immediately instead of
blocking. The subscription fires when the run completes — including in a later turn, and including
when the payload has already been cleaned up (resolving through SCOPE_4's replay record).

**Why:** `PARITY-GAPS.md` records this as part of the `subagent_wait` gap: *"there is no
`{id, nonBlocking:true}` wake subscription"*.

### [AUG] §7 — the five edits SUBTASK2 actually is

**(a) `WaitParams` gains the field.** `background/wait.rs:206-220` today:

```rust
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WaitParams {
    #[serde(skip_serializing_if = "Option::is_none")] pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub all: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub timeout_ms: Option<u64>,
}
```

Add `pub non_blocking: Option<bool>` (serialises as `nonBlocking`, matching upstream's
`SubagentWaitParams.nonBlocking`, `extension/schemas.ts:407-409`).

**(b) The JSON Schema must be edited in the SAME change.** `extension/wait_tool.rs:52-72`'s
`wait_tool_parameters()` ends with `"additionalProperties": false` (`:70`). A caller passing
`nonBlocking` against the un-edited schema is rejected by the host before `serde` ever sees it.
Note also that `WaitParams` carries `#[serde(default)]` and **not** `deny_unknown_fields`, so
`serde` would silently DROP the key — schema-only or struct-only is a silent no-op either way.
Update the description text too; upstream's bullet is `wait-tool.ts:22`.

**(c) Two rejections, before any listing.** Upstream `:561-566` of `subagent-wait.ts`:

```ts
if (params.nonBlocking && !params.id)  return result("Non-blocking wait subscriptions require id so the registration can bind one exact run identity.", true);
if (params.nonBlocking && params.all)  return result("nonBlocking cannot be combined with all; subscribe to one exact run id.", true);
```

**(d) The arming site is AFTER prefix resolution and AFTER ambiguity rejection.** Upstream
`:590-600`, which sits between `matches.length > 1` (`:586`) and the foreground/async dispatch
(`:601`). cyrup's structurally identical point is `background/wait.rs:785-807` — after the
`AmbiguousId` return at `:794-804` and after `effective_id` is narrowed at `:806`. Arm there,
using the **resolved exact** id (`run_id_of(run)`), with `requested_id = params.id`.

Upstream also refuses when there is no manager (`:591-593`):

> *"Non-blocking wait subscriptions require a long-lived interactive subagent runtime; this runtime
> can only use blocking bg_wait calls."*

— which is the `ctx?.hasUI` gate at `wait-tool.ts:33`
(`...(subscriptions && ctx?.hasUI ? { subscribe: … } : {})`). cyrup's analog is
`HostCtx::has_ui` (`crates/cyrup-ext/src/native.rs:131`, `facade.rs:119`), already used at
`native_impl.rs:376,402,465`. Thread it: `WaitDeps::subscribe: Option<Arc<dyn …>>`, `None` when
`!has_ui` or when no manager exists — mirroring `completion_bus: Option<…>`'s own no-bus
degradation doc (`wait.rs:238-241`).

**(e) `WaitVerdict` gains a variant — and this is deliberately a breaking change.**
`WaitOutcome::is_error` (`wait.rs:637-650`) has **no `_ =>` arm**, and `WaitVerdict`'s own doc
(`:536-537`) says why:

> *"Every variant names one upstream `return` site. Adding one forces every reader's `match` to
> decide, which a `bool` never does."*

Add `WaitVerdict::SubscriptionArmed { token: SubscriptionToken }` (upstream's `:596` return is a
plain non-error `result(text)`), and let the compiler find every match. Success text, verbatim from
`:596`:

```
Armed wait subscription {token} for exact {kind} run {runId}. Returning immediately; this session
will be woken on completion, failure, attention, reconciliation failure, or timeout. Inspect armed
subscriptions with subagent({ action: "status" }).
```

**(f) The manager is owned by the executor, and lives beside its three siblings.**
`extension/executor/mod.rs:427` (`completion_bus`), `:435` (`wait_completions`), `:446`
(`inline_answers`) are the pattern — each is a clone-shares-one-instance accessor whose doc says
*"must outlive any single watcher install — the watcher is REPLACED on every `SessionStart`."*
Same for this. Lifecycle, mapping upstream's four call sites:

| upstream | cyrup |
|---|---|
| `createWaitSubscriptionManager(pi, state)` — `extension/index.ts:493`, once at load | `SubagentExecutor::new` (`extension/executor/mod.rs:276-…`) |
| `registerWaitTool(…, waitSubscriptionManager, …)` — `:781` | `WaitTool::execute` (`extension/wait_tool.rs:134-165`), gated on `has_ui` |
| `waitSubscriptionManager.restore()` — `:971` (session_start) | `HostEvent::SessionStart` arm, `extension/host/native_impl.rs:298-377` |
| `waitSubscriptionManager.dispose()` — `:1009` | `HostEvent::SessionShutdown` arm, `native_impl.rs:467-491` |

**(g) The wake edge.** `background/watch/observer.rs:38-45` already names this task:

> *"pi's `SUBAGENT_ASYNC_COMPLETE_EVENT` … `extension/index.ts:648-659` @v0.43.0 registers three
> listeners on the one event; **`wait-subscriptions.ts` adds a fourth**."*

The composite is assembled at `extension/executor/notices.rs:452-472`, today with exactly three
members in a documented order (`wait_completions()` FIRST — `:454-460` — then
`MissionSyncCompletionObserver`, then `completion_bus`). Register the manager's reconcile as a
**fourth member, LAST**, so the `WaitCompletionStore` record is already written when the
subscription reconciles. Upstream subscribes to six channels (`:277-284`) — of which cyrup has one
real edge (the result-file observation) plus the 1 s timer; say so as a `[CYRUP-DELTA]` rather than
inventing five channels.

### [AUG] §8 — how a fired subscription reads a cleaned-up payload (the SCOPE_4 dependency)

Upstream `:249-257`:

```ts
completion = readCompletionReplay(resultsDir, record.runId, { sessionId: record.sessionId, now: now() })?.completion;
…
const archiveDetail = completion?.archivePath ? ` Completion archive: ${completion.archivePath}.` : "";
settle(record, run.state === "complete" ? "completed" : run.state, `${detail}${archiveDetail}`, completion);
```

Landed cyrup analog — `background/completion_replay/store.rs:133-171`:

```rust
pub async fn read_completion_replay(results_dir: &Path, run_id: &RunId,
                                    filter: ReplayReadFilter<'_>) -> Option<CompletionReplayRecord>
pub struct ReplayReadFilter<'a> { pub session_id: Option<&'a SessionId>, pub now: Option<i64> }
```

Four behaviours to know before calling it (from its own doc table, `store.rs:106-118`):

* absent / unparseable / unknown version → `None`, **no delete**;
* `validate_replay_record` rejected → `None`, **deletes the record**;
* `session_id` filter mismatch → `None`, **no delete** (a foreign record is not this caller's to reap);
* `expires_at <= now` → `None`, **deletes record AND archive**.

Pass `session_id: Some(&record.session_id)` — the record always has one (§3), unlike
`collect.rs:123` which passes `run.session_id.as_ref()` including `None`.
`CompletionReplayRecord::completion` is a `WaitCompletion` (`record.rs:41`) and
`WaitCompletion::archive_path` is `Option<String>` — `store.rs:309` already notes it *"must
actually be populated (`wait-subscriptions.ts:255`)"*, i.e. it was populated **for this task**.

Do **not** re-implement rung-3 resolution: `collect_wait_completions`
(`wait_completions/collect.rs:41-140`) is the three-rung reader and its third rung (`:119-131`) is
this exact call. Prefer routing through it when the shape fits; call
`read_completion_replay` directly only for the single-run case, and say which in the doc.

---

## SUBTASK3 — render armed subscriptions

**Where:** the status/fleet reporting surface

**Change:** `formatWaitSubscriptions` (`:89`) renders what is currently armed. Session-scoped by
construction — a subscription belonging to another session is never rendered here.

### [AUG] §9 — the exact render, and the exact seam

Upstream `:89-97`, verbatim:

```ts
export function formatWaitSubscriptions(state: Pick<SubagentState, "waitSubscriptions">, now = Date.now()): string | undefined {
	const subscriptions = [...(state.waitSubscriptions?.values() ?? [])].sort((left, right) => left.createdAt - right.createdAt);
	if (subscriptions.length === 0) return undefined;
	const lines = [`Armed wait subscriptions (${subscriptions.length}):`];
	for (const record of subscriptions) {
		lines.push(`- ${record.token}: ${record.targetKind} run ${record.runId}, timeout in ${Math.max(0, record.expiresAt - now)}ms`);
	}
	return lines.join("\n");
}
```

Three details a paraphrase loses: sort is by **`createdAt` ascending**; empty is **`undefined`**,
never `""` (so the join below omits it entirely); the remaining-time clamp is
`Math.max(0, expiresAt - now)` and the unit suffix is a bare `ms`, **not**
`format_duration` (`wait.rs:394`). Signature: `-> Option<String>`.

The **only** consumer is `run-status.ts:390-393`:

```ts
const waitSubscriptions = deps.state ? formatWaitSubscriptions(deps.state, deps.now?.() ?? Date.now()) : undefined;
return { content: [{ type: "text", text: [formatAsyncRunList(runs), waitSubscriptions].filter(Boolean).join("\n\n") }], details: { mode: "single", results: [] } };
```

cyrup's identical point is **`extension/executor/status.rs:382-384`**, inside
`control_status_view`'s no-id, non-transcript branch:

```rust
let runs = run_status::list_active_runs(&async_root, &results_dir, self.current_session_id().as_deref()).await…;
if !transcript {
    return Ok(run_status::format_run_list(&runs));      // <- pi's formatAsyncRunList(runs)
}
```

Join with `"\n\n"` and drop the `None`. **Only this branch** — not the by-id form (`:402`), not the
by-dir form (`:406`), not the fleet view (`:357`); upstream renders it in exactly one place.

**The "session-scoped by construction" claim is TRUE, and here is exactly why** (pin it, per §7's
test): the formatter reads `state.waitSubscriptions`, whose only two writers are `arm` (`:306`,
which stamps `state.currentSessionId` at `:291-292`) and `restore` (`:328`, behind gate `:327`).
The foreign sweep never inserts. So no filtering happens *in* the formatter and none is needed —
but that is a property of `restore`, and a regression there is invisible at the formatter. Test
both ends.

`SCOPE_10` also edits the status surface but **does not touch this seam** — its SUBTASK2 is S6 /
`run-status.ts:609-612` (live workflow controls) and its SUBTASK1 is `async-status-snapshot.ts`.
No collision.

---

## Tests

| test | pins |
|---|---|
| `a_non_blocking_wait_arms_a_subscription_and_returns_immediately` | the feature |
| `an_armed_subscription_fires_when_the_run_completes` | the wake |
| `an_armed_subscription_fires_after_cleanup_via_the_replay_record` | the SCOPE_4 dependency — the hard case |
| `a_record_with_no_session_id_is_invalid` | `:72` |
| `enumeration_skips_the_current_session` | `:157` |
| `delivery_requires_the_current_session` | `:206` — the opposite comparison, asserted separately |
| `a_record_whose_run_reports_a_different_session_is_dropped` | `:215` contents-decide |
| `format_renders_only_this_sessions_subscriptions` | `:89` |
| `a_subscription_survives_a_turn_boundary` | the durability claim, not just the in-memory bus |

The third and last are the two that would silently pass against the existing in-memory bus if the
durable half were skipped — write them so they fail without it.

### [AUG] §10 — how each test must be written to actually fail before / pass after

All nine above are **kept**. Each row gains a construction note; the six additions below are new
coverage the draft's table does not reach, not replacements.

* `a_non_blocking_wait_arms_a_subscription_and_returns_immediately` — drive
  `wait_for_subagents` against a run that is and stays `Running`, with a
  `timeout_ms` far larger than the test's own budget. Assert the call returns **well under** the
  poll interval, the verdict is `WaitVerdict::SubscriptionArmed { .. }`, and a `<token>.json`
  exists on disk. Returning-fast alone is not enough: without the on-disk assertion this passes
  against a stub that just returns.
* `an_armed_subscription_fires_when_the_run_completes` — arm, then write a terminal
  `ResultFile`/`status.json`, then call `reconcile()` **directly** (do not sleep on the 1 s timer).
  Assert the notifier captured one message whose `outcome` is `Completed`, and that the record file
  is **gone**.
* `an_armed_subscription_fires_after_cleanup_via_the_replay_record` — **this one must be built to
  fail without rung 3.** Sequence: arm → `write_completion_replay`
  (`completion_replay/store.rs`, `CompletionReplayWrite { results_dir, run_id, session_id,
  completion, data, now, ttl_ms }`) → **unlink the payload AND leave no in-process
  `WaitCompletionStore` entry** (construct the manager with a fresh/empty store, so rungs 1-2
  cannot answer) → `reconcile()`. Assert the delivered `details.completions[0]` carries the
  `archive_path` from the replay record. If the store is shared with the writer, this passes
  vacuously against rung 1 — that is the silent pass the draft warns about.
* `a_record_with_no_session_id_is_invalid` — write `{"version":1,…,"sessionId":""}` and
  `{"version":1,…}` with the key absent; assert both fail to **deserialize** (§3 makes this a
  type-level property), and that `restore()` loads neither.
* `enumeration_skips_the_current_session` — ⚠ see §1. Write it as the **foreign sweep**: two
  records, both with `expires_at` more than `FOREIGN_SWEEP_GRACE_MS` in the past, one for session A
  (current) and one for session B. Force-sweep. Assert **B's file is unlinked and A's is not**.
  A test that merely checks A is absent from some list passes against a no-op sweep.
* `delivery_requires_the_current_session` — a record for session B, manager running as session A.
  Call `reconcile()`. Assert the notifier captured **nothing** and B's file still exists (`:206`
  returns before the timeout branch, so an expired foreign record is not settled either — that is
  the whole reason the foreign sweep exists).
* `a_record_whose_run_reports_a_different_session_is_dropped` — **write it for BOTH branches**
  (§6.1): foreground, where the probe returns a `ForegroundTargetState` whose `session_id` differs
  → `:215`; and async, where `reconcile_by_id` returns a `RunStatus` whose `session_id` differs →
  the async twin. Both settle `CouldNotBeReconciled`. One-branch coverage leaves the branch the
  draft never spelled out untested.
* `format_renders_only_this_sessions_subscriptions` — assert **both** ends per §9: (i) the exact
  string, heading `Armed wait subscriptions (N):`, rows ordered by `created_at` ascending, suffix
  `, timeout in {ms}ms`, and `0ms` for an already-expired record (the `max(0, …)` clamp);
  (ii) that `restore()` over a directory holding a foreign record leaves it **out** of `armed()`.
* `a_subscription_survives_a_turn_boundary` — **must cross a manager instance.** Arm with manager
  #1 against a temp dir; `dispose()` it; build manager #2 over the **same directory** with the same
  session id **and a fresh, empty `WaitCompletionStore`**; `restore()`; then terminalise and
  `reconcile()`. Assert it fires. Reusing one manager tests the in-memory map, which is exactly the
  silent pass the draft warns about.

**[AUG] Six more, for the mechanisms §6 introduced:**

* `a_subscription_file_whose_name_disagrees_with_its_token_is_ignored` — `:154` / `:326`.
* `a_foreign_record_within_the_grace_window_is_not_swept` — `:158`; the complement of
  `enumeration_skips_the_current_session`, and the thing that makes the grace non-vacuous.
* `a_restored_foreground_record_whose_run_is_absent_is_not_settled_on_the_first_pass` — §6.4
  (`:213`, `:329`). Then assert it **does** settle once the probe reports the run.
* `an_async_run_that_is_terminal_is_found_and_settled` — §6.1. Build it so it fails against a
  `list_active_runs`-based implementation: terminalise the run **before** the first `reconcile()`.
* `a_step_level_needs_attention_settles_the_subscription` — §6.2. Set only
  `steps[0].telemetry.activity_state = NeedsAttention`, leave `RunStatus::telemetry.activity_state`
  at `None`. Fails against a run-level-only predicate.
* `arming_without_a_session_identity_is_an_error` — `:291-292`.

**Test placement:** inline `#[cfg(test)] mod tests` per file, matching
`completion_replay/mod.rs:114-162` and every sibling in `background/`. The crate has no `tests/`
directory. Head each module with the same
`#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]`
block `completion_replay/mod.rs:116-121` uses — the workspace denies all four
(`Cargo.toml:101-102`).

---

## Benchmarks

None. Subscriptions are armed per user request and read on completion events.

---

## Definition of done

- `{id, nonBlocking: true}` arms a durable subscription and returns immediately.
- An armed subscription fires across a turn boundary and after payload cleanup.
- All five session gates ported with their upstream comparisons intact — including the two that
  point opposite ways.
- Tests pass; workspace 0 failed; clippy exit 0.

### [AUG] §11 — added acceptance conditions (nothing above is relaxed)

- The **sixth** session comparison, `settle`'s `:181`, is ported too (§1).
- `format_resume_first_failed_run_detail` exists in `background/resume_guidance.rs`, built over
  `format_async_revive_command` (`:63`), and `resume_guidance.rs:23-29`'s debt note is updated to
  say it is now paid (§6.6). ⚠ That module doc also states *"That module … is unported in cyrup"* —
  that sentence becomes stale on landing and must be corrected in the same change.
- `PARITY-GAPS.md` **VL-S8** (`:1218-1219`) is narrowed, not closed: the subscription half is done;
  the tool RENAME (`wait` → `subagent_wait`) and auto-drain are **not in this task's scope** and
  the row must still say so. Do not close the row.
- `background/watch/observer.rs:40`'s *"`wait-subscriptions.ts` adds a fourth"* and
  `extension/executor/notices.rs:449`'s twin are now satisfied by a real fourth member (§7g); both
  comments get updated to name it.
- `cargo fmt -p cyrup-ext-subagents` only. `cargo fmt --all` is a repo-wide no-op today and must
  stay one.
- Every `[CYRUP-DELTA]` named in §6 (6.2's unported step disjunct in `wait.rs:407`, 6.3's
  unpersisted foreground attention, 6.5's chosen ack ordering, 6.7's timer shape, §7g's one wake
  channel versus upstream's six) is written into the module doc at the seam it applies to, in the
  style `background/wait.rs:44-55` and `background/watch/observer.rs:139-146` already use.

---

## Research notes

* Upstream: `pi-subagents/src/runs/background/wait-subscriptions.ts` (HEAD `7fe9dee1`).
* Existing wake mechanism to build on, not replace: `background/wait.rs:206` (`WaitDeps::completion_bus`),
  `background/watch/observer.rs` (`CompletionBus`, already a `CompletionObserver` returning `bool`).
* `wait.rs:30-50`'s module docs explain the two-part wake shape and the 500 ms latency floor; extend
  rather than contradict them.
* `PARITY-GAPS.md` `SUBA-034` (event-bus wake) is already closed; this is the subscription half.

### [AUG] §12 — corrections to the research notes above (the notes are kept; these qualify them)

* **`WaitDeps::completion_bus` is at `background/wait.rs:242`, not `:206`.** `wait.rs` is 2901
  lines and was restructured by SCOPE batch 1; the `:206` citation predates it. The subscription
  is taken at `:747-750`, and the ordering argument (*"subscribe BEFORE the first listing, never
  after"*) is at `:741-746`.
* The `CompletionObserver` trait is `background/watch/observer.rs:22-36`; `CompletionBus` is
  `:150-191` and its `CompletionObserver` impl `:193-…`. `CompletionEvent` (`:84-106`) carries
  `run_id`, `outcome: ClassifiedOutcome` and SCOPE_17's `summary: String`.
* `wait.rs:30-50` is now **`wait.rs:33-54`** (the `# Wake mechanism (SUBA-034)` heading is at `:33`;
  the latency-floor paragraph at `:44-52`; `completion_bus: None` at `:54`). Two further sections
  landed after the note was written and are directly relevant: `# Payload resolution` (`:56-74`)
  and `# Resolving a completion after cleanup (SUBA-056)` (`:76-85`).
* **Stale in `PARITY-GAPS.md` VL-S8** (`:1219`): it cites `extension.rs:6704` for
  `WAIT_TOOL_NAME`. `extension.rs` no longer exists as a monolith; the constant is
  `extension/wait_tool.rs:16`. Correct the citation when narrowing the row.
* **Stale in the scheduling brief, not in this file:** SCOPE_8 / SCOPE_9 / SCOPE_10 were read and
  **none declares a dependency on SCOPE_11**. Each depends on WORKFLOW_6 (`SCOPE_8.md:57`,
  `SCOPE_9.md:30`, `SCOPE_10.md:30`); the `SCOPE_11`…`SCOPE_17` mention at each file's `:15` is a
  **programme grouping** ("multi-instance session-partitioning"), not a runway. This task's public
  surface should still be treated as load-bearing — it is the fourth composite-observer member and
  a new `WaitVerdict` variant — but it does **not** block those three.
* Upstream's retention reaper reads this directory: `async-retention.ts:307-316`'s
  `parseWaitRunIds` protects a run referenced by a live subscription from being reaped, re-read
  before each destructive action (`:751`, `:794`, `:818`, `:861`, `:878`), and skips the whole pass
  as `"wait-references-unknown"` when any record is unparseable (`:752-755`). **cyrup has no
  `async-retention.ts` port** (`PARITY-GAPS.md:55` lists it, 912 LOC, still open), so there is
  nothing to wire this into and it is **OUT OF SCOPE here**. Recorded so the next reader does not
  re-derive it as a missing invariant, and so whoever ports the reaper knows the coupling exists.
