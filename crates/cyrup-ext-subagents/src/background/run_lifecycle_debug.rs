//! `debug.run` — the run-lifecycle diagnostic dump an operator reads when an async run is stuck:
//! pi `runs/background/run-status.ts:47-108` @v0.68.0 (`formatProcessTerminal` `:47-50`,
//! `debugProcessTerminal` `:52-58`, `formatCapacityOwner` `:60-72`, `formatWorkflowDebug`
//! `:74-85`, `formatRunLifecycleDebug` `:87-108`).
//!
//! # The three process-terminal lines, and why the pair is not a duplicate
//!
//! Upstream prints `Process terminal file: <dir>/process-terminal.json` (`:95`) and then BOTH
//! `Status process terminal:` and `Sidecar process terminal:` (`:102-103`), fed by
//! [`debug_process_terminal`] — pi's `debugProcessTerminal` (`:52-58`): a
//! [`read_process_terminal`](crate::background::process_terminal::read_process_terminal) of the
//! sidecar and the status's own `processTerminal` overlay, both read against the same expectation
//! (`:53`: this run's id, and the runner instance the status itself names).
//!
//! This build writes and reads both ([`crate::background::process_terminal`]), so all three lines
//! are printed and the file the first one names is a file the launch really wrote — before the
//! runner was spawned, so it exists even for a run that died in its first millisecond.
//!
//! The two lines answer different questions, which is why upstream prints both and why neither
//! may be dropped as redundant:
//!
//! - The **sidecar** is the artifact
//!   [`finalize_process_terminal`](crate::background::process_terminal::finalize_process_terminal)
//!   wrote, and it is the one every other consumer reads (the capacity release rung, the
//!   active-run index).
//! - The **overlay** is the copy
//!   [`overlay_status`](crate::background::process_terminal::overlay_status) left on
//!   `status.json` in the same close.
//!
//! So a DISAGREEMENT between them is itself the diagnosis: an `observed` sidecar under a
//! `pending` overlay says the proof landed and the status write that follows it did not, and a
//! `pending` sidecar with no overlay at all says the runner never reached its close — which is
//! exactly the crash-versus-slow-start question an operator opens this dump to answer.
//!
//! # Lines upstream prints that are absent here, each because the datum does not exist
//!
//! - `Workflow parent:` / `Workflow key:` / `Lane:` (`:99-101`, and `:77`/`:79` in
//!   `formatWorkflowDebug`): [`RunStatus`] has no `parent_workflow_run_id`, no top-level
//!   `workflow_key` and no `lane` — `background/async_retention/policy.rs`'s
//!   `has_workflow_reference` records the same absence. Upstream itself omits these lines when the
//!   fields are `undefined`, so the output is upstream's own for a status where they are.
//! - The per-step `async yes|no|unknown` term and the `lane`/`worktree`/`branch`/`provider` tail
//!   (`:82`): [`crate::background::StepStatus`] has none of those fields. The `run <id>` suffix,
//!   the witness that matters for a stuck run, is kept.

use crate::background::active_async_capacity::ActiveAsyncCapacityInspection;
use crate::background::process_terminal::{
    ProcessTerminal, ProcessTerminalError, ProcessTerminalReason, ProofExpectation,
    read_process_terminal, unknown_proof,
};
use crate::background::run_status::{run_mode_label, run_state_label, step_state_label};
use crate::background::{RunDir, RunMode, RunPaths, RunStatus};

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
    /// pi `sidecarProcessTerminal` (`:87`) — [`DebugProcessTerminal::sidecar`].
    pub sidecar: Option<&'a ProcessTerminal>,
    /// pi `overlayProcessTerminal` (`:87`) — [`DebugProcessTerminal::overlay`].
    pub overlay: Option<&'a ProcessTerminal>,
    /// pi `inspectActiveAsyncCapacityOwner`'s answer for this run (`:515`).
    pub capacity: &'a ActiveAsyncCapacityInspection,
}

/// pi `debugProcessTerminal`'s return shape (`run-status.ts:52`) — the two process-terminal
/// records the dump prints, read in one pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DebugProcessTerminal {
    /// `<run_dir>/process-terminal.json` as read back (`:55`). `None` means the file is not there
    /// at all, which is a distinct answer from an unreadable one: a run that never reached
    /// [`initialize_process_terminal`](crate::background::process_terminal::initialize_process_terminal)
    /// has no sidecar, and the dump says `missing` rather than inventing a state.
    pub sidecar: Option<ProcessTerminal>,
    /// `status.processTerminal` (`:56`), checked against the same expectation.
    pub overlay: Option<ProcessTerminal>,
}

