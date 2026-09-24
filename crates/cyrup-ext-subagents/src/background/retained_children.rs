//! `children.list` — the retained children of this session's workflow runs, with whether each can
//! be resumed (pi `src/runs/background/retained-children.ts`, 135 lines @v0.68.0).
//!
//! Upstream's `listRetainedChildren` (`:83-108`) selects ASYNC RUNS that carry a
//! `parentWorkflowRunId` and exactly one step: every upstream workflow child is a sibling async
//! run with its own directory. cyrup's workflow children are not. Every child a `workflowScript`
//! launches is a FOREGROUND run (`extension/executor/workflow.rs:855-905` feeds
//! `run_foreground_streaming`; `grep -n spawn_background workflow.rs` is zero-hit), it writes no
//! `ResultFile` and owns no async directory, and the `parent_workflow_run_id` its launch request
//! carries lands on an in-memory `ForegroundControlEntry`, never on disk —
//! [`crate::background::async_retention`]'s policy note records that `RunStatus` has no
//! `parent_workflow_run_id` and no top-level `workflow_key`, and that note stays true.
//!
//! What IS on disk is the WORKFLOW run: `workflow_launch.rs` writes `<async_root>/<wf>/status.json`
//! with `mode: Workflow`, `WorkflowRunHost::publish_steps` republishes `steps` as each child
//! settles, and the terminal write rebuilds them from
//! [`crate::workflows::workflow_step_statuses`]. The settled children of a workflow are therefore
//! retained as STEP ROWS of the workflow's own status file, and one upstream "retained child run"
//! is one cyrup `(workflow status, step index)` pair. That is the selection this module makes:
//! workflow-mode runs that are not display-dismissed — in ANY state, because upstream's state
//! gates are over the child — and within each, every step row in a retained state that carries a
//! `run_id`.
//!
//! The scan is [`crate::tui::fleet::collect_async_runs_by_scan`] — the DIRECTORY-SCAN half of the
//! crate's `listAsyncRuns` port (pi `repairScan`, `async-status.ts:503-504`, an unbounded
//! `readdirSync`), with the same `sessionId` filter (`:585`) applied BEFORE the bound, and then
//! the newest [`crate::tui::fleet::MAX_FLEET_HISTORY_CANDIDATES`] of THIS session's runs (which
//! [`MAX_RETAINED_CHILD_CANDIDATES`] equals and a test pins). That is the shape upstream's
//! `listRetainedChildren` gets from `entryLimit: 100`, which `listAsyncRuns` hands only to the
//! per-session terminal-index read (`:519`): a hundred of the caller's own runs, never a hundred
//! of everybody's with the caller's filtered out afterwards.
//! NOT [`crate::tui::fleet::collect_fleet_history`]: that one takes
//! its candidates from the active/terminal run indexes whenever either is non-empty, and a
//! foreground workflow run that settled is in neither. Both indexes are filed through one
//! writer, [`crate::background::active_run_index::update_active_run_index`] (the terminal index
//! is written from inside it, `active_run_index.rs:280`), and `settle_foreground_workflow`
//! (`workflow_launch.rs`) never calls it: it writes `status.json` and the receipt. The writer's
//! production callers (`grep -rn 'update_active_run_index(' src`, outside `#[cfg(test)]`) are
//! the detached runner (`runner_main/entry.rs`, `runner_main/finish.rs`), a DETACHED workflow's
//! settle (`workflow_detach/mod.rs`), the stale-marker release in `active_run_index.rs`
//! (`read_live_active_run_ids`, over a run already marked), and the reconciler's two repairs in
//! `reconcile.rs` — `repair_from_result`, over a `ResultFile` a foreground workflow never writes,
//! and the stale-dead mark, over a `Running` status whose `pid` is dead. Only the last can reach
//! a foreground workflow at all: its status carries this process's pid
//! (`workflow_launch.rs:157-162`), so a workflow left `Running` by a host that died is filed as
//! `Failed` the next time something reconciles it. A workflow's SETTLED step rows are filed
//! nowhere; only its crash is — which is why the index cannot be the retention source. Upstream's
//! children ARE indexed async runs, so upstream can read the indexes; cyrup's retention lives on
//! un-indexed files, so this scan reads the directory.
//!
//! # [CYRUP-DELTA] Two rungs of upstream's row have no datum on a step row
//!
//! - **`task`** (`:99` `boundedTaskSummary(step.description)`): the step row carries no
//!   description ([`StepStatus`]), the receipt entry carries none, and `foreground-history.json`
//!   carries none — the task text exists only in the in-memory `results[0].task` of the settle
//!   call. The row therefore always takes upstream's own `(no task summary)` branch (`:123`);
//!   [`bounded_task_summary`] is ported and applied so the day a description lands on the row it
//!   prints bounded.
//! - **`parentRunId?`** is not optional here: a cyrup retained child is always a step of a
//!   workflow run, so [`RetainedChild::parent_run_id`] is the workflow's id, and the `workflow:`
//!   line is always printed. The resume hint names `(parent, index)` — see
//!   [`format_retained_children`].
//!
//! # Where the recovery descriptor is read
//!
//! Upstream reads `readAsyncRecoveryDescriptor(run.asyncDir)` (`:59`) — the CHILD's own async
//! dir. A cyrup workflow child has none, so [`child_resumability`] reads the descriptor at the
//! WORKFLOW run's dir ([`RunDir::recovery_descriptor`]) — the same file
//! `SubagentExecutor::revive_from_transcript` reads for `resume { id: <workflow>, index }`, so the
//! listing's verdict and the verb's agree on the same on-disk state. The SENTENCES differ: the
//! listing prints pi's `missing recovery descriptor`, while `resume` refuses with
//! [`RecoveryDescriptorError::Missing`]'s own message (`Async child '<id>' is missing its required
//! run fan-out recovery identity. Start a new run instead.`). Today no producer writes one for a
//! workflow (`RecoveryDescriptor::for_single_launch`'s single production caller is the async
//! SINGLE launch), so every workflow child lists as `not resumable (missing recovery descriptor)`
//! until a per-child descriptor location exists; nothing here fabricates a resumable verdict.

