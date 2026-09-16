//! The retention decision for ONE async-root entry. **Pure.**
//!
//! # Why this is a separate, I/O-free module
//!
//! Upstream's `runSkipReason` (`:279-305`) is the same decision, but it is not pure: it calls
//! `fs.existsSync` four times (`:294` via `activeMarkerExists`, `:298` twice, `:300` via
//! `hasResumableContract`) and `fs.statSync` twice via `statusTimestamp` (`:179`). Making it pure
//! in cyrup means hoisting every one of those probes into
//! [`scan_run_candidates`](super::scan_run_candidates) as a pre-computed fact. That is the whole
//! design, and it is only sound if the fact list is EXHAUSTIVE — a missing fact is a guard that
//! silently never fires.
//!
//! The precedent is [`crate::background::delivery`]'s `Attribution`, classified by a pure
//! `Attribution::classify`, and the rule its module doc states: *"The shell touches the lock and
//! the filesystem; the core decides."*
//!
//! # Three outcomes, not two
//!
//! Upstream's `string | undefined` return looks two-valued, but its caller (`:781-833`) makes a
//! three-way decision, and folding it back to two loses the tombstone case entirely. The mapping
//! is exact:
//!
//! | `(already_tombstoned, skip_reason, past_grace)` | upstream | here |
//! |---|---|---|
//! | `(false, Some(r), _)` | `:781-783` count + continue | [`RetentionDecision::Keep`] |
//! | `(false, None, _)` | `:807-833` rename → recheck → rm | [`RetentionDecision::Tombstone`] |
//! | `(true,  Some(r), _)` | `:786-788` / `:781-783` | [`RetentionDecision::Keep`] |
//! | `(true,  None, false)` | `:790-792` | `Keep(`[`SkipReason::TombstoneGrace`]`)` |
//! | `(true,  None, true)` | `:794-805` recheck → `rmSync` | [`RetentionDecision::Reap`] |

use std::collections::BTreeSet;
use std::path::PathBuf;

use crate::background::{RunId, RunMode, RunStatus};
use crate::identity::RunDirName;

use super::RUN_TOMBSTONE_PREFIX;
use super::tombstone::{TombstoneMarkerState, marker_matches};

/// The `recheck-` prefix SCOPE_14 stamps onto a reason produced by the SECOND decision, the one
/// taken after the rename (pi `:797`, `:827`).
///
/// Provided here, beside the vocabulary it composes with, so part B cannot invent a second
/// spelling of it. See [`SkipReason::recheck_key`].
pub const RECHECK_REASON_PREFIX: &str = "recheck-";

/// Why a candidate is not reapable. A CLOSED enum whose [`SkipReason::as_str`] produces
/// upstream's exact literals, because those strings are the keys of
/// `AsyncRetentionResult::skipped` (`:110`) and SCOPE_14's report is compared against upstream's
/// vocabulary.
///
/// The shape (closed enum + `as_str`) follows
/// [`crate::background::wait_subscriptions::WaitTargetKind::as_str`] and
/// `background::run_status::run_state_label`.
///
/// The ORDER of the variants is upstream's evaluation order, which is load-bearing: cheapest and
/// most dangerous first, so the reason a caller sees names the FIRST reason the run was spared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SkipReason {
    /// `invalid-status` (`:289`) — no readable `status.json`.
    ///
    /// [CYRUP-DELTA] upstream also reaches this through `readStatus`'s `RUN_MODES` check
    /// (`:150`): a `mode` outside `{single, parallel, chain, workflow}` makes the whole status
    /// `undefined`. [`RunMode`] is a closed `serde` enum, so an out-of-set mode is a
    /// DESERIALIZATION failure and lands here too — the same behaviour reached by a different
    /// mechanism, and therefore NOT a separate guard.
    InvalidStatus,
    /// `identity-mismatch` (`:290-291`) — the id fails
    /// [`RunDirName::parse`](crate::identity::RunDirName::parse), or the directory name disagrees
    /// with `status.runId` and is not tombstone-prefixed.
    IdentityMismatch,
    /// `runtime-reference` (`:292`) — a live in-process job still tracks this run.
    RuntimeReference,
    /// `wait-reference` (`:293`) — an armed wait subscription names this run.
    WaitReference,
    /// `active-index` (`:294`) — `<async_root>/.active-runs/<runId>` exists.
    ActiveIndex,
    /// `non-terminal` (`:295`) — the run is not `Complete`/`Failed`/`Stopped`.
    NonTerminal,
    /// `workflow-reference` (`:296`).
    WorkflowReference,
    /// `nested-reference` (`:297`).
    NestedReference,
    /// `mission-reference` (`:298`).
    MissionReference,
    /// `handoff-reference` (`:299`).
    HandoffReference,
    /// `resumable` (`:300`) — a session transcript or recovery descriptor still resolves.
    Resumable,
    /// `unknown-age` (`:301-302`) — the timestamp could not be established, so the run's age is
    /// unknown and it is kept. Unreadable never means reapable.
    UnknownAge,
    /// `recent` (`:303`) — inside the retention window.
    Recent,
    /// `run-tombstone-marker` (`:787`) — an existing `.deleting-run-*` tree whose marker does not
    /// resolve to it (absent, unreadable, or naming another tree).
    RunTombstoneMarker,
    /// `tombstone-grace` (`:791`) — an existing `.deleting-run-*` tree whose own mtime is inside
    /// [`ASYNC_RETENTION_TOMBSTONE_GRACE_MS`](super::ASYNC_RETENTION_TOMBSTONE_GRACE_MS).
    ///
    /// Crash / concurrency hysteresis, NOT a delay between tombstoning and reaping — see that
    /// constant's doc.
    TombstoneGrace,
}

