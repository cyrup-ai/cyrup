//! Human-readable status reporting for the `status` control action (C5).
//!
//! This module is the Rust port of pi's `runs/background/run-status.ts` `inspectSubagentStatus`
//! (`run-status.ts:101-273`) and the `async-status.ts` `listAsyncRuns`/`formatAsyncRunList`
//! no-id "list active runs" shape it delegates to (`async-status.ts:366-537`). It owns the
//! *rendering* half of the `status` action; the *acting* control ops (`interrupt`/`resume`/
//! `append-step`) live in [`super::control`]. `extension.rs`'s control-action dispatch arms route
//! into both.
//!
//! # What this reproduces vs. pi (an honest, scoped delta)
//!
//! pi's status report carries a large live-telemetry surface — per-step activity glyphs,
//! `currentTool`/`recentTools`/token counts, nested-run descendant trees, parallel-group
//! normalization, model+thinking suffixes — most of which is fed by fields cyrup's
//! [`RunStatus`]/[`StepStatus`] deliberately do not yet carry (the workflow-graph snapshot and
//! live activity telemetry are tracked as separate net-new work in the gap analysis, not this
//! tier's dispatch task). This module renders the faithful SUBSET cyrup's status schema supports:
//! run identity/state/mode/progress, pending-append count, start/update timestamps, per-step
//! agent+state(+model+error) lines, the reconciliation-repaired terminal `Result:` reference, the
//! `Log`/`Events` artifact references, and — for a non-running run — the exact
//! `formatResumeGuidance` shape (`run-status.ts:51-66`) an LLM reads to revive a child. Nested
//! descendants, activity labels, and parallel-group step-index nesting are omitted here (they have
//! no source fields), documented rather than silently faked.
//!
//! Every disk read runs through [`super::control::reconcile_before_control_op`] first (R-SA-079:
//! never render a `status.json` that might be stale relative to an authoritative terminal
//! [`super::ResultFile`] or a since-dead pid), exactly as pi's `inspectSubagentStatus`/`listAsyncRuns`
//! call `reconcileAsyncRun` before summarizing.

use std::path::{Path, PathBuf};

use crate::error::SubagentError;

use super::control::{reconcile_before_control_op, validate_safe_token};
use super::{RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus};
use crate::identity::SessionId;

// =================================================================================================
// Label helpers — lowercase wire spellings matching pi's `state`/`mode`/step-`status` strings
// =================================================================================================

/// The lowercase state string pi renders (`AsyncStatus.state`): `queued`/`running`/`paused`/
/// `complete`/`failed`/`stopped` (`async-status.ts:69` @v0.43.0 declares exactly this union, minus
/// the unported `rejected` checkpoint state).
pub(crate) fn run_state_label(state: RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running => "running",
        RunState::Paused => "paused",
        RunState::Complete => "complete",
        RunState::Failed => "failed",
        // G77: its own word, never `"failed"` — every upstream reader that renders a run's state
        // for a human prints `"stopped"` verbatim (`run-status.ts:478-479`, `notify.ts:210`).
        RunState::Stopped => "stopped",
    }
}

/// The lowercase mode string pi renders (`SubagentRunMode`): `single`/`parallel`/`chain`. The
/// crate's single definition lives in [`crate::formatters::run_mode_label`]; re-exported here so
/// every existing `run_status::run_mode_label`/`super::run_status::run_mode_label` call site keeps
/// resolving unchanged.
pub(crate) use crate::formatters::run_mode_label;

/// The lowercase per-step status string pi renders (`AsyncJobStep.status`).
pub(crate) fn step_state_label(state: StepState) -> &'static str {
    match state {
        StepState::Pending => "pending",
        StepState::Running => "running",
        StepState::Paused => "paused",
        StepState::Complete => "complete",
        StepState::Failed => "failed",
        // G77 — pi `step.status = "stopped"` (`subagent-runner.ts:2967`).
        StepState::Stopped => "stopped",
    }
}

// =================================================================================================
// Progress + step-line labels (`run-status.ts:63-75`, `async-status.ts:486-505`)
// =================================================================================================

/// The one-line progress label (pi `formatAsyncRunProgressLabel`, `async-status.ts:486-505`),
/// rendered from the fields cyrup's [`RunStatus`] carries. pi's parallel-group normalization and
/// per-group `formatParallelOutcome` are approximated here by cyrup's own step list: for a parallel
/// run, the count of terminal steps over the total; for a chain, the logical step cursor over
/// `chain_step_count`; otherwise the step cursor over the flat step count.
pub(crate) fn progress_label(status: &RunStatus) -> String {
    let step_count = status.steps.len().max(1);
    let chain_step_count = status.chain_step_count.unwrap_or(step_count);
    match status.mode {
        RunMode::Parallel => {
            let done = status
                .steps
                .iter()
                .filter(|step| step.status.is_terminal())
                .count();
            format!("{done}/{} complete", status.steps.len().max(1))
        }
        RunMode::Chain => match status.current_step {
            Some(cur) => format!("step {}/{chain_step_count}", cur.saturating_add(1)),
            None => format!("steps {chain_step_count}"),
        },
        RunMode::Single => match status.current_step {
            Some(cur) => format!("step {}/{step_count}", cur.saturating_add(1)),
            None => format!("steps {step_count}"),
        },
        RunMode::Workflow => {
            // A workflow's inventory is discovered, not declared: there is no denominator to
            // divide by until `inventoryComplete`. Report what is settled out of what is KNOWN,
            // exactly as `Parallel` does — never `step n/N`, which would invent a total the
            // script has not fixed.
            let done = status
                .steps
                .iter()
                .filter(|step| step.status.is_terminal())
                .count();
            format!("{done}/{} complete", status.steps.len().max(1))
        }
    }
}

/// The per-step line prefix (pi `stepLineLabel`, `run-status.ts:63-75`): `Agent i/N` for a
/// standalone parallel run, `Step i/N` for a chain, bare `Step i` for a single run. pi's
/// parallel-group-aware `Step X/Y Agent A/B` nesting inside a chain is flattened here to the
/// enclosing `Step i/N` (cyrup does not yet carry the per-group step-index projection that nesting
/// needs).
fn step_line_label(status: &RunStatus, index: usize) -> String {
    let one_based = index.saturating_add(1);
    match status.mode {
        RunMode::Parallel => format!("Agent {one_based}/{}", status.steps.len().max(1)),
        RunMode::Chain => format!(
            "Step {one_based}/{}",
            status.chain_step_count.unwrap_or(status.steps.len().max(1))
        ),
        RunMode::Single => format!("Step {one_based}"),
        // `"Child"` rather than `"Agent"`/`"Step"` because a workflow row is addressed by its
        // `WorkflowKey`, not by position — `StepStatus::workflow_key` is what a caller actually
        // names it by. The denominator is the DISCOVERED count, same rationale as the progress
        // label above.
        RunMode::Workflow => format!("Child {one_based}/{}", status.steps.len().max(1)),
    }
}

// =================================================================================================
// Resume guidance (`run-status.ts:51-66`)
// =================================================================================================

/// G90: pi `formatSteeringSummary` (`run-status.ts:88-93` @v0.43.0) — `"3 steers, last
/// 2026-08-10T12:00:00.000Z"`, with either half omitted when unknown and `None` when neither is.
fn format_steering_summary(steer_count: Option<u64>, last_steer_at: Option<i64>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(count) = steer_count {
        parts.push(format!(
            "{count} steer{}",
            if count == 1 { "" } else { "s" }
        ));
    }
    if let Some(at) = last_steer_at {
        parts.push(format!("last {}", format_iso8601_millis(at)));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// pi's verbatim resume-guidance line for a STOPPED run (`run-status.ts:52` @v0.43.0). Public so
/// the `status`-action renderer, the `resume` refusal path and their tests all assert the one
/// string rather than three drifting copies.
pub const STOPPED_NOT_RESUMABLE_GUIDANCE: &str =
    "Resume: unavailable; stopped runs are not resumable. Start a new run instead.";

/// Whether `session_file` points at an on-disk file that currently exists (pi's
/// `hasExistingSessionFile`, `run-status.ts:42-44`).
fn session_file_exists(session_file: &Option<PathBuf>) -> bool {
    session_file.as_ref().is_some_and(|path| path.exists())
}

/// The `Resume:`/`Revive:` guidance line for a non-running run (pi `formatResumeGuidance`,
/// `run-status.ts:51-66`). Every cyrup [`StepStatus`] carries a known agent, so the "known child"
/// filter is trivially every step; the branch structure otherwise mirrors pi exactly:
/// single-child-with-transcript → whole-run `Revive`, else first child with a transcript →
/// `Revive child` with its index, else the unavailable notice.
///
/// G77 — `stopped` short-circuits BEFORE every other branch, exactly as upstream's own first line
/// does (`run-status.ts:51-52` @v0.43.0: `if (options.stopped) return "Resume: unavailable;
/// stopped runs are not resumable. Start a new run instead.";`). A stopped run's children may well
/// have persisted session files, so without this guard the function would happily hand the model a
/// `Revive:` line for a run [`super::control::resume`] is required to refuse
/// (`async-resume.ts:406`).
fn format_resume_guidance(run_id: &str, steps: &[StepStatus], stopped: bool) -> String {
    if stopped {
        return STOPPED_NOT_RESUMABLE_GUIDANCE.to_string();
    }
    let unavailable = "Resume: unavailable; no child session file was persisted.".to_string();
    if run_id.is_empty() || steps.is_empty() {
        return unavailable;
    }
    if steps.len() == 1
        && steps
            .first()
            .is_some_and(|step| session_file_exists(&step.session_file))
    {
        return format!(
            "Revive: subagent({{ action: \"resume\", id: \"{run_id}\", message: \"...\" }})"
        );
    }
    if let Some((index, _)) = steps
        .iter()
        .enumerate()
        .find(|(_, step)| session_file_exists(&step.session_file))
    {
        return format!(
            "Revive child: subagent({{ action: \"resume\", id: \"{run_id}\", index: {index}, \
             message: \"...\" }})"
        );
    }
    unavailable
}

// =================================================================================================
// ISO-8601 UTC timestamp formatting (pi renders `new Date(ms).toISOString()`)
// =================================================================================================

/// The crate's one ISO-8601 renderer, lifted to [`crate::time`] beside the clock it renders and
/// re-exported here so this module's callers keep their path.
pub(crate) use crate::time::format_iso8601_millis;

// =================================================================================================
// S6 — live workflow controls (`run-status.ts:609-613`, consumed at `:684-691`)
// =================================================================================================

/// One element of pi's `[...deps.state.foregroundControls.values()]` (`run-status.ts:610`),
/// reduced to the three facts S6 tests plus the one its rendered line interpolates.
///
/// `run_id` is a FIELD here because it is the `foreground_controls` MAP KEY on the cyrup side:
/// [`crate::extension::executor::notices::ForegroundControlEntry`] carries no run id of its own
/// (pi's `ForegroundRunControl.runId` is a field), which is the same reason
/// `extension/executor/workflow_steering.rs` carries a `control_run_id` beside its cloned entry.
///
/// `child_indexes` is `control.activeChildren.keys()`, ALREADY ascending: `active_children` is a
/// `BTreeMap` (chosen for exactly this), so pi's `.sort((left, right) => left - right)` at `:687`
/// is a no-op here and is deliberately NOT re-applied.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveWorkflowControlCandidate {
    /// The `foreground_controls` key this entry was registered under.
    pub run_id: String,
    /// pi `control.sessionId`.
    pub session_id: Option<SessionId>,
    /// pi `control.parentWorkflowRunId`.
    pub parent_workflow_run_id: Option<RunId>,
    /// pi `[...control.activeChildren.keys()]`.
    pub child_indexes: Vec<usize>,
}

impl LiveWorkflowControlCandidate {
    /// The admitted projection — the two fields `:688` interpolates, and nothing else.
    fn to_live(&self) -> LiveWorkflowControl {
        LiveWorkflowControl {
            run_id: self.run_id.clone(),
            child_indexes: self.child_indexes.clone(),
        }
    }
}

/// One element of pi's `liveWorkflowControls` (`run-status.ts:609-613`) — a foreground control
/// that survived all four terms of the S6 filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveWorkflowControl {
    /// The control's run id, interpolated into the `id:` of the rendered steer hint.
    pub run_id: String,
    /// Its active children, ascending; one rendered line each.
    pub child_indexes: Vec<usize>,
}

