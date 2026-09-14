---
stage: aug
status: done
updated: 2026-09-14 06:00
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

> ## ⚠ SCOPE CORRECTION (2026-09-12 aug) — two of the three original subtasks were factually wrong
>
> The pre-aug draft of this file was written from the filename, not the source. Reading
> [`workflow-detach-reconcile.ts`](../../../workspace/cyrup/tmp/pi-subagents/src/runs/foreground/workflow-detach-reconcile.ts)
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
>    [`workflow_controllers.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow_controllers.rs).
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

**Depends on WORKFLOW_6** ([`workflow_controllers.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/workflow_controllers.rs)) — landed.

---

## §0 — Ground truth: what is already ported (verified, do not re-port)

The draft budgeted "302 LOC". That is the upstream file's size, **not** the remaining work. The
helper layer it imports was ported wholesale by earlier tasks. Verified inventory:

| upstream symbol (`workflow-settlement.ts` / `workflow-receipt.ts`) | cyrup equivalent | status |
|---|---|---|
| `applyDetachedChildSettlement` | `workflows::apply_detached_child_settlement` ([settlement.rs:271](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `promoteSettledPausedWorkflow` | `workflows::promote_settled_paused_workflow` ([:165](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `findWorkflowSettlementStep` | `workflows::find_workflow_settlement_step` ([:132](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported (returns an **index**) |
| `classifyWorkflowSettlement` | `workflows::classify_workflow_settlement` ([:382](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `planWorkflowSettlement` / `WorkflowSettlementPlan` | `workflows::plan_workflow_settlement` ([:649](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)), `PlanWorkflowSettlement` ([:592](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `workflowRecoveryActions` | `workflows::workflow_recovery_actions` ([:413](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `workflowTerminalOutcomeForResult` | `workflows::workflow_terminal_outcome_for_result` ([:466](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `workflowOutputPathMappingSummary` | `workflows::workflow_output_path_mapping_summary` ([:490](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) + `output_path_mappings_of` ([:515](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs)) | ✅ ported |
| `readWorkflowReceipt` / `writeWorkflowReceipt` / `workflowReceiptPath` | `workflows::{read,write}_workflow_receipt`, `workflow_receipt_path` ([receipt.rs:660/607/446](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/receipt.rs)) | ✅ ported |
| `WorkflowReceipt` / `Entry` / `Resume` | [receipt.rs:321/233/164](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/receipt.rs) | ✅ ported |
| `normalizeExternalCliRunnerStatus` / `externalCliReceiptMetadata` | [`runner/status.rs:316/402`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/runner/status.rs) | ✅ ported |
| `WorkflowPublicChild` | — | **intentionally absent**: cyrup's terminal payload is the typed [`ResultFile`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/records.rs) (`records.rs:483`) with `results: Vec<SingleResult>`, not an open map. Do **not** introduce a `PublicChild` type. |
| `WorkflowStatusStep` | — | folded into `background::StepStatus` ([records.rs:24](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/records.rs)); `output_path_mapping` is already a field (`:137`). |

**The load-bearing consequence:** `apply_detached_child_settlement`,
`promote_settled_paused_workflow` and `classify_workflow_settlement` currently have **zero
production callers** — only their own unit tests. They are ported, correct, and dead. **This task
is what wakes them up.** That is the actual value proposition, and it is why this task is worth
doing even though the helper layer is complete.

---

## §1 — The one real blocker: there is no detach hook, so there is no caller

Upstream's call site is
[`subagent-executor.ts:3951-3976`](../../../workspace/cyrup/tmp/pi-subagents/src/runs/foreground/subagent-executor.ts) —
the `onDetachedExit` callback, fired from
[`execution.ts:2364`](../../../workspace/cyrup/tmp/pi-subagents/src/runs/foreground/execution.ts)
when a foreground child that was **detached** (via `detachForeground`, `execution.ts:638`, reached
either by the `/detach` slash command at `slash-commands.ts:984` or by intercom coordination at
`execution.ts:788`) eventually terminates.

**cyrup has none of that machinery.** Verified:

- No `detach_foreground`, no `on_detach_ready`, no `on_detached_exit` anywhere in the crate.
- cyrup's `SingleResult::detached` ([`run_result.rs:62`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/run_result.rs))
  is set by the drive loop (`drive_attempt.rs`, `detached_seen` at `:183`/`:247`) when a blocking
  `contact_supervisor` ask is surfaced — the *same trigger family* as upstream's intercom arm, but
  it is a **flag on the returned result**, not a "return the tool call early and keep the child
  running" mechanism, and nothing fires on the detached child's later exit.
- The only detached-exit remnant is `disarm_structured_guard_on_detach`
  ([`exec/mod.rs:879`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/exec/mod.rs)) —
  cleanup is *disarmed*, and then never later performed.

### Decision — build the reconciler as a caller-independent, disk-driven unit

Do **not** build a detach mechanism in this task (that is a foreground-lifecycle task, not this
one, and the draft's dependency chain does not authorise it). Instead:

**Port the reconciler so its input is the run directory plus a settled child result — not a live
callback closure.** Every value upstream reads off in-process state has a durable source:

| upstream in-process read | disk / registry source to use instead |
|---|---|
| `state.asyncJobs.get(workflowRunId)?.asyncDir` | `RunPaths` for the workflow run ([`run_paths.rs:85`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/run_paths.rs)) — upstream already falls back to `path.join(DIRS.async, runId)` when the job is absent, so the disk path is the *primary*, not a degradation |
| `state.foregroundControls.values()` filtered by `parentWorkflowRunId`/`workflowKey` | `ForegroundControlEntry` ([`notices.rs:20-97`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs)) — `parent_workflow_run_id` (`:73`), `workflow_key` (`:78`), `session_id` (`:68`) — already populated by WORKFLOW_6 |
| `readStatus(asyncDir)` | `read_status_file` ([`control.rs:316`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs)) |

This keeps the module **fully exercisable today** against a tempdir containing a `paused` workflow
`status.json`, and makes wiring it to a future detach hook a one-line call rather than a rewrite.
It is the identical posture [`finish.rs:190-203`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/finish.rs)
already documents for `apply_workflow_settlement_plan` — see §3.

---

## SUBTASK1 — `extension/executor/workflow_detach/`

**Where:** new module `crates/cyrup-ext-subagents/src/extension/executor/workflow_detach/`, declared
in [`executor/mod.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/mod.rs)
alongside its siblings (`pub(crate) mod workflow_detach;`, alphabetical — after `workflow_controllers`,
before `workflow_steering`).

**Ports:** [`workflow-detach-reconcile.ts`](../../../workspace/cyrup/tmp/pi-subagents/src/runs/foreground/workflow-detach-reconcile.ts).

**Layout** — the draft's `{detect,reconcile,report}` split does not match the source's actual seams.
Use the three the file really has:

```
workflow_detach/
  mod.rs        // the driver: reconcile_detached_workflow_child_completion (:170-302)
  identity.rs   // the live-identity backfill guard (:177-190) + usage_with_value (:41-45)
  receipt.rs    // reconcile_workflow_receipt (:98-160) — resumability + external-adapter merge
  children.rs   // workflow_result_children (:47-96) — rebuild the results array
```

`report.rs` is not warranted: "reporting" upstream is two `appendFileSync` calls to `events.jsonl`
plus one event emit, all inside the driver, and cyrup already owns the bounded-append primitive
(`RunPaths::events` must be written through `crate::jsonl::BoundedJsonlWriter`, per its own doc at
[`run_paths.rs:90-94`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/run_paths.rs)).

### The driver, prescriptively

Upstream's driver is a straight pipeline. Port it in the same order — the order **is** the
behaviour (a receipt failure must be able to demote a status that was already promoted):

```rust
/// pi `reconcileDetachedWorkflowChildCompletion` (`workflow-detach-reconcile.ts:170-302`).
///
/// Returns `false` — WITHOUT writing anything — when the run has no status, or when the status is
/// not a `paused` workflow, or when the settling child matches no step. Upstream's three early
/// `return false`s are the idempotency guarantee: replaying this after the workflow already
/// settled is a no-op, because `apply_detached_child_settlement` refuses a non-`Paused` status
/// (`settlement.rs:277`).
pub(crate) async fn reconcile_detached_workflow_child_completion(
    input: DetachedWorkflowChildCompletion<'_>,
) -> Result<bool, SubagentError> {
    let Some(status) = read_status_file(&input.run_paths.status).await? else {
        return Ok(false);                                    // `:173`
    };

    // `:175-190` — live-identity backfill, BEFORE settlement. See SUBTASK2.
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
        return Ok(false);                                    // `:196`
    };
    let mut next = next;

    // `:197-200` — the settled step keeps the mapping derived from THIS result's task/output.
    if let Some(mapping) = &input.result.output_path_mapping
        && let Some(index) = find_workflow_settlement_step(&next.steps, input.child_run_id, None, None)
        && let Some(step) = next.steps.get_mut(index)
    {
        step.output_path_mapping = Some(mapping.clone());
    }

    // `:210` — classification is computed on the SETTLED status, before the receipt pass.
    let resolution = classify_workflow_settlement(&next, input.result.interrupted);

    // `:211-217` — a receipt fault is CAPTURED, never propagated: available child evidence must
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
        event_metadata: reconciled_from_detached_child(input.child_run_id),   // `:249`
    });
    // …then §3's write path.
}
```

**Note the `terminal_outcome` argument is read from the receipt** (`:247`), not recomputed from the
result — upstream deliberately lets the receipt's merged outcome win, because
`reconcileWorkflowReceipt` has already folded `entry.terminalOutcome` into it.

### `workflow_result_children` (`children.rs`)

Upstream has **two arms** (`:56-96`) and both are required:

- **Arm A — an existing result file was readable and its `results` is an array (`:56`):** map over
  the *existing* entries, rewriting only the one whose `runId` matches, and **clearing `detached`**
  on it (`detached: undefined`, `:64`). Every other entry is preserved byte-for-byte — this is what
  the upstream case *"preserves sibling output path mappings when rebuilding from workflow status"*
  protects. Non-object entries pass through untouched (`:59`).
- **Arm B — no existing result file (`:78`):** rebuild the whole array from `status.steps`, and
  back-fill `outputReference`/`terminalOutcome` **from the receipt entry for that step's
  `workflowKey`** when the step is not the settling child (`:86`, `:89`). Dropping this fallback
  loses sibling evidence whenever the paused result file was already collected.

In cyrup this is `Vec<SingleResult>`, not a JSON map. Arm A therefore reads the prior
[`ResultFile`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/records.rs)
(`records.rs:483`) and mutates the matching `SingleResult`; Arm B constructs from `status.steps`.
The `success` predicate is `exit_code == Some(0) && error.is_none() && !interrupted` (`:62`) — it
does **not** consult `stopped`, matching `apply_detached_child_settlement`'s own documented quirk.

`usage_with_value` (`:41-45`) is the guard that suppresses an all-zero usage block; port it
verbatim — an all-zero `usage` on a reconciled child is upstream-invisible and must stay so.

### `reconcile_workflow_receipt` (`receipt.rs`)

Upstream `:98-160`. Required behaviours, in order:

1. **Absent receipt file → `None`, not an error** (`:101`). A workflow that never wrote a receipt
   reconciles fine.
2. **The child must be identifiable by stable key** — find the step by `runId`, take its
   `workflowKey`; **throw if there is none** (`:105`) and **throw if the receipt has no such entry**
   (`:107`). These are the two faults the driver converts into `receipt_persistence_error`, and
   they are what the upstream *"publishes detached completion when the workflow receipt is
   malformed"* case exercises. Do not soften either to a silent skip.
3. **Resumability** (`:109-115`): upstream calls `resolveAsyncResumeTarget(..., { requireSessionFile: true, sessionId: status.sessionId })`
   and maps `kind === "revive"` → `Resumable`, anything else → `NotResumable { reason: "child is still running" }`,
   and any throw → `NotResumable { reason: <error text> }`.
   **cyrup has no `resolve_async_resume_target`.** The nearest equivalent is
   [`control.rs:2225`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/control.rs)'s
   `resume()`, which is a *performing* call, not a *probing* one. **Do not call it.** Derive the
   verdict from the child's own terminal record instead, which is what `requireSessionFile: true`
   is actually asking:
   `Resumable { latest_run_id }` iff the child's status is terminal **and** carries a
   `session_file`; otherwise `NotResumable { reason: "child is still running" }`. Use
   `WorkflowReceiptResume` ([receipt.rs:164](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/receipt.rs)),
   whose `Resumable` arm structurally requires the `latest_run_id` upstream sets at `:127`.
4. **External-CLI override is last and absolute** (`:122`): if an external adapter is present,
   `resumability` becomes `NotResumable { reason: externalAdapter.nonResumableReason }`
   **regardless** of step 3. Read the child's own status for the single-step external runner
   (`:119-120`) and normalise through
   [`normalize_external_cli_runner_status`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/runner/status.rs)
   (`:316`) / `external_cli_receipt_metadata` (`:402`). This is what keeps mixed Pi/external
   entries on their own computed resumability rather than a blanket verdict.
5. **`terminalOutcome` merges, it does not replace** (`:123`):
   `workflow_terminal_outcome_for_result(result).or(entry.terminal_outcome)`. A detached timeout
   outcome stays local to its entry and must not be promoted onto the workflow.
6. The receipt's top-level `state` maps from the **settled** status (`:146`), `workflowChildren` is
   overwritten from it (`:151`), and `recovery` is set **iff** a `resolution` was passed — else
   both `workflowResolution` and `recovery` are removed (`:154-157`).

---

## SUBTASK2 (replaces "adopt only this session's detached workflows") — identity, not adoption

There are exactly three `sessionId` references in the upstream file. None is an ownership gate:

| ref | upstream | what it actually does |
|---|---|---|
| 1 | `resolveAsyncResumeTarget(…, { sessionId: status.sessionId })` (`:111`) | scopes the **resumability probe** to the workflow's own session |
| 2 | `sessionId: next.sessionId ?? existing?.sessionId` (`:243`) | **propagates** the session onto the published result, preferring the live status and falling back to the prior result file |
| 3 | `sessionId: plan.status.sessionId` (`:293`) | **propagates** the session onto the completion event |

**The requirement is propagation fidelity, not a new predicate.** The draft's instinct — *"do not
introduce a fourth predicate"* — was right; its mechanism was wrong. Concretely:

- The published `ResultFile` **must** carry `session_id` and `completion_owner_id`
  ([`records.rs:483`+](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/records.rs)).
  These are already documented there as the partition key for `result-index/sessions/` and the
  second half of `owns_completion`. The **existing** `SessionGate`
  ([`gate.rs:29-51`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/delivery/gate.rs),
  `admits(self, current, record)`, `Strict`/`Permissive`) then does the partitioning for free.
  A reconciled result that drops `session_id` is undeliverable by construction —
  `write_async_result_file` ([`result_index/write.rs:223`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/result_index/write.rs))
  documents a missing session as a **hard error**, so this fails loudly rather than silently
  stranding the workflow.
- Note ref 2's fallback order (`next.sessionId` **then** `existing.sessionId`): this is precisely
  the upstream case *"preserves quiet schedule attribution when the paused result file is already
  gone"*. Reversing the order, or dropping the fallback, loses attribution for a workflow whose
  paused result file was collected before the child exited.
- **Do not add a `SessionGate` call in this module.** The gate belongs to delivery; adding a second
  enforcement point here is the "fourth predicate" the draft correctly warned against.

### The live-identity backfill (`identity.rs`) — the real correctness subtlety

`:177-190` is the part that is easy to get wrong. Before settling, upstream may write the child's
`runId` onto a step that has none — but **only** under a triple guard:

1. exactly **one** `foregroundControl` matches this `parentWorkflowRunId` + `workflowKey`, and its
   `runId` is the settling child (`:183-186`); **and**
2. exactly **one** status step carries that `workflowKey` (`:188`); **and**
3. that step has `runId === undefined` **and** `sessionFile === undefined` **and**
   `status === "paused"` **and** `activityState === "needs_attention"` (`:189-190`).

All three exist to refuse a claim the caller cannot actually justify. Two same-key live attempts
must resolve to **nothing** — never "the first one". Equally, a completion that matches only on
`workflowKey`, with no confirmed live control, must be treated as **stale** and ignored. Port the
guard whole; relaxing any clause admits cross-lane corruption that no later stage can detect.

Source for the matching controls is `ForegroundControlEntry`
([`notices.rs:20-97`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/notices.rs)):
`parent_workflow_run_id` (`:73`), `workflow_key` (`:78`).

---

## SUBTASK3 (replaces the `workflow_controllers` claim) — wake the deferred seam

WORKFLOW_3 left this task an explicit, documented seam. From
[`finish.rs:190-203`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/finish.rs):

> *"Lives beside `finish_run` rather than inside it… A future async workflow arm calls this and
> then `finish_run` — a call, not a rewrite of either."*
> *"`#[allow(dead_code)]` outside `#[cfg(test)]`: this task's own unit test below is this
> function's only caller until a future async workflow arm becomes the first production one."*

**SCOPE_8 is that arm.** Required:

1. Use `apply_workflow_settlement_plan(&plan, &mut status) -> WorkflowResultFields`
   ([`finish.rs:204`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/finish.rs))
   to stamp the plan. Do **not** open-code the assembly — upstream open-codes it twice and that
   duplication is exactly what this helper was extracted to prevent.
2. **Remove the now-false `#[cfg_attr(not(test), allow(dead_code))]`** from
   `apply_workflow_settlement_plan` (`finish.rs:203`) and update its doc comment: the "future async
   workflow arm" has arrived and should be named. Same for `WorkflowResultFields` if it carries the
   same attribute.
3. **Visibility:** `finish_run` is `pub(super)` (`finish.rs:226`) — reachable only inside
   `background::runner_main`. `apply_workflow_settlement_plan` is already `pub(crate)` (`:204`).
   This module lives in `extension::executor`, so it can call the latter but **not** the former.
   Widen `finish_run` to `pub(crate)` and re-export it from
   [`runner_main/mod.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/mod.rs)
   (which currently exports only `config`/`entry`/`events`/`executor` items). Widening one
   function's visibility is the correct move; duplicating its write-ordering logic is not —
   `finish_run` is R-SA-077's write-ordering funnel and that ordering is the guarantee.

### The write path (`:275-301`), in order — the ordering is the guarantee

```
write status.json (atomic)            // `:275`  — via RunPaths::status
update the run index                  // `:276`  — see the gap note below
write the result file                 // `:281`  — write_async_result_file (stage → index → promote)
append receipt_write_failed event     // `:282-289` — only when receipt_error.is_some()
append subagent.workflow.completed    // `:291-292` — only when plan.completion_event.is_some()
emit the async-complete event         // `:293-301`
```

**Gap note — `updateActiveRunIndex` is unported.** cyrup has only
[`terminal_run_index`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/terminal_run_index/entry.rs)
(session-partitioned terminal markers: `index_root` `:108`, `session_index_dir` `:117`,
`marker_path` `:143`). Since every state this reconciler publishes is terminal or `paused`, route
terminal settlements through the **terminal** index and skip the active-index update. Do not invent
an active-run index here; that is SCOPE_9's surface.

**Event emission must be gated on `plan.completion_event`** (`:290`). When another detached child
is still open, `promote_settled_paused_workflow` leaves the workflow `paused`, the plan produces
**no** completion event, and this reconciler must write status + result and then stop. Emitting a
completion for a still-paused workflow is the worst available failure: it publishes a terminal
verdict the workflow has not reached.

`event_metadata` must carry `reconciledFromDetachedChild: <childRunId>` (`:249`), and
`WorkflowCompletionEvent::extra` ([settlement.rs:533](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs))
already merges caller keys **last** for exactly this purpose.

---

## SUBTASK4 — the `promote_paused_workflow_if_settled` call site

Upstream exports a second, thinner entry point used by the workflow's **own** settlement path
(`subagent-executor.ts:5763`): after a workflow fails with `errorKind: "detached-child"` and is
therefore parked at `paused`, it immediately asks whether the workflow is *already* settled (every
detached child having exited before the engine unwound) and promotes it if so.

`WorkflowScriptErrorKind::DetachedChild` already exists in cyrup
([`scripted/types.rs:249-250`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/scripted/types.rs)).
Expose the thin wrapper from this module —

```rust
/// pi `promotePausedWorkflowIfSettled` (`workflow-detach-reconcile.ts:39-41`) — the re-export the
/// workflow's own settlement path calls, distinct from the driver's internal use.
pub(crate) fn promote_paused_workflow_if_settled(
    status: &RunStatus,
    trace: &[WorkflowScriptTraceEntry],
) -> Result<Option<SettledWorkflowStatus>, String> {
    promote_settled_paused_workflow(status, trace, crate::time::now_epoch_millis())
}
```

— and likewise `apply_detached_child_to_paused_workflow` for `applyDetachedChildToPausedWorkflow`
(`:32-37`). Both are deliberate upstream indirection: the settlement module owns the logic, this
module owns the *entry point the detach lifecycle calls*. Keep the indirection; it is the seam that
lets the call sites move without touching `workflows/`.

---

## Required behaviours (fold into the implementation; not a test plan)

These encode the upstream contract verified in
[`workflow-detach-reconcile.test.ts`](../../../workspace/cyrup/tmp/pi-subagents/test/unit/workflow-detach-reconcile.test.ts).
Each is a property the code must have:

- A detached child that **succeeds** with no persisted continuation fails the workflow **closed**,
  with `UNSUPPORTED_DETACHED_WORKFLOW_CONTINUATION` — never "complete".
- Error precedence on promotion is fixed and load-bearing: failed sibling → this child
  interrupted/stopped → this child's own error → interrupted sibling. `PromotedWorkflowError`
  ([settlement.rs](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/workflows/settlement.rs))
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

## Definition of done

- `extension/executor/workflow_detach/{mod,identity,receipt,children}.rs` exists and is declared in
  [`executor/mod.rs`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/extension/executor/mod.rs).
- `reconcile_detached_workflow_child_completion` settles a `paused` workflow from its run directory
  plus a settled child result, publishing status + result + events in the §3 order.
- `apply_detached_child_settlement`, `promote_settled_paused_workflow` and
  `classify_workflow_settlement` have a **production** caller (they have none today).
- `apply_workflow_settlement_plan` is called from production and its
  `#[cfg_attr(not(test), allow(dead_code))]` is removed; `finish_run` is `pub(crate)` and
  re-exported.
- Session identity propagates onto both the published result and the completion event, with
  upstream's `status → existing` fallback order; no new session predicate is introduced.
- No `workflow_controllers` mutation is added by this task (it is WORKFLOW_6's, and already done).
- Reconciling twice is a no-op.
- `cargo clippy --workspace --all-targets` exits 0; workspace builds clean.

---

## Research notes

* Upstream: [`workflow-detach-reconcile.ts`](../../../workspace/cyrup/tmp/pi-subagents/src/runs/foreground/workflow-detach-reconcile.ts)
  (304 LOC, HEAD `57278d82`). Vendored for citation at `./tmp/pi-subagents` (symlink).
* Upstream helper layer: [`workflow-settlement.ts`](../../../workspace/cyrup/tmp/pi-subagents/src/workflows/workflow-settlement.ts)
  (249 LOC) and `workflow-receipt.ts` (366 LOC) — **both already ported**; see §0.
* Sole upstream caller: [`subagent-executor.ts:3951-3976`](../../../workspace/cyrup/tmp/pi-subagents/src/runs/foreground/subagent-executor.ts)
  (`onDetachedExit`); second entry point at `:5763` (`promotePausedWorkflowIfSettled`).
* `workflowControllers` is mutated only at `subagent-executor.ts:5098`/`:5272`/`:5796` and
  `extension/index.ts:1041` — all ported by WORKFLOW_6. **Not this file's concern.**
* Related upstream reader: `async-dismiss-action.ts:39` treats a live `workflowControllers` entry as
  "not dismissible" — WORKFLOW_8 ports that; keep the two views of the registry consistent.
* `PARITY-GAPS.md` `SUBA-057` ("`dismiss` — a recovered workflow with no live controller is stuck
  'running' forever") is adjacent; this task plus WORKFLOW_8 should close it.
* Shape precedent the draft named — `background/reconcile.rs`'s `reconcile_now`
  ([`:331`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/reconcile.rs)) /
  `ReconcileOutcome` ([`:148`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/reconcile.rs)) —
  is a **crash-recovery** reconciler for a run whose *runner process* died. This task's reconciler
  settles a *live* workflow whose *child* detached. Borrow the `Outcome`-struct return shape; do
  **not** borrow its liveness-probing logic.
* The genuinely better precedent is [`finish.rs:182-219`](../../../workspace/cyrup/crates/cyrup-ext-subagents/src/background/runner_main/finish.rs)
  (`WorkflowResultFields` + `apply_workflow_settlement_plan`), which was written *for* this task.

/home/d0m17bw/.flux/-home-d0m17bw-workspace-cyrup/todo/SCOPE_8.md
