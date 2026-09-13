//! RunMode (func-SA §4.5)
//! RunState / StepState — monotone-forward-only lifecycle (func-SA §4.5)
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

/// Which shape of run this is — mirrors [`crate::spawn::chain_graph::RunnerStep`]'s three-way
/// discriminant at the whole-run granularity rather than the per-step granularity: a `Chain` run
/// may itself contain `ParallelGroup`/`DynamicGroup` steps internally, but the run *as a whole* is
/// tagged by its outermost shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunMode {
    /// One agent invocation, no fan-out.
    Single,
    /// A static-width parallel fan-out group, run standalone (not as one step of a larger chain).
    Parallel,
    /// A linear sequence of steps (`ChainGraph`), each possibly itself a `ParallelGroup`/
    /// `DynamicGroup`.
    Chain,
    /// A declarative multi-child graph driven by a workflow script — pi `SubagentRunMode`'s
    /// `"workflow"` (`shared/types.ts:400`).
    ///
    /// Produced exclusively by the FOREGROUND arm (`route_workflow_mode` in
    /// `extension/tool/routing.rs`); the background runner (`background/runner_main/`) never
    /// emits this mode (§0.6: async detachment was cut, `runs.all` already supplies real
    /// concurrency). The existing "steps are discovered rather than declared" paragraph below
    /// is exactly what `chain_step_count: None` + §3.3's `workflow_step_statuses` implement.
    ///
    /// Distinct from [`Self::Chain`], which is a linear list this crate's own runner walks: a
    /// workflow is driven by a SCRIPT that decides at runtime which children to launch, in what
    /// shape, and how to react to each result. The run's steps are therefore discovered as it
    /// executes rather than declared up front, which is why a workflow carries a separate child
    /// INVENTORY ([`crate::workflows::WorkflowChildSummary`]) that a `Chain` does not need — a
    /// `Chain`'s `RunStatus::steps` IS its inventory, a workflow's is not.
    Workflow,
}

/// The overall lifecycle state of a background run (func-SA §4.5's `RunStatus.state`).
///
/// Transitions are **monotone-forward-only**: once a run reaches a given state, it may only move
/// to a state ranked strictly higher by [`RunState::rank`], with the sole documented exception of
/// `Paused -> Running` (interrupt-then-resume, R-SA-084/086, which resumes the *same* logical run
/// record in place rather than minting a new one) and `Paused` itself being reachable from
/// `Running` (interrupt, R-SA-084) even though `Paused` does not rank above `Running` in the
/// terminal-progress sense — see [`RunState::can_transition_to`] for the exact, non-linear
/// adjacency this enum actually permits; "monotone-forward" here means "never silently regresses
/// past a state some reader may already have observed and acted on as final", not a strict total
/// order.
///
/// # Enforcement (R-SA-075/077's forward-only requirement)
///
/// `RunState` is a plain, freely-constructible `enum` — not a family of distinct marker types —
/// because [`RunStatus`](crate::background::RunStatus) must remain a single, uniformly `serde`-(de)serializable struct that
/// round-trips through `status.json` regardless of which state it currently holds (a type-state
/// encoding, with one Rust type per lifecycle state, would make `RunStatus` itself generic over
/// state and break that uniform (de)serialization contract, plus every `HashMap<RunId,
/// RunStatus>`/`Vec<RunStatus>` call site elsewhere in this subsystem). Forward-only-ness is
/// therefore enforced **at the mutation boundary**, not at the type level: every in-process state
/// change MUST go through [`RunState::try_advance`] (which returns `Err` on a disallowed
/// transition rather than silently permitting it), and every on-disk write MUST go through
/// [`RunStatus::advance_state`](crate::background::RunStatus::advance_state), which calls `try_advance` before touching `self.state`. A caller
/// that bypasses these methods and assigns `.state` directly defeats the guard — by convention,
/// no code in this crate does that (`rg 'run_status.state =' -- '!*/background/mod.rs'` prior to a
/// merge is the intended lint until a stricter type-level encoding is judged worth the
/// serialization complexity it would add).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunState {
    /// Minted and about to be spawned, but the detached runner has not yet written its own
    /// initial status (covers the R-SA-090 grace window before hop-2 confirms).
    Queued,
    /// The detached runner has written its initial `status.json` and is actively executing steps.
    Running,
    /// An interrupt was consumed (R-SA-084): every then-running step was marked `Paused`, not
    /// `Failed`. A soft, resumable, non-terminal state.
    Paused,
    /// Every step finished without a run-ending failure.
    Complete,
    /// The run ended in failure — either a step's own failure or a synthesized failure from
    /// stale-dead reconciliation (R-SA-092).
    Failed,
    /// G77 — pi `stopRunner` (`runs/background/subagent-runner.ts:2955-2984` @v0.43.0): an explicit
    /// user/agent **stop** request (`control/stop.json`, pi `StopRequest`
    /// `runs/background/control-channel.ts:49-53`) was consumed. Every then-running-or-pending step
    /// was marked [`StepState::Stopped`] with `exitCode: 1` and the literal
    /// [`STOP_MESSAGE`](crate::background::control::STOP_MESSAGE), and the run's terminal record is
    /// `state: "stopped"` (pi `statusPayload.state = "stopped"`, `subagent-runner.ts:2959`).
    ///
    /// **A first-class terminal state, NOT an alias for [`Self::Failed`] or [`Self::Paused`].**
    /// Upstream distinguishes all three at every reader: `stopRunner`'s own guard is
    /// `if (stopped || timedOut || interrupted || state !== "running") return`
    /// (`subagent-runner.ts:2955-2986`), so a stop and a timeout and an interrupt are mutually exclusive
    /// verdicts; `resolveSubagentResultStatus` ranks `"stopped"` above `"paused"` and above the
    /// `success`/exit-code fallbacks (`intercom/result-intercom.ts:31-35`);
    /// `resolveGroupedStatus` gives `"stopped"` its own precedence slot between `"failed"` and
    /// `"paused"` (`result-intercom.ts:84-87`); `async-resume.ts:406` REFUSES to resume a stopped
    /// run (`"was stopped and cannot be resumed"`) where a paused one is exactly what `resume`
    /// exists for; and `notify.ts:210` renders a fourth `status` word for it. Folding `Stopped`
    /// into `Failed` would silently make a stopped run look resumable-or-not identically to a
    /// crashed one and would erase the distinct user-visible string at every one of those sites.
    Stopped,
}

