---
stage: qa
status: completed
updated: 2026-09-15 18:00
---

# SCOPE_16 — scheduled runs, part B: trigger loop and tool surface

OBJECTIVE: complete `scheduled-runs.ts` — fire schedules when due, and expose the tool actions that
create, list and cancel them. This turns SCOPE_15's store on.

**Depends on SCOPE_15**, and on **WORKFLOW_10** (a fired schedule spawns an async run, which must claim
a per-session capacity slot).

---

## 0. AUGMENTATION PREFACE — read this before the subtasks

This file was a 96-line intent draft. Every citation below was re-opened in the tree on
2026-09-15 at branch `claude/subagents-scope-3` (cut from `main` @ `b2fdc7e`, after PR #134, PR #137
and PR #139 landed). Upstream was read **only** via
`git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:<path>` — never from a working tree, never
from an unpinned HEAD.

**Upstream pin: `v0.67.0`** (annotated tag; `git rev-parse HEAD` = `13f8f3286c7ce42d0aecfd72bca76b26912b1445`).
`git show v0.67.0:src/runs/background/scheduled-runs.ts | wc -l` = **1006**.

The draft (and SCOPE_15) cite `HEAD 7fe9dee1`. That commit is real (`git cat-file -t 7fe9dee1` →
`commit`) and **is an ancestor of `v0.67.0`** (`git merge-base --is-ancestor 7fe9dee1 v0.67.0`
succeeds); the file is **979** lines there, which is where SCOPE_15's "979 LOC" and both files'
`:98` / `:466-470` / `:619-620` line numbers come from. **Every line number in this file is
`v0.67.0`**, matching the pin SCOPE_13/SCOPE_14 already standardised on. The `7fe9dee1` → `v0.67.0`
shifts that matter are recorded in §0.2.

`git diff v0.67.0 HEAD -- src/runs/background/scheduled-runs.ts` is **not** empty (11 insertions):
post-`v0.67.0` upstream added `args: Record<string, unknown>` to `ScheduleTarget` and
`normalizeWorkflowArgs`/`deepFreezeWorkflowArgs` to `parseScheduleTarget`. **Out of scope. Do not
port it**; it belongs with whatever task lands `workflow-resources.ts`.

> **AMENDED 2026-09-15, after re-verifying against `v0.68.0` (latest).** The scope call above is
> kept — the *behaviour* (`normalizeWorkflowArgs` / `deepFreezeWorkflowArgs`) stays out of this
> task. But the split is now explicit rather than implied: **SCOPE_15 carries the `args` FIELD** on
> the persisted record (see `SCOPE_15.md` §3.1), because it lands on disk and adding it later is a
> migration rather than an edit. So this task will find `args` already present on
> `ScheduleTarget`, defaulted to `{}` and round-tripped, and must neither remove it nor act on it.
> Part A reserves the shape; part B gives it meaning, in a later task.
>
> Everything else this task ports is unchanged at `v0.68.0` — the trigger loop, the store layout
> and the tool surface all match what is described below.

### 0.1 Is this already implemented? **No.**

`grep -rn "scheduled_runs\|ScheduledRun" crates/ --include=*.rs` returns **zero hits**.
`ls crates/cyrup-ext-subagents/src/background/` shows no `scheduled_runs` directory.
`alreadyImplemented = false`.

What *does* exist already, and must be wired rather than rebuilt (each verified, §6 has the
citations):

* `registration/authority.rs:73` already maps `"schedule.create"` → `AuthorityAction::ScheduleCreate`,
  with the explicit note at `:66` that it is mapped *"even though cyrup does not dispatch it yet
  (SUBA-016)"*. SCOPE_16 is the change that gives it a dispatch.
* `extension/tool/text.rs:299-312`'s `DESTRUCTIVE_MANAGEMENT_ACTIONS` already carries
  `"schedule.delete"` verbatim, deliberately ahead of its dispatch.
* `registration/tool_description.rs:713-726`'s
  `the_compact_description_advertises_no_verb_cyrup_cannot_dispatch` is a standing guard that flips
  meaning the day these verbs land. See §0.3 D9 — it cannot be satisfied the way its own doc says.

### 0.2 NINE claims in the draft that are FALSE or incomplete against the code

Each is restated **in place** in the subtask it belongs to, next to the mechanism that actually
works. Nothing is dropped — every *objective* is preserved; only the *mechanism* changes.

| # | Draft claim | What the code says |
|---|---|---|
| **D1** | *"`scheduled_runs/trigger.rs` — the loop that finds due schedules and fires them"* / *"The trigger loop wakes on an interval and does nothing when no schedule is due"* | **Upstream has no polling loop.** Firing is per-schedule `setTimeout` armed by `arm()` (`:788-804`) and re-armed on every create (`:657`), resume (`:686`), restore (`:785`), fire-skip (`:830`), launch outcome (`:847`, `:867`, `:884`, `:897`) and finish (`:928`). The only due-**scan** is `runDue()` (`:712-723`), reached **only** from the user-invoked `schedule.run-due` action (`:554`). The draft's interval loop is a cyrup design choice, not a port. §1.2 keeps the objective and names the mechanism. |
| **D2** | *"A fired schedule spawns a background run through the existing async spawn path (`extension/executor/background.rs`)"* | Wrong twice. **(a)** Upstream's schedule target is `workflowScript`-**only**: `sanitizeTarget` (`:436-451`) refuses `tasks`/`chain`/`agent`/`task`, and `parseScheduleTarget` (`:295`) calls an `agent`/`task` target a *"removed legacy agent target"*. **(b)** cyrup **cannot** run a `workflowScript` asynchronously: `routing.rs:556-561` refuses `async: true` with a **permanent** refusal, and `background/state.rs:25-28` records that `RunMode::Workflow` is *"Produced exclusively by the FOREGROUND arm … the background runner never emits this mode (§0.6: async detachment was cut)"*. `spawn_background` takes a `BackgroundSingleRequest` (`background.rs:46-48`) — one agent + one task — and has no script shape at all. §1.3 gives the mechanism that works. |
| **D3** | *"it acquires a **per-session capacity slot** (WORKFLOW_10) — a schedule cannot exceed its session's cap"* | **WORKFLOW_10 does not exist in this build and has no task file.** `grep -rn "active_async_capacity" crates/` → **zero hits**; `ls .flux/todo \| grep WORKFLOW_1` → only 17/18/19/20/21. `extension/executor/workflow_controllers.rs:16-19` states it outright: *"its one real consumer (WORKFLOW_10's per-session capacity gate) **does not exist in this build**"*. Upstream `scheduled-runs.ts` has no capacity notion either. §5 keeps the objective against the gate that *does* exist. |
| **D4** | *"Listing is session-scoped for **display** … the answer is filtered by the session that created each entry where one is recorded"* | Upstream's `list()` (`:661-665`) filters **nothing**. It sorts by `trigger.nextRunAt` and renders a `session-only`/`project` **column**. The session predicate that exists — `scheduleBelongsToSession` (`:500-505`) — gates **execution** (`:694`, `:715`, `:748`, `:827`), is keyed on **`ownerSessionFile`**, not `sessionId`, and only bites when `sessionOnly === true`. §3.4. |
| **D5** | *"`scheduled_runs/tool.rs` — the tool actions: create / list / cancel"* | **Nine** actions (`SCHEDULED_RUN_ACTIONS`, `:19-29`): `schedule.create`, `.list`, `.show`, `.history`, `.pause`, `.resume`, `.run`, `.run-due`, `.delete`. There is no `cancel`; the verb is `schedule.delete`, which is why cyrup's `DESTRUCTIVE_MANAGEMENT_ACTIONS` already spells it that way. §3. |
| **D6** | *"Use SCOPE_14's shared staggered scheduler rather than a fourth ad-hoc `tokio::spawn`"* | `spawn_retention_sweep` (`extension/executor/notices.rs:648-682`) is a **one-shot** `sleep(delay)`-then-sweep task; its own doc (`:617-627`) says *"delayed and detached, rather than at install or **on an interval**"*. It cannot host a recurring trigger. SCOPE_14's own augmentation (§3.1) confirms the change there is a third **stage** inside that one-shot task, not a general scheduler. There is also no *fourth* task to avoid: today there is exactly **one** (`spawn_retention_sweep`) plus the wait-subscription reconcile timer. §4.2 names the precedent that does fit. |
| **D7** | *"`:466-470` (the pinned-session proxy)"*, and *"pins `getSessionId` to a captured value, so a schedule firing mid-session-change sees one consistent identity **for its whole execution**"* | At `v0.67.0` `snapshotContext` is `:466-479`. `getSessionId()` is read at **`:468`** and pinned at **`:472`**; **`getSessionFile()` is read at `:469` and pinned at `:473`** — the draft names only the id, and the *file* is the one the session-ownership predicate actually reads (`:503`). The pin's scope is the **project binding** (`selectProject` → `this.contexts.set(root, snapshotContext(ctx, projectCwd))`, `:956`), refreshed on every `selectProject`, **not** one fire. §2. |
| **D8** | *"**Depends on** … **WORKFLOW_10**"* | See D3 — there is no such task in `.flux/todo/`, `.flux/backlog/` or `.flux/done/`. The real dependency edge is **SCOPE_15** and **SCOPE_14** only. `dependsOn` in the structured return reflects that; the *requirement* D3 was serving is preserved in §5. |
| **D9** | (carried, not in the draft, but it will break the build) *"if a later sweep lands `scheduledRuns` (SUBA-016), **restoring that line** is what makes this test keep passing with it back in"* — `registration/tool_description.rs:713-716` | The line it means to restore is `@v0.34.0`'s *"• Opt-in schedule actions: schedule, schedule-list, schedule-status, schedule-cancel."* **That vocabulary no longer exists upstream.** At `v0.67.0`, `src/extension/tool-description.ts:43` names `schedule.*` inside the management-discovery bullet and adds *"Schedules take script inputs, not direct children"*. §4.5. |

### 0.3 Two further citation drifts (`7fe9dee1` → `v0.67.0`)

Both are SCOPE_15's citations, restated here because SCOPE_16's tool surface calls straight into
them and an implementor cross-reading the two files must not be sent to the wrong line:

* `scheduledRunStorePath` is **`:99-103`** at `v0.67.0` (SCOPE_15 says `:98`). The `_sessionId`
  parameter and its underscore are exactly as SCOPE_15 describes — confirmed verbatim.
* The capability-ceiling gate is **`:623-624`** at `v0.67.0` (SCOPE_15 says `:619-620`).
  `const sessionId = ctx.sessionManager.getSessionId() ?? "unknown";` is `:623`; the refusal
  `Cannot persist a schedule while a capability ceiling is active.` is `:624`. The `?? "unknown"`
  and the exact string are confirmed. `resolveCapabilityCeiling` in the deps type is **`:87`**
  (SCOPE_15 says `:86`).

---

## 0.4 WHAT THIS TASK REQUIRES OF SCOPE_15 (part A) — the contract

SCOPE_15 is being augmented in parallel. Stated as a hard interface contract; if part A lands
differently, SCOPE_16's first commit is the adapter, not a rewrite.

**R15-1 — module path.** `crates/cyrup-ext-subagents/src/background/scheduled_runs/{mod,store,schedule}.rs`,
declared in `background/mod.rs`'s sorted `pub mod` block. Alphabetically it lands between
`runner_main` (`background/mod.rs:48`) and `spawn_detached` (`:49`); note `background/mod.rs` has
**two** `pub mod` blocks (`:36-51` and `:69-79`, split by the `DEFAULT_ASYNC_CHILD_TIMEOUT_MS`
const at `:67`) — either is defensible, the first matches `result_index`/`completion_replay`.
SCOPE_16 adds `trigger.rs` and `tool.rs` to the same directory.

**R15-2 — the record types, as owned Rust with `serde`.** SCOPE_16 reads and writes every field:

```rust
pub enum ScheduleTrigger {                                    // pi :40-42
    Once     { at: String, next_run_at: Option<String> },
    Interval { every: String, every_ms: i64, anchor_at: String, next_run_at: String },
}
pub struct ScheduleTarget { pub workflow_script: String, pub base_ref: Option<String> } // pi :43 @v0.67.0
pub struct ScheduleRecord {                                   // pi :45-63
    // schema_version: 1 (version-tolerant per SCOPE_15 SUBTASK2)
    pub id: String, pub name: String, pub cwd: PathBuf,
    pub trigger: ScheduleTrigger, pub target: ScheduleTarget,
    pub overlap: /* "skip" only */, pub catch_up: ScheduleCatchUp /* None | Latest */,
    pub timeout_ms: Option<u64>, pub paused: bool,
    pub session_only: Option<bool>, pub quiet: Option<bool>,
    pub owner_session_file: Option<PathBuf>,
    pub created_at: String, pub updated_at: String,
    pub active_run_id: Option<String>, pub last_run_id: Option<String>,
}
pub struct ScheduleRunRecord {                                // pi :65-77
    pub id: String, pub schedule_id: String, pub planned_at: String,
    pub due_reason: ScheduleDueReason /* Timer | RunDue | Manual */,
    pub state: ScheduleRunState /* Running|Skipped|Missed|Completed|FailedLaunch|FailedRun */,
    pub started_at: Option<String>, pub completed_at: Option<String>,
    pub async_id: Option<String>, pub async_dir: Option<PathBuf>, pub error: Option<String>,
}
```

⚠ **`active_run_id` / `last_run_id` are `ScheduleRunRecord` ids, NOT run ids.** Upstream sets
`schedule.activeRunId = run.id` at `:870` where `run.id` is `this.randomId()` from `:837`. The
`RunId` of the spawned run lives in `ScheduleRunRecord::async_id` (`:880`). Conflating them makes
`remove()`'s active-run guard (`:728-736`) look up the wrong record. Keep them `String`, not
`crate::background::RunId`.

⚠ **Timestamps are ISO-8601 strings on disk, not epoch integers.** `timestamp(value)` (`:145-147`)
is `new Date(value).toISOString()`, and `nextRunAt()` (`:403-409`) parses back with `Date.parse`,
throwing `Schedule '<id>' has invalid nextRunAt.` on a non-finite result. SCOPE_16's trigger
arithmetic is entirely in `i64` epoch millis (`crate::time::now_epoch_millis()`, `time.rs:18`) and
converts only at the record boundary. Part A owns the two converters; if it did not land them,
SCOPE_16 adds them in `schedule.rs`, not in `trigger.rs`.

**R15-3 — the store.** `ScheduleStore` (pi `:317-385`) with `root: PathBuf` and
`project_cwd: Option<PathBuf>`, exposing at minimum `ids()`, `list()`, `get(id)`, `find(id)`,
`write(&record)`, `delete(id)`, `history(id)`, `write_run(schedule, run, event)`,
`append_event(schedule, event)`. SCOPE_16 calls all nine. `write`/`write_run` go through
`crate::background::atomic` (`write_atomic_json`, `atomic.rs:75`, async; or
`write_private_atomic_json_blocking`, `atomic.rs:209` — upstream's `writePrivateAtomicJson` is the
0600 form, so **the private one is the port**). The root is
`crate::artifacts::project_subagents_dir(cwd).join("schedules")` (`artifacts.rs:155`) when no
`storeRoot` is configured — pi `:100`.

**R15-4 — `SCHEDULE_ID` validation is part A's and is reused verbatim.** pi `:35`
`/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/`, enforced by `validateScheduleId` (`:149-152`) with the
string *"Schedule id must be 1-64 characters and contain only letters, numbers, '.', '_', or '-'."*.
`ids()` (`:333-335`) re-filters directory entries through the SAME regex, so a hand-created
directory with a bad name is invisible rather than fatal. SCOPE_16's `schedule.create` calls it on
both a caller-supplied `id` and a generated one (`:628`).

**R15-5 — the capability-ceiling gate is part A's, and SCOPE_16 supplies its caller.** Part A owns
the predicate wired to `exec/capability_ceiling.rs`; SCOPE_16 owns the `schedule.create` call site
(pi `:623-624`) and must pass `self.executor.current_session_id().unwrap_or_else(|| "unknown".into())`
— see §3.3 for why the literal fallback is load-bearing, and
`exec/capability_ceiling.rs:408` (`resolve_capability_ceiling(session_id: Option<&str>, inherited)`)
for the shape it resolves against.

**R15-6 — what part A must NOT have done.** No `tokio::spawn`, no timer, no tool registration. If
part A armed anything, SCOPE_16 removes it rather than adding a second driver.

---

## SUBTASK1 — `scheduled_runs/trigger.rs`

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"The loop that finds due schedules and fires them.
> A fired schedule spawns a background run through the existing async spawn path
> (`extension/executor/background.rs`), which means: it acquires a **per-session capacity slot**
> (WORKFLOW_10) — a schedule cannot exceed its session's cap; the spawned run carries `session_id`
> and `completion_owner_id` like any other, so its result is delivered to the right instance and no
> other. **Which session does a fired schedule belong to?** The session that is live when it fires,
> not the one that created it — the store is cwd-keyed precisely because schedules outlive sessions.
> Make this explicit in the docs; it is the question a reader will have, and getting it wrong either
> strands results (attributing to a dead session) or misdelivers them."*

The last sentence is **correct and confirmed** — it is the one part of this subtask the draft got
exactly right, and §1.5 pins it. The first three are D1/D2/D3; §1.2, §1.3 and §5 restate them.

### 1.1 The state machine `trigger.rs` owns, verbatim from `v0.67.0`

This is the part the draft reduced to "the loop", and it is 90% of the module. Port all of it.

| fn | pi | What it does |
|---|---|---|
| `next_run_at(schedule)` | `:403-409` | Parse `trigger.nextRunAt`; **throws** `Schedule '<id>' has invalid nextRunAt.` on a non-finite parse. Not `Option` — an unparseable stamp is an error, not "no next run". |
| `next_after(trigger, planned, now)` | `:396-401` | `None` for `once`; for `interval`, `planned + every_ms`, advanced by whole periods **while `next <= now`**. The `while`, not a single add: a process asleep for six intervals must land in the future, not six fires behind. |
| `due_planned_at(schedule, now)` | `:411-415` | The **catch-up** rule. Returns `next` unchanged unless (`next <= now` AND `catchUp == "latest"` AND `trigger.kind == "interval"`), in which case it returns `next + floor((now-next)/every_ms) * every_ms` — i.e. the **most recent** missed occurrence, so `catchUp: "latest"` fires **once**, at the latest slot, never a backlog. |
| `has_pending_schedule_work(s)` | `:392-394` | `active_run_id.is_some() \|\| trigger.next_run_at.is_some()`. Only used by `create`'s `maxPending` count (`:625`). |
| `arm(schedule, store, not_before)` | `:788-804` | Clear this schedule's timer; return if paused or `next_run_at` is `None`; else schedule `fire` at `min(max(0, next-now, not_before-now), MAX_TIMER_DELAY_MS)`. |
| `fire(store, id)` | `:819-832` | Clear timer → **`store.find(id)`, return silently if gone** → session-ownership check → `due_planned_at`; return if `None` or paused; **re-`arm` and return if `planned > now`**; else `launch(..., "timer", advance=true)`. |
| `launch(store, s, planned, due_reason, advance, quiet)` | `:834-900` | The whole overlap + lock + spawn + failure-rollback body. §1.4. |
| `finish_run(store, s, run, success, error)` | `:902-929` | Settle a run, back-fill a `skipped` record for an occurrence that came due **while the run was in flight** (`:905-918`), clear `active_run_id`, remove `active.lock`, re-`arm`. |
| `record_missed(store, s, planned, due_reason)` | `:931-946` | Write a `Missed` record and advance `next_run_at`. Only reachable when `catch_up == None` (`:719`, `:777`). |
| `restore_one(store, s, not_before, rearm)` | `:747-786` | Crash recovery. §1.6. |
| `restore(store)` | `:743-745` | `restore_one` over `store.list()`. |
| `restore_after_fire_error(store, id)` | `:806-817` | Re-arm after a `fire` that returned `Err`, at the **next** slot for an interval (so a poisoned schedule does not hot-loop) and **not at all** for a `once`. |

`MAX_TIMER_DELAY_MS = 2_147_483_647` (`:31`) is a **JS `setTimeout` i32 limit**. `tokio::time::sleep`
takes a `Duration` and has no such ceiling. **`[CYRUP-DELTA]`: drop the constant, and say why in a
doc line** — carrying it would silently cap a legitimate 30-day schedule at 24.8 days. This
is also why §1.2's tick shape is not merely a convenience.

`STALE_LAUNCH_CLAIM_MS = 5 * 60_000` (`:34`) **is** ported — it is §1.6's recovery window.
`DEFAULT_MAX_PENDING = 20` (`:32`) and `MAX_HISTORY = 100` (`:33`) are ported; `MAX_HISTORY` is the
`write_run` truncation at `:374` and belongs to part A's store if it landed `write_run`, here if not.

### 1.2 D1 — there is no upstream loop. The mechanism that works here.

> **ORIGINAL REQUIREMENT PRESERVED:** due schedules fire without a user asking; not-yet-due ones do
> not. That objective is unchanged.

Upstream re-arms one `setTimeout` per schedule. Reproducing that literally in Rust is one
`tokio::time::sleep_until` task per schedule plus a `HashMap<key, JoinHandle>` — buildable, but it
multiplies tasks by schedules and gives every `arm`/`clear_timer` pair a cancellation race that the
in-tree precedent does not have.

**Take the precedent instead.** `WaitSubscriptionManager::ensure_reconcile_timer`
(`background/wait_subscriptions/manager.rs:889-912`) is the exact shape, verified:

```rust
fn ensure_reconcile_timer(self: &Arc<Self>) {
    let mut slot = self.reconcile_task.lock().unwrap_or_else(PoisonError::into_inner);
    if slot.as_ref().is_some_and(|task| !task.is_finished()) { return; }
    // WEAK, not strong: a strong handle inside the task would be a reference cycle through the
    // task's own closure, so `Drop` would never run and the task would outlive its manager.
    let manager = Arc::downgrade(self);
    *slot = Some(tokio::spawn(async move {
        loop {
            tokio::time::sleep(RECONCILE_INTERVAL).await;
            let Some(manager) = Weak::upgrade(&manager) else { return };
            if manager.is_disposed() { return; }
            manager.reconcile().await;
        }
    }));
}
```

with `reconcile_task: Mutex<Option<tokio::task::JoinHandle<()>>>` (`:277`), `dispose()` (`:923-934`)
setting `disposed` under the lock **before** `abort_reconcile_timer()` (`:936-944`), and
`RECONCILE_INTERVAL` = `Duration::from_secs(1)` (asserted at `:2003`). Copy this shape exactly,
including the `Weak` and the `is_finished()` idempotence — its doc at `:885-888` even names the
reason a timer is armed from `restore()` rather than `new()` (`tokio::spawn` needs a runtime; the
executor is constructed outside one).

So `trigger.rs` exposes:

```rust
/// [CYRUP-DELTA] pi arms one `setTimeout` per schedule (`arm`, `:788-804`); cyrup drives one tick
/// for the whole store. The externally observable contract is identical to within one tick: a
/// schedule whose `nextRunAt <= now` fires, one whose `nextRunAt > now` does not.
pub const SCHEDULE_TICK: std::time::Duration = std::time::Duration::from_secs(30);

/// ONE tick. Pure with respect to time — `now` is injected, never read inside — so every test in
/// the table below drives it directly instead of sleeping.
pub async fn tick_due_schedules(store: &ScheduleStore, ctx: &ScheduleFireContext, now: i64)
    -> Vec<ScheduleRunRecord>;
```

`tick_due_schedules` is `runDue`'s body (`:715-721`) with upstream's own predicate verbatim —
`belongs_to_session && !paused && next_run_at(s).is_some_and(|n| n <= now)` — and upstream's own
branch: a schedule with **no active run**, `catch_up == None`, and `planned < now` is `record_missed`
(`:719`); everything else is `launch(..., "run-due"|"timer", advance=true)` (`:720`).

⚠ **The tick replaces `arm`/`clear_timer`/`fire`/`timerKey`/`stopTimers`, but NOT `restore_one`.**
The tick decides *when*; `restore_one` decides *what state a schedule was left in by a crash*, and
that is `SessionStart`-edge work (§1.6). Deleting `arm` is a delta; deleting `restore_one` is a bug.

⚠ **Pick the tick against the smallest schedulable interval.** `parseScheduleInterval` (`:134-143`)
accepts `m|h|d|w` with a minimum of `1m` = 60 000 ms. A 30 s tick keeps worst-case lateness at half
the smallest legal interval. `SCHEDULE_TICK` is a **`pub const`** so a test can assert it, and the
tick task reads it once — do not make it a parameter of `tick_due_schedules`, which takes `now`.

### 1.3 D2 — what a fired schedule actually launches

> **ORIGINAL REQUIREMENT PRESERVED:** a fired schedule produces a **real run**, carrying
> `session_id` and `completion_owner_id` like any other, so its result reaches the right instance
> and no other.

The target is `workflowScript`. Confirmed three ways at `v0.67.0`:

* `ScheduleTarget = { workflowScript: string; baseRef?: string }` (`:43`).
* `sanitizeTarget` (`:436-451`): refuses `tasks`/`chain` (*"Recurring schedules require
  workflowScript; legacy tasks and chain inputs are unsupported."*), refuses `agent`/`task`
  (*"schedule.create requires workflowScript. Use workflowScript: \"return runs.run('main', { agent, task })\"."*),
  requires a non-empty script, refuses `context: "fork"` (*"Scheduled runs require fresh context."*)
  and `async: false` (*"Scheduled runs are always async."*).
* `parseScheduleTarget` (`:295`): an `agent`/`task` record is a *"removed legacy agent target"*.
* `extension/tool-description.ts:43` @v0.67.0: *"Schedules take script inputs, not direct children."*

**cyrup refuses the async-workflow shape, permanently.** `extension/tool/routing.rs:546-561`:

```rust
// The async refusal is permanent (§0.6): `runs.all` already provides real in-workflow
// concurrency (fan-out width is free; only depth costs, and `RunMode::Chain` handles that).
// Async would buy only detachment, which this build does not implement for Workflow.
if p.r#async == Some(true) {
    return Err(ToolError::new(
        "workflowScript always runs in the foreground: runs.all already provides real in-workflow \
         concurrency, so async would buy detachment only. Omit async or pass async:false.",
    ));
}
```

and `background/state.rs:22-37` records the structural half: `RunMode::Workflow` is *"Produced
exclusively by the FOREGROUND arm (`route_workflow_mode` …); the background runner
(`background/runner_main/`) **never emits this mode**"*.

So `spawn_background` (`extension/executor/background.rs:46`) is **not** the path — it takes a
`BackgroundSingleRequest` (agent + task + overrides, `:49-70`) and neither it nor
`spawn_background_steps` (`:375`) has a script shape.

**THE MECHANISM THAT WORKS: an in-process, headless workflow run.** `route_workflow_mode`
(`routing.rs:609-810`) already builds every piece; a fired schedule needs the same block with the
tool-call inputs stubbed:

| What `route_workflow_mode` uses | Where | What a fire supplies |
|---|---|---|
| `crate::background::RunId::new()` | `:617` | same |
| `crate::identity::RunDirName::for_run(&id)` | `:618` | same |
| `default_async_root_in(&cfg.roots, cwd)` + `ensure_accessible_dir` | `:624-631` | same, `cwd` = `schedule.cwd` |
| `RunStatus::queued(id, RunMode::Workflow, Some(std::process::id()))` | `:633-638` | same |
| `status.session_id = SessionId::parse_opt(executor.current_session_id().as_deref())` | `:639-640` | **the PINNED id**, §2 |
| `status.tool_call_id = Some(call_id.as_str().to_string())` | `:645` | **`None`** — there is no tool call. `with_workflow_children` (`workflows/settlement.rs:102-105`) falls back to the run id, which `route_workflow_mode`'s own comment at `:643-644` already documents as the legal degradation. |
| `advance_state(RunState::Running)` + `write_atomic_json(run_dir/"status.json")` | `:648-655` | same, and **the `Queued -> Complete` note at `:646-647` applies identically** |
| `register_workflow_controller(&id, cancel)` | `:667-669` | same — this is what makes a fired run reachable by `action: "interrupt"` |
| `WorkflowRunHost::new(executor, cwd, on_update, id, status, run_dir, async_root)` | `:671-681` | **`on_update` = a no-op sink** |
| `run_workflow_script(RunWorkflowScriptOptions { … })` | `:700-…` | same, `state: None` (a schedule binds no mission — upstream `executionParams` sets `mission: false`, `:459`) |

The no-op sink is legal and cheap: `pub type ToolUpdateSink = Box<dyn FnMut(ToolUpdate) + Send + 'static>`
(`crates/cyrup-core/src/tool.rs:123`), so `Box::new(|_| {}) as ToolUpdateSink` satisfies
`WorkflowRunHost::new`'s third parameter, which wraps it into
`SharedUpdateSink = Arc<Mutex<ToolUpdateSink>>` (`extension/executor/workflow.rs:72`).

**THE WORK, stated plainly:** that ~200-line block at `routing.rs:609-810` must be **extracted** into
one reusable async fn — call it `SubagentExecutor::launch_headless_workflow_run` — taking
`{ cwd, script, timeout_ms, base_ref, pinned_session, cancel }` and returning `(RunId, PathBuf)`
plus a `JoinHandle`. `route_workflow_mode` then calls it with the real sink and call id; the trigger
calls it with the stubs. **This extraction is the single largest and riskiest piece of SCOPE_16**,
because `route_workflow_mode` is the only production emitter of `RunMode::Workflow` and every
WORKFLOW_17-21 behaviour rides it. Two guards:

1. Extract **without behaviour change first**, as its own commit, with the existing
   `routing_tests.rs` (2320 lines, 31 end-to-end cases) green before the trigger is wired.
2. `register_stop_child` (`:727-740`), `on_emit` (WORKFLOW_21), `on_host_step` (WORKFLOW_19) and the
   terminal receipt write all live in that block and must travel with it — a fired run that skipped
   the terminal write would strand a `Running` status forever.

**The result-delivery chain is then free, and this is the acceptance property.** A headless workflow
run writes its result exactly as a tool-driven one does, so `ResultsWatcher` observes it and
republishes it as a `CompletionEvent` — `extension/executor/workflow_detach/mod.rs:44-52` states the
invariant: *"cyrup's runner is a detached OS process whose only signal is the terminal `ResultFile`
it writes … **Writing the result file IS the emit**."* The delivery gate then admits it only for the
session that owns it, via `OwnershipSnapshot` (§2.3).

**`base_ref` is carried but not consumed.** `ScheduleTarget::base_ref` (`:43`) reaches upstream's
worktree setup. cyrup has `crate::spawn::worktree` but `route_workflow_mode` threads no base ref.
**Persist the field (part A already must, R15-2), refuse a non-`None` value at `schedule.create`
with a named message, and record it as a gap** — silently dropping a user's `baseRef` would run the
schedule against the wrong tree.

### 1.4 `launch` — the overlap lock (`:834-900`). The draft omits it entirely.

This is the correctness core and it must be ported line for line.

1. `:836` capture `next_run_at_before_claim` **before** anything mutates the record.
2. `:838-849` if `schedule.active_run_id.is_some()`: the run is `Skipped`; if `advance`, advance
   `next_run_at` via `next_after` and write; `write_run(…, "schedule.skipped_overlap")`; re-arm;
   return. **No lock is taken on this path.**
3. `:850-856` `lock_path = store.directory(id, create=true).join("active.lock")`; `mkdir -p` the
   parent 0700; open **`"wx"`** — i.e. `O_CREAT | O_EXCL`, mode 0600 — and write `run.id` into it.
   In Rust: `std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&lock_path)`.
   **`create_new(true)` is the whole mechanism** — it is the cross-process mutual exclusion that
   makes `overlap: "skip"` true when two cyrup instances share one cwd.
4. `:857-869` on `EEXIST` **only** (`ErrorKind::AlreadyExists`): same `Skipped` path as step 2. Any
   other error **propagates** (`:858` re-throws) — a permissions fault must not read as "skipped".
5. `:870-875` claim: `active_run_id = run.id`, `last_run_id = run.id`, advance `next_run_at` if
   `advance`, `updated_at`, `store.write`, `write_run(…, "schedule.run.started")`.
6. `:876-885` launch. On success record `async_id` (`result.details.asyncId ?? result.details.runId`)
   and `async_dir`, add to `observed_async_ids`, `write_run(…, "schedule.run.attached_async")`,
   re-arm, return with `state` still `Running`.
7. `:886-899` on failure: `state = FailedLaunch`, stamp `completed_at`/`error`, **re-read the record
   from the store** (`:890` `store.get(schedule.id)` — the in-memory copy is stale), clear
   `active_run_id`, **restore `next_run_at` to `next_run_at_before_claim` when `!advance`** (`:892`
   — a manual run must not eat the next scheduled slot), write, `write_run(…, "schedule.run.failed")`,
   **`fs.rmSync(lockPath, {force: true})`**, re-arm.

⚠ **The lock file is removed on exactly two paths: launch failure (`:896`) and `finish_run`
(`:926`).** A successful launch leaves it held for the whole run. A process that dies in between
leaves it on disk — which is what §1.6's `STALE_LAUNCH_CLAIM_MS` recovery exists to clear. Do not
"fix" this with a `Drop` guard: a `Drop` would release the lock when the *orchestrator* exits, while
the run it guards is still going.

⚠ `write_run` (`:371-378`) is three writes: the per-run file, the truncated `history.json`
(`MAX_HISTORY` = 100, newest first, de-duplicated by run id), and an **append** to `events.jsonl`
(0600). `crate::jsonl` and `artifacts::append_jsonl` (`artifacts.rs:347`) are the in-tree appenders.

### 1.5 Which session a fired schedule belongs to — **the draft is right, and here is the proof**

`fire`/`runDue`/`launch` all reach the live context through `requireContext(store)` (`:980-984`),
which reads `this.contexts.get(store.root)` — the map `selectProject` (`:956`) refreshes on every
binding with **the session that is live now**. Nothing anywhere reads a creating session off the
record; `ScheduleRecord` has no `createdBySessionId` field (`:45-63`). `ownerSessionFile` (`:58`) is
only consulted when `sessionOnly === true` (`:501`), and it is an ownership **gate**, not an
attribution.

**Write this into `trigger.rs`'s module doc, with the consequence spelled out:** the run's
`status.session_id` and its completion owner are **this process, this session** — the one live at
fire time — because attributing a fresh run to a dead session strands its result behind
`OwnershipSnapshot::readable_sessions()` (`background/delivery/ownership.rs:70-80`), which enumerates
the current session plus claimed predecessors and nothing else.

**And the counterweight, which the draft does not mention:** a `sessionOnly` schedule does **not**
fire in a later session at all — `fire` returns early at `:827`, `runDue` filters at `:715`,
`restore_one` returns at `:748`. So "a schedule created in one session fires in a later one" is true
**only** for `sessionOnly != true`, and the test at the bottom must say so in its own name.

### 1.6 `restore_one` (`:747-786`) — crash recovery. Also absent from the draft.

Runs on the `SessionStart` edge, per store, over `store.list()`:

* If `active_run_id` is set, find its `ScheduleRunRecord` in `history`.
* A `Running` record with an `async_id` re-registers into `observed_async_ids` (`:751`).
* A `Running` record with an `async_dir` re-reads `<async_dir>/status.json`; a terminal state
  (`complete|failed|stopped|rejected`) calls `finish_run` (`:756`). **`ENOENT` is swallowed; every
  other error propagates** (`:758`). In cyrup that read is
  `crate::background::control::read_status_file` and the errno predicates are
  `background::result_index::errno::is_absent` (`errno.rs:31`) — all four predicates there are
  `pub(crate)` and reachable from `background/scheduled_runs/` with **no visibility change**.
* **The stale-claim recovery** (`:761-772`): if `active_run_id` is set but there is no record, or the
  record is not `Running`, **or** the record is `Running` with **no `async_id`** and
  `started_at + STALE_LAUNCH_CLAIM_MS <= now` — mark it `FailedLaunch` with the exact string
  *"Recovered a stale launch claim before an async run was attached."*, clear `active_run_id`,
  write, and **delete `active.lock`** (`:771`). This is the only thing that unwedges a schedule
  whose process died between step 3 and step 6 of §1.4.
* Then, unless `!rearm || paused`: a `next_run_at` in the past with **no** active run and
  `catch_up == None` is `record_missed` (`:777-784`), whose failure re-arms at `not_before` and
  re-raises. Finally `arm(…, not_before)` — in cyrup, "finally ensure the tick is running".

---

## SUBTASK2 — the session-proxy shape

> **ORIGINAL REQUIREMENT, preserved verbatim:**
> ```ts
> const sessionId = source.getSessionId();                    // :466
> if (property === "getSessionId") return () => sessionId;    // :470
> ```
> *"Upstream wraps the session source in a proxy that pins `getSessionId` to a captured value, so a
> schedule firing mid-session-change sees one consistent identity for its whole execution. Port the
> *property* (a pinned identity for the duration of a fire), not the JS proxy mechanism — in Rust
> this is capturing the `SessionId` up front and passing it down, which is what `OwnershipSnapshot`
> already does for the drain loop."*

The instruction — port the property, not the proxy — is **exactly right**. Three corrections to the
detail (D7), then the mechanism.

### 2.1 The real upstream text, at `v0.67.0:466-479`

```ts
function snapshotContext(ctx: ExtensionContext, cwd: string): ExtensionContext {
	const source = ctx.sessionManager;
	const sessionId = source.getSessionId();                              // :468
	const sessionFile = source.getSessionFile();                          // :469
	const sessionManager = new Proxy(source, {
		get(target, property) {
			if (property === "getSessionId") return () => sessionId;      // :472
			if (property === "getSessionFile") return () => sessionFile;  // :473
			const value = Reflect.get(target, property, target) as unknown;
			return typeof value === "function" ? value.bind(target) : value;
		},
	});
	return { ...ctx, cwd, sessionManager };
}
```

**Two values are pinned, not one.** `sessionFile` (`:469`/`:473`) is the one
`scheduleBelongsToSession` actually reads (`:503`), so pinning only the id would leave the
sessionOnly gate reading live state — the precise bug the pin exists to prevent.

**The pin's scope is the BINDING, not the fire.** It is taken in `selectProject`
(`:956` `this.contexts.set(root, snapshotContext(ctx, projectCwd))`), which runs from `bindSession`
(`:531`) and from the top of every `handleToolCall` (`:545`). `selectProject` also compares the
**previous** context's normalized session file against the new one (`:953-955`) and re-`restore`s the
store when they differ (`:963-964`) — i.e. a session change is *detected*, and re-binding is the
response. Restate the requirement accordingly: **a pinned identity for the duration of a binding,
re-taken when the binding changes — and therefore constant across any single fire, which is the
property the draft asked for.**

### 2.2 Why this matters in cyrup specifically

`SubagentExecutor::current_session_id` (`extension/executor/session_state.rs:55-60`) is:

```rust
pub fn current_session_id(&self) -> Option<String> {
    self.host_services().and_then(|services| services.session_id()).filter(|id| !id.is_empty())
}
```

Its own doc (`:42-53`) says it is *"Read straight off the bound P-1 backend **on every call**"* and
that *"a session SWITCH inside one process moves the live id"*. So a trigger that called
`current_session_id()` once to filter, again to stamp `status.session_id`, and a third time for the
completion owner could observe **three different identities** in one fire on a session switch —
splitting exactly the way `OwnershipSnapshot` exists to prevent for the drain loop.

### 2.3 The mechanism

Capture once, per binding, into an owned struct; thread it down. `SessionId` is
`pub struct SessionId(Arc<str>)` (`identity/session_id.rs:41`) with
`parse(raw: &str) -> Option<Self>` (`:47`), `parse_opt(Option<&str>) -> Option<Self>` (`:59`) and
`as_str(&self) -> &str` (`:66`) — cheap to clone, so a snapshot costs nothing.

```rust
/// pi `snapshotContext` (`scheduled-runs.ts:466-479` @v0.67.0) — the PROPERTY, not the Proxy.
///
/// Both values upstream pins are pinned: the id (`:468`/`:472`) and the session FILE
/// (`:469`/`:473`), because the file is what `scheduleBelongsToSession` (`:503`) reads.
///
/// Re-taken on each `SessionStart` edge exactly as `selectProject` (`:948-967`) re-snapshots on
/// each binding, and constant for the whole of any one fire — which is the property that keeps a
/// fired run's `status.session_id`, its completion owner and the `sessionOnly` gate from observing
/// three different identities across one launch.
///
/// The precedent is `background/delivery/ownership.rs`'s `OwnershipSnapshot` (`:28-32`), taken once
/// before a drain loop for the same reason.
#[derive(Clone, Debug)]
pub struct ScheduleSessionSnapshot {
    session_id: Option<SessionId>,
    session_file: Option<PathBuf>,
}
```

`session_file` is normalized on capture, per `normalizedSessionFile` (`:487-491`):
`path::absolute`/`canonicalize` then, **on Windows only**, lowercase. Guard the lowercasing with
`cfg!(windows)` — lowercasing a POSIX path makes two distinct sessions compare equal.

**`ScheduleFireContext` (the value `tick_due_schedules` takes) carries this snapshot, the executor
handle and the cwd, and nothing reads `current_session_id()` below it.** That is the invariant a
reviewer checks by grep: **zero** `current_session_id` calls under `background/scheduled_runs/`.

---

## SUBTASK3 — `scheduled_runs/tool.rs`

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"The tool actions: create / list / cancel. Listing
> is session-scoped for *display* even though the store is not — a user asks 'what have I
> scheduled', and the answer is filtered by the session that created each entry where one is
> recorded."*

### 3.1 D5 — the surface is nine actions, in upstream's order (`:19-29`)

```
schedule.create   schedule.list   schedule.show     schedule.history   schedule.pause
schedule.resume   schedule.run    schedule.run-due  schedule.delete
```

Model on `crate::missions::MissionAction` (`missions/actions.rs:78-93`), the in-tree precedent for a
dotted action family: a `#[derive(Clone, Copy)] pub enum`, `as_str(self) -> &'static str` (`:98-108`),
`from_wire(&str) -> Option<Self>` (`:114-…`, whose `None` means *"not a mission action, fall through
to the rest of the dispatch table"*), and `is_mutating(self)`. Plus the handler pair:

```rust
pub struct ScheduledRunActionContext { /* cwd, session snapshot, config, executor handle */ }
pub struct ScheduledRunActionOutcome { pub text: String, pub details: serde_json::Value }
pub fn handle_scheduled_run_action(action, params, ctx) -> Result<Outcome, ScheduledRunError>;
```

mirroring `MissionActionContext` (`:184-193`), `MissionActionOutcome` (`:198-203`) and
`handle_mission_action` (`:852-856`). The `Outcome`-not-`ToolResult` split is deliberate and already
documented at `missions/actions.rs:195-197`; keep it, so `tool.rs` stays free of `cyrup_core`.

Upstream's `textResult` (`:417-424`) attaches
`details: { mode: "management", results: [], schedules: { records?, runs? } }`, where `records` is
`publicScheduleRecord` — **`ownerSessionFile` stripped** (`:426-429`). Port the stripping: the
session file is a filesystem path that a model has no business seeing.

⚠ `is_mutating` must return `true` for `create`/`pause`/`resume`/`run`/`run-due`/`delete` and
`false` for `list`/`show`/`history`, and the child-safe fanout tool must refuse the mutating six
with the existing sentence *"Action '{action}' is not available from child-safe subagent fanout
mode."* — the same shape `routing.rs:1665-1669` already applies to `mission.*`.

### 3.2 Per-action bodies, with upstream's verbatim strings

`schedule.create` (`:606-659`) — the gate order is load-bearing; each gate assumes the previous held:

| # | pi | Refusal (verbatim) |
|---|---|---|
| 1 | `:608-609` | `sanitizeTarget`'s four messages (§1.3) |
| 2 | `:612` | `schedule.create requires exactly one trigger: at or every.` (`Boolean(at) === Boolean(every)` — neither **or** both) |
| 3 | `:613` | `This first recurring slice supports overlap='skip' only.` |
| 4 | `:614` | `catchUp must be 'none' or 'latest'.` |
| 5 | `:615` | `Mission attachment is deferred from this first schedule slice.` |
| 6 | `:616` | `Calendar schedules are deferred from this first safe slice. Use a fixed interval such as every:'24h' or every:'7d'.` — triggered by `on`/`timezone`, or `every` ∈ {`day`,`week`,`month`,`year`} |
| 7 | `:617` | `quiet must be a boolean.` |
| 8 | `:618` | `quiet is only supported for recurring schedules.` (an `at` schedule with `quiet: true`) |
| 9 | `:620` | `sessionOnly schedules cannot use an explicit cross-project cwd.` |
| 10 | `:622` | `sessionOnly schedules require a persisted current session.` |
| 11 | **`:624`** | **`Cannot persist a schedule while a capability ceiling is active.`** — SCOPE_15's contract string |
| 12 | `:627` | `Schedule limit reached (${maxPending}).` — counted over `has_pending_schedule_work` |
| 13 | `:629` | `Schedule '${id}' already exists.` |

then `parseScheduledRunTime` (`:105-132`) or `parseScheduleInterval` (`:134-143`), the record build
(`:639-654` — note `catchUp` **defaults to `"latest"`**, `:647`), `store.write`,
`appendEvent(…, "schedule.created")`, arm, and the 7-line success text (`:658`).

`parseScheduledRunTime` has its own five messages and two accepted forms — `+<n><s|m|h|d>` with
`n >= 1`, or a **zone-bearing** ISO stamp `YYYY-MM-DDTHH:MM(:SS(.mmm))?(Z|±HH:MM)` — plus a real
calendar check (`daysInMonth`, `:127`) and `Scheduled time ${iso} is in the past.` (`:130`). Port it
in `schedule.rs`, not `tool.rs`, and give it its own unit tests; it is the one piece of this task
with genuinely fiddly input handling. `chrono` is already a workspace dependency used by
`crate::time`.

`schedule.list` (`:661-665`) — §3.4.
`schedule.show` (`:667-670`) — nine labelled lines; `shortenPath(schedule.cwd)` is cyrup's
`crate::formatters`.
`schedule.history` (`:672-676`) — `store.history(id)` rendered, or `No runs recorded for schedule ${id}.`
`schedule.pause`/`.resume` (`:678-688`) — one fn with a `paused: bool`; the idempotent reply is
`Schedule ${id} is already ${paused ? "paused" : "active"}.` and is **not** an error.
`schedule.run` (`:690-710`) — `launch(..., "manual", advance=false, quiet)`; on `Running`, advance
`next_run_at` (interval) or clear it (once) and append `schedule.manual_satisfied`. Refuses a
non-owner with `Skipped schedule ${id}: current session is not its owner.` (`:695`). Returns
`is_error = (state == FailedLaunch)` (`:709`).
`schedule.run-due` (`:712-723`) — §1.2's tick body, invoked on demand; `Processed ${n} due schedule(s).`
or `No schedules are due.`
`schedule.delete` (`:725-741`) — **the active-run guard**: if `active_run_id` is set, look up the
record; it is safe to delete only if the record's `scheduleId` matches, its state is `Running`, it
has both `async_id` and `async_dir`, **and** `<async_dir>/status.json` reports `runId == async_id`
with state ∈ `complete|failed|stopped|rejected`. Otherwise refuse with
`Schedule ${id} has active run ${activeRunId}; stop that run before deleting the schedule.`
Then clear the timer, `appendEvent(…, "schedule.deleted")`, `store.delete`, and
`Deleted schedule ${id}.`

Two cross-cutting behaviours: `handleToolCall`'s outer `try/catch` (`:558-560`) turns **every**
thrown error into `textResult(message, …, isError=true)` — so `handle_scheduled_run_action`'s
`Err` must be rendered as an error-flagged `ToolResult`, not propagated as a `ToolError` — and the
`scheduledRunsEnabled` gate (`:544`) answers
`Scheduled runs are disabled by scheduledRuns.enabled=false.` before the switch.

### 3.3 The `"unknown"` session fallback, restated because SCOPE_16 owns the call site

`:623` is `ctx.sessionManager.getSessionId() ?? "unknown"`. SCOPE_15 owns the gate; **SCOPE_16 owns
the argument**, so the literal must be supplied here:

```rust
let session_id = ctx.session.session_id().map_or_else(|| "unknown".to_string(), |s| s.as_str().to_string());
```

Not `Option<&str>`. The reason SCOPE_15 gives is the operative one: an `Option` lets a headless host
with no session gain the ability to persist schedules that a ceiling-bound one lacks. Note the
asymmetry with `resolve_capability_ceiling(session_id: Option<&str>, …)`
(`exec/capability_ceiling.rs:408`), whose `None` means "consult only the inherited ceiling" — the
adapter is one `Some(&literal)`, and it must be written deliberately, with this sentence beside it.

### 3.4 D4 — listing is NOT filtered upstream

> **ORIGINAL REQUIREMENT PRESERVED:** *a user asks "what have I scheduled" and gets a useful answer*.

`list()` (`:661-665`) verbatim:

```ts
const schedules = this.requireStore().list()
    .sort((a, b) => (a.trigger.nextRunAt ?? "").localeCompare(b.trigger.nextRunAt ?? ""));
if (!schedules.length) return textResult("No project schedules.", []);
return textResult([`Project schedules: ${schedules.length}`, ...schedules.map((item) =>
  `- ${item.id} | ${item.paused ? "paused" : item.activeRunId ? "running" : "scheduled"} | ${item.trigger.nextRunAt ?? "no next run"} | ${item.sessionOnly === true ? "session-only" : "project"} | ${item.name}`
)].join("\n"), schedules);
```

No filter. The header literally says **"Project schedules"**, and the fourth column is the
per-row `session-only` / `project` marker. The session dimension is surfaced as **information**, not
as a filter — and that is the correct design, because a filtered list would hide project schedules
the same user can still reach with `schedule.show` and `schedule.delete`, and would make
`maxPending`'s count (which is over the *whole* store, `:625`) unexplainable from the UI.

**Port `list()` exactly.** Then satisfy the draft's objective with the one thing upstream does do
and the draft did not name: **`sessionOnly` is the display distinction**, and `schedule.create`'s
own reply already reports `Session only: yes|no` (`:658`). The test below is renamed accordingly, and
keeps its original intent: the listing must let a user tell their own session-scoped schedules from
the project's.

---

## SUBTASK4 — register and wire

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"**Where:** `extension/tool/routing.rs` and the
> extension's action schema. Register the actions and start the trigger loop at install, alongside
> the existing detached schedules. Use SCOPE_14's shared staggered scheduler rather than a fourth
> ad-hoc `tokio::spawn`."*

Four of the five clauses hold; the fifth is D6. Both files named moved in #134/#139 and were
re-read; the line numbers below are current.

### 4.1 The tool-surface edits — six files, all verified

1. **`extension/tool/text.rs:215-289`** — append the nine verbs to `SUBAGENT_ACTIONS`, **after
   `"watchdog.recommend-model"`** (`:289`). That is upstream's own index:
   `shared/types.ts:2760` @v0.67.0 ends `… "watchdog.recommend-model", "schedule.create",
   "schedule.list", "schedule.show", "schedule.history", "schedule.pause", "schedule.resume",
   "schedule.run", "schedule.run-due", "schedule.delete"`.
   `DESTRUCTIVE_MANAGEMENT_ACTIONS` (`:299-312`) **already contains `"schedule.delete"`** — no edit,
   and `unknown_subagent_action_message` (`:395-…`) then applies its stricter distance-1 did-you-mean
   rule to it automatically.

2. **`extension/tool/schema.rs:349-361`** — the `action` enum is
   `"enum": SUBAGENT_ACTIONS` (`:359`), **derived, not hand-written**, so step 1 advertises them with
   no edit here. The test at **`:823-881`** asserts the enum's exact values *and order* against a
   hand-written `vec![…]`; extend that vec by the same nine, in the same place, with the
   `shared/types.ts:2760` citation.

3. **`extension/tool/schema.rs` `subagent_tool_parameters()` (`:319-…`)** — add the properties
   upstream declares at `extension/schemas.ts` @v0.67.0. `name` (`:279`) and `id`/`timeoutMs`/`cwd`
   already exist; the new ones are:
   ```
   at        :311  string,  "schedule.create: delay (+10m) or zoned ISO timestamp."
   every     :312  string,  "schedule.create interval, e.g. 30m/6h/2d/2w."
   sessionOnly :313 boolean (no description upstream)
   quiet     :314  boolean (no description upstream)
   on        :315  anyOf[string,integer], "Reserved calendar selector."
   timezone  :316  string  (no description upstream)
   overlap   :317  string, enum ["skip"]
   catchUp   :318  string, enum ["none","latest"], "Missed schedule occurrences; default latest."
   baseRef   :345  string  (no description upstream)
   ```
   `on` and `timezone` are **declared and refused** (`:616`) — port both, so a model that tries a
   calendar schedule gets upstream's actionable sentence instead of a schema rejection.

4. **`extension/tool/params.rs:85-215`** — `SubagentToolParams` has **none** of
   `name`/`at`/`every`/`session_only`/`quiet`/`on`/`timezone`/`overlap`/`catch_up`/`base_ref`
   (verified by grep). Add them, all `Option<_>`, `#[serde(rename_all = "camelCase")]` already
   handles the casing. `on` needs the same untagged string-or-integer helper `thinking` uses
   (`:161-163`). Then extend `provided_keys()` (`:503-…`) with the nine camelCase names, and add
   `fn schedule_action_params(&self) -> ScheduledRunActionParams` next to
   `mission_action_params()` (`:218-…`), which is the exact precedent.

5. **`extension/tool/routing.rs:1546`** — a new `match` arm in `route_action`, in upstream's own
   dispatch position. `route_control_action`'s doc at `:1595-1597` already names *"the
   `append-step`/`schedule.*` blocks"* as upstream's neighbourhood, and
   `subagent-executor.ts` dispatches `schedule.*` between `append-step` and `dismiss`. Place the arm
   **after** the `mission.*` arm (`:1655-1690`) and **before** `"validate"` (`:1715`), guarded the
   same way:
   ```rust
   schedule_action if ScheduledRunAction::from_wire(schedule_action).is_some() => { … }
   ```
   The arm (a) consults `AuthorityAction::for_tool_action(action)` for `schedule.create`
   (`registration/authority.rs:64-74`, default `AuthorityDecision::Auto`, `:86`), (b) applies the
   child-safe denylist, (c) builds `p.schedule_action_params()`, (d) calls
   `handle_scheduled_run_action`, (e) maps `Outcome { text, details }` into a `ToolResult` and an
   `Err` into an **error-flagged result**, not a `ToolError` (§3.2).

6. **`registration/tool_description.rs`** — §4.5.

⚠ **`schema.rs:1060`'s `every_advertised_schema_property_is_read_outside_provided_keys` will fail
unless the new params are read from inside `src/extension/`.** The guard walks **only the
`src/extension/` tree** (`:1061-1064`), excises every `fn provided_keys` body (`:1103-1123`), and
then requires a literal `.<snake_field>` read for every advertised property (`:1126-1139`). A read
that lives in `background/scheduled_runs/tool.rs` **does not count**. The fix is structural, not a
workaround: `schedule_action_params()` in `extension/tool/params.rs` reads all nine, which is
exactly why `mission_action_params()` lives there and not in `missions/`.

### 4.2 D6 — the loop's home. `spawn_retention_sweep` cannot host it.

> **ORIGINAL REQUIREMENT PRESERVED:** the trigger must not be a *fourth* ad-hoc `tokio::spawn`
> racing the others at session start.

`spawn_retention_sweep` (`extension/executor/notices.rs:648-682`) is:

```rust
pub(crate) fn spawn_retention_sweep(results_dir: PathBuf, delay: Duration) -> JoinHandle<()> {
    tokio::spawn(async move { tokio::time::sleep(delay).await; /* two sweeps */ })
}
```

— a **one-shot**. Its own doc header is *"Why delayed and detached, rather than at install or **on an
interval**"* (`:619`) and it explains that `tokio::spawn` + `sleep` is the analogue of upstream's
`unref`'d timer, *"so a session that ends inside the delay simply never sweeps"*. A schedule that
never fires because the session was young is not an acceptable degradation. And there is no fourth
task to avoid: today there are **two** periodic/delayed tasks in this crate —
`spawn_retention_sweep` and `WaitSubscriptionManager`'s reconcile timer.

**The trigger belongs to a `ScheduledRunManager`, with its own timer, on the
`WaitSubscriptionManager` pattern** (§1.2). That is the in-tree convention for *"a durable store
with a periodic reconciler that must be armed on `SessionStart` and disposed on `SessionShutdown`"*,
and scheduled runs are literally that.

Concretely, mirroring `extension/executor/wait_subscriptions.rs:276-325`:

```rust
// extension/executor/scheduled_runs.rs  (NEW — the executor half, for the same reason
// wait_subscriptions.rs exists: `background/` sits below `extension/executor/`, and the manager
// needs the executor's late-bound HostServices slot and the workflow launch path.)
impl SubagentExecutor {
    pub async fn install_scheduled_runs(self: &Arc<Self>, cwd: &Path) -> Arc<ScheduledRunManager>;
    pub fn dispose_scheduled_runs(&self);
}
```

`install_scheduled_runs` takes the `ScheduleSessionSnapshot` (§2.3), builds the store from
`config_snapshot().await` + `cwd`, `replace`s any previous manager and `dispose()`s it, then calls
`manager.restore().await` — which runs `restore_one` over the store (§1.6) **and** arms the tick.
`ScheduledRunManager` holds the executor as a `Weak` (`ExecutorSubscriptionSessions`'s reason at
`wait_subscriptions.rs:41-44` applies verbatim: the executor owns the manager, so a strong handle is
a cycle).

### 4.3 The two lifecycle edges, cited

* **`extension/host/native_impl.rs:298`** — the `HostEvent::SessionStart { .. }` arm. Install
  **after** `capture_parent_session_anchor()` (`:336`) — the snapshot must see a bound session — and
  next to `install_wait_subscriptions` (`:381`). **Unlike wait subscriptions, do NOT gate on
  `ctx.has_ui`** (`:380`): that gate exists because a headless run ends in one turn and could never
  receive a later-turn wake (`:374-379`); a schedule's output is a run on disk, which a headless
  process can produce perfectly well. Record that divergence in a comment rather than copying the
  `if ctx.has_ui` by reflex.
* **`extension/host/native_impl.rs:481`** — the `HostEvent::SessionShutdown { .. }` arm.
  `dispose_scheduled_runs()` beside `dispose_wait_subscriptions()` (`:493`), before
  `teardown_session()` (`:494`). Dispose must **abort the tick and clear the slot**, and must
  **not** touch the store — that is the whole point of a cwd-keyed store, and
  `wait_subscriptions/manager.rs:919-922` already documents the equivalent (*"The RECORDS stay on
  disk — that is the entire point of the durable half"*).

⚠ **The in-flight `active.lock` is deliberately NOT released on shutdown.** §1.4's note applies:
§1.6's stale-claim recovery is what clears it on the next start. Releasing it at dispose would let
the next session double-launch a schedule whose run is still going.

### 4.4 Config

`SubagentExtensionConfig` (`registration/mod.rs:79-…`) has **no** `scheduled_runs` field. Add it —
SCOPE_15 does not need it (the store path and the ceiling gate read neither key), SCOPE_16 needs
both:

```rust
/// pi `ExtensionConfig.scheduledRuns?: ScheduledRunsConfig` (`shared/types.ts:2501-2506,2651`
/// @v0.67.0).
#[serde(skip_serializing_if = "Option::is_none")]
pub scheduled_runs: Option<ScheduledRunsConfig>,

pub struct ScheduledRunsConfig {
    pub enabled: Option<bool>,     // pi :2502 — see the tri-state note
    pub max_pending: Option<u32>,  // pi :2503
    pub store_root: Option<PathBuf>, // pi :2505 "Absolute or `~/` root for per-project durable schedules."
}
```

⚠ **`enabled` is a tri-state and `Option<bool>` is the correct shape here**, unlike
`async_by_default` (`registration/mod.rs:87`, a plain `bool`). `scheduledRunsEnabled` is
`config.scheduledRuns?.enabled !== false` (`:96`) — absent and `true` both enable, only the literal
`false` disables — so `Option<bool>` with `!= Some(false)` reproduces it, and a plain `bool` with
`#[serde(default)]` would disable-by-default. Read it through an accessor
(`fn scheduled_runs_enabled(&self) -> bool`), never inline, so the polarity lives in one place.
`resolveMaxPending` (`:387-390`) is `integer && >= 1`, else `DEFAULT_MAX_PENDING` (20).

`store_root` reaches `scheduledRunStorePath`'s third parameter (`:99-103`) and flips the store from
`project_subagents_dir(cwd)/schedules` to `<root>/<sha256(resolve(cwd))[..20]>`. It also flips
`ScheduleStore::project_cwd` to `None` (`:960`), which **disables `assertScheduleRoot`'s
escape checks** (`:231-257`). SCOPE_15 owns the path fn; SCOPE_16 owns the config that feeds it, so
the coupling is recorded here.

### 4.5 D9 — the tool-description guard cannot be satisfied as its own doc says

`registration/tool_description.rs:92-102` carries a `[CYRUP-DELTA]` stating that upstream's
`COMPACT_SUBAGENT_TOOL_DESCRIPTION` (`tool-description.ts:80` **@v0.34.0**) carries the bullet
*"• Opt-in schedule actions: schedule, schedule-list, schedule-status, schedule-cancel. Schedule
only explicit delayed runs the user asked for."*, and `:713-716` says restoring that line is what
makes `the_compact_description_advertises_no_verb_cyrup_cannot_dispatch` (`:717-726`) keep passing.

**That vocabulary no longer exists.** At `v0.67.0`, `src/extension/tool-description.ts:43` reads:

> `• Management discovery: list/get/models/guide; create/update/delete/eject/disable/enable/reset/refine; mission.*, schedule.*, watchdog.*, inspector.*, project.*, lane.status/recordMerge/recordSupersession; … Schedules take script inputs, not direct children; recipes live in the missions guide.`

So there is no `schedule-list`/`schedule-status`/`schedule-cancel` line to restore, and the test's
four loop values are **not** cyrup's new verbs — `.contains("schedule")` would now match the
`schedule.*` text the description should carry, so the test would fail on a correct change.

**The required edit:** rewrite the `[CYRUP-DELTA]` to record that the deletion was against a
`v0.34.0` line since replaced; add a `schedule.*` mention to `COMPACT_SUBAGENT_TOOL_DESCRIPTION`'s
MANAGE / CONTROL block (`:117-119`) naming the nine verbs cyrup now dispatches; and convert the test
from a **hard-coded four-verb denylist** into the mechanical check it was always trying to be —
every dotted verb the compact text names must appear in `SUBAGENT_ACTIONS`. That closes the class
rather than moving the goalposts, and it is what stops the same drift the next time a family lands.

---

## 5. Capacity — the objective D3's mechanism cannot serve, and the one that can

> **ORIGINAL REQUIREMENT, preserved verbatim:** *"it acquires a **per-session capacity slot**
> (WORKFLOW_10) — a schedule cannot exceed its session's cap"*, and the two tests
> `a_fired_schedule_claims_a_capacity_slot_for_the_live_session` /
> `a_fired_schedule_is_refused_when_the_session_is_at_its_cap` (*"the cap actually binds"*).

**WORKFLOW_10 does not exist** (D3), and neither does `active_async_capacity`. Upstream has no
capacity notion in this file either — its two bounds are `maxPending` at create time (`:625-627`)
and the `overlap: "skip"` `active.lock` at fire time (§1.4).

**The gate that exists and meets the objective** is the per-session spawn budget:

```rust
// extension/executor/spawn_budget.rs:61
pub fn reserve_subagent_spawns(&self, requested: u32, max_spawns: u32) -> Result<(), String>
```

Its doc (`:14-28`) is the contract: *"charged UP FRONT (`count = used + requested`) and never
refunded"*; the comparison is pi's strict `used + requested > maxSpawns`, so a call landing exactly
**on** the cap is allowed; `requested == 0` is a no-op; **and the session identity is
`root_parent_session()`** (`session_state.rs:33-40`), which resets the counter in place on a session
change. The cap value is `SubagentExtensionConfig::max_subagent_spawns_per_session`
(`registration/mod.rs:97`, default 40), and `0` means unlimited there (`:105-106`).

**This is not optional politeness — the schedule path is currently unbilled.** `Tool::execute`
dispatches `action` and **returns** at `extension/tool/mod.rs:252-255`, which is **above** the
depth guard and above the spawn-budget charge that every execution mode pays (`:256-…`). So a
`schedule.run` / `schedule.run-due` that launched a run through `route_action` would spend a child
the session never paid for. The doc at `extension/executor/spawn_budget.rs:29-47` lists the three
entry points that each charge exactly once — *"EVERY route into execution charges here, so the
budget cannot be walked around by picking a different surface"*. **A fired schedule is a fourth
route, and this task is what keeps that sentence true.**

Charge **inside `launch`**, immediately before §1.4 step 3 (the lock), so a refusal costs no lock:

* refused → the `ScheduleRunRecord` is `FailedLaunch` with `reserve_subagent_spawns`' own returned
  message as `run.error`, `write_run(…, "schedule.run.failed")`, and **`next_run_at` advanced**
  (a schedule refused at the cap must not hot-retry every tick).
* admitted → proceed.

**Restate the two DoD lines and the two tests against this gate**, keeping their intent verbatim:
"a fired schedule claims a slot against the live session's budget" and "a fired schedule is refused
when that session is at its cap". When WORKFLOW_10 lands, its gate joins this one; it does not
replace it — a workflow-concurrency slot and a session spawn budget bound different things.

---

## Tests

Every name below is the draft's, or the draft's with a correction recorded next to it. Fail-before /
pass-after is stated for each. Unit tests live beside their module; the end-to-end ones belong in
`extension/tool/routing_tests.rs` (`#[path]` sibling of `routing.rs`, 2320 lines, whose header at
`:4-8` explains the split) or a new `src/tests/scheduled_runs_integration.rs` registered in
`src/tests/mod.rs`.

| test | pins | fail-before / pass-after |
|---|---|---|
| `a_due_schedule_fires` | the tick | Drive `tick_due_schedules(store, ctx, now)` with `next_run_at = now - 1`. **Before:** no such fn. **After:** one `ScheduleRunRecord` in state `Running` (or `FailedLaunch` under a stub launcher), `active_run_id` set, `active.lock` present. |
| `a_not_yet_due_schedule_does_not_fire` | the bound | `next_run_at = now + 1`. Empty result, no lock file, record untouched. |
| `a_fired_schedule_claims_a_spawn_slot_for_the_live_session` *(was `…_capacity_slot_…`, §5)* | §5 | With `max_subagent_spawns_per_session = 5` and 0 used, a fire leaves `spawn_budget_snapshot()` at 1 used. **Before:** the schedule path never charges. |
| `a_fired_schedule_is_refused_when_the_session_is_at_its_spawn_cap` *(was `…_at_its_cap`, §5)* | the cap binds | Cap 1, 1 used → the fire produces `FailedLaunch` carrying `reserve_subagent_spawns`' message, **no `active.lock` is created**, and `next_run_at` has advanced. |
| `a_fired_runs_result_is_delivered_to_the_live_session_only` | **THE ACCEPTANCE TEST** — schedule → launch → budget → delivery | Fire with session A pinned; assert the run dir's `status.json` carries `session_id == A`; then assert `OwnershipSnapshot` for a session B (`ownership.rs:49-59`, `readable_sessions` `:70-80`) does not admit it, and for A does. **Before:** no fired run exists at all. |
| `the_session_identity_is_pinned_for_the_duration_of_a_fire` | `:466-479` (§2) | Swap the stub `HostServices::session_id` **between** the tick's filter step and its launch step; assert the record's `session_id`, the `sessionOnly` gate's verdict and the completion owner all name the **captured** identity. **Before:** `current_session_id()` reads live on every call (`session_state.rs:55-60`) and the three would diverge. |
| **`[AUG]` `the_session_file_is_pinned_too`** | D7 — `:469`/`:473` | Same swap, asserting `scheduleBelongsToSession`'s input. Separate test because pinning only the id is the likely partial port and it passes the test above. |
| `a_non_session_only_schedule_created_in_one_session_fires_in_a_later_one` *(was `a_schedule_created_in_one_session_fires_in_a_later_one`, §1.5)* | the cwd-keyed store's reason to exist | Write under session A, dispose, re-install under session B, tick → it fires and is attributed to **B**. |
| **`[AUG]` `a_session_only_schedule_does_not_fire_in_a_later_session`** | `:827`, `:715`, `:748` — the counterweight | Same setup with `sessionOnly: true`; the tick returns empty and the record is untouched. Without this, a "fix" that drops the ownership gate passes every other row. |
| `listing_reports_session_only_and_project_schedules_distinctly` *(was `listing_schedules_is_filtered_for_display`, §3.4)* | SUBTASK3 | Two schedules, one `sessionOnly`; `schedule.list` returns **both**, sorted by `next_run_at`, with the fourth column reading `session-only` and `project`. **Explicitly asserts the list is NOT filtered** — the draft's own wording would have made the opposite assertion. |
| `cancelling_a_schedule_removes_it_from_the_store` | lifecycle | `schedule.delete` → directory gone, `ids()` no longer lists it, `Deleted schedule <id>.` **Before:** no `delete` action. |
| **`[AUG]` `deleting_a_schedule_with_a_live_run_is_refused`** | `:728-736` | The guard the draft's one-line "cancel" hides. Assert the verbatim `Schedule <id> has active run <runId>; stop that run before deleting the schedule.` |
| **`[AUG]` `a_second_fire_while_a_run_is_active_is_skipped_not_doubled`** | `:838-849` — `overlap: "skip"` | Tick twice without settling. Second record is `Skipped`, event `schedule.skipped_overlap`, exactly one launch. |
| **`[AUG]` `a_concurrent_fire_loses_the_exclusive_lock_and_skips`** | `:854-869` — the `O_EXCL` claim | Pre-create `active.lock` with `create_new`, then tick. `Skipped`, no second launch, **the pre-existing lock file is not removed**. The cross-process half of `overlap: "skip"`. |
| **`[AUG]` `a_failed_launch_releases_the_lock_and_restores_the_next_run_at`** | `:886-899` | Stub launcher returns `Err` on a `schedule.run` (`advance = false`). Record `FailedLaunch`, `active_run_id` cleared, `active.lock` **gone**, `next_run_at` back to its pre-claim value. |
| **`[AUG]` `a_stale_launch_claim_is_recovered_on_restore`** | `:761-772`, `STALE_LAUNCH_CLAIM_MS` | Hand-write `active_run_id` + a `Running` record with **no** `async_id` and `started_at = now - 6min`; `restore()` → `FailedLaunch` with the verbatim *"Recovered a stale launch claim before an async run was attached."*, `active_run_id` cleared, lock removed. |
| **`[AUG]` `catch_up_latest_fires_once_at_the_latest_missed_slot`** | `:411-415` | `every = 1h`, `next_run_at = now - 5h`, `catchUp: "latest"` → **one** record with `planned_at == now - 1h` (not five records, not `now - 5h`). |
| **`[AUG]` `catch_up_none_records_a_missed_occurrence_instead_of_firing`** | `:719`, `:777-784`, `:931-946` | Same setup with `catchUp: "none"` → one `Missed` record, **no launch**, `next_run_at` advanced past `now`. |
| **`[AUG]` `a_schedule_asleep_for_many_intervals_lands_in_the_future`** | `:396-401`'s `while` | After a fire with `now` six periods past `planned`, `next_run_at > now`. A single `+= every_ms` fails this. |
| **`[AUG]` `every_scheduled_run_action_dispatches`** | SUBTASK4 / the advertise-vs-dispatch invariant | For each of the nine, `route_action` must not return `unknown_subagent_action_message`. The exact shape `schema.rs:885-900` already applies to the watchdog and management families. |
| **`[AUG]` `the_action_enum_carries_the_nine_schedule_verbs_in_pis_order`** | `shared/types.ts:2760` | Extends the existing `subagent_tool_schema_exposes_the_full_pi_parameter_union` vec (`schema.rs:823-881`). Fails before the `text.rs` edit. |
| **`[AUG]` `every_advertised_schedule_property_is_read_outside_provided_keys`** | `schema.rs:1060` | Not a new test — the **existing** guard, which will fail the moment §4.1 step 3 lands without step 4's `schedule_action_params()` in `extension/tool/params.rs`. Named here so it is expected, not debugged. |
| **`[AUG]` `a_capability_ceiling_refuses_schedule_create_through_the_tool`** | `:624` end-to-end | SCOPE_15 pins the predicate; this pins the **wiring**, with the verbatim string, reached through `route_action`. |
| **`[AUG]` `schedule_create_refuses_a_calendar_trigger_with_pis_own_sentence`** | `:616` | `on`/`timezone`/`every: "day"`. Pins that the two reserved params are *declared and refused*, not silently dropped. |
| **`[AUG]` `the_max_pending_limit_refuses_the_twenty_first_schedule`** | `:625-627`, `:32` | Verbatim `Schedule limit reached (20).` |
| **`[AUG]` `schedule_create_refuses_a_non_workflow_script_target`** | `:436-451` (§1.3) | `{agent, task}` → the verbatim *"schedule.create requires workflowScript. Use workflowScript: \"return runs.run('main', { agent, task })\"."*. The single most likely wrong implementation of this task. |
| **`[AUG]` `extracting_the_workflow_launch_changes_no_foreground_behaviour`** | §1.3's extraction risk | Not one test: the **existing** `routing_tests.rs` suite, required green on the extraction commit *before* the trigger is wired. Call it out in the DoD, because a silent regression here breaks WORKFLOW_17-21. |

Tick/timer behaviour is exercised with `#[tokio::test(start_paused = true)]` + `tokio::time::advance`,
as `wait_subscriptions/manager.rs`'s own timer tests do. **No production test sleeps**:
`tick_due_schedules` takes `now`, so every row above except the two timer-arming ones drives it
directly.

---

## Benchmarks

None. **Unchanged from the draft, and now with the reason checked:** the tick reads one directory
(`ids()`, `:330-336`) and at most one small JSON per schedule, bounded by `maxPending` = 20, once
every `SCHEDULE_TICK`. SCOPE_14 owns the one performance-scoped item in this programme.

---

## Definition of done

Checkable by reading the cited code and running the named tests.

- [ ] Due schedules fire; not-yet-due ones do not (`a_due_schedule_fires`,
      `a_not_yet_due_schedule_does_not_fire`), through `tick_due_schedules(store, ctx, now)` with
      `now` injected — **not** through a sleeping loop, and with `MAX_TIMER_DELAY_MS` deliberately
      dropped and documented (§1.1).
- [ ] The full `launch` state machine is ported: overlap skip, the `create_new` 0600 `active.lock`,
      the `EEXIST`-only skip, the claim, and the failure rollback that restores `next_run_at` and
      removes the lock (`a_second_fire_while_a_run_is_active_is_skipped_not_doubled`,
      `a_concurrent_fire_loses_the_exclusive_lock_and_skips`,
      `a_failed_launch_releases_the_lock_and_restores_the_next_run_at`).
- [ ] `catch_up` `latest` and `none` both behave as `:411-415` / `:719` specify, and a long sleep
      lands in the future (three named `[AUG]` tests).
- [ ] `restore_one`'s stale-launch-claim recovery runs on `SessionStart` with upstream's verbatim
      string (`a_stale_launch_claim_is_recovered_on_restore`).
- [ ] A fired schedule claims a slot against the **live session's spawn budget**
      (`reserve_subagent_spawns`, `extension/executor/spawn_budget.rs:61`) and is refused at the cap
      without taking the lock (§5's two tests). The doc at `spawn_budget.rs:29-47` is updated to list
      the schedule path as a fourth billed route.
- [ ] A fired run's result is delivered only to the live session, verified end to end
      (`a_fired_runs_result_is_delivered_to_the_live_session_only`).
- [ ] Session identity **and session file** are pinned for the duration of a binding and therefore of
      a fire (`the_session_identity_is_pinned_for_the_duration_of_a_fire`,
      `the_session_file_is_pinned_too`); `grep -rn "current_session_id" background/scheduled_runs/`
      returns **zero**.
- [ ] A non-`sessionOnly` schedule created in one session fires in a later one, and a `sessionOnly`
      one does not (two named tests).
- [ ] All **nine** actions registered in `SUBAGENT_ACTIONS` at upstream's index and dispatched from
      `route_action` (`every_scheduled_run_action_dispatches`,
      `the_action_enum_carries_the_nine_schedule_verbs_in_pis_order`); the nine new schema properties
      are read from `extension/tool/params.rs::schedule_action_params`
      (`every_advertised_schema_property_is_read_outside_provided_keys` green).
- [ ] `schedule.create` refuses a non-`workflowScript` target with upstream's verbatim sentence, and
      every gate in §3.2's table fires in upstream's order with upstream's string.
- [ ] The trigger runs from a `ScheduledRunManager` armed on `SessionStart`
      (`native_impl.rs:298`, after `:336`) and disposed on `SessionShutdown` (`:481`, beside `:493`),
      on the `WaitSubscriptionManager::ensure_reconcile_timer` pattern — **not** inside
      `spawn_retention_sweep`, with the reason recorded in a doc comment (§4.2).
- [ ] `SubagentExtensionConfig::scheduled_runs` lands with `enabled` as a tri-state `Option<bool>`
      read through one accessor (§4.4).
- [ ] `registration/tool_description.rs`'s `[CYRUP-DELTA]` and its guard test are corrected to the
      `v0.67.0` surface (§4.5), and the compact description names `schedule.*`.
- [ ] `routing_tests.rs` is green on the workflow-launch extraction commit **before** the trigger is
      wired (§1.3).
- [ ] Workspace `--no-fail-fast` 0 failed; clippy exit 0.

---

## 6. Seam index — every citation, re-opened 2026-09-15

**Upstream, `v0.67.0` (`git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:<path>`)**

| Path | Lines | What |
|---|---|---|
| `src/runs/background/scheduled-runs.ts` | `:19-29` | `SCHEDULED_RUN_ACTIONS` (nine) |
| " | `:31-35` | `MAX_TIMER_DELAY_MS`, `DEFAULT_MAX_PENDING`, `MAX_HISTORY`, `STALE_LAUNCH_CLAIM_MS`, `SCHEDULE_ID` |
| " | `:40-77` | `ScheduleTrigger`, `ScheduleTarget`, `ScheduleRecord`, `ScheduleRunRecord` |
| " | `:87`, `:96`, `:99-103` | ceiling dep, `scheduledRunsEnabled`, `scheduledRunStorePath` |
| " | `:105-143` | `parseScheduledRunTime`, `parseScheduleInterval` |
| " | `:317-385` | `ScheduleStore` |
| " | `:387-415` | `resolveMaxPending`, `hasPendingScheduleWork`, `nextAfter`, `nextRunAt`, `duePlannedAt` |
| " | `:417-464` | `textResult`, `publicScheduleRecord`, `targetLabel`, `sanitizeTarget`, `executionParams` |
| " | **`:466-479`** | **`snapshotContext` — SUBTASK2's pin (id `:468`/`:472`, file `:469`/`:473`)** |
| " | `:487-505` | `normalizedSessionFile`, `scheduleBelongsToSession` |
| " | `:542-561` | `handleToolCall` dispatch + outer catch |
| " | `:606-741` | the nine action bodies (`create` `:606`, ceiling gate **`:623-624`**) |
| " | `:743-832` | `restore`, `restoreOne`, `arm`, `restoreAfterFireError`, `fire` |
| " | `:834-946` | `launch`, `finishRun`, `recordMissed` |
| " | `:948-1001` | `selectProject` (the pin's site, `:956`), `requireContext`, the timer map |
| `src/shared/types.ts` | `:2501-2506`, `:2651` | `ScheduledRunsConfig` |
| " | `:2760` | `SUBAGENT_ACTIONS` — the nine at the end |
| `src/extension/schemas.ts` | `:279`, `:311-318`, `:345` | `name`, the eight schedule params, `baseRef` |
| `src/extension/tool-description.ts` | `:43` | the `schedule.*` bullet (§4.5) |

**cyrup, branch `claude/subagents-scope-3` @ `b2fdc7e`** (all under `crates/cyrup-ext-subagents/src/`)

| Path | Lines | What |
|---|---|---|
| `extension/tool/routing.rs` | `:1540-1546` | `route_action`'s `match` — the new arm's home |
| " | `:1591-1592`, `:1655-1690` | the management band; the `mission.*` arm to copy |
| " | **`:546-561`** | **the permanent async-`workflowScript` refusal (D2)** |
| " | `:609-810` | the workflow launch block to extract (§1.3) |
| " | `:1739` | `unknown_subagent_action_message` call site |
| `extension/tool/schema.rs` | `:319-361` | `subagent_tool_parameters`; `action` enum derived at `:359` |
| " | `:823-881` | the exact-order action-enum assertion |
| " | `:1060-1139` | `every_advertised_schema_property_is_read_outside_provided_keys` (§4.1 ⚠) |
| `extension/tool/params.rs` | `:85-215` | `SubagentToolParams` — nine fields missing |
| " | `:218-…` | `mission_action_params()` — the precedent |
| " | `:503-…` | `provided_keys()` |
| `extension/tool/text.rs` | `:215-289` | `SUBAGENT_ACTIONS` |
| " | `:299-312` | `DESTRUCTIVE_MANAGEMENT_ACTIONS` — already has `schedule.delete` |
| " | `:395-…` | `unknown_subagent_action_message` |
| `extension/tool/mod.rs` | `:252-255` | `route_action` returns **above** the spawn-budget charge (§5) |
| `extension/tool/routing_tests.rs` | 2320 lines | the 31 end-to-end cases that must stay green |
| `extension/executor/notices.rs` | `:603-611` | `stop_completion_watcher` teardown |
| " | **`:615`, `:648-682`** | **`RETENTION_SWEEP_DELAY`, `spawn_retention_sweep` — one-shot (D6)** |
| `background/wait_subscriptions/manager.rs` | `:271-277`, **`:885-912`**, `:914-944` | **the timer precedent (§1.2, §4.2)** |
| `extension/executor/wait_subscriptions.rs` | `:41-56`, `:276-325` | the executor-half precedent (§4.2) |
| `extension/host/native_impl.rs` | `:298`, `:336`, `:380-384` | `SessionStart`; anchor capture; the `has_ui` gate not to copy |
| " | `:481`, `:493-494` | `SessionShutdown` |
| `extension/executor/session_state.rs` | `:33-40`, **`:42-60`** | `root_parent_session`; **`current_session_id` reads live (§2.2)** |
| `extension/executor/spawn_budget.rs` | `:14-47`, **`:61`**, `:82`, `:153` | the budget doc; **`reserve_subagent_spawns` (§5)** |
| `extension/executor/background.rs` | `:46-48`, `:500`, `:513` | `spawn_background`; `session_id`/`completion_owner_id` stamping |
| `extension/executor/workflow.rs` | `:72`, `:160-220` | `SharedUpdateSink`; `WorkflowRunHost` |
| `extension/executor/workflow_controllers.rs` | **`:16-19`** | **"WORKFLOW_10's per-session capacity gate does not exist in this build" (D3)** |
| `extension/executor/workflow_detach/mod.rs` | `:44-52`, `:570` | "Writing the result file IS the emit"; `scheduleOrigin` has no field anywhere |
| `background/state.rs` | **`:22-37`** | **`RunMode::Workflow` is foreground-only (D2)** |
| `background/delivery/ownership.rs` | `:28-32`, `:49-59`, `:70-80`, `:161-181` | `OwnershipSnapshot` — SUBTASK2's precedent and the delivery assertion |
| `background/mod.rs` | `:36-51`, `:69-79` | the two `pub mod` blocks (R15-1) |
| `background/result_index/errno.rs` | `:22`, `:31`, `:47`, `:57` | the four `pub(crate)` predicates, reusable as-is |
| `background/atomic.rs` | `:75`, `:209` | `write_atomic_json`, `write_private_atomic_json_blocking` |
| `artifacts.rs` | `:155`, `:347` | `project_subagents_dir`, `append_jsonl` |
| `identity/session_id.rs` | `:41`, `:47`, `:59`, `:66` | `SessionId` |
| `time.rs` | `:18` | `now_epoch_millis` |
| `exec/capability_ceiling.rs` | `:408`, `:429` | `resolve_capability_ceiling` (§3.3) |
| `missions/actions.rs` | `:78-93`, `:98-118`, `:184-203`, `:852-856` | the action-family precedent (§3.1) |
| `registration/mod.rs` | `:79-…`, `:97`, `:105-106` | `SubagentExtensionConfig`; the session spawn cap |
| `registration/authority.rs` | `:32`, `:43`, `:64-74`, `:86`, `:266-277` | `ScheduleCreate` — already mapped, awaiting this dispatch |
| `registration/tool_description.rs` | `:92-102`, `:108-127`, **`:713-726`** | **the stale `[CYRUP-DELTA]` and its guard (§4.5, D9)** |
| `crates/cyrup-core/src/tool.rs` | `:123` | `ToolUpdateSink` — the no-op sink (§1.3) |

---

## 7. Open questions the implementor MUST NOT silently decide

Listed in `unresolvedQuestions` as well.

1. **`SCHEDULE_TICK` value.** §1.2 argues 30 s against the 1-minute minimum interval. 60 s halves the
   wakeups and makes worst-case lateness equal the smallest legal interval. Pick deliberately and
   record the reason; do not leave it at whatever the first test needed.
2. **Does the headless workflow launch belong to SCOPE_16 at all?** §1.3's extraction is the largest
   and riskiest piece, and it touches the only production emitter of `RunMode::Workflow`. The
   alternative is a SCOPE_16 that lands the store-driven half (nine actions, tick, lock, recovery,
   budget) with the launch behind a seam that `schedule.run` reports as unavailable — smaller, safe,
   and a visibly incomplete feature. **This is the single decision that changes this task's size;
   a human should make it.**
3. **`baseRef`.** §1.3 proposes persist + refuse-at-create + record the gap, because silently
   dropping it runs the schedule against the wrong tree. The alternative is threading
   `crate::spawn::worktree` into the extracted launch, which is a second unbounded piece.
4. **`scheduleOrigin` has no home.** Upstream stamps `{ id, name?, quiet? }` onto the launch (`:461`)
   so an unattended completion names its origin; `workflow_detach/mod.rs:570` confirms cyrup has
   **no field anywhere** for it, and `background/watch/message.rs:31` confirms the notify renderer
   cannot consult one. Either add a field (widening `RunStatus`/`ResultFile`, a schema change) or
   accept that a scheduled completion is indistinguishable from any other. **Do not quietly accept.**
5. **`storeRoot` disables the escape checks.** `:960` passes `projectCwd: undefined` when a store
   root is configured, which turns off `assertScheduleRoot` (`:231-257`). Is that acceptable in
   cyrup, or does the config key ship disabled until the checks are ported? Interacts with SCOPE_15.
6. **`quiet`.** Its only upstream effect is on the completion notice via `scheduleOrigin` (`:461`) —
   which per (4) cyrup cannot carry. Accept the param and store it (round-trip fidelity), or refuse
   it at create as unsupported? Accepting a knob that does nothing is its own defect.
7. **The tick's default for a headless host.** §4.3 argues against copying the `has_ui` gate. If a
   `cyrup -p` run installs the manager, it can fire a schedule in a one-shot process. Intended, or
   should schedules be interactive-only?
8. **Windows `normalizedSessionFile` / `normalizedComparisonPath`.** `:154-165` and `:487-491` carry
   real Windows handling (`realpathSync.native`, UNC prefix stripping, lowercasing). How much is in
   scope? The workspace has a `WINDOWS_BUILD_UNGATED_DEPS` item already open. §2.3 requires at
   minimum that the lowercasing be `cfg!(windows)`-guarded.