impl SkipReason {
    /// Upstream's literal (`:289-303`, `:787`, `:791`) — the `skipped` map's key.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidStatus => "invalid-status",
            Self::IdentityMismatch => "identity-mismatch",
            Self::RuntimeReference => "runtime-reference",
            Self::WaitReference => "wait-reference",
            Self::ActiveIndex => "active-index",
            Self::NonTerminal => "non-terminal",
            Self::WorkflowReference => "workflow-reference",
            Self::NestedReference => "nested-reference",
            Self::MissionReference => "mission-reference",
            Self::HandoffReference => "handoff-reference",
            Self::Resumable => "resumable",
            Self::UnknownAge => "unknown-age",
            Self::Recent => "recent",
            Self::RunTombstoneMarker => "run-tombstone-marker",
            Self::TombstoneGrace => "tombstone-grace",
        }
    }

    /// The same reason as seen by the SECOND decision, after the rename: pi's
    /// `` increment(result.skipped, `recheck-${finalReason}`) `` (`:797`, `:827`).
    #[must_use]
    pub fn recheck_key(self) -> String {
        format!("{RECHECK_REASON_PREFIX}{}", self.as_str())
    }

    /// Every variant, in upstream's evaluation order — so a report can pre-seed its tally and a
    /// test can prove the vocabulary is exhaustive and uniquely spelled.
    pub const ALL: [Self; 15] = [
        Self::InvalidStatus,
        Self::IdentityMismatch,
        Self::RuntimeReference,
        Self::WaitReference,
        Self::ActiveIndex,
        Self::NonTerminal,
        Self::WorkflowReference,
        Self::NestedReference,
        Self::MissionReference,
        Self::HandoffReference,
        Self::Resumable,
        Self::UnknownAge,
        Self::Recent,
        Self::RunTombstoneMarker,
        Self::TombstoneGrace,
    ];
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The retention decision for ONE async-root entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetentionDecision {
    /// Not a candidate. Carries WHY, because the reason is `AsyncRetentionResult::skipped`'s key
    /// and a bare `false` would throw it away.
    Keep(SkipReason),
    /// A live run tree that may be retired: rename it under
    /// [`super::RUN_TOMBSTONE_PREFIX`], re-decide, and only then delete.
    /// SCOPE_14 performs the rename; this module only decides.
    Tombstone,
    /// An EXISTING `.deleting-run-*` tree, past the grace, that still decides reapable: delete
    /// it. Never returned for an untombstoned run — retiring one always goes through
    /// [`RetentionDecision::Tombstone`] first.
    Reap,
}