/// The subset of pi's `deps` that [`format_status`] reads — `deps.state?.currentSessionId`,
/// `deps.state?.workflowControllers`, `deps.state.foregroundControls`.
///
/// # Why the renderer is HANDED this rather than reading the registries itself
///
/// Both registries are `std::sync::Mutex`es
/// (`extension/executor/mod.rs:157,171`) and this crate's rule — stated at `mod.rs:168-169` and
/// again at `workflow_child_stops.rs:36` — is that no `.await` happens inside their critical
/// sections. [`format_status`] is `async`. So the projection is built by the executor, which owns
/// the locks, and arrives here already materialised and lock-free.
///
/// # Why `live_workflow_run_ids` is a SET and not the bool `deps.state?.workflowControllers?.has(status.runId)`
///
/// Upstream evaluates `has(runId)` at the call site because it already holds the status. cyrup's
/// entry point is [`inspect_status_by_id`], which RESOLVES the selector (an id, or a unique
/// directory-name prefix) to a [`RunId`] inside this module — so at the moment the executor builds
/// these deps there is no run id yet to ask about. A snapshot of
/// [`crate::extension::executor::workflow_controllers`]' keys answers the same question at the
/// same instant, and `contains` at the seam below IS `has`.
///
/// `Default::default()` is upstream's `deps.state === undefined`: every optional chain
/// short-circuits, so `liveWorkflowControls` is `[]` (`:613`) and a running workflow renders the
/// `Steer: unavailable;` sentence.
#[derive(Clone, Debug, Default)]
pub struct RunStatusRenderDeps {
    /// pi `deps.state?.currentSessionId`.
    pub current_session: Option<SessionId>,
    /// pi `deps.state?.workflowControllers`' key set — see this type's own doc.
    pub live_workflow_run_ids: std::collections::HashSet<RunId>,
    /// pi `[...deps.state.foregroundControls.values()]`, unfiltered. Both session comparisons
    /// happen HERE, in [`Self::live_workflow_controls`], so they stay in one expression.
    pub foreground_controls: Vec<LiveWorkflowControlCandidate>,
}

impl RunStatusRenderDeps {
    /// pi `run-status.ts:609-613`. FOUR terms; all four are required and none is redundant.
    ///
    /// ```ts
    /// const liveWorkflowControls =
    ///     status.mode === "workflow"
    ///     && deps.state?.currentSessionId === status.sessionId
    ///     && deps.state?.workflowControllers?.has(status.runId)
    ///         ? [...deps.state.foregroundControls.values()].filter((control) =>
    ///               control.parentWorkflowRunId === status.runId
    ///            && control.sessionId === status.sessionId
    ///            && (control.activeChildren?.size ?? 0) > 0)
    ///         : [];
    /// ```
    ///
    /// # ⚠ The two session comparisons are `Option` EQUALITY, and NOT [`crate::background::delivery::SessionGate`]
    ///
    /// Reaching for `SessionGate` here compiles and passes a happy-path test, and is wrong. Both
    /// gate classes disagree with upstream's `===` in exactly one row, and in DIFFERENT rows:
    ///
    /// | `current` | `record` | pi `a === b` | `Strict.admits` | `Permissive.admits` |
    /// |---|---|---|---|---|
    /// | `None` | `None` | **true** | false | true |
    /// | `None` | `Some(x)` | **false** | false | true |
    /// | `Some(x)` | `None` | false | false | false |
    /// | `Some(x)` | `Some(x)` | true | true | true |
    /// | `Some(x)` | `Some(y)` | false | false | false |
    ///
    /// The row that bites is `None`/`None`: a headless or SDK-embedded orchestrator with no
    /// session identity, running a workflow whose `status.json` records no session. Upstream
    /// renders its steer hints; `SessionGate::Strict` would suppress them and the run would
    /// report `Steer: unavailable; …` for its whole life even though the route is right there.
    /// `SessionGate::Permissive` fails the other way, admitting a control from a session this
    /// status does not name.
    ///
    /// The sibling gate at `run-status.ts:249` IS `SessionGate::Strict` — see
    /// [`crate::extension::executor`]'s live-foreground transcript refusal. These are different
    /// gates with different classes and the difference is upstream's, not cyrup's.
    ///
    /// # The two comparisons are against two DIFFERENT objects
    ///
    /// Gate 1 is the STATE's current session vs the status's; gate 2 is each CONTROL's session vs
    /// the status's. They are not redundant: a control can carry a stale session after a rotation,
    /// and collapsing them would let it through.
    ///
    /// # Ordering
    ///
    /// Upstream's argument that the registry check happens "before any disk read"
    /// (`workflow_controllers.rs:133-136`) does NOT transfer: cyrup reaches [`format_status`]
    /// only after reconciliation has already read the run, so the registry term here is an
    /// ordinary conjunct and is documented as one rather than as an ordering guarantee.
    #[must_use]
    pub fn live_workflow_controls(&self, status: &RunStatus) -> Vec<LiveWorkflowControl> {
        if status.mode != RunMode::Workflow
            // gate 1 — `deps.state?.currentSessionId === status.sessionId`.
            || self.current_session.as_ref() != status.session_id.as_ref()
            // `deps.state?.workflowControllers?.has(status.runId)`.
            || !self.live_workflow_run_ids.contains(&status.run_id)
        {
            return Vec::new(); // `:613`
        }
        self.foreground_controls
            .iter()
            .filter(|control| {
                // `:610`
                control.parent_workflow_run_id.as_ref() == Some(&status.run_id)
                    // `:611` — gate 2, `Option` equality again for the reason above.
                    && control.session_id.as_ref() == status.session_id.as_ref()
                    // `:612` — `(control.activeChildren?.size ?? 0) > 0`.
                    && !control.child_indexes.is_empty()
            })
            .map(LiveWorkflowControlCandidate::to_live)
            .collect()
    }
}

/// pi `run-status.ts:685` — the sentence a running workflow gets when no live foreground route
/// survived the S6 filter. Wire text; reproduced byte-for-byte.
const STEER_UNAVAILABLE_NOTICE: &str =
    "Steer: unavailable; no live foreground route is registered in the active session.";

// =================================================================================================
// Single-run status report (`inspectSubagentStatus`, run-status.ts:101-273)
// =================================================================================================

