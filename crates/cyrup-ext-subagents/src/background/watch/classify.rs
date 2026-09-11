//! R-SA-100 terminal-state classification: the four-way [`ClassifiedOutcome`] and the OR'd
//! state/success/stopped-child decision every notification and every bus event is keyed on. Split
//! out of `background/watch.rs`; ports pi `runs/background/notify.ts:199-210`.

use crate::background::{ResultFile, RunState};

// =================================================================================================
// classify_outcome (R-SA-100)
// =================================================================================================

/// R-SA-100: classify a [`ResultFile`] as completed/failed/paused using at least two
/// independently-populated OR'd signals — here, the explicit `state` field OR'd against the
/// `success`/exit-indicator-derived signal, since `state == Complete && !success` (every step
/// individually failed acceptance, but the run itself finished without a run-ending crash) is a
/// real, legitimate combination `runner_main::finish_run` can produce, and a classifier that
/// looked at ONLY `state` would misreport that case as unconditionally "completed" when a caller
/// may specifically want to distinguish it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifiedOutcome {
    Completed,
    Failed,
    Paused,
    /// G77 — pi `buildCompletionDetails`'s fourth status word (`notify.ts:210`: `const status =
    /// stopped ? "stopped" : paused ? "paused" : result.success ? "completed" : "failed"`). Note
    /// upstream evaluates `stopped` FIRST, ahead of both `paused` and `success`, and derives it
    /// from `result.stopped === true || result.state === "stopped" || …` plus the same test over
    /// every child (`notify.ts:199-205`) — so a stopped run is never reported as failed, and never
    /// as paused either.
    Stopped,
}

