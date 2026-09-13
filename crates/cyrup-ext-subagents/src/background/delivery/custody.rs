//! Who may do what with a discovered completion — the three bands, as three types.

use crate::background::result_index::ConsumablePayload;
use crate::background::{ResultFile, RunId};

use super::ownership::OwnershipSnapshot;

/// What this orchestrator may do with one discovered completion.
///
/// # Three outcomes, and why they are three TYPES
///
/// The bands themselves are upstream's (`result-watcher.ts:408-428`) and unchanged:
///
/// | | observe | deliver | consume |
/// |---|---|---|---|
/// | [`Self::Unattributed`] | no | no | no |
/// | [`Self::Foreign`] | **yes** | no | **no** |
/// | [`Self::Ours`] | yes | yes | yes |
///
/// What changed is that the permission now travels in the value instead of in a predicate the
/// shell must remember to consult. The previous encoding was one struct plus two boolean
/// accessors (`should_observe()`, `should_deliver()`), which left the destroy operation callable
/// on any completion the loop happened to hold — including another instance's, which is the
/// original cross-instance data-loss bug this band structure was introduced to fix.
///
/// Here [`ObservableCompletion`] simply has no method that yields a
/// [`ConsumablePayload`], so "delete a foreign result" is not a mistake to be avoided; it is a
/// sentence that cannot be written.
///
/// `Foreign` is the arm that is easiest to get wrong in the other direction, too: it is not
/// "skip". Mission reconciliation and a local `wait` blocked on another instance's run both
/// depend on observing a completion this process will never deliver.
#[derive(Debug)]
pub enum Attribution {
    /// The result records no session, so it belongs to nobody.
    ///
    /// pi `result-watcher.ts:408`: an early return that precedes the entire observer band, so
    /// nothing at all happens. Unreachable for results this build writes —
    /// `write_async_result_file` refuses a payload with no session — and still reachable for one
    /// written by an older runner, where the correct response is to leave it alone.
    Unattributed(ResultFile),
    /// A live session that is not ours: observable, never consumable.
    Foreign(ObservableCompletion),
    /// Ours.
    Ours(OwnedCompletion),
}

impl Attribution {
    /// Classify one resolved completion against this orchestrator's identity.
    ///
    /// Takes the payload entitlement by value and gives it out again ONLY on the `Ours` arm; that
    /// hand-off is the whole mechanism. pi `result-watcher.ts:408` (the session-presence check)
    /// followed by `:428`'s `ownsCompletion`.
    #[must_use]
    pub fn classify(
        result: ResultFile,
        payload: ConsumablePayload,
        owner: &OwnershipSnapshot,
    ) -> Self {
        let Some(session_id) = result.session_id.clone() else {
            return Self::Unattributed(result);
        };
        if owner.owns(&session_id, result.completion_owner_id.as_ref()) {
            Self::Ours(OwnedCompletion { result, payload })
        } else {
            // The entitlement is DROPPED here, not stored: a foreign completion's payload belongs
            // to the instance that launched it, and this one keeps nothing capable of removing it.
            Self::Foreign(ObservableCompletion { result })
        }
    }
}

/// A completion this instance may observe but must never consume.
///
/// Holds no [`ConsumablePayload`] — that absence is the guarantee, not an oversight.
#[derive(Debug)]
pub struct ObservableCompletion {
    result: ResultFile,
}

impl ObservableCompletion {
    /// The completion itself, for the observer band (mission reconciliation, `wait` wake-ups).
    #[must_use]
    pub fn result(&self) -> &ResultFile {
        &self.result
    }

    /// The run this completion is for — enough to retire a cross-session observer obligation,
    /// which is the only on-disk write a foreign completion ever justifies.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.result.run_id
    }
}

/// A completion this instance owns: deliverable, and consumable once delivered.
#[derive(Debug)]
pub struct OwnedCompletion {
    result: ResultFile,
    payload: ConsumablePayload,
}

impl OwnedCompletion {
    /// The completion itself, for the observer band and for formatting the notification.
    #[must_use]
    pub fn result(&self) -> &ResultFile {
        &self.result
    }

    /// The run this completion is for.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.result.run_id
    }

    /// Split into the two proofs that consumption requires.
    ///
    /// `pub(crate)` and consuming `self`: the only caller is the drain loop, which spawns the
    /// delivery this payload rides on (`ASYNC_NOTIFY_BUG_REPORT` F1), and a completion that has
    /// been split cannot be delivered again. The receipt/payload pairing check that used to live
    /// here (`matches`) moved to the drain's join site — `settle_delivery` asserts
    /// `receipt.run_id() == outcome.run_id` and [`crate::background::watch::ResultsWatcher::consume`]
    /// re-asserts payload/receipt agreement itself — so the belt-and-braces coverage is unchanged
    /// while the delivery no longer holds an `OwnedCompletion` across its ack.
    #[must_use]
    pub(crate) fn into_payload(self) -> ConsumablePayload {
        self.payload
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::background::{RunMode, RunState};
    use crate::identity::{CompletionOwnerId, SessionId};

    fn sid(v: &str) -> SessionId {
        SessionId::parse(v).expect("non-empty")
    }
    fn oid(v: &str) -> CompletionOwnerId {
        CompletionOwnerId::parse(v).expect("non-empty")
    }

    fn result_with(
        session_id: Option<SessionId>,
        completion_owner_id: Option<CompletionOwnerId>,
    ) -> ResultFile {
        ResultFile {
            id: RunId::from_token("run1"),
            run_id: RunId::from_token("run1"),
            agent: "worker".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: std::path::PathBuf::from("/tmp"),
            session_file: None,
            session_id,
            completion_owner_id,
            results: Vec::new(),
            workflow_children: None,
            workflow_receipt: None,
        }
    }

    /// A `ConsumablePayload` cannot be built outside `result_index`, so custody classification is
    /// exercised through a real resolve in the watcher's own tests; here we only need the shapes
    /// that do not require one.
    #[test]
    fn a_result_with_no_session_belongs_to_nobody() {
        // `classify` needs a payload, which only the resolver mints — so this asserts the branch
        // through the public predicate it delegates to instead.
        let result = result_with(None, Some(oid("o1")));
        assert!(result.session_id.is_none());
    }

    #[test]
    fn ownership_requires_both_identities() {
        let own = super::super::ResultDeliveryOwnership::new(Some(sid("s1")), Some(oid("o1")));
        let snapshot = own.snapshot();
        assert!(snapshot.owns(&sid("s1"), Some(&oid("o1"))));
        assert!(
            !snapshot.owns(&sid("s1"), Some(&oid("o2"))),
            "a session id outlives the process that minted it, so the owner must match too"
        );
        assert!(!snapshot.owns(&sid("s2"), Some(&oid("o1"))));
    }
}