/// Every fact [`decide`] needs, gathered by [`scan_run_candidates`](super::scan_run_candidates).
/// Plain data: no path here is ever dereferenced by the policy.
#[derive(Clone, Debug)]
pub struct RunRetentionFacts {
    /// The async-root entry's own name — what upstream calls `path.basename(runDir)` (`:291`) and
    /// what the tombstone-prefix test is applied to (`:785`).
    pub dir_name: String,
    /// `<async_root>/<dir_name>`, ALREADY absolutised by
    /// [`normalize_tombstone_path`](super::normalize_tombstone_path), so the marker comparison at
    /// `:242` is a pure `==` here instead of upstream's two `path.resolve` calls.
    pub run_dir: PathBuf,
    /// The parsed `status.json` (`readStatus`, `:148-157`), or `None` when it is missing,
    /// unreadable or does not deserialize.
    pub status: Option<RunStatus>,
    /// `statusTimestamp` (`:174-185`): `max(ended_at ?? last_update, max(mtime(run_dir),
    /// mtime(run_dir/status.json)))`, or `None` when either `stat` failed.
    ///
    /// [CYRUP-DELTA] upstream also returns `undefined` when either logical field is
    /// present-but-non-finite. [`RunStatus::ended_at`] is `Option<i64>` and
    /// [`RunStatus::last_update`] is `i64`, so "non-finite" is unrepresentable on this side; the
    /// `stat`-fails branch is the live one, and it maps to [`SkipReason::UnknownAge`].
    pub timestamp: Option<i64>,
    /// The tombstone TREE's own mtime — `Some` iff [`Self::dir_name`] starts with
    /// [`super::RUN_TOMBSTONE_PREFIX`] and its mtime could be read.
    ///
    /// This, not the run's `started_at`, is what the grace is measured against (`:790`).
    pub tombstone_mtime: Option<i64>,
    /// The run-tombstone marker for `status.run_id`, already self-healed
    /// ([`read_run_tombstone_marker_self_healing`](super::read_run_tombstone_marker_self_healing)).
    pub marker_state: TombstoneMarkerState,
    /// `activeMarkerExists` (`:205-207`) — `<async_root>/.active-runs/<runId>`.
    pub active_marker: bool,
    /// `:298` — `<run_dir>/mission.json` exists, or the mission observer index names this run.
    pub mission_reference: bool,
    /// `hasUnresolvedRunHandoff` (`:268-277`).
    pub unresolved_handoff: bool,
    /// `hasResumableContract` (`:191-203`).
    pub resumable: bool,
}

/// Upstream `runSkipReason` (`:279-305`) PLUS its caller's tombstone branch (`:785-805`), as ONE
/// pure decision.
///
/// No I/O and no clock read — `now` is a parameter, and every filesystem answer arrives as a
/// field of `facts`.
///
/// The guard order is upstream's and must not be reordered: it is what makes the returned reason
/// the FIRST reason the run was spared, and the liveness guards (rows 4-6) deliberately precede
/// the age check so that a tracked 90-day-old run is reported as `runtime-reference` rather than
/// being reaped.
#[must_use]
pub fn decide(
    facts: &RunRetentionFacts,
    now: i64,
    retention_ms: i64,
    tombstone_grace_ms: i64,
    protected_run_ids: &BTreeSet<RunId>,
    wait_run_ids: &BTreeSet<RunId>,
) -> RetentionDecision {
    if let Some(reason) = skip_reason(facts, now, retention_ms, protected_run_ids, wait_run_ids) {
        return RetentionDecision::Keep(reason);
    }
    // `skip_reason` returning `None` guarantees a status; this re-binding is the only way to say
    // so without an `unwrap` (pi writes `status!` at `:786`, `:801`, `:809`).
    let Some(status) = facts.status.as_ref() else {
        return RetentionDecision::Keep(SkipReason::InvalidStatus);
    };

    if !facts.dir_name.starts_with(RUN_TOMBSTONE_PREFIX) {
        // pi `:807-811`: an untombstoned candidate is RETIRED, never deleted in place.
        return RetentionDecision::Tombstone;
    }

    // pi `:786-788` — an existing tombstone whose marker does not resolve to it is never reaped,
    // because after the rename the marker is the only thing that maps the run id back to a tree.
    if !marker_matches(&facts.marker_state, &status.run_id, &facts.run_dir) {
        return RetentionDecision::Keep(SkipReason::RunTombstoneMarker);
    }

    // pi `:790` — the grace is measured against the TOMBSTONE's mtime. An unreadable mtime keeps
    // it, for `unknown-age`'s reason: an age this pass cannot establish is never an old age.
    match facts.tombstone_mtime {
        Some(mtime) if now.saturating_sub(mtime) >= tombstone_grace_ms => RetentionDecision::Reap,
        _ => RetentionDecision::Keep(SkipReason::TombstoneGrace),
    }
}

