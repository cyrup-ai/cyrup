//! Human-readable status reporting for the `status` control action (C5).
//!
//! This module is the Rust port of pi's `runs/background/run-status.ts` `inspectSubagentStatus`
//! (`run-status.ts:101-273`) and the `async-status.ts` `listAsyncRuns`/`formatAsyncRunList`
//! no-id "list active runs" shape it delegates to (`async-status.ts:223-338`). It owns the
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
//! `formatResumeGuidance` shape (`run-status.ts:36-50`) an LLM reads to revive a child. Nested
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

/// The lowercase mode string pi renders (`SubagentRunMode`): `single`/`parallel`/`chain`.
pub(crate) fn run_mode_label(mode: RunMode) -> &'static str {
    match mode {
        RunMode::Single => "single",
        RunMode::Parallel => "parallel",
        RunMode::Chain => "chain",
    }
}

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
// Progress + step-line labels (`run-status.ts:52-63`, `async-status.ts:289-308`)
// =================================================================================================

/// The one-line progress label (pi `formatAsyncRunProgressLabel`, `async-status.ts:289-308`),
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
    }
}

/// The per-step line prefix (pi `stepLineLabel`, `run-status.ts:52-63`): `Agent i/N` for a
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
    }
}

// =================================================================================================
// Resume guidance (`run-status.ts:36-50`)
// =================================================================================================