/// pi `debugProcessTerminal` (`run-status.ts:52-58`) — read both halves against one expectation.
///
/// # `[CYRUP-DELTA]` — the overlay's sanitize already happened, one layer lower
///
/// Upstream's `AsyncStatus.processTerminal` is untyped JSON, so `:56` runs
/// `sanitizeProcessTerminal` over it here, at read time. cyrup's
/// [`RunStatus::process_terminal`](crate::background::RunStatus::process_terminal) is a typed
/// field whose own decoder (`process_terminal::deserialize_overlay`) IS that call: a corrupt
/// `processTerminal` key became an `unknown` / [`ProcessTerminalReason::ProofWriteFailed`] proof
/// at the moment `status.json` was parsed, which is the only moment a Rust reader gets, and the
/// status still parsed.
///
/// What a serde field decoder cannot do is upstream's `:53` expectation: it is handed no access
/// to the sibling `runId`. So the one check upstream makes here that the decoder could not is
/// re-applied here, over the decoded value, with upstream's own refusal sentence as the
/// diagnostic — an overlay naming ANOTHER run is a `status.json` copied or restored from a
/// different run, and reporting its state as this run's would be a lie about which process the
/// dump is describing.
pub async fn debug_process_terminal(run_dir: &RunDir, status: &RunStatus) -> DebugProcessTerminal {
    // pi `:53` — `{ runId: status.runId, runnerProcessInstanceId: status.processTerminal?.… }`.
    // The instance half is taken from the overlay ITSELF, so it constrains only the sidecar: a
    // sidecar written by a different runner than the one the status names is not this run's proof.
    let expected = ProofExpectation {
        run_id: Some(&status.run_id),
        runner_process_instance_id: status
            .process_terminal
            .as_ref()
            .map(ProcessTerminal::runner_process_instance_id),
    };
    DebugProcessTerminal {
        sidecar: read_process_terminal(run_dir, expected).await,
        overlay: status.process_terminal.as_ref().map(|proof| {
            if proof.run_id() == &status.run_id {
                return proof.clone();
            }
            // pi `unknownProof(fallback.runId, fallback.runnerProcessInstanceId, …)` (`:186`):
            // the degraded record is attributed to the run the READER expected, never to the one
            // the bad overlay claimed.
            unknown_proof(
                status.run_id.clone(),
                proof.runner_process_instance_id().clone(),
                ProcessTerminalReason::ProofWriteFailed,
                Some(
                    ProcessTerminalError::ProofRunMismatch {
                        label: run_dir.status().display().to_string(),
                        actual: proof.run_id().as_str().to_string(),
                        expected: status.run_id.as_str().to_string(),
                    }
                    .to_string(),
                ),
            )
        }),
    }
}

/// pi `formatProcessTerminal` (`run-status.ts:47-50`) — `{state}{ (reason)}{ · runner {id}}`, and
/// `missing` for a record that is not there.
#[must_use]
pub fn format_process_terminal(value: Option<&ProcessTerminal>) -> String {
    let Some(value) = value else {
        return "missing".to_string();
    };
    let mut line = value.state().as_str().to_string();
    if let Some(reason) = value.reason() {
        line.push_str(&format!(" ({})", reason.as_str()));
    }
    // pi's third term is guarded by `value.runnerProcessInstanceId ?`; cyrup's field is non-empty
    // BY TYPE (every proof carries the instance it belongs to), so the guard is the type's.
    line.push_str(&format!(" · runner {}", value.runner_process_instance_id()));
    line
}

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
    // pi `:69` — the identity the release verdict matches the proof against, bound by
    // `mark_started` once the spawn is confirmed.
    if let Some(instance) = owner.runner_process_instance_id.as_ref() {
        lines.push(format!("Capacity runner: {instance}"));
    }
    // [CYRUP-DELTA] Upstream has no runner pid to print: its runner is a child it keeps a handle
    // to. cyrup's is detached (`crates/cyrup/src/subagent_runner_cmd.rs:1-7`), so the pid is the
    // input to the no-proof fallback ladder beneath the proof rung
    // (`active_async_capacity/key.rs`'s `runner_pid` block), and an operator reading this dump
    // about a run whose sidecar is still `pending` needs the pid that ladder is probing.
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