/// Renders one reconciled [`RunStatus`] as pi's full status report (`run-status.ts:202-243`) —
/// run identity/state/mode/progress, pending appends, timestamps, dir, the authoritative terminal
/// `Result:` reference (when its file exists), per-step lines, S6's live-workflow steer hints,
/// resume guidance for a non-running run, and the `Log`/`Events` artifact references (when they
/// exist).
///
/// `deps` is pi's `deps` (`run-status.ts:609-613`), already materialised out of the executor's
/// live registries — see [`RunStatusRenderDeps`] for why this renderer is handed it rather than
/// reading them. [`RunStatusRenderDeps::default`] is upstream's `deps.state === undefined`.
async fn format_status(status: &RunStatus, paths: &RunPaths, deps: &RunStatusRenderDeps) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Run: {}", status.run_id));
    // pi `run-status.ts:373-385` @v0.43.0: the DURABLE MISSION this run is bound to, read back
    // from the `mission.json` binding its own async dir carries, rendered immediately after
    // `Run:` and omitted entirely when the run has no mission. An unreadable/invalid binding is a
    // WARNING appended to the report (`nestedWarning`), never a failure of the status report
    // itself.
    match crate::missions::read_mission_binding(&paths.run_dir) {
        Ok(Some(binding)) => lines.push(format!("Mission: {}", binding.mission_id)),
        Ok(None) => {}
        Err(e) => lines.push(format!("Warning: Mission binding unavailable: {e}")),
    }
    lines.push(format!("State: {}", run_state_label(status.state)));
    // pi `status.error ? \`Error: ${status.error}\` : undefined` (`run-status.ts:581`) — the
    // RUN-level error (`RunStatus::error`, distinct from a per-step `StepStatus::error`), placed
    // between `State:` and `Mode:` exactly as upstream's own neighbouring lines are (WORKFLOW_3
    // §3b).
    if let Some(error) = status.error.as_deref() {
        lines.push(format!("Error: {error}"));
    }
    lines.push(format!("Mode: {}", run_mode_label(status.mode)));
    lines.push(format!("Progress: {}", progress_label(status)));
    if let Some(pending) = status.pending_appends
        && pending > 0
    {
        lines.push(format!("Pending appends: {pending}"));
    }
    lines.push(format!(
        "Started: {}",
        format_iso8601_millis(status.started_at)
    ));
    lines.push(format!(
        "Updated: {}",
        format_iso8601_millis(status.last_update)
    ));
    lines.push(format!("Dir: {}", paths.run_dir.display()));
    // Resolved rather than probed at a fixed path: a promoted payload lives under
    // `result-owned/<enc(session)>/`, so testing the legacy root would report "no result" for
    // every run this build completes.
    if let Some(session_id) = status.session_id.as_ref()
        && let Some(result_path) = paths.resolve_result(session_id, &status.run_id).await
    {
        lines.push(format!("Result: {}", result_path.display()));
    } else if paths.legacy_result_root.exists() {
        lines.push(format!("Result: {}", paths.legacy_result_root.display()));
    }

    for (index, step) in status.steps.iter().enumerate() {
        let model_text = step
            .model
            .as_ref()
            .map(|model| format!(" ({})", model.as_str()))
            .unwrap_or_default();
        let error_text = step
            .error
            .as_ref()
            .map(|error| format!(", error: {error}"))
            .unwrap_or_default();
        // G90: pi's `steeringSuffix` (`run-status.ts:413,419` @v0.43.0), between the activity text
        // and the error text. This is what makes an `action: "steer"` VISIBLE: without it the tool
        // would report "Steering queued" and the status report would look identical whether the
        // runner accepted the request or dropped it.
        let steering_text =
            format_steering_summary(step.telemetry.steer_count, step.telemetry.last_steer_at);
        let steering_suffix = steering_text
            .map(|text| format!(", steering: {text}"))
            .unwrap_or_default();
        lines.push(format!(
            "{}: {} {}{}{}{}",
            step_line_label(status, index),
            step.agent,
            step_state_label(step.status),
            model_text,
            steering_suffix,
            error_text
        ));
        // SUBA-3c — pi `run-status.ts:630`: a step whose child was killed by its deadline with
        // unreviewed tracked changes shows the two recovery lines under its step line (indent
        // `"  "`). The status step carries the FULL summary; only the narrowed projection is
        // rendered — an ordinary timeout (no `recoveryNeeded`) stays silent by the renderer's own
        // early return.
        let recovery_projection = step
            .timeout_recovery
            .as_ref()
            .map(crate::exec::mutation_evidence::TimeoutRecoverySummary::project);
        lines.extend(
            crate::exec::mutation_evidence::format_timeout_recovery_lines(
                recovery_projection.as_ref(),
                "  ",
            ),
        );
        let step_log = paths.step_output_log(index);
        if step_log.exists() {
            lines.push(format!("  Output: {}", step_log.display()));
        }
    }

    // S6 — pi `run-status.ts:684-691`, the CONSUMER of `:609-613`. Positioned by upstream's own
    // relative order, which is the transferable anchor (the same argument the `Workflow receipt:`
    // line below already makes): after the per-step lines and immediately before it.
    //
    // Both strings are wire text an operator copies out of the report and pastes back as a tool
    // call, so both are byte-for-byte upstream's.
    if status.mode == RunMode::Workflow && status.state == RunState::Running {
        let live_workflow_controls = deps.live_workflow_controls(status);
        if live_workflow_controls.is_empty() {
            lines.push(STEER_UNAVAILABLE_NOTICE.to_string()); // `:685`
        } else {
            for control in &live_workflow_controls {
                // `:687` — ALREADY ascending (`LiveWorkflowControlCandidate::child_indexes`), so
                // upstream's numeric re-sort is a no-op and is not repeated.
                for index in &control.child_indexes {
                    lines.push(format!(
                        "Steer live foreground child: subagent({{ action: \"steer\", id: \"{}\", \
                         index: {index}, message: \"...\" }})",
                        control.run_id
                    )); // `:688`
                }
            }
        }
    }

    // pi `if (status.workflowReceiptPath) lines.push(\`Workflow receipt: ${...}\`)`
    // (`run-status.ts:697`) — immediately before `Session:` upstream, which cyrup's own report has
    // no equivalent line for; the transferable anchor is upstream's OWN relative order —
    // immediately before the resume-guidance line that follows (WORKFLOW_3 §3b).
    if let Some(path) = status.workflow_receipt_path.as_ref() {
        lines.push(format!("Workflow receipt: {}", path.display()));
    }

    if status.state != RunState::Running {
        lines.push(format_resume_guidance(
            status.run_id.as_str(),
            &status.steps,
            // G77 — pi `formatResumeGuidance(…, { stopped: status.state === "stopped" ||
            // status.stopped === true })` (`run-status.ts:445`). cyrup carries the stop verdict on
            // `state` alone (no redundant `stopped` boolean on `RunStatus`), so this is the whole
            // predicate.
            status.state == RunState::Stopped,
        ));
    }
    if paths.run_log_md.exists() {
        lines.push(format!("Log: {}", paths.run_log_md.display()));
    }
    if paths.events.exists() {
        lines.push(format!("Events: {}", paths.events.display()));
    }

    lines.join("\n")
}

