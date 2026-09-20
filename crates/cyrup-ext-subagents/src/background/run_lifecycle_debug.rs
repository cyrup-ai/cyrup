//! `debug.run` — the run-lifecycle diagnostic dump an operator reads when an async run is stuck:
//! pi `runs/background/run-status.ts:47-108` @v0.68.0 (`formatCapacityOwner` `:60-72`,
//! `formatWorkflowDebug` `:74-85`, `formatRunLifecycleDebug` `:87-108`), rendered over the data
//! this build actually has.
//!
//! # The process-terminal lines, and why there are two instead of upstream's three
//!
//! Upstream prints `Process terminal file: <dir>/process-terminal.json` (`:95`) and then BOTH
//! `Status process terminal:` and `Sidecar process terminal:` (`:102-103`), fed by
//! `debugProcessTerminal` (`:52-58`) — a `readProcessTerminal` of the sidecar and a
//! `sanitizeProcessTerminal` of the status's own `processTerminal` overlay. **cyrup has neither
//! the sidecar nor the overlay, neither a reader nor a writer**:
//! `grep -rn 'process-terminal\|process_terminal\|ProcessTerminal' crates/cyrup-ext-subagents/src`
//! finds only `//!`/`///`/`//` comments and string literals (`active_run_index.rs`,
//! `active_async_capacity/{inspect,key,mod,tests}.rs`, `rpc/ping.rs`, `registration/doctor.rs`) —
//! no symbol; [`crate::background::RunDir`] exposes no such path; [`RunStatus`] and
//! [`crate::background::StepStatus`] carry no `process_terminal` field, so upstream's overlay keys
//! would be dropped by serde on read and are never written. Ledger row VL-S4
//! (`docs/gap-analysis/PARITY-GAPS.md`, "Process-terminal record") records the gap and stays open;
//! this verb neither closes nor narrows it. Printing upstream's file path would name a file no
//! code path has ever written, and a diagnostic tool that names a file that does not exist is
//! lying — so the dump prints ONE line saying the record is not kept, and ONE line reporting the
//! substitute this build really has: the run's recorded runner pid, probed by
//! [`crate::background::reconcile::check_pid_liveness`], which is exactly the stand-in
//! `active_async_capacity`'s §D3 (`background/active_async_capacity/mod.rs`) already uses in
//! place of upstream's `processTerminal.state === "observed"` proof.
//!
//! # Lines upstream prints that are absent here, each because the datum does not exist
//!
//! - `Workflow parent:` / `Workflow key:` / `Lane:` (`:99-101`, and `:77`/`:79` in
//!   `formatWorkflowDebug`): [`RunStatus`] has no `parent_workflow_run_id`, no top-level
//!   `workflow_key` and no `lane` — `background/async_retention/policy.rs`'s
//!   `has_workflow_reference` records the same absence. Upstream itself omits these lines when the
//!   fields are `undefined`, so the output is upstream's own for a status where they are.
//! - `Capacity runner: <runnerProcessInstanceId>` (`:69`): cyrup mints no instance id
//!   (`active_async_capacity/key.rs`, "The one field that is NOT upstream's");
//!   [`ActiveAsyncCapacityOwner::runner_pid`] is the §D3 substitute, printed as
//!   `Capacity runner pid: <n>`.
//! - The per-step `async yes|no|unknown` term and the `lane`/`worktree`/`branch`/`provider` tail
//!   (`:82`): [`crate::background::StepStatus`] has none of those fields. The `run <id>` suffix,
//!   the witness that matters for a stuck run, is kept.
//!
//! [`ActiveAsyncCapacityOwner::runner_pid`]: crate::background::active_async_capacity::ActiveAsyncCapacityOwner::runner_pid

use crate::background::active_async_capacity::ActiveAsyncCapacityInspection;
use crate::background::active_async_capacity::inspect::liveness_word;
use crate::background::reconcile::Liveness;
use crate::background::run_status::{run_mode_label, run_state_label, step_state_label};
use crate::background::{RunMode, RunPaths, RunStatus};