use std::path::{Path, PathBuf};

use cyrup_core::Usage;

use crate::background::{
    RecoveryDescriptor, RecoveryDescriptorError, RunDir, RunId, RunMode, RunStatus, StepState,
    StepStatus,
};
use crate::workflows::WorkflowKey;

/// pi `MAX_RETAINED_CHILDREN` (`retained-children.ts:7`).
pub const MAX_RETAINED_CHILDREN: usize = 10;
/// pi `MAX_RETAINED_CHILD_CANDIDATES` (`:8`) — the `entryLimit` of the scan, which is
/// [`crate::tui::fleet::MAX_FLEET_HISTORY_CANDIDATES`] by construction.
pub const MAX_RETAINED_CHILD_CANDIDATES: usize = crate::tui::fleet::MAX_FLEET_HISTORY_CANDIDATES;
/// pi `MAX_TASK_SUMMARY_LENGTH` (`:9`).
pub const MAX_TASK_SUMMARY_LENGTH: usize = 120;

/// pi `RetainedChildState` (`:11`) — the states a retained child may be listed in.
///
/// Upstream's `run` at `:85-104` is the CHILD async run, so `isRetainedChildState(run.state)`
/// (`:87`) and `isTerminalStepStatus(step.status)` (`:89`) both gate the child's own state. A
/// cyrup child IS its step row, so both gates are this one conversion over [`StepState`]:
/// `Pending`/`Running` are refused, the four terminal-or-paused states convert.
/// `isRetainedChildState` also admits `"partial"`, but `isTerminalStepStatus` does not, and a
/// cyrup child is its step row — so [`StepState::Partial`] is refused, as upstream's step gate
/// refuses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetainedChildState {
    /// `complete`.
    Complete,
    /// `failed`.
    Failed,
    /// `paused`.
    Paused,
    /// `stopped`.
    Stopped,
}

impl RetainedChildState {
    /// The lowercase word the listing prints (`:121`), the same spelling
    /// `run_status::run_state_label` uses for the run.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Paused => "paused",
            Self::Stopped => "stopped",
        }
    }
}

impl TryFrom<StepState> for RetainedChildState {
    type Error = StepState;

    fn try_from(state: StepState) -> Result<Self, Self::Error> {
        match state {
            StepState::Complete => Ok(Self::Complete),
            StepState::Failed => Ok(Self::Failed),
            StepState::Paused => Ok(Self::Paused),
            StepState::Stopped => Ok(Self::Stopped),
            // SUBA-100 — upstream's STEP gate, `isTerminalStepStatus` (`:33-35` @v0.68.0), does
            // not list `"partial"`, so a partial step is not retained, whatever its run's state.
            StepState::Pending | StepState::Running | StepState::Partial => Err(state),
        }
    }
}

/// pi `RetainedChildResumability` (`:12-14`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Resumability {
    /// The child can be resumed from this persisted session file.
    Resumable {
        /// pi `sessionPath`.
        session_path: PathBuf,
    },
    /// The child cannot be resumed, with pi's reason sentence.
    NotResumable {
        /// pi `reason`.
        reason: String,
    },
}

