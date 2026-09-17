---
stage: qa
status: completed
updated: 2026-09-14 20:00
---

# SCOPE_8 — detached-workflow-child completion reconciliation

> Renamed 2026-09-09 from `SCOPE_8.md` — position 9 of 12 in the WORKFLOW_1|WORKFLOW_12 dependency-ordered sequence.

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

---

> ## ⚠⚠ RE-AUG 2026-09-14 — READ THIS BEFORE ANYTHING ELSE
>
> The body below (the 2026-09-12 aug) was written **before SCOPE batch 1 landed 9715 insertions**
> (PR #137: SCOPE_3, SCOPE_4, SCOPE_3j, SCOPE_18, SCOPE_17). **Every `file:line` in the pre-existing
> body has been re-opened and re-verified on the current tree.** Two classes of correction follow:
>
> 1. **Path base.** Every link in the old body pointed at `../../../workspace/cyrup/…`, a tree that
>    does not exist here. The repo root is `/home/user/cyrup`; the crate is
>    `crates/cyrup-ext-subagents`. All citations below are re-spelled crate-relative as
>    `src/<path>:<line>` and must be read that way.
> 2. **Upstream pin.** The old body cited *"HEAD `57278d82`"* — an unpinned working-tree HEAD
>    (today's HEAD is `4ab1b1b8`). **Every upstream line number in the old body's write-path section
>    is wrong by ~20 lines.** The whole file is re-pinned to **`v0.67.0`** and re-read with
>    `git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:<path>`. §U below is the corrected
>    line map; where the old body's number disagrees, **§U wins**.
>
> **Triage status re-verified 2026-09-14: GENUINELY NOT STARTED.**
> - `src/extension/executor/workflow_detach/` does not exist.
> - `src/extension/executor/mod.rs:8-30` declares 20 submodules; `workflow_detach` is not among them
>   (`workflow_child_stops`, `workflow_controllers`, `workflow_steering` are — the alphabetical slot
>   the new module takes is between the latter two, exactly as SUBTASK1 says).
> - `reconcile_detached_workflow_child_completion`, `promote_paused_workflow_if_settled`,
>   `apply_detached_child_to_paused_workflow`, `workflow_result_children`,
>   `backfill_live_child_identity`, `reconcile_workflow_receipt`, `DetachedWorkflowChildCompletion`
>   — **zero occurrences** anywhere under `crates/`.
> - `apply_workflow_settlement_plan` still carries `#[cfg_attr(not(test), allow(dead_code))]`
>   (`src/background/runner_main/finish.rs:203`).
>
> **Nothing in this task is already implemented.** Everything below is work to do.
>
> New material added by this re-aug, all of it ADDITIVE: §U (upstream line map), §V (verified cyrup
> seam inventory), §W (six things that cannot work as the old body describes, each recorded next to
> its original requirement), §X (named tests), and the `⟡ AUG` inline blocks. **No original
> requirement has been weakened or deleted.**

---

> ## ⚠ SCOPE CORRECTION (2026-09-12 aug) — two of the three original subtasks were factually wrong
>
> The pre-aug draft of this file was written from the filename, not the source. Reading
> `workflow-detach-reconcile.ts` (pinned: `v0.67.0:src/runs/foreground/workflow-detach-reconcile.ts`)
> in full falsifies both of its headline claims:
>
> 1. **There is no "adoption" in this file.** The old SUBTASK2 ("adopt only this session's
>    detached workflows", "gate on session before taking ownership") describes behaviour that does
>    not exist anywhere upstream. The file never takes ownership of anything; it reconciles a
>    workflow that *this* process is already the parent of. The three `sessionId` references the
>    draft correctly counted are **propagation**, not a predicate — see §2.
> 2. **This file never touches `workflowControllers`.** The old SUBTASK3 ("a reconciled/adopted
>    workflow inserts into `workflow_controllers`; a released one removes") is false. The registry
>    is mutated at exactly four upstream sites, all outside this file:
>    `subagent-executor.ts:5098` (insert), `:5272` (rollback), `:5796` (settlement `finally`), and
>    `extension/index.ts:1041` (teardown clear) — **all four already ported by WORKFLOW_6** into
>    `src/extension/executor/workflow_controllers.rs`.
>    There is no leak for this task to fix and no `liveWorkflowRunIds` corruption for WORKFLOW_10
>    to inherit from here.
>
> Both are replaced below. The **real** content of this task is narrower and better-supported than
> the draft implied: the helper layer is already ported, and this task is the **production caller**
> the prior tasks explicitly deferred to it.

**OBJECTIVE:** port `reconcileDetachedWorkflowChildCompletion` — the lifecycle that settles a
**paused** workflow when one of its **detached foreground children** finally exits, so the workflow
is neither stranded `paused` forever nor silently promoted to "complete" on evidence that its
JavaScript continuation never actually persisted.

**Depends on WORKFLOW_6** (`src/extension/executor/workflow_controllers.rs`) — landed.

---

## §0 — Ground truth: what is already ported (verified, do not re-port)

The draft budgeted "302 LOC". That is the upstream file's size, **not** the remaining work. The
helper layer it imports was ported wholesale by earlier tasks. Verified inventory:

> ⟡ **AUG 2026-09-14 — every line number in this table was stale by ~4 and is corrected below.**
> `settlement.rs` is now 1231 lines; `receipt.rs` 1466; the module-level items were re-enumerated
> with `grep -n '^pub fn \|^pub struct \|^pub enum ' src/workflows/settlement.rs`. **Also note the
> visibility correction: every one of these is `pub fn`, not `pub(crate) fn`** — they are re-exported
> from `src/workflows/mod.rs:140-150`, so the new module imports them as `crate::workflows::*`.

| upstream symbol (`workflow-settlement.ts` / `workflow-receipt.ts`) | cyrup equivalent (VERIFIED 2026-09-14) | status |
|---|---|---|
| `applyDetachedChildSettlement` | `workflows::apply_detached_child_settlement` (`src/workflows/settlement.rs:273`, was cited `:271`) | ✅ ported |
| `promoteSettledPausedWorkflow` | `workflows::promote_settled_paused_workflow` (`settlement.rs:165`) | ✅ ported |
| `findWorkflowSettlementStep` | `workflows::find_workflow_settlement_step` (`settlement.rs:132`) | ✅ ported (returns an **index**) |
| `classifyWorkflowSettlement` | `workflows::classify_workflow_settlement` (`settlement.rs:386`, was cited `:382`) | ✅ ported |
| `planWorkflowSettlement` / `WorkflowSettlementPlan` | `workflows::plan_workflow_settlement` (`settlement.rs:653`, was `:649`), `PlanWorkflowSettlement` (`settlement.rs:596`, was `:592`), `WorkflowSettlementPlan` (`settlement.rs:627`) | ✅ ported |
| `workflowRecoveryActions` | `workflows::workflow_recovery_actions` (`settlement.rs:417`, was `:413`) | ✅ ported |
| `workflowTerminalOutcomeForResult` | `workflows::workflow_terminal_outcome_for_result` (`settlement.rs:470`, was `:466`) — **takes `WorkflowBudgetSignals`, NOT `&SingleResult`**; see §W-4 | ✅ ported |
| `workflowOutputPathMappingSummary` | `workflows::workflow_output_path_mapping_summary` (`settlement.rs:494`, was `:490`) + `output_path_mappings_of` (`settlement.rs:519`, was `:515`) | ✅ ported — **and both are DEAD today, with zero callers anywhere; this task wakes them too** |
| `readWorkflowReceipt` / `writeWorkflowReceipt` / `workflowReceiptPath` | `workflows::{read,write}_workflow_receipt`, `workflow_receipt_path` (`src/workflows/receipt.rs:657` / `:604` / `:441`; old cites `660/607/446` were off by 3-5) | ✅ ported — **`read_workflow_receipt` returns `Result`, never `Option`; see §W-3** |
| `WorkflowReceipt` / `Entry` / `Resume` | `receipt.rs:316` / `:231` / `:164` (old cites `321/233/164`) | ✅ ported |
| `normalizeExternalCliRunnerStatus` / `externalCliReceiptMetadata` | `src/runner/status.rs:316` / `:402` — **both line numbers verified correct** | ✅ ported |
| `WorkflowPublicChild` | — | **intentionally absent**: cyrup's terminal payload is the typed `ResultFile` (`src/background/records.rs:483`) with `results: Vec<SingleResult>`, not an open map. Do **not** introduce a `PublicChild` type. |
| `WorkflowStatusStep` | — | folded into `background::StepStatus` (`records.rs:24`); `output_path_mapping` is already a field (`records.rs:137` — **verified exact**). |

**The load-bearing consequence:** `apply_detached_child_settlement`,
`promote_settled_paused_workflow` and `classify_workflow_settlement` currently have **zero
production callers** — only their own unit tests. They are ported, correct, and dead. **This task
is what wakes them up.** That is the actual value proposition, and it is why this task is worth
doing even though the helper layer is complete.

> ⟡ **AUG 2026-09-14 — re-verified by exhaustive grep, and the list is LONGER than claimed.**
> `grep -rn '\b<sym>\b' --include=*.rs crates/` excluding `src/workflows/settlement.rs` and
> `src/workflows/mod.rs` finds, for each:
> - `apply_detached_child_settlement` → ONE hit, a doc-comment reference at `src/background/records.rs:127`. **No caller.**
> - `promote_settled_paused_workflow` → ONE hit, a doc-comment at `records.rs:316`. **No caller.**
> - `classify_workflow_settlement` → ONE hit, a doc-comment at `records.rs:128`. **No caller.**
> - `find_workflow_settlement_step` → **ZERO hits**. Reached only from inside `apply_detached_child_settlement`.
> - `workflow_output_path_mapping_summary` → **ZERO hits.** Add it to the wake list.
> - `output_path_mappings_of` → **ZERO hits.** (But see §W-6: its `&[WorkflowScriptChildResult]`
>   adapter is the WRONG one for this task; use the generic `IntoIterator` signature directly.)
>
> For contrast, `plan_workflow_settlement` **already has** a production caller —
> `SubagentTool::settle_foreground_workflow` at `src/extension/tool/routing.rs:894` / `:1056`. That
> function (`routing.rs:1002-1098`) is the single best in-tree precedent for this task's write path;
> see §W-2.

---

## §1 — The one real blocker: there is no detach hook, so there is no caller

Upstream's call site is `subagent-executor.ts:3951-3976` (pin: `v0.67.0`) —
the `onDetachedExit` callback, fired from `execution.ts:2364`
when a foreground child that was **detached** (via `detachForeground`, `execution.ts:638`, reached
either by the `/detach` slash command at `slash-commands.ts:984` or by intercom coordination at
`execution.ts:788`) eventually terminates.

**cyrup has none of that machinery.** Verified (re-verified 2026-09-14):

- No `detach_foreground`, no `on_detach_ready`, no `on_detached_exit` anywhere in the crate.
- cyrup's `SingleResult::detached` (`src/exec/run_result.rs:62` — **verified exact**)
  is set by the drive loop (`src/exec/drive_attempt.rs`: `detached_seen` field at `:183`, read into
  the result at `:247`, set at `:350-353` — the old cite `:183`/`:247` is correct) when a blocking
  `contact_supervisor` ask is surfaced — the *same trigger family* as upstream's intercom arm, but
  it is a **flag on the returned result**, not a "return the tool call early and keep the child
  running" mechanism, and nothing fires on the detached child's later exit.
- The only detached-exit remnant is `disarm_structured_guard_on_detach`
  (`src/exec/mod.rs:907`, called at `:681` — **old cite `:879` was stale, corrected**) —
  cleanup is *disarmed*, and then never later performed.

### Decision — build the reconciler as a caller-independent, disk-driven unit

Do **not** build a detach mechanism in this task (that is a foreground-lifecycle task, not this
one, and the draft's dependency chain does not authorise it). Instead:

**Port the reconciler so its input is the run directory plus a settled child result — not a live
callback closure.** Every value upstream reads off in-process state has a durable source:

| upstream in-process read | disk / registry source to use instead (VERIFIED) |
|---|---|
| `state.asyncJobs.get(workflowRunId)?.asyncDir` (`:178-179`) | `RunPaths` for the workflow run (`src/background/run_paths.rs:85`, constructed by `RunPaths::for_run(async_root, results_dir, run_id)` at `:129`) — upstream already falls back to `path.join(DIRS.async, runId)` when the job is absent, so the disk path is the *primary*, not a degradation. **Upstream's `if (job) { job.status = … }` block at `:257-263` has NO cyrup analogue and is correctly dropped: cyrup's tracker is refreshed by re-reading `status.json`, not by in-place mutation.** |
| `state.foregroundControls.values()` filtered by `parentWorkflowRunId`/`workflowKey` (`:182-184`) | `ForegroundControlEntry` (`src/extension/executor/notices.rs:20`) — `session_id` (`:68`), `parent_workflow_run_id: Option<RunId>` (`:73`), `workflow_key: Option<WorkflowKey>` (`:78`) — already populated by WORKFLOW_6. **But the entry has NO `run_id` field: see §W-5.** |
| `readStatus(asyncDir)` (`:180`) | `read_status_file` (`src/background/control.rs:316`, `pub(crate) async fn read_status_file(path: &Path) -> Result<Option<RunStatus>, SubagentError>` — **verified exact**) |
| `JSON.parse(readFileSync(resultPath))` (`:202-209`) | resolve with `RunPaths::resolve_result(&session_id, &run_id)` (`run_paths.rs:155-168`) or `result_index::resolve_payload` (`src/background/result_index/locate.rs:153`), then `serde_json::from_slice::<ResultFile>` — the idiom at `src/background/watch/results_watcher.rs:217`. **`background::control::read_result_file` (`control.rs:326`) is PRIVATE to that module; do not try to call it.** |

This keeps the module **fully exercisable today** against a tempdir containing a `paused` workflow
`status.json`, and makes wiring it to a future detach hook a one-line call rather than a rewrite.
It is the identical posture `src/background/runner_main/finish.rs:182-203`
already documents for `apply_workflow_settlement_plan` — see §3.

---

## SUBTASK1 — `extension/executor/workflow_detach/`

**Where:** new module `crates/cyrup-ext-subagents/src/extension/executor/workflow_detach/`, declared
in `src/extension/executor/mod.rs` alongside its siblings (`pub(crate) mod workflow_detach;`,
alphabetical — after `workflow_controllers` (`mod.rs:28`), before `workflow_steering` (`mod.rs:29`)).

> ⟡ **AUG 2026-09-14 — verified.** `src/extension/executor/mod.rs:8-29` is the declaration block:
> `background, chain, control, foreground, foreground_actions, foreground_control,
> foreground_history, nested_control, notices, paths, reports, requests, resolve, session_state,
> spawn_budget, status, workflow, workflow_child_stops, workflow_controllers, workflow_steering` —
> all `pub(crate) mod`, strictly alphabetical. The new line goes between `:28` and `:29`.

**Ports:** `v0.67.0:src/runs/foreground/workflow-detach-reconcile.ts` (304 lines).

**Layout** — the draft's `{detect,reconcile,report}` split does not match the source's actual seams.
Use the three the file really has:

```
workflow_detach/
  mod.rs        // the driver: reconcile_detached_workflow_child_completion (upstream :170-304)
  identity.rs   // the live-identity backfill guard (:182-191) + usage_with_value (:44-48)
  receipt.rs    // reconcile_workflow_receipt (:97-159) — resumability + external-adapter merge
  children.rs   // workflow_result_children (:50-95) — rebuild the results array
```

`report.rs` is not warranted: "reporting" upstream is two `appendFileSync` calls to `events.jsonl`
plus one event emit, all inside the driver, and cyrup already owns the bounded-append primitive
(`RunPaths::events` must be written through `crate::jsonl::BoundedJsonlWriter`, per its own doc at
`src/background/run_paths.rs:88-94` — **verified exact**).

> ⟡ **AUG 2026-09-14 — the bounded-append API, concretely.**
> `BoundedJsonlWriter::create(path: &Path) -> io::Result<Self>` (`src/jsonl.rs:86`) opens
> create+append and seeds `bytes_written` from the existing file size; `create_with_cap(path, cap)`
> (`:97`) is the test-sized sibling. `write_line(&mut self, line: &str) -> io::Result<()>`
> (`src/jsonl.rs:127`) appends `line` + `\n` **all-or-nothing** and becomes a silent no-op once the
> cap is reached (R-SA-136) — a dropped event is NOT an error and must not fail the reconcile.
> **Do NOT use `background::runner_main::events::append_event`**: it is `pub(super)` inside the
> private `mod events;` (`src/background/runner_main/mod.rs:79`), unreachable from
> `extension::executor`, and it stamps its own `type`+`ts` in a shape that does not match
> upstream's `{ts, runId, type, …}` at `:266-272`. Hand-build the two event objects.

### The driver, prescriptively

Upstream's driver is a straight pipeline. Port it in the same order — the order **is** the
behaviour (a receipt failure must be able to demote a status that was already promoted):

```rust
/// pi `reconcileDetachedWorkflowChildCompletion` (`workflow-detach-reconcile.ts:170-304` @v0.67.0).
///
/// Returns `false` — WITHOUT writing anything — when the run has no status, or when the status is
/// not a `paused` workflow, or when the settling child matches no step. Upstream's three early
/// `return false`s are the idempotency guarantee: replaying this after the workflow already
/// settled is a no-op, because `apply_detached_child_settlement` refuses a non-`Paused` status
/// (`settlement.rs:278-282`).
pub(crate) async fn reconcile_detached_workflow_child_completion(
    input: DetachedWorkflowChildCompletion<'_>,
) -> Result<bool, SubagentError> {
    let Some(status) = read_status_file(&input.run_paths.status).await? else {
        return Ok(false);                                    // upstream `:181`  (old cite `:173`)
    };

    // `:182-191` — live-identity backfill, BEFORE settlement. See SUBTASK2.  (old cite `:175-190`)
    let mut status = backfill_live_child_identity(status, &input);

    let Some(next) = apply_detached_child_settlement(
        &status,
        DetachedChildSettlement {
            child_run_id: input.child_run_id,
            exit_code: input.result.exit_code,
            error: input.result.error.as_deref(),
            interrupted: input.result.interrupted,
            session_file: input.result.session_file.as_deref(),
            session_name: input.result.session_name.as_deref(),
            stopped: input.result.stopped,
            workflow_key: input.workflow_key,
            now: crate::time::now_epoch_millis(),
        },
        input.trace,
    )? else {
        return Ok(false);                                    // upstream `:197`  (old cite `:196`)
    };
    let mut next = next;

    // `:198-201` — the settled step keeps the mapping derived from THIS result's task/output.
    if let Some(mapping) = &input.result.output_path_mapping
        && let Some(index) = find_workflow_settlement_step(&next.steps, input.child_run_id, None, None)
        && let Some(step) = next.steps.get_mut(index)
    {
        step.output_path_mapping = Some(mapping.clone());
    }

    // `:213` — classification is computed on the SETTLED status, before the receipt pass.
    let resolution = classify_workflow_settlement(&next, input.result.interrupted);

    // `:214-220` — a receipt fault is CAPTURED, never propagated: available child evidence must
    // still be published. `plan_workflow_settlement` is what turns it into a failed status.
    let (receipt, receipt_path, receipt_error) =
        match reconcile_workflow_receipt(&next, input.child_run_id, &input.result, input.run_paths, resolution) {
            Ok(Some(published)) => (Some(published.receipt), Some(published.path), None),
            Ok(None) => (None, None, None),
            Err(e) => (None, None, Some(format!("Failed to reconcile async workflow receipt: {e}"))),
        };

    let children = workflow_result_children(&next, input.child_run_id, &input.result, existing.as_ref(), receipt.as_ref());
    let summary = detached_settlement_summary(&next, &children, input.child_run_id, resolution, receipt.as_ref(), existing.as_ref());

    let plan = plan_workflow_settlement(PlanWorkflowSettlement {
        status: &next,
        summary,
        trace: input.trace,
        receipt,
        receipt_path,
        receipt_persistence_error: receipt_error.clone(),
        resolution,
        terminal_outcome: receipt.as_ref().and_then(|r| r.terminal_outcome.clone()),
        now: None,
        event_metadata: reconciled_from_detached_child(input.child_run_id),   // `:253`
    });
    // …then §3's write path.
}
```

> ⟡ **AUG 2026-09-14 — FIVE things in that sketch do not compile against the current tree.**
> Each is stated next to the line it breaks; none of them changes what the driver must *do*.
>
> **(a) `input.result.output_path_mapping` does not exist.** `SingleResult`
> (`src/exec/run_result.rs:22-243`) has `task` (`:27`), `saved_output_path: Option<String>` (`:176`),
> `structured_output_path` (`:225`), `artifact_paths` (`:234`) — and **no `output_path_mapping` and
> no `output_reference`**. Upstream computes it at `:198-199` as
> `outputPathMappingFromTask(result.task, result.savedOutputPath ?? result.outputReference?.path)`.
> **`outputPathMappingFromTask` (`v0.67.0:src/runs/shared/single-output.ts:120-125`) and its helper
> `requestedOutputPathFromTask` are BOTH UNPORTED** — `grep -rn 'requested_output_path' crates/`
> returns nothing. The only in-tree construction of `WorkflowOutputPathMapping`
> (`src/workflows/types.rs:633-638`, `{ requested_path: String, saved_path: String }`) is a test
> fixture at `settlement.rs:1075`. **This requirement stands; the mechanism is §W-1.**
>
> **(b) `let mut next = next;` then never re-bound, and `let mut status` is never read after the
> backfill.** `backfill_live_child_identity` must *return* the mutated status and the driver must
> pass **that** value to `apply_detached_child_settlement` (upstream mutates `candidate` in place on
> the same `status` object at `:190`). Take `status` by value, return `RunStatus`.
>
> **(c) `existing` is used at the `children`/`summary` lines but never bound.** Upstream reads it at
> `:202-209`, **before** the receipt pass. Insert that read between the `find_workflow_settlement_step`
> block and the `classify_workflow_settlement` line, per §1's table row 4. Upstream re-throws any
> non-`ENOENT` error (`:208`); the cyrup equivalent is: absent → `None`; unparseable → **propagate**,
> not swallow.
>
> **(d) `receipt` is moved into the `PlanWorkflowSettlement` literal and then read again on the very
> next line** (`terminal_outcome: receipt.as_ref()…`). Compute `terminal_outcome` into a local
> **before** constructing the literal. Upstream reads `receipt?.terminalOutcome` at `:252`.
>
> **(e) `find_workflow_settlement_step(&next.steps, id, None, None)` is exact-`run_id`-only** —
> and that is CORRECT, not a bug. `settlement.rs:132-152` returns the `runId` position first, and
> only falls through to the by-key rung when **both** `workflow_key` **and** `session_file` are
> `Some` (`:145-147`), with a `candidates.len() == 1` third guard (`:154-158`). Upstream's second
> call at `:200` likewise passes neither. Keep `None, None` and say why in the doc comment.
>
> **(f) Error type.** `read_status_file` yields `SubagentError` (`control.rs:316`);
> `apply_detached_child_settlement` / `promote_settled_paused_workflow` / `with_workflow_children`
> all yield **`String`** (`settlement.rs:169`, `:277`, `:99`). Lift with
> `SubagentError::Management(msg)` (`src/error.rs:88`) — it is the crate's carrier for
> "a settlement invariant said no". Do not `unwrap`.

**Note the `terminal_outcome` argument is read from the receipt** (upstream `:252`; old cite `:247`),
not recomputed from the result — upstream deliberately lets the receipt's merged outcome win, because
`reconcileWorkflowReceipt` has already folded `entry.terminalOutcome` into it.

### The summary string (`mod.rs`) — upstream `:222-227`, spelled out

> ⟡ **AUG 2026-09-14 — the old body named `detached_settlement_summary(…)` with a six-argument
> signature and never said what it computes. It is a three-arm ternary plus a concatenated mapping
> clause. Ported literally:**
>
> ```text
> let recovery = workflow_recovery_actions(receipt.as_ref());         // `:222`
> let head = if resolution == Some(SettledAwaitingResume) {           // `:223`
>     format!("Workflow lanes settled after detached child {child} finished. JavaScript workflow \
>              continuation was not persisted.{}",
>         if recovery.is_empty() { " No retained child is resumable." }
>         else { " Use the listed keyed recovery action to continue a child." })
> } else if next.state == RunState::Complete {                        // `:225-226`
>     format!("Workflow completed after detached child {child} finished.")
> } else {                                                            // `:227`
>     next.error.clone().or(existing_summary).unwrap_or("Workflow failed.".into())
> };
> let summary = format!("{head}{}", workflow_output_path_mapping_summary(mappings));
> ```
>
> Two notes the implementor must not skip:
> - **`workflow_recovery_actions` is called TWICE** upstream — once here at `:222` for the summary's
>   branch, once inside `planWorkflowSettlement` (`settlement.rs:711`) for `plan.recovery`. That is
>   not redundant: the summary is an *input* to the plan, so it cannot read `plan.recovery`.
> - **`existing_summary` (upstream `existing?.summary`, `:227`) has no cyrup carrier** —
>   `ResultFile` (`records.rs:483-543`) has **no `summary` field**. See §W-7.
> - `workflow_output_path_mapping_summary`'s leading space and trailing period are upstream's and are
>   concatenated, per its own doc (`settlement.rs:487-492`). Do not trim either.

### `workflow_result_children` (`children.rs`)

Upstream has **two arms** (`:56-94`) and both are required:

- **Arm A — an existing result file was readable and its `results` is an array (`:56`):** map over
  the *existing* entries, rewriting only the one whose `runId` matches, and **clearing `detached`**
  on it (`detached: undefined`, `:66`; old cite `:64`). Every other entry is preserved byte-for-byte
  — this is what the upstream case *"preserves sibling output path mappings when rebuilding from
  workflow status"* protects. Non-object entries pass through untouched (`:58`; old cite `:59`).
- **Arm B — no existing result file (`:79`; old cite `:78`):** rebuild the whole array from
  `status.steps`, and back-fill `outputReference`/`terminalOutcome` **from the receipt entry for
  that step's `workflowKey`** when the step is not the settling child (`:88` and `:91`; the old
  cites `:86`/`:89` were off — **`:89` is the `outputPathMapping` fallback, a THIRD back-fill the
  old body did not name**: `step.runId === childRunId && outputPathMapping ? {outputPathMapping} :
  step.outputPathMapping ? {outputPathMapping: step.outputPathMapping} : {}`). Dropping these
  fallbacks loses sibling evidence whenever the paused result file was already collected.

In cyrup this is `Vec<SingleResult>`, not a JSON map. Arm A therefore reads the prior
`ResultFile` (`src/background/records.rs:483`, `results: Vec<SingleResult>` at `:528`) and mutates
the matching `SingleResult`; Arm B constructs from `status.steps`.
The `success` predicate is `exit_code == Some(0) && error.is_none() && !interrupted` (`:63`; old
cite `:62`) — it does **not** consult `stopped`, matching `apply_detached_child_settlement`'s own
documented quirk (`settlement.rs:266-270`).

> ⟡ **AUG 2026-09-14 — Arm A's "match by `runId`" and Arm B's field map, against real types.**
> - The join key upstream is `child.runId`. On `SingleResult` that is
>   **`child_run_id: Option<RunId>`** (`run_result.rs:202`), not `run_id`. Match
>   `child.child_run_id.as_ref().map(RunId::as_str) == Some(input.child_run_id)`.
> - `output` upstream is `getSingleResultOutput(result)` (`:51`). **Unported.** The crate idiom is
>   `result.final_output.as_deref().unwrap_or("")` (`src/background/watch/message.rs:218`). Use it.
> - `outputState` (`:65`, `:86`) is `output.trim() ? "present" : "absent"` →
>   `SubagentOutputState::{Present, Absent}` (`SingleResult::output_state`, `run_result.rs:211`).
> - `detached: undefined` (`:66`) → `SingleResult::detached = false` (`run_result.rs:62`); the field
>   is a plain `bool`, so "clear" means `false`, not "remove".
> - Arm B's `workflowKey`/`agent`/`sessionName`/`runId` come off `StepStatus`
>   (`records.rs:108`/`:26`/`:123`/`:115`) and `step.status === "completed" || "complete"` (`:84`)
>   collapses to `step.status == StepState::Complete` — cyrup has ONE spelling.
> - Arm B's `outputReference` has **no `SingleResult` field**; the receipt's carrier is
>   `WorkflowReceiptEntry::output_reference: Option<String>` (`receipt.rs:245`). See §W-7 for where
>   it can land on a `SingleResult` (`saved_output_path`, `run_result.rs:176`) and the caveat.
> - Receipt lookup is `receipt.entry(&key)` (`receipt.rs:365-368`), **not** index syntax —
>   `WorkflowReceipt::entries` is a `Vec<WorkflowReceiptEntry>` in insertion order (`receipt.rs:331`),
>   serialized as a JSON object keyed by `entry.key`.

`usage_with_value` (`:44-48`; old cite `:41-45`) is the guard that suppresses an all-zero usage
block; port it verbatim — an all-zero `usage` on a reconciled child is upstream-invisible and must
stay so.

> ⟡ **AUG 2026-09-14** — upstream tests six fields (`input|output|cacheRead|cacheWrite|cost|turns`).
> cyrup's carrier is `cyrup_core::Usage` on `SingleResult::usage` (`run_result.rs:29`), which is a
> **non-`Option` value type**. So `usageWithValue` becomes "is this `Usage` all-default", and the
> `undefined` result becomes "do not overwrite the existing entry's usage" — the observable
> behaviour is identical, the representation is not. Implement it as
> `fn usage_with_value(usage: &Usage) -> Option<&Usage>` returning `None` for `Usage::default()`.

### `reconcile_workflow_receipt` (`receipt.rs`)

Upstream `:97-159` (old cite `:98-160`). Required behaviours, in order:

1. **Absent receipt file → `None`, not an error** (`:100`; old cite `:101`). A workflow that never
   wrote a receipt reconciles fine.
   > ⟡ **AUG 2026-09-14 — see §W-3. `read_workflow_receipt` returns `Result`, never `Option`**, and
   > for a live paused run the absent-file arm is **`MayStillBeActive`**, not `NotFound`.
2. **The child must be identifiable by stable key** — find the step by `runId`, take its
   `workflowKey`; **throw if there is none** (`:104`; old cite `:105`) and **throw if the receipt
   has no such entry** (`:106`; old cite `:107`). These are the two faults the driver converts into
   `receipt_persistence_error`, and they are what the upstream *"publishes detached completion when
   the workflow receipt is malformed"* case exercises. Do not soften either to a silent skip.
   > ⟡ **AUG 2026-09-14** — the two messages are matched by upstream tests and must be ported
   > verbatim: `Workflow receipt '<runId>' cannot identify detached child '<childRunId>' by stable
   > key.` and `Workflow receipt '<runId>' has no detached child key '<key>'.`
3. **Resumability** (`:107-113`; old cite `:109-115`): upstream calls
   `resolveAsyncResumeTarget(..., { requireSessionFile: true, sessionId: status.sessionId })`
   and maps `kind === "revive"` → `Resumable`, anything else → `NotResumable { reason: "child is still running" }`,
   and any throw → `NotResumable { reason: <error text> }`.
   **cyrup has no `resolve_async_resume_target`.** The nearest equivalent is
   `src/background/control.rs:2225`'s `resume()`, which is a *performing* call, not a *probing*
   one. **Do not call it.** Derive the verdict from the child's own terminal record instead, which
   is what `requireSessionFile: true` is actually asking:
   `Resumable { latest_run_id }` iff the child's status is terminal **and** carries a
   `session_file`; otherwise `NotResumable { reason: "child is still running" }`. Use
   `WorkflowReceiptResume` (`src/workflows/receipt.rs:164`),
   whose `Resumable` arm structurally requires the `latest_run_id` upstream sets at `:127`.
   > ⟡ **AUG 2026-09-14 — exact shape.** `WorkflowReceiptResume::Resumable { latest_run_id: Bounded<256> }`
   > / `::NotResumable { latest_run_id: Option<Bounded<256>>, reason: String }` (`receipt.rs:164-178`).
   > There is **no separate `resumability` field** on `WorkflowReceiptEntry` — `resume:
   > WorkflowReceiptResume` (`receipt.rs:257`) is both halves as one value, which is exactly why
   > upstream's `latestRunId: entry.latestRunId ?? childRunId` (`:127`) must be written as
   > `entry.resume.latest_run_id().cloned().unwrap_or_else(|| Bounded::…(child_run_id))` — the
   > accessor is `latest_run_id()` at `receipt.rs:181-187`. `reason` must never be blank
   > (`receipt.rs:176`). `is_resumable()` (`receipt.rs:191-193`) is the ONE predicate
   > `workflow_recovery_actions` filters on (`settlement.rs:424`).
4. **External-CLI override is last and absolute** (`:121`; old cite `:122`): if an external adapter
   is present, `resumability` becomes `NotResumable { reason: externalAdapter.nonResumableReason }`
   **regardless** of step 3. Read the child's own status for the single-step external runner
   (`:115-116`; old cite `:119-120`) and normalise through
   `normalize_external_cli_runner_status` (`src/runner/status.rs:316`) /
   `external_cli_receipt_metadata` (`src/runner/status.rs:402`). This is what keeps mixed
   Pi/external entries on their own computed resumability rather than a blanket verdict.
   > ⟡ **AUG 2026-09-14 — signatures verified, and they are NOT struct-argument style:**
   > `normalize_external_cli_runner_status(value: &serde_json::Value) -> Option<ExternalCliRunnerStatus>`
   > (`runner/status.rs:316`) — it takes RAW JSON and returns `None` unless `type == "external-cli"`
   > with a non-blank `command`; and
   > `external_cli_receipt_metadata(runner: &ExternalCliRunnerStatus, external_process: Option<&ExternalProcessStatus>, output_reference: Option<&str>) -> serde_json::Value`
   > (`runner/status.rs:402`) — **three positional args**, returning an opaque `Value`, which is
   > exactly the type `WorkflowReceiptEntry::external_adapter: Option<serde_json::Value>`
   > (`receipt.rs:253`) holds. `nonResumableReason` must therefore be read back **out of that
   > `Value`** (`metadata["nonResumableReason"].as_str()`), not off a typed field.
   > `SingleResult` carries `runner` and `external_process` (both present — see the field list
   > `finish.rs:314-316` constructs). The child's own `status.json` is read via `read_status_file`
   > against the CHILD's `RunPaths`, which the caller must therefore be able to derive: see §Y-2.
5. **`terminalOutcome` merges, it does not replace** (`:120`; old cite `:123`):
   `workflow_terminal_outcome_for_result(result).or(entry.terminal_outcome)`. A detached timeout
   outcome stays local to its entry and must not be promoted onto the workflow.
   > ⟡ **AUG 2026-09-14 — §W-4: the signature is
   > `workflow_terminal_outcome_for_result(signals: WorkflowBudgetSignals) -> Option<WorkflowTerminalOutcome>`
   > (`settlement.rs:470-472`), NOT `(&SingleResult)`.** Call it as
   > `workflow_terminal_outcome_for_result(WorkflowBudgetSignals::from_single_result(result))`
   > (`settlement.rs:461-468`). Note `tool_budget_blocked` is hard-`false` there because
   > `SingleResult` has no carrier for it (documented at `settlement.rs:444-457`) — that is a
   > pre-existing, deliberate gap, **not** something to fix in this task.
6. The receipt's top-level `state` maps from the **settled** status (`:144`; old cite `:146`),
   `workflowChildren` is overwritten from it (`:149`; old cite `:151`), and `recovery` is set
   **iff** a `resolution` was passed — else both `workflowResolution` and `recovery` are removed
   (`:153-157`; old cite `:154-157`).
   > ⟡ **AUG 2026-09-14** — the `status.state → WorkflowReceiptState` map at `:144` is already
   > ported as the private `workflow_receipt_state` (`settlement.rs:74`), and
   > `plan_workflow_settlement` **already applies it** to `plan.receipt` at `settlement.rs:693-704`
   > (`next_receipt.state = workflow_receipt_state(status.state)`, `workflow_children`,
   > `workflow_resolution`, `recovery`). Steps 6's three writes therefore happen **twice** — once
   > here on the receipt that is WRITTEN TO DISK, once again inside the composer on the copy that
   > reaches `plan.receipt`. Upstream has the same double-application (`:144-157` then
   > `workflow-settlement.ts:216-221`). **Port both.** Do not "optimize" the first away: the
   > on-disk receipt is written at `:158` BEFORE the plan exists, and `plan_workflow_settlement`
   > discards its `receipt` entirely when `receipt_persistence_error` is set (`settlement.rs:688`).
   > `workflow_receipt_state` is private to `settlement.rs`; re-derive the four-arm match locally
   > (`complete→Complete, stopped→Stopped, paused→Paused, else→Failed`) rather than widening it.
   > Write through `write_workflow_receipt(run_dir: &Path, receipt: &WorkflowReceipt) -> io::Result<PathBuf>`
   > (`receipt.rs:604`), which takes the **already-resolved run directory** (`run_paths.run_dir`),
   > while `read_workflow_receipt(async_root: &Path, run: &RunDirName)` (`receipt.rs:657`) takes the
   > **async ROOT** plus a `RunDirName`. That asymmetry is upstream's (`:98` `path.dirname(asyncDir)`
   > vs `:158` `asyncDir`) and is deliberate — do not "fix" it. `RunDirName::for_run(&run_id)` is
   > infallible (`src/identity/run_dir_name.rs:58`); the async root is
   > `run_paths.run_dir.parent()`.

---

## SUBTASK2 (replaces "adopt only this session's detached workflows") — identity, not adoption

There are exactly three `sessionId` references in the upstream file. None is an ownership gate:

| ref | upstream (@v0.67.0) | what it actually does |
|---|---|---|
| 1 | `resolveAsyncResumeTarget(…, { sessionId: status.sessionId })` (`:109`; old cite `:111`) | scopes the **resumability probe** to the workflow's own session |
| 2 | `sessionId: next.sessionId ?? existing?.sessionId` (`:244`; old cite `:243`) | **propagates** the session onto the published result, preferring the live status and falling back to the prior result file |
| 3 | `sessionId: plan.status.sessionId` (`:296`; old cite `:293`) | **propagates** the session onto the completion event |

**The requirement is propagation fidelity, not a new predicate.** The draft's instinct — *"do not
introduce a fourth predicate"* — was right; its mechanism was wrong. Concretely:

- The published `ResultFile` **must** carry `session_id` and `completion_owner_id`
  (`src/background/records.rs:518` and `:525` — **verified exact**).
  These are already documented there as the partition key for `result-index/sessions/` and the
  second half of `owns_completion`. The **existing** `SessionGate`
  (`src/background/delivery/gate.rs:30` for the enum, `:51` for `admits(self, current, record)`,
  `Strict`/`Permissive` — **verified exact**) then does the partitioning for free.
  A reconciled result that drops `session_id` is undeliverable by construction —
  `write_async_result_file` (`src/background/result_index/write.rs:223`, **verified exact**)
  documents a missing session as a **hard error**, so this fails loudly rather than silently
  stranding the workflow.
  > ⟡ **AUG 2026-09-14** — precisely: `write_async_result_file(request: &ResultWrite<'_>, payload:
  > &(impl Serialize + Sync)) -> io::Result<PayloadState>` takes a **non-optional** `session_id:
  > &SessionId` on `ResultWrite`. The refusal therefore happens one level up, at whatever builds the
  > `ResultWrite` — mirror `finish.rs:477-500`'s `write_result_file`, whose `let Some(session_id) =
  > status.session_id.as_ref() else { return Err(SubagentError::Management(…)) }` (`finish.rs:478-484`)
  > is the exact refusal to reproduce. Also reproduce its `PayloadState::Staged` warn-not-fail arm
  > (`finish.rs:493-499`) — a staged payload is written and will be promoted on read, not a failure.
- Note ref 2's fallback order (`next.sessionId` **then** `existing.sessionId`): this is precisely
  the upstream case *"preserves quiet schedule attribution when the paused result file is already
  gone"*. Reversing the order, or dropping the fallback, loses attribution for a workflow whose
  paused result file was collected before the child exited.
- **Do not add a `SessionGate` call in this module.** The gate belongs to delivery; adding a second
  enforcement point here is the "fourth predicate" the draft correctly warned against.

### The live-identity backfill (`identity.rs`) — the real correctness subtlety

`:182-191` (old cite `:177-190`) is the part that is easy to get wrong. Before settling, upstream
may write the child's `runId` onto a step that has none — but **only** under a triple guard:

1. exactly **one** `foregroundControl` matches this `parentWorkflowRunId` + `workflowKey`, and its
   `runId` is the settling child (`:182-185`); **and**
2. exactly **one** status step carries that `workflowKey` (`:187-188`); **and**
3. that step has `runId === undefined` **and** `sessionFile === undefined` **and**
   `status === "paused"` **and** `activityState === "needs_attention"` (`:189-190`).

All three exist to refuse a claim the caller cannot actually justify. Two same-key live attempts
must resolve to **nothing** — never "the first one". Equally, a completion that matches only on
`workflowKey`, with no confirmed live control, must be treated as **stale** and ignored. Port the
guard whole; relaxing any clause admits cross-lane corruption that no later stage can detect.

> ⟡ **AUG 2026-09-14 — a ZEROTH clause the old body omitted, and the real predicate spellings.**
> - **Clause 0 (upstream `:182`): `input.workflowKey === undefined` ⇒ `matchingControls = []`,
>   so `confirmedLiveIdentity` is `false` and the whole backfill is skipped.** A completion with no
>   workflow key can never backfill. Port it; it is the outermost guard.
> - Clause 3 in cyrup types: `step.run_id.is_none()` (`records.rs:115`) `&& step.session_file.is_none()`
>   (`records.rs:31`) `&& step.status == StepState::Paused` (`records.rs:28`) `&&
>   step.telemetry.activity_state == Some(ActivityState::NeedsAttention)` (`records.rs:142`,
>   `StepTelemetry::activity_state`).
> - The write is `step.run_id = Some(RunId::from_token(child_run_id.to_string()))` — the same
>   constructor `apply_detached_child_settlement` uses at `settlement.rs:313`.

Source for the matching controls is `ForegroundControlEntry` (`src/extension/executor/notices.rs:20`):
`parent_workflow_run_id` (`notices.rs:73`), `workflow_key` (`notices.rs:78`).

> ⟡ **AUG 2026-09-14 — §W-5, the one that will silently break: `ForegroundControlEntry` has NO
> `run_id` field.** Its 19 fields (`notices.rs:20-98`) are `interrupt, current_agent, current_index,
> current_activity_state, mode, description, current_tool, current_path, turn_count, tool_count,
> tokens, started_at, updated_at, session_id, parent_workflow_run_id, workflow_key, cwd,
> session_name, active_children`. The run id is the **MAP KEY**: the registry is
> `foreground_controls: Arc<std::sync::Mutex<HashMap<String, ForegroundControlEntry>>>`
> (`src/extension/executor/mod.rs:146`), documented at `:138-145` as `targetRunId -> {…}`. So
> upstream's `matchingControls[0]?.runId === input.childRunId` (`:185`) becomes a comparison against
> the **key**, and the filter must iterate `(key, entry)` pairs, not values.
>
> Two further mechanics:
> - The accessor `SubagentExecutor::foreground_controls()` (`mod.rs:528-534`) is
>   **`#[cfg(test)]`-gated**. The *field* is private-to-module, and `workflow_detach` is a descendant
>   of `extension::executor`, so a descendant module may read `executor.foreground_controls` **by
>   field** in production. That works, but it couples the "disk-driven unit" of §1 to a live
>   executor handle — which contradicts §1's own decision. **See §Y-1: this is an open question the
>   implementor must not silently decide.**
> - The lock is a `std::sync::Mutex`, documented at `mod.rs:152-159` as "every access is a short
>   synchronous read". **Never hold it across an `.await`.** Collect the matching run ids into a
>   `Vec<RunId>` inside a block, drop the guard, then proceed.

---

## SUBTASK3 (replaces the `workflow_controllers` claim) — wake the deferred seam

WORKFLOW_3 left this task an explicit, documented seam. From
`src/background/runner_main/finish.rs:190-203`:

> *"Lives beside `finish_run` rather than inside it… A future async workflow arm calls this and
> then `finish_run` — a call, not a rewrite of either."*
> *"`#[allow(dead_code)]` outside `#[cfg(test)]`: this task's own unit test below is this
> function's only caller until a future async workflow arm becomes the first production one."*

**SCOPE_8 is that arm.** Required:

1. Use `apply_workflow_settlement_plan(&plan, &mut status) -> WorkflowResultFields`
   (`src/background/runner_main/finish.rs:204`, **verified exact**)
   to stamp the plan. Do **not** open-code the assembly — upstream open-codes it twice and that
   duplication is exactly what this helper was extracted to prevent.
   > ⟡ **AUG 2026-09-14 — exact signature and effect, verified at `finish.rs:204-219`:**
   > `pub(crate) fn apply_workflow_settlement_plan(plan: &WorkflowSettlementPlan, status: &mut RunStatus) -> WorkflowResultFields`.
   > It does `*status = plan.status.clone()` and returns
   > `WorkflowResultFields { workflow_children: plan.status.workflow_children.clone(),
   > workflow_receipt: (plan.receipt, plan.receipt_path) both-Some → Some(WorkflowReceiptRef{path, receipt}) }`
   > (`finish.rs:181-188` for the struct). **`WorkflowResultFields` does NOT carry the `dead_code`
   > attribute** — only `apply_workflow_settlement_plan` does (`finish.rs:203`), so point 2's "same
   > for `WorkflowResultFields`" is a no-op; check and leave it alone.
2. **Remove the now-false `#[cfg_attr(not(test), allow(dead_code))]`** from
   `apply_workflow_settlement_plan` (`finish.rs:203`) and update its doc comment: the "future async
   workflow arm" has arrived and should be named. Same for `WorkflowResultFields` if it carries the
   same attribute.
   > ⟡ **AUG 2026-09-14 — STILL REQUIRED, and still true: the attribute is at `finish.rs:203`,
   > verbatim `#[cfg_attr(not(test), allow(dead_code))]`.** Its doc paragraph at `finish.rs:200-202`
   > and the "Lives beside `finish_run`… none of which is a workflow today" paragraph at `:194-198`
   > both become false and must be rewritten to name SCOPE_8. So must `WorkflowResultFields`'
   > `[Default] (both None) for every one of finish_run's six current callers` doc (`finish.rs:178-181`).
3. **Visibility:** `finish_run` is `pub(super)` (`finish.rs:226`) — reachable only inside
   `background::runner_main`. `apply_workflow_settlement_plan` is already `pub(crate)` (`finish.rs:204`).
   This module lives in `extension::executor`, so it can call the latter but **not** the former.
   Widen `finish_run` to `pub(crate)` and re-export it from
   `src/background/runner_main/mod.rs`
   (which currently exports only `config`/`entry`/`events`/`executor` items). Widening one
   function's visibility is the correct move; duplicating its write-ordering logic is not —
   `finish_run` is R-SA-077's write-ordering funnel and that ordering is the guarantee.

> ⟡⟡ **AUG 2026-09-14 — POINT 3 CANNOT WORK AS WRITTEN. Two independent blockers, plus one that
> makes the first half of the sentence false. The REQUIREMENT it serves — "do not duplicate the
> write-ordering logic" — stands, and §W-2 is the mechanism that honours it.**
>
> **(i) `apply_workflow_settlement_plan` is NOT reachable from `extension::executor` today, contrary
> to this point's claim.** `src/background/runner_main/mod.rs:81` declares `mod finish;` — **private**.
> Its re-export block (`mod.rs:86-89`) is exactly:
> ```rust
> pub use config::{ConfigConsumeOutcome, RunnerConfig, read_and_delete_config};
> pub use entry::{RunnerOverrides, run, run_with};
> pub use events::{ASYNC_EVENTS_MAX_BYTES_ENV, resolve_async_events_cap_bytes};
> pub(crate) use executor::ExecSingleStepExecutor;
> ```
> `finish` appears nowhere. A `pub(crate)` item inside a private module is unreachable from outside
> that module tree. **Required, and NEW: add
> `pub(crate) use finish::{WorkflowResultFields, apply_workflow_settlement_plan};` to
> `runner_main/mod.rs`.** (`background/mod.rs:48` already has `pub mod runner_main;`, so that is the
> only hop needed.)
>
> **(ii) Calling `finish_run` would write NOTHING in this task's own scenario.** `finish_run`'s first
> statement is `if terminal_result_exists(run_paths, &status).await { warn!(…); return; }`
> (`finish.rs:237-245`), and `terminal_result_exists` (`finish.rs:161-172`) resolves the run's
> published payload via `run_paths.resolve_result(session_id, &status.run_id)`. A **paused** run DOES
> publish a terminal `ResultFile`: `settle_loop_outcome`'s `LoopOutcome::Interrupted` arm returns
> `(RunState::Paused, results, None)` (`finish.rs:49-56`) straight into `finish_run`, and
> `finish_runs_synthesized_child_carries_stopped_only_for_a_stopped_run` (`finish.rs:1023-1060`)
> asserts the `(RunState::Paused, false, true)` row reads its result back off disk. **This task's
> entire job is a second, superseding write over an already-published paused result — precisely the
> case that guard refuses.** Widening `finish_run` and calling it produces a silent no-op plus a
> `warn!`.
>
> **(iii) Even without (ii), `finish_run` builds the wrong `ResultFile`.** It synthesizes `agent`
> from `status.steps` (`finish.rs:390-396`), derives `success` as
> `terminal_state == Complete && results.iter().all(|r| r.exit_code == 0)` (`finish.rs:338`), and
> injects a placeholder `SingleResult` when `results` is empty (`finish.rs:332-330`). Upstream's
> reconciler publishes `agent: "workflow"`, `mode: "workflow"` (`:237-238`), `success:
> plan.status.state === "complete"` (via `plan.success`, `settlement.rs:707`) and the **rebuilt**
> `workflow_result_children` array. Those are different payloads.
>
> **Therefore: do NOT widen `finish_run`, do NOT re-export it, do NOT call it.** Points 1 and 2 are
> unchanged and mandatory. Point 3's visibility work becomes blocker (i) only. See §W-2 for the
> write path that replaces it.

### The write path (`:255-301`), in order — the ordering is the guarantee

> ⟡ **AUG 2026-09-14 — the old body's line numbers for this block were ~20 lines high (it cited
> `:275-301`). The v0.67.0 numbers are below and are authoritative.**

```
write status.json (atomic)            // `:255`  — via RunPaths::status   [old cite `:275`]
update the run index                  // `:256`  — see the gap note below [old cite `:276`]
(job mirror: job.status/updatedAt/…)  // `:257-263` — NO cyrup analogue, correctly dropped (§1)
write the result file                 // `:264`  — write_async_result_file (stage → index → promote) [old `:281`]
append receipt_write_failed event     // `:265-273` — only when receipt_error.is_some()  [old `:282-289`]
append subagent.workflow.completed    // `:274-280` — only when plan.completion_event.is_some() [old `:291-292`]
emit the async-complete event         // `:281-301` — see §W-8: cyrup emits this by WRITING the result file [old `:293-301`]
```

> ⟡ **AUG 2026-09-14 — concrete calls for each rung, all verified:**
> 1. `crate::background::atomic::write_atomic_json(&run_paths.status, &plan.status).await` — the
>    exact call `settle_foreground_workflow` makes (`src/extension/tool/routing.rs:1074-1076`) and
>    the one `finish_run` makes (`finish.rs:349`).
> 2. `crate::background::terminal_run_index::update_terminal_run_index(&run_paths.run_dir, &plan.status).await`
>    (`src/background/terminal_run_index/update.rs:25`). **It self-gates** on `is_indexed_state`
>    (`terminal_run_index/entry.rs:103-105`: everything except `Queued`/`Running` — **`Paused` is
>    INCLUDED**) and on `status.session_id`, so call it unconditionally with no branch of your own.
>    Best-effort: `finish.rs:352-366` logs and continues on `Err`; do the same.
> 3. `result_index::write_async_result_file(&ResultWrite{ results_dir, session_id, run_id, written_at,
>    async_dir: Some(&run_paths.run_dir), tool_call_id: None }, &result_file).await`
>    (`result_index/write.rs:223`), mirroring `finish.rs:485-500`.
> 4/5. `BoundedJsonlWriter::create(&run_paths.events).await?` then one `write_line` per event object.
>    Open the writer **once** and reuse it for both events.
> 6. **Nothing.** See §W-8.

**Gap note — `updateActiveRunIndex` is unported.** cyrup has only
`terminal_run_index` (`src/background/terminal_run_index/entry.rs`: session-partitioned terminal
markers — `index_root` `:108`, `session_index_dir` `:117`, `marker_path` `:143`; **all three line
numbers verified exact**). Since every state this reconciler publishes is terminal or `paused`,
route terminal settlements through the **terminal** index and skip the active-index update. Do not
invent an active-run index here; **that is SCOPE_9's surface and this task must not touch it.**

**Event emission must be gated on `plan.completion_event`** (`:274`; old cite `:290`). When another
detached child is still open, `promote_settled_paused_workflow` leaves the workflow `paused`, the
plan produces **no** completion event, and this reconciler must write status + result and then stop.
Emitting a completion for a still-paused workflow is the worst available failure: it publishes a
terminal verdict the workflow has not reached.

> ⟡ **AUG 2026-09-14 — `plan.completion_event` is `terminal.then(…)` where `terminal =
> status.state.is_terminal()` (`settlement.rs:715-728`), and `RunState::is_terminal()` is
> `Complete | Failed | Stopped` — `Paused` is excluded.** So the gate works exactly as described,
> and `promote_settled_paused_workflow` returning `None` (`settlement.rs:173-183`: still-open step
> ⇒ `None`) leaves `apply_detached_child_settlement` yielding the un-promoted `next` (`settlement.rs:373`),
> which is still `Paused`. **The chain is verified end to end.**

`event_metadata` must carry `reconciledFromDetachedChild: <childRunId>` (`:253`; old cite `:249`),
and `WorkflowCompletionEvent::extra` (`src/workflows/settlement.rs:537` for the struct, `:582-584`
for the merge-last serializer) already merges caller keys **last** for exactly this purpose.

---

## SUBTASK4 — the `promote_paused_workflow_if_settled` call site

Upstream exports a second, thinner entry point used by the workflow's **own** settlement path
(`subagent-executor.ts:5763`): after a workflow fails with `errorKind: "detached-child"` and is
therefore parked at `paused`, it immediately asks whether the workflow is *already* settled (every
detached child having exited before the engine unwound) and promotes it if so.

`WorkflowScriptErrorKind::DetachedChild` already exists in cyrup
(`src/workflows/scripted/types.rs:250` for the variant, `:249-251` with its `#[serde(rename =
"detached-child")]` — **verified**). Expose the thin wrapper from this module —

```rust
/// pi `promotePausedWorkflowIfSettled` (`workflow-detach-reconcile.ts:40-42` @v0.67.0) — the
/// re-export the workflow's own settlement path calls, distinct from the driver's internal use.
pub(crate) fn promote_paused_workflow_if_settled(
    status: &RunStatus,
    trace: &[WorkflowScriptTraceEntry],
) -> Result<Option<SettledWorkflowStatus>, String> {
    promote_settled_paused_workflow(status, trace, crate::time::now_epoch_millis())
}
```

— and likewise `apply_detached_child_to_paused_workflow` for `applyDetachedChildToPausedWorkflow`
(`:33-38`; old cite `:32-37`). Both are deliberate upstream indirection: the settlement module owns
the logic, this module owns the *entry point the detach lifecycle calls*. Keep the indirection; it
is the seam that lets the call sites move without touching `workflows/`.

> ⟡ **AUG 2026-09-14 — the sketch compiles as written.** `promote_settled_paused_workflow(status:
> &RunStatus, trace: &[WorkflowScriptTraceEntry], now: i64) -> Result<Option<SettledWorkflowStatus>, String>`
> (`settlement.rs:165-169`); `SettledWorkflowStatus` is `pub type … = RunStatus` (`settlement.rs:53`).
> The sibling wrapper is
> `apply_detached_child_to_paused_workflow(status: &RunStatus, input: DetachedChildSettlement<'_>,
> trace: &[WorkflowScriptTraceEntry]) -> Result<Option<SettledWorkflowStatus>, String>` delegating to
> `apply_detached_child_settlement` (`settlement.rs:273-277`); `DetachedChildSettlement<'a>` is
> `settlement.rs:203-227` with `{child_run_id: &'a str, exit_code: Option<i32>, error: Option<&'a str>,
> interrupted: bool, session_file: Option<&'a Path>, session_name: Option<&'a str>, stopped: bool,
> workflow_key: Option<&'a WorkflowKey>, now: i64}`. **Note `exit_code: Option<i32>` on the input but
> `exit_code: i32` (non-optional) on `SingleResult` (`run_result.rs:28`)** — the driver must wrap it
> as `Some(result.exit_code)`.
> **Both wrappers will be dead on arrival** (no in-tree caller exists for the
> `errorKind: "detached-child"` path yet — `WorkflowScriptErrorKind::DetachedChild` has no producer
> that parks a run at `Paused`). They are still required by this subtask; gate them with the same
> `#[cfg_attr(not(test), allow(dead_code))]` discipline `finish.rs:203` used, and say in the doc
> WHICH future task becomes the caller. See §Y-4.

---

## §U — Upstream line map, re-pinned to `v0.67.0` (2026-09-14)

Read with `git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:src/runs/foreground/workflow-detach-reconcile.ts`.
Where the body above disagrees, **this table wins.**

| what | v0.67.0 | old (unpinned HEAD `57278d82`) |
|---|---|---|
| `applyDetachedChildToPausedWorkflow` | `:33-38` | `:32-37` |
| `promotePausedWorkflowIfSettled` | `:40-42` | `:39-41` |
| `usageWithValue` | `:44-48` | `:41-45` |
| `workflowResultChildren` | `:50-95` | `:47-96` |
| — Arm A (existing array) | `:56-77` | `:56-96` |
| — non-object passthrough | `:58` | `:59` |
| — `success` predicate | `:63` | `:62` |
| — `detached: undefined` | `:66` | `:64` |
| — Arm B (rebuild from steps) | `:79-94` | `:78` |
| — receipt `outputReference` back-fill | `:88` | `:86` |
| — receipt `outputPathMapping` back-fill | `:89` | *(unnamed)* |
| — receipt `terminalOutcome` back-fill | `:91` | `:89` |
| `reconcileWorkflowReceipt` | `:97-159` | `:98-160` |
| — absent receipt ⇒ `undefined` | `:100` | `:101` |
| — throw: no stable key | `:104` | `:105` |
| — throw: no receipt entry | `:106` | `:107` |
| — resumability probe | `:107-113` | `:109-115` |
| — `sessionId` on the probe | `:109` | `:111` |
| — external child status read | `:115-116` | `:119-120` |
| — `terminalOutcome` merge | `:120` | `:123` |
| — external-adapter override | `:121` | `:122` |
| — `latestRunId ?? childRunId` | `:127` | `:127` ✓ |
| — receipt `state` from settled status | `:144` | `:146` |
| — `workflowChildren` overwrite | `:149` | `:151` |
| — resolution/recovery set-or-delete | `:153-157` | `:154-157` |
| — write receipt | `:158` | *(unnamed)* |
| `appendDetachedWorkflowEvent` | `:161-168` | *(unnamed)* |
| `reconcileDetachedWorkflowChildCompletion` | `:170-304` | `:170-302` |
| — no status ⇒ `false` | `:181` | `:173` |
| — matching controls filter | `:182-184` | `:183-186` |
| — `confirmedLiveIdentity` | `:185` | `:185` ✓ |
| — single-step-by-key | `:187-188` | `:188` |
| — triple guard + write | `:189-190` | `:189-190` ✓ |
| — no settlement ⇒ `false` | `:197` | `:196` |
| — `outputPathMapping` onto settled step | `:198-201` | `:197-200` |
| — read existing result file | `:202-209` | *(unnamed)* |
| — `classifyWorkflowSettlement` | `:213` | `:210` |
| — receipt try/catch | `:214-220` | `:211-217` |
| — `workflowResultChildren` call | `:221` | *(unnamed)* |
| — `workflowRecoveryActions` (for summary) | `:222` | *(unnamed)* |
| — summary ternary | `:223-227` | *(unnamed)* |
| — `planWorkflowSettlement` call | `:228-254` | `:228` |
| — `sessionId: next ?? existing` | `:244` | `:243` |
| — `terminalOutcome: receipt?.terminalOutcome` | `:252` | `:247` |
| — `eventMetadata` | `:253` | `:249` |
| — write status.json | `:255` | `:275` |
| — `updateActiveRunIndex` | `:256` | `:276` |
| — job mirror | `:257-263` | *(unnamed)* |
| — `writeAsyncResultFile` | `:264` | `:281` |
| — `receipt_write_failed` event | `:265-273` | `:282-289` |
| — completion-event gate | `:274` | `:290` |
| — `subagent.workflow.completed` append | `:274-280` | `:291-292` |
| — bus emit | `:281-301` | `:293-301` |
| — `sessionId` on the event | `:296` | `:293` |

---

## §V — Verified cyrup seam inventory (2026-09-14, every line opened)

| seam | file:line | current shape |
|---|---|---|
| module declaration slot | `src/extension/executor/mod.rs:28-29` | `pub(crate) mod workflow_controllers;` / `pub(crate) mod workflow_steering;` |
| live control registry | `src/extension/executor/mod.rs:146` | `foreground_controls: Arc<std::sync::Mutex<HashMap<String, ForegroundControlEntry>>>` — **key is the run id** |
| control-registry accessor | `src/extension/executor/mod.rs:528-534` | `#[cfg(test)] pub(crate) fn foreground_controls(&self) -> &Mutex<HashMap<String, ForegroundControlEntry>>` |
| control entry | `src/extension/executor/notices.rs:20` | fields at `:68` `session_id: Option<SessionId>`, `:73` `parent_workflow_run_id: Option<RunId>`, `:78` `workflow_key: Option<WorkflowKey>`; **no `run_id`** |
| status reader | `src/background/control.rs:316` | `pub(crate) async fn read_status_file(&Path) -> Result<Option<RunStatus>, SubagentError>` |
| result reader | `src/background/control.rs:326` | `async fn read_result_file(&Path) -> Result<Option<ResultFile>, SubagentError>` — **PRIVATE** |
| run paths | `src/background/run_paths.rs:85` | `RunPaths { run_dir, status, events, control_inbox, append_dir, runner_stdout_log, runner_stderr_log, run_log_md, results_dir, legacy_result_root }`; ctor `for_run` `:129`; `resolve_result` `:155-168` |
| `events.jsonl` discipline | `src/background/run_paths.rs:88-94` | MUST go through `BoundedJsonlWriter` |
| bounded jsonl | `src/jsonl.rs:86` / `:97` / `:127` | `create` / `create_with_cap` / `write_line` (silent no-op at cap) |
| atomic status write | `src/background/atomic.rs` via `write_atomic_json` | used at `routing.rs:1074`, `finish.rs:349` |
| terminal index | `src/background/terminal_run_index/update.rs:25` | `update_terminal_run_index(&Path /*run dir*/, &RunStatus) -> io::Result<()>`; self-gated `entry.rs:103-105` |
| result write | `src/background/result_index/write.rs:223` | `write_async_result_file(&ResultWrite<'_>, &impl Serialize+Sync) -> io::Result<PayloadState>` |
| result write precedent | `src/background/runner_main/finish.rs:477-500` | `write_result_file` — session refusal + `Staged` warn |
| `ResultFile` | `src/background/records.rs:483-543` | `id, run_id, agent, mode, state, success, cwd: PathBuf, session_file, session_id, completion_owner_id, results: Vec<SingleResult>, workflow_children, workflow_receipt`. **No `summary`, no `tool_call_id`, no `async_dir`, no `schedule_origin`, no `workflow`.** |
| `RunStatus` | `src/background/records.rs:210-352` | `cwd: Option<PathBuf>` `:262`; `session_id` `:230`; `completion_owner_id` `:239`; `tool_call_id` `:332`; `workflow_children` `:339`; `workflow_receipt_path` `:346`. **No trace / no `workflow` script state.** |
| `StepStatus` | `src/background/records.rs:24-180` | `agent :26`, `status :28`, `session_file :31`, `error :65`, `stopped :94`, `workflow_key :108`, `run_id :115`, `session_name :123`, `interrupted :133`, `output_path_mapping :137`, `telemetry :142` |
| `SingleResult` | `src/exec/run_result.rs:22-243` | `task :27`, `exit_code: i32 :28`, `usage :29`, `final_output :50`, `detached :62`, `interrupted :67`, `timed_out :68`, `stopped :112`, `turn_budget_exceeded :141`, `error :160`, `saved_output_path :176`, `session_file :187`, `child_run_id: Option<RunId> :202`, `output_state :211`. **No `output_path_mapping`, no `output_reference`, no `session_name`.** |
| settlement helpers | `src/workflows/settlement.rs` | `with_workflow_children :97`, `find_workflow_settlement_step :132`, `promote_settled_paused_workflow :165`, `DetachedChildSettlement :203`, `apply_detached_child_settlement :273`, `classify_workflow_settlement :386`, `workflow_recovery_actions :417`, `WorkflowBudgetSignals :444`, `workflow_terminal_outcome_for_result :470`, `workflow_output_path_mapping_summary :494`, `output_path_mappings_of :519`, `WorkflowCompletionEvent :537`, `PlanWorkflowSettlement :596`, `WorkflowSettlementPlan :627`, `plan_workflow_settlement :653` |
| settlement constants | `src/workflows/settlement.rs:33` / `:40` / `:44` | `UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION` / `INTERRUPTED_DETACHED_CHILD` / `EVIDENCE_PERSISTENCE_FAILED` |
| receipt types/IO | `src/workflows/receipt.rs` | `WorkflowReceiptState :85`, `WorkflowTerminalResolution :100`, `WorkflowRecoveryAction :140`, `WorkflowReceiptResume :164` (`latest_run_id()` `:181`, `is_resumable()` `:191`), `WorkflowReceiptEntry :231`, `WorkflowReceipt :316` (`entry()` `:365`), `WorkflowReceiptRef :381`, `WorkflowReceiptError :406`, `workflow_receipt_path :441`, `build_workflow_receipt :474`, `write_workflow_receipt :604`, `read_workflow_receipt :657` |
| external CLI | `src/runner/status.rs:316` / `:402` | `normalize_external_cli_runner_status(&Value) -> Option<ExternalCliRunnerStatus>` / `external_cli_receipt_metadata(&ExternalCliRunnerStatus, Option<&ExternalProcessStatus>, Option<&str>) -> Value` |
| settlement-plan stamper | `src/background/runner_main/finish.rs:181` / `:203` / `:204` | `WorkflowResultFields` / the dead-code attr / `apply_workflow_settlement_plan` |
| `runner_main` re-exports | `src/background/runner_main/mod.rs:76-89` | `mod finish;` is PRIVATE and NOT re-exported |
| `finish_run` | `src/background/runner_main/finish.rs:226` | `pub(super) async fn`, double-invocation guard at `:237-245` via `terminal_result_exists` `:161-172` |
| best in-tree precedent | `src/extension/tool/routing.rs:1002-1098` | `settle_foreground_workflow` — the existing `plan_workflow_settlement` caller |
| session gate | `src/background/delivery/gate.rs:30` / `:51` | `enum SessionGate {Strict, Permissive}` / `admits(self, Option<&SessionId>, Option<&SessionId>) -> bool` |
| completion fan-out | `src/background/watch/observer.rs:1-5`, `:76-108` | `CompletionObserver` + `CompletionEvent` — the ResultFile-driven replacement for pi's bus emit |
| `RunDirName` | `src/identity/run_dir_name.rs:37` / `:58` / `:70` | `parse` / `for_run(&RunId)` (infallible) / `resolve_in(&Path)` |
| error carrier | `src/error.rs:88` | `SubagentError::Management(String)` |
| shape precedent (return struct only) | `src/background/reconcile.rs:148` / `:331` | `pub struct ReconcileOutcome` / `pub async fn reconcile_now(&RunPaths, Option<SystemTime>) -> io::Result<ReconcileOutcome>` |

---

## §W — What cannot work as the draft describes, and the mechanism that can

Each entry is recorded **next to** its original requirement above; this is the index.

**W-1 — `outputPathMappingFromTask` is unported, so the mapping has no source.**
The requirement ("the settled step keeps the mapping derived from THIS result's task/output", and
the whole *"preserves sibling output path mappings"* case) **stands**. The draft's mechanism
(`input.result.output_path_mapping`) does not exist (§V, `SingleResult`).
*Mechanism:* port `output_path_mapping_from_task(task: &str, saved_path: Option<&str>) ->
Option<WorkflowOutputPathMapping>` from `v0.67.0:src/runs/shared/single-output.ts:120-125` —
five lines: derive `requested_path` from the task (its helper `requestedOutputPathFromTask` is in the
same file and also unported), return `None` if either half is missing, and return `None` when the
requested path is absolute and normalizes equal to the saved path. Put it in the new
`workflow_detach/children.rs` (it is used by both the driver and Arm A/B), **not** in
`workflows/settlement.rs` — this task does not own that module.
*Alternative, if the implementor judges the task-string parser out of scope:* add
`output_path_mapping: Option<WorkflowOutputPathMapping>` to `DetachedWorkflowChildCompletion` and
make it the caller's responsibility. **Either is acceptable; silently dropping the mapping is not.**
See §Y-3.

**W-2 — `finish_run` must not be the write path.** (Full argument in the ⟡⟡ block under SUBTASK3.)
*Mechanism:* follow `settle_foreground_workflow` (`src/extension/tool/routing.rs:1002-1098`), which
is already the production `plan_workflow_settlement` caller: build → persist receipt → plan → write
`status.json` with `write_atomic_json` → publish. The reconciler adds the two rungs that function
does not need (terminal index, `write_async_result_file`), both taken verbatim from
`finish.rs:352-366` and `finish.rs:477-500`. **`apply_workflow_settlement_plan` is still the stamper
and its dead-code attribute still comes off** — that requirement is untouched; only the `finish_run`
widening is retracted, replaced by re-exporting `finish::{WorkflowResultFields,
apply_workflow_settlement_plan}` from `runner_main/mod.rs` (blocker (i)).

**W-3 — `read_workflow_receipt` returns `Result`, not `Option`.**
`read_workflow_receipt(async_root: &Path, run: &RunDirName) -> Result<WorkflowReceipt, WorkflowReceiptError>`
(`receipt.rs:657-664`). Its absent-file branch (`receipt.rs:684-700`) probes the run dir and returns
**`MayStillBeActive`** when `status.json` **or** `events.jsonl` exists, else `NotFound`.
**For a live paused workflow both files exist, so the absent-receipt case arrives as
`MayStillBeActive` — not `NotFound`.**
*Mechanism:* map `Err(NotFound | MayStillBeActive) => Ok(None)` (upstream's `!existsSync ⇒ undefined`,
`:100`), and let `Err(Unreadable | Invalid)` become the `receipt_persistence_error` the driver
captures. Do **not** pre-probe with `workflow_receipt_path(...).exists()` and then read — that is a
TOCTOU the typed error already removes.

**W-4 — `workflow_terminal_outcome_for_result` does not take a `&SingleResult`.**
Signature is `(signals: WorkflowBudgetSignals) -> Option<WorkflowTerminalOutcome>`
(`settlement.rs:470-472`). *Mechanism:* `WorkflowBudgetSignals::from_single_result(result)`
(`settlement.rs:461-468`).

**W-5 — `ForegroundControlEntry` has no `run_id`; the registry key is the run id.** (Detail in the
⟡ block under SUBTASK2.) *Mechanism:* filter `(key, entry)` pairs and compare `key` to
`child_run_id`; never hold the `std::sync::Mutex` across an `await`.

**W-6 — `output_path_mappings_of` is the wrong adapter for this task.**
It takes `&[WorkflowScriptChildResult]` (`settlement.rs:519-522`), and this reconciler's children
are `Vec<SingleResult>`, which carry no mapping at all. *Mechanism:* call
`workflow_output_path_mapping_summary` directly with its generic
`impl IntoIterator<Item = (Option<&str>, &WorkflowOutputPathMapping)>` signature
(`settlement.rs:494-498`), fed from `next.steps` as
`step.output_path_mapping.as_ref().map(|m| (step.workflow_key.as_ref().map(WorkflowKey::as_str), m))`.
That generic signature exists for exactly this — its own doc calls it "a future untyped-list
adapter, where a caller can have a mapping with no key" (`settlement.rs:515-518`), and the
`"child"` fallback inside the summary (`settlement.rs:503`) becomes reachable for the first time.

**W-7 — three upstream `baseResult` keys have no `ResultFile` home.**
`plan_workflow_settlement` deliberately does **not** take `children`/`baseResult`
(`settlement.rs:589-595`), so the driver assembles the `ResultFile` itself. Mapping upstream
`:232-247` onto `records.rs:483-543`:
| upstream key | cyrup home |
|---|---|
| `id`, `runId` | `ResultFile::id`/`run_id` ← `plan.status.run_id` |
| `agent: "workflow"` | `ResultFile::agent` |
| `mode: "workflow"` | `ResultFile::mode = RunMode::Workflow` |
| `endedAt` | — (on `RunStatus`, not `ResultFile`) |
| `cwd` | `ResultFile::cwd: PathBuf` ← `plan.status.cwd` (`Option<PathBuf>`!) — needs an explicit fallback |
| `sessionId` | `ResultFile::session_id` ← `plan.status.session_id.clone().or(existing.session_id)` — **the §2 ref-2 order** |
| `completionOwnerId` | `ResultFile::completion_owner_id` |
| results array | `ResultFile::results` ← `workflow_result_children` |
| `workflowChildren`, `workflowReceipt` | from `apply_workflow_settlement_plan`'s `WorkflowResultFields` |
| `toolCallId` | **no field** |
| `asyncDir` | **no field** |
| `workflow` (script state) | **no field** |
| `reconciledFromDetachedChild` | **no field** — survives only on the completion EVENT's `extra` |
| `scheduleOrigin` | **no field anywhere in the crate** (`grep -rn schedule_origin crates/` ⇒ 0) |
| `summary` | **no field** — so `plan.summary` cannot be published on the result, and upstream's `existing?.summary` fallback (`:227`) is unrepresentable |
*Mechanism:* publish only the fields that exist. **Do not add fields to `ResultFile`** — its wire
shape is read by `wait`, the watcher and the projector, and widening it is not this task's scope.
The `summary` loss is real and must be recorded, not papered over; §Y-5 raises it.

**W-8 — cyrup has no write-site bus emit; writing the result file IS the emit.**
Upstream `:281-301` does `input.events?.emit(SUBAGENT_ASYNC_COMPLETE_EVENT, …)`. cyrup's equivalent
signal is produced downstream: `ResultsWatcher` observes the terminal `ResultFile`, classifies it,
and publishes a `CompletionEvent` on `CompletionBus` to every `CompletionObserver`
(`src/background/watch/observer.rs:1-5`, `:12-36`, `:76-108`). The module doc states the reason
verbatim: *"pi's completion signal is an in-process event because pi's runner is in-process;
cyrup's runner is a detached OS process whose only signal is the terminal ResultFile it writes"*.
*Mechanism:* **omit the emit rung entirely.** Write step 3 (`write_async_result_file`) is what
produces the completion. Adding a second, direct publish here would double-deliver every reconciled
workflow to the notification, the mission sync and every `wait`. Say so in the driver's doc comment
so a later reader does not "restore the missing emit". The **`subagent.workflow.completed`
`events.jsonl` append (`:274-280`) is NOT the bus emit and IS still required.**

---

## Required behaviours (fold into the implementation; not a test plan)

These encode the upstream contract verified in
`v0.67.0:test/unit/workflow-detach-reconcile.test.ts` (790 lines, 29 cases).
Each is a property the code must have:

- A detached child that **succeeds** with no persisted continuation fails the workflow **closed**,
  with `UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION` — never "complete".
- Error precedence on promotion is fixed and load-bearing: failed sibling → this child
  interrupted/stopped → this child's own error → interrupted sibling. `PromotedWorkflowError`
  (`src/workflows/settlement.rs:228-241`, applied at `:354-376`)
  already encodes it; do not re-derive it here.
- While **another** detached child still needs attention, the workflow stays `paused` and **no**
  completion is logged.
- A completion matching only on `workflowKey`, with no confirmed live control, is **stale** and
  ignored. Two ambiguous same-key live attempts resolve to nothing.
- A malformed receipt — or a failure to journal the receipt error — still **publishes** the
  detached completion, with `EVIDENCE_PERSISTENCE_FAILED` demoting the status.
- A detached **timeout** outcome stays on its own receipt entry and is not promoted workflow-wide.
- Sibling `outputPathMapping`s survive a rebuild from workflow status.
- Re-running the reconciler after settlement is a **no-op** (the non-`Paused` guard), so a crash
  between backfill and publish replays safely.

---

## §X — Behaviours to pin, as named tests (fail-before / pass-after)

Every one fails today: the module does not exist. Place them in
`src/extension/executor/workflow_detach/mod.rs`'s own `#[cfg(test)] mod tests`, except where noted.
Drive them against a `tempfile::tempdir()` holding a `paused` workflow `status.json`, following
`finish.rs`'s own fixture style (`run_paths_in`, `finish.rs:614-622`).

**Driver / idempotency**
1. `no_status_file_returns_false_without_writing` — upstream `:181`. Assert no `status.json`,
   no result, no `events.jsonl` appear.
2. `a_non_paused_workflow_returns_false_and_writes_nothing` — the `settlement.rs:278-282` guard.
3. `reconciling_twice_is_a_no_op` — run the full driver, capture the on-disk bytes of
   `status.json` + the result payload, run it again, assert byte-identical and the second call
   returns `false`.
4. `a_child_matching_no_step_returns_false` — upstream `:197`.

**Settlement semantics (these are what wake the three dead helpers)**
5. `a_successful_detached_child_fails_the_workflow_closed_with_the_unsupported_continuation_error`
   — asserts `plan.status.error == Some(UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION.into())`
   (`settlement.rs:33`).
6. `another_open_detached_child_keeps_the_workflow_paused_and_logs_no_completion` — assert
   `plan.completion_event.is_none()`, `status.state == RunState::Paused`, and that `events.jsonl`
   contains **no** `subagent.workflow.completed` line.
7. `a_failed_sibling_outranks_this_childs_interruption` — the `PromotedWorkflowError` precedence,
   observed through the driver rather than re-derived.
8. `a_stopped_detached_child_classifies_as_interrupted_evidence` — `classify_workflow_settlement`
   reached from production for the first time.

**Identity backfill (`identity.rs`)**
9. `a_live_control_backfills_a_step_that_has_no_run_id`
10. `two_ambiguous_same_key_live_attempts_backfill_nothing` — must not pick the first.
11. `a_completion_matching_only_on_workflow_key_is_stale_and_ignored` — no confirmed live control.
12. `a_completion_with_no_workflow_key_never_backfills` — clause 0, upstream `:182`.
13. `a_step_that_already_has_a_session_file_is_not_backfilled` — clause 3, one sub-clause at a time
    (four assertions: `run_id`, `session_file`, `status`, `activity_state`).

**Receipt (`receipt.rs`)**
14. `an_absent_receipt_reconciles_to_none_not_an_error` — and specifically that
    `WorkflowReceiptError::MayStillBeActive` (not only `NotFound`) maps to `Ok(None)` (§W-3).
15. `a_step_with_no_workflow_key_raises_the_verbatim_cannot_identify_message`
16. `a_receipt_with_no_entry_for_the_key_raises_the_verbatim_no_detached_child_key_message`
17. `a_malformed_receipt_still_publishes_the_completion_with_evidence_persistence_failed` — assert
    the status error begins with `EVIDENCE_PERSISTENCE_FAILED` (`settlement.rs:44`) and that the
    result file **was** written.
18. `a_receipt_write_failure_is_journalled_and_does_not_fail_the_reconcile` — the
    `subagent.workflow.receipt_write_failed` line at upstream `:265-273`.
19. `an_external_cli_adapter_overrides_a_resumable_verdict` — upstream `:121`; assert the
    `NotResumable::reason` comes from the adapter metadata's `nonResumableReason`.
20. `mixed_pi_and_external_entries_keep_their_own_computed_resumability` — one of each in
    `receipt.entries`; assert only the external one is forced.
21. `a_detached_timeout_outcome_stays_on_its_own_entry_and_is_not_promoted_workflow_wide` —
    `entry.terminal_outcome == Some(Partial{Timeout})` while `plan.status` carries none.
22. `a_resumable_entry_keeps_its_existing_latest_run_id_and_falls_back_to_the_child` — upstream `:127`.

**Children (`children.rs`)**
23. `arm_a_rewrites_only_the_matching_child_and_clears_its_detached_flag` — every sibling
    byte-identical; the target's `detached == false`.
24. `arm_a_preserves_sibling_output_path_mappings` — the named upstream case.
25. `arm_b_rebuilds_from_steps_and_backfills_output_reference_from_the_receipt_entry` — upstream `:88`.
26. `arm_b_backfills_terminal_outcome_from_the_receipt_entry` — upstream `:91`.
27. `arm_b_backfills_output_path_mapping_from_the_step` — upstream `:89`, the rung the old body missed.
28. `an_all_zero_usage_is_suppressed` — `usage_with_value`.
29. `the_success_predicate_ignores_stopped` — `exit_code == Some(0) && error.is_none() &&
    !interrupted` is `true` for a stopped child (`settlement.rs:264-268`'s documented quirk).

**Identity propagation (§2)**
30. `the_published_result_carries_session_id_and_completion_owner_id`
31. `the_session_falls_back_to_the_existing_result_file_when_the_status_has_none` — the
    `next → existing` order, upstream `:244`.
32. `a_reconciled_result_with_no_session_anywhere_is_a_hard_error` — mirrors
    `finish.rs:478-484`; assert `SubagentError::Management`, and that `status.json` was written
    first anyway (R-SA-077's independence).

**Write ordering / SUBTASK3**
33. `the_terminal_index_marker_is_written_for_a_settled_workflow` — through
    `update_terminal_run_index`; assert the marker under `<async_root>/.terminal-runs/<enc>/`.
34. `no_active_run_index_is_created` — assert nothing but `.terminal-runs` appears; this is the
    explicit SCOPE_9 boundary.
35. `apply_workflow_settlement_plan_has_a_production_caller` — compile-level: removing
    `#[cfg_attr(not(test), allow(dead_code))]` from `finish.rs:203` must leave
    `cargo clippy --workspace --all-targets -- -D warnings` clean. (Not a `#[test]`; it is the
    clippy gate in the DoD.)

**SUBTASK4**
36. `promote_paused_workflow_if_settled_delegates_with_the_crate_clock`
37. `apply_detached_child_to_paused_workflow_delegates_unchanged`

---

## Definition of done

- `extension/executor/workflow_detach/{mod,identity,receipt,children}.rs` exists and is declared in
  `src/extension/executor/mod.rs` between `:28` and `:29`.
- `reconcile_detached_workflow_child_completion` settles a `paused` workflow from its run directory
  plus a settled child result, publishing status + result + events in the §3 order **(as corrected
  by §W-2 and §W-8: no `finish_run` call, no direct bus emit)**.
- `apply_detached_child_settlement`, `promote_settled_paused_workflow` and
  `classify_workflow_settlement` have a **production** caller (they have none today).
  **Add: `find_workflow_settlement_step`, `workflow_output_path_mapping_summary` — also dead today.**
- `apply_workflow_settlement_plan` is called from production and its
  `#[cfg_attr(not(test), allow(dead_code))]` is removed; ~~`finish_run` is `pub(crate)` and
  re-exported~~ → **superseded by §W-2: `finish::{WorkflowResultFields,
  apply_workflow_settlement_plan}` are re-exported from `src/background/runner_main/mod.rs`;
  `finish_run` stays `pub(super)` and is NOT called.** The original requirement's purpose — "do not
  duplicate `finish_run`'s write-ordering logic" — is honoured by reusing
  `write_atomic_json` → `update_terminal_run_index` → `write_async_result_file` in that exact order,
  which is the ordering `finish_run` itself performs (`finish.rs:349`, `:352`, `:489`).
- Session identity propagates onto both the published result and the completion event, with
  upstream's `status → existing` fallback order; no new session predicate is introduced.
- No `workflow_controllers` mutation is added by this task (it is WORKFLOW_6's, and already done).
- **No active-run index is created or touched (SCOPE_9's surface).**
- Reconciling twice is a no-op.
- `cargo nextest run -p cyrup-ext-subagents` passes; the workspace stays at 10027 passing.
- `cargo fmt -p cyrup-ext-subagents` only (repo-wide `cargo fmt --all` must remain a no-op).
- `cargo clippy --workspace --all-targets -- -D warnings` exits 0; workspace builds clean.

---

## §Y — Open questions the implementor must NOT silently decide

1. **Does the driver take an `&SubagentExecutor`, or pre-resolved control identity?** §1 decided
   "caller-independent, disk-driven unit", but SUBTASK2's backfill needs
   `state.foregroundControls` (§W-5), which lives on the executor. The two candidate shapes:
   (a) `DetachedWorkflowChildCompletion { … , live_controls: &[(RunId, Option<RunId>, Option<WorkflowKey>)] }`
   — the caller snapshots the registry, the module keeps the whole triple guard; or
   (b) the driver takes `&SubagentExecutor` and reads the private field directly (legal: descendant
   module), giving up §1's testability posture. **(a) preserves §1; (b) is closer to upstream.**
   Pick one and say why in the module doc; do not invent a third.
2. **Where does the CHILD's `RunPaths` come from?** `reconcile_workflow_receipt` step 4 must read
   the child's own `status.json` (upstream `readStatus(path.join(DIRS.async, childRunId))`, `:115`).
   The reconciler holds the WORKFLOW's `RunPaths`. Deriving the child's as
   `RunPaths::for_run(workflow_run_paths.run_dir.parent()?, &workflow_run_paths.results_dir, &child_run_id)`
   assumes the child lives under the same async root. Is that assumption safe, or must the child's
   `RunPaths` be an explicit input field?
3. **Port `output_path_mapping_from_task`, or take the mapping as an input field?** (§W-1.) Porting
   drags in `requestedOutputPathFromTask`'s task-string parsing, which is arguably
   foreground-lifecycle surface; taking it as input pushes the derivation onto a caller that does
   not exist yet. **Neither is free; choose explicitly.**
4. **What `trace` does a disk-driven reconciler pass?** `plan_workflow_settlement` and
   `apply_detached_child_settlement` both require `trace: &[WorkflowScriptTraceEntry]`
   (`settlement.rs:600`, `:276`), and **`RunStatus` has no trace and no `workflow` script-state
   field** (§V) — upstream reads `next.workflow` off the status at `:240`. Passing `&[]` skips
   pass 1 of `workflow_child_summary` (`src/workflows/child_summary.rs:205-212`); passes 2 (steps)
   and 3 (children) still populate the inventory, so the rows survive but any trace-only
   `agent`/`sessionName`/`model` carry-forward (`child_summary.rs:210`) is lost. Is `&[]` acceptable,
   or must the trace become an input field on `DetachedWorkflowChildCompletion` (and if so, who
   supplies it)?
5. **`plan.summary` has nowhere to go on `ResultFile` (§W-7), and upstream's `existing?.summary`
   fallback at `:227` is therefore also unrepresentable.** Options: drop it (losing the operator-facing
   sentence entirely), put it on the completion event's `extra` only, or raise a follow-up to add
   `summary` to `ResultFile`. **Adding the field is OUT OF SCOPE here** — it changes a wire shape four
   readers consume. Which of the first two?
6. **Do the two SUBTASK4 wrappers get `#[cfg_attr(not(test), allow(dead_code))]`?** They have no
   in-tree caller (§SUBTASK4 ⟡). Carrying the attribute reproduces exactly the dead-code debt this
   task is retiring elsewhere; omitting it fails `-D warnings`. Name the future caller in the doc
   either way.
7. **Which `RunState` does a non-promoted (still-`Paused`) reconcile write to the terminal index?**
   `is_indexed_state` admits `Paused` (`terminal_run_index/entry.rs:103-105`), so calling
   `update_terminal_run_index` unconditionally writes a marker for a workflow that has NOT finished.
   Upstream's `updateActiveRunIndex` routes non-terminal states to the ACTIVE index instead — which
   cyrup does not have and this task must not build. Is writing a terminal marker for a `Paused`
   workflow correct here, or should the call be gated on `plan.status.state.is_terminal()`?

---

## Research notes

* Upstream: `v0.67.0:src/runs/foreground/workflow-detach-reconcile.ts` (304 LOC). Read via
  `git -C /home/user/cyrup/tmp/pi-subagents show v0.67.0:<path>` — **never from the working tree**
  (its HEAD is `4ab1b1b8` and moves). The 2026-09-12 aug's pin, HEAD `57278d82`, is the source of
  every stale line number §U corrects.
* Upstream helper layer: `v0.67.0:src/workflows/workflow-settlement.ts` (249 LOC) and
  `workflow-receipt.ts` (366 LOC) — **both already ported**; see §0.
* Upstream tests: `v0.67.0:test/unit/workflow-detach-reconcile.test.ts` (790 LOC, 29 cases —
  8 under `describe("applyDetachedChildToPausedWorkflow")` at `:30`, 21 under
  `describe("reconcileDetachedWorkflowChildCompletion")` at `:147`). §X maps them to Rust names.
* Sole upstream caller: `subagent-executor.ts:3951-3976` (`onDetachedExit`); second entry point at
  `:5763` (`promotePausedWorkflowIfSettled`).
* `workflowControllers` is mutated only at `subagent-executor.ts:5098`/`:5272`/`:5796` and
  `extension/index.ts:1041` — all ported by WORKFLOW_6. **Not this file's concern.**
* Related upstream reader: `async-dismiss-action.ts:39` treats a live `workflowControllers` entry as
  "not dismissible" — WORKFLOW_8 ports that; keep the two views of the registry consistent.
* `PARITY-GAPS.md` `SUBA-057` ("`dismiss` — a recovered workflow with no live controller is stuck
  'running' forever") is adjacent; this task plus WORKFLOW_8 should close it.
* Shape precedent the draft named — `background/reconcile.rs`'s `reconcile_now`
  (`src/background/reconcile.rs:331`) / `ReconcileOutcome` (`src/background/reconcile.rs:148`) —
  is a **crash-recovery** reconciler for a run whose *runner process* died. This task's reconciler
  settles a *live* workflow whose *child* detached. Borrow the `Outcome`-struct return shape; do
  **not** borrow its liveness-probing logic.
* The genuinely better precedent is **two** functions, not one:
  `src/background/runner_main/finish.rs:176-219` (`WorkflowResultFields` +
  `apply_workflow_settlement_plan`), which was written *for* this task and supplies the STAMP; and
  `src/extension/tool/routing.rs:1002-1098` (`settle_foreground_workflow`), the existing production
  `plan_workflow_settlement` caller, which supplies the WRITE SHAPE (§W-2).
* Batch-1 context (PR #137, 9715 insertions) that post-dates the 2026-09-12 aug and was checked for
  collisions with this task: `background/completion_replay/` (SCOPE_4), `exec/model_exclusions/`
  (SCOPE_3j), `background/wait_completions/` session-index resolution (SCOPE_3),
  `ForegroundChildControl::steer` (SCOPE_18). **None of them touches any seam this task edits**;
  the only overlap is that `wait_completions/project.rs` reads `ResultFile::workflow_children`,
  which this task now becomes a second producer of.
