//! SCOPE_10 — the two SESSION-AWARE entry points: pi
//! `runs/background/async-status-snapshot.ts:30-44` (`@7fe9dee1`).
//!
//! This is the only file in the module that knows what a session is. [`super::project`] projects
//! whatever it is handed; the decision about WHOSE runs may be handed to it is made here, once.
//!
//! # `:31` is STRICT in all three of its arms
//!
//! ```ts
//! if (!state || !sessionId || state.currentSessionId !== sessionId) return [];
//! ```
//!
//! All three collapse to an EMPTY snapshot, and the middle one is the arm a one-line summary
//! hides: a caller with no requested session gets nothing, NOT "everything unfiltered". That is
//! [`crate::background::delivery::SessionGate::Strict`]'s class exactly
//! (`background/delivery/gate.rs:19,52-57`) — `None` current refuses — and unlike S6 in
//! [`crate::background::run_status`] there is no `None`/`None` row to disagree about, because the
//! REQUESTED session is a `&SessionId` here rather than an `Option`. The gate is therefore
//! written as the direct comparison upstream writes, with the `Option` handled by the parameter
//! type instead of by a gate class.

use crate::identity::SessionId;
use crate::tui::fleet_state::{AsyncRunView, FleetState};

use super::build_async_status_snapshot;
use super::types::{AsyncStatusSnapshot, AsyncStatusSnapshotOptions};

/// pi `asyncStatusSnapshotJobsForState` (`async-status-snapshot.ts:30-40`).
///
/// `state` is [`FleetState`] — cyrup's port of upstream's `SubagentState`
/// (`tui/fleet_state.rs:1-30`) — and its `tracked_jobs` is upstream's `state.asyncJobs`.
///
/// # `[CYRUP-DELTA, unrepresentable]` — the second loop has no second source
///
/// Upstream runs TWO loops (`:33-38`): `state.asyncJobs` first, then `state.fleetJobs`, with
/// `!jobs.has(job.asyncId)` making `asyncJobs` win a collision. cyrup has no `fleetJobs` analogue:
/// [`FleetState::tracked_jobs`] documents itself as *"pi `state.fleetJobs ?? state.asyncJobs`"*
/// (`tui/fleet_state.rs:487-489`) — the two upstream maps are already collapsed into one on this
/// side, by the producer. So the second loop and its first-writer-wins dedup collapse away, and a
/// second map is NOT invented here to make the loop count match. The dedup itself is kept, over
/// the one source, because a `Vec` can carry a duplicate id where upstream's `Map` cannot.
#[must_use]
pub fn async_status_snapshot_jobs_for_state<'a>(
    state: Option<&'a FleetState>,
    session_id: Option<&SessionId>,
) -> Vec<&'a AsyncRunView> {
    // `:31` — STRICT, all three arms.
    let (Some(state), Some(session_id)) = (state, session_id) else {
        return Vec::new();
    };
    if state.current_session_id.as_deref() != Some(session_id.as_str()) {
        return Vec::new();
    }
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    let mut jobs: Vec<&AsyncRunView> = Vec::new();
    for job in &state.tracked_jobs {
        // `:34` — the PER-JOB filter, over the run's OWN recorded session
        // (`RunStatus::session_id`, the typed one), not over `AsyncRunView::session_id`, which is
        // the same value re-stringified for the fleet renderer.
        if job.status.session_id.as_ref() != Some(session_id) {
            continue;
        }
        if seen.insert(job.status.run_id.as_str()) {
            jobs.push(job);
        }
    }
    jobs
}