/// pi `subagent-executor.ts:6532-6533` @v0.68.0 — refused before the view check.
pub const DEBUG_RUN_REQUIRES_TARGET: &str = "action='debug.run' requires id, runId, or dir.";
/// pi `subagent-executor.ts:6535-6536` — refused after the target check.
pub const DEBUG_RUN_NO_VIEWS: &str = "action='debug.run' does not support status views.";
/// pi `run-status.ts:714-721` — a run known only by its terminal result file has no directory to
/// dump.
pub const DEBUG_RUN_NEEDS_STATUS_DIR: &str =
    "Run lifecycle debug needs an async run directory with status.json.";

/// pi `formatRunLifecycleDebug`'s input (`run-status.ts:87`), over the data cyrup has.
pub struct RunLifecycleDebug<'a> {
    /// The reconciled status of the run.
    pub status: &'a RunStatus,
    /// The run's resolved paths — `Dir:` and `Status file:` are read off these, never re-derived.
    pub paths: &'a RunPaths,
    /// The pid probe standing in for upstream's process-terminal pair.
    pub runner: RunnerLiveness,
    /// pi `inspectActiveAsyncCapacityOwner`'s answer for this run (`:515`).
    pub capacity: &'a ActiveAsyncCapacityInspection,
}

/// What the dump reports where upstream reports its `sidecar`/`overlay` `ProcessTerminal` pair.
///
/// [CYRUP-DELTA] Upstream's `debugProcessTerminal` (`run-status.ts:52-58`) reads a
/// `process-terminal.json` sidecar and a `status.processTerminal` overlay; cyrup writes neither
/// and reads neither (VL-S4 — see the module doc for the grep), so the only process-terminal fact
/// this build can report is the one §D3 already substitutes for the proof in
/// `active_async_capacity::inspect::runner_release_verdict`: the run's recorded pid
/// ([`RunStatus::pid`]) and its liveness under
/// [`check_pid_liveness`](crate::background::reconcile::check_pid_liveness).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerLiveness {
    /// `status.pid` is `None` — the runner never recorded its own pid.
    NotRecorded,
    /// `status.pid` is `Some(pid)` and the probe answered.
    Probed {
        /// The pid the run recorded.
        pid: u32,
        /// The probe's answer. `Unknown` is reported as `unknown`, never as `dead`
        /// (`background/reconcile.rs`, R-SA-089).
        liveness: Liveness,
    },
}

impl RunnerLiveness {
    /// Probe `status.pid` with `probe` — production passes
    /// [`check_pid_liveness`](crate::background::reconcile::check_pid_liveness); a test injects
    /// a constant to pin the rendering of each outcome.
    pub fn probe(status: &RunStatus, probe: impl Fn(u32) -> Liveness) -> Self {
        match status.pid {
            Some(pid) => Self::Probed {
                pid,
                liveness: probe(pid),
            },
            None => Self::NotRecorded,
        }
    }

    fn line(self) -> String {
        match self {
            Self::NotRecorded => "Runner pid: not recorded".to_string(),
            Self::Probed { pid, liveness } => {
                format!("Runner pid: {pid} ({})", liveness_word(liveness))
            }
        }
    }
}

/// The one line that stands where upstream's `Process terminal file:` / `Status process
/// terminal:` / `Sidecar process terminal:` trio stands.
const PROCESS_TERMINAL_NOT_RECORDED: &str = "Process terminal: not recorded — this build writes \
                                             no process-terminal.json (VL-S4); runner pid \
                                             liveness stands in for the proof";

/// pi `formatCapacityOwner` (`run-status.ts:60-72`).
#[must_use]
pub fn format_capacity_owner(inspect: &ActiveAsyncCapacityInspection) -> Vec<String> {
    let release = format!(
        "Active capacity: {} — {}",
        inspect.release.state_word(),
        inspect.release.reason()
    );
    let Some(owner) = inspect.owner.as_ref() else {
        return vec![release];
    };
    let mut lines = vec![
        release,
        format!(
            "Capacity owner: {} slot {}, {}, generation {}",
            inspect.relation.as_str(),
            owner.slot,
            owner.kind.as_str(),
            owner.generation
        ),
        format!("Capacity session: {}", owner.owner_session_id),
    ];
    if let Some(source) = owner.source_run_id.as_ref() {
        lines.push(format!("Capacity source run: {source}"));
    }
    lines.push(format!("Capacity async dir: {}", owner.async_dir.display()));
    // pi `:69` prints `Capacity runner: <runnerProcessInstanceId>`; cyrup mints no instance id
    // and binds the runner's real pid instead (`key.rs`), so the line says what it holds.
    if let Some(pid) = owner.runner_pid {
        lines.push(format!("Capacity runner pid: {pid}"));
    }
    if let Some(started_at) = owner.runner_started_at {
        lines.push(format!(
            "Capacity runner started: {}",
            crate::time::format_iso8601_millis(started_at)
        ));
    }
    lines
}