impl RunState {
    /// A rank used only to describe "how far along" a state is for documentation/diagnostic
    /// purposes (e.g. UI ordering). **Not** consulted by [`RunState::can_transition_to`] — the
    /// actual transition table is the explicit adjacency list below, because the true allowed-
    /// transition graph is not a simple linear order (`Paused` can both follow `Running` and
    /// precede a fresh `Running` again on resume, R-SA-086).
    #[must_use]
    pub fn rank(self) -> u8 {
        match self {
            RunState::Queued => 0,
            RunState::Running => 1,
            RunState::Paused => 2,
            RunState::Complete => 3,
            RunState::Failed => 3,
            RunState::Stopped => 3,
        }
    }

    /// `true` once a run in this state will never again be mutated by the runner that owns it
    /// (`Complete`/`Failed`/`Stopped`). `Paused` is deliberately **not** terminal — R-SA-084 is
    /// explicit that interrupt is a soft, resumable pause, never a terminal state.
    ///
    /// [`RunState::Stopped`] **is** terminal in its own right (G77): pi treats a stopped run as
    /// finished-and-non-resumable (`async-resume.ts:406` throws rather than reviving it,
    /// `chain-root-attachment.ts:60`'s `TERMINAL_STATES` set contains `"stopped"`, and
    /// `stale-run-reconciler.ts:292`'s `isTerminalState` returns true for it) — it is neither a
    /// pause nor a failure.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            RunState::Complete | RunState::Failed | RunState::Stopped
        )
    }

    /// Returns `true` if a transition from `self` to `next` is permitted.
    ///
    /// The explicit adjacency (not a derived `<` on [`RunState::rank`]) is:
    /// - `Queued -> Running | Failed` (spawn confirmed and the runner took over; or the runner
    ///   itself failed to even start, e.g. a spawn-level I/O error surfaced before any step ran).
    /// - `Running -> Paused | Complete | Failed` (interrupt; normal completion; step/stale-dead
    ///   failure).
    /// - `Paused -> Running | Failed` (resume respawns and steers execution forward again,
    ///   R-SA-086; or a paused run is later reconciled to `Failed` by long-staleness reconciliation,
    ///   R-SA-091, if it is never resumed).
    /// - `Queued -> Stopped` / `Running -> Stopped` (G77): an explicit stop request was consumed.
    ///   Upstream's `stopRunner` gate is `statusPayload.state !== "running"` returns early
    ///   (`subagent-runner.ts:2955-2986`), and the parent-side `stopAsyncRun` only accepts a target whose
    ///   reconciled state is `"running"` or `"queued"` (`async-stop-action.ts:41`) — so those are
    ///   exactly the two predecessors, and a `Paused` run is deliberately NOT stoppable (upstream
    ///   returns `"No running or queued async run was found"` for it).
    /// - `Complete`/`Failed`/`Stopped` are terminal: no outgoing transition is permitted, including
    ///   to themselves — a caller that already observed a terminal state and tries to write the same
    ///   terminal state again should treat that as a no-op at a layer above this guard, not as a
    ///   fresh "transition".
    #[must_use]
    pub fn can_transition_to(self, next: RunState) -> bool {
        matches!(
            (self, next),
            (RunState::Queued, RunState::Running)
                | (RunState::Queued, RunState::Failed)
                | (RunState::Queued, RunState::Stopped)
                | (RunState::Running, RunState::Paused)
                | (RunState::Running, RunState::Complete)
                | (RunState::Running, RunState::Failed)
                | (RunState::Running, RunState::Stopped)
                | (RunState::Paused, RunState::Running)
                | (RunState::Paused, RunState::Failed)
        )
    }

    /// Attempts to advance from `self` to `next`, returning the new state on success or
    /// [`RunStateTransitionError`] if the transition is not permitted by
    /// [`RunState::can_transition_to`]. This is the single choke point every in-process state
    /// mutation in this subsystem MUST route through (see the enforcement note on the type
    /// itself).
    ///
    /// # Errors
    ///
    /// Returns [`RunStateTransitionError`] if `next` is not reachable from `self`.
    pub fn try_advance(self, next: RunState) -> Result<RunState, RunStateTransitionError> {
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(RunStateTransitionError {
                from: self,
                to: next,
            })
        }
    }
}