/// pi `runSkipReason` (`:279-305`) proper — the fourteen guards, in upstream's order, WITHOUT the
/// tombstone branch [`decide`] wraps around them.
///
/// # Why the sweep needs this separately from [`decide`]
///
/// Upstream's RE-CHECK after the rename (`:817-819`, `:794-795`) calls `runSkipReason` and
/// **not** the tombstone branch, and the difference is load-bearing. The freshly renamed tree is
/// named `.deleting-run-*`, so [`decide`] would apply the grace to it — and the grace is measured
/// against the tree's mtime, which `rename(2)` does not touch. A run whose directory mtime happens
/// to be recent would come back as [`SkipReason::TombstoneGrace`] and be rolled back on every
/// pass, forever, while upstream deletes it. The grace exists for a tombstone that OUTLIVED the
/// pass that made it (see
/// [`ASYNC_RETENTION_TOMBSTONE_GRACE_MS`](super::ASYNC_RETENTION_TOMBSTONE_GRACE_MS)), never for
/// one minted three statements ago.
///
/// So: [`decide`] is the FIRST decision, on the entry as the scan found it; this is the SECOND
/// decision, taken after a destructive action has been staged and the wait-subscription set
/// re-read. `None` means "still reapable"; `Some(reason)` is stamped with
/// [`SkipReason::recheck_key`] and rolls the action back.
#[must_use]
pub fn skip_reason(
    facts: &RunRetentionFacts,
    now: i64,
    retention_ms: i64,
    protected_run_ids: &BTreeSet<RunId>,
    wait_run_ids: &BTreeSet<RunId>,
) -> Option<SkipReason> {
    // 1. `:289`. NOT `facts.status.as_ref()?` — `?` on an `Option<SkipReason>` return means "NO
    //    skip reason", i.e. REAP a run whose status could not be read. The reason must be named.
    let Some(status) = facts.status.as_ref() else {
        return Some(SkipReason::InvalidStatus);
    };
    let run_id = &status.run_id;

    // 2. `:290` `validRunId` — non-empty, not `.`/`..`, `basename(id) === id`. cyrup already owns
    //    that guard as `RunDirName::parse`, which additionally rejects `\` and NUL and trims;
    //    every extra rejection resolves to KEEP, which is the safe direction.
    let Some(parsed) = RunDirName::parse(run_id.as_str()) else {
        return Some(SkipReason::IdentityMismatch);
    };
    // 3. `:291` — the directory must be named after the run it claims to be, UNLESS it is a
    //    tombstone (whose name is a random id by construction). Losing the `&& !startsWith`
    //    exemption would make every tombstone permanently unreapable.
    if facts.dir_name != parsed.as_str() && !facts.dir_name.starts_with(RUN_TOMBSTONE_PREFIX) {
        return Some(SkipReason::IdentityMismatch);
    }
    // 4. `:292`
    if protected_run_ids.contains(run_id) {
        return Some(SkipReason::RuntimeReference);
    }
    // 5. `:293`
    if wait_run_ids.contains(run_id) {
        return Some(SkipReason::WaitReference);
    }
    // 6. `:294`
    if facts.active_marker {
        return Some(SkipReason::ActiveIndex);
    }
    // 7. `:295`. [CYRUP-DELTA] upstream's `TERMINAL_STATES` (`:28`) is
    //    `{complete, failed, stopped, rejected}`; cyrup's `RunState` has no `Rejected`, and
    //    `RunState::is_terminal()` is exactly `Complete | Failed | Stopped`. Use the method rather
    //    than re-spelling the set. `Paused` is explicitly NON-terminal, so a paused run is never
    //    reaped.
    if !status.state.is_terminal() {
        return Some(SkipReason::NonTerminal);
    }
    // 8. `:296`
    if has_workflow_reference(status) {
        return Some(SkipReason::WorkflowReference);
    }
    // 9. `:297`
    if has_nested_references(status) {
        return Some(SkipReason::NestedReference);
    }
    // 10. `:298`
    if facts.mission_reference {
        return Some(SkipReason::MissionReference);
    }
    // 11. `:299`
    if facts.unresolved_handoff {
        return Some(SkipReason::HandoffReference);
    }
    // 12. `:300`
    if facts.resumable {
        return Some(SkipReason::Resumable);
    }
    // 13. `:301-302`. Same trap as row 1: `facts.timestamp?` would return `None` from THIS
    //     function, which means "nothing is protecting this run" — the exact inversion of
    //     `unknown-age`. An age this pass cannot establish is never an old age.
    let Some(timestamp) = facts.timestamp else {
        return Some(SkipReason::UnknownAge);
    };
    // 14. `:303` — STRICTLY greater. A run whose timestamp is exactly the cutoff is old enough.
    if timestamp > now.saturating_sub(retention_ms) {
        return Some(SkipReason::Recent);
    }
    None
}

