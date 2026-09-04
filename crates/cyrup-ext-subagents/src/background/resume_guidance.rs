//! SUBA-060 — "resume-first" guidance for FAILED async runs: the Rust port of pi-subagents'
//! `src/runs/background/resume-guidance.ts` (new at v0.45.2, `b283d27`; absent at v0.43.0).
//!
//! # The defect this closes
//!
//! When a background run fails, cyrup's `wait` result said only *how many* runs failed. An
//! orchestrator model's default response to "one run failed" is to spawn a fresh child — which
//! discards the failed child's persisted session transcript and re-pays for every turn it already
//! took. cyrup HAS the revival machinery ([`super::control::resume`]'s
//! `ResumeOutcome::RespawnFromTranscript`); what was missing is the sentence that tells the model
//! to use it BEFORE relaunching.
//!
//! # Scope, and the one upstream function deliberately not ported here
//!
//! Upstream has three formatters and exactly two live consumers at v0.47.1:
//!
//! * `formatResumeFirstFailedRunsNote` → `subagent-wait.ts:617`, folded into the `wait` tool's
//!   result text between the outcome summary and the attention note. Ported as
//!   [`format_resume_first_failed_runs_note`] and wired at the identical position in
//!   [`super::wait`].
//! * `formatAsyncReviveCommand` → the shared command builder both of the others call. Ported as
//!   [`format_async_revive_command`].
//! * `formatResumeFirstFailedRunDetail` → `wait-subscriptions.ts:183` ONLY. That module
//!   (`runs/background/wait-subscriptions.ts`) is unported in cyrup and tracked as PARITY-GAPS
//!   **VL-S8**; it is the *only* call site upstream. Porting the formatter now would add a `pub fn`
//!   with no caller in this crate, and an unreachable capability is the defect class this backlog
//!   keeps filing (SUBA-043/SUBA-047). It is therefore owed BY VL-S8 — whoever lands wait
//!   subscriptions writes it there, over [`format_async_revive_command`], which is the only part
//!   of it that carries real logic. Recorded here so it is not re-derived as a missing function.
//!
//! # Mapping upstream's `AsyncRunSummary` onto cyrup's [`RunStatus`]
//!
//! Upstream's summary carries an explicit `step.index`; cyrup's [`super::StepStatus`] does not, because a
//! [`RunStatus::steps`] vector IS in step order — the same equivalence
//! `run_status::format_resume_guidance`'s `enumerate()` already relies on. `run.state` is
//! [`RunState`] and `step.status` is [`StepState`]; `sessionFile`/`fs.existsSync` is
//! [`session_file_exists`], the same predicate `run_status.rs` uses for its own `Revive:` line.

use super::{RunState, RunStatus, StepState};

/// pi `fs.existsSync(candidate.sessionFile)` (`resume-guidance.ts:5,8`) — a session file counts
/// only if it is still on disk, because the whole point of the guidance is that reviving it will
/// work.
fn session_file_exists(path: Option<&std::path::PathBuf>) -> bool {
    path.is_some_and(|path| path.exists())
}

/// pi `formatAsyncReviveCommand` (`resume-guidance.ts:4-14`) — the literal `subagent({ … })` call
/// the model should issue to continue the ORIGINAL run.
///
/// Branch order is upstream's, and both branches matter:
///
/// 1. the first `failed` step whose session file still exists wins, and `index:` is emitted only
///    when the run has more than one step (`run.steps.length === 1 ? "" : ", index: ${step.index}"`
///    at `:12`) — a single-step run addresses the run itself, so an index would be noise;
/// 2. with no such step, a SINGLE-step run may still be revived through the RUN-level session file
///    (`:8-10`), which is the shape an async single takes when the failure happened before any
///    per-step record was written.
///
/// Returns `None` when nothing is revivable, which is what makes the callers' "…or don't say
/// anything" behaviour possible rather than emitting guidance that cannot be followed.
#[must_use]
pub fn format_async_revive_command(run: &RunStatus) -> Option<String> {
    const MESSAGE: &str = "Continue from the persisted child session and report the result.";
    let id = run.run_id.as_str();
    let failed_step = run.steps.iter().enumerate().find(|(_, step)| {
        step.status == StepState::Failed && session_file_exists(step.session_file.as_ref())
    });
    let Some((index, _)) = failed_step else {
        if run.steps.len() == 1 && session_file_exists(run.session_file.as_ref()) {
            return Some(format!(
                "subagent({{ action: \"resume\", id: \"{id}\", message: \"{MESSAGE}\" }})"
            ));
        }
        return None;
    };
    let index = if run.steps.len() == 1 {
        String::new()
    } else {
        format!(", index: {index}")
    };
    Some(format!(
        "subagent({{ action: \"resume\", id: \"{id}\"{index}, message: \"{MESSAGE}\" }})"
    ))
}