/// Runs the R-SA-079 reconciliation gate against `paths` and renders the resulting status, or
/// returns `Ok(None)` for a run that has neither a `status.json` nor a terminal [`super::ResultFile`]
/// (pi's `"Async run not found. Provide id or dir."` case, `run-status.ts:154-160`).
///
/// # Errors
///
/// Propagates a genuine I/O/parse failure from the reconciliation read (anything other than the
/// "neither file exists" not-found case).
async fn inspect_paths(
    paths: &RunPaths,
    deps: &RunStatusRenderDeps,
) -> Result<Option<String>, SubagentError> {
    match reconcile_before_control_op(paths).await {
        // SUBA-057 — pi `run-status.ts:332-345`: a dismissed run answers with the display-dismissed
        // report instead of the ordinary one. `reconcile_before_control_op` has already erased the
        // marker if an authoritative `ResultFile` arrived, so reaching here means the dismissal is
        // still the truest thing known about this run.
        Ok(status) => Ok(Some(match status.display_dismissed_at {
            Some(dismissed_at) => {
                format_display_dismissed_status(status.run_id.as_str(), dismissed_at)
            }
            None => format_status(&status, paths, deps).await,
        })),
        Err(SubagentError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// SUBA-057 — pi's display-dismissed single-run report (`runs/background/run-status.ts:342-345`
/// @v0.47.1), reached upstream when reconciliation returns a `null` status but the on-disk record
/// still carries `displayDismissedAt`.
///
/// Rendered by `{action:"status", id}` for a run the operator dismissed, in place of the ordinary
/// [`format_status`] report. It exists so an id-addressed lookup of a dismissed run is answered
/// honestly — "you dismissed this, and nothing was killed" — rather than with either a stale
/// `running` report or a bare "not found", both of which would be lies about the same record.
///
/// The timestamp goes through this module's own [`format_iso8601_millis`] — the same millisecond-
/// precision UTC renderer every other cyrup-written timestamp uses — matching upstream's
/// `new Date(displayDismissedAt).toISOString()`.
#[must_use]
pub fn format_display_dismissed_status(run_id: &str, dismissed_at_epoch_millis: i64) -> String {
    let dismissed = format_iso8601_millis(dismissed_at_epoch_millis);
    format!(
        "Run: {run_id}\nState: display-dismissed\nDismissed: {dismissed}\nNo running work was \
         terminated."
    )
}

/// Resolve a run-id selector (exact id, or a unique dir-name prefix) against `async_root`/
/// `results_dir` — pi's `resolveSubagentRunId` for the single async namespace this crate owns
/// (`run-status.ts:130-140`). Returns the resolved [`RunId`], or `None` if nothing matches.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `selector` fails the R-SA-087 safe-token gate, or
/// [`SubagentError::AmbiguousRunId`] if a prefix matches more than one run directory.
pub(crate) async fn resolve_run_id(
    async_root: &Path,
    results_dir: &Path,
    selector: &str,
) -> Result<Option<RunId>, SubagentError> {
    validate_safe_token(selector)?;

    // pi `resolveTargetedAsyncRun` (`async-status.ts:235`) rejects the reserved index directories
    // outright — `.terminal-runs` is a sibling of the run dirs inside the async root, so without
    // this a targeted selector could "resolve" to the index itself.
    if crate::background::terminal_run_index::is_reserved_async_root_entry(selector) {
        return Ok(None);
    }

    // Exact match first: a run directory, its status.json, or its terminal result file already
    // exists under this exact id.
    let exact = RunPaths::for_run(
        async_root,
        results_dir,
        &RunId::from_token(selector.to_string()),
    );
    if path_exists(&exact.run_dir).await
        || path_exists(&exact.status).await
        || path_exists(&exact.legacy_result_root).await
    {
        return Ok(Some(RunId::from_token(selector.to_string())));
    }

    // Prefix fallback (R-SA moderate: run-id prefix resolution) — scan the async root for run
    // directories whose name starts with `selector`. A single match resolves; more than one is
    // ambiguous; none falls through to `None`.
    let mut matches: Vec<String> = Vec::new();
    if let Ok(mut entries) = tokio::fs::read_dir(async_root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            let Ok(file_type) = entry.file_type().await else {
                continue;
            };
            if !file_type.is_dir() {
                continue;
            }
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with(selector)
                // The reserved index dirs are not runs and must not count as (or toward an
                // ambiguous set of) prefix matches (pi `async-status.ts:235`'s reject, applied to
                // every prefix candidate at `:508`).
                && !crate::background::terminal_run_index::is_reserved_async_root_entry(name)
            {
                matches.push(name.to_string());
            }
        }
    }

    match matches.as_slice() {
        [] => Ok(None),
        [single] => Ok(Some(RunId::from_token(single.clone()))),
        _ => Err(SubagentError::AmbiguousRunId(format!(
            "{} runs match id prefix {selector:?}; provide the full run id",
            matches.len()
        ))),
    }
}

/// Non-erroring existence probe (a read-only lookup treats an I/O error the same as "absent").
async fn path_exists(path: &Path) -> bool {
    tokio::fs::try_exists(path).await.unwrap_or(false)
}

/// The `status` action's by-id form (pi `inspectSubagentStatus` with an `id`/`runId`,
/// `run-status.ts:128-243`): resolve the selector, reconcile, and render the full per-step report.
/// Returns `Ok(None)` for an unresolved/absent run (the caller renders the not-found notice).
///
/// `deps` is upstream's second argument, threaded through to [`format_status`] for S6's live
/// workflow controls; [`RunStatusRenderDeps::default`] is upstream's `deps.state === undefined`
/// and renders the report with no steer hints.
///
/// # Errors
///
/// Propagates safe-token/ambiguity resolution errors and genuine reconciliation I/O failures.
pub async fn inspect_status_by_id(
    async_root: &Path,
    results_dir: &Path,
    selector: &str,
    deps: &RunStatusRenderDeps,
) -> Result<Option<String>, SubagentError> {
    let Some(run_id) = resolve_run_id(async_root, results_dir, selector).await? else {
        return Ok(None);
    };
    let paths = RunPaths::for_run(async_root, results_dir, &run_id);
    inspect_paths(&paths, deps).await
}

/// G92: the same id resolution + R-SA-079 reconciliation [`inspect_status_by_id`] performs, but
/// handing back the reconciled [`RunStatus`] and its [`RunPaths`] instead of a rendered report — so
/// `view: "transcript"` can render a DIFFERENT view over the identical, identically-gated snapshot
/// rather than re-reading `status.json` behind the reconciliation.
///
/// # Errors
///
/// Propagates safe-token/ambiguity resolution errors and genuine reconciliation I/O failures.
pub async fn reconcile_by_id(
    async_root: &Path,
    results_dir: &Path,
    selector: &str,
) -> Result<Option<(RunStatus, RunPaths)>, SubagentError> {
    let Some(run_id) = resolve_run_id(async_root, results_dir, selector).await? else {
        return Ok(None);
    };
    reconcile_paths(RunPaths::for_run(async_root, results_dir, &run_id)).await
}

/// G92: [`reconcile_by_id`]'s `dir`-addressed twin, mirroring [`inspect_status_by_dir`].
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `async_dir` has no usable basename, or propagates
/// a genuine reconciliation I/O failure.
pub async fn reconcile_by_dir(
    async_dir: &Path,
    results_dir: &Path,
) -> Result<Option<(RunStatus, RunPaths)>, SubagentError> {
    let run_id = async_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| RunId::from_token(name.to_string()))
        .ok_or_else(|| {
            SubagentError::UnsafePathToken(format!(
                "async dir has no run-id basename: {}",
                async_dir.display()
            ))
        })?;
    let async_root = async_dir.parent().unwrap_or(async_dir);
    reconcile_paths(RunPaths::for_run(async_root, results_dir, &run_id)).await
}

/// [`inspect_paths`]'s snapshot-returning twin: same reconciliation gate, same "neither file
/// exists" → `Ok(None)` mapping, no rendering.
async fn reconcile_paths(paths: RunPaths) -> Result<Option<(RunStatus, RunPaths)>, SubagentError> {
    match reconcile_before_control_op(&paths).await {
        Ok(status) => Ok(Some((status, paths))),
        Err(SubagentError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The `status` action's by-`dir` form (pi `resolveAsyncRunLocation`, `run-status.ts:142`): render
/// the run whose async directory is `async_dir` directly, deriving its sibling result file from
/// `results_dir` and the directory's own basename. Returns `Ok(None)` when neither status nor
/// result exists there.
///
/// `deps` is [`inspect_status_by_id`]'s, for the same reason and with the same default.
///
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `async_dir` has no usable basename, or propagates
/// a genuine reconciliation I/O failure.
pub async fn inspect_status_by_dir(
    async_dir: &Path,
    results_dir: &Path,
    deps: &RunStatusRenderDeps,
) -> Result<Option<String>, SubagentError> {
    let run_id = async_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| RunId::from_token(name.to_string()))
        .ok_or_else(|| {
            SubagentError::UnsafePathToken(format!(
                "async dir has no run-id basename: {}",
                async_dir.display()
            ))
        })?;
    let async_root = async_dir.parent().unwrap_or(async_dir);
    let paths = RunPaths::for_run(async_root, results_dir, &run_id);
    inspect_paths(&paths, deps).await
}

// =================================================================================================
// No-id "list active runs" (`listAsyncRuns`/`formatAsyncRunList`, async-status.ts:366-537)
// =================================================================================================

/// One active-run summary row for the no-id list: its own run directory plus the reconciled status.
#[derive(Clone, Debug)]
pub struct ActiveRun {
    /// The run's own directory (`async_root/<run_id>`), rendered as the `dir` field.
    pub dir: PathBuf,
    /// The reconciled status snapshot.
    pub status: RunStatus,
}

/// State-ordering rank for the active-run list (pi `sortRuns`, `async-status.ts:345-364`, restricted
/// to the queued/running states this list surfaces): running before queued.
fn list_rank(state: RunState) -> u8 {
    match state {
        RunState::Running => 0,
        RunState::Queued => 1,
        // G77: upstream's `sortRuns` gives `failed`, `stopped` and `paused` the SAME rank 2 and
        // `complete` rank 3 (`async-status.ts:346-354` @v0.43.0 — `case "stopped": return 2;`
        // sits between the `failed` and `paused` cases, all three returning 2). This list only ever
        // holds queued/running rows, so every terminal state collapses to one bucket here; the
        // `Stopped` arm is spelled out rather than swept into a catch-all so a future state cannot
        // silently inherit rank 2.
        RunState::Paused | RunState::Complete | RunState::Failed | RunState::Stopped => 2,
    }
}

/// Enumerate every currently-active (queued or running) background run under `async_root` (pi
/// `listAsyncRuns` with `states: ["queued", "running"]`, `async-status.ts:366-449`): scan the run
/// directories, reconcile each (R-SA-079), keep the queued/running ones, and sort running-first
/// then most-recently-updated first. A missing `async_root` (no runs ever spawned for this cwd) is
/// an empty list, not an error. A run whose status cannot be reconciled (e.g. a half-written
/// directory) is skipped rather than aborting the whole scan (graceful degradation, matching pi's
/// `if (!status) continue`).
///
/// # Session scoping (SUBA-031)
///
/// `session_id` is pi's `options.sessionId` and the filter is pi's literal one — `if
/// (options.sessionId && status.sessionId !== options.sessionId) continue;`
/// (`async-status.ts:432` @v0.43.0). Two consequences are upstream's, not incidental:
///
/// * `None` (a headless / unpersisted orchestrator with no live session identity) applies **no**
///   filter at all, exactly as pi's falsy `options.sessionId` does — the async root is then scoped
///   by cwd alone, which is the behaviour every reader had before this parameter existed.
/// * When a filter IS supplied, a run whose own [`RunStatus::session_id`] is `None` is **dropped**.
///   pi compares with `!==` against `undefined`, so an unattributed run is not "everyone's", it is
///   nobody's. That is why [`crate::background::runner_main::RunnerConfig::session_id`] carries the
///   launching session explicitly rather than leaving the runner to re-derive it from the
///   permission-system-published anchor, which is absent whenever that extension is not loaded.
///
/// # Where the candidates come from (pi `async-status.ts:506-516` @`v0.68.0`)
///
/// Upstream stopped enumerating the async root for this listing: with no `repairScan`, the
/// candidate set is `readActiveRunIndex(asyncDirRoot)`. cyrup reads it through
/// [`crate::background::active_run_index::read_live_active_run_ids`], which also performs
/// upstream's two in-read index repairs, and falls back to the directory scan when that reader
/// answers `None` (no index exists in this root) or an empty list. The fallback is
/// [`crate::tui::fleet::collect_fleet_history`]'s idiom for the TERMINAL index and it holds for
/// the same reason: the index accelerates a listing, it must never be the only way a run can be
/// found, and an async root written before the index existed must still list.
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] only if `async_root` exists but its directory listing itself
/// fails.
pub async fn list_active_runs(
    async_root: &Path,
    results_dir: &Path,
    session_id: Option<&str>,
) -> Result<Vec<ActiveRun>, SubagentError> {
    let mut runs: Vec<ActiveRun> = Vec::new();

    for run_id in active_run_candidates(async_root).await? {
        let paths = RunPaths::for_run(async_root, results_dir, &run_id);
        // Reconcile before summarizing (R-SA-079); a run that reconciles to a terminal/paused
        // state, or that cannot be read at all, is simply not an *active* run.
        let Ok(status) = reconcile_before_control_op(&paths).await else {
            continue;
        };
        // SUBA-057 — pi `if (status.displayDismissedAt !== undefined) { …; continue; }`
        // (`async-status.ts:455-458` @v0.47.1), applied in pi's own position: FIRST, ahead of both
        // the session filter and the state filter. This one `continue` is what actually makes a
        // dismissed run disappear from `/subagents-fleet` and from `{action:"status"}` with no id,
        // since both render from this list. It is display-only: nothing about the run's `state` or
        // its files is changed, and `reconcile_before_control_op` above has already erased the
        // marker if a real `ResultFile` arrived in the meantime, so a run that genuinely finished
        // after being dismissed is NOT hidden here.
        if status.display_dismissed_at.is_some() {
            continue;
        }
        // pi `if (options.sessionId && status.sessionId !== options.sessionId) continue;`
        // (`async-status.ts:432`), applied AFTER the state filter for the same reason pi applies it
        // there: both are cheap rejections that run before any further per-run work.
        if let Some(wanted) = session_id
            && status.session_id.as_ref().map(SessionId::as_str) != Some(wanted)
        {
            continue;
        }
        if matches!(status.state, RunState::Queued | RunState::Running) {
            runs.push(ActiveRun {
                dir: paths.run_dir.clone(),
                status,
            });
        }
    }

    runs.sort_by(|a, b| {
        list_rank(a.status.state)
            .cmp(&list_rank(b.status.state))
            .then_with(|| b.status.last_update.cmp(&a.status.last_update))
    });
    Ok(runs)
}

/// The run ids [`list_active_runs`] considers, index-first with the directory scan underneath.
///
/// [`crate::background::active_run_index::read_live_active_run_ids`] is the bounded read of
/// `<async_root>/.active-runs` (pi `async-status.ts:506-516`); it has already dropped every entry
/// whose run reads back non-active and repaired the index for those. Only a `None` (this root has
/// no index) or an empty answer falls through to `read_dir`, and the scan then reproduces pi's
/// `repairScan` arm (`:503-504`) verbatim, reserved index dirs skipped.
///
/// # Errors
///
/// The scan's own listing failure. A missing `async_root` is an empty list, not an error, and the
/// index read never fails — a listing that cannot read the index still has the scan.
async fn active_run_candidates(async_root: &Path) -> Result<Vec<RunId>, SubagentError> {
    if let Some(indexed) =
        crate::background::active_run_index::read_live_active_run_ids(async_root).await
        && !indexed.is_empty()
    {
        return Ok(indexed);
    }

    let mut candidates: Vec<RunId> = Vec::new();
    let mut entries = match tokio::fs::read_dir(async_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(candidates),
        Err(e) => return Err(SubagentError::Spawn(e)),
    };
    while let Some(entry) = entries.next_entry().await.map_err(SubagentError::Spawn)? {
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        // pi `entry !== ACTIVE_RUN_INDEX_DIR && entry !== TERMINAL_RUN_INDEX_DIR`
        // (`async-status.ts:504`): the reserved index dirs live inside the async root but are not
        // runs — without this, every listing reconciles `.terminal-runs` as a status-less run.
        if crate::background::terminal_run_index::is_reserved_async_root_entry(&name) {
            continue;
        }
        candidates.push(RunId::from_token(name));
    }
    Ok(candidates)
}

/// Render the active-run list (pi `formatAsyncRunList`, `async-status.ts:516-537`): a
/// `No active async runs.` sentinel when empty, otherwise a `Active async runs: N` heading and one
/// `- id | state | mode | progress[ | K pending appends] | dir` header per run followed by its
/// `  n. agent | status[ | model]` step lines.
#[must_use]
pub fn format_run_list(runs: &[ActiveRun]) -> String {
    if runs.is_empty() {
        return "No active async runs.".to_string();
    }

    let mut lines: Vec<String> = vec![format!("Active async runs: {}", runs.len()), String::new()];
    for run in runs {
        let status = &run.status;
        let pending = status
            .pending_appends
            .filter(|count| *count > 0)
            .map(|count| {
                format!(
                    " | {count} pending append{}",
                    if count == 1 { "" } else { "s" }
                )
            })
            .unwrap_or_default();
        lines.push(format!(
            "- {} | {} | {} | {}{} | {}",
            status.run_id,
            run_state_label(status.state),
            run_mode_label(status.mode),
            progress_label(status),
            pending,
            run.dir.display()
        ));
        for (index, step) in status.steps.iter().enumerate() {
            let model_text = step
                .model
                .as_ref()
                .map(|model| format!(" | {}", model.as_str()))
                .unwrap_or_default();
            lines.push(format!(
                "  {}. {} | {}{}",
                index.saturating_add(1),
                step.agent,
                step_state_label(step.status),
                model_text
            ));
        }
        lines.push(String::new());
    }

    // pi trims the trailing blank line (`.trimEnd()`).
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines.join("\n")
}

// =================================================================================================
// Tests
// =================================================================================================

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::atomic::write_atomic_json;
    use crate::background::control::{self, InterruptOutcome, ResumeOutcome};
    use crate::spawn::chain_graph::{RunnerStep, SingleStepSpec};

    /// G90: the runner's accepted-steer counters must SURFACE in the report a user reads
    /// (pi `steeringSuffix`, `run-status.ts:413,419` @v0.43.0). Without this the tool would say
    /// "Steering queued" and the status report would look identical whether the runner accepted
    /// the request or dropped it on the floor.
    #[tokio::test]
    async fn a_steps_accepted_steers_are_rendered_in_pis_steering_suffix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = RunId::from_token("steersuffix1".to_string());
        let paths = RunPaths::for_run(dir.path(), dir.path(), &id);
        let mut status = RunStatus::queued(id, RunMode::Single, Some(1));
        status.state = RunState::Running;
        let mut step = StepStatus::pending("scout");
        step.status = StepState::Running;
        step.telemetry.steer_count = Some(2);
        step.telemetry.last_steer_at = Some(1_700_000_000_000);
        status.steps = vec![step];

        let report = format_status(&status, &paths, &RunStatusRenderDeps::default()).await;
        assert!(
            report.contains(
                "Step 1: scout running, steering: 2 steers, last 2023-11-14T22:13:20.000Z"
            ),
            "{report}"
        );

        // Singular, and no suffix at all when nothing was ever steered.
        status.steps[0].telemetry.steer_count = Some(1);
        status.steps[0].telemetry.last_steer_at = None;
        assert!(
            format_status(&status, &paths, &RunStatusRenderDeps::default())
                .await
                .contains(", steering: 1 steer"),
            "singular form"
        );
        status.steps[0].telemetry.steer_count = None;
        let quiet = format_status(&status, &paths, &RunStatusRenderDeps::default()).await;
        assert!(
            !quiet.contains("steering:"),
            "an unsteered step gets no suffix: {quiet}"
        );
    }

    /// SUBA-3c — pi `run-status.ts:630`: a step killed by its deadline with unreviewed tracked
    /// changes renders the two recovery lines under its step line at indent `"  "`, projected
    /// from the FULL summary the status step carries; an ordinary timeout (no `recoveryNeeded`)
    /// renders nothing.
    #[tokio::test]
    async fn a_steps_timeout_recovery_renders_the_projected_lines_in_the_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let id = RunId::from_token("recoveryrender1".to_string());
        let paths = RunPaths::for_run(dir.path(), dir.path(), &id);
        let mut status = RunStatus::queued(id, RunMode::Single, Some(1));
        status.state = RunState::Failed;
        let mut step = StepStatus::pending("coder");
        step.status = StepState::Failed;
        let evidence = crate::exec::mutation_evidence::TrackedMutationEvidence {
            source: Default::default(),
            tracked_only: true,
            changed_files: vec!["src/half-written.rs".to_string()],
            attempted_mutation: true,
            truncated: false,
            unavailable: None,
        };
        step.timeout_recovery = Some(
            crate::exec::mutation_evidence::build_timeout_recovery_summary(
                crate::exec::mutation_evidence::TimeoutRecoveryInput {
                    termination: crate::exec::mutation_evidence::Termination::TimedOut,
                    evidence: &evidence,
                    required_output_missing: Some(true),
                    current_tool: None,
                    current_tool_args: None,
                    current_path: None,
                    session_file: None,
                    transcript_path: None,
                    artifact_paths: None,
                },
            ),
        );
        status.steps = vec![step];

        let report = format_status(&status, &paths, &RunStatusRenderDeps::default()).await;
        assert!(
            report.contains(
                "  Recovery needed: review the diff and artifacts before resuming or launching dependent stages."
            ),
            "{report}"
        );
        assert!(
            report.contains(
                "  Recovery evidence: requested report: missing; changed tracked files: src/half-written.rs (1); classification: timed-out-with-dirty-worktree"
            ),
            "{report}"
        );

        // An ordinary timeout — report written, or clean worktree — stays quiet (`:140`).
        let quiet_evidence = crate::exec::mutation_evidence::TrackedMutationEvidence {
            source: Default::default(),
            tracked_only: true,
            changed_files: Vec::new(),
            attempted_mutation: false,
            truncated: false,
            unavailable: None,
        };
        status.steps[0].timeout_recovery = Some(
            crate::exec::mutation_evidence::build_timeout_recovery_summary(
                crate::exec::mutation_evidence::TimeoutRecoveryInput {
                    termination: crate::exec::mutation_evidence::Termination::TimedOut,
                    evidence: &quiet_evidence,
                    required_output_missing: Some(true),
                    current_tool: None,
                    current_tool_args: None,
                    current_path: None,
                    session_file: None,
                    transcript_path: None,
                    artifact_paths: None,
                },
            ),
        );
        let quiet = format_status(&status, &paths, &RunStatusRenderDeps::default()).await;
        assert!(!quiet.contains("Recovery"), "{quiet}");
    }

    /// G77 — the `status` action's rendering of a stopped run: its own state word, and pi's
    /// verbatim not-resumable guidance INSTEAD of the `Revive:` line the same steps would
    /// otherwise earn. The step below deliberately carries an existing `session_file`, which is
    /// precisely the input that would produce a `Revive:` line without the stopped short-circuit.
    #[tokio::test]
    async fn a_stopped_run_renders_its_own_state_word_and_refuses_to_offer_a_revive() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("session.jsonl");
        std::fs::write(&transcript, b"{}\n").expect("write transcript");

        let id = RunId::from_token("stoprender01".to_string());
        let paths = RunPaths::for_run(dir.path(), dir.path(), &id);
        let mut status = RunStatus::queued(id, RunMode::Single, Some(1));
        status.state = RunState::Stopped;
        let mut step = StepStatus::pending("scout");
        step.status = StepState::Stopped;
        step.session_file = Some(transcript);
        step.error = Some(control::STOP_MESSAGE.to_string());
        status.steps = vec![step];

        let report = format_status(&status, &paths, &RunStatusRenderDeps::default()).await;
        assert!(report.contains("State: stopped"), "{report}");
        assert!(report.contains("Step 1: scout stopped"), "{report}");
        assert!(report.contains(STOPPED_NOT_RESUMABLE_GUIDANCE), "{report}");
        assert!(
            !report.contains("Revive:"),
            "a stopped run must never be offered a Revive line even though its step HAS a \
             transcript (pi `run-status.ts:51-52` short-circuits before the transcript checks): \
             {report}"
        );
    }

    /// G77 — the lowercase wire words every human-facing renderer prints. `stopped` is its own
    /// word on both enums, never `failed`.
    #[test]
    fn stopped_gets_its_own_state_and_step_label() {
        assert_eq!(run_state_label(RunState::Stopped), "stopped");
        assert_eq!(step_state_label(StepState::Stopped), "stopped");
        // The other five are unchanged.
        assert_eq!(run_state_label(RunState::Failed), "failed");
        assert_eq!(run_state_label(RunState::Paused), "paused");
        assert_eq!(step_state_label(StepState::Failed), "failed");
        // pi `sortRuns` (`async-status.ts:346-354`) ranks stopped with failed/paused at 2.
        assert_eq!(list_rank(RunState::Stopped), list_rank(RunState::Failed));
        assert_eq!(list_rank(RunState::Stopped), list_rank(RunState::Paused));
        assert!(list_rank(RunState::Running) < list_rank(RunState::Stopped));
    }

    fn roots() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().expect("real tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        (dir, async_root, results_dir)
    }

    async fn write_status(paths: &RunPaths, status: &RunStatus) {
        tokio::fs::create_dir_all(&paths.run_dir)
            .await
            .expect("mkdir run_dir");
        write_atomic_json(&paths.status, status)
            .await
            .expect("write status");
    }

    fn running_status(run_id: &RunId, mode: RunMode, steps: Vec<StepStatus>) -> RunStatus {
        let mut status = RunStatus::queued(run_id.clone(), mode, None);
        status
            .advance_state(RunState::Running)
            .expect("Queued -> Running");
        status.chain_step_count = Some(steps.len().max(1));
        status.current_step = if steps.is_empty() { None } else { Some(0) };
        status.steps = steps;
        status
    }

    fn single_step_spec(agent: &str) -> SingleStepSpec {
        SingleStepSpec {
            skills: None,
            session_dir: None,
            agent: agent.to_string(),
            task: format!("do {agent}"),
            cwd: None,
            model: None,
            tools: None,
            extensions: None,
            session_file: None,
            max_depth_override: None,
            structured_output_schema: None,
            output: None,
            output_path: None,
            output_mode: None,
            reads: None,
            acceptance: None,
            context: None,
            agent_scope: None,
        }
    }

    // ---------------------------------------------------------------------------------------
    // status, no id: lists active (queued/running) runs, excludes terminal ones
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn list_active_runs_lists_running_and_queued_excludes_terminal() {
        let (_dir, async_root, results_dir) = roots();

        // A running chain run.
        let running_id = RunId::from_token("run0running");
        let running_paths = RunPaths::for_run(&async_root, &results_dir, &running_id);
        let mut running_step = StepStatus::pending("researcher");
        running_step.status = StepState::Running;
        let running = running_status(&running_id, RunMode::Chain, vec![running_step]);
        write_status(&running_paths, &running).await;

        // A queued single run.
        let queued_id = RunId::from_token("run0queued0");
        let queued_paths = RunPaths::for_run(&async_root, &results_dir, &queued_id);
        let queued = RunStatus::queued(queued_id.clone(), RunMode::Single, None);
        write_status(&queued_paths, &queued).await;

        // A completed run — must NOT appear in the active list.
        let done_id = RunId::from_token("run0donexxx");
        let done_paths = RunPaths::for_run(&async_root, &results_dir, &done_id);
        let mut done = running_status(&done_id, RunMode::Single, vec![]);
        done.advance_state(RunState::Complete).expect("-> Complete");
        write_status(&done_paths, &done).await;

        let active = list_active_runs(&async_root, &results_dir, None)
            .await
            .expect("list active");
        let ids: Vec<&str> = active.iter().map(|r| r.status.run_id.as_str()).collect();
        assert!(ids.contains(&"run0running"), "running run must be listed");
        assert!(ids.contains(&"run0queued0"), "queued run must be listed");
        assert!(
            !ids.contains(&"run0donexxx"),
            "completed run must be excluded"
        );

        // running sorts before queued.
        assert_eq!(
            active.first().map(|r| r.status.run_id.as_str()),
            Some("run0running")
        );

        let rendered = format_run_list(&active);
        assert!(
            rendered.starts_with("Active async runs: 2"),
            "heading: {rendered}"
        );
        assert!(
            rendered.contains("run0running | running | chain"),
            "run line: {rendered}"
        );
        assert!(
            !rendered.contains("run0donexxx"),
            "terminal run absent: {rendered}"
        );
    }

    /// The ACTIVE-run index is what [`list_active_runs`] enumerates — not the async root — so the
    /// whole `wait`/`status`/`auto_drain`/control family reaches
    /// [`crate::background::active_run_index::read_live_active_run_ids`] through this one listing.
    ///
    /// Proved by the only asymmetry a scan cannot reproduce: BOTH runs are running on disk, and
    /// only one carries an index marker. A directory scan lists two; the index lists one.
    #[tokio::test]
    async fn list_active_runs_enumerates_the_active_run_index_not_the_directory() {
        let (_dir, async_root, results_dir) = roots();

        let indexed_id = RunId::from_token("run0indexed");
        let indexed_paths = RunPaths::for_run(&async_root, &results_dir, &indexed_id);
        let indexed = running_status(&indexed_id, RunMode::Single, vec![StepStatus::pending("a")]);
        write_status(&indexed_paths, &indexed).await;
        crate::background::active_run_index::update_active_run_index(
            &indexed_paths.run_dir,
            &indexed,
        )
        .await
        .expect("index the launch");

        // Running on disk, absent from the index — the shape a scan cannot tell from the above.
        let unindexed_id = RunId::from_token("run0scanonly");
        let unindexed_paths = RunPaths::for_run(&async_root, &results_dir, &unindexed_id);
        let unindexed = running_status(
            &unindexed_id,
            RunMode::Single,
            vec![StepStatus::pending("a")],
        );
        write_status(&unindexed_paths, &unindexed).await;

        let ids: Vec<String> = list_active_runs(&async_root, &results_dir, None)
            .await
            .expect("list active")
            .iter()
            .map(|run| run.status.run_id.as_str().to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["run0indexed"],
            "the listing is the index's, not the directory's"
        );
    }

    /// pi `async-status.ts:514`/`:570` — an index entry whose run has no readable `status.json` is
    /// a phantom, and the listing that trips over it calls `updateActiveRunIndex(asyncDir,
    /// "failed")` ([`crate::background::active_run_index::mark_active_run_failed`]) to release it.
    ///
    /// Reached here only through `list_active_runs`: the marker is planted by hand, the production
    /// listing is the single call, and the marker is gone afterwards.
    #[tokio::test]
    async fn a_phantom_active_marker_is_released_by_the_listing_that_trips_over_it() {
        let (_dir, async_root, results_dir) = roots();

        let real_id = RunId::from_token("run0realrun");
        let real_paths = RunPaths::for_run(&async_root, &results_dir, &real_id);
        let real = running_status(&real_id, RunMode::Single, vec![StepStatus::pending("a")]);
        write_status(&real_paths, &real).await;
        crate::background::active_run_index::update_active_run_index(&real_paths.run_dir, &real)
            .await
            .expect("index the launch");

        // A marker with no run behind it at all — the residue of a directory removed underneath
        // the index.
        let phantom = async_root
            .join(crate::background::active_run_index::ACTIVE_RUN_INDEX_DIR)
            .join("run0phantom0");
        tokio::fs::write(&phantom, b"").await.expect("plant marker");

        let ids: Vec<String> = list_active_runs(&async_root, &results_dir, None)
            .await
            .expect("list active")
            .iter()
            .map(|run| run.status.run_id.as_str().to_string())
            .collect();
        assert_eq!(ids, vec!["run0realrun"], "the phantom is not a run");
        assert!(
            !phantom.exists(),
            "the listing released the phantom's marker"
        );
        assert!(
            real_paths.run_dir.parent().is_some_and(|root| root
                .join(crate::background::active_run_index::ACTIVE_RUN_INDEX_DIR)
                .join("run0realrun")
                .is_file()),
            "the live run's own marker is untouched"
        );
    }

    #[tokio::test]
    async fn format_run_list_empty_is_the_no_active_sentinel() {
        let (_dir, async_root, results_dir) = roots();
        let active = list_active_runs(&async_root, &results_dir, None)
            .await
            .expect("list active over a missing async root is empty, not an error");
        assert!(active.is_empty());
        assert_eq!(format_run_list(&active), "No active async runs.");
    }

    /// SUBA-031 — the session filter is pi's `if (options.sessionId && status.sessionId !==
    /// options.sessionId) continue;` (`async-status.ts:432` @v0.43.0), all three of its arms.
    ///
    /// The `None` leg is asserted FIRST and deliberately: it establishes that all three runs are on
    /// disk, active, and reachable, so the two filtered legs below are proving an exclusion rather
    /// than passing vacuously on an empty root.
    #[tokio::test]
    async fn list_active_runs_scopes_to_the_requested_session() {
        let (_dir, async_root, results_dir) = roots();

        for (token, session) in [
            ("run0sessa00", Some("session-a")),
            ("run0sessb00", Some("session-b")),
            ("run0noneses", None),
        ] {
            let id = RunId::from_token(token);
            let paths = RunPaths::for_run(&async_root, &results_dir, &id);
            let mut status = running_status(&id, RunMode::Single, vec![StepStatus::pending("a")]);
            status.session_id = SessionId::parse_opt(session);
            write_status(&paths, &status).await;
        }

        async fn ids(async_root: &Path, results_dir: &Path, session: Option<&str>) -> Vec<String> {
            let mut ids: Vec<String> = list_active_runs(async_root, results_dir, session)
                .await
                .expect("list active")
                .iter()
                .map(|r| r.status.run_id.as_str().to_string())
                .collect();
            ids.sort();
            ids
        }

        // No filter (pi's falsy `options.sessionId`): every active run, whatever it recorded.
        assert_eq!(
            ids(&async_root, &results_dir, None).await,
            vec!["run0noneses", "run0sessa00", "run0sessb00"],
            "an unfiltered listing must still surface all three runs"
        );

        // A filter drops the OTHER session's run — the SUBA-031 defect itself: two cyrup sessions
        // in the same repo used to see (and `wait` on) each other's background runs.
        //
        // It also drops the UNATTRIBUTED run, which is pi's `!==` against `undefined` and not an
        // accident: a run nobody claimed is not everybody's.
        assert_eq!(
            ids(&async_root, &results_dir, Some("session-a")).await,
            vec!["run0sessa00"]
        );
        assert_eq!(
            ids(&async_root, &results_dir, Some("session-b")).await,
            vec!["run0sessb00"]
        );
        assert!(
            ids(&async_root, &results_dir, Some("session-c"))
                .await
                .is_empty(),
            "a session that launched nothing sees nothing"
        );
    }

    // ---------------------------------------------------------------------------------------
    // status by id: full per-step progress
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn inspect_status_by_id_renders_full_per_step_progress() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("run0chain00");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let mut step0 = StepStatus::pending("researcher");
        step0.status = StepState::Running;
        step0.model = Some(cyrup_core::ModelId::from("claude-sonnet"));
        let step1 = StepStatus::pending("writer"); // Pending
        let status = running_status(&run_id, RunMode::Chain, vec![step0, step1]);
        write_status(&paths, &status).await;

        let report = inspect_status_by_id(
            &async_root,
            &results_dir,
            "run0chain00",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("inspect ok")
        .expect("run found");

        assert!(report.contains("Run: run0chain00"), "{report}");
        assert!(report.contains("State: running"), "{report}");
        assert!(report.contains("Mode: chain"), "{report}");
        assert!(report.contains("Progress: step 1/2"), "{report}");
        assert!(
            report.contains("Step 1/2: researcher running (claude-sonnet)"),
            "per-step line with model: {report}"
        );
        assert!(report.contains("Step 2/2: writer pending"), "{report}");
    }

    #[tokio::test]
    async fn inspect_status_by_id_prefix_resolves_a_unique_run() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("abcdef123456");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let status = running_status(
            &run_id,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        );
        write_status(&paths, &status).await;

        let report = inspect_status_by_id(
            &async_root,
            &results_dir,
            "abcdef",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("inspect ok")
        .expect("prefix resolves");
        assert!(report.contains("Run: abcdef123456"), "{report}");
    }

    /// [`resolve_run_id`]'s two ERROR arms, reached through the production entry point rather
    /// than by calling the resolver: the R-SA-087 safe-token gate, and pi's ambiguity refusal
    /// (`async-status.ts:508`). Both were uncovered — only the resolver's resolving paths had
    /// tests, so a regression that silently picked the first prefix match, or that dropped the
    /// token gate, would have gone unnoticed here.
    #[tokio::test]
    async fn inspect_status_by_id_refuses_an_unsafe_token_and_an_ambiguous_prefix() {
        let (_dir, async_root, results_dir) = roots();
        for token in ["run0ambig001", "run0ambig002"] {
            let id = RunId::from_token(token);
            let paths = RunPaths::for_run(&async_root, &results_dir, &id);
            let status = running_status(&id, RunMode::Single, vec![StepStatus::pending("a")]);
            write_status(&paths, &status).await;
        }

        let unsafe_selector = inspect_status_by_id(
            &async_root,
            &results_dir,
            "../etc/passwd",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect_err("a selector that escapes the async root is refused before any read");
        assert!(
            matches!(unsafe_selector, SubagentError::UnsafePathToken(_)),
            "{unsafe_selector:?}"
        );

        let ambiguous = inspect_status_by_id(
            &async_root,
            &results_dir,
            "run0ambig",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect_err("a prefix matching two runs must refuse, never silently pick one");
        match ambiguous {
            SubagentError::AmbiguousRunId(message) => assert!(
                message.contains("2 runs match id prefix \"run0ambig\""),
                "{message}"
            ),
            other => panic!("expected AmbiguousRunId, got {other:?}"),
        }

        // The control: one more character resolves the same selector, so the refusal above is
        // about ambiguity and not about the prefix being unresolvable in this root.
        let report = inspect_status_by_id(
            &async_root,
            &results_dir,
            "run0ambig001",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("an exact id resolves")
        .expect("the run is found");
        assert!(report.contains("Run: run0ambig001"), "{report}");
    }

    /// SUBA-057 — the dismissed-run pair, both halves of which live in this module and neither of
    /// which had a test: an id-addressed lookup answers with
    /// [`format_display_dismissed_status`] instead of a stale `running` report, and the no-id
    /// listing (`list_active_runs`' own `continue`) drops the run entirely.
    #[tokio::test]
    async fn a_display_dismissed_run_is_reported_as_dismissed_and_never_listed_as_active() {
        let (_dir, async_root, results_dir) = roots();

        let dismissed_id = RunId::from_token("run0dismiss");
        let dismissed_paths = RunPaths::for_run(&async_root, &results_dir, &dismissed_id);
        let mut dismissed = running_status(
            &dismissed_id,
            RunMode::Single,
            vec![StepStatus::pending("scout")],
        );
        dismissed.display_dismissed_at = Some(1_700_000_000_000);
        write_status(&dismissed_paths, &dismissed).await;

        // A live neighbour, so the listing assertion below is an EXCLUSION rather than an empty
        // root passing vacuously.
        let live_id = RunId::from_token("run0liveone");
        let live_paths = RunPaths::for_run(&async_root, &results_dir, &live_id);
        let live = running_status(
            &live_id,
            RunMode::Single,
            vec![StepStatus::pending("scout")],
        );
        write_status(&live_paths, &live).await;

        let report = inspect_status_by_id(
            &async_root,
            &results_dir,
            "run0dismiss",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("inspect ok")
        .expect("a dismissed run is still addressable by id");
        assert_eq!(
            report,
            "Run: run0dismiss\nState: display-dismissed\nDismissed: \
             2023-11-14T22:13:20.000Z\nNo running work was terminated.",
            "the dismissal outranks the run's own recorded `running` state"
        );

        let ids: Vec<String> = list_active_runs(&async_root, &results_dir, None)
            .await
            .expect("list active")
            .iter()
            .map(|run| run.status.run_id.as_str().to_string())
            .collect();
        assert_eq!(
            ids,
            vec!["run0liveone"],
            "a dismissed run disappears from the fleet listing while its neighbour stays"
        );
    }

    #[tokio::test]
    async fn inspect_status_by_id_missing_run_is_none() {
        let (_dir, async_root, results_dir) = roots();
        let found = inspect_status_by_id(
            &async_root,
            &results_dir,
            "nosuchrun00",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("inspect ok");
        assert!(
            found.is_none(),
            "a missing run resolves to None (not-found notice)"
        );
    }

    // ---------------------------------------------------------------------------------------
    // interrupt yields a soft Paused (not a Failed/kill) — end to end
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn interrupt_then_runner_consume_yields_paused_status() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("run0intr000");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let mut running_step = StepStatus::pending("researcher");
        running_step.status = StepState::Running;
        // Give the step a persisted transcript so post-pause resume guidance is a real Revive line.
        let session_file = paths.run_dir.join("session.jsonl");
        running_step.session_file = Some(session_file.clone());
        let status = running_status(&run_id, RunMode::Single, vec![running_step]);
        write_status(&paths, &status).await;
        tokio::fs::write(&session_file, b"{}\n")
            .await
            .expect("write session");

        // Orchestrator side: deliver the interrupt.
        let outcome = control::interrupt(
            &async_root,
            &results_dir,
            "run0intr000",
            "interrupt-action",
            None,
            None,
        )
        .await
        .expect("interrupt delivered");
        assert_eq!(outcome, InterruptOutcome::Delivered);
        assert!(
            tokio::fs::try_exists(&paths.control_inbox)
                .await
                .expect("exists"),
            "the control-inbox interrupt request must be written"
        );

        // Runner side (simulated): consume the request and soft-pause — R-SA-084: Paused, not Failed.
        let consumed = control::consume_interrupt_request(&paths)
            .await
            .expect("consume ok");
        assert!(
            consumed.is_some(),
            "the pending interrupt must be consumable"
        );
        let mut paused = status;
        if let Some(step) = paused.steps.first_mut() {
            step.status = StepState::Paused;
        }
        paused
            .advance_state(RunState::Paused)
            .expect("Running -> Paused");
        write_status(&paths, &paused).await;

        // Observable outcome: the status report now reads paused, with a real revive guidance line.
        let report = inspect_status_by_id(
            &async_root,
            &results_dir,
            "run0intr000",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("inspect ok")
        .expect("found");
        assert!(
            report.contains("State: paused"),
            "soft-pause, not failed: {report}"
        );
        assert!(
            report.contains("researcher paused"),
            "the step is paused: {report}"
        );
        assert!(
            report.contains("Revive: subagent({ action: \"resume\", id: \"run0intr000\""),
            "a paused run with a transcript offers whole-run revive guidance: {report}"
        );
    }

    // ---------------------------------------------------------------------------------------
    // append-step enqueues onto a running chain (pending count surfaces in the status report)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn append_step_enqueues_onto_running_chain_and_surfaces_pending() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("run0appchn0");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);

        let mut running_step = StepStatus::pending("researcher");
        running_step.status = StepState::Running;
        let status = running_status(&run_id, RunMode::Chain, vec![running_step]);
        write_status(&paths, &status).await;

        let outcome = control::append_step(
            &async_root,
            &results_dir,
            "run0appchn0",
            vec![RunnerStep::SingleStep(single_step_spec("writer"))],
        )
        .await
        .expect("append enqueued onto a running chain");
        assert!(matches!(outcome, control::AppendOutcome::Enqueued { .. }));

        let pending = control::count_pending_appends(&paths.append_dir)
            .await
            .expect("count");
        assert_eq!(pending, 1, "exactly one append is now pending");

        // The pending count surfaces in the status report (append_step persists it into status.json).
        let report = inspect_status_by_id(
            &async_root,
            &results_dir,
            "run0appchn0",
            &RunStatusRenderDeps::default(),
        )
        .await
        .expect("inspect ok")
        .expect("found");
        assert!(report.contains("Pending appends: 1"), "{report}");
    }

    #[tokio::test]
    async fn append_step_rejects_a_non_chain_run() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("run0single0");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let status = running_status(
            &run_id,
            RunMode::Single,
            vec![StepStatus::pending("worker")],
        );
        write_status(&paths, &status).await;

        let err = control::append_step(
            &async_root,
            &results_dir,
            "run0single0",
            vec![RunnerStep::SingleStep(single_step_spec("writer"))],
        )
        .await
        .expect_err("append onto a single run is rejected");
        assert!(matches!(err, SubagentError::MalformedSettings(_)));
    }

    // ---------------------------------------------------------------------------------------
    // resume distinguishes running-selection (steer live) vs terminal-revival (respawn/hard-fail)
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn resume_running_run_selects_the_live_child_terminal_run_takes_revival_branch() {
        let (_dir, async_root, results_dir) = roots();

        // Running run: resume steers the single live child (no respawn).
        let running_id = RunId::from_token("run0live000");
        let running_paths = RunPaths::for_run(&async_root, &results_dir, &running_id);
        let mut running_step = StepStatus::pending("researcher");
        running_step.status = StepState::Running;
        let running = running_status(&running_id, RunMode::Single, vec![running_step]);
        write_status(&running_paths, &running).await;

        let steer = control::resume(&async_root, &results_dir, "run0live000", None, None)
            .await
            .expect("resume on a running run resolves");
        assert_eq!(
            steer,
            ResumeOutcome::SteerRunning { step_index: 0 },
            "a running run resolves to the live-steer branch, not revival"
        );

        // Terminal run WITHOUT a transcript: resume takes the revival branch and hard-fails
        // (R-SA-085: no silent fallback to a fresh session).
        let dead_id = RunId::from_token("run0dead000");
        let dead_paths = RunPaths::for_run(&async_root, &results_dir, &dead_id);
        let mut dead_step = StepStatus::pending("researcher");
        dead_step.status = StepState::Failed; // terminal step, no session_file
        let mut dead = running_status(&dead_id, RunMode::Single, vec![dead_step]);
        dead.advance_state(RunState::Failed).expect("-> Failed");
        write_status(&dead_paths, &dead).await;

        let revival = control::resume(&async_root, &results_dir, "run0dead000", None, None).await;
        assert!(
            matches!(revival, Err(SubagentError::ResumeNoTranscript)),
            "a terminal run with no transcript takes the revival branch and hard-fails: {revival:?}"
        );

        // Terminal run WITH a transcript: revival branch resolves the transcript to respawn from.
        let revive_id = RunId::from_token("run0revive0");
        let revive_paths = RunPaths::for_run(&async_root, &results_dir, &revive_id);
        let session_file = revive_paths.run_dir.join("session.jsonl");
        let mut revive_step = StepStatus::pending("researcher");
        revive_step.status = StepState::Complete;
        revive_step.session_file = Some(session_file.clone());
        let mut revive = running_status(&revive_id, RunMode::Single, vec![revive_step]);
        revive
            .advance_state(RunState::Complete)
            .expect("-> Complete");
        write_status(&revive_paths, &revive).await;

        let respawn = control::resume(&async_root, &results_dir, "run0revive0", None, None)
            .await
            .expect("resume on a terminal run with a transcript resolves");
        assert_eq!(
            respawn,
            ResumeOutcome::RespawnFromTranscript {
                step_index: 0,
                session_file,
            },
            "a terminal run with a transcript resolves to the respawn-from-transcript branch"
        );
    }

    // ---------------------------------------------------------------------------------------
    // SCOPE_10 S6 — live workflow controls (`run-status.ts:609-613`) and their consumer
    // (`:684-691`).
    //
    // Every test below fails on the pre-SCOPE_10 tree for the same reason: `format_status` took
    // no `deps` at all, so no steer line — neither the hints nor the `Steer: unavailable;`
    // sentence — could be rendered for any input.
    // ---------------------------------------------------------------------------------------

    /// A workflow run in `state`, owned by `session`. The walk goes through `Running` because
    /// `RunState`'s transition guard is monotone-forward and `Queued -> Complete` is not one of
    /// its edges (`background/state.rs:162-175`).
    fn workflow_status(run_id: &RunId, session: Option<&str>, state: RunState) -> RunStatus {
        let mut status = RunStatus::queued(run_id.clone(), RunMode::Workflow, None);
        if state != RunState::Queued {
            status
                .advance_state(RunState::Running)
                .expect("Queued -> Running");
            if state != RunState::Running {
                status
                    .advance_state(state)
                    .expect("Running -> the test state");
            }
        }
        status.session_id = SessionId::parse_opt(session);
        status
    }

    fn candidate(
        run_id: &str,
        session: Option<&str>,
        parent: Option<&RunId>,
        child_indexes: Vec<usize>,
    ) -> LiveWorkflowControlCandidate {
        LiveWorkflowControlCandidate {
            run_id: run_id.to_string(),
            session_id: SessionId::parse_opt(session),
            parent_workflow_run_id: parent.cloned(),
            child_indexes,
        }
    }

    fn render_deps(
        current: Option<&str>,
        live: &[&RunId],
        controls: Vec<LiveWorkflowControlCandidate>,
    ) -> RunStatusRenderDeps {
        RunStatusRenderDeps {
            current_session: SessionId::parse_opt(current),
            live_workflow_run_ids: live.iter().map(|id| (*id).clone()).collect(),
            foreground_controls: controls,
        }
    }

    /// `:611` — gate 2, on its own. The registry has the run and gate 1 passes, so the ONLY thing
    /// that can exclude the second control is its own session. The two comparisons are against
    /// two different objects and a control can carry a stale session after a rotation; collapsing
    /// them would let that control through and steer another instance's child.
    #[test]
    fn live_workflow_controls_require_both_session_comparisons() {
        let run_id = RunId::from_token("wf0000000001");
        let status = workflow_status(&run_id, Some("session-a"), RunState::Running);
        let deps = render_deps(
            Some("session-a"),
            &[&run_id],
            vec![
                candidate("fg-own", Some("session-a"), Some(&run_id), vec![0]),
                candidate("fg-foreign", Some("session-b"), Some(&run_id), vec![0]),
            ],
        );
        let live = deps.live_workflow_controls(&status);
        assert_eq!(live.len(), 1, "{live:?}");
        assert_eq!(live[0].run_id, "fg-own");
    }

    /// `:609`'s `deps.state?.workflowControllers?.has(status.runId)` arm. A `status.json` saying
    /// `workflow`/`running` is not evidence THIS process is driving it — another instance may be,
    /// and its controls are not ours to steer.
    #[test]
    fn live_workflow_controls_are_empty_without_a_registry_entry() {
        let run_id = RunId::from_token("wf0000000001");
        let status = workflow_status(&run_id, Some("session-a"), RunState::Running);
        let controls = vec![candidate(
            "fg-own",
            Some("session-a"),
            Some(&run_id),
            vec![0],
        )];
        assert!(
            render_deps(Some("session-a"), &[], controls.clone())
                .live_workflow_controls(&status)
                .is_empty(),
            "no registry entry means this process is not driving the workflow"
        );
        // Control: the identical input WITH the registry entry is admitted, so the assertion
        // above is proving the registry term rather than passing vacuously.
        assert_eq!(
            render_deps(Some("session-a"), &[&run_id], controls)
                .live_workflow_controls(&status)
                .len(),
            1
        );
    }

    /// `:612` — `(control.activeChildren?.size ?? 0) > 0`. A control whose child has already
    /// finished is still registered for a moment; steering it would deliver into a dead inbox.
    #[test]
    fn live_workflow_controls_exclude_children_with_no_active_children() {
        let run_id = RunId::from_token("wf0000000001");
        let status = workflow_status(&run_id, Some("session-a"), RunState::Running);
        let deps = render_deps(
            Some("session-a"),
            &[&run_id],
            vec![candidate(
                "fg-own",
                Some("session-a"),
                Some(&run_id),
                vec![],
            )],
        );
        assert!(deps.live_workflow_controls(&status).is_empty());
    }

    /// `:609`'s gate 1, on its own: the registry is live and the control matches the status in
    /// every other respect, and only the STATE's current session differs from the status's.
    #[test]
    fn live_workflow_controls_are_empty_when_the_state_session_differs_from_the_status() {
        let run_id = RunId::from_token("wf0000000001");
        let status = workflow_status(&run_id, Some("session-a"), RunState::Running);
        let deps = render_deps(
            Some("session-b"),
            &[&run_id],
            vec![candidate(
                "fg-own",
                Some("session-a"),
                Some(&run_id),
                vec![0],
            )],
        );
        assert!(deps.live_workflow_controls(&status).is_empty());
    }

    /// The `None`/`None` row: a headless or SDK-embedded orchestrator with no session identity,
    /// running a workflow whose `status.json` records no session. Upstream's `===` is TRUE here
    /// and the hints render.
    ///
    /// This is the test that fails the moment anyone "fixes" the two comparisons into
    /// `SessionGate::Strict` — which refuses a `None` current session — and it is the whole
    /// reason `run_status.rs` mentions no gate class at all.
    #[test]
    fn live_workflow_controls_admit_an_unattributed_run_in_a_sessionless_host() {
        let run_id = RunId::from_token("wf0000000001");
        let status = workflow_status(&run_id, None, RunState::Running);
        let deps = render_deps(
            None,
            &[&run_id],
            vec![candidate("fg-own", None, Some(&run_id), vec![0])],
        );
        assert_eq!(
            deps.live_workflow_controls(&status).len(),
            1,
            "a sessionless host steering its own unattributed workflow is upstream's behaviour"
        );
    }

    /// `:685`'s exact sentence, and its position: after the per-step lines and immediately before
    /// `Workflow receipt:`. The relative order is the transferable anchor (upstream's own
    /// `Session:` line has no cyrup counterpart).
    #[tokio::test]
    async fn a_running_workflow_with_no_live_route_renders_the_unavailable_sentence() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("wf0000000001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let mut status = workflow_status(&run_id, Some("session-a"), RunState::Running);
        status.steps = vec![StepStatus::pending("scout")];
        status.workflow_receipt_path = Some(async_root.join("receipt.json"));

        let report = format_status(&status, &paths, &RunStatusRenderDeps::default()).await;
        assert!(
            report.contains(
                "Steer: unavailable; no live foreground route is registered in the active session."
            ),
            "{report}"
        );
        let notice = report
            .find("Steer: unavailable;")
            .expect("the notice is present");
        // A WORKFLOW row is labelled `Child N/M`, not `Step N` (`step_line_label`'s own note).
        let step = report
            .find("Child 1/1: scout")
            .expect("the step line is present");
        let receipt = report
            .find("Workflow receipt:")
            .expect("the receipt line is present");
        assert!(step < notice && notice < receipt, "{report}");
    }

    /// `:687-688` — one line per ACTIVE CHILD INDEX, ascending, byte-for-byte upstream's text.
    ///
    /// The indexes are produced the way the executor produces them — out of a `BTreeMap`'s
    /// `keys()` — which is exactly why upstream's `.sort((left, right) => left - right)` is not
    /// repeated in the renderer: the ordering is the map's, and this test is what says so.
    #[tokio::test]
    async fn live_workflow_child_steer_hints_are_emitted_in_ascending_index_order() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("wf0000000001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let status = workflow_status(&run_id, Some("session-a"), RunState::Running);

        let mut active_children: std::collections::BTreeMap<usize, ()> =
            std::collections::BTreeMap::new();
        for index in [5usize, 0, 2] {
            active_children.insert(index, ());
        }
        let deps = render_deps(
            Some("session-a"),
            &[&run_id],
            vec![candidate(
                "fg-own",
                Some("session-a"),
                Some(&run_id),
                active_children.keys().copied().collect(),
            )],
        );

        let report = format_status(&status, &paths, &deps).await;
        let hints: Vec<&str> = report
            .lines()
            .filter(|line| line.starts_with("Steer live foreground child:"))
            .collect();
        assert_eq!(
            hints,
            vec![
                "Steer live foreground child: subagent({ action: \"steer\", id: \"fg-own\", index: 0, message: \"...\" })",
                "Steer live foreground child: subagent({ action: \"steer\", id: \"fg-own\", index: 2, message: \"...\" })",
                "Steer live foreground child: subagent({ action: \"steer\", id: \"fg-own\", index: 5, message: \"...\" })",
            ],
            "{report}"
        );
        assert!(
            !report.contains("Steer: unavailable;"),
            "a run WITH a live route never also gets the unavailable sentence: {report}"
        );
    }

    /// `:684`'s `status.state === "running"` conjunct. A settled workflow has nothing to steer, so
    /// it gets neither the hints nor — and this is the half that is easy to lose — the
    /// `Steer: unavailable;` sentence, which would otherwise read as a fault on every completed
    /// workflow in the log.
    #[tokio::test]
    async fn a_non_running_workflow_renders_no_steer_lines() {
        let (_dir, async_root, results_dir) = roots();
        let run_id = RunId::from_token("wf0000000001");
        let paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
        let status = workflow_status(&run_id, Some("session-a"), RunState::Complete);
        let deps = render_deps(
            Some("session-a"),
            &[&run_id],
            vec![candidate(
                "fg-own",
                Some("session-a"),
                Some(&run_id),
                vec![0],
            )],
        );
        let report = format_status(&status, &paths, &deps).await;
        assert!(!report.contains("Steer"), "{report}");

        // And the same for a non-workflow run, whatever the registry says (`:684`'s first
        // conjunct).
        let single = RunId::from_token("run000000001");
        let single_paths = RunPaths::for_run(&async_root, &results_dir, &single);
        let mut single_status = RunStatus::queued(single.clone(), RunMode::Single, None);
        single_status
            .advance_state(RunState::Running)
            .expect("Queued -> Running");
        single_status.session_id = SessionId::parse("session-a");
        let single_report = format_status(
            &single_status,
            &single_paths,
            &render_deps(
                Some("session-a"),
                &[&single],
                vec![candidate(
                    "fg-own",
                    Some("session-a"),
                    Some(&single),
                    vec![0],
                )],
            ),
        )
        .await;
        assert!(!single_report.contains("Steer"), "{single_report}");
    }

    /// SCOPE_13 — the async-root reaper renames a run tree onto `.deleting-run-<id>` IN PLACE,
    /// beside the live runs. This is the production listing behind `/subagents-fleet` and
    /// `{action:"status"}` with no id: without the `RUN_TOMBSTONE_PREFIX` arm on
    /// [`crate::background::terminal_run_index::is_reserved_async_root_entry`], a tombstone that
    /// outlives its pass — exactly the case the 24 h grace exists for — would be reconciled and
    /// rendered to the user as a run.
    #[tokio::test]
    async fn a_run_tombstone_is_never_listed_as_an_active_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let async_root = dir.path().join("async");
        let results_dir = dir.path().join("results");
        tokio::fs::create_dir_all(&async_root).await.expect("mkdir");
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");

        let live = RunId::from_token("run000000001".to_string());
        let mut live_status = RunStatus::queued(live.clone(), RunMode::Single, None);
        live_status
            .advance_state(RunState::Running)
            .expect("Queued -> Running");
        let live_dir = async_root.join(live.as_str());
        tokio::fs::create_dir_all(&live_dir).await.expect("mkdir");
        write_atomic_json(
            &crate::background::RunDir::for_existing(&live_dir).status(),
            &live_status,
        )
        .await
        .expect("write live status");

        // A tombstone carrying a perfectly readable RUNNING status — the shape that would
        // otherwise survive every other filter in this listing.
        let retired = RunId::from_token("run000000002".to_string());
        let mut retired_status = RunStatus::queued(retired, RunMode::Single, None);
        retired_status
            .advance_state(RunState::Running)
            .expect("Queued -> Running");
        let tombstone_dir = async_root.join(".deleting-run-0123456789abcdef");
        tokio::fs::create_dir_all(&tombstone_dir)
            .await
            .expect("mkdir");
        write_atomic_json(
            &crate::background::RunDir::for_existing(&tombstone_dir).status(),
            &retired_status,
        )
        .await
        .expect("write retired status");
        // And the reaper's own maintenance directory, which is likewise not a run.
        tokio::fs::create_dir_all(crate::background::async_retention::maintenance_root(
            &async_root,
        ))
        .await
        .expect("mkdir maintenance");

        let candidates = active_run_candidates(&async_root)
            .await
            .expect("candidate scan");
        assert_eq!(
            candidates.iter().map(RunId::as_str).collect::<Vec<_>>(),
            vec!["run000000001"],
            "neither the tombstone nor the maintenance dir is a run candidate"
        );

        let listed = list_active_runs(&async_root, &results_dir, None)
            .await
            .expect("listing");
        assert_eq!(listed.len(), 1, "{listed:#?}");
        assert_eq!(listed[0].status.run_id.as_str(), "run000000001");
        let rendered = format_run_list(&listed);
        assert!(
            !rendered.contains(".deleting-run-"),
            "a tombstone must never reach a user-visible fleet listing: {rendered}"
        );
    }
}
