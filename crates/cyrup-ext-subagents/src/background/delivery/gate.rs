//! [`SessionGate`] — the session-membership check, in its two upstream strictness classes.
//!
//! Ports the predicate pi spells out inline at `async-dismiss-action.ts:32`,
//! `async-steering-action.ts:48`, `async-stop-action.ts:34`, `run-status.ts:249`, `:494`, `:521`
//! and `async-job-tracker.ts:658`, `:709`.

use crate::identity::SessionId;

/// Whether an operation may touch a record owned by some session.
///
/// # Two classes, differing in exactly one arm
///
/// Upstream writes this check by hand at eight call sites, in **two** shapes that look nearly
/// identical and are not interchangeable:
///
/// ```text
/// STRICT      !currentSessionId || x.sessionId !== currentSessionId    no session => REFUSE
/// PERMISSIVE   currentSessionId && x.sessionId !== currentSessionId    no session => ALLOW
/// ```
///
/// When a current session exists the two agree exactly. They diverge **only** when the caller has
/// no session identity at all — which is precisely why collapsing them looks harmless in testing
/// and is not. A strict-ified `stop` breaks headless and SDK embedders, which legitimately have no
/// session and must still be able to control their own runs. A permissive-ised `dismiss` lets a
/// host with no identity dismiss anything in the directory.
///
/// Which class a given call site takes is **ported, not chosen**; every construction site names
/// the upstream line it comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionGate {
    /// No current session means refuse.
    ///
    /// pi `async-dismiss-action.ts:32` (`!state.currentSessionId || status.sessionId !== ...`) and
    /// `run-status.ts:249`, which *throws* rather than returning a refusal.
    Strict,
    /// No current session means allow.
    ///
    /// pi `async-stop-action.ts:34`, `async-steering-action.ts:48`, `run-status.ts:494`/`:521`,
    /// `async-job-tracker.ts:658`/`:709` (`state.currentSessionId && ...`).
    Permissive,
}

impl SessionGate {
    /// `true` if a caller in `current` may act on a record owned by `record`.
    ///
    /// Note an absent `record` (a run with no recorded session) is **refused** whenever `current`
    /// is present — `Some(cur) != None`. That matches pi's `!==` against `undefined` and is the
    /// same rule `run_status.rs`'s listing filter already applies: an unattributed run belongs to
    /// nobody, so it is nobody's to act on.
    #[must_use]
    pub fn admits(self, current: Option<&SessionId>, record: Option<&SessionId>) -> bool {
        match current {
            None => self == Self::Permissive,
            Some(current) => record == Some(current),
        }
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

    fn sid(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }

    /// The exhaustive 2x3x2 table: {Strict, Permissive} x {no current, own, foreign} x present or
    /// absent record. Written out rather than generated so each row's *reason* is visible.
    #[test]
    fn the_full_admission_table() {
        let mine = sid("s1");
        let theirs = sid("s2");

        // --- a caller WITH a session: both classes agree, always ---
        for gate in [SessionGate::Strict, SessionGate::Permissive] {
            assert!(
                gate.admits(Some(&mine), Some(&mine)),
                "{gate:?}: own record"
            );
            assert!(
                !gate.admits(Some(&mine), Some(&theirs)),
                "{gate:?}: foreign record"
            );
            assert!(
                !gate.admits(Some(&mine), None),
                "{gate:?}: unattributed record"
            );
        }

        // --- a caller WITHOUT a session: the ONLY place the classes differ ---
        assert!(!SessionGate::Strict.admits(None, Some(&mine)));
        assert!(!SessionGate::Strict.admits(None, None));
        assert!(SessionGate::Permissive.admits(None, Some(&mine)));
        assert!(SessionGate::Permissive.admits(None, None));
    }

    #[test]
    fn the_two_classes_differ_only_in_the_no_session_arm() {
        // Stated as its own property: if this ever becomes vacuously true, the classes have been
        // collapsed and one of the two upstream behaviours has been silently dropped.
        let mine = sid("s1");
        let theirs = sid("s2");
        for record in [Some(&mine), Some(&theirs), None] {
            assert_eq!(
                SessionGate::Strict.admits(Some(&mine), record),
                SessionGate::Permissive.admits(Some(&mine), record),
                "with a current session the classes must agree (record={record:?})"
            );
        }
        assert_ne!(
            SessionGate::Strict.admits(None, Some(&mine)),
            SessionGate::Permissive.admits(None, Some(&mine)),
            "with no current session they MUST differ, or a class has been lost"
        );
    }

    #[test]
    fn a_headless_host_can_still_control_its_own_runs() {
        // The concrete reason `Permissive` exists: an SDK embedder has no session identity and
        // must not be locked out of stop/steer.
        assert!(SessionGate::Permissive.admits(None, Some(&sid("whatever"))));
    }

    #[test]
    fn a_headless_host_cannot_dismiss() {
        // The concrete reason `Strict` exists: dismissal asserts ownership, so an anonymous host
        // must not be able to claim it.
        assert!(!SessionGate::Strict.admits(None, Some(&sid("whatever"))));
    }

    #[test]
    fn an_unattributed_run_belongs_to_nobody() {
        assert!(!SessionGate::Strict.admits(Some(&sid("s1")), None));
        assert!(!SessionGate::Permissive.admits(Some(&sid("s1")), None));
    }
}