/// Classify `result` per R-SA-100 — a `Paused` state is NEVER reclassified as `Failed` regardless
/// of `success` (R-SA-100: "a paused (interrupted) run MUST NOT be classified as failed" —
/// `success` is not a meaningful signal for a `Paused` run in `runner_main::finish_run`'s own
/// construction, since a paused run's `success` is computed from whatever partial `results` had
/// accumulated before the interrupt, which is not the signal this classification cares about).
#[must_use]
pub fn classify_outcome(result: &ResultFile) -> ClassifiedOutcome {
    match result.state {
        // G77 — checked BEFORE `Paused`/`success`, mirroring `notify.ts:199-210`'s own ordering:
        // `stopped` is derived first and wins outright. Also OR'd against the per-child signal
        // (`result.results?.some((child) => child.stopped === true || …)`, `notify.ts:203-205`) so
        // a run whose overall `state` was never repaired to `Stopped` but whose children were
        // stopped still classifies as stopped rather than failed.
        RunState::Stopped => ClassifiedOutcome::Stopped,
        _ if result.results.iter().any(|child| child.stopped) => ClassifiedOutcome::Stopped,
        RunState::Paused => ClassifiedOutcome::Paused,
        RunState::Complete if result.success => ClassifiedOutcome::Completed,
        RunState::Complete => ClassifiedOutcome::Failed, // state says done, success says no
        RunState::Failed => ClassifiedOutcome::Failed,
        RunState::Queued | RunState::Running => {
            // Should not occur for a genuinely terminal ResultFile (finish_run only ever writes
            // Complete/Failed/Paused) — classified as Failed defensively rather than panicking or
            // returning an Option, since a caller consuming this classification has no sane
            // "still running" bucket to put a RESULT FILE'S OWN state into (its very presence
            // already means the run reached SOME terminal write).
            ClassifiedOutcome::Failed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::sample_result;
    use super::*;
    use crate::background::RunState;

    // ---------------------------------------------------------------------------------------
    // G77 — `stopped` is the FOURTH completion classification (pi `notify.ts:199-210`)
    // ---------------------------------------------------------------------------------------

    /// A stopped run classifies as [`ClassifiedOutcome::Stopped`] — never `Failed` (which is what a
    /// `success: false` run without this arm would produce) and never `Paused`. Both of upstream's
    /// signals are covered: the run's own `state`, and the per-child `stopped` flag ORed over
    /// `result.results` (`notify.ts:200-205`).
    #[test]
    fn classify_outcome_reports_stopped_from_either_the_run_state_or_a_stopped_child() {
        let mut by_state = sample_result("run-stop-1", RunState::Stopped, false);
        assert_eq!(classify_outcome(&by_state), ClassifiedOutcome::Stopped);

        // …and it wins even if `success` were somehow true.
        by_state.success = true;
        assert_eq!(classify_outcome(&by_state), ClassifiedOutcome::Stopped);

        // The per-child OR: the run's own state was never repaired, but a child says stopped.
        let mut by_child = sample_result("run-stop-2", RunState::Failed, false);
        by_child.results.push(stopped_child());
        assert_eq!(classify_outcome(&by_child), ClassifiedOutcome::Stopped);

        // Every pre-G77 classification is untouched.
        assert_eq!(
            classify_outcome(&sample_result("run-ok", RunState::Complete, true)),
            ClassifiedOutcome::Completed
        );
        assert_eq!(
            classify_outcome(&sample_result("run-bad", RunState::Failed, false)),
            ClassifiedOutcome::Failed
        );
        assert_eq!(
            classify_outcome(&sample_result("run-pause", RunState::Paused, false)),
            ClassifiedOutcome::Paused
        );
    }

    // ---------------------------------------------------------------------------------------
    // classify_outcome: R-SA-100 OR'd signal classification
    // ---------------------------------------------------------------------------------------

    #[test]
    fn classify_outcome_paused_is_never_failed_regardless_of_success_flag() {
        let paused = sample_result("run00004", RunState::Paused, false);
        assert_eq!(classify_outcome(&paused), ClassifiedOutcome::Paused);
        let paused_success_true = sample_result("run00005", RunState::Paused, true);
        assert_eq!(
            classify_outcome(&paused_success_true),
            ClassifiedOutcome::Paused
        );
    }

    #[test]
    fn classify_outcome_complete_with_success_false_is_failed_not_completed() {
        let result = sample_result("run00006", RunState::Complete, false);
        assert_eq!(classify_outcome(&result), ClassifiedOutcome::Failed);
    }

    #[test]
    fn classify_outcome_complete_with_success_true_is_completed() {
        let result = sample_result("run00007", RunState::Complete, true);
        assert_eq!(classify_outcome(&result), ClassifiedOutcome::Completed);
    }

    #[test]
    fn classify_outcome_failed_state_is_always_failed() {
        let result = sample_result("run00012", RunState::Failed, false);
        assert_eq!(classify_outcome(&result), ClassifiedOutcome::Failed);
    }

    /// A `SingleResult` that was terminated by an explicit stop.
    fn stopped_child() -> crate::exec::SingleResult {
        crate::exec::SingleResult {
            // SUBA-021: no usage budget on this path (see the field doc).
            usage_budget: None,
            turn_budget: None,
            turn_budget_exceeded: false,
            wrap_up_requested: false,
            child_run_id: None,
            agent: "researcher".to_string(),
            task: String::new(),
            exit_code: 1,
            usage: cyrup_core::Usage::default(),
            turns: 0,
            model: None,
            attempted_models: Vec::new(),
            model_attempts: Vec::new(),
            final_output: None,
            structured_output: None,
            session_file: None,
            output_state: Default::default(),
            structured_output_path: None,
            artifact_paths: None,
            acceptance: None,
            detached: false,
            interrupted: false,
            timed_out: false,
            timeout_recovery: None,
            context_overflow: false,
            stopped: true,
            process_signal: None,
            error: None,
            saved_output_path: None,
            tool_calls: Vec::new(),
            output_truncated: false,
            control_events: Vec::new(),
            progress: None,
            runner: None,
            external_process: None,
            // Test fixture: no child was planned, so there is no surface to report.
            tool_surface: crate::exec::tool_surface::ResolvedToolSurface::default(),
        }
    }
}
