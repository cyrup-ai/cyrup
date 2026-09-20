---
stage: done
status: completed
updated: 2026-09-20
---

# `children.list` + `debug.run` — the two remaining verbs that need no external binary

OBJECTIVE: close 2 of the 9 verbs missing from `SUBAGENT_ACTIONS` (50 → 52). Both are small, both sit
on data that already exists on disk, and both correct in-tree records that have gone stale. The
other 7 (`inspector.*`, `project.*`) require third-party terminal binaries and are a separate
decision.

---

## Verb 1 — `children.list`: list the retained children of a workflow run

### The in-tree record is wrong three ways — verified, cite these when you correct them

1. `extension/tool/text.rs:252` says *"`children.list` needs RETENTION, and this build retains
   nothing: the foreground `WorkflowRunHost`'s `settled` list is dropped with the tool call."*
   **Stale.** Upstream's `listRetainedChildren` (`retained-children.ts:83`) is a **disk scan**:
   `listAsyncRuns(asyncDirRoot, { sessionId, states: ["complete","failed","paused","stopped"],
   reconcile: false })`, filtered to runs with `parentWorkflowRunId` and exactly one terminal step.
   Nothing is held in memory. cyrup writes async runs to disk.
2. `missions/goal_driver.rs:30` — `[CYRUP-DELTA] RetainedChild has no producer in cyrup yet …
   cyrup has no workflowScript runtime (the identifier appears nowhere in this crate)`. **False.**
   `workflows/scripted/` has 10 files (`engine.rs`, `analyzer.rs`, `permit.rs`, `recovery.rs`,
   `settlement.rs`, …) and `workflowScript` appears in **35** files.
3. `extension/executor/notices.rs:592` — same false premise, passing an empty list *"rather than
   faked"*. The list is not empty on disk.

**The producer already exists**: `extension/executor/workflow.rs:886` writes
`parent_workflow_run_id: Some(self.workflow_run_id.clone())` on every workflow child;
`background/run_status.rs:251` carries it on `RunStatus`; `:1912`, `workflow_steering.rs:420`,
`workflow.rs:1743` all thread it. `RetainedChild` is already typed in `missions/goal_driver.rs`.

### Upstream, pinned at v0.68.0 — `src/runs/background/retained-children.ts` (135 lines)
- `RetainedChild:16` — `runId, parentRunId?, workflowKey?, state, agent, taskSummary, completedAt,
  resumability, sessionPath?, tokenTotals?`
- `isRetainedChildState:28` — `complete | failed | partial | paused | stopped`
- `retainedSessionFile:37` — `.jsonl`, regular file, not a symlink (`lstat`), ENOENT → reason
- **`childResumability:53`** — stopped → no; external runner → no; session file check; then the
  recovery descriptor must exist, `sourceRunId === run.id`, `agent === step.agent`; then
  `resolveRetainedWorktreeCwd(parallelHandoffPath(run.asyncDir), run.id, step.index) ?? descriptor.cwd ?? run.cwd`
  must be an existing directory. **Every one of those dependencies landed in #143/#144**:
  `handoff::resolve_retained_worktree_cwd`, `background::recovery_descriptor::read`, `RunDir::handoff()`.
- `boundedTaskSummary:77`, `MAX_TASK_SUMMARY_LENGTH`, `MAX_RETAINED_CHILD_CANDIDATES`, `MAX_RETAINED_CHILDREN`
- **`listRetainedChildren:83`** and **`formatRetainedChildren:110`** — note the formatter's
  "keep one resumable child in the window" rule and the fallback-challenge sentence when none is.
- Dispatch: `subagent-executor.ts` — find the `"children.list"` arm; it is read-only (NOT in
  `MUTATING_MANAGEMENT_ACTIONS` at `:213`).

### cyrup seams
- The async-root scan: cyrup has no `list_async_runs` by that name — find the existing scanner the
  fleet view and `active_run_index.rs` use, and reuse it (`fleet_view.rs:394` already projects
  `parent_workflow_run_id`).
- The two consumers that currently pass an empty list: `notices.rs:592` and `goal_driver.rs`
  (`collect_goal_continuation_notices`). Once a real list exists, they must receive it — and their
  `[CYRUP-DELTA]`s must be deleted, not reworded.
- Add the verb at pi's own index (between `models` and `guide`, `text.rs:244` says so).

### Definition of done
- `subagent({action:"children.list"})` lists the retained children of the active session's workflow
  runs, newest first, with resumability computed by the real predicate.
- The reachability test launches a workflow whose child settles, then calls the verb through
  `SubagentTool::execute` and asserts the child appears with `resumability: resumable` — and a
  second case where the recovery descriptor is removed asserts `not resumable (missing recovery
  descriptor)`. Mutation: stub the scan to empty → fail.
- The three stale notes are corrected and the two empty-list consumers receive the real list.

---

## Verb 2 — `debug.run`: a run-lifecycle diagnostic dump

When a run is stuck, this is how an operator sees why. It is a MODE on `status`: same dispatch arm
(`subagent-executor.ts`: `if (action === "status" || action === "debug.run")`), label "Debug run",
requires `id`/`runId`/`dir`, rejects `view`.

### Upstream, pinned at v0.68.0 — `src/runs/background/run-status.ts` (786 lines)
- `debugProcessTerminal:52` — reads the `process-terminal.json` **sidecar** and sanitizes the
  status's own `processTerminal` **overlay** against `{ runId, runnerProcessInstanceId }`.
- `formatCapacityOwner:60` — over `ActiveAsyncCapacityInspection` (release state/reason, owner
  slot/kind/generation/session/sourceRun/asyncDir/runner/startedAt).
- `formatWorkflowDebug:74` — parent, key, lane, and one line per step.
- **`formatRunLifecycleDebug:87`** — the dump: run, dir, status file, workflow receipt, process
  terminal file, session, state, mode, parent/key/lane, BOTH process terminals, capacity owner
  lines, workflow lines.
- The four arms: `:409` (location via `resolveAsyncRunLocation`), `:486` and `:513` (the two
  status tiers), `:715` (the "needs an async run directory with status.json" refusal).