/// pi `buildAsyncStatusSnapshotForState` (`async-status-snapshot.ts:42-44`) — [`build_async_status_snapshot`]
/// over exactly the jobs [`async_status_snapshot_jobs_for_state`] admits.
#[must_use]
pub fn build_async_status_snapshot_for_state(
    state: Option<&FleetState>,
    session_id: Option<&SessionId>,
    options: &AsyncStatusSnapshotOptions,
) -> AsyncStatusSnapshot {
    build_async_status_snapshot(
        async_status_snapshot_jobs_for_state(state, session_id),
        options,
    )
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::testfixtures::run_view;
    use super::*;

    fn state_with(current: Option<&str>, jobs: Vec<AsyncRunView>) -> FleetState {
        FleetState {
            current_session_id: current.map(str::to_string),
            tracked_jobs: jobs,
            ..FleetState::default()
        }
    }

    fn ids(jobs: &[&AsyncRunView]) -> Vec<String> {
        jobs.iter()
            .map(|job| job.status.run_id.as_str().to_string())
            .collect()
    }

    /// `:31`'s THIRD arm — the state's own current session must BE the requested one. Two cyrup
    /// instances share a per-cwd async root, so a snapshot built from instance A's state for
    /// instance B's session would hand B a list of A's runs.
    #[test]
    fn the_snapshot_is_empty_when_the_state_session_does_not_match() {
        let state = state_with(
            Some("session-a"),
            vec![run_view("run0aaa00000", Some("session-b"), 10)],
        );
        let wanted = SessionId::parse("session-b").expect("valid session id");
        assert!(
            async_status_snapshot_jobs_for_state(Some(&state), Some(&wanted)).is_empty(),
            "the state belongs to session-a; it cannot answer for session-b"
        );
        assert!(
            build_async_status_snapshot_for_state(
                Some(&state),
                Some(&wanted),
                &AsyncStatusSnapshotOptions::default()
            )
            .runs
            .is_empty()
        );
    }

    /// `:31`'s SECOND arm — the one a one-line summary of this filter hides. STRICT means a
    /// request with NO session yields `[]`; it does NOT mean "no filter, show everything". The
    /// state below is fully populated and self-consistent, so nothing but the missing requested
    /// session can be what empties the result.
    #[test]
    fn the_snapshot_is_empty_when_there_is_no_requested_session() {
        let state = state_with(
            Some("session-a"),
            vec![run_view("run0aaa00000", Some("session-a"), 10)],
        );
        let wanted = SessionId::parse("session-a").expect("valid session id");
        assert_eq!(
            ids(&async_status_snapshot_jobs_for_state(
                Some(&state),
                Some(&wanted)
            )),
            vec!["run0aaa00000"],
            "control: the run IS visible to its own session"
        );
        assert!(
            async_status_snapshot_jobs_for_state(Some(&state), None).is_empty(),
            "no requested session is a REFUSAL, not an unfiltered listing"
        );
        // `:31`'s FIRST arm, for completeness: no state at all.
        assert!(async_status_snapshot_jobs_for_state(None, Some(&wanted)).is_empty());
    }

    /// `:34` — the SECOND filter, per job. The state gate above passing does not make every job
    /// in that state the caller's: a process that outlived a session rotation still tracks the
    /// previous session's runs.
    #[test]
    fn the_snapshot_excludes_jobs_from_other_sessions() {
        let state = state_with(
            Some("session-a"),
            vec![
                run_view("run0aaa00000", Some("session-a"), 30),
                run_view("run0bbb00000", Some("session-b"), 20),
                // A run nobody claimed is not everybody's — the same rule `list_active_runs`
                // already applies (`run_status.rs`'s session-scoping block).
                run_view("run0ccc00000", None, 10),
            ],
        );
        let wanted = SessionId::parse("session-a").expect("valid session id");
        assert_eq!(
            ids(&async_status_snapshot_jobs_for_state(
                Some(&state),
                Some(&wanted)
            )),
            vec!["run0aaa00000"]
        );
    }

    /// The dedup the `[CYRUP-DELTA]` above keeps over the ONE source: upstream's `Map` cannot
    /// hold a duplicate `asyncId`, cyrup's `Vec<AsyncRunView>` can, and a duplicated run would be
    /// projected twice and counted twice against `maxRuns`.
    #[test]
    fn the_snapshot_admits_each_run_id_once() {
        let state = state_with(
            Some("session-a"),
            vec![
                run_view("run0aaa00000", Some("session-a"), 30),
                run_view("run0aaa00000", Some("session-a"), 40),
            ],
        );
        let wanted = SessionId::parse("session-a").expect("valid session id");
        let jobs = async_status_snapshot_jobs_for_state(Some(&state), Some(&wanted));
        assert_eq!(ids(&jobs), vec!["run0aaa00000"]);
        // First writer wins, exactly as `:37`'s `!jobs.has(job.asyncId)` decides.
        assert_eq!(jobs[0].status.last_update, 30);
    }
}