/// pi `formatWorkflowDebug` (`run-status.ts:74-85`).
///
/// The gate (`:75`, `mode === "workflow" || parentWorkflowRunId || workflowKey || lane`) is
/// `has_workflow_reference`'s three disjuncts (`background/async_retention/policy.rs`): the mode,
/// the run's own child inventory, or any step carrying a workflow key — the three places a
/// workflow reference lives on cyrup's [`RunStatus`].
#[must_use]
pub fn format_workflow_debug(status: &RunStatus) -> Vec<String> {
    let has_workflow_reference = status.mode == RunMode::Workflow
        || status.workflow_children.is_some()
        || status.steps.iter().any(|step| step.workflow_key.is_some());
    if !has_workflow_reference {
        return Vec::new();
    }
    let mut lines = Vec::new();
    if status.mode == RunMode::Workflow {
        lines.push(format!("Workflow children: {}", status.steps.len()));
    }
    for (index, step) in status.steps.iter().enumerate() {
        let key = step
            .workflow_key
            .as_ref()
            .map_or_else(|| "n/a".to_string(), ToString::to_string);
        // pi `runStatusStepDisplayName` (`:166-168`): the trimmed session name, else the agent.
        // (Upstream's middle rung, `label (agent)`, has no `StepStatus::label` to read.)
        let display_name = step
            .session_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(step.agent.as_str());
        let mut line = format!(
            "  {}. key {key} · {display_name} · {}",
            index + 1,
            step_state_label(step.status)
        );
        if let Some(run_id) = step.run_id.as_ref() {
            line.push_str(&format!(" · run {run_id}"));
        }
        lines.push(line);
    }
    lines
}