/// pi `formatRunLifecycleDebug` (`run-status.ts:87-108`), in upstream's line order.
#[must_use]
pub fn format_run_lifecycle_debug(input: &RunLifecycleDebug<'_>) -> String {
    let RunLifecycleDebug {
        status,
        paths,
        sidecar,
        overlay,
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
    // pi `:95` — the sidecar's path, spelled by its one accessor so the dump can never name a
    // file the writer does not write.
    lines.push(format!(
        "Process terminal file: {}",
        RunDir::for_existing(&paths.run_dir)
            .process_terminal()
            .display()
    ));
    lines.push(format!(
        "Session: {}",
        status
            .session_id
            .as_ref()
            .map_or("unknown", crate::identity::SessionId::as_str)
    ));
    lines.push(format!("State: {}", run_state_label(status.state)));
    lines.push(format!("Mode: {}", run_mode_label(status.mode)));
    // pi `:102-103`, in upstream's order: the status's own copy first, then the artifact.
    lines.push(format!(
        "Status process terminal: {}",
        format_process_terminal(*overlay)
    ));
    lines.push(format!(
        "Sidecar process terminal: {}",
        format_process_terminal(*sidecar)
    ));
    lines.extend(format_capacity_owner(capacity));
    lines.extend(format_workflow_debug(status));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::active_async_capacity::{
        ActiveAsyncCapacityReleaseVerdict, CapacityRelation,
    };
    use crate::background::process_terminal::{
        ProcessInstanceExit, ProcessTerminalBase, ProcessTerminalState, RunnerProcessInstanceId,
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

    fn render(
        status: &RunStatus,
        sidecar: Option<&ProcessTerminal>,
        overlay: Option<&ProcessTerminal>,
    ) -> String {
        let paths = RunPaths::for_run(Path::new("/async"), Path::new("/results"), &status.run_id);
        let capacity = not_owned();
        format_run_lifecycle_debug(&RunLifecycleDebug {
            status,
            paths: &paths,
            sidecar,
            overlay,
            capacity: &capacity,
        })
    }

    fn status(mode: RunMode, pid: Option<u32>) -> RunStatus {
        RunStatus::queued(RunId::from_token("run-x".to_string()), mode, pid)
    }

    fn instance() -> RunnerProcessInstanceId {
        RunnerProcessInstanceId::from_token("inst-1".to_string())
    }

    fn base(run_id: &str) -> ProcessTerminalBase {
        ProcessTerminalBase::new(RunId::from_token(run_id.to_string()), instance())
    }

    fn observed(run_id: &str) -> ProcessTerminal {
        ProcessTerminal::Observed {
            base: base(run_id),
            observed_at: 1_700_000_000_000,
            instances: vec![ProcessInstanceExit::Runner {
                process_instance_id: instance(),
                close_observed_at: 1_700_000_000_000,
                exit_code: Some(0),
                signal: None,
            }],
            canonical_session: None,
        }
    }

    /// pi `formatProcessTerminal` (`:47-50`), all three terms: the absent record's word, the bare
    /// state, the `(reason)` parenthetical only the `unknown` arm has, and the ` · runner <id>`
    /// tail that says WHICH runner the record is about.
    #[test]
    fn format_process_terminal_renders_upstreams_three_terms() {
        assert_eq!(format_process_terminal(None), "missing");
        assert_eq!(
            format_process_terminal(Some(&ProcessTerminal::Pending {
                base: base("run-x")
            })),
            "pending · runner inst-1"
        );
        assert_eq!(
            format_process_terminal(Some(&ProcessTerminal::Unknown {
                base: base("run-x"),
                reason: ProcessTerminalReason::WriterCloseUnverified,
                diagnostic: None,
            })),
            "unknown (writer-close-unverified) · runner inst-1"
        );
        assert_eq!(
            format_process_terminal(Some(&observed("run-x"))),
            "observed · runner inst-1"
        );
    }

    /// The dump names the sidecar file (pi `:95`) and prints BOTH records (pi `:102-103`), and
    /// the disagreement between them is visible: an `observed` sidecar under a `pending` overlay
    /// is a proof that landed while the status write behind it did not.
    #[test]
    fn the_dump_names_the_sidecar_and_prints_both_records() {
        let mut run = status(RunMode::Single, Some(7));
        run.process_terminal = Some(ProcessTerminal::Pending {
            base: base("run-x"),
        });
        let sidecar = observed("run-x");
        let text = render(&run, Some(&sidecar), run.process_terminal.as_ref());

        assert!(
            text.lines()
                .any(|line| line == "Process terminal file: /async/run-x/process-terminal.json"),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line == "Status process terminal: pending · runner inst-1"),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line == "Sidecar process terminal: observed · runner inst-1"),
            "{text}"
        );
        assert!(
            !text.contains("Process terminal: not recorded"),
            "the substitute line is gone, and so is the premise under it: {text}"
        );
    }

    /// A run that never reached `initialize_process_terminal` has neither record, and the dump
    /// says `missing` twice rather than inventing a state for either.
    #[test]
    fn an_absent_pair_renders_missing_on_both_lines() {
        let run = status(RunMode::Single, None);
        let text = render(&run, None, None);
        assert!(
            text.lines()
                .any(|line| line == "Status process terminal: missing"),
            "{text}"
        );
        assert!(
            text.lines()
                .any(|line| line == "Sidecar process terminal: missing"),
            "{text}"
        );
    }

    /// Upstream's line order (`run-status.ts:89-107`) over a single-mode status: no workflow
    /// block at all (`:75`'s gate), and the three process-terminal lines in their places — the
    /// file path before `Session:`, the pair after `Mode:`.
    #[test]
    fn a_single_mode_status_renders_the_header_block_and_no_workflow_block() {
        let mut single = status(RunMode::Single, Some(9));
        single.session_id = crate::identity::SessionId::parse("sess-1");
        single.state = RunState::Running;
        let sidecar = ProcessTerminal::Pending {
            base: base("run-x"),
        };
        let text = render(&single, Some(&sidecar), None);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines,
            vec![
                "Run lifecycle debug",
                "Run: run-x",
                "Dir: /async/run-x",
                "Status file: /async/run-x/status.json",
                "Process terminal file: /async/run-x/process-terminal.json",
                "Session: sess-1",
                "State: running",
                "Mode: single",
                "Status process terminal: missing",
                "Sidecar process terminal: pending · runner inst-1",
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

        let text = render(&workflow, None, None);
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

    /// pi `debugProcessTerminal` (`:52-58`) over a real run directory: the sidecar is read off
    /// disk and the overlay comes off the status, both against `:53`'s expectation.
    #[tokio::test]
    async fn debug_process_terminal_reads_the_sidecar_and_the_overlay() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run_dir = RunDir::for_existing(tmp.path());
        let mut run = status(RunMode::Single, Some(11));
        run.process_terminal = Some(ProcessTerminal::Pending {
            base: base("run-x"),
        });
        tokio::fs::write(
            run_dir.process_terminal(),
            serde_json::to_vec(&observed("run-x")).expect("encode"),
        )
        .await
        .expect("write sidecar");

        let pair = debug_process_terminal(&run_dir, &run).await;
        assert_eq!(
            pair.sidecar.as_ref().map(ProcessTerminal::state),
            Some(ProcessTerminalState::Observed)
        );
        assert_eq!(
            pair.overlay.as_ref().map(ProcessTerminal::state),
            Some(ProcessTerminalState::Pending)
        );
    }

    /// pi `:53`'s expectation is not decoration. A sidecar belonging to ANOTHER run — a
    /// `process-terminal.json` copied or restored into this directory — is refused at the read
    /// and degrades to `unknown (proof-write-failed)`, so the dump can never report another
    /// run's close as this one's.
    #[tokio::test]
    async fn a_sidecar_from_another_run_degrades_instead_of_being_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run_dir = RunDir::for_existing(tmp.path());
        let run = status(RunMode::Single, None);
        tokio::fs::write(
            run_dir.process_terminal(),
            serde_json::to_vec(&observed("some-other-run")).expect("encode"),
        )
        .await
        .expect("write sidecar");

        let pair = debug_process_terminal(&run_dir, &run).await;
        let sidecar = pair
            .sidecar
            .expect("a file is present, so the read answers");
        assert_eq!(
            sidecar.reason(),
            Some(ProcessTerminalReason::ProofWriteFailed)
        );
        assert!(
            format_process_terminal(Some(&sidecar)).starts_with("unknown (proof-write-failed)"),
            "{sidecar:?}"
        );
    }

    /// The overlay half of `:53`, which the field decoder cannot make: an overlay naming another
    /// run is a `status.json` that came from somewhere else, and the dump degrades it rather than
    /// reporting its state as this run's.
    #[tokio::test]
    async fn an_overlay_from_another_run_degrades_instead_of_being_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run_dir = RunDir::for_existing(tmp.path());
        let mut run = status(RunMode::Single, None);
        run.process_terminal = Some(observed("some-other-run"));

        let pair = debug_process_terminal(&run_dir, &run).await;
        let overlay = pair.overlay.expect("the status carries a record");
        assert_eq!(
            overlay.reason(),
            Some(ProcessTerminalReason::ProofWriteFailed)
        );
        // The degraded record is attributed to the run the READER expected (pi `:186`).
        assert_eq!(overlay.run_id().as_str(), "run-x");
        let ProcessTerminal::Unknown { diagnostic, .. } = &overlay else {
            panic!("a degraded overlay is always the unknown arm: {overlay:?}");
        };
        assert!(
            diagnostic
                .as_deref()
                .is_some_and(|text| text.contains("belongs to run 'some-other-run'")),
            "{diagnostic:?}"
        );
    }
}