/// pi `:296` — `status.mode === "workflow" || status.parentWorkflowRunId || status.workflowKey`.
///
/// [CYRUP-DELTA] [`RunStatus`] has **no `parent_workflow_run_id` and no top-level `workflow_key`**
/// — those two upstream fields do not exist on this side and are therefore not checked, rather
/// than pretended to be. What cyrup has is
/// [`RunStatus::mode`]`== `[`RunMode::Workflow`], [`RunStatus::workflow_children`] (the run's own
/// live/final child inventory), and the per-step
/// [`StepStatus::workflow_key`](crate::background::StepStatus::workflow_key) that carries the
/// stable lane key upstream also puts at the top level. Any of the three is a workflow reference.
fn has_workflow_reference(status: &RunStatus) -> bool {
    status.mode == RunMode::Workflow
        || status.workflow_children.is_some()
        || status.steps.iter().any(|step| step.workflow_key.is_some())
}

/// pi `hasNestedReferences` (`:187-189`) — `status.isNested === true || steps.some(s =>
/// s.children?.length)`.
///
/// [CYRUP-DELTA] cyrup has no `is_nested` field; that half is absent and is not simulated. The
/// second half ports exactly: upstream's `steps[].children` is `NestedRunSummary[]`
/// (`shared/types.ts:1934`), i.e. the further background runs a step itself spawned, and cyrup
/// records those as
/// [`StepStatus::nested_run_ids`](crate::background::StepStatus::nested_run_ids) — *"Run-ids of
/// any further background runs this step itself spawned (R-SA-104's nested descendants)"*. A run
/// whose steps have descendants is never reaped, because reaping the parent would orphan the
/// children's only route back to it.
///
/// `ParallelGroupStatus::children` is deliberately NOT consulted: those are the group's own
/// member step records, not nested RUNS, and treating them as references would make every
/// parallel run permanently unreapable.
fn has_nested_references(status: &RunStatus) -> bool {
    status
        .steps
        .iter()
        .any(|step| !step.nested_run_ids.is_empty())
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
    use crate::background::{RunState, StepStatus};

    const RUN: &str = "0123456789abcdef0123456789abcdef";
    const NOW: i64 = 1_700_000_000_000;
    const RETENTION: i64 = super::super::ASYNC_RETENTION_MS;
    const GRACE: i64 = super::super::ASYNC_RETENTION_TOMBSTONE_GRACE_MS;

    fn run_id(token: &str) -> RunId {
        RunId::from_token(token.to_string())
    }

    fn terminal_status(token: &str) -> RunStatus {
        let mut status = RunStatus::queued(run_id(token), RunMode::Single, Some(7));
        status.state = RunState::Complete;
        status.started_at = 1;
        status.last_update = 1;
        status.ended_at = Some(1);
        status
    }

    /// A reapable-looking orphan: terminal, correctly named, older than the window, no live
    /// reference of any kind.
    fn orphan(token: &str) -> RunRetentionFacts {
        RunRetentionFacts {
            dir_name: token.to_string(),
            run_dir: PathBuf::from("/async").join(token),
            status: Some(terminal_status(token)),
            timestamp: Some(NOW - RETENTION - 1),
            tombstone_mtime: None,
            marker_state: TombstoneMarkerState::Absent,
            active_marker: false,
            mission_reference: false,
            unresolved_handoff: false,
            resumable: false,
        }
    }

    /// The same run, already renamed onto a tombstone, with a marker that resolves to it.
    fn tombstoned(token: &str, dir_name: &str, tombstone_mtime: i64) -> RunRetentionFacts {
        let run_dir = PathBuf::from("/async").join(dir_name);
        RunRetentionFacts {
            dir_name: dir_name.to_string(),
            marker_state: TombstoneMarkerState::Present {
                run_id: run_id(token),
                tombstone_path: run_dir.clone(),
            },
            tombstone_mtime: Some(tombstone_mtime),
            run_dir,
            ..orphan(token)
        }
    }

    fn decide_default(facts: &RunRetentionFacts) -> RetentionDecision {
        decide(
            facts,
            NOW,
            RETENTION,
            GRACE,
            &BTreeSet::new(),
            &BTreeSet::new(),
        )
    }

    /// `:303` is `timestamp > cutoff → "recent"` — STRICTLY greater. The boundary pair is the
    /// whole test: exactly-at-cutoff proceeds, one millisecond younger is kept.
    #[test]
    fn a_run_younger_than_the_window_is_kept() {
        let cutoff = NOW - RETENTION;

        let mut at_cutoff = orphan(RUN);
        at_cutoff.timestamp = Some(cutoff);
        assert_eq!(decide_default(&at_cutoff), RetentionDecision::Tombstone);

        let mut one_ms_younger = orphan(RUN);
        one_ms_younger.timestamp = Some(cutoff + 1);
        assert_eq!(
            decide_default(&one_ms_younger),
            RetentionDecision::Keep(SkipReason::Recent)
        );
    }

    /// An untombstoned candidate past the window yields `Tombstone`, NOT `Reap` — retiring a run
    /// always goes through the rename first (`:807-811`). `Reap` is only ever returned for an
    /// entry that was ALREADY `.deleting-run-*` when the pass began, which is why this test is
    /// named for the decision it asserts rather than for the deletion that eventually follows it.
    #[test]
    fn an_orphaned_terminal_run_past_the_window_is_tombstoned_not_reaped() {
        assert_eq!(decide_default(&orphan(RUN)), RetentionDecision::Tombstone);
    }

    /// The liveness guards precede the age check (`:292-294` before `:303`), so age can never
    /// override them. Asserting the REASON, not merely "not Reap", is what catches a reordering.
    #[test]
    fn a_still_tracked_run_is_never_reaped_regardless_of_age() {
        let facts = orphan(RUN);
        let far_future = i64::MAX / 4;

        let protected = BTreeSet::from([run_id(RUN)]);
        assert_eq!(
            decide(
                &facts,
                far_future,
                RETENTION,
                GRACE,
                &protected,
                &BTreeSet::new()
            ),
            RetentionDecision::Keep(SkipReason::RuntimeReference)
        );

        let waiting = BTreeSet::from([run_id(RUN)]);
        assert_eq!(
            decide(
                &facts,
                far_future,
                RETENTION,
                GRACE,
                &BTreeSet::new(),
                &waiting
            ),
            RetentionDecision::Keep(SkipReason::WaitReference)
        );

        let mut indexed_active = facts.clone();
        indexed_active.active_marker = true;
        assert_eq!(
            decide(
                &indexed_active,
                far_future,
                RETENTION,
                GRACE,
                &BTreeSet::new(),
                &BTreeSet::new()
            ),
            RetentionDecision::Keep(SkipReason::ActiveIndex)
        );
    }

    /// All six `RunState` values, because the one that matters is `Paused`: it is explicitly
    /// non-terminal (`RunState::is_terminal`), so a paused run is never reaped.
    #[test]
    fn a_non_terminal_run_is_never_reaped() {
        for state in [RunState::Complete, RunState::Failed, RunState::Stopped] {
            let mut facts = orphan(RUN);
            if let Some(status) = facts.status.as_mut() {
                status.state = state;
            }
            assert_eq!(
                decide_default(&facts),
                RetentionDecision::Tombstone,
                "{state:?} is terminal and must proceed"
            );
        }
        for state in [RunState::Queued, RunState::Running, RunState::Paused] {
            let mut facts = orphan(RUN);
            if let Some(status) = facts.status.as_mut() {
                status.state = state;
            }
            assert_eq!(
                decide_default(&facts),
                RetentionDecision::Keep(SkipReason::NonTerminal),
                "{state:?} is NOT terminal and must be kept"
            );
        }
    }

    /// The grace is measured against the TOMBSTONE TREE's mtime (`:790`), never against the run's
    /// own `started_at` — hence an ancient run with a freshly minted tombstone.
    #[test]
    fn a_tombstoned_run_inside_the_grace_is_kept() {
        let facts = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE + 1);
        assert_eq!(facts.status.as_ref().unwrap().started_at, 1, "an old run");
        assert_eq!(
            decide_default(&facts),
            RetentionDecision::Keep(SkipReason::TombstoneGrace)
        );
    }

    #[test]
    fn a_tombstoned_run_past_the_grace_is_reaped() {
        let facts = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1);
        assert_eq!(decide_default(&facts), RetentionDecision::Reap);

        // And the boundary itself: exactly `grace` old is past it (`:790` keeps only while
        // `now - mtime < grace`).
        let at_boundary = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE);
        assert_eq!(decide_default(&at_boundary), RetentionDecision::Reap);
    }

    /// An unreadable tombstone mtime is a keep, for `unknown-age`'s reason: an age this pass
    /// cannot establish is never an old age.
    #[test]
    fn a_tombstone_with_an_unreadable_mtime_is_kept() {
        let mut facts = tombstoned(RUN, ".deleting-run-abc", 0);
        facts.tombstone_mtime = None;
        assert_eq!(
            decide_default(&facts),
            RetentionDecision::Keep(SkipReason::TombstoneGrace)
        );
    }

    /// Three concrete fact sets, three variants, in one body — so collapsing the enum is both a
    /// compile error and an assertion failure.
    #[test]
    fn the_policy_has_three_outcomes_not_two() {
        let mut recent = orphan(RUN);
        recent.timestamp = Some(NOW);
        let keep = decide_default(&recent);
        let tombstone = decide_default(&orphan(RUN));
        let reap = decide_default(&tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1));

        assert_eq!(keep, RetentionDecision::Keep(SkipReason::Recent));
        assert_eq!(tombstone, RetentionDecision::Tombstone);
        assert_eq!(reap, RetentionDecision::Reap);
        assert_ne!(keep, tombstone);
        assert_ne!(tombstone, reap);
        assert_ne!(keep, reap);
    }

    /// Every `SkipReason` must be producible by some fact set, and no two may share an
    /// `as_str()` — these strings are `AsyncRetentionResult::skipped`'s keys, compared against
    /// upstream's vocabulary.
    #[test]
    fn every_skip_reason_is_reachable_and_uniquely_named() {
        let mut produced: BTreeSet<SkipReason> = BTreeSet::new();
        let mut record =
            |facts: &RunRetentionFacts, protected: &BTreeSet<RunId>, waiting: &BTreeSet<RunId>| {
                if let RetentionDecision::Keep(reason) =
                    decide(facts, NOW, RETENTION, GRACE, protected, waiting)
                {
                    produced.insert(reason);
                }
            };
        let none = BTreeSet::new();

        let mut no_status = orphan(RUN);
        no_status.status = None;
        record(&no_status, &none, &none);

        let mut bad_id = orphan(RUN);
        if let Some(status) = bad_id.status.as_mut() {
            status.run_id = run_id("..");
        }
        record(&bad_id, &none, &none);

        record(&orphan(RUN), &BTreeSet::from([run_id(RUN)]), &none);
        record(&orphan(RUN), &none, &BTreeSet::from([run_id(RUN)]));

        let mut active = orphan(RUN);
        active.active_marker = true;
        record(&active, &none, &none);

        let mut queued = orphan(RUN);
        if let Some(status) = queued.status.as_mut() {
            status.state = RunState::Queued;
        }
        record(&queued, &none, &none);

        let mut workflow = orphan(RUN);
        if let Some(status) = workflow.status.as_mut() {
            status.mode = RunMode::Workflow;
        }
        record(&workflow, &none, &none);

        let mut nested = orphan(RUN);
        if let Some(status) = nested.status.as_mut() {
            let mut step = StepStatus::pending("child");
            step.nested_run_ids.push(run_id("nested-1"));
            status.steps.push(step);
        }
        record(&nested, &none, &none);

        let mut mission = orphan(RUN);
        mission.mission_reference = true;
        record(&mission, &none, &none);

        let mut handoff = orphan(RUN);
        handoff.unresolved_handoff = true;
        record(&handoff, &none, &none);

        let mut resumable = orphan(RUN);
        resumable.resumable = true;
        record(&resumable, &none, &none);

        let mut unknown_age = orphan(RUN);
        unknown_age.timestamp = None;
        record(&unknown_age, &none, &none);

        let mut recent = orphan(RUN);
        recent.timestamp = Some(NOW);
        record(&recent, &none, &none);

        let mut marker_missing = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1);
        marker_missing.marker_state = TombstoneMarkerState::Absent;
        record(&marker_missing, &none, &none);

        record(
            &tombstoned(RUN, ".deleting-run-abc", NOW - GRACE + 1),
            &none,
            &none,
        );

        assert_eq!(
            produced,
            SkipReason::ALL.into_iter().collect::<BTreeSet<_>>(),
            "every SkipReason must be produced by a real fact set"
        );

        let names: BTreeSet<&str> = SkipReason::ALL.iter().map(|r| r.as_str()).collect();
        assert_eq!(
            names.len(),
            SkipReason::ALL.len(),
            "two SkipReason variants share an as_str()"
        );
        assert_eq!(
            SkipReason::Recent.recheck_key(),
            "recheck-recent",
            "the recheck- prefix composes with the same vocabulary, so SCOPE_14 never invents a \
             second spelling"
        );
    }

    /// `:291`, both halves. The exemption for a tombstone-prefixed name is the half most likely to
    /// be lost, and losing it makes every tombstone permanently unreapable.
    #[test]
    fn a_run_whose_directory_name_disagrees_with_its_status_is_kept() {
        let mut misnamed = orphan(RUN);
        misnamed.dir_name = "some-other-name".to_string();
        assert_eq!(
            decide_default(&misnamed),
            RetentionDecision::Keep(SkipReason::IdentityMismatch)
        );

        // A `.deleting-run-*` name never matches its run id BY CONSTRUCTION, so it is exempt.
        let exempt = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1);
        assert_ne!(exempt.dir_name, RUN);
        assert_eq!(decide_default(&exempt), RetentionDecision::Reap);
    }

    #[test]
    fn a_run_with_an_unreadable_mtime_is_kept_not_reaped() {
        let mut facts = orphan(RUN);
        facts.timestamp = None;
        assert_eq!(
            decide_default(&facts),
            RetentionDecision::Keep(SkipReason::UnknownAge)
        );
    }

    /// `:247` vs `:248`: `Absent` and `Unreadable` behave identically on the RUN side (both refuse
    /// the reap), and that is the point — an existing tombstone tree whose marker cannot be
    /// resolved to it is never deleted.
    #[test]
    fn an_unreadable_tombstone_marker_blocks_the_reap() {
        let mut unreadable = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1);
        unreadable.marker_state = TombstoneMarkerState::Unreadable;
        assert_eq!(
            decide_default(&unreadable),
            RetentionDecision::Keep(SkipReason::RunTombstoneMarker)
        );

        let mut absent = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1);
        absent.marker_state = TombstoneMarkerState::Absent;
        assert_eq!(
            decide_default(&absent),
            RetentionDecision::Keep(SkipReason::RunTombstoneMarker)
        );

        // A marker that resolves to a DIFFERENT tree is equally not a match.
        let mut elsewhere = tombstoned(RUN, ".deleting-run-abc", NOW - GRACE - 1);
        elsewhere.marker_state = TombstoneMarkerState::Present {
            run_id: run_id(RUN),
            tombstone_path: PathBuf::from("/async/.deleting-run-somewhere-else"),
        };
        assert_eq!(
            decide_default(&elsewhere),
            RetentionDecision::Keep(SkipReason::RunTombstoneMarker)
        );
    }

    /// The workflow guard's three cyrup disjuncts, each on its own — the two upstream fields that
    /// do not exist here (`parentWorkflowRunId`, top-level `workflowKey`) cannot be tested because
    /// they cannot be written.
    #[test]
    fn any_workflow_reference_keeps_the_run() {
        let mut by_mode = orphan(RUN);
        if let Some(status) = by_mode.status.as_mut() {
            status.mode = RunMode::Workflow;
        }
        assert_eq!(
            decide_default(&by_mode),
            RetentionDecision::Keep(SkipReason::WorkflowReference)
        );

        let mut by_children = orphan(RUN);
        if let Some(status) = by_children.status.as_mut() {
            status.workflow_children = serde_json::from_value(serde_json::json!({
                "version": 1,
                "parentToolCallId": "tc-1",
                "workflowRunId": RUN,
                "inventoryComplete": true,
                "workflowState": "completed",
                "children": [],
            }))
            .ok();
            assert!(status.workflow_children.is_some());
        }
        assert_eq!(
            decide_default(&by_children),
            RetentionDecision::Keep(SkipReason::WorkflowReference)
        );

        let mut by_step_key = orphan(RUN);
        if let Some(status) = by_step_key.status.as_mut() {
            let mut step = StepStatus::pending("child");
            step.workflow_key = crate::workflows::WorkflowKey::parse("lane-a").ok();
            assert!(step.workflow_key.is_some());
            status.steps.push(step);
        }
        assert_eq!(
            decide_default(&by_step_key),
            RetentionDecision::Keep(SkipReason::WorkflowReference)
        );
    }

    /// Purity itself is a property of the signature — [`decide`] takes `now` and every filesystem
    /// answer as arguments and owns no I/O — so no test can assert it, and an earlier name for this
    /// one claimed exactly that. What IS worth pinning is the arithmetic at the two ends of the
    /// clock's range: the cutoff is computed with `saturating_sub`, so neither extreme may wrap
    /// into the opposite decision, and a wrap at `i64::MAX` would reap a run this pass believes is
    /// from the future.
    #[test]
    fn the_cutoff_saturates_at_both_ends_of_the_clock() {
        let facts = orphan(RUN);
        assert_eq!(
            decide(
                &facts,
                0,
                RETENTION,
                GRACE,
                &BTreeSet::new(),
                &BTreeSet::new()
            ),
            RetentionDecision::Keep(SkipReason::Recent),
            "at now = 0 the cutoff is below every real timestamp, so everything is 'recent'"
        );
        assert_eq!(
            decide(
                &facts,
                i64::MIN,
                RETENTION,
                GRACE,
                &BTreeSet::new(),
                &BTreeSet::new()
            ),
            RetentionDecision::Keep(SkipReason::Recent),
            "and `now - retention` saturates instead of wrapping to a huge POSITIVE cutoff, which \
             would read every run as ancient and reap the lot"
        );
        assert_eq!(
            decide(
                &facts,
                i64::MAX,
                RETENTION,
                GRACE,
                &BTreeSet::new(),
                &BTreeSet::new()
            ),
            RetentionDecision::Tombstone
        );
    }
}