impl Default for RunState {
    /// A freshly minted run always starts `Queued` (before the detached runner has written its
    /// own initial status).
    fn default() -> Self {
        RunState::Queued
    }
}

/// A disallowed [`RunState`] transition was attempted (forward-only enforcement, R-SA-075/077).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("illegal run-state transition: {from:?} -> {to:?}")]
pub struct RunStateTransitionError {
    /// The state the transition was attempted from.
    pub from: RunState,
    /// The state the transition attempted to reach.
    pub to: RunState,
}

/// The lifecycle state of one step within a run (func-SA §4.5's `StepStatus.status`).
///
/// Distinct from [`RunState`]: a step starts `Pending` (not yet dispatched) rather than `Queued`,
/// and has no `Paused` predecessor requirement — a step is marked `Paused` directly from
/// `Running` on interrupt (R-SA-084), mirroring but not reusing `RunState`'s transition table,
/// since a step never independently resumes (resume re-selects and re-drives it via the parent
/// run, R-SA-086) the way a whole run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StepState {
    /// Declared (present in the chain's step list) but not yet dispatched.
    Pending,
    /// A child subprocess for this step is currently spawned and running.
    Running,
    /// Interrupted mid-flight (R-SA-084) — end timestamp recorded, not terminal.
    Paused,
    /// Finished without failure.
    Complete,
    /// Finished with a failure (including a synthesized stale-dead failure, R-SA-092).
    Failed,
    /// G77 — pi `step.status = "stopped"` (`subagent-runner.ts:2967`): this step was still
    /// `Running` or `Pending` when an explicit stop request landed, so the runner marked it
    /// `stopped` (with `exitCode: 1` and the stop message as its `error`) rather than `failed` or
    /// `paused`. Terminal and non-resumable, exactly like [`RunState::Stopped`].
    Stopped,
}