impl Resumability {
    fn not_resumable(reason: impl Into<String>) -> Self {
        Self::NotResumable {
            reason: reason.into(),
        }
    }

    /// `resumability.state === "resumable"`.
    #[must_use]
    pub fn is_resumable(&self) -> bool {
        matches!(self, Self::Resumable { .. })
    }
}

/// pi `RetainedChild` (`:16-27`).
///
/// `PartialEq` only: [`Usage`] carries `f64` cost figures, so `token_totals` has no `Eq`.
#[derive(Clone, Debug, PartialEq)]
pub struct RetainedChild {
    /// The CHILD's own run id (`step.run_id`).
    pub run_id: RunId,
    /// The workflow run the child is a step of — always present here (module doc).
    pub parent_run_id: RunId,
    /// [CYRUP-DELTA] cyrup addresses a workflow child as `(parent, index)`: `resume` takes the
    /// workflow id and this index (`SubagentExecutor::control_resume` accepts `index`), because
    /// the child has no async dir of its own to be addressed by. The resume hint prints it.
    pub step_index: usize,
    /// pi `workflowKey?` — the step's lane key.
    pub workflow_key: Option<WorkflowKey>,
    /// pi `state` — the CHILD run's state (`:97` `run.state`, `run` being the child async run
    /// selected at `:86`); here the step row's own [`StepStatus::status`].
    pub state: RetainedChildState,
    /// pi `agent`.
    pub agent: String,
    /// pi `taskSummary` — see the module's `[CYRUP-DELTA]`.
    pub task_summary: String,
    /// pi `completedAt` — the CHILD's `run.endedAt ?? run.lastUpdate` (`:90`); here the step
    /// row's [`StepStatus::ended_at`] (stamped when the child settled,
    /// [`crate::workflows::carry_step_settle_times`]), falling back — for a row written before
    /// that stamp existed — to the workflow status file's own `ended_at ?? last_update`, the last
    /// time the file the row sits in was written.
    pub completed_at: i64,
    /// pi `resumability`.
    pub resumability: Resumability,
    /// pi `tokenTotals?` — the step's usage when it carries a value.
    pub token_totals: Option<Usage>,
}

/// pi `retainedSessionFile` (`:37-50`) — the session-file rung, every sentence verbatim.
///
/// `lstatSync` is [`tokio::fs::symlink_metadata`]: a symlink is refused as "not a regular file"
/// rather than followed.
async fn retained_session_file(session_file: Option<&Path>) -> Resumability {
    let Some(path) = session_file else {
        return Resumability::not_resumable("no persisted session file");
    };
    if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
        return Resumability::not_resumable("persisted session file is not a .jsonl file");
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) => {
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Resumability::not_resumable("persisted session file is not a regular file");
            }
            Resumability::Resumable {
                session_path: path.to_path_buf(),
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Resumability::not_resumable(
            format!("persisted session file is missing: {}", path.display()),
        ),
        Err(error) => Resumability::not_resumable(format!(
            "persisted session file could not be inspected: {error}"
        )),
    }
}