/// pi `formatResumeFirstFailedRunsNote` (`resume-guidance.ts:23-33`) — the sentence appended to a
/// `wait` result that saw at least one failed, revivable run.
///
/// The LEADING SPACE is upstream's own (`:32` returns `` ` Resume-first: …` ``) and is load-bearing
/// here for the same reason it is there: the caller interpolates it directly after the outcome
/// clause with no separator of its own.
///
/// The singular/plural split is upstream's verbatim, including the slightly odd doubled clause in
/// the plural form ("…before retrying before reporting failure or launching a replacement"), which
/// falls out of concatenating `guidance` with the shared tail at `:32`. It is reproduced rather
/// than tidied: this string is model-facing text pinned by upstream's own tests.
#[must_use]
pub fn format_resume_first_failed_runs_note(runs: &[RunStatus]) -> String {
    let resumable: Vec<(&RunStatus, String)> = runs
        .iter()
        .filter(|run| run.state == RunState::Failed)
        .filter_map(|run| format_async_revive_command(run).map(|command| (run, command)))
        .collect();
    let guidance = match resumable.as_slice() {
        [] => return String::new(),
        [(run, command)] => format!(
            "failed run \"{}\" has a persisted child session. Revive the original run with {command}",
            run.run_id.as_str()
        ),
        many => format!(
            "{} failed runs have persisted child sessions. Inspect status and revive each original \
             run before retrying",
            many.len()
        ),
    };
    format!(
        " Resume-first: {guidance} before reporting failure or launching a replacement. Launch a \
         replacement only if revive fails or the user explicitly asks for one."
    )
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::background::{RunId, RunMode, StepStatus};
    use std::path::PathBuf;

    fn run_with(
        state: RunState,
        steps: Vec<StepStatus>,
        session_file: Option<PathBuf>,
    ) -> RunStatus {
        let mut status = RunStatus::queued(RunId::from_token("run-1"), RunMode::Single, None);
        status.state = state;
        status.steps = steps;
        status.session_file = session_file;
        status
    }

    fn step(state: StepState, session_file: Option<PathBuf>) -> StepStatus {
        let mut step = StepStatus::pending("agent-a");
        step.status = state;
        step.session_file = session_file;
        step
    }

    /// pi `resume-guidance.ts:12` — `index` is emitted for a MULTI-step run and omitted for a
    /// single-step one. The item's Verify names exactly these two cases.
    #[test]
    fn the_revive_command_carries_an_index_only_for_a_multi_step_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("child.jsonl");
        std::fs::write(&transcript, b"{}").expect("write transcript");

        let multi = run_with(
            RunState::Failed,
            vec![
                step(StepState::Failed, Some(transcript.clone())),
                step(StepState::Pending, None),
            ],
            None,
        );
        assert_eq!(
            format_async_revive_command(&multi).as_deref(),
            Some(
                "subagent({ action: \"resume\", id: \"run-1\", index: 0, message: \"Continue from \
                 the persisted child session and report the result.\" })"
            )
        );

        let single = run_with(
            RunState::Failed,
            vec![step(StepState::Failed, Some(transcript.clone()))],
            None,
        );
        assert_eq!(
            format_async_revive_command(&single).as_deref(),
            Some(
                "subagent({ action: \"resume\", id: \"run-1\", message: \"Continue from the \
                 persisted child session and report the result.\" })"
            )
        );

        // The SECOND step is the failed one, so the index must be 1, not "the first step".
        let second = run_with(
            RunState::Failed,
            vec![
                step(StepState::Complete, Some(transcript.clone())),
                step(StepState::Failed, Some(transcript)),
            ],
            None,
        );
        assert!(
            format_async_revive_command(&second)
                .expect("revivable")
                .contains("index: 1"),
            "the failed step's own position is the index pi passes"
        );
    }

    /// pi `resume-guidance.ts:8-10` — the run-level `sessionFile` fallback for a single-step run
    /// whose step record never got a transcript of its own.
    #[test]
    fn a_single_step_run_falls_back_to_the_run_level_session_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("run.jsonl");
        std::fs::write(&transcript, b"{}").expect("write transcript");

        let run = run_with(
            RunState::Failed,
            vec![step(StepState::Failed, None)],
            Some(transcript),
        );
        assert!(
            format_async_revive_command(&run)
                .expect("revivable through the run-level session file")
                .starts_with("subagent({ action: \"resume\", id: \"run-1\", message:"),
            "the run-level fallback never carries an index"
        );

        // Two steps and no per-step transcript: upstream's fallback is single-step ONLY.
        let two = run_with(
            RunState::Failed,
            vec![step(StepState::Failed, None), step(StepState::Failed, None)],
            Some(dir.path().join("run.jsonl")),
        );
        assert_eq!(format_async_revive_command(&two), None);
    }

    /// A session file that is RECORDED but no longer on disk must not be advertised — pi gates
    /// every branch on `fs.existsSync`, so a run whose artifacts were swept produces no guidance
    /// rather than a `resume` call that will fail.
    #[test]
    fn a_recorded_but_missing_session_file_yields_no_command() {
        let run = run_with(
            RunState::Failed,
            vec![step(
                StepState::Failed,
                Some(PathBuf::from("/nonexistent/child.jsonl")),
            )],
            None,
        );
        assert_eq!(format_async_revive_command(&run), None);
    }

    /// pi `resume-guidance.ts:24` — only `failed` runs contribute. A COMPLETE run with a persisted
    /// transcript is revivable in principle and must still produce no note, or every successful
    /// wait would carry resume-first guidance.
    #[test]
    fn the_note_covers_failed_runs_only_and_is_empty_otherwise() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("child.jsonl");
        std::fs::write(&transcript, b"{}").expect("write transcript");

        let complete = run_with(
            RunState::Complete,
            vec![step(StepState::Complete, Some(transcript.clone()))],
            None,
        );
        assert_eq!(format_resume_first_failed_runs_note(&[complete]), "");
        assert_eq!(format_resume_first_failed_runs_note(&[]), "");

        // Failed but NOT revivable (no transcript) — pi drops it from `resumable`, so the note is
        // empty rather than advising a revive that cannot work.
        let unrevivable = run_with(RunState::Failed, vec![step(StepState::Failed, None)], None);
        assert_eq!(format_resume_first_failed_runs_note(&[unrevivable]), "");

        let failed = run_with(
            RunState::Failed,
            vec![step(StepState::Failed, Some(transcript))],
            None,
        );
        let note = format_resume_first_failed_runs_note(&[failed]);
        assert_eq!(
            note,
            " Resume-first: failed run \"run-1\" has a persisted child session. Revive the original \
             run with subagent({ action: \"resume\", id: \"run-1\", message: \"Continue from the \
             persisted child session and report the result.\" }) before reporting failure or \
             launching a replacement. Launch a replacement only if revive fails or the user \
             explicitly asks for one."
        );
        assert!(
            note.starts_with(' '),
            "pi's own leading space is load-bearing"
        );
    }

    /// pi `resume-guidance.ts:29-31` — the plural branch names the COUNT and stops naming a
    /// specific command, because the model has to inspect status to pick between them.
    #[test]
    fn two_failed_revivable_runs_produce_the_plural_note() {
        let dir = tempfile::tempdir().expect("tempdir");
        let transcript = dir.path().join("child.jsonl");
        std::fs::write(&transcript, b"{}").expect("write transcript");

        let mut second = run_with(
            RunState::Failed,
            vec![step(StepState::Failed, Some(transcript.clone()))],
            None,
        );
        second.run_id = RunId::from_token("run-2");
        let first = run_with(
            RunState::Failed,
            vec![step(StepState::Failed, Some(transcript))],
            None,
        );

        assert_eq!(
            format_resume_first_failed_runs_note(&[first, second]),
            " Resume-first: 2 failed runs have persisted child sessions. Inspect status and revive \
             each original run before retrying before reporting failure or launching a \
             replacement. Launch a replacement only if revive fails or the user explicitly asks \
             for one."
        );
    }
}