impl StepState {
    /// `true` for `Complete`/`Failed`/`Stopped` — mirrors [`RunState::is_terminal`]'s exclusion of
    /// `Paused` for the identical reason (R-SA-084: pause is soft and resumable, never terminal),
    /// and its inclusion of `Stopped` for the reason documented there (pi
    /// `chain-root-attachment.ts:61`'s `TERMINAL_STEP_STATUSES` set lists `"stopped"` alongside
    /// `"complete"`/`"completed"`/`"failed"`/`"paused"`).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            StepState::Complete | StepState::Failed | StepState::Stopped
        )
    }
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

    #[test]
    fn run_state_default_is_queued() {
        assert_eq!(RunState::default(), RunState::Queued);
    }

    #[test]
    fn run_state_queued_can_advance_to_running_or_failed_only() {
        assert!(RunState::Queued.can_transition_to(RunState::Running));
        assert!(RunState::Queued.can_transition_to(RunState::Failed));
        assert!(!RunState::Queued.can_transition_to(RunState::Complete));
        assert!(!RunState::Queued.can_transition_to(RunState::Paused));
        assert!(!RunState::Queued.can_transition_to(RunState::Queued));
    }

    #[test]
    fn run_state_running_can_advance_to_paused_complete_or_failed() {
        assert!(RunState::Running.can_transition_to(RunState::Paused));
        assert!(RunState::Running.can_transition_to(RunState::Complete));
        assert!(RunState::Running.can_transition_to(RunState::Failed));
        assert!(!RunState::Running.can_transition_to(RunState::Queued));
        assert!(!RunState::Running.can_transition_to(RunState::Running));
    }

    #[test]
    fn run_state_paused_can_resume_to_running_or_reconcile_to_failed() {
        assert!(RunState::Paused.can_transition_to(RunState::Running));
        assert!(RunState::Paused.can_transition_to(RunState::Failed));
        assert!(!RunState::Paused.can_transition_to(RunState::Complete));
        assert!(!RunState::Paused.can_transition_to(RunState::Queued));
        assert!(!RunState::Paused.can_transition_to(RunState::Paused));
    }

    #[test]
    fn run_state_terminal_states_permit_no_outgoing_transition() {
        for terminal in [RunState::Complete, RunState::Failed] {
            for candidate in [
                RunState::Queued,
                RunState::Running,
                RunState::Paused,
                RunState::Complete,
                RunState::Failed,
            ] {
                assert!(
                    !terminal.can_transition_to(candidate),
                    "{terminal:?} must not transition to {candidate:?}, terminal states are sinks"
                );
            }
        }
    }

    #[test]
    fn run_state_is_terminal_excludes_paused() {
        assert!(!RunState::Queued.is_terminal());
        assert!(!RunState::Running.is_terminal());
        assert!(
            !RunState::Paused.is_terminal(),
            "R-SA-084: interrupt is a soft pause, never terminal"
        );
        assert!(RunState::Complete.is_terminal());
        assert!(RunState::Failed.is_terminal());
    }

    #[test]
    fn run_state_try_advance_ok_on_legal_transition() {
        let result = RunState::Queued.try_advance(RunState::Running);
        assert_eq!(result, Ok(RunState::Running));
    }

    #[test]
    fn run_state_try_advance_err_on_illegal_transition() {
        let result = RunState::Complete.try_advance(RunState::Running);
        assert_eq!(
            result,
            Err(RunStateTransitionError {
                from: RunState::Complete,
                to: RunState::Running,
            })
        );
    }

    #[test]
    fn run_state_try_advance_err_message_is_human_readable() {
        let err = RunState::Failed
            .try_advance(RunState::Queued)
            .expect_err("Failed -> Queued must be illegal");
        let message = err.to_string();
        assert!(message.contains("Failed"));
        assert!(message.contains("Queued"));
    }

    /// G77 — `Stopped` is terminal in its own right, reachable only from `Running`/`Queued`, and a
    /// dead end thereafter. Every claim is checked against a named upstream site.
    #[test]
    fn stopped_is_a_first_class_terminal_run_state_not_an_alias() {
        // Terminal (pi `chain-root-attachment.ts:60` TERMINAL_STATES, `stale-run-reconciler.ts:292`
        // isTerminalState) — and distinct from the other two terminal states.
        assert!(RunState::Stopped.is_terminal());
        assert_ne!(RunState::Stopped, RunState::Failed);
        assert_ne!(RunState::Stopped, RunState::Paused);
        assert_eq!(RunState::Stopped.rank(), RunState::Failed.rank());

        // Reachable from exactly the two states pi's `stopAsyncRun` guard accepts
        // (`async-stop-action.ts:41`: `state !== "running" && state !== "queued"` is the refusal).
        assert!(RunState::Running.can_transition_to(RunState::Stopped));
        assert!(RunState::Queued.can_transition_to(RunState::Stopped));
        assert!(
            !RunState::Paused.can_transition_to(RunState::Stopped),
            "a paused run is not stoppable upstream — `stopAsyncRun` answers `No running or queued \
             async run was found`"
        );

        // A dead end: no outgoing transition at all, including to itself.
        for next in [
            RunState::Queued,
            RunState::Running,
            RunState::Paused,
            RunState::Complete,
            RunState::Failed,
            RunState::Stopped,
        ] {
            assert!(
                !RunState::Stopped.can_transition_to(next),
                "Stopped is terminal; Stopped -> {next:?} must be rejected"
            );
            RunState::Stopped
                .try_advance(next)
                .expect_err("every outgoing transition from Stopped is illegal");
        }
    }

    /// G77 — the per-step counterpart (pi `subagent-runner.ts:2967` `step.status = "stopped"`;
    /// `chain-root-attachment.ts:61` TERMINAL_STEP_STATUSES).
    #[test]
    fn stopped_is_a_first_class_terminal_step_state_not_an_alias() {
        assert!(StepState::Stopped.is_terminal());
        assert_ne!(StepState::Stopped, StepState::Failed);
        assert_ne!(StepState::Stopped, StepState::Paused);
        // The pre-existing terminality relations are untouched.
        assert!(!StepState::Pending.is_terminal());
        assert!(!StepState::Running.is_terminal());
        assert!(!StepState::Paused.is_terminal());
    }

    /// G77 — the wire spelling is `"stopped"` on both enums, which is what a pi-shaped
    /// `status.json`/result file round-trips through.
    #[test]
    fn stopped_serializes_as_the_lowercase_pi_wire_word() {
        assert_eq!(
            serde_json::to_value(RunState::Stopped).expect("serialize"),
            serde_json::json!("stopped")
        );
        assert_eq!(
            serde_json::to_value(StepState::Stopped).expect("serialize"),
            serde_json::json!("stopped")
        );
        assert_eq!(
            serde_json::from_value::<RunState>(serde_json::json!("stopped")).expect("deserialize"),
            RunState::Stopped
        );
        assert_eq!(
            serde_json::from_value::<StepState>(serde_json::json!("stopped")).expect("deserialize"),
            StepState::Stopped
        );
    }

    /// G77 — the workflow-graph projection: a stopped step normalizes to
    /// [`WorkflowNodeStatus::Stopped`] (pi `subagent-runner.ts:2163-2166`) and rolls up over a
    /// parallel group ahead of `failed`/`paused` (`:2181-2186`).

    #[test]
    fn run_state_full_lifecycle_walks_queued_running_paused_running_complete() {
        // A realistic full lifecycle: spawn -> run -> interrupt -> resume -> finish.
        let mut state = RunState::Queued;
        for next in [
            RunState::Running,
            RunState::Paused,
            RunState::Running,
            RunState::Complete,
        ] {
            state = state.try_advance(next).expect("each hop is legal");
        }
        assert_eq!(state, RunState::Complete);
    }

    #[test]
    fn run_state_full_lifecycle_walks_queued_running_failed() {
        let mut state = RunState::Queued;
        for next in [RunState::Running, RunState::Failed] {
            state = state.try_advance(next).expect("each hop is legal");
        }
        assert_eq!(state, RunState::Failed);
    }

    #[test]
    fn step_state_is_terminal_excludes_paused() {
        assert!(!StepState::Pending.is_terminal());
        assert!(!StepState::Running.is_terminal());
        assert!(!StepState::Paused.is_terminal());
        assert!(StepState::Complete.is_terminal());
        assert!(StepState::Failed.is_terminal());
    }

    #[test]
    fn run_mode_serializes_as_camel_case() {
        assert_eq!(
            serde_json::to_string(&RunMode::Single).expect("serializes"),
            "\"single\""
        );
        assert_eq!(
            serde_json::to_string(&RunMode::Parallel).expect("serializes"),
            "\"parallel\""
        );
        assert_eq!(
            serde_json::to_string(&RunMode::Chain).expect("serializes"),
            "\"chain\""
        );
        // SCOPE_3d — the fourth `SubagentRunMode` word (`shared/types.ts:400`), and every
        // variant round-trips (a `status.json` written with any of the four reads back).
        assert_eq!(
            serde_json::to_string(&RunMode::Workflow).expect("serializes"),
            "\"workflow\""
        );
        for mode in [
            RunMode::Single,
            RunMode::Parallel,
            RunMode::Chain,
            RunMode::Workflow,
        ] {
            let json = serde_json::to_string(&mode).expect("serializes");
            let back: RunMode = serde_json::from_str(&json).expect("round-trips");
            assert_eq!(back, mode);
        }
    }
}