/// pi `childResumability` (`:52-74`), rung by rung, over a workflow status and one of its step
/// rows.
///
/// The cwd ladder (`:68`) is `resolve_retained_worktree_cwd(...) ?? descriptor.cwd ?? run.cwd`
/// upstream; here the final `?? run.cwd` is unreachable because
/// [`RecoveryDescriptor::cwd`] is not optional. Any [`crate::handoff::HandoffError`] from the
/// manifest read — and a `LaneId` that will not parse — is upstream's `catch` at `:70-71`, a
/// NOT-RESUMABLE verdict, deliberately unlike the revive's swallow-and-fall-through: a revive is
/// choosing where to run, a listing is stating whether it can.
async fn child_resumability(
    async_root: &Path,
    status: &RunStatus,
    step_index: usize,
    step: &StepStatus,
) -> Resumability {
    // `:53` — `run.state === "stopped" || step.status === "stopped"`, both the CHILD's; the step
    // row is the one record cyrup has of it.
    if step.status == StepState::Stopped {
        return Resumability::not_resumable("stopped run");
    }
    // `:54` — `step.runner?.type === "external-cli" || step.runner?.type === "external-job"`:
    // [`StepStatus::runner`] is the child's own `SingleResult::runner`, filled at settle by
    // `workflow_step_statuses`, and its `kind` is upstream's `type` discriminant.
    if step
        .runner
        .as_ref()
        .is_some_and(|runner| runner.kind == "external-cli" || runner.kind == "external-job")
    {
        return Resumability::not_resumable("external runner");
    }
    // `:55-56`
    let session = retained_session_file(
        step.session_file
            .as_deref()
            .or(status.session_file.as_deref()),
    )
    .await;
    if !session.is_resumable() {
        return session;
    }
    // `:57-65` — read at the WORKFLOW run's dir (module doc).
    let run_dir = RunDir::new(async_root, &status.run_id);
    let descriptor = match RecoveryDescriptor::read(&run_dir.recovery_descriptor()).await {
        Ok(Some(descriptor)) => descriptor,
        Ok(None) => return Resumability::not_resumable("missing recovery descriptor"),
        Err(error) => {
            return Resumability::not_resumable(format!("invalid recovery descriptor: {error}"));
        }
    };
    match descriptor.assert_belongs_to(status, &step.agent) {
        Ok(()) => {}
        Err(RecoveryDescriptorError::SourceRunMismatch { found, .. }) => {
            return Resumability::not_resumable(format!(
                "recovery descriptor belongs to run {found}"
            ));
        }
        Err(RecoveryDescriptorError::AgentMismatch {
            descriptor_agent, ..
        }) => {
            return Resumability::not_resumable(format!(
                "recovery descriptor belongs to agent {descriptor_agent}"
            ));
        }
        Err(error) => {
            return Resumability::not_resumable(format!("invalid recovery descriptor: {error}"));
        }
    }
    // `:66-72`
    let required_cwd = match resolve_required_cwd(&run_dir, status, step_index).await {
        Ok(Some(worktree)) => worktree,
        Ok(None) => descriptor.cwd.clone(),
        Err(error) => {
            return Resumability::not_resumable(format!("resume dependency unavailable: {error}"));
        }
    };
    // `:69` — `fs.statSync(requiredCwd).isDirectory()`: a path that exists but is not a
    // directory is `required cwd is missing: <path>`; a `stat` that throws is the `catch` at
    // `:70-71`, `resume dependency unavailable: <error>`. Node spells ENOENT as
    // `ENOENT: no such file or directory, stat '<path>'`, so a deleted cwd prints that sentence.
    match tokio::fs::metadata(&required_cwd).await {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Resumability::not_resumable(format!(
                "required cwd is missing: {}",
                required_cwd.display()
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Resumability::not_resumable(format!(
                "resume dependency unavailable: ENOENT: no such file or directory, stat '{}'",
                required_cwd.display()
            ));
        }
        Err(error) => {
            return Resumability::not_resumable(format!("resume dependency unavailable: {error}"));
        }
    }
    // `:73`
    session
}

/// `resolveRetainedWorktreeCwd(parallelHandoffPath(run.asyncDir), run.id, step.index)` (`:68`),
/// with the `LaneId` parse folded into the same fallible arm.
async fn resolve_required_cwd(
    run_dir: &RunDir,
    status: &RunStatus,
    step_index: usize,
) -> Result<Option<PathBuf>, crate::handoff::HandoffError> {
    let lane = crate::handoff::LaneId::parse(status.run_id.as_str())?;
    crate::handoff::resolve_retained_worktree_cwd(
        &run_dir.handoff(),
        &lane,
        u32::try_from(step_index).unwrap_or(u32::MAX),
    )
    .await
}

/// pi `boundedTaskSummary` (`:76-81`): whitespace-collapsed, trimmed, and cut to
/// [`MAX_TASK_SUMMARY_LENGTH`] with a trailing `…` replacing the last character.
#[must_use]
pub fn bounded_task_summary(value: Option<&str>) -> String {
    let normalized = value
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.chars().count() > MAX_TASK_SUMMARY_LENGTH {
        let mut cut: String = normalized
            .chars()
            .take(MAX_TASK_SUMMARY_LENGTH - 1)
            .collect();
        cut.push('…');
        cut
    } else {
        normalized
    }
}

/// pi `listRetainedChildren` (`:83-108`), over the workflow runs the directory scan returns.
///
/// `session_id: None` is pi's falsy `sessionId`: no session filter, which is what
/// [`crate::tui::fleet::collect_async_runs_by_scan`] implements (module doc: why the scan and
/// not the indexed fleet history). The scan does not apply upstream's `displayDismissedAt` skip
/// (`async-status.ts:579`), so it is applied here, to the workflow status the rows sit in. The
/// WORKFLOW's own state does not gate its rows: upstream's `states` filter (`:84`) and
/// `isRetainedChildState` (`:87`) are over the CHILD run, which here is the step row, so a
/// settled child of a workflow that is still `Running` is listed, and a `failed` child of a
/// workflow that caught the failure and completed is listed as `failed`. `completedAt`'s
/// `undefined` guard (`:91`) is unreachable: [`RunStatus::last_update`] is not optional.
///
/// # Errors
///
/// The scan's own directory-listing error, when the async root exists but cannot be read.
pub async fn list_retained_children(
    async_root: &Path,
    results_dir: &Path,
    session_id: Option<&str>,
) -> Result<Vec<RetainedChild>, String> {
    let runs =
        crate::tui::fleet::collect_async_runs_by_scan(async_root, results_dir, session_id).await?;
    let mut children = Vec::new();
    for view in &runs {
        let status = &view.status;
        if status.mode != RunMode::Workflow || status.display_dismissed_at.is_some() {
            continue;
        }
        for (step_index, step) in status.steps.iter().enumerate() {
            let Some(run_id) = step.run_id.clone() else {
                continue;
            };
            // `:87` + `:89`, both over the child: the row's own state.
            let Ok(state) = RetainedChildState::try_from(step.status) else {
                continue;
            };
            let completed_at = step
                .ended_at
                .unwrap_or_else(|| status.ended_at.unwrap_or(status.last_update));
            let resumability = child_resumability(async_root, status, step_index, step).await;
            children.push(RetainedChild {
                run_id,
                parent_run_id: status.run_id.clone(),
                step_index,
                workflow_key: step.workflow_key.clone(),
                state,
                agent: step.agent.clone(),
                task_summary: bounded_task_summary(None),
                completed_at,
                resumability,
                token_totals: (step.usage != Usage::default()).then(|| step.usage.clone()),
            });
        }
    }
    // `:106` — newest first; `sort_by_key` is stable, as `Array.prototype.sort` is.
    children.sort_by_key(|child| std::cmp::Reverse(child.completed_at));
    Ok(children)
}

/// pi `formatRetainedChildren` (`:110-135`).
///
/// The window is the first [`MAX_RETAINED_CHILDREN`]; when none of those is resumable, the first
/// resumable child BEYOND the window replaces the window's last slot (`:113-116`), so a resumable
/// child is retained when one exists at all. Two `[CYRUP-DELTA]`s, both from the module doc: the
/// `workflow:` line is unconditional (the parent is not optional), and the resume hint is
/// `subagent({ action: "resume", id: "<workflow>", index: <step>, message: "..." })` because a
/// workflow child is addressed through its workflow run and step index.
#[must_use]
pub fn format_retained_children(children: &[RetainedChild]) -> String {
    if children.is_empty() {
        return "No retained workflow children in the active parent session. If a retained-writer \
                challenge is required, launch a same-role fallback challenge and label it as \
                fallback."
            .to_string();
    }
    let mut retained: Vec<&RetainedChild> = children.iter().take(MAX_RETAINED_CHILDREN).collect();
    if !retained
        .iter()
        .any(|child| child.resumability.is_resumable())
        && let Some(resumable) = children
            .iter()
            .skip(MAX_RETAINED_CHILDREN)
            .find(|child| child.resumability.is_resumable())
        && retained.len() == MAX_RETAINED_CHILDREN
        && let Some(last) = retained.last_mut()
    {
        *last = resumable;
    }
    let has_resumable_child = retained
        .iter()
        .any(|child| child.resumability.is_resumable());
    let mut lines = vec![format!(
        "Retained workflow children (up to {MAX_RETAINED_CHILDREN}; newest first, with a \
         resumable child retained when available):"
    )];
    for child in retained {
        lines.push(format!(
            "- {} | {} | {} | {}",
            child.run_id,
            child.agent,
            child.state.label(),
            crate::time::format_iso8601_millis(child.completed_at)
        ));
        lines.push(match &child.workflow_key {
            Some(key) => format!("  workflow: {} ({key})", child.parent_run_id),
            None => format!("  workflow: {}", child.parent_run_id),
        });
        lines.push(format!(
            "  task: {}",
            if child.task_summary.is_empty() {
                "(no task summary)"
            } else {
                child.task_summary.as_str()
            }
        ));
        match &child.resumability {
            Resumability::Resumable { session_path } => {
                lines.push("  resumability: resumable".to_string());
                lines.push(format!("  session: {}", session_path.display()));
                lines.push(format!(
                    "  resume: subagent({{ action: \"resume\", id: \"{}\", index: {}, message: \
                     \"...\" }})",
                    child.parent_run_id, child.step_index
                ));
            }
            Resumability::NotResumable { reason } => {
                lines.push(format!("  resumability: not resumable ({reason})"));
            }
        }
        if let Some(usage) = &child.token_totals {
            lines.push(format!(
                "  tokens: input {}, output {}, total {}",
                usage.input, usage.output, usage.total_tokens
            ));
        }
    }
    if !has_resumable_child {
        lines.push(
            "No resumable retained child is listed. Launch a same-role fallback challenge and \
             label it as fallback."
                .to_string(),
        );
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn child(run_id: &str, completed_at: i64, resumability: Resumability) -> RetainedChild {
        RetainedChild {
            run_id: RunId::from_token(run_id),
            parent_run_id: RunId::from_token("wf-1"),
            step_index: 0,
            workflow_key: None,
            state: RetainedChildState::Complete,
            agent: "worker".to_string(),
            task_summary: String::new(),
            completed_at,
            resumability,
            token_totals: None,
        }
    }

    #[test]
    fn the_candidate_limit_is_the_fleet_scans_own() {
        assert_eq!(
            MAX_RETAINED_CHILD_CANDIDATES,
            crate::tui::fleet::MAX_FLEET_HISTORY_CANDIDATES
        );
        assert_eq!(MAX_RETAINED_CHILD_CANDIDATES, 100);
    }

    /// `:76-81` — whitespace collapse, trim, and the 119-plus-ellipsis cut.
    #[test]
    fn bounded_task_summary_collapses_and_cuts() {
        assert_eq!(bounded_task_summary(None), "");
        assert_eq!(bounded_task_summary(Some("  a \n\t b   c ")), "a b c");
        let exact = "x".repeat(MAX_TASK_SUMMARY_LENGTH);
        assert_eq!(bounded_task_summary(Some(&exact)), exact);
        let long = "y".repeat(MAX_TASK_SUMMARY_LENGTH + 1);
        let cut = bounded_task_summary(Some(&long));
        assert_eq!(cut.chars().count(), MAX_TASK_SUMMARY_LENGTH);
        assert!(cut.ends_with('…'));
        assert!(cut.starts_with(&"y".repeat(MAX_TASK_SUMMARY_LENGTH - 1)));
    }

    /// `:29-31` minus `partial`, over the CHILD's state (the step row): only the four retained
    /// states convert.
    #[test]
    fn only_retained_step_states_convert() {
        assert_eq!(
            RetainedChildState::try_from(StepState::Running),
            Err(StepState::Running)
        );
        assert_eq!(
            RetainedChildState::try_from(StepState::Pending),
            Err(StepState::Pending)
        );
        assert_eq!(
            RetainedChildState::try_from(StepState::Complete),
            Ok(RetainedChildState::Complete)
        );
        assert_eq!(
            RetainedChildState::try_from(StepState::Failed),
            Ok(RetainedChildState::Failed)
        );
        assert_eq!(
            RetainedChildState::try_from(StepState::Paused),
            Ok(RetainedChildState::Paused)
        );
        assert_eq!(
            RetainedChildState::try_from(StepState::Stopped),
            Ok(RetainedChildState::Stopped)
        );
    }

    /// A workflow status file with `steps` as given, written where the scan reads it.
    async fn write_workflow_status(
        async_root: &Path,
        run_id: &str,
        state: crate::background::RunState,
        steps: Vec<StepStatus>,
    ) {
        write_workflow_status_in_session(async_root, run_id, "s-1", state, steps).await;
    }

    async fn write_workflow_status_in_session(
        async_root: &Path,
        run_id: &str,
        session: &str,
        state: crate::background::RunState,
        steps: Vec<StepStatus>,
    ) {
        let run_id = RunId::from_token(run_id);
        let mut status = RunStatus::queued(run_id.clone(), RunMode::Workflow, None);
        status.session_id = crate::identity::SessionId::parse_opt(Some(session));
        status.steps = steps;
        status
            .advance_state(crate::background::RunState::Running)
            .unwrap();
        if state != crate::background::RunState::Running {
            status.advance_state(state).unwrap();
        }
        let paths = crate::background::RunPaths::for_run(async_root, async_root, &run_id);
        tokio::fs::create_dir_all(&paths.run_dir).await.unwrap();
        crate::background::atomic::write_atomic_json(&paths.status, &status)
            .await
            .unwrap();
    }

    fn step(agent: &str, run_id: &str, status: StepState, ended_at: Option<i64>) -> StepStatus {
        let mut step = StepStatus::pending(agent);
        step.run_id = Some(RunId::from_token(run_id));
        step.status = status;
        step.ended_at = ended_at;
        step
    }

    /// The scan bounds AFTER the session filter (module doc): with [`MAX_RETAINED_CHILD_CANDIDATES`]
    /// newer workflows from another session on disk, this session's one older workflow is still
    /// listed. Bounding first — the fleet's index-fallback cut — would drop it.
    #[tokio::test]
    async fn newer_runs_of_other_sessions_do_not_push_this_sessions_out_of_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mine = "wf-mine";
        write_workflow_status(
            root,
            mine,
            crate::background::RunState::Complete,
            vec![step("coder", "child-mine", StepState::Complete, Some(10))],
        )
        .await;
        set_status_mtime(root, mine, 1_000);
        for i in 0..MAX_RETAINED_CHILD_CANDIDATES {
            let id = format!("wf-other-{i}");
            write_workflow_status_in_session(
                root,
                &id,
                "s-2",
                crate::background::RunState::Complete,
                vec![step(
                    "coder",
                    &format!("child-other-{i}"),
                    StepState::Complete,
                    Some(10),
                )],
            )
            .await;
            set_status_mtime(root, &id, 2_000 + u64::try_from(i).unwrap());
        }
        let children = list_retained_children(root, root, Some("s-1"))
            .await
            .unwrap();
        assert_eq!(
            children
                .iter()
                .map(|c| c.run_id.as_str())
                .collect::<Vec<_>>(),
            vec!["child-mine"]
        );
        let theirs = list_retained_children(root, root, Some("s-2"))
            .await
            .unwrap();
        assert_eq!(theirs.len(), MAX_RETAINED_CHILD_CANDIDATES);
    }

    /// The scan orders by `status.json` mtime; pin it explicitly so the test does not depend on
    /// write order landing in distinct milliseconds.
    fn set_status_mtime(async_root: &Path, run_id: &str, secs: u64) {
        let paths = crate::background::RunPaths::for_run(
            async_root,
            async_root,
            &RunId::from_token(run_id),
        );
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&paths.status)
            .unwrap();
        file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }

    /// `:54` — an external runner is refused BEFORE the session rung: a row whose session file
    /// would pass still answers `external runner`, for both of upstream's discriminants, and a
    /// native row (no `runner`) falls through to the session rung's own answer.
    #[tokio::test]
    async fn an_external_runner_is_never_resumable() {
        let dir = tempfile::tempdir().unwrap();
        let session = dir.path().join("child.jsonl");
        std::fs::write(&session, "{}\n").unwrap();
        let run_id = RunId::from_token("wf-ext");
        let mut status = RunStatus::queued(run_id.clone(), RunMode::Workflow, None);
        status.session_file = Some(session.clone());
        for kind in ["external-cli", "external-job"] {
            let mut row = step("coder", "child-1", StepState::Complete, Some(10));
            row.runner = Some(crate::runner::status::ExternalCliRunnerStatus {
                kind: kind.to_string(),
                ..external_runner_status()
            });
            assert_eq!(
                child_resumability(dir.path(), &status, 0, &row).await,
                Resumability::not_resumable("external runner"),
                "{kind}"
            );
        }
        let native = step("coder", "child-1", StepState::Complete, Some(10));
        assert_eq!(
            child_resumability(dir.path(), &status, 0, &native).await,
            Resumability::not_resumable("missing recovery descriptor"),
            "a native row passes the runner rung and the session rung, and stops at the descriptor"
        );
    }

    /// The descriptor the external-CLI launch itself publishes (`resolve_external_cli_runner_status`),
    /// so the row carries exactly what `SingleResult::runner` would.
    fn external_runner_status() -> crate::runner::status::ExternalCliRunnerStatus {
        crate::runner::status::resolve_external_cli_runner_status(None, "claude", &[])
    }

    /// `:86-90` over the CHILD: the row's state and `endedAt` are the child's, and the workflow's
    /// own state is not a gate — a settled child of a `Running` workflow is listed, a `failed`
    /// child of a `Complete` workflow prints `failed`, and a `Running` row is not a child yet.
    #[tokio::test]
    async fn the_row_carries_the_childs_state_not_the_workflows() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("async");
        write_workflow_status(
            &root,
            "wf-running",
            crate::background::RunState::Running,
            vec![
                step(
                    "worker",
                    "child-a",
                    StepState::Complete,
                    Some(1_700_000_000_000),
                ),
                step("worker", "child-b", StepState::Running, None),
            ],
        )
        .await;
        write_workflow_status(
            &root,
            "wf-complete",
            crate::background::RunState::Complete,
            vec![step(
                "worker",
                "child-c",
                StepState::Failed,
                Some(1_700_000_001_000),
            )],
        )
        .await;

        let children = list_retained_children(&root, &root, Some("s-1"))
            .await
            .unwrap();
        let rows: Vec<(String, RetainedChildState, i64)> = children
            .iter()
            .map(|c| (c.run_id.as_str().to_string(), c.state, c.completed_at))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "child-c".to_string(),
                    RetainedChildState::Failed,
                    1_700_000_001_000
                ),
                (
                    "child-a".to_string(),
                    RetainedChildState::Complete,
                    1_700_000_000_000
                ),
            ],
            "{children:#?}"
        );
        // Another session's workflow is filtered by the scan's `sessionId` rung (`:585`).
        assert!(
            list_retained_children(&root, &root, Some("s-other"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// `:113-116` — eleven children, only the eleventh resumable: it replaces slot 10, and the
    /// fallback-challenge trailer is NOT printed.
    #[test]
    fn a_resumable_child_beyond_the_window_replaces_the_last_slot() {
        let mut children: Vec<RetainedChild> = (0..10)
            .map(|i| {
                child(
                    &format!("c{i}"),
                    1_000 - i64::from(i),
                    Resumability::not_resumable("stopped run"),
                )
            })
            .collect();
        children.push(child(
            "c10",
            0,
            Resumability::Resumable {
                session_path: PathBuf::from("/s/c10.jsonl"),
            },
        ));
        let text = format_retained_children(&children);
        let rows: Vec<&str> = text.lines().filter(|l| l.starts_with("- ")).collect();
        assert_eq!(rows.len(), MAX_RETAINED_CHILDREN);
        assert!(rows[9].starts_with("- c10 | worker | complete | 1970-01-01T00:00:00.000Z"));
        assert!(!rows.iter().any(|r| r.starts_with("- c9 ")));
        assert!(text.contains("  resumability: resumable\n  session: /s/c10.jsonl\n  resume: subagent({ action: \"resume\", id: \"wf-1\", index: 0, message: \"...\" })"));
        assert!(!text.contains("No resumable retained child is listed."));
    }

    /// `:111` and `:133` — the two fallback-challenge sentences.
    #[test]
    fn the_empty_and_no_resumable_sentences_are_pis() {
        assert_eq!(
            format_retained_children(&[]),
            "No retained workflow children in the active parent session. If a retained-writer \
             challenge is required, launch a same-role fallback challenge and label it as \
             fallback."
        );
        let text = format_retained_children(&[child(
            "c0",
            1_700_000_000_000,
            Resumability::not_resumable("missing recovery descriptor"),
        )]);
        assert_eq!(
            text,
            "Retained workflow children (up to 10; newest first, with a resumable child retained \
             when available):\n\
             - c0 | worker | complete | 2023-11-14T22:13:20.000Z\n\
             \x20 workflow: wf-1\n\
             \x20 task: (no task summary)\n\
             \x20 resumability: not resumable (missing recovery descriptor)\n\
             No resumable retained child is listed. Launch a same-role fallback challenge and \
             label it as fallback."
        );
    }

    /// `:37-50` — every session-file sentence, over real files.
    #[tokio::test]
    async fn the_session_file_rung_says_why() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            retained_session_file(None).await,
            Resumability::not_resumable("no persisted session file")
        );
        assert_eq!(
            retained_session_file(Some(&dir.path().join("s.json"))).await,
            Resumability::not_resumable("persisted session file is not a .jsonl file")
        );
        let missing = dir.path().join("gone.jsonl");
        assert_eq!(
            retained_session_file(Some(&missing)).await,
            Resumability::not_resumable(format!(
                "persisted session file is missing: {}",
                missing.display()
            ))
        );
        let regular = dir.path().join("s.jsonl");
        std::fs::write(&regular, "{}\n").unwrap();
        assert_eq!(
            retained_session_file(Some(&regular)).await,
            Resumability::Resumable {
                session_path: regular.clone()
            }
        );
        let link = dir.path().join("link.jsonl");
        std::os::unix::fs::symlink(&regular, &link).unwrap();
        assert_eq!(
            retained_session_file(Some(&link)).await,
            Resumability::not_resumable("persisted session file is not a regular file")
        );
        let as_dir = dir.path().join("d.jsonl");
        std::fs::create_dir(&as_dir).unwrap();
        assert_eq!(
            retained_session_file(Some(&as_dir)).await,
            Resumability::not_resumable("persisted session file is not a regular file")
        );
    }
}