### cyrup seams — and one thing the augment MUST settle
- Capacity inspection: `background/active_async_capacity/inspect.rs` (landed with #141). Its
  `:152` cites `runs/background/process-terminal.ts` types.
- **`process_terminal` — how real is it?** `background/active_run_index.rs:56` says
  *"`readProcessTerminal(asyncDir)?.state === "observed"` is the OTHER disjunct of the reader's"*,
  which implies a reader exists. But ledger row `VL-S4` (`PARITY-GAPS.md:1445`) says the
  process-terminal record is *"STILL OPEN … the SCOPE sequence made it LOAD-BEARING"* and that
  `active_async_capacity` substituted runner-pid liveness for the missing proof. **Establish
  exactly what exists**: is there a reader of `process-terminal.json`? a writer? If `debug.run`
  can print a real sidecar, do so; if cyrup has only the pid substitute, print THAT honestly
  ("Process terminal: not recorded; runner pid liveness: …") and say in the spec that `debug.run`
  is complete over cyrup's data and that VL-S4 remains its own row. **Do not fabricate a sidecar
  line from the pid substitute.**
- `RunStatus` already carries `parent_workflow_run_id`, `workflow_key` (check), `lane` (#143).

### Definition of done
- `subagent({action:"debug.run", id})` prints the lifecycle dump for a real async run; `view` is
  refused with pi's sentence; a directory without `status.json` is refused with pi's sentence.
- Reachability test drives `SubagentTool::execute` against a real on-disk run and asserts the
  capacity-owner lines reflect the real slot. Mutation: drop the capacity section → fail.

---

## Rules
- Upstream reads ONLY via `git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`.
- Port the behaviour, adapted to Rust; the on-disk formats do not change.
- Every `[CYRUP-DELTA]` states a reason that is TRUE — this task exists partly because three did not.
- No `allow(dead_code)`, no stub, no narrowing-and-reporting-done.
- Gates: fmt; clippy `--workspace --all-targets --features test-fixtures -- -D warnings`;
  `nextest run --workspace --features test-fixtures` (baseline **10 661** / 9 skipped);
  `nextest run -p cyrup-it --features it` (baseline **566**).

---

## [AUG — debug.run]

Research only. Every upstream line below was read with
`git -C /home/user/cyrup/tmp/pi-subagents show v0.68.0:<path>`; every cyrup line with the tree at
HEAD. Nothing in this section was inferred from a doc comment — each claim carries the grep.

### 0. THE THING SETTLED — what cyrup has for the process-terminal proof

**Nothing. No reader, no writer, no sidecar, no status overlay.** Only the runner-pid substitute.

The grep, whole crate:

```
grep -rn 'process-terminal\|process_terminal\|ProcessTerminal' crates/cyrup-ext-subagents/src
```

12 hits, **all inside `//!`/`///`/`//` comments or string literals**, none a symbol:
`background/active_run_index.rs:56-57,:359-361` · `background/active_async_capacity/inspect.rs:5,
:33,:152,:223,:332,:449` · `.../key.rs:93-94` · `.../mod.rs:63` · `.../tests.rs:340` ·
`extension/rpc/ping.rs:60-61,:66` · `registration/doctor.rs:1019`. Three more checks that close
the question from the other side:

- `RunDir` (`background/run_paths.rs:39-110`) exposes `status()`, `events()`, `handoff()`,
  `recovery_descriptor()` — **no process-terminal path**. `RunPaths` (`:123-171`) likewise.
- `RunStatus` (`background/records.rs:225-381`) has **no `process_terminal` field**; `StepStatus`
  (`records.rs:24-160`) neither. Upstream's `overlayStatus` (`process-terminal.ts:224-241`) writes
  `status.processTerminal` and `step.processTerminal` — cyrup's serde model would drop both keys on
  read and never writes them.
- `grep -rn 'Process terminal' crates/cyrup-ext-subagents/src` → **0**. cyrup's `status` view does
  not print upstream's `Process terminal:` line (`run-status.ts:578`) either, so `debug.run` is not
  the first surface to face this — it is the first to have to say so explicitly.

The sentence the seed flagged — `active_run_index.rs:56` *"`readProcessTerminal(asyncDir)?.state ===
"observed"` is the OTHER disjunct of the reader's staleness rung"* — is a citation of **upstream's**
reader (`async-status.ts:575`), and the very next line says *"cyrup's `RunStatus` carries no
process-terminal record at all, so `read_live_active_run_ids` applies the age disjunct alone"*.
It is a true statement of absence, not evidence of a reader. **Ledger row VL-S4
(`docs/gap-analysis/PARITY-GAPS.md:1445-1448`) is correct and stays open**; `debug.run` neither
closes nor narrows it.

**What cyrup DOES have** — and what the dump may therefore print, labelled as what it is:

| datum | where | how it is produced |
|---|---|---|
| `RunStatus::pid: Option<u32>` | `records.rs:42` | the run's own recorded runner pid |
| `ActiveAsyncCapacityOwner::runner_pid` / `runner_started_at` | `key.rs:140,:143` | bound by `mark_started(pid)` at `extension/executor/background.rs:865-866`, "the first statement after the spawn is CONFIRMED" |
| `reconcile::check_pid_liveness(pid) -> Liveness { Alive, Dead, Unknown }` | `background/reconcile.rs:73-80, :108-114` | `kill(pid, 0)`; `ESRCH` ⇒ `Dead`; anything else ⇒ `Unknown`, **never collapsed to dead** (`:69`) |
| `runner_release_verdict` proof rung | `inspect.rs:223-241` | "runner pid N is confirmed gone and the run is terminal" — §D3's stand-in for `processTerminal.state === "observed"` |

So the honest port of upstream's two process-terminal lines is ONE line saying the record is not
kept plus ONE line reporting the pid probe. **Do not print `Process terminal file: <dir>/process-
terminal.json`** (`run-status.ts:95`) — upstream prints that path unconditionally, but in cyrup it
names a file no code path has ever written, which is exactly the diagnostic-tool lie the directive
forbids.

### 1. Upstream, pinned at v0.68.0 — quoted with line numbers

**`src/runs/background/run-status.ts`** (786 lines; imports `readProcessTerminal, sanitizeProcessTerminal`
at `:18`, `inspectActiveAsyncCapacityOwner` + `ActiveAsyncCapacityInspection` at `:13`,
`resolveAsyncRunLocation` at `:20`)

- `:47-50` `formatProcessTerminal(value)` → `"missing"` | `` `${state}${reason ? ` (${reason})` : ""}${runnerProcessInstanceId ? ` · runner ${id}` : ""}` ``
- `:52-58` `debugProcessTerminal(asyncDir, status)` → `{ sidecar: readProcessTerminal(asyncDir, expected),
  overlay: sanitizeProcessTerminal(status.processTerminal, expected, "<asyncDir>/status.json") }` with
  `expected = { runId: status.runId, runnerProcessInstanceId: status.processTerminal?.runnerProcessInstanceId }`.
- `:60-72` `formatCapacityOwner(inspect)`:
  ```
  no owner → [`Active capacity: ${release.state} — ${release.reason}`]
  else     → `Active capacity: ${release.state} — ${release.reason}`
             `Capacity owner: ${relation} slot ${owner.slot}, ${owner.kind}, generation ${owner.generation}`
             `Capacity session: ${owner.ownerSessionId}`
             owner.sourceRunId          ? `Capacity source run: ${owner.sourceRunId}`
             `Capacity async dir: ${owner.asyncDir}`
             owner.runnerProcessInstanceId ? `Capacity runner: ${owner.runnerProcessInstanceId}`
             owner.runnerStartedAt !== undefined ? `Capacity runner started: ${new Date(...).toISOString()}`
  ```
- `:74-85` `formatWorkflowDebug(status)`: `:75` returns `[]` unless `mode === "workflow" ||
  parentWorkflowRunId || workflowKey || lane`; then `Workflow parent: <id> (<key>)?`,
  `Workflow children: <steps.length>` (workflow mode only), `Lane: <key> (<mode>)?`, and per step
  `:82`: ``  `${i+1}. key ${step.workflowKey ?? "n/a"} · ${runStatusStepDisplayName(step)} · ${step.status} · async ${step.async === undefined ? "unknown" : step.async ? "yes" : "no"}${step.runId ? ` · run ${step.runId}` : ""}${step.lane ? ` · lane ${step.lane.key}` : ""}${step.worktreePath ? ` · worktree … · branch … · provider …` : ""}` ``
  (`runStatusStepDisplayName` `:166-168` = `sessionName.trim() || (label ? "label (agent)" : agent)`).
- `:87-108` `formatRunLifecycleDebug({status, asyncDir, sidecarProcessTerminal, overlayProcessTerminal, capacity})`:
  ```
  "Run lifecycle debug"
  `Run: ${status.runId}`
  `Dir: ${asyncDir}`
  `Status file: ${asyncDir}/status.json`
  status.workflowReceiptPath ? `Workflow receipt: ${…}`
  `Process terminal file: ${asyncDir}/process-terminal.json`
  `Session: ${status.sessionId ?? "unknown"}`
  `State: ${status.state}`
  `Mode: ${status.mode}`
  status.parentWorkflowRunId ? `Workflow parent: …`
  status.workflowKey ? `Workflow key: …`
  status.lane ? `Lane: …`
  `Status process terminal: ${formatProcessTerminal(overlay)}`
  `Sidecar process terminal: ${formatProcessTerminal(sidecar)}`
  ...formatCapacityOwner(capacity)
  ...formatWorkflowDebug(status)
  ```
  joined with `"\n"`, `undefined` filtered.
- The four arms:
  - `:406-410` — `if (params.action === "debug.run") location = resolveAsyncRunLocation(params, asyncDirRoot, resultsDir);` — **bypasses** `resolveSubagentRunId` (foreground/nested) entirely.
  - `:463-468` — `!asyncDir && !resultPath` → `"Async run not found. Provide id or dir."` (isError).
  - `:472-475` — `diskStatus = readStatus(asyncDir)`; `reconciliation = reconcileAsyncRun(asyncDir, …)` (the FULL stale-run reconciler, not a narrow read).
  - `:485-492` — `!status && diskStatus.displayDismissedAt !== undefined` and `debug.run` → dump over **`diskStatus`**, `details: { mode:"single", results:[], workflowReceiptPath?, lifecycleStatus?: { processTerminal: sidecar ?? overlay } }`.
  - `:512-519` — `status` present and `debug.run` → same dump over the reconciled `status`; capacity via `inspectActiveAsyncCapacityOwner({ runId, sessionId, asyncDir }, { rootDir, liveWorkflowRunIds: new Set(deps.state?.workflowControllers?.keys()), abandonedSlotReleaseAfterMs })`.
  - `:714-721` — `resultPath` only and `debug.run` → `"Run lifecycle debug needs an async run directory with status.json."` (isError).
  - `:781-785` — terminal fall-through `"Status file not found."` (isError).

**`src/runs/foreground/subagent-executor.ts`**
- `:213` `MUTATING_MANAGEMENT_ACTIONS` — 31 members; **`debug.run` is absent** (child-open, read-only).
- `:6515-6538` the shared arm:
  ```
  if (action === "status" || action === "debug.run") {
    …targetRunId = id ?? runId; hasDirectoryTarget = Boolean(dir);
    targetLabel = action === "debug.run" ? "Debug run" : formatStatusTargetLabel(…)   // :6519
    if (action === "debug.run") {
      if (!targetRunId && !hasDirectoryTarget) return … "action='debug.run' requires id, runId, or dir."   // :6532-6533
      if (paramsWithResolvedCwd.view)          return … "action='debug.run' does not support status views."   // :6535-6536
      return withBudget(inspectSubagentStatus(paramsWithResolvedCwd, { state, nested, sessionRoots, abandonedSlotReleaseAfterMs }))  // :6538
    }
  ```
  Order: target check BEFORE view check. `withBudget` (`:6520-6528`) prefixes `"Debug run\n"` to the
  first text block and appends spawn-budget status — cyrup ports neither `formatStatusTargetLabel`
  (`:2652-2663`) nor `withSpawnBudgetStatus` for `status` (`grep -rn 'Status target' src` → 0 outside
  `tui/fleet.rs:1093`, a different thing), so the label prefix is a **pre-existing status-side delta**;
  do not add it for `debug.run` alone (see §4).

**`src/shared/types.ts:2801`** `SUBAGENT_ACTIONS` — `… "project.close", "status", "debug.run",
"grant-spawn-budget", "interrupt", …` → cyrup's index is **immediately after `"status"`**.

**`src/runs/background/process-terminal.ts`** (310 lines) — recorded here so the executor never has
to re-derive that it is unported: `:63-69` the two sidecar paths; `:119-132`
`initializeProcessTerminal` (writer of `state:"pending"` + the candidate); `:180-188`
`sanitizeProcessTerminal`; `:190-199` `readProcessTerminal` (ENOENT → `undefined`, anything else →
`unknownProof(…, "proof-write-failed")`); `:224-241` `overlayStatus` (writes `status.processTerminal`
and per-step `step.processTerminal` back into `status.json`); `:243-310` `finalizeProcessTerminal`
(writer of `observed`/`unknown`, `releaseActiveRunIndex` on observed at `:303`, and the
`subagent.run.process_terminal` event at `:305`). **Zero of this exists in cyrup.**

**`src/runs/background/active-async-capacity.ts`** — `:15-29` `ActiveAsyncCapacityOwner` (`slot`,
`runId`, `sourceRunId?`, `generation`, `kind: "runner"|"workflow"`, `asyncDir`, `reservedAt`,
`runnerProcessInstanceId?`, `runnerStartedAt?`); `:58-61` verdict union `releasable|retained|not-owned`
each with `reason`; `:63-68` `ActiveAsyncCapacityInspection { owner?, relation: "current"|"source"|"none",
slotDir?, release }`; `:315-339` `inspectActiveAsyncCapacityOwner`.

**`src/runs/background/async-resume.ts:223-259`** `resolveAsyncRunLocation(params, asyncDirRoot, resultsDir)`
— **no session filter**; `dir` form asserts inside the async root (`:229`) and throws
`Async run id '<id>' does not match directory '<basename>'.` on mismatch (`:231-233`); id form:
exact dir/result (`:238-247`), then prefix with `MIN_SAFE_ASYNC_RUN_PREFIX_LENGTH` (`:248-250`) and
the ambiguity throw (`:255-257`).

### 2. `ActiveAsyncCapacityInspection` → `background/active_async_capacity/inspect.rs` (field map)

| upstream (`active-async-capacity.ts`) | cyrup (`inspect.rs` unless noted) |
|---|---|
| `ActiveAsyncCapacityInspection` `:63-68` | `pub struct ActiveAsyncCapacityInspection { owner: Option<ActiveAsyncCapacityOwner>, relation: CapacityRelation, slot_dir: Option<PathBuf>, release: ActiveAsyncCapacityReleaseVerdict }` `:119-130` |
| `relation: "current"\|"source"\|"none"` | `enum CapacityRelation { Current, Source, None }` `:109-117` |
| `release.state` / `release.reason` | `enum ActiveAsyncCapacityReleaseVerdict { Releasable{reason, evidence}, Retained{reason}, NotOwned{reason} }` `:64-84`; `is_releasable()` `:88`; **no `state_word()`/`reason()` accessor yet — add both (§4)** |
| `owner.slot` / `.kind` / `.generation` / `.ownerSessionId` / `.sourceRunId` / `.asyncDir` / `.reservedAt` | `key.rs:117` `slot: u32` / `:129` `kind: ActiveAsyncCapacityKind { Runner, Workflow }` (`key.rs:65-72`) / `:127` `generation: u32` (fresh claim = `0`, `claim.rs:479`; transfer bumps `:611`) / `:111` `owner_session_id: SessionId` / `:124` `source_run_id: Option<RunId>` / `:132` `async_dir: PathBuf` / `:134` `reserved_at: i64` |
| `owner.runnerProcessInstanceId?` | **does not exist** — `key.rs:140` `runner_pid: Option<u32>` is the §D3 substitute (`key.rs:93-99` records why) |
| `owner.runnerStartedAt?` | `key.rs:143` `runner_started_at: Option<i64>` (epoch ms) |
| `inspectActiveAsyncCapacityOwner({runId, sessionId, asyncDir}, {rootDir, liveWorkflowRunIds, abandonedSlotReleaseAfterMs})` `:315` | `pub async fn inspect_active_async_capacity_owner(run_id: &RunId, session_id: Option<&SessionId>, async_dir: Option<&Path>, options: &CapacityOptions) -> io::Result<ActiveAsyncCapacityInspection>` `:488-537`; `session_id: None` scans every pool (`:480-482`) |
| the three options | `CapacityOptions` (`config.rs:117-215`): `new(root_dir)`, `.with_live_workflow_run_ids(HashSet<RunId>)`, `.with_abandoned_slot_release(AbandonedSlotRelease)`, `.with_pid_liveness(Arc<dyn Fn(u32)->Liveness>)` (test seam) |
| production options builder | `SubagentExecutor::capacity_options(&cfg, live_workflow_run_ids)` `extension/executor/background.rs:392-406` — root from `active_async_capacity_root_in(&cfg.roots)` (`artifact_roots.rs:384` = `roots.run_scratch()/<CAPACITY_SUBDIR>`), policy resolved ONCE; live set from `self.live_workflow_run_ids()` (`workflow_controllers.rs:148`) |

Public re-exports: `background/active_async_capacity/mod.rs:93-101` (`ActiveAsyncCapacityInspection`,
`CapacityRelation`, `ActiveAsyncCapacityReleaseVerdict`, `inspect_active_async_capacity_owner`,
`ActiveAsyncCapacityKind`, `ActiveAsyncCapacityOwner`, `read_owner`, `slot_dir`, `session_pool_dir`);
`background/mod.rs:39` `pub mod active_async_capacity`.

### 3. What cyrup already has — everything else the dump needs (grep-verified)

- **Dispatch seam.** `extension/tool/routing.rs:1151` — `"status" | "interrupt" | "stop" | "dismiss" |
  "resume" | "steer" | "append-step" => self.route_control_action(action, p, cwd)`; the `"status"`
  arm at `:2177-2196` reads `p.id.or(p.run_id)`, `p.dir`, `p.view/lines/index` and calls
  `control_status_view` (`extension/executor/status.rs:327-334`). Result mapping at `:2297-2305`:
  `Ok(text)` → `ToolResult { content: [text], details: {"mode":"management"} }`, `Err(msg)` →
  `ToolError`. The authority consult at the top of `route_control_action` (`:2129-2140`) maps only
  `stop|steer|schedule.create|worktree.discard` (`registration/authority.rs:92-104`) — `debug.run`
  falls to `_ => None`, ungated, exactly as upstream leaves it.
- **Params.** `extension/tool/params.rs:85` `SubagentToolParams { id: Option<String> :97, run_id :98,
  dir :99, index :100, child_id :110, view :115, lines :119 }`. No new schema property is needed;
  upstream's `dir` description is `"Async directory for status/control."` (`schemas.ts:295-297`) and
  never names `debug.run`, so `schema.rs:386` stays.
- **The advertised list.** `extension/tool/text.rs:239-370` `SUBAGENT_ACTIONS` — `"status"` at
  `:281`, `"grant-spawn-budget"` at `:285`. The schema enum is DERIVED from it (`schema.rs:359`).
  Pin tests that go red on insertion and must be updated together: `schema.rs:913-1012` (whole-list
  `assert_eq!`, `"status"` at `:956`), `registration/guide.rs:292` (packaged `tool-reference.md`
  must contain the token), `registration/tool_description.rs:754` (every `family.verb` token in
  the compact description must be advertised — only bites if the description is edited).
- **Location resolution.** `background/run_id_resolver.rs:183-200` `resolve_async_run_id(id,
  async_root, results_dir, current_session: Option<&SessionId>) -> Result<Option<AsyncRunLocation>,
  ResolveRunIdError>`; `AsyncRunLocation { async_dir: Option<PathBuf>, result_path: Option<PathBuf>,
  resolved_id: RunId }` (`:23-30`) — `async_dir: None` with `result_path: Some` IS upstream's
  `:714` "result only" state. Pass **`None`** for the session: upstream's `resolveAsyncRunLocation`
  has no session filter, and `location_belongs_to` (`:152-158`) is permissive on `None`.
  `ResolveRunIdError` (`:38-52`) carries pi's exact ambiguity sentence.
- **Reconciliation.** Upstream `:475` is the FULL reconciler. cyrup's twin is
  `background::reconcile::reconcile_now(&RunPaths, spawn_confirmed_at: Option<SystemTime>) ->
  io::Result<ReconcileOutcome { status, action }>` (`reconcile.rs:331-340`, `:148-154`): a
  `Running` status whose pid probes `Dead` is synthesised to `Failed` with a diagnosis
  (`:284-321`); an absent `status.json` outside the grace window is a synthesised `Failed`
  (`:252-267`, `:447-461`); a `ResultFile` repairs in place. `resume_tracking` already calls it
  in production (`extension/executor/status.rs:77`). (`run_status::reconcile_by_id/_by_dir`
  `:712-741` use the NARROW `reconcile_before_control_op` `background/control.rs:206` — fine for
  `status`, but `debug.run` is the tool for "why is this stuck", so it wants the probe.)
- **Status fields for the header block.** `records.rs`: `run_id :3(rel)`, `session_id:
  Option<SessionId> :21`, `mode :32`, `state :34`, `pid: Option<u32> :42`, `steps :80`,
  `display_dismissed_at :105`, `workflow_children :130`, `workflow_receipt_path: Option<PathBuf> :137`.
  Labels: `run_status::run_state_label` (`run_status.rs:45`, `pub(crate)`), `run_mode_label`,
  `step_state_label` (`:65`). ISO timestamps: `crate::time::format_iso8601_millis(i64)` (`time.rs:47`).
  `SessionId`/`RunId` both `Display` (`identity/session_id.rs:71`, `background/run_id.rs:71`).
- **Display-dismissed arm.** `reconcile_before_control_op` erases the marker when a `ResultFile`
  proves the run (`control.rs:222-230`); `run_status::inspect_paths` (`:566-575`) renders
  `format_display_dismissed_status` when it survives. Upstream `:485-492` renders the DUMP (over
  `diskStatus`) for `debug.run` in that state, not the dismissed report — so `debug.run` must not
  route through `inspect_paths`; it reads the status and dumps regardless of `display_dismissed_at`.

### 4. The Rust shape

**New module `crates/cyrup-ext-subagents/src/background/run_lifecycle_debug.rs`** (add `pub mod
run_lifecycle_debug;` to `background/mod.rs`). Module doc cites `run-status.ts:47-108` @v0.68.0 and
says, in one paragraph with the grep from §0, why the two process-terminal lines are one
not-recorded line plus the pid probe (cite VL-S4 `PARITY-GAPS.md:1445` and §D3 `mod.rs:60-72`).

```rust
/// pi `run-status.ts:87-108`'s input, over the data cyrup has.
pub struct RunLifecycleDebug<'a> {
    pub status: &'a RunStatus,
    pub paths: &'a RunPaths,
    pub runner: RunnerLiveness,
    pub capacity: &'a ActiveAsyncCapacityInspection,
}

/// [CYRUP-DELTA] stands where upstream's `sidecar`/`overlay` `ProcessTerminal` pair stands
/// (`run-status.ts:52-58`): cyrup writes no `process-terminal.json` and no status overlay
/// (VL-S4), so the only process-terminal fact this build can report is the pid probe §D3 already
/// substitutes for the proof in `active_async_capacity::inspect::runner_release_verdict`.
pub enum RunnerLiveness {
    NotRecorded,                                   // status.pid == None
    Probed { pid: u32, liveness: Liveness },       // check_pid_liveness(pid)
}
impl RunnerLiveness {
    pub fn probe(status: &RunStatus, probe: impl Fn(u32) -> Liveness) -> Self;   // caller passes check_pid_liveness
}

pub fn format_capacity_owner(inspect: &ActiveAsyncCapacityInspection) -> Vec<String>;   // :60-72
pub fn format_workflow_debug(status: &RunStatus) -> Vec<String>;                       // :74-85
pub fn format_run_lifecycle_debug(input: &RunLifecycleDebug<'_>) -> String;            // :87-108

pub const DEBUG_RUN_REQUIRES_TARGET: &str = "action='debug.run' requires id, runId, or dir.";
pub const DEBUG_RUN_NO_VIEWS: &str = "action='debug.run' does not support status views.";
pub const DEBUG_RUN_NEEDS_STATUS_DIR: &str = "Run lifecycle debug needs an async run directory with status.json.";
```

The rendered lines, in upstream's order, over cyrup's data — this IS the contract the test pins:

```
Run lifecycle debug
Run: <run_id>
Dir: <paths.run_dir>
Status file: <paths.status>
Workflow receipt: <workflow_receipt_path>                       (only when Some)
Session: <session_id | unknown>
State: <run_state_label>
Mode: <run_mode_label>
Process terminal: not recorded — this build writes no process-terminal.json (VL-S4); runner pid liveness stands in for the proof
Runner pid: <pid> (<alive|dead|unknown>)          |  Runner pid: not recorded
Active capacity: <releasable|retained|not-owned> — <reason>
Capacity owner: <current|source> slot <n>, <runner|workflow>, generation <g>     (owner present)
Capacity session: <owner_session_id>
Capacity source run: <id>                                       (when Some)
Capacity async dir: <owner.async_dir>
Capacity runner pid: <runner_pid>                               (when Some — replaces upstream's `Capacity runner: <runnerProcessInstanceId>`, which cyrup does not mint; key.rs:93-99)
Capacity runner started: <iso8601 of runner_started_at>         (when Some)
Workflow children: <steps.len()>                                (mode == Workflow)
  <i>. key <workflow_key | n/a> · <session_name.trim() | agent> · <step_state_label>[ · run <run_id>]
```

Lines upstream prints that are **deliberately absent, each because the datum does not exist here**
(state each reason in the module doc, once, with the file:line):
`Process terminal file:` (`:95`; would name a never-written file), `Status process terminal:` /
`Sidecar process terminal:` (`:102-103`; no overlay, no sidecar), `Workflow parent:` /
`Workflow key:` / `Lane:` (`:99-101`, `:77`, `:79`; `RunStatus` has no `parent_workflow_run_id`,
`workflow_key` or `lane` — `async_retention/policy.rs:367` already records this; upstream itself
omits these when undefined, so the output is upstream's own for a status where they are undefined),
the per-step `async yes|no|unknown` term (`:82`; no `StepStatus::async` — `inspect.rs:335-346`
Q5 explains it is constant-false by construction, and the `run <id>` suffix is the witness that
matters), the per-step `lane`/`worktree`/`branch`/`provider` tail (`:82`; no such `StepStatus`
fields). `formatWorkflowDebug`'s gate (`:75`) becomes `has_workflow_reference`'s three disjuncts
(`policy.rs:374-378`): `mode == Workflow || workflow_children.is_some() || steps.any(workflow_key)`.

Small accessors to add so the formatter has no private `match`es to duplicate: on
`ActiveAsyncCapacityReleaseVerdict` (`inspect.rs:86`) `pub fn state_word(&self) -> &'static str`
(`"releasable"|"retained"|"not-owned"`, upstream's literal strings) and `pub fn reason(&self) -> &str`;
on `CapacityRelation` `pub fn as_str` (`"current"|"source"|"none"`); on `ActiveAsyncCapacityKind`
(`key.rs:65`) `pub fn as_str` (`"runner"|"workflow"`, the serde spelling `owner.json` already uses).
Promote `liveness_word` (`inspect.rs:573-579`) to `pub(crate)` rather than copying it.

**Executor method** — `extension/executor/status.rs`, next to `control_status_view`:

```rust
pub async fn control_debug_run(&self, cwd: &Path, id: Option<&str>, dir: Option<&str>)
    -> Result<String, String>
```
1. roots → `default_async_root_in` / `default_results_dir_in` (same two lines as `:336-338`).
2. Location — `dir` form: basename → `RunId::from_token`, `RunPaths::for_run(dir.parent(), results, id)`
   (the shape `reconcile_by_dir` `:729-741` uses); id form: `resolve_async_run_id(id, &async_root,
   &results_dir, None)` mapped `Err(e) → Err(e.to_string())`, `Ok(None)` → `Err("Async run not found.
   Provide id or dir.")`, `Ok(Some(loc)) if loc.async_dir.is_none()` → `Err(DEBUG_RUN_NEEDS_STATUS_DIR)`
   (upstream `:714-721`), else `RunPaths::for_run(&async_root, &results_dir, &loc.resolved_id)`.
3. `reconcile_now(&paths, None)` — `Err(NotFound)` → `Err("Status file not found.")` (upstream `:781-785`);
   other I/O → `Err(e.to_string())`.
4. `options = Self::capacity_options(&cfg, self.live_workflow_run_ids())` — `cfg` is the same
   `config_snapshot()` already taken for roots; `inspect_active_async_capacity_owner(&status.run_id,
   status.session_id.as_ref(), Some(&paths.run_dir), &options).await.map_err(|e| e.to_string())?`
   — the three identities upstream passes at `:515`.
5. `RunnerLiveness::probe(&status, crate::background::reconcile::check_pid_liveness)`.
6. `Ok(format_run_lifecycle_debug(&RunLifecycleDebug { .. }))`.

**Routing** — `routing.rs:1151` add `"debug.run"` to the control band; in `route_control_action`
add the arm **before** `"status"` (`:2177`):
```rust
"debug.run" => {
    let target = p.id.as_deref().or(p.run_id.as_deref());          // pi :6517 `id ?? runId`
    if target.is_none() && p.dir.is_none() { return Err(ToolError::new(DEBUG_RUN_REQUIRES_TARGET)); }   // :6532
    if p.view.is_some()                     { return Err(ToolError::new(DEBUG_RUN_NO_VIEWS)); }         // :6535
    self.executor.control_debug_run(cwd, target, p.dir.as_deref()).await
}
```
Details stay `{"mode":"management"}` via the existing mapping at `:2297-2305`; upstream's
`details.workflowReceiptPath` is rendered as the `Workflow receipt:` text line anyway, and
`details.lifecycleStatus.processTerminal` is the sidecar — omitted for the §0 reason. No
child-safe gate: not in `:213`, and there is no no-target branch to gate.

**Advertise** — `text.rs`: insert `"debug.run",` immediately after `"status",` (`:281`) with a
two-line note citing `types.ts:2801` and the dispatch arm. Update the whole-list pin in
`schema.rs:956` and add a `| \`debug.run\` | control | Run-lifecycle diagnostic dump for one async
run (\`id\`/\`runId\`/\`dir\`); refuses \`view\` |` row under `status` in
`resources/docs/tool-reference.md:27`.

**Notes whose premise this change falsifies — rewrite, do not append to:**
- `text.rs:165-170` — currently says `debug.run` is "an unported member" of `SUBAGENT_ACTIONS`.
  After landing: it IS dispatched; keep only the true half (absent from `:213`, read-only,
  child-open, dispatched by the shared `status || debug.run` arm at `:6515`).
- `schema.rs:988-990` — "cyrup omits `inspector.*`/`project.*`/`debug.run`" → drop `debug.run`.
- Docs that count it as missing: `docs/gap-analysis/PARITY-GAPS.md:36,:1547`,
  `docs/gap-analysis/00-residual-ledger.md:144,:310`, `docs/gap-analysis/09-cyrup-ext-subagents.md:234,:681`.

**Notes whose premise stays TRUE — leave byte-for-byte** (each re-grepped in §0):
`active_run_index.rs:56-59,:359-363`, `inspect.rs:147-177,:332-333,:433-438`, `key.rs:93-99`,
`mod.rs:60-72`, `ping.rs:60-61,:66`, `doctor.rs:1019-1023`, `async_retention/policy.rs:367-373`.
`debug.run` adds ONE new `[CYRUP-DELTA]` (on `RunnerLiveness`) and it must cite these, not restate them.

### 5. Production call sites (the chain a reader can follow end to end)

`SubagentTool::execute` (`extension/tool/mod.rs:176`) → `normalize_public_subagent_execution` →
`route_action("debug.run", …)` (`routing.rs:1085`, band at `:1151`) → `route_control_action`
(`:2123`; authority consult `:2129` → `None`) → new `"debug.run"` arm → `SubagentExecutor::
control_debug_run` (`executor/status.rs`) → `resolve_async_run_id` (`run_id_resolver.rs:183`) →
`reconcile_now` (`reconcile.rs:331`) → `capacity_options` (`background.rs:392`) →
`inspect_active_async_capacity_owner` (`inspect.rs:488`) → `check_pid_liveness` (`reconcile.rs:108`)
→ `format_run_lifecycle_debug`. Both registrations share the one `SubagentTool` (`mod.rs:128`),
so the child-safe tool reaches it too, as upstream's does.

### 6. Reachability test — and why it FAILS if gutted

**`crates/cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs`**, registered as
`mod debug_run_lifecycle_integration;` in `tests/subagents/main.rs` (block at `:117-132`; bump the
"38 files today" header at `main.rs:1-5`). Env per the task: `CYRUP_IT_BIN_DIR` → the existing
`target/debug/build/cyrup-it-*/out/it-bins/debug`, AWS vars unset. Everything under a per-test
`tempfile::tempdir()`; roots via `Roots::sandboxed(home)` (pattern: `fleet_inspector_integration.rs:194-208`).

**Test A — `debug_run_prints_the_real_capacity_slot_for_a_real_async_run`.** Every datum real:
1. `SubagentExtensionConfig { spawn_command: Some(SpawnCommand { binary: support::bins::subagent_fixture(),
   base_args: vec![] }), max_active_async_runs_per_session: Some(1), roots: Roots::sandboxed(home), .. }`
   → `SubagentsExtension::with_config_and_cwd(cfg, cwd)`; `ext.executor().set_host_services(Arc::new(host))`
   where `host.session_id()` returns `"debug-session"` (a `HostServices` impl like
   `fleet_inspector_integration.rs:57-69`). With no script the fixture exits 0 at once — the hop-1
   process is genuinely spawned and confirmed (`subagent_persona_and_depth_integration.rs:1155-1230`
   is the precedent that `spawn_command` reaches the detached path).
2. `let run_id = ext.executor().spawn_background_steps(cwd, BackgroundStepsSpec{ steps: [worker], mode: Single, transfer_from: None, .. }).await?`
   — this is the PRODUCTION claim path: `acquire` (`background.rs:624-634`) writes `slot-0/owner.json`
   under `active_async_capacity_root_in(&roots)/<IndexSegment("debug-session")>`, `mark_started(pid)`
   (`:865`) binds `runner_pid`/`runner_started_at`.
3. Read the owner back with `active_async_capacity::read_owner(&slot_dir(&session_pool_dir(..), 0))`
   → `runner_pid = Some(pid)`; poll `check_pid_liveness(pid) == Dead` (bounded, e.g. 10 s / 50 ms).
4. Write the terminal record the runner would have written into the run dir the spawn created
   (`RunStatus::queued(run_id, Single, Some(pid))` → `Running` → `Complete`, `session_id =
   "debug-session"`, one `StepStatus` `Complete`) with `write_atomic_json(&paths.status, ..)` —
   the exact fixture shape `fleet_inspector_integration.rs:169-191` and `background.rs:1225-1243`
   use. (The fixture-as-runner does not write `status.json`; the slot, dir, pid and session are all
   the production spawn's own.)
5. `ext.subagent_tool().execute(ToolCallId::from("debug-run"), json!({"action":"debug.run","id": run_id}), CancelToken::new(), Box::new(|_| {}))`
   → text. Assert, each on its own line:
   `Run lifecycle debug`; `Run: <run_id>`; `Dir: <async_root>/<run_id>`; `Status file: …/status.json`;
   `Session: debug-session`; `State: complete`; `Mode: single`;
   `Process terminal: not recorded`; `Runner pid: <pid> (dead)`;
   `Active capacity: releasable — runner pid <pid> is confirmed gone and the run is terminal`
   (the literal from `inspect.rs:228-230`); `Capacity owner: current slot 0, runner, generation 0`;
   `Capacity session: debug-session`; `Capacity async dir: <async_root>/<run_id>`;
   `Capacity runner pid: <pid>`; `Capacity runner started: <iso>` (prefix match on `Capacity runner started: 20`).
   And assert the text does **not** contain `Process terminal file:` nor `process-terminal.json`
   followed by nothing — i.e. `!text.lines().any(|l| l.starts_with("Process terminal file:"))`.
6. Second call with `"dir": "<async_root>/<run_id>"` and no id → identical `Capacity owner:` line
   (the `:409` `dir` form).

Why gutting fails it: dropping the capacity section (the DoD mutation) removes every `Capacity …`
line and `Active capacity:` → five asserts fail; routing `debug.run` to `control_status_view` loses
the `Run lifecycle debug` header and prints `Progress:` instead; short-circuiting
`inspect_active_async_capacity_owner` to the `None` relation prints
`Active capacity: not-owned — no active-capacity slot records this run` ≠ the asserted literal;
replacing the probe with a constant prints `(alive)`/`(unknown)` ≠ `(dead)`; reintroducing
upstream's `Process terminal file:` line trips the negative assert.

**Test B — refusals, in-crate `extension/tool/routing_tests.rs` via `scoped_tool`/`dispatch_tool`
(`testsupport.rs:225-240`)**: `{action:"debug.run"}` → `Err` = `DEBUG_RUN_REQUIRES_TARGET`;
`{action:"debug.run", id:"x", view:"fleet"}` → `DEBUG_RUN_NO_VIEWS` (and prove ORDER: with neither
target nor view-less… i.e. `{action:"debug.run", view:"fleet"}` alone yields the TARGET sentence,
`:6532` before `:6535`); a result-only run (write `<results_dir>/<id>.json` = `{}`; no run dir) →
`DEBUG_RUN_NEEDS_STATUS_DIR`; unknown id → `Async run not found. Provide id or dir.`; a `dir` whose
run has neither file → `Status file not found.`. Also assert `subagent_actions()` contains
`"debug.run"` at `position("status") + 1` (the advertise-vs-dispatch invariant every verb in this
crate pins, e.g. `routing_tests.rs:2703-2712`).

**Test C — honesty pin, unit test in `run_lifecycle_debug.rs`**: a `RunStatus` with `pid: None`
renders `Runner pid: not recorded`; with `pid: Some(7)` and an injected probe returning `Unknown`
renders `Runner pid: 7 (unknown)` — never `dead` (the `reconcile.rs:69` rule); a workflow-mode
status with two steps renders `Workflow children: 2` and the two `  N. key … · … · …` lines and a
single-mode status renders no workflow block (`:75`'s gate).

### 7. Files touched (implementation)

- `crates/cyrup-ext-subagents/src/background/run_lifecycle_debug.rs` — NEW (types, three formatters, three refusal consts, unit tests)
- `crates/cyrup-ext-subagents/src/background/mod.rs` — `pub mod run_lifecycle_debug;`
- `crates/cyrup-ext-subagents/src/background/active_async_capacity/inspect.rs` — `state_word()`/`reason()` on the verdict, `as_str()` on `CapacityRelation`, `liveness_word` → `pub(crate)`
- `crates/cyrup-ext-subagents/src/background/active_async_capacity/key.rs` — `ActiveAsyncCapacityKind::as_str()`
- `crates/cyrup-ext-subagents/src/extension/executor/status.rs` — `control_debug_run`
- `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` — `:1151` band + new arm in `route_control_action`
- `crates/cyrup-ext-subagents/src/extension/tool/text.rs` — `"debug.run"` after `"status"`; rewrite `:165-170`
- `crates/cyrup-ext-subagents/src/extension/tool/schema.rs` — `:956` pin; rewrite `:988-990`
- `crates/cyrup-ext-subagents/resources/docs/tool-reference.md` — the `debug.run` row
- `crates/cyrup-ext-subagents/src/extension/tool/routing_tests.rs` — Test B
- `crates/cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs` — NEW (Test A); `tests/subagents/main.rs` registration + header count
- `docs/gap-analysis/PARITY-GAPS.md:36,:1547`, `00-residual-ledger.md:144,:310`, `09-cyrup-ext-subagents.md:234,:681` — count `debug.run` as landed (50 → 51 by this verb alone; VL-S4 row untouched and still open)

### 8. Anchors in the seed that are wrong (re-verified)

- **`background/run_status.rs:251` "carries `parent_workflow_run_id` on `RunStatus`"** — WRONG.
  `:251` is `LiveWorkflowControlCandidate::parent_workflow_run_id` (`:245-253`), the foreground-
  control projection, not `RunStatus`. `RunStatus` (`records.rs:225-381`) has no
  `parent_workflow_run_id`, no top-level `workflow_key`, no `lane`; `async_retention/policy.rs:367`
  states exactly that. Likewise `workflow.rs:886`, `workflow_steering.rs:420`, `workflow.rs:1743`
  thread it on control/candidate entries, and `fleet_view.rs:394` is `ForegroundFleetEntry`.
  Consequence for this verb: the `Workflow parent:` / `Workflow key:` / `Lane:` lines are not
  renderable from a status file and are omitted (§4), not "checked".
- **"`RunStatus` already carries … `lane` (#143)"** — no `lane` field anywhere on `RunStatus` or
  `StepStatus` (`grep -n 'pub lane' records.rs` → 0; the only `lane` hits are doc text at `:108,:368`).
- **`active_run_index.rs:56` "implies a reader exists"** — it does not; see §0.
- Seed line 87 "BOTH process terminals" — in cyrup there are zero; see §0.
- `subagent-executor.ts` lives at `src/runs/foreground/subagent-executor.ts` at v0.68.0 (not
  `src/extension/`); `MUTATING_MANAGEMENT_ACTIONS` is at `:213` as stated.

### 9. Out of scope, recorded so nobody widens this verb into them

VL-S4 (the process-terminal record) and VL-S3 (session lease) stay open, unchanged. The missing
`Status target:`/spawn-budget prefix on `status` is a pre-existing delta and is not fixed here.
`reconcile_by_id`'s narrow reconciler for `status` is not changed here either — `debug.run` simply
uses the full one because upstream does (`:475`).

---

## [AUG — children.list]

Research pass, 2026-09-19, on branch `claude/subagents-verbs` at `fccc3e5`. Upstream read ONLY via
`git -C tmp/pi-subagents show v0.68.0:<path>`. Every claim below was re-grepped against the tree.

### 0. The seed spec's premise is half wrong, and the half that is wrong changes the design

The seed says *"cyrup writes async runs to disk"* and *"the producer already exists:
`workflow.rs:886` writes `parent_workflow_run_id` … `run_status.rs:251` carries it on `RunStatus`"*.
Verified:

| Claim | Verdict | Evidence |
|---|---|---|
| `run_status.rs:251` carries `parent_workflow_run_id` on `RunStatus` | **WRONG** | `:251` is a field of `LiveWorkflowControlCandidate` (`run_status.rs:245-251`), an IN-MEMORY foreground control projection. `RunStatus` (`records.rs:225-380`) has no such field; `async_retention/policy.rs:367` records exactly that: *"[CYRUP-DELTA] `RunStatus` has **no `parent_workflow_run_id` and no top-level `workflow_key`**"*. That delta is TRUE and stays. |
| `workflow.rs:886` is "the producer" | **half** | `:886` sets `parent_workflow_run_id: Some(self.workflow_run_id.clone())` on a `ForegroundRunRequest` fed to `run_foreground_streaming` (`workflow.rs:855-905`). It lands on a `ForegroundControlEntry` (`foreground.rs:1011`, in-memory) — never on disk. |
| a workflow child is an async run with its own dir | **WRONG** | Every workflow child is FOREGROUND. `DetachedWorkflowChild`'s doc (`workflow.rs:159-166`): *"a foreground child writes no `ResultFile`, so nothing on disk names it"*. `grep -n spawn_background workflow.rs` → 0 hits. `routing.rs:548` still refuses `async: true` for Workflow. |
| `text.rs:252` "this build retains nothing" | **STALE**, but for a different reason than the seed gives | The WORKFLOW run is a real async-root run: `workflow_launch.rs:150-180` writes `<async_root>/<wf>/status.json` with `mode: Workflow`, `session_id`, `cwd`; `WorkflowRunHost::publish_steps` (`workflow.rs:342-368`) republishes `steps` as each child settles; the terminal write (`workflow_launch.rs:612-618, 682`) rebuilds `steps` from `workflow_step_statuses(children)`; and `runner_main/finish.rs`-style index maintenance lands it in `.terminal-runs` (the fleet reads it back at `routing_tests.rs:2545-2560`). So the settled children of a workflow ARE on disk — as **step rows of the workflow's own `status.json`**, not as sibling async runs. That is the retention. |

**Consequence.** Upstream's `listRetainedChildren` selects *async runs* carrying `parentWorkflowRunId`
with exactly one step (`retained-children.ts:84-89`). cyrup's equivalent selection is *workflow runs*
(`RunMode::Workflow`) and, within each, every terminal step row that carries a `run_id`. One upstream
"retained child run" ⇔ one cyrup `(workflow status, step index)` pair:

| upstream field (`retained-children.ts:16-27`) | cyrup source |
|---|---|
| `runId` | `step.run_id` (`StepStatus::run_id`, set by `workflow_step_statuses` `child_summary.rs:711` from `WorkflowScriptChildResult::run_id`; also by `apply_detached_child_settlement` `settlement.rs:310-312`) |
| `parentRunId?` | `status.run_id` — always `Some` here (upstream's optional covers a non-workflow retained run, which cyrup never produces) |
| `workflowKey?` | `step.workflow_key` (`child_summary.rs:710`) |
| `state` | `status.state` — `Complete \| Failed \| Paused \| Stopped` (cyrup has no `partial`; `RunState` `state.rs:70-100`) |
| `agent` | `step.agent` |
| `taskSummary` | see §4 — the step row carries no description |
| `completedAt` | `status.ended_at.unwrap_or(status.last_update)` — upstream's own `run.endedAt ?? run.lastUpdate` (`:90`) is run-level, so no step timestamp is needed |
| `resumability` | §3 |
| `tokenTotals?` | `step.usage` after §2's fix, via `usage_with_value` (`workflow_detach/children.rs:30`) |

### 1. Upstream, verbatim anchors (`src/runs/background/retained-children.ts`, 135 lines @v0.68.0)

- `:7-9` — `MAX_RETAINED_CHILDREN = 10`, `MAX_RETAINED_CHILD_CANDIDATES = 100`, `MAX_TASK_SUMMARY_LENGTH = 120`.
- `:11-14` — `RetainedChildState = "complete" | "failed" | "paused" | "stopped"`;
  `RetainedChildResumability = { state: "resumable"; sessionPath } | { state: "not-resumable"; reason }`.
- `:16-27` — `RetainedChild` (fields above).
- `:29-31` — `isRetainedChildState` also admits `"partial"` (cyrup has none).
- `:33-35` — `isTerminalStepStatus`: `complete | completed | failed | paused | stopped`.
- `:37-50` — `retainedSessionFile`: no file → `"no persisted session file"`; not `.jsonl` →
  `"persisted session file is not a .jsonl file"`; `lstatSync` then `!isFile() || isSymbolicLink()` →
  `"persisted session file is not a regular file"`; `ENOENT` → `` `persisted session file is missing: ${sessionFile}` ``;
  other → `` `persisted session file could not be inspected: ${message}` ``.
- `:52-74` — `childResumability`: `:53` stopped run/step → `"stopped run"`; `:54` `external-cli|external-job`
  runner → `"external runner"`; `:55-56` session check; `:57-65` `readAsyncRecoveryDescriptor(run.asyncDir)`:
  `undefined` → `"missing recovery descriptor"`, `sourceRunId !== run.id` → `` `recovery descriptor belongs to run ${…}` ``,
  `agent !== step.agent` → `` `recovery descriptor belongs to agent ${…}` ``, throw → `` `invalid recovery descriptor: ${…}` ``;
  `:66-72` `requiredCwd = resolveRetainedWorktreeCwd(parallelHandoffPath(run.asyncDir), run.id, step.index) ?? recoveryDescriptor.cwd ?? run.cwd`,
  not a directory → `` `required cwd is missing: ${requiredCwd ?? "unknown"}` ``, throw → `` `resume dependency unavailable: ${…}` ``;
  `:73` returns the session verdict.
- `:76-81` — `boundedTaskSummary`: whitespace-collapsed, trimmed, `> 120` → first 119 chars + `…`.
- `:83-108` — `listRetainedChildren(asyncDirRoot, sessionId)`: `listAsyncRuns(root, { sessionId, states: ["complete","failed","paused","stopped"], entryLimit: 100, reconcile: false })`,
  drop runs without `parentWorkflowRunId` or with `steps.length !== 1`, non-retained state, non-terminal step, or no `completedAt`;
  sort `completedAt` descending.
- `:110-135` — `formatRetainedChildren`: empty → `"No retained workflow children in the active parent session. If a retained-writer challenge is required, launch a same-role fallback challenge and label it as fallback."`;
  window = first 10; if none in the window is resumable, the first resumable beyond the window REPLACES slot 10 (`:113-116`);
  header `` `Retained workflow children (up to 10; newest first, with a resumable child retained when available):` ``;
  per child `- {runId} | {agent} | {state} | {ISO completedAt}`, `  workflow: {parentRunId}[ ({workflowKey})]`,
  `  task: {taskSummary || "(no task summary)"}`, `  resumability: resumable` | `  resumability: not resumable ({reason})`,
  resumable only: `  session: {path}` and `` `  resume: subagent({ action: "resume", id: "${runId}", message: "..." })` ``,
  `  tokens: input {i}, output {o}, total {t}` when present; trailer when nothing resumable:
  `"No resumable retained child is listed. Launch a same-role fallback challenge and label it as fallback."`.
- Dispatch — `src/runs/foreground/subagent-executor.ts:6467-6473`:
  `deps.state.currentSessionId = resolveCurrentSessionId(ctx.sessionManager); const children = listRetainedChildren(DIRS.async, deps.state.currentSessionId); return { content: [{ type: "text", text: formatRetainedChildren(children) }], details: { mode: "management", results: [] } };`
  — sits immediately BEFORE the `doctor` arm; NOT a member of `MUTATING_MANAGEMENT_ACTIONS` (`:213`).
- Enum position — `src/shared/types.ts:2801`: `… "models", "children.list", "guide", "validate", "create", …`.
- Second consumer — `src/extension/index.ts:840`: `const retainedChildren = listRetainedChildren(DIRS.async, ownerSessionId);` fed to `collectGoalContinuationNotices` (`:841`).
- `listAsyncRuns` (`src/runs/background/async-status.ts:486-600`): with terminal `states`, candidates come from
  `readRecentTerminalRunIndex(asyncDirRoot, { sessionId, limit: entryLimit })` (`:519`), never a directory scan
  unless `repairScan`; `:565-570` drop a foreign-session run BEFORE reconcile; `:579` `displayDismissedAt` skip;
  `:584-585` state and session filters.

### 2. What cyrup already has (grep-verified), and the one on-disk gap

**The scanner — REUSE, do not write a second.** `crate::tui::fleet::collect_fleet_history(async_root, results_dir, current_session_id: Option<&str>) -> Result<Vec<AsyncRunView>, String>` (`tui/fleet.rs:475-512`). Its own doc says it *is* `listAsyncRuns(root, { sessionId, entryLimit: MAX_FLEET_HISTORY_CANDIDATES (=100, `:100`), reconcile: false })`. Its candidate set (`fleet_history_candidates`, `:515-600`) is the index union upstream builds at `async-status.ts:506-521`: `active_run_index::read_live_active_run_ids` + `terminal_run_index::read_recent_terminal_run_index(root, session, Some(100))` (`:554-560`), falling back to a newest-first `read_dir` bounded scan (`:572-600`) that skips `is_reserved_async_root_entry`. It reads `status.json` without reconciling (`:486-491`) and applies pi's strict `status.sessionId !== options.sessionId` drop (`:496-500`). `AsyncRunView` (`tui/fleet_state.rs:394-409`) carries `paths: RunPaths` and `status: RunStatus`.
`background/run_status::list_active_runs` (`run_status.rs:855-902`) is the OTHER production scanner the seed alludes to, but it is hard-filtered to `Queued | Running` (`:895`) and reconciles — unusable for a terminal listing, exactly as `wait_subscriptions/manager.rs:554-559` already notes for itself.
Precedent for `background/*` calling into `tui`: `active_run_index.rs`, `runner_main/finish.rs`, `async_status_snapshot/{mod,project,state}.rs` all import `crate::tui`.

**The resumability chain — every piece landed:**
- `crate::background::RecoveryDescriptor::read(path) -> Result<Option<Self>, RecoveryDescriptorError>` (`recovery_descriptor.rs:629-659`), `Ok(None)` iff absent; `assert_belongs_to(&status, step_agent)` (`:771-793`) is upstream's `:61-62` pair and its doc already cites `retained-children.ts:61`. Errors `SourceRunMismatch { run_id, found }` / `AgentMismatch { run_id, descriptor_agent, step_agent }`. `descriptor.cwd: PathBuf` (non-optional, `:296`).
- `crate::background::RunDir::new(async_root, &run_id).recovery_descriptor()` / `.handoff()` (`run_paths.rs:96-111`) — the ONLY spellings of both literals.
- `crate::handoff::resolve_retained_worktree_cwd(&manifest_path, &LaneId, child_index: u32) -> Result<Option<PathBuf>, HandoffError>` (`handoff/read.rs:227-…`); `LaneId::parse(&str)` (`handoff/model.rs:305`). Its one production caller is the resume revive at `extension/executor/control.rs:340-378`, which uses EXACTLY the pair this verb needs — `RunDir::new(&async_root, &RunId::from_token(source_run_id)).handoff()` + `u32::try_from(step_index)`.
- The revive itself (`control.rs:282-320`) reads the descriptor at the SOURCE RUN's dir and refuses `RecoveryDescriptorError::Missing` with pi's sentence — this is what a `resume` of `(workflow id, index)` does today, and §3 makes the listing say the same thing.

**The on-disk gap (must be closed, or the session rung is dead for every child).** `workflow_step_statuses` (`child_summary.rs:690-717`) copies `agent / status / workflow_key / run_id / stopped / interrupted / error` onto the step row and NOTHING ELSE — `session_file`, `usage`, `model` stay `None`/default, although `StepStatus` has all three fields (`records.rs:24-40`) and every other run mode fills them. The data IS in hand: `WorkflowScriptChildResult::results[0]` is `serde_json::to_value(&SingleResult)` (`workflow.rs:503-505`), camelCase, carrying `sessionFile`, `usage`, `model`, `task`. `child_summary.rs:311-343` already reads `sessionName`/`model`/`thinking` off that same first object element through a `result_field` closure. The detached path already persists a session file onto the step row (`settlement.rs:324-326`), so the settled path leaving it empty is an asymmetry, not a policy. Upstream's step summary carries `sessionFile`/`tokens`/`model` (`async-status.ts:30-80`: `:42 sessionFile`, `:35 tokens`, `:38 model`).
Fix: in `workflow_step_statuses`, set `step.session_file = results[0].sessionFile (as PathBuf)`, `step.model = results[0].model`, `step.usage = results[0].usage` (deserialize with `serde_json::from_value::<Usage>`, `unwrap_or_default`). No new key: these are existing serialized fields of `StepStatus`. **Reachability test for the fix is §5's case A: without it every child answers `no persisted session file`.**

**The two goal-driver consumers — corrected count.** `grep -rn "collect_goal_continuation_notices(" crates/` → ONE production call: `extension/executor/notices.rs:596-602` passing `&[]`; every other `&[]` is a test in `goal_driver.rs:626-1004`. Upstream's second consumer is the verb arm itself (`subagent-executor.ts:6467`), which cyrup lacks. So: one call site changes (`notices.rs:596`), one arm is added.

**Other things that exist:** `missions::RetainedChild { run_id, parent_run_id, agent }` (`goal_driver.rs:67-74`) + `retained_resume_target` (`:419-429`) + the wrapping (`:449-456`), re-exported at `missions/mod.rs:60`; `missions::format_iso8601_millis(ms)` (`missions/mod.rs:159`, `pub(crate)`); `crate::time::now_epoch_millis`; `extension::executor::paths::{default_async_root_in, default_results_dir_in}(&roots, cwd)` (`paths.rs:23,29`); `SubagentExecutor::current_session_id() -> Option<String>` (`session_state.rs:55`); `SubagentExecutor::config_snapshot().await.roots`; `resume` already accepts `index` (`control.rs:112-118`, routing `"resume"` arm).

### 3. The predicate, rung by rung, mapped onto cyrup — and what is TRUE about each

`fn child_resumability(async_root, status: &RunStatus, step_index: usize, step: &StepStatus) -> Resumability` (async):

1. `status.state == Stopped || step.status == Stopped` → `NotResumable("stopped run")`. (`:53`)
2. **External runner — rung has no data in cyrup and is recorded, not simulated.** `StepStatus` carries no runner record (`records.rs:24-140`, grep `runner` → 0); the only external-runner surface is `SingleResult::runner: Option<ExternalCliRunnerStatus>` (`run_result.rs:319-328`), which is not on the step row. The port has no rung 2 and says so in a `[CYRUP-DELTA]` whose premise is that grep. (Adding a runner field to `StepStatus` would be a new on-disk key — out of scope; recorded as residual R2.)
3. `retained_session_file(step.session_file.as_ref().or(status.session_file.as_ref()))` — port `:37-50` with `tokio::fs::symlink_metadata` (the `lstat`), `extension() == Some("jsonl")`, `!is_file() || file_type().is_symlink()`; `ErrorKind::NotFound` → `"persisted session file is missing: {path}"`. All five sentences verbatim.
4. `RecoveryDescriptor::read(&RunDir::new(async_root, &status.run_id).recovery_descriptor()).await` — `Ok(None)` → `"missing recovery descriptor"`; `Err(e)` → `"invalid recovery descriptor: {e}"`; then `descriptor.assert_belongs_to(status, &step.agent)` → `SourceRunMismatch { found, .. }` → `"recovery descriptor belongs to run {found}"`, `AgentMismatch { descriptor_agent, .. }` → `"recovery descriptor belongs to agent {descriptor_agent}"`.
   **TRUE premise to write down:** the descriptor's cyrup location for a workflow child is the WORKFLOW run dir — the same file `revive_from_transcript` (`control.rs:301-320`) reads for `resume { id: <workflow>, index }` — and today NO producer writes one for a workflow (`for_single_launch`'s single production caller is the async SINGLE launch, `extension/executor/background.rs:779`). So, until R1 lands, every workflow child lists as `not resumable (missing recovery descriptor)` — pi's reason sentence. (`resume` refuses the same absence with `RecoveryDescriptorError::Missing`'s own sentence, "Async child '<id>' is missing its required run fan-out recovery identity. Start a new run instead." — `recovery_descriptor.rs:155-161`, raised at `control.rs:319`; corrected in `[FIX — round 1]`, the original text here claimed byte-for-byte equality.) The listing and the verb agree on the verdict; nothing is fabricated.
5. cwd ladder (`:66-72`): `resolve_retained_worktree_cwd(&RunDir::new(async_root,&status.run_id).handoff(), &LaneId::parse(status.run_id.as_str())?, u32::try_from(step_index).unwrap_or(u32::MAX)).await` — `Ok(Some(p))` → `p`; `Ok(None)` → `descriptor.cwd`; `Err(e)` (any `HandoffError`, and a `LaneId::parse` failure) → `"resume dependency unavailable: {e}"` — upstream's catch, NOT `control.rs:363-375`'s swallow (that one is a revive choosing to fall through; this one is a verdict, and pi returns not-resumable). Upstream's final `?? run.cwd` is unreachable here because `descriptor.cwd` is non-optional in cyrup — state that in the doc comment. Then `tokio::fs::metadata(&cwd)` not a dir / error → `"required cwd is missing: {cwd}"`.
6. Return the rung-3 verdict (`:73`).

### 4. Rust shape

**New file `crates/cyrup-ext-subagents/src/background/retained_children.rs`** (declared `pub mod retained_children;` in `background/mod.rs`'s public block `:39-102`; re-export `RetainedChild`, `RetainedChildState`, `Resumability`, `list_retained_children`, `format_retained_children` from `background/mod.rs:121-146`):

```rust
pub const MAX_RETAINED_CHILDREN: usize = 10;            // :7
pub const MAX_RETAINED_CHILD_CANDIDATES: usize = 100;   // :8 — equals tui::fleet::MAX_FLEET_HISTORY_CANDIDATES; assert_eq! in a test
pub const MAX_TASK_SUMMARY_LENGTH: usize = 120;         // :9

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedChildState { Complete, Failed, Paused, Stopped }   // :11 — TryFrom<RunState>, no `partial`
impl RetainedChildState { pub fn label(self) -> &'static str }       // reuse run_state_label wording

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resumability { Resumable { session_path: PathBuf }, NotResumable { reason: String } }  // :12-14

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetainedChild {
    pub run_id: RunId,                          // the CHILD's id (step.run_id)
    pub parent_run_id: RunId,                   // the workflow's id — see §0 for why not Option
    pub step_index: usize,                      // [CYRUP-DELTA] cyrup addresses a workflow child as (parent, index); the resume hint needs it
    pub workflow_key: Option<WorkflowKey>,
    pub state: RetainedChildState,
    pub agent: String,
    pub task_summary: String,
    pub completed_at: i64,
    pub resumability: Resumability,
    pub token_totals: Option<Usage>,
}

pub async fn list_retained_children(async_root: &Path, results_dir: &Path, session_id: Option<&str>) -> Result<Vec<RetainedChild>, String>;
pub fn format_retained_children(children: &[RetainedChild]) -> String;
```

`list_retained_children`: `collect_fleet_history(async_root, results_dir, session_id).await?` → for each view with `status.mode == RunMode::Workflow` and `RetainedChildState::try_from(status.state).is_ok()` (`:87`), `display_dismissed_at.is_none()` (already dropped by the scanner? — NO: `collect_fleet_history` does not apply `:579`; apply it here) → for each `(index, step)` with `step.run_id.is_some()` and `step.status.is_terminal() || step.status == Paused` (`:33-35`) → build the row; `completed_at = status.ended_at.unwrap_or(status.last_update)` (the `undefined` guard at `:91` is unreachable because `last_update: i64` is not optional — say so); sort by `completed_at` descending (`:106`). Pass `session_id` through unchanged: `None` means pi's falsy `sessionId` (no filter), which is what `collect_fleet_history` implements.

`task_summary`: **`[CYRUP-DELTA]`** — the step row has no description (`records.rs:24-140`), the receipt entry has none (`receipt.rs:231-258`), `foreground-history.json` has none (`foreground_history/record.rs:80-135`); the task text exists only in the in-memory `results[0].task` of the settle call. The port therefore always takes upstream's own `(no task summary)` branch (`:123`), and `bounded_task_summary` is ported and applied to the empty string so the day a description lands on the row it prints bounded. Residual R3 below.

`format_retained_children`: port `:110-135` verbatim, with two `[CYRUP-DELTA]`s whose premises are §0: the `workflow:` line is always printed (parent is not optional), and the resume hint is
`` `  resume: subagent({ action: "resume", id: "{parent_run_id}", index: {step_index}, message: "..." })` `` — because a workflow child has no async dir of its own (`workflow.rs:855-905`), `resume` addresses it through the workflow run and the step index (`control.rs:112-118` accepts `index`; `background/control.rs:2229-2286` resolves the step). ISO via `missions::format_iso8601_millis` (make it `pub(crate)` reachable; it already is). Tokens: `input {usage.input}, output {usage.output}, total {usage.total_tokens}`.

**Dispatch** — `extension/tool/routing.rs:1085` `route_action`: new arm placed where upstream's is (`:6467`, just before `"doctor"`); it is a READ and must NOT go through `route_control_action` (the `inspect` arm's stated reason, `routing.rs:1154-1163`):

```rust
"children.list" => {
    let cfg = self.executor.config_snapshot().await;
    let async_root = default_async_root_in(&cfg.roots, cwd);
    let results_dir = default_results_dir_in(&cfg.roots, cwd);
    let session = self.executor.current_session_id();            // pi :6468 resolveCurrentSessionId
    let children = crate::background::retained_children::list_retained_children(&async_root, &results_dir, session.as_deref()).await.map_err(ToolError::new)?;
    Ok(ToolResult { content: vec![Content::text(format_retained_children(&children))], details: None, terminate: TerminateHint::Unspecified, ..Default::default() })
}
```

**Advertise** — `extension/tool/text.rs`: insert `"children.list"` between `"models"` and `"guide"` (pi `types.ts:2801`); the schema enum is DERIVED from that list (`schema.rs:359`), so nothing else advertises. Also `DESTRUCTIVE_MANAGEMENT_ACTIONS` stays untouched (read verb).

**Goal-driver wiring** — `notices.rs:596`: replace `&[]` with the mapped list:
```rust
let roots = self.config_snapshot().await.roots;
let retained: Vec<crate::missions::RetainedChild> = crate::background::retained_children::list_retained_children(
        &default_async_root_in(&roots, cwd), &default_results_dir_in(&roots, cwd), Some(&owner_session_id))
    .await.unwrap_or_default()   // best-effort by construction, like the whole block (`:571-573`)
    .iter().map(crate::missions::RetainedChild::from).collect();
```
with `impl From<&background::retained_children::RetainedChild> for missions::RetainedChild` (three fields; `parent_run_id: Some(parent.as_str().to_string())`). `retained_resume_target` (`goal_driver.rs:419-429`) then matches a mission whose latest linked run is the WORKFLOW (`child.parent_run_id == latest_run.run_id`) — the shape `mission.attach-run` of a workflow id produces.

### 5. Stale notes — confirmed with the grep, and what replaces each

| Note | Grep | Verdict | Action |
|---|---|---|---|
| `text.rs:244-258` — *"`children.list` needs RETENTION, and this build retains nothing: the foreground `WorkflowRunHost`'s `settled` list is dropped with the tool call, so the listing would still always be empty"* | `settled` IS dropped (`workflow.rs:214`), but `publish_steps` (`:342-368`) and the terminal write (`workflow_launch.rs:612-618,682`) persist it as step rows first | **STALE** — retention exists on disk | Replace with the verb entry at pi's index and a one-line pointer to `retained_children.rs`'s module doc (§0's table). |
| `text.rs:262-266` — *"cyrup omits `children.list` (see the note above), so `validate` follows `guide` directly"* | same | **STALE** | Reword: `validate` follows `guide` at pi's own index; nothing omitted. |
| `goal_driver.rs:30-39` — *"cyrup has no `workflowScript` runtime (the identifier appears nowhere in this crate)"* | `grep -rl workflowScript crates/cyrup-ext-subagents/src` → **30 files**; `workflows/scripted/engine.rs:1` is *"The workflowScript runtime"* | **FALSE** | DELETE the `[CYRUP-DELTA]` heading and paragraph; `:61-66`'s *"see the module's `[CYRUP-DELTA]` note on why nothing in cyrup produces one yet"* → *"produced by `background::retained_children`, mapped by `From`"*. |
| `notices.rs:592-595` — *"a `workflowScript` concept this crate has no runtime for — so it is necessarily empty here and is passed as such rather than faked"* | same grep | **FALSE** | DELETE; the call passes the real list (§4). |
| `schema.rs:933-942` (test comment) — *"this build retains none — `WorkflowRunHost`'s `settled` vec dies with the tool call"* + the enum vector lacks the verb | as row 1 | **STALE** | Insert `"children.list"` after `"models"` in the expected vector; drop the paragraph. |
| `registration/guide.rs:262-275` (`guide_is_advertised_in_pis_own_position`) asserts `guide`'s predecessor is `models` *"cyrup omits the unported `children.list`"* | — | **STALE** (goes red on the insert — that is its job, `:274-278` says so) | Assert predecessor `children.list`. |
| `handoff/read.rs:184-195` — *"`children.list` — a verb cyrup does not have … Whoever ports `children.list` wants pi `:128-160` and `retained-children.ts:60-75`"* | upstream `git grep resolveParallelHandoffChild v0.68.0 -- src` → only its own definition (`parallel-handoff.ts:128`); `retained-children.ts:68` calls `resolveRetainedWorktreeCwd`, which cyrup HAS (`read.rs:227`) | **STALE on the verb, WRONG on the caller** | Rewrite: `resolveParallelHandoffChild` has NO production caller at v0.68.0 (the `:68` reference was to an older line), and `children.list`'s cwd rung is `resolve_retained_worktree_cwd`, whose second production caller is now `background/retained_children.rs`. |
| `async_retention/policy.rs:365-373` — *"`RunStatus` has no `parent_workflow_run_id`"* | `grep -n parent_workflow_run_id records.rs` → 0 | **TRUE** | Keep. Cite it from the new module. |
| `docs/gap-analysis/09-cyrup-ext-subagents.md:610` (SUBA-055 residual: *"cyrup has no `parentWorkflowRunId` and no retained-child concept"*) and `:681` (SUBA-005) | — | STALE | One-line closure notes (docs only). |

### 6. Production call sites (exact)

1. `extension/tool/routing.rs` — `route_action`, new `"children.list"` arm (before `"doctor"` at `:1100`).
2. `extension/tool/text.rs` — `SUBAGENT_ACTIONS`, index between `"models"` and `"guide"`.
3. `extension/executor/notices.rs:596` — `raise_goal_continuation_notices` passes the real list.
4. `workflows/child_summary.rs:690-717` — `workflow_step_statuses` fills `session_file` / `model` / `usage` from `results[0]` (reached by every workflow settle: `workflow.rs:352` and `workflow_launch.rs:616`).
5. `background/retained_children.rs` — calls `tui::fleet::collect_fleet_history`, `RecoveryDescriptor::read` + `assert_belongs_to`, `handoff::resolve_retained_worktree_cwd`, `RunDir::{handoff,recovery_descriptor}`.

### 7. Reachability test — and why it fails if gutted

`extension/tool/routing_tests.rs` (the `a_detached_workflow_child_reconciles_the_paused_workflow` idiom, `:2482-2570`), `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`:

```
children_list_lists_a_settled_workflow_child_with_the_real_predicate
```
1. `SubagentExecutor::new()`, `arm_scoped_missions`, `set_host_services(FixedSessionHost("session-children"))`; `cfg.spawn_command` = a COMPLETING scripted child (the `scheduled_runs.rs:396-418` `agent_start / message_end / agent_settled / exit 0` shape, without the marker).
2. Pre-create `<dir>/sessions/` and write a regular `child.jsonl` there; dispatch
   `{ "workflowScript": "await runs.run(\"a\", { agent: \"worker\", task: \"T\", model: \"sonnet\", sessionDir: \"<dir>/sessions\", share: true }); return \"done\";" }`.
   `resolve_single_run_session_root` (`tool/task_items.rs:345-372`) uses an explicit `sessionDir` AS-IS and `resolve_run_channels` appends the `run-0` leaf (`foreground.rs:731-733`: `.map(|root| root.join("run-0"))`), so the file to plant is `<dir>/sessions/run-0/child.jsonl`; `resolve_result_session_file` (`exec/mod.rs:1015-1030`, the `share == Some(true) && session_dir` branch) then returns it through `find_latest_session_file_by_mtime` (`registration/cost.rs`, newest regular `.jsonl` in that one directory), so `SingleResult::session_file` is `Some(real regular .jsonl)` and, after §2's fix, so is the step row.
3. Read `workflowRunId` off `details`; assert the workflow's `status.json` step 0 has `sessionFile` (this is the §2 fix's own pin).
4. **Case A (predicate live, descriptor absent):** dispatch `{ "action": "children.list" }` → text contains
   `- <child run id> | worker | complete | <ISO>`, `  workflow: <wf id> (a)`, `  task: (no task summary)`,
   `  resumability: not resumable (missing recovery descriptor)` and the fallback-challenge trailer.
   *Gut the session fill (§2) → the reason is `no persisted session file` → fails. Gut the descriptor rung → the row reads `resumable` → fails.*
5. **Case B (resumable):** write a descriptor with the PRODUCTION writer — `RecoveryDescriptor::for_single_launch(&runner_config, LaunchInputs::default())` (construction as `recovery_descriptor.rs:1170-1180`) with `source_run_id = <wf id>`, `agent = "worker"`, `cwd = dir` → `.write(&RunDir::new(&async_root,&wf).recovery_descriptor())`. Re-dispatch → `  resumability: resumable`, `  session: <the .jsonl>`, `` `  resume: subagent({ action: "resume", id: "<wf id>", index: 0, message: "..." })` ``, and NO trailer.
   *Gut `assert_belongs_to` → a descriptor written for agent `"other"` still reads resumable → the mismatch sub-case (write `agent: "other"` → `recovery descriptor belongs to agent other`) fails.*
6. **Case C (session missing):** delete the `.jsonl` → `persisted session file is missing: <path>`.
7. **Mutation named by the DoD:** stub `list_retained_children` to `Ok(vec![])` → the reply is the empty sentence → Case A's first assertion fails.

Second test, `children_list_is_advertised_at_pis_index_and_read_only`: `subagent_actions()` contains the verb directly after `"models"`; dispatching it through a tool constructed with `allow_mutating_management = false` (the child-safe gate, cf. `refine_verbs_are_both_advertised_and_dispatched` `routing_tests.rs:2700-2740`) is NOT refused.

Third, goal-driver wiring, `notices.rs` (idiom at `:1803-1840` with `Recording` sink): after the same settled workflow, `create_mission { goal, budget, owner_session_id: "session-children" }` + `attach` the WORKFLOW run id as a `complete` link, then `raise_goal_continuation_notices(dir)` == 1 and the delivered message contains `Resume retained child <child id> (worker) for:` — the wrapping at `goal_driver.rs:449-456`, reachable ONLY if `notices.rs:596` passes a non-empty list. *Gut the call back to `&[]` → the message lacks the prefix → fails.*

`cargo nextest run -p cyrup-ext-subagents -E 'test(children_list) or test(retained)'` plus the unit tests inside `retained_children.rs` (`bounded_task_summary` 120/119+`…`; the window rule `:113-116` with 11 fixtures where only #11 is resumable; `RetainedChildState::try_from(Running|Queued)` refused). Existing pins that WILL go red and must be updated, by design: `guide.rs:262-275`, `schema.rs:905-1015`, `guide.rs:292-300` (`the_tool_reference_topic_names_every_dispatched_verb` — add the row to `resources/docs/tool-reference.md`'s Actions table after `models`: `` | `children.list` | management | List a workflow run's settled children, newest first, with whether each can be resumed | ``).

### 8. Files touched

- NEW `crates/cyrup-ext-subagents/src/background/retained_children.rs`
- `crates/cyrup-ext-subagents/src/background/mod.rs` (declare + re-export)
- `crates/cyrup-ext-subagents/src/workflows/child_summary.rs` (`workflow_step_statuses`: session_file/model/usage from `results[0]`)
- `crates/cyrup-ext-subagents/src/extension/tool/routing.rs` (arm)
- `crates/cyrup-ext-subagents/src/extension/tool/text.rs` (enum + note rewrite)
- `crates/cyrup-ext-subagents/src/extension/tool/schema.rs` (test vector + comment)
- `crates/cyrup-ext-subagents/src/extension/executor/notices.rs` (`:592-602`)
- `crates/cyrup-ext-subagents/src/missions/goal_driver.rs` (`:30-39` delete, `:61-66` doc, `From` impl)
- `crates/cyrup-ext-subagents/src/registration/guide.rs` (`:262-275` predecessor)
- `crates/cyrup-ext-subagents/src/handoff/read.rs` (`:184-195` rewrite)
- `crates/cyrup-ext-subagents/resources/docs/tool-reference.md` (row)
- `crates/cyrup-ext-subagents/src/extension/tool/routing_tests.rs` (tests §7)
- `docs/gap-analysis/09-cyrup-ext-subagents.md` (`:610`, `:681` closure notes)

### 9. Residuals — recorded, NOT closed by this verb (each keeps a TRUE premise)

- **R1 — no recovery descriptor for workflow children.** `for_single_launch` is written only by the async SINGLE launch (`background.rs:779`); a workflow run dir holds one `recovery-descriptor.json` slot and a workflow has N children with N agents, so a per-child location is needed before any workflow child can list `resumable` in production. Until then the listing says `missing recovery descriptor`, which is what `resume` refuses with (`control.rs:301-320`). Owner call: where the per-child descriptor lives (`steer-targets/<index>/` already exists as a per-child subtree, `control::step_steer_inbox_dir`).
- **R2 — external-runner rung.** No runner record on `StepStatus` (`records.rs:24-140`); rung `:54` has no data and is absent, stated.
- **R3 — task summary.** No description on the step row / receipt / history; `(no task summary)` is upstream's own branch (`:123`) and the only honest output.
- **`debug.run`** — sibling AUG; nothing here touches `status`.

## [EXEC — children.list]

Executed 2026-09-19 on `claude/subagents-verbs` from `bf4e128`. Upstream read ONLY via
`git -C tmp/pi-subagents show v0.68.0:<path>`. Two exec passes worked this tree concurrently; the
result below is the union, validated as one tree — every file was re-read before it was edited and
every mutation was restored byte-for-byte (`cmp`) before the next one.

### What landed

**NEW `crates/cyrup-ext-subagents/src/background/retained_children.rs`** (≈620 lines, declared
`pub mod retained_children;` at `background/mod.rs:68` and re-exported as
`background::{RetainedChild, RetainedChildState, Resumability, list_retained_children,
format_retained_children}`) — the port of `src/runs/background/retained-children.ts` (135 lines
@v0.68.0):

| upstream | here |
|---|---|
| `MAX_RETAINED_CHILDREN = 10` / `MAX_RETAINED_CHILD_CANDIDATES = 100` / `MAX_TASK_SUMMARY_LENGTH = 120` (`:7-9`) | same three consts; the candidate bound IS `tui::fleet::MAX_FLEET_HISTORY_CANDIDATES` and `the_candidate_limit_is_the_fleet_scans_own` pins it |
| `RetainedChildState` (`:11`) | `enum RetainedChildState { Complete, Failed, Paused, Stopped }` + `TryFrom<RunState>` refusing `Queued`/`Running`; no `partial` — `RunState` has none |
| `RetainedChildResumability` (`:12-14`) | `enum Resumability { Resumable { session_path }, NotResumable { reason } }` |
| `RetainedChild` (`:16-27`) | `struct RetainedChild { run_id, parent_run_id, step_index, workflow_key, state, agent, task_summary, completed_at, resumability, token_totals }` — `parent_run_id` is not optional (only a workflow produces one) and `step_index` is the one `[CYRUP-DELTA]` field, because `resume` addresses a workflow child as `(workflow id, index)` |
| `retainedSessionFile` (`:37-50`) | `retained_session_file` — `symlink_metadata` (the `lstat`), five sentences verbatim |
| `childResumability` (`:52-74`) | `child_resumability` at `:221` — rung 1 (stopped), rung 3 (session), rung 4 (`RecoveryDescriptor::read` at the WORKFLOW run dir + `assert_belongs_to`, the three reason sentences verbatim), rung 5 (`resolve_retained_worktree_cwd` ?? `descriptor.cwd`, then `metadata().is_dir()`), rung 6. Rung 2 (external runner) is ABSENT and recorded: `StepStatus` has no runner record (`grep runner records.rs` = 0) |
| `boundedTaskSummary` (`:76-81`) | `bounded_task_summary` at `:309`, applied to `None` — the step row / receipt / history carry no description, so the row always takes upstream's own `(no task summary)` branch |
| `listRetainedChildren` (`:83-108`) | `list_retained_children(async_root, results_dir, session_id)` at `:337` over `tui::fleet::collect_fleet_history` (the existing `listAsyncRuns(root,{sessionId,entryLimit:100,reconcile:false})`), filtered `mode == Workflow` ∧ retained state ∧ `display_dismissed_at.is_none()`, one child per terminal step row with a `run_id`; `completed_at = ended_at ?? last_update`; `sort_by_key(Reverse(completed_at))` (stable) |
| `formatRetainedChildren` (`:110-135`) | `format_retained_children` at `:390` — both fallback sentences byte-identical, the window rule `:113-116` (a resumable child beyond the window replaces slot 10), tokens line; the `workflow:` line is unconditional and the hint is `subagent({ action: "resume", id: "<workflow>", index: <n>, message: "..." })` |

**The design correction the AUG made stands.** A cyrup workflow child is FOREGROUND and owns no
async dir; `RunStatus` has no `parent_workflow_run_id` (`async_retention/policy.rs:365-373`'s
delta is TRUE and was kept). The retention is the workflow's own `status.json` step rows
(`publish_steps` + the terminal write), and one upstream "retained child run" ⇔ one cyrup
`(workflow status, step index)`. No new on-disk key was added anywhere.

**Wiring (production call sites):**

1. `extension/tool/routing.rs:1105` — `"children.list"` arm in `route_action`, immediately before
   `"doctor"` (pi `subagent-executor.ts:6467-6473`): `config_snapshot().roots` →
   `default_async_root_in`/`default_results_dir_in`, `current_session_id()` (pi
   `resolveCurrentSessionId`), text reply, `details: None`. Its own arm, NOT the control band — a
   read must not pass `route_control_action`'s authority consult.
2. `extension/tool/text.rs:250` — `"children.list"` between `"models"` and `"guide"` (pi
   `shared/types.ts:2801`); the schema enum derives from it. Not in `DESTRUCTIVE_MANAGEMENT_ACTIONS`.
3. `extension/executor/notices.rs:598-611` — `raise_goal_continuation_notices` passes the REAL
   list (`list_retained_children(..., Some(&owner_session_id))`, `unwrap_or_default()` because the
   whole block is best-effort by construction), mapped through the new
   `impl From<&background::RetainedChild> for missions::RetainedChild` (`goal_driver.rs:70`).
4. `workflows/child_summary.rs:706-717` — `workflow_step_statuses` now fills `session_file` /
   `model` / `usage` from `results[0]` (the `serde_json::to_value(&SingleResult)` the host settles
   with). Reached by every workflow settle (`workflow.rs:350 publish_steps`,
   `workflow_launch.rs:633` terminal write). Without it every child answered `no persisted session
   file` — mutation (b) below is its proof.
5. `background/retained_children.rs` — calls `tui::fleet::collect_fleet_history`,
   `RecoveryDescriptor::read` + `assert_belongs_to`, `handoff::resolve_retained_worktree_cwd`,
   `RunDir::{handoff, recovery_descriptor}` — the second production caller of the handoff cwd
   resolver.

**Docs:** `resources/docs/tool-reference.md:20` gains the `children.list` row (the
`the_tool_reference_topic_names_every_dispatched_verb` pin); `docs/gap-analysis/09` gets closure
notes on the SUBA-055 residual and the SUBA-005 unowned-verb list.

### Stale notes corrected (each premise re-grepped before the edit)

| Note | Was | Now |
|---|---|---|
| `extension/tool/text.rs:244-258` | "`children.list` needs RETENTION, and this build retains nothing … the listing would still always be empty" | DELETED; replaced by the verb entry at pi's index with a pointer to the module doc. Retention exists on disk as step rows. |
| `extension/tool/text.rs:262-266` | "cyrup omits `children.list`, so `validate` follows `guide` directly" | reworded: `validate` follows `guide` at pi's own index; nothing omitted |
| `missions/goal_driver.rs:30-39` | `[CYRUP-DELTA]` "cyrup has no `workflowScript` runtime (the identifier appears nowhere in this crate)" — FALSE, `grep -rl workflowScript src` = 30 files | DELETED (heading + paragraph); `:61-66` now names the producer and the `From` |
| `extension/executor/notices.rs:592-595` | `[CYRUP-DELTA]` "a `workflowScript` concept this crate has no runtime for — so it is necessarily empty here" — FALSE | DELETED; the call passes the real list |
| `extension/tool/schema.rs:933-942` | "this build retains none — `WorkflowRunHost`'s `settled` vec dies with the tool call" + enum vector without the verb | paragraph dropped; `"children.list"` inserted after `"models"` in the expected vector |
| `registration/guide.rs:262-275` | pin asserting `guide`'s predecessor is `models` "cyrup omits the unported `children.list`" | asserts predecessor `children.list` (`:270`) |
| `handoff/read.rs:184-195` | "a verb cyrup does not have" + "Upstream's only caller is `retained-children.ts:68`" — WRONG on both: `git grep resolveParallelHandoffChild v0.68.0 -- src` finds only the definition, and `:68` calls `resolveRetainedWorktreeCwd` | rewritten (`:184-194`): no upstream production caller; `children.list` is ported and its cwd rung is `resolve_retained_worktree_cwd` |
| `async_retention/policy.rs:365-373` | "`RunStatus` has no `parent_workflow_run_id`" | TRUE — KEPT, cited by the new module |
| `docs/gap-analysis/09:610,:681` | SUBA-055 residual / SUBA-005 unowned list | closure notes appended |

### Reachability tests (all through `SubagentTool::execute`, nothing touches the listing directly)

`extension/tool/children_list_tests.rs` (a `#[path]` sibling declared at `routing.rs:2777`, like
`lane_actions_tests`):

- `children_list_lists_a_settled_workflow_child_with_the_real_predicate` (multi_thread) — a real
  `workflowScript` dispatch with `cfg.spawn_command` = a completing scripted child
  (`testsupport::write_completing_child_binary`, the `agent_start`/`message_end`/`agent_settled`/exit 0
  shape) and `runs.run("a", { agent: "worker", task: "T", model: "sonnet", sessionDir: "<dir>/sessions", share: true })`
  over a pre-planted regular `<dir>/sessions/run-0/child.jsonl`. Pins the workflow's `status.json`
  step 0 `sessionFile` (the `child_summary` fix). **Case A** (no descriptor): `- <child id> | worker
  | complete | <ISO endedAt>`, `  workflow: <wf id> (a)`, `  task: (no task summary)`,
  `  resumability: not resumable (missing recovery descriptor)`, trailer present, no `session:`.
  **Case B**: a descriptor written with the PRODUCTION writer
  (`RecoveryDescriptor::for_single_launch` over `resolve_plan_personas(["worker"])`, written to
  `RunDir::new(root, &wf).recovery_descriptor()`) → `resumability: resumable`, `session: <the
  .jsonl>`, `resume: subagent({ action: "resume", id: "<wf>", index: 0, message: "..." })`, NO
  trailer. **B′**: the same writer for agent `reviewer` → `recovery descriptor belongs to agent
  reviewer`. **Case C**: the `.jsonl` deleted → `persisted session file is missing: <path>`.
- `children_list_is_advertised_at_pis_index_and_passes_the_child_safe_gate` — `subagent_actions()`
  has the verb directly after `models` and before `guide`; a `SubagentTool::new_child_safe`
  (`allow_mutating_management = false`) dispatch is NOT refused and answers pi's empty sentence.

`extension/executor/notices.rs:1965` `mod retained_children_goal_tests`:

- `a_settled_workflow_child_wraps_the_goal_notice_as_a_retained_resume` — same settled workflow
  under `FixedSessionIdHost("session-children")`, a goal mission owned by that session with the
  WORKFLOW run attached as its latest `complete` link → `raise_goal_continuation_notices == 1` and
  the delivered message contains `Next ready action: Resume retained child <child id> (worker) for:
  Continue objective: …` — reachable only through `goal_driver.rs::next_ready_action` with a
  non-empty list whose `parent_run_id` matches.

Unit tests inside `retained_children.rs`: the candidate bound, `bounded_task_summary` (120 pass /
121 → 119+`…`), `TryFrom<RunState>` refusing `Running`/`Queued`, the window rule with eleven
fixtures where only #11 is resumable, both fallback sentences, and every session-file verdict over
real files (regular / symlink / directory / missing / not-`.jsonl`).

### Mutations run (apply → targeted `cargo nextest` → observe FAIL → restore, `cmp`-verified)

| # | Mutation | Failing test |
|---|---|---|
| a | `list_retained_children` returns `Ok(vec![])` when the scan is non-empty | `children_list_lists_a_settled_workflow_child_with_the_real_predicate` — `assertion left != right failed: the settled child must be listed` (`children_list_tests.rs:196`); `a_settled_workflow_child_wraps_the_goal_notice_as_a_retained_resume` also FAILs |
| b | `workflow_step_statuses`: `step.session_file = None` | `…with_the_real_predicate` — `the terminal write must persist the child's session file onto its step row` (`:175`, the row shows `"sessionFile":null`) |
| c | descriptor rung: `Ok(None) => return session` | `…with_the_real_predicate` — case A's `missing recovery descriptor` assertion (`:218`), the child read as resumable |
| d | `assert_belongs_to` replaced by `Ok(())` | `…with_the_real_predicate` — B′'s `recovery descriptor belongs to agent reviewer` assertion (`:275`) |
| e | `notices.rs` passes `&[]` again | `a_settled_workflow_child_wraps_the_goal_notice_as_a_retained_resume` — the `Resume retained child` message assertion (`notices.rs:2101`) |
| f | `"children.list"` removed from `SUBAGENT_ACTIONS` | `children_list_is_advertised_at_pis_index_and_passes_the_child_safe_gate` — `` `children.list` must be in SUBAGENT_ACTIONS`` (`:312`) |
| g | the `route_action` arm renamed (`"children.list-disconnected"`) | `…passes_the_child_safe_gate` — `a read verb is not refused by the child-safe gate` (`:325`, the unknown-action arm answered) |

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean (one
  `unnecessary_sort_by` on the listing's sort was fixed to `sort_by_key(Reverse(..))`, same
  stable order).
- `cargo nextest run --workspace --features test-fixtures` — ``Summary [ 108.508s] 10670 tests run: 10670 passed, 9 skipped``
  (baseline 10661 / 9 skipped; the delta is this pass's 9 new tests).

### Residuals (each with a TRUE premise, none closed here)

- **R1 — no recovery descriptor for a workflow child.** `for_single_launch` is written only by
  the async SINGLE launch (`extension/executor/background.rs:779`); a workflow run dir holds one
  `recovery-descriptor.json` slot for N children with N agents. In production every row therefore
  lists `not resumable (missing recovery descriptor)` — pi's reason sentence; `resume` refuses
  the same absence with `RecoveryDescriptorError::Missing`'s different sentence
  (`recovery_descriptor.rs:155-161`, raised at `control.rs:319`; the "byte-for-byte" claim that
  stood here was corrected in `[FIX — round 1]`). The test reaches `resumable` by planting a descriptor with the production
  writer; the verb fabricates nothing. Owner call: the per-child descriptor location
  (`steer-targets/<index>/` already exists as a per-child subtree).
- **R2 — external-runner rung** (`retained-children.ts:54`): no runner record on `StepStatus`; the
  rung is absent and recorded in `child_resumability`'s doc.
- **R3 — task summary**: no description on the step row / receipt / history; `(no task summary)` is
  upstream's own branch.
- `debug.run` is the sibling section's; nothing here touches `status`.
- `docs/gap-analysis/PARITY-GAPS.md` and `00-residual-ledger.md` were reconciled in the same pass
  (the verb gap reads **8**; `children.list` has a CLOSED row) — 7 once `debug.run` lands.

---

## [EXEC — debug.run]

Executed 2026-09-19 on `claude/subagents-verbs` over the tree the `[EXEC — children.list]` pass
left (`bf4e128` + its uncommitted changes). Upstream read ONLY via
`git -C tmp/pi-subagents show v0.68.0:<path>`; every premise below was re-grepped before it was
written; every mutation was restored byte-for-byte (`cmp`) before the next one.

### What landed

**NEW `crates/cyrup-ext-subagents/src/background/run_lifecycle_debug.rs`** (declared `pub mod
run_lifecycle_debug;` at `background/mod.rs`) — the port of `run-status.ts:47-108` @v0.68.0:

| upstream | here |
|---|---|
| the two refusal sentences (`subagent-executor.ts:6532-6536`) + the result-only refusal (`run-status.ts:714-721`) | `DEBUG_RUN_REQUIRES_TARGET` `:52`, `DEBUG_RUN_NO_VIEWS` `:54`, `DEBUG_RUN_NEEDS_STATUS_DIR` `:57`, pi's exact sentences |
| `formatRunLifecycleDebug`'s input (`:87`) | `pub struct RunLifecycleDebug<'a> { status, paths, runner, capacity }` `:61` |
| `debugProcessTerminal`'s `{ sidecar, overlay }` (`:52-58`) | `pub enum RunnerLiveness { NotRecorded, Probed { pid, liveness } }` `:82` with `probe(status, impl Fn(u32) -> Liveness)` `:99` — production passes `check_pid_liveness`; the ONE new `[CYRUP-DELTA]` (`:74`) cites VL-S4 and §D3 |
| `formatCapacityOwner` (`:60-72`) | `format_capacity_owner` `:127` — `Active capacity: <state> — <reason>`, `Capacity owner: <relation> slot <n>, <kind>, generation <g>`, `Capacity session:`, `Capacity source run:`?, `Capacity async dir:`, **`Capacity runner pid: <n>`** in place of `Capacity runner: <runnerProcessInstanceId>` (`key.rs:93-99`: cyrup mints none), `Capacity runner started: <iso>` |
| `formatWorkflowDebug` (`:74-85`) | `format_workflow_debug` `:172` — gate = `has_workflow_reference`'s three disjuncts (`async_retention/policy.rs`); `Workflow children: <n>` (workflow mode) + `  <i>. key <k|n/a> · <session_name.trim() | agent> · <step state>[ · run <id>]`; no `Workflow parent:`/`Lane:` (no such `RunStatus` field), no `async yes/no` / `lane` / `worktree` tail (no such `StepStatus` field) — each reason in the module doc, once |
| `formatRunLifecycleDebug` (`:87-108`) | `format_run_lifecycle_debug` `:212` — header, `Run:`, `Dir:`, `Status file:`, `Workflow receipt:`?, `Session:`, `State:`, `Mode:`, then the two process-terminal lines below, then the capacity lines, then the workflow lines |

**The process-terminal lines.** Re-grepped before writing: `grep -rn 'process-terminal\|process_terminal\|ProcessTerminal' crates/cyrup-ext-subagents/src` → every hit a `//!`/`///`/`//` comment or string literal (`active_run_index.rs`, `active_async_capacity/{inspect,key,mod,tests}.rs`, `rpc/ping.rs`, `registration/doctor.rs`), no symbol; `grep -rn 'Process terminal' src` → 0 before this change; `RunDir`/`RunPaths` expose no such path; `RunStatus`/`StepStatus` have no such field. So the dump prints exactly:

```
Process terminal: not recorded — this build writes no process-terminal.json (VL-S4); runner pid liveness stands in for the proof
Runner pid: <pid> (alive|dead|unknown)        |  Runner pid: not recorded
```

and NEVER `Process terminal file:` / `Status process terminal:` / `Sidecar process terminal:`. VL-S4 stays open, untouched.

**Accessors added so the formatter duplicates no private `match`:** `ActiveAsyncCapacityReleaseVerdict::state_word()` (`inspect.rs:96`, `releasable|retained|not-owned`) and `reason()` (`:106`); `CapacityRelation::as_str()` (`:144`); `ActiveAsyncCapacityKind::as_str()` (`key.rs:78`, the `owner.json` spelling); `liveness_word` promoted to `pub(crate)` (`inspect.rs:607`).

**Executor** — `SubagentExecutor::control_debug_run(&self, cwd, id: Option<&str>, dir: Option<&str>) -> Result<String, String>` (`extension/executor/status.rs:343`): roots → location (`dir` form: basename → `RunId::from_token` + `RunPaths::for_run(parent, results, id)`; id form: `resolve_async_run_id(id, root, results, None)` — no session filter, pi `async-resume.ts:223-259` — `Ok(None)` → `Async run not found. Provide id or dir.`, `async_dir: None` → `DEBUG_RUN_NEEDS_STATUS_DIR`) → `reconcile_now(&paths, None)`, the FULL reconciler (pi `:475`) → `Self::capacity_options(&cfg, self.live_workflow_run_ids())` + `inspect_active_async_capacity_owner(&run_id, session_id, Some(&run_dir), &options)` (pi `:515`'s three identities) → `RunnerLiveness::probe(&status, check_pid_liveness)` → `format_run_lifecycle_debug`. **One thing the AUG did not know:** cyrup's `reconcile_now` never returns "no status" — with neither `status.json` nor a result it synthesises `Failed` WITHOUT writing (`reconcile_missing_status`) — so pi's `:781-785` fall-through is detected as "`status.json` did not exist and the outcome's action is not `RepairedFromResult`" → `Status file not found.`, and the refusal test asserts no `status.json` is left behind. The display-dismissed marker is not a refusal (pi `:485-492` dumps anyway), which is why this does not route through `inspect_paths`.

**Routing** — `routing.rs:1187`: `"debug.run"` joins the control band (`"status" | "debug.run" | …`); the arm at `:2215` in `route_control_action` reads `p.id.or(p.run_id)`/`p.dir`/`p.view` with pi's order — target refusal (`:6532`) BEFORE view refusal (`:6535`). The authority consult maps it to `None` (`registration/authority.rs`), so nothing gates it — as upstream leaves it. Result mapping is the existing `{"mode":"management"}`. No `Debug run\n` label prefix: `formatStatusTargetLabel`/`withSpawnBudgetStatus` are an unported status-side delta (`grep -rn 'Status target' src` → 0 outside `tui/fleet.rs`), not this verb's to add alone.

**Advertised** — `text.rs:274` `"debug.run"` directly after `"status"` (pi `shared/types.ts:2801`); `schema.rs:955` whole-list pin updated; `resources/docs/tool-reference.md` gains the row under `status` (the `the_tool_reference_topic_names_every_dispatched_verb` pin). `SUBAGENT_ACTIONS` 51 → 52.

**Production call sites (the chain, end to end):** `SubagentTool::execute` (`extension/tool/mod.rs`) → `normalize_public_subagent_execution` → `route_action` (`routing.rs:1085`, band `:1187`) → `route_control_action` (authority consult → `None`) → `"debug.run"` arm `:2215` → `SubagentExecutor::control_debug_run` (`executor/status.rs:343`) → `resolve_async_run_id` (`run_id_resolver.rs:183`) → `reconcile_now` (`reconcile.rs:331`) → `capacity_options` (`background.rs:392`) → `inspect_active_async_capacity_owner` (`inspect.rs`) → `check_pid_liveness` (`reconcile.rs:108`) → `format_run_lifecycle_debug`. Both registrations share the one `SubagentTool`, so the child-safe tool reaches it too (not in `:213`).

### Notes whose premise this change falsified — rewritten, not appended to

| Note | Was | Now |
|---|---|---|
| `extension/tool/text.rs:165-170` | "`debug.run` … an unported member" of `SUBAGENT_ACTIONS` | absent from `:213`, read-only, dispatched by pi's shared `status \|\| debug.run` arm AND by cyrup through the control band into `control_debug_run` |
| `extension/tool/schema.rs:984-985` | "cyrup omits `inspector.*`/`project.*`/`debug.run`" | "cyrup omits `inspector.*`/`project.*`" |
| `extension/tool/routing.rs:2342` | "only the six control verbs reach `route_control_action`" (already stale at seven) | "only the control band's verbs in `route_action`" |
| `docs/gap-analysis/PARITY-GAPS.md:36-40`, `:1575-1576` | `debug.run` among the remaining verbs; gap **8** | new CLOSED row for `debug.run`; gap **7** (`inspector.*`/`project.*`) |
| `docs/gap-analysis/00-residual-ledger.md:144-146`, `:312-313` | same count | same correction |
| `docs/gap-analysis/09-cyrup-ext-subagents.md:234`, `:681` | `debug.run` "genuinely unowned" | landed; nothing on SUBA-005's row is unowned |

**Notes re-grepped and left byte-for-byte (premise TRUE):** `active_run_index.rs:56-59,:359-363`, `inspect.rs` §D3 rungs, `key.rs:93-99`, `active_async_capacity/mod.rs` §D3, `rpc/ping.rs:60-66`, `doctor.rs:1019-1023`, `async_retention/policy.rs:365-373`. The seed's anchors the AUG flagged wrong (`run_status.rs:251` is `LiveWorkflowControlCandidate`, not `RunStatus`; no `lane` field anywhere) were confirmed wrong again (`grep -n 'pub lane\|pub parent_workflow_run_id' records.rs` → 0) and the dump prints none of those lines.

### Reachability tests

- **Test A** — `crates/cyrup-it/tests/subagents/debug_run_lifecycle_integration.rs`
  `debug_run_prints_the_real_capacity_slot_for_a_real_async_run` (registered in
  `tests/subagents/main.rs`; header 38 → 39 files). `SubagentExtensionConfig { spawn_command:
  subagent_fixture() (no script → exits 0), max_active_async_runs_per_session: Some(1), roots:
  Roots::sandboxed(home) }` → `with_config_and_cwd`; host `session_id() = "debug-session"`;
  `executor.spawn_background_steps(cwd, one worker step, Single, transfer_from: None)` — the
  PRODUCTION claim path writes `slot-0/owner.json` and `mark_started(pid)` binds the pid;
  `read_owner(slot_dir(session_pool_dir(..), 0))` → the real pid; bounded poll until
  `check_pid_liveness(pid) == Dead`; the runner's terminal `status.json` (`queued(run, Single,
  Some(pid))` → Running → Complete, session `debug-session`, one Complete step) written into the
  spawn-created run dir; then `ext.subagent_tool().execute({action:"debug.run", id})` asserting
  exact lines: `Run lifecycle debug` (first line), `Run:`, `Dir:`, `Status file:`, `Session:
  debug-session`, `State: complete`, `Mode: single`, `Process terminal: not recorded…`, `Runner
  pid: <pid> (dead)`, `Active capacity: releasable — runner pid <pid> is confirmed gone and the
  run is terminal`, `Capacity owner: current slot 0, runner, generation 0`, `Capacity session:
  debug-session`, `Capacity async dir: <run dir>`, `Capacity runner pid: <pid>`, `Capacity runner
  started: 20…`, NO `Process terminal file:` line, NO `Workflow children:`; then the `dir` form →
  the same `Capacity owner:` line.
- **Test B** — `extension/tool/routing_tests.rs:3074` `mod debug_run_refusals`, through
  `dispatch_tool` over a tool whose executor roots are `Roots::sandboxed(tempdir)`:
  `debug_run_is_advertised_directly_after_status` (`position("status") + 1`);
  `the_target_refusal_comes_before_the_view_refusal` (`{}` → TARGET; `{view}` → TARGET;
  `{id, view}` → NO_VIEWS); `the_three_location_refusals_are_pis_sentences` (unknown id →
  `Async run not found. Provide id or dir.`; `<results_dir>/<id>.json` only →
  `DEBUG_RUN_NEEDS_STATUS_DIR`; empty dir → `Status file not found.` and no `status.json`
  synthesised).
- **Test C** — unit, `run_lifecycle_debug.rs::tests`: `the_pid_probe_reports_exactly_what_it_saw`
  (`pid: None` → `Runner pid: not recorded`; injected `Unknown` → `(unknown)`, never `dead`; no
  `Process terminal file:`), `a_single_mode_status_renders_the_header_block_and_no_workflow_block`
  (the whole line vector, in upstream's order), `a_workflow_status_renders_its_children_and_one_line_per_step`
  (`Workflow children: 2` + the two step lines, session name preferred, `run <id>` suffix,
  `Workflow receipt:` when set).

### Mutations run (apply → targeted `cargo nextest` → observe FAIL → restore, `cmp`-verified)

| # | Mutation | Failing test |
|---|---|---|
| 1 | `format_run_lifecycle_debug`: `lines.extend(format_capacity_owner(capacity))` removed | Test A — `expected the line "Active capacity: releasable — runner pid <pid> is confirmed gone and the run is terminal"` (`:170`) |
| 2 | routing arm sends `debug.run` to `control_status_view` | Test A — first line `Run: <id>` ≠ `Run lifecycle debug` (`:240`) |
| 3 | `control_debug_run`: inspection replaced by a constant `relation: None` / `not-owned` | Test A — the `Active capacity: releasable — …` line missing (`:170`) |
| 4 | `control_debug_run`: probe replaced by a constant `Alive` | Test A — `expected the line "Runner pid: <pid> (dead)"` (`:170`) |
| 5 | upstream's `Process terminal file: <dir>/process-terminal.json` line reintroduced | Test A — `the dump must not name a sidecar this build never writes` (`:275` after `cargo fmt`; the run reported `:272` pre-format) |
| 6 | `"debug.run"` removed from `SUBAGENT_ACTIONS` | Test B — `debug_run_is_advertised_directly_after_status`: `left: Some("grant-spawn-budget") right: Some("debug.run")` (`:3107`) |
| 7 | the two refusals swapped (view before target) | Test B — `the_target_refusal_comes_before_the_view_refusal`: `{view}` alone answered `does not support status views.` (`:3132`) |

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` — ``Summary [  96.428s] 10676 tests run: 10676 passed, 9 skipped`` (baseline 10661 / 9 skipped; the predecessor's pass added 9, this one 6).
- `cargo nextest run -p cyrup-it --features it` (`CYRUP_IT_BIN_DIR` = the existing `target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug`, AWS vars unset) — ``Summary [ 312.461s] 567 tests run: 567 passed, 0 skipped`` (baseline 566; the delta is Test A).

### Residuals (TRUE premises, none closed here)

- **VL-S4** stays open: no process-terminal sidecar, overlay, reader or writer. `debug.run` is complete over cyrup's data and says so on its own output.
- The `Debug run` / `Status target:` label prefix and the spawn-budget trailer (`withSpawnBudgetStatus`) are a pre-existing status-side delta, unported for `status` and therefore not added for `debug.run` alone.
- `reconcile_by_id`'s NARROW reconciler for `status` is unchanged; `debug.run` uses the full one because upstream does (`:475`).
- The verb gap now reads **7** — `inspector.{open,command,status,close}` and `project.{open,status,close}`, each needing a third-party terminal binary; a separate decision, as the seed says.

### Second-pass verification (a concurrent EXEC, 2026-09-20 UTC)

A second `debug.run` EXEC pass was dispatched onto this tree while the pass above was mid-flight.
It found the verb already landed (`run_lifecycle_debug.rs`, `control_debug_run`, the routing arm,
the advertise pin, the docs row, Tests A/B/C), so it wrote **no source**: it waited for the tree to
go quiet (no cargo process and no source mtime change for a sustained window), then re-verified
everything above independently, on its own runs — nothing here is copied from the section above.

- **Premises re-grepped at the quiet tree:** `grep -rn 'Process terminal' src` → hits only in
  `run_lifecycle_debug.rs` and Test A; `process_terminal|ProcessTerminal` → no non-comment symbol;
  `git diff --stat` on `active_run_index.rs`, `active_async_capacity/mod.rs`, `rpc/ping.rs`,
  `registration/doctor.rs`, `async_retention/policy.rs` → empty (the TRUE-premise notes are
  byte-for-byte untouched); `control_debug_run` is `pub` at `executor/status.rs:343` and
  `SubagentExecutor` is re-exported at `extension/mod.rs:77`, so the rewritten `text.rs` note's
  doc-link resolves. Upstream re-read at the pin: `run-status.ts:47-108`,
  `subagent-executor.ts:6515-6538`, `shared/types.ts:2801` (`"status"` at token 36, `"debug.run"`
  at 37) — each matches the port line for line.
- **Tests, own run:** the 6 in-crate `debug.run` tests (`-E 'test(debug_run) or
  test(run_lifecycle_debug)'`) → `6 passed`; Test A (`-p cyrup-it --features it -E
  'test(debug_run)'`, `CYRUP_IT_BIN_DIR` = `target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug`,
  AWS vars unset) → `1 passed`.
- **Mutations re-run (apply → targeted nextest → FAIL → restore → `cmp` identical):**

  | # | Mutation | Failing test (own observation) |
  |---|---|---|
  | 1 (the DoD one) | `format_run_lifecycle_debug`: `lines.extend(format_capacity_owner(capacity))` removed | Test A — `expected the line "Active capacity: releasable — runner pid <pid> is confirmed gone and the run is terminal"` at `debug_run_lifecycle_integration.rs:170` |
  | 2 | `"debug.run"` removed from `SUBAGENT_ACTIONS` (`text.rs:274`) | Test B — `debug_run_is_advertised_directly_after_status`: `left: Some("grant-spawn-budget") right: Some("debug.run")` |
  | 3 | `control_debug_run`: `check_pid_liveness` replaced by `\|_\| Liveness::Alive` | Test A — `expected the line "Runner pid: <pid> (dead)"` at `:170` |

- **Gates, own run:** `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets
  --features test-fixtures -- -D warnings` clean; `cargo nextest run --workspace --features
  test-fixtures` → ``Summary [  95.815s] 10676 tests run: 10676 passed, 9 skipped``. The full
  `-p cyrup-it --features it` suite was NOT re-run by this pass (only Test A, above); the `567`
  figure is the first pass's.

---

## [FIX — round 1]

Remediation of the ten defects and five false premises, 2026-09-20, on `claude/subagents-verbs`
over the `[EXEC — debug.run]` tree. Upstream read ONLY via
`git -C tmp/pi-subagents show v0.68.0:<path>`; every premise below was re-grepped before it was
written; every mutation was restored and `cmp`-verified before the next. No git command that
writes to the repo was run.

### Fixed (10 of 10 defects; none refuted)

| # | Defect | Fix | Where |
|---|---|---|---|
| 1 | `debug.run` repaired-and-dumped a `status.json` it wrote itself when the run dir had a result but no `status.json` | The split is on the FILE'S EXISTENCE, BEFORE the reconciler: no `status.json` → `DEBUG_RUN_NEEDS_STATUS_DIR` when `location.result_path.is_some()` (pi `:714-721`), else `Status file not found.` (`:781-785`); `reconcile_now` runs only over a `status.json` that was already there. The `RepairedFromResult` carve-out and its false comment are gone. | `extension/executor/status.rs:393-426` |
| 2, 4, 6 | The `dir` form ignored a conflicting `id`/`runId` and accepted any directory | NEW `background::resolve_async_run_dir(dir, requested_id, cwd, async_root, results_dir)` — pi `resolveAsyncRunLocation:227-235`: Node `path.resolve` (cwd-join + lexical normalisation via `exec::output::normalize_lexically`), `assertInsideRoot` (`:229`; the root itself passes, as `relative === ""` does), the mismatch throw (`:231-233`), `exactResultPath` (`:234`). Three typed variants on `ResolveRunIdError` carry pi's sentences: `OutsideAsyncRoot` → `Async run directory must be inside <root>.`, `DirectoryMismatch` → `Async run id '<id>' does not match directory '<basename>'.`, `NoBasename`. `control_debug_run`'s `(Some(dir), requested)` arm calls it with `id.or(runId)`. | `background/run_id_resolver.rs:49-70,:76-135`; `status.rs:368-376` |
| 3 | Row `state`/`completed_at` were the WORKFLOW's; the doc cited `:97`/`:90` as if `run` were the parent | `RetainedChildState: TryFrom<StepState>` (refuses `Pending`/`Running`) is both `:87` and `:89` — the row IS the child; `state = try_from(step.status)`, `completed_at = step.ended_at ?? (status.ended_at ?? status.last_update)`; the workflow's own state no longer gates its rows (a settled child of a `Running` workflow is listed; a `failed` child of a `Complete` workflow prints `failed`); rung `:53` is `step.status == Stopped`. Because `workflow_step_statuses` derives rows from `WorkflowScriptChildResult`, which carries no settle time, NEW `workflows::carry_step_settle_times(previous, steps, now)` stamps `ended_at` on a newly terminal row and carries a predecessor's (matched by `workflow_key`, then `run_id`) across BOTH rebuilds — `publish_steps` (`workflow.rs:355`) and the terminal write (`workflow_launch.rs:636`). Field docs at `:176-188` now say whose the values are. | `background/retained_children.rs:84-129,:239-242,:353-407`; `workflows/child_summary.rs:740-765` |
| 5 | Candidate source was the indexed fleet history, which hides every foreground workflow run once either run index is non-empty | NEW `tui::fleet::collect_async_runs_by_scan` (pi `listAsyncRuns` with `repairScan`, `async-status.ts:503`; same `sessionId` filter `:585`, same `entryLimit` mtime pass) over the extracted `scan_async_root_candidates` (the former fallback of `fleet_history_candidates`, now shared, not duplicated) and `read_candidate_views` (the former body of `collect_fleet_history`). `list_retained_children` calls the scan; `collect_fleet_history` is unchanged for the inspector. | `tui/fleet.rs:488-546,:610-661`; `retained_children.rs:23-33,:373-374` |
| 7 | A deleted cwd printed `required cwd is missing`, upstream's sentence for "exists but not a directory" | `Ok(meta) if is_dir` passes; `Ok(_)` → `required cwd is missing: <path>` (`:69`); `Err(NotFound)` → `resume dependency unavailable: ENOENT: no such file or directory, stat '<path>'` (Node's ENOENT spelling, the `catch` at `:70-71`); any other `Err` → `resume dependency unavailable: <error>`. | `retained_children.rs:290-312` |
| 8 | Four in-tree notes claimed `not resumable (missing recovery descriptor)` is byte-for-byte `resume`'s refusal | Each corrected to the truth: same file, same verdict, DIFFERENT sentence — `resume` refuses with `RecoveryDescriptorError::Missing` (`recovery_descriptor.rs:155-161`, raised at `control.rs:319`). | `children_list_tests.rs:239-242`; `retained_children.rs:56-64`; `PARITY-GAPS.md:1607-1612`; `09-cyrup-ext-subagents.md:611`; this file's AUG §4 and EXEC R1 (edited in place, marked as corrected here) |
| 9 | `[CYRUP-DELTA]` cited "`grep runner records.rs` → 0" (actual: 28 comment hits) | Now states the true premise: `StepStatus` (`records.rs:20-160`) declares no `runner` field; every `runner` in that file is a comment citing `subagent-runner.ts`. | `retained_children.rs:43-46` |
| 10 | `children.list` answered `details: None` | `Some({ "mode": "management", "results": [] })` — pi `subagent-executor.ts:6471`. | `extension/tool/routing.rs:1123-1125` |

### False premises (5 of 5; each grepped, then corrected or deleted)

1. `status.rs` comment on upstream's fall-through — DELETED with the carve-out; the replacement
   (`:403-412`) states what upstream does (`stale-run-reconciler.ts:369` returns `status: null`
   with no `status.json`; never repairs) and what cyrup's `reconcile_now` would do
   (`reconcile.rs:355-445` `repair_from_result`, `existing: None => needs_repair = true`, writes
   `status.json` + `update_active_run_index`), which is why the existence check comes first.
2. `retained_children.rs` `:161`/`:167` — the field docs now name the CHILD (`run` at
   `:85-104` is selected by `run.parentWorkflowRunId && run.steps.length === 1`) and the step
   row's `status`/`ended_at` as the data.
3. `retained_children.rs:23-26` — rewritten: the scan is `collect_async_runs_by_scan`, and the
   paragraph states why `collect_fleet_history` cannot serve (indexed candidates; foreground
   workflow runs write no index — `settle_foreground_workflow` writes `status.json` + receipt;
   `grep -rn 'update_active_run_index(' src` production callers: `runner_main/entry.rs`,
   `runner_main/finish.rs`, and `reconcile.rs`'s repair).
4. "byte-for-byte what `resume` refuses with" — corrected in all four places (defect 8).
5. "`grep runner records.rs` → 0" — corrected (defect 9).

### Tests (all through the production path unless marked unit)

- `children_list_tests.rs::children_list_lists_a_settled_workflow_child_with_the_real_predicate`
  — now pins `details` (`:209-213`); `completed_at` from `steps[0].endedAt` ≤ the workflow's
  `endedAt` (`:192-203`); **Case A′** (`:247-284`): a Complete single-run status for the same
  session filed with the PRODUCTION `update_active_run_index` → the child is STILL listed;
  **Case D** (`:343-393`): descriptor cwd = an existing subdir → `resumable`; `rmdir` →
  `resume dependency unavailable: ENOENT: no such file or directory, stat '<path>'`; a file in
  its place → `required cwd is missing: <path>`.
- NEW `children_list_tests.rs::children_list_prints_the_childs_own_state_under_a_completed_workflow`
  (`:417-511`): `write_failing_child_binary` (`testsupport.rs:343`, exit 1) under
  `try { await runs.run(...) } catch (e) {}`; the workflow's `status.json` is `complete` with
  step 0 `failed`; the row prints `| worker | failed |`.
- NEW unit `retained_children.rs::the_row_carries_the_childs_state_not_the_workflows` (`:609`):
  a `Running` workflow with steps `[Complete(child-a), Running(child-b)]` and a `Complete`
  workflow with `[Failed(child-c)]` → `[(child-c, Failed, its endedAt), (child-a, Complete, its
  endedAt)]`; another session lists nothing. `only_retained_step_states_convert` (`:546`) replaces
  the `RunState` conversion test.
- NEW unit `child_summary.rs::settle_times_are_carried_by_key_then_run_id_and_stamped_once` (`:783`).
- `routing_tests.rs::the_three_location_refusals_are_pis_sentences` (`:3146`) — a run dir with
  `events.jsonl` + `<results_dir>/<id>.json` and no `status.json`, by `id` AND by `dir` →
  `DEBUG_RUN_NEEDS_STATUS_DIR`, and no `status.json` appears (`:3185-3206`).
- NEW `routing_tests.rs::the_dir_form_refuses_an_outside_directory_and_a_mismatched_id` (`:3213`):
  an outside dir and a `..`-escaping dir → `Async run directory must be inside <root>.`;
  `id`/`runId` ≠ basename → the mismatch sentence; the matching id passes to the on-disk refusal.

### Mutations run (apply → targeted `cargo nextest` → observe FAIL → restore, `cmp`-verified)

| # | Mutation | Failing test |
|---|---|---|
| 1 | `list_retained_children` back on `collect_fleet_history` | `…with_the_real_predicate` — Case A′ (`children_list_tests.rs:277`), the empty sentence after the index write |
| 2 | row `state` from the WORKFLOW's `status.state` again | `the_row_carries_the_childs_state_not_the_workflows` (`left: (child-c, Complete…)`), `children_list_prints_the_childs_own_state…` (`:469`) |
| 3 | `carry_step_settle_times` stamps nothing | `settle_times_are_carried…` (`kept by workflow key: left None`), `…with_the_real_predicate` (`:196`, `endedAt` missing on the row) |
| 4 | cwd rung's ENOENT arm collapsed to `required cwd is missing` | `…with_the_real_predicate` — Case D (`:371`) |
| 5 | `reconcile_now` before the existence split (the old carve-out) | `the_three_location_refusals_are_pis_sentences` (`:3200`) — the reconciler parsed the `{}` result (`missing field id`) instead of refusing |
| 6 | `assertInsideRoot` dropped (`if false && …`) | `the_dir_form_refuses…` (`:3225`) — `Status file not found.` ≠ the inside-root sentence |
| 7 | the `id`/`dir` mismatch check dropped | `the_dir_form_refuses…` (`:3262`) — `Status file not found.` ≠ the mismatch sentence |
| 8 | `children.list` `details: None` again | `…with_the_real_predicate` (`:210`) |

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy --workspace --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run --workspace --features test-fixtures` — ``Summary [  95.609s] 10680 tests run: 10680 passed, 9 skipped`` (previous 10676 / 9; +4: two `children_list_tests`/`routing_tests` cases and two units — `only_retained_step_states_convert` replaced its predecessor one-for-one).
- `cargo nextest run -p cyrup-it --features it` (`CYRUP_IT_BIN_DIR=target/debug/build/cyrup-it-87b505e3149e7da5/out/it-bins/debug`, AWS vars unset) — ``Summary [ 347.549s] 567 tests run: 567 passed, 0 skipped`` (unchanged: Test A still passes with the `dir` form now asserted inside the root).

### Residuals (TRUE premises)

- `status`'s own `dir` form (`run_status::reconcile_by_dir`) still takes the directory as given
  without `assertInsideRoot` or the mismatch check — the pre-existing deviation the defect named;
  `resolve_async_run_dir` is the port it can adopt. Not this verb's.
- R1/R2/R3 of `[EXEC — children.list]` stand, with R1's wording corrected (defect 8).
- VL-S4 unchanged.

## [FIX — round 2]

Remediation of the three (triplicated) defects and the five listed premises, 2026-09-20, on
`claude/subagents-verbs` over the `[FIX — round 1]` tree. Upstream read ONLY via
`git -C tmp/pi-subagents show v0.68.0:<path>`; every premise below was re-grepped before it was
written; the one mutation was restored and `cmp`-verified. No git command that writes to the
repo was run.

### Fixed (3 of 3 distinct defects; none refuted)

| # | Defect | Fix | Where |
|---|---|---|---|
| 1 | "the only production callers of `update_active_run_index` are `runner_main/entry.rs` and `runner_main/finish.rs`" (`retained_children.rs:31-33`, echoed at `fleet.rs:495-497`) — `grep -rn 'update_active_run_index(' src` outside `#[cfg(test)]` finds SIX: `runner_main/entry.rs:323`, `runner_main/finish.rs:374`, `active_run_index.rs:393` (`read_live_active_run_ids`, stale-marker release), `reconcile.rs:431` (`repair_from_result`), `reconcile.rs:554` (stale-dead mark), `workflow_detach/mod.rs:449` (DETACHED workflow settle) | Rewritten to the true reason: both indexes are filed through that ONE writer (the terminal index is written from inside it, `active_run_index.rs:280`; `update_terminal_run_index` has no other production caller), `settle_foreground_workflow` never calls it, and the six callers are named with what each runs over. The rewrite also records the case the old sentence hid: the foreground workflow's status carries THIS process's pid (`workflow_launch.rs:157-162`), so a workflow left `Running` by a dead host IS filed as `Failed` by the stale-dead mark (`reconcile.rs:283-320` step 3/4 → `:554`) the next time something reconciles it. The conclusion is therefore stated for what is true — a SETTLED foreground workflow is in neither index; a crash is filed, settled step rows never are — which is why the scan, not the index, is the retention source. `fleet.rs` now points at that paragraph instead of restating a caller list. | `retained_children.rs:28-45`; `tui/fleet.rs:494-501` |
| 2 | `collect_async_runs_by_scan`'s doc cited `async-status.ts:378-379` (the `stopRequestedAt`/`turnBudget` step-field spreads) | `:486` (the `listAsyncRuns` signature) and `:503` (the `repairScan` arm) — `git show v0.68.0:src/runs/background/async-status.ts \| sed -n '486p;503p'`. | `tui/fleet.rs:489-490` |
| 3 | Stale assertion message "the workflow's state and endedAt" on the fixed row assertion | "the child's own state and endedAt (pi `:97` `run.state`, `:90` `run.endedAt`, `run` being the child)" — `retained-children.ts:86-90,97` @v0.68.0. | `children_list_tests.rs:231-232` |

### False premises (5 listed; 1 corrected here, 4 already corrected in round 1 — each re-grepped)

1. `status.rs` "upstream would repair-and-proceed when a result exists" — ALREADY GONE in round 1
   (defect 1 there). The current comment (`status.rs:403-412`) says: upstream's `reconcileAsyncRun`
   returns `status: null` with no `status.json` and never repairs (`stale-run-reconciler.ts:369`
   `if (!effectiveStatus) return { status: null, repaired: false }`; the `startedRun` synthesis at
   `:367` is not passed by `run-status.ts:473` — `grep -n startedRun run-status.ts` → 0), so the
   verb reaches the `:714-721` refusal with a result file and `:781-785` without. Re-verified
   against `run-status.ts:463-468,:471-474,:714-721,:781-785` @v0.68.0 this round.
2. `retained_children.rs` `:161`/`:167` (`run.state`/`run.endedAt` "the WORKFLOW's") — ALREADY
   CORRECTED in round 1; now `:187-188` and `:194-198` say the CHILD's (`run` at
   `retained-children.ts:85-86` is the child async run, selected by `run.parentWorkflowRunId &&
   run.steps.length === 1`).
3. `retained_children.rs:23-26` ("the scan is `collect_fleet_history` … reused") — ALREADY
   REWRITTEN in round 1 to `collect_async_runs_by_scan` (`retained_children.rs:385` is the call);
   the paragraph's REASON was defect 1 above and is now true.
4. "byte-for-byte what `resume` refuses with" — ALREADY CORRECTED in round 1 at all four sites;
   `grep -rn 'byte-for-byte' docs/gap-analysis crates/cyrup-ext-subagents/src` no longer hits
   `children.list`/`resume`; `09-cyrup-ext-subagents.md:611` and `PARITY-GAPS.md` say "different
   sentence".
5. "`grep runner records.rs` → 0" — ALREADY CORRECTED in round 1 (`retained_children.rs:55-58`:
   `StepStatus` declares no `runner` field; `grep -c runner records.rs` → 28, all comments).

Also corrected by this round, though not on the list: round 1's own §"False premises" item 3
above named three production callers of `update_active_run_index`; the true set is the six in
defect 1. That paragraph is left as written (it is a dated record) and superseded here.

### Tests

No new test: every edit is a doc comment or an assertion message. The contract the message
describes is pinned by the existing production-path tests
`children_list_tests.rs::children_list_lists_a_settled_workflow_child_with_the_real_predicate`
(`:192-203,:226-233`, Case A′ `:247-284` for the scan-vs-index premise) and
`children_list_prints_the_childs_own_state_under_a_completed_workflow`, plus the unit
`retained_children.rs::the_row_carries_the_childs_state_not_the_workflows`.

### Mutations run (apply → targeted `cargo nextest` → observe FAIL → restore, `cmp`-verified)

| # | Mutation | Failing test |
|---|---|---|
| 1 | `retained_children.rs:397` row `state` ← `try_from(StepState::Complete)` (constant, ignoring the step) | `the_row_carries_the_childs_state_not_the_workflows` (`left: [("child-b", Complete…), ("child-c", Complete…), …]` vs `right: [("child-c", Failed…), …]`), `children_list_prints_the_childs_own_state_under_a_completed_workflow` |

### Gates

- `cargo fmt --all -- --check` — clean.
- `cargo clippy -p cyrup-ext-subagents --all-targets --features test-fixtures -- -D warnings` — clean.
- `cargo nextest run -p cyrup-ext-subagents --features test-fixtures` — ``Summary [  38.256s] 4237 tests run: 4237 passed, 0 skipped``.
  (Crate-scoped: the diff is comments and one assertion string in this crate; disk had 704 MB free,
  so the workspace and `cyrup-it` runs from round 1 were not repeated.)

### Residuals (TRUE premises)

- Unchanged from round 1. One premise is now recorded rather than hidden: a foreground workflow
  whose host process dies mid-run is filed into the terminal index as `Failed` by the reconciler's
  stale-dead mark; `children.list` still lists its settled rows (they are in its `status.json`,
  which the scan reads), and the index entry is not read by the listing.

## [FIX — round 3]

Orchestrator pass, 2026-09-20, after a second QA sweep (a duplicate run of this workflow that ran
concurrently with the first; its executors stood down, its QA lenses did not). Three findings
survived the first run's two remediation rounds; each is fixed in code, with a mutation proof.

1. **False premise — `entryLimit`** (`tui/fleet.rs` `collect_async_runs_by_scan` doc and
   `retained_children.rs` module doc): upstream's `repairScan` arm (`async-status.ts:503-504`) is
   an unbounded `readdirSync`; `entryLimit` is consumed only by the per-session terminal-index read
   (`:519`). cyrup's scan cut to the 100 newest `status.json` across ALL sessions BEFORE the
   session filter — a real narrowing, not just a wrong comment. Fix: `scan_async_root_candidates`
   takes `limit: Option<usize>`; the fleet's index fallback keeps `Some(100)`, `children.list`'s
   scan passes `None`, filters by session, then truncates. Test
   `newer_runs_of_other_sessions_do_not_push_this_sessions_out_of_the_window` plants 100 newer
   `s-2` workflows (mtimes pinned with `File::set_modified`) and one older `s-1` workflow; bound-
   before-filter mutation → `left: [] right: ["child-mine"]`; restored.
2. **External-runner rung dropped though the datum is in hand** (`retained-children.ts:54`): the
   same `results[0]` the settle reads `sessionFile`/`model`/`usage` off carries
   `SingleResult::runner` (`exec/run_result.rs`, `ExternalCliRunnerStatus`, cyrup has real external
   CLI runners). Fix: `StepStatus::runner: Option<ExternalCliRunnerStatus>` (pi
   `AsyncJobStep.runner`, `shared/types.ts:1315`; optional on the wire, old files round-trip),
   filled by `workflow_step_statuses`, and `child_resumability` refuses `external-cli` /
   `external-job` with pi's `external runner` before the session rung. The
   `[CYRUP-DELTA]` bullet claiming "no `runner` field … every `runner` in that file is a comment
   citing `subagent-runner.ts`" (also overstated — 12 of the 28 hits cite nothing) is deleted with
   its premise. Tests: `an_external_runner_is_never_resumable` (gut the return → `missing recovery
   descriptor`), `step_rows_carry_session_model_usage_and_runner_from_the_first_result` (drop the
   fill → `left: None`); both restored. Ledger residual R2 closed.
3. Ledger `children.list` row updated for both (R2 closed; the scan's filter-before-bound
   recorded).