/// G90: pi `formatSteeringSummary` (`run-status.ts:76-81` @v0.34.0) — `"3 steers, last
/// 2026-08-10T12:00:00.000Z"`, with either half omitted when unknown and `None` when neither is.
fn format_steering_summary(steer_count: Option<u64>, last_steer_at: Option<i64>) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(count) = steer_count {
        parts.push(format!("{count} steer{}", if count == 1 { "" } else { "s" }));
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
/// `hasExistingSessionFile`, `run-status.ts:32-34`).
fn session_file_exists(session_file: &Option<PathBuf>) -> bool {
    session_file.as_ref().is_some_and(|path| path.exists())
}

/// The `Resume:`/`Revive:` guidance line for a non-running run (pi `formatResumeGuidance`,
/// `run-status.ts:36-50`). Every cyrup [`StepStatus`] carries a known agent, so the "known child"
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

/// Formats an epoch-millisecond timestamp as an ISO-8601 UTC string
/// (`YYYY-MM-DDTHH:MM:SS.mmmZ`), matching pi's `new Date(ms).toISOString()` output shape. Pure
/// arithmetic (Howard Hinnant's proleptic-Gregorian `civil_from_days`), so it needs no date-time
/// dependency and cannot panic.
fn format_iso8601_millis(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400);
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's `civil_from_days`: convert a count of days since the Unix epoch
/// (1970-01-01) into a proleptic-Gregorian `(year, month, day)`. Integer-only, total.
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

// =================================================================================================
// Single-run status report (`inspectSubagentStatus`, run-status.ts:101-273)
// =================================================================================================

/// Renders one reconciled [`RunStatus`] as pi's full status report (`run-status.ts:202-243`) —
/// run identity/state/mode/progress, pending appends, timestamps, dir, the authoritative terminal
/// `Result:` reference (when its file exists), per-step lines, resume guidance for a non-running
/// run, and the `Log`/`Events` artifact references (when they exist).
fn format_status(status: &RunStatus, paths: &RunPaths) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Run: {}", status.run_id));
    lines.push(format!("State: {}", run_state_label(status.state)));
    lines.push(format!("Mode: {}", run_mode_label(status.mode)));
    lines.push(format!("Progress: {}", progress_label(status)));
    if let Some(pending) = status.pending_appends
        && pending > 0
    {
        lines.push(format!("Pending appends: {pending}"));
    }
    lines.push(format!("Started: {}", format_iso8601_millis(status.started_at)));
    lines.push(format!("Updated: {}", format_iso8601_millis(status.last_update)));
    lines.push(format!("Dir: {}", paths.run_dir.display()));
    if paths.result.exists() {
        lines.push(format!("Result: {}", paths.result.display()));
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
        // G90: pi's `steeringSuffix` (`run-status.ts:369,375` @v0.34.0), between the activity text
        // and the error text. This is what makes an `action: "steer"` VISIBLE: without it the tool
        // would report "Steering queued" and the status report would look identical whether the
        // runner accepted the request or dropped it.
        let steering_text = format_steering_summary(
            step.telemetry.steer_count,
            step.telemetry.last_steer_at,
        );
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
        let step_log = paths.step_output_log(index);
        if step_log.exists() {
            lines.push(format!("  Output: {}", step_log.display()));
        }
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
async fn inspect_paths(paths: &RunPaths) -> Result<Option<String>, SubagentError> {
    match reconcile_before_control_op(paths).await {
        Ok(status) => Ok(Some(format_status(&status, paths))),
        Err(SubagentError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
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

    // Exact match first: a run directory, its status.json, or its terminal result file already
    // exists under this exact id.
    let exact = RunPaths::for_run(async_root, results_dir, &RunId::from_token(selector.to_string()));
    if path_exists(&exact.run_dir).await
        || path_exists(&exact.status).await
        || path_exists(&exact.result).await
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
/// # Errors
///
/// Propagates safe-token/ambiguity resolution errors and genuine reconciliation I/O failures.
pub async fn inspect_status_by_id(
    async_root: &Path,
    results_dir: &Path,
    selector: &str,
) -> Result<Option<String>, SubagentError> {
    let Some(run_id) = resolve_run_id(async_root, results_dir, selector).await? else {
        return Ok(None);
    };
    let paths = RunPaths::for_run(async_root, results_dir, &run_id);
    inspect_paths(&paths).await
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
/// # Errors
///
/// Returns [`SubagentError::UnsafePathToken`] if `async_dir` has no usable basename, or propagates
/// a genuine reconciliation I/O failure.
pub async fn inspect_status_by_dir(
    async_dir: &Path,
    results_dir: &Path,
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
    inspect_paths(&paths).await
}

// =================================================================================================
// No-id "list active runs" (`listAsyncRuns`/`formatAsyncRunList`, async-status.ts:223-338)
// =================================================================================================

/// One active-run summary row for the no-id list: its own run directory plus the reconciled status.
#[derive(Clone, Debug)]
pub struct ActiveRun {
    /// The run's own directory (`async_root/<run_id>`), rendered as the `dir` field.
    pub dir: PathBuf,
    /// The reconciled status snapshot.
    pub status: RunStatus,
}

/// State-ordering rank for the active-run list (pi `sortRuns`, `async-status.ts:204-221`, restricted
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
/// `listAsyncRuns` with `states: ["queued", "running"]`, `async-status.ts:223-258`): scan the run
/// directories, reconcile each (R-SA-079), keep the queued/running ones, and sort running-first
/// then most-recently-updated first. A missing `async_root` (no runs ever spawned for this cwd) is
/// an empty list, not an error. A run whose status cannot be reconciled (e.g. a half-written
/// directory) is skipped rather than aborting the whole scan (graceful degradation, matching pi's
/// `if (!status) continue`).
///
/// # Errors
///
/// Returns [`SubagentError::Spawn`] only if `async_root` exists but its directory listing itself
/// fails.
pub async fn list_active_runs(
    async_root: &Path,
    results_dir: &Path,
) -> Result<Vec<ActiveRun>, SubagentError> {
    let mut runs: Vec<ActiveRun> = Vec::new();

    let mut entries = match tokio::fs::read_dir(async_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(runs),
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
        let run_id = RunId::from_token(name);
        let paths = RunPaths::for_run(async_root, results_dir, &run_id);
        // Reconcile before summarizing (R-SA-079); a run that reconciles to a terminal/paused
        // state, or that cannot be read at all, is simply not an *active* run.
        let Ok(status) = reconcile_before_control_op(&paths).await else {
            continue;
        };
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

/// Render the active-run list (pi `formatAsyncRunList`, `async-status.ts:318-338`): a
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
            .map(|count| format!(" | {count} pending append{}", if count == 1 { "" } else { "s" }))
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
    /// (pi `steeringSuffix`, `run-status.ts:369,375` @v0.34.0). Without this the tool would say
    /// "Steering queued" and the status report would look identical whether the runner accepted
    /// the request or dropped it on the floor.
    #[test]
    fn a_steps_accepted_steers_are_rendered_in_pis_steering_suffix() {
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

        let report = format_status(&status, &paths);
        assert!(
            report.contains("Step 1: scout running, steering: 2 steers, last 2023-11-14T22:13:20.000Z"),
            "{report}"
        );

        // Singular, and no suffix at all when nothing was ever steered.
        status.steps[0].telemetry.steer_count = Some(1);
        status.steps[0].telemetry.last_steer_at = None;
        assert!(format_status(&status, &paths).contains(", steering: 1 steer"), "singular form");
        status.steps[0].telemetry.steer_count = None;
        let quiet = format_status(&status, &paths);
        assert!(!quiet.contains("steering:"), "an unsteered step gets no suffix: {quiet}");
    }

    /// G77 — the `status` action's rendering of a stopped run: its own state word, and pi's
    /// verbatim not-resumable guidance INSTEAD of the `Revive:` line the same steps would
    /// otherwise earn. The step below deliberately carries an existing `session_file`, which is
    /// precisely the input that would produce a `Revive:` line without the stopped short-circuit.
    #[test]
    fn a_stopped_run_renders_its_own_state_word_and_refuses_to_offer_a_revive() {
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

        let report = format_status(&status, &paths);
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

    #[test]
    fn iso8601_formats_a_known_epoch() {
        assert_eq!(format_iso8601_millis(0), "1970-01-01T00:00:00.000Z");
        // 2021-01-01T00:00:00.000Z == 1_609_459_200_000 ms.
        assert_eq!(format_iso8601_millis(1_609_459_200_000), "2021-01-01T00:00:00.000Z");
        // Same day + 12:34:56.789 (== 45_296_789 ms of day) exercises the time + millis fields.
        assert_eq!(
            format_iso8601_millis(1_609_459_200_000 + 45_296_789),
            "2021-01-01T12:34:56.789Z"
        );
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

        let active = list_active_runs(&async_root, &results_dir)
            .await
            .expect("list active");
        let ids: Vec<&str> = active.iter().map(|r| r.status.run_id.as_str()).collect();
        assert!(ids.contains(&"run0running"), "running run must be listed");
        assert!(ids.contains(&"run0queued0"), "queued run must be listed");
        assert!(!ids.contains(&"run0donexxx"), "completed run must be excluded");

        // running sorts before queued.
        assert_eq!(active.first().map(|r| r.status.run_id.as_str()), Some("run0running"));

        let rendered = format_run_list(&active);
        assert!(rendered.starts_with("Active async runs: 2"), "heading: {rendered}");
        assert!(rendered.contains("run0running | running | chain"), "run line: {rendered}");
        assert!(!rendered.contains("run0donexxx"), "terminal run absent: {rendered}");
    }

    #[tokio::test]
    async fn format_run_list_empty_is_the_no_active_sentinel() {
        let (_dir, async_root, results_dir) = roots();
        let active = list_active_runs(&async_root, &results_dir)
            .await
            .expect("list active over a missing async root is empty, not an error");
        assert!(active.is_empty());
        assert_eq!(format_run_list(&active), "No active async runs.");
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

        let report = inspect_status_by_id(&async_root, &results_dir, "run0chain00")
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
        let status = running_status(&run_id, RunMode::Single, vec![StepStatus::pending("worker")]);
        write_status(&paths, &status).await;

        let report = inspect_status_by_id(&async_root, &results_dir, "abcdef")
            .await
            .expect("inspect ok")
            .expect("prefix resolves");
        assert!(report.contains("Run: abcdef123456"), "{report}");
    }

    #[tokio::test]
    async fn inspect_status_by_id_missing_run_is_none() {
        let (_dir, async_root, results_dir) = roots();
        let found = inspect_status_by_id(&async_root, &results_dir, "nosuchrun00")
            .await
            .expect("inspect ok");
        assert!(found.is_none(), "a missing run resolves to None (not-found notice)");
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
        tokio::fs::write(&session_file, b"{}\n").await.expect("write session");

        // Orchestrator side: deliver the interrupt.
        let outcome = control::interrupt(&async_root, &results_dir, "run0intr000", "interrupt-action", None)
            .await
            .expect("interrupt delivered");
        assert_eq!(outcome, InterruptOutcome::Delivered);
        assert!(
            tokio::fs::try_exists(&paths.control_inbox).await.expect("exists"),
            "the control-inbox interrupt request must be written"
        );

        // Runner side (simulated): consume the request and soft-pause — R-SA-084: Paused, not Failed.
        let consumed = control::consume_interrupt_request(&paths)
            .await
            .expect("consume ok");
        assert!(consumed.is_some(), "the pending interrupt must be consumable");
        let mut paused = status;
        if let Some(step) = paused.steps.first_mut() {
            step.status = StepState::Paused;
        }
        paused.advance_state(RunState::Paused).expect("Running -> Paused");
        write_status(&paths, &paused).await;

        // Observable outcome: the status report now reads paused, with a real revive guidance line.
        let report = inspect_status_by_id(&async_root, &results_dir, "run0intr000")
            .await
            .expect("inspect ok")
            .expect("found");
        assert!(report.contains("State: paused"), "soft-pause, not failed: {report}");
        assert!(report.contains("researcher paused"), "the step is paused: {report}");
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
        let report = inspect_status_by_id(&async_root, &results_dir, "run0appchn0")
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
        let status = running_status(&run_id, RunMode::Single, vec![StepStatus::pending("worker")]);
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

        let steer = control::resume(&async_root, &results_dir, "run0live000", None)
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

        let revival = control::resume(&async_root, &results_dir, "run0dead000", None).await;
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
        revive.advance_state(RunState::Complete).expect("-> Complete");
        write_status(&revive_paths, &revive).await;

        let respawn = control::resume(&async_root, &results_dir, "run0revive0", None)
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
}