/// pi `formatRunLifecycleDebug` (`run-status.ts:87-108`), in upstream's line order, with the
/// two process-terminal lines the module doc explains in place of upstream's three.
#[must_use]
pub fn format_run_lifecycle_debug(input: &RunLifecycleDebug<'_>) -> String {
    let RunLifecycleDebug {
        status,
        paths,
        runner,
        capacity,
    } = input;
    let mut lines = vec![
        "Run lifecycle debug".to_string(),
        format!("Run: {}", status.run_id),
        format!("Dir: {}", paths.run_dir.display()),
        format!("Status file: {}", paths.status.display()),
    ];
    if let Some(receipt) = status.workflow_receipt_path.as_ref() {
        lines.push(format!("Workflow receipt: {}", receipt.display()));
    }
    lines.push(format!(
        "Session: {}",
        status
            .session_id
            .as_ref()
            .map_or("unknown", crate::identity::SessionId::as_str)
    ));
    lines.push(format!("State: {}", run_state_label(status.state)));
    lines.push(format!("Mode: {}", run_mode_label(status.mode)));
    lines.push(PROCESS_TERMINAL_NOT_RECORDED.to_string());
    lines.push(runner.line());
    lines.extend(format_capacity_owner(capacity));
    lines.extend(format_workflow_debug(status));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::background::active_async_capacity::{
        ActiveAsyncCapacityReleaseVerdict, CapacityRelation,
    };
    use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus};
    use std::path::Path;

    fn not_owned() -> ActiveAsyncCapacityInspection {
        ActiveAsyncCapacityInspection {
            owner: None,
            relation: CapacityRelation::None,
            slot_dir: None,
            release: ActiveAsyncCapacityReleaseVerdict::NotOwned {
                reason: "no active-capacity slot records this run".to_string(),
            },
        }
    }

    fn render(status: &RunStatus, runner: RunnerLiveness) -> String {
        let paths = RunPaths::for_run(Path::new("/async"), Path::new("/results"), &status.run_id);
        let capacity = not_owned();
        format_run_lifecycle_debug(&RunLifecycleDebug {
            status,
            paths: &paths,
            runner,
            capacity: &capacity,
        })
    }

    fn status(mode: RunMode, pid: Option<u32>) -> RunStatus {
        RunStatus::queued(RunId::from_token("run-x".to_string()), mode, pid)
    }

    /// The honesty pin: a status with no pid says so, and an `Unknown` probe is NEVER rendered as
    /// `dead` (`background/reconcile.rs`, R-SA-089).
    #[test]
    fn the_pid_probe_reports_exactly_what_it_saw() {
        let no_pid = status(RunMode::Single, None);
        let text = render(&no_pid, RunnerLiveness::probe(&no_pid, |_| Liveness::Dead));
        assert!(
            text.lines().any(|line| line == "Runner pid: not recorded"),
            "{text}"
        );

        let with_pid = status(RunMode::Single, Some(7));
        let text = render(
            &with_pid,
            RunnerLiveness::probe(&with_pid, |pid| {
                assert_eq!(pid, 7);
                Liveness::Unknown
            }),
        );
        assert!(
            text.lines().any(|line| line == "Runner pid: 7 (unknown)"),
            "{text}"
        );
        assert!(!text.contains("dead"), "{text}");
        assert!(
            text.lines()
                .any(|line| line.starts_with("Process terminal: not recorded")),
            "{text}"
        );
        assert!(
            !text
                .lines()
                .any(|line| line.starts_with("Process terminal file:")),
            "the dump must never name a sidecar this build does not write: {text}"
        );
    }

    /// Upstream's line order (`run-status.ts:89-107`) over a single-mode status: no workflow
    /// block at all (`:75`'s gate).
    #[test]
    fn a_single_mode_status_renders_the_header_block_and_no_workflow_block() {
        let mut single = status(RunMode::Single, Some(9));
        single.session_id = crate::identity::SessionId::parse("sess-1");
        single.state = RunState::Running;
        let text = render(&single, RunnerLiveness::probe(&single, |_| Liveness::Alive));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            vec![
                "Run lifecycle debug",
                "Run: run-x",
                "Dir: /async/run-x",
                "Status file: /async/run-x/status.json",
                "Session: sess-1",
                "State: running",
                "Mode: single",
                PROCESS_TERMINAL_NOT_RECORDED,
                "Runner pid: 9 (alive)",
                "Active capacity: not-owned — no active-capacity slot records this run",
            ]
        );
    }

    /// `formatWorkflowDebug` (`:74-85`): a workflow-mode status lists its children count and one
    /// line per step, with the session name preferred over the agent and the child run id as the
    /// suffix.
    #[test]
    fn a_workflow_status_renders_its_children_and_one_line_per_step() {
        let mut workflow = status(RunMode::Workflow, None);
        let mut first = StepStatus::pending("worker");
        first.status = StepState::Complete;
        first.workflow_key = crate::workflows::WorkflowKey::parse("a").ok();
        first.run_id = Some(RunId::from_token("child-a".to_string()));
        let mut second = StepStatus::pending("reviewer");
        second.status = StepState::Running;
        second.session_name = Some("  Review pass  ".to_string());
        workflow.steps = vec![first, second];
        workflow.workflow_receipt_path = Some("/receipts/run-x.json".into());

        let text = render(&workflow, RunnerLiveness::NotRecorded);
        assert!(
            text.lines()
                .any(|line| line == "Workflow receipt: /receipts/run-x.json"),
            "{text}"
        );
        assert!(
            text.lines().any(|line| line == "Session: unknown"),
            "{text}"
        );
        assert!(text.lines().any(|line| line == "Mode: workflow"), "{text}");
        let tail: Vec<&str> = text
            .lines()
            .rev()
            .take(3)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        assert_eq!(
            tail,
            vec![
                "Workflow children: 2",
                "  1. key a · worker · complete · run child-a",
                "  2. key n/a · Review pass · running",
            ]
        );
    }
}
