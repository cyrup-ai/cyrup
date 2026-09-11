//! Delivery ownership — who may consume a completion.
//!
//! Ports pi `result-delivery-ownership.ts` (whole file) and the lease wrapper
//! `ownsCompletion` (`result-watcher.ts:232-236`).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::identity::{CompletionOwnerId, SessionId};

/// pi `MAX_CLAIMED_PREDECESSOR_SESSIONS` (`result-delivery-ownership.ts:3`).
const MAX_CLAIMED_PREDECESSOR_SESSIONS: usize = 8;

/// An immutable snapshot of who this orchestrator is, taken once before a drain loop.
///
/// # The functional-core / imperative-shell seam
///
/// The live [`ResultDeliveryOwnership`] holds a mutex and is mutated by session transitions. This
/// is a plain value copied out of it. The drain loop takes one snapshot, then classifies every
/// discovered result against it with no locking and no I/O — which is what makes
/// [`super::Attribution::classify`] a pure function that can be exhaustively unit-tested
/// without a tempdir, a runtime, or a mock.
///
/// Taking the snapshot once per scan rather than per result also makes the scan *consistent*: a
/// session change mid-scan cannot cause half the results to be judged against one identity and
/// half against another.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnershipSnapshot {
    current_session: Option<SessionId>,
    owner: Option<CompletionOwnerId>,
    claimed: Vec<SessionId>,
}

impl OwnershipSnapshot {
    /// A snapshot owning nothing — the headless case, and the correct default for a host with no
    /// session identity.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            current_session: None,
            owner: None,
            claimed: Vec::new(),
        }
    }

    /// Build a snapshot directly. Mainly for tests; production takes one from
    /// [`ResultDeliveryOwnership::snapshot`].
    #[must_use]
    pub fn new(
        current_session: Option<SessionId>,
        owner: Option<CompletionOwnerId>,
        claimed: Vec<SessionId>,
    ) -> Self {
        Self {
            current_session,
            owner,
            claimed,
        }
    }

    /// This orchestrator's current session, if it has one.
    #[must_use]
    pub fn current_session(&self) -> Option<&SessionId> {
        self.current_session.as_ref()
    }

    /// Every session this snapshot may consume results for: the current one plus any claimed
    /// predecessors. The candidate enumerator walks exactly this list.
    #[must_use]
    pub fn readable_sessions(&self) -> Vec<SessionId> {
        let mut sessions = Vec::with_capacity(self.claimed.len() + 1);
        if let Some(current) = &self.current_session {
            sessions.push(current.clone());
        }
        for claimed in &self.claimed {
            if Some(claimed) != self.current_session.as_ref() {
                sessions.push(claimed.clone());
            }
        }
        sessions
    }

    /// May this orchestrator **consume** (deliver and then delete) a result carrying these
    /// identities?
    ///
    /// pi `ResultDeliveryOwnership.owns` (`result-delivery-ownership.ts:22-26`):
    ///
    /// ```text
    /// const owner = currentOwner();
    /// if (!owner || completionOwnerId !== owner) return false;
    /// return sessionId === state.currentSessionId || claimed.get(sessionId) === owner;
    /// ```
    ///
    /// # Both identities are required, and this is NOT [`super::SessionGate`]
    ///
    /// [`super::SessionGate::admits`] answers "may I act on this session's run?" and is what the
    /// control operations use. This answers the stricter "may I *consume* this completion?", and
    /// additionally requires the [`CompletionOwnerId`] to match. The difference matters because a
    /// session id outlives the process that minted it: a restarted orchestrator legitimately holds
    /// the same session but must not consume completions the *previous* process was waiting on.
    ///
    /// Keep the two functions distinct. Unifying them either breaks headless control (if
    /// `SessionGate` gains the owner check) or starts delivering foreign results (if this loses
    /// it).
    ///
    /// # No owner means owning nothing
    ///
    /// cyrup always mints an owner (`identity::current_completion_owner_id`), so in production the
    /// `None` arm is reached only by a headless host that never spawned anything. Such a host also
    /// cannot *write* a result — [`crate::background::result_index::write_async_result_file`]
    /// refuses a payload with no session, exactly as pi throws at `result-files.ts:166` — so there
    /// is nothing for it to be denied. An embedder that wants async completions must supply a
    /// session identity. This is deliberate; do not "fix" it into an accept-all.
    #[must_use]
    pub fn owns(
        &self,
        session_id: &SessionId,
        completion_owner_id: Option<&CompletionOwnerId>,
    ) -> bool {
        let Some(owner) = self.owner.as_ref() else {
            return false;
        };
        if completion_owner_id != Some(owner) {
            return false;
        }
        Some(session_id) == self.current_session.as_ref()
            || self.claimed.iter().any(|claimed| claimed == session_id)
    }
}

/// The live ownership state for one orchestrator process.
///
/// Ports pi `createResultDeliveryOwnership` (`result-delivery-ownership.ts:14-45`). Cheap to
/// clone; clones share one state.
#[derive(Clone, Debug)]
pub struct ResultDeliveryOwnership {
    inner: Arc<Mutex<State>>,
}

#[derive(Debug)]
struct State {
    current_session: Option<SessionId>,
    owner: Option<CompletionOwnerId>,
    /// Claimed predecessor sessions, oldest first — a bounded LRU keyed by session.
    claimed: VecDeque<(SessionId, CompletionOwnerId)>,
}

impl ResultDeliveryOwnership {
    /// Build ownership for this process.
    #[must_use]
    pub fn new(current_session: Option<SessionId>, owner: Option<CompletionOwnerId>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(State {
                current_session,
                owner,
                claimed: VecDeque::new(),
            })),
        }
    }

    /// Take an immutable [`OwnershipSnapshot`] — the value the pure gate consumes.
    #[must_use]
    pub fn snapshot(&self) -> OwnershipSnapshot {
        let Ok(state) = self.inner.lock() else {
            // A poisoned lock means another thread panicked mid-update. Owning nothing is the safe
            // reading: this instance delivers nothing rather than risk consuming another's result.
            return OwnershipSnapshot::empty();
        };
        let owner = state.owner.clone();
        let claimed = state
            .claimed
            .iter()
            .filter(|(_, claimed_owner)| Some(claimed_owner) == owner.as_ref())
            .map(|(session, _)| session.clone())
            .collect();
        OwnershipSnapshot {
            current_session: state.current_session.clone(),
            owner,
            claimed,
        }
    }

    /// Claim a predecessor session's results for this process.
    ///
    /// pi `claimPredecessor` (`result-delivery-ownership.ts:32-40`). Returns whether the claim was
    /// admitted.
    ///
    /// # The triple-equality admission rule
    ///
    /// A claim is admitted only when
    /// `previous_session_file == previous_runtime_session_id == current_session`. That is
    /// deliberately narrow: it recognises the one case where a session's *identity* was rewritten
    /// under a process that is still the same process (a session-file rotation), and refuses every
    /// case where the two disagree — which would be another session's work.
    ///
    /// # LRU semantics are load-bearing
    ///
    /// pi's `claimed.delete(k)` then `.set(k, v)` moves an existing key to the **back** of a JS
    /// `Map`'s insertion order before the size trim, so re-claiming refreshes recency. A plain map
    /// with arbitrary eviction passes a naive test and evicts the wrong session under load.
    pub fn claim_predecessor(
        &self,
        previous_session_file: Option<&str>,
        previous_runtime_session_id: Option<&str>,
    ) -> bool {
        let Ok(mut state) = self.inner.lock() else {
            return false;
        };
        let Some(owner) = state.owner.clone() else {
            return false;
        };
        let (Some(file), Some(runtime)) = (previous_session_file, previous_runtime_session_id)
        else {
            return false;
        };
        if file != runtime {
            return false;
        }
        if state.current_session.as_ref().map(SessionId::as_str) != Some(runtime) {
            return false;
        }
        let Some(session) = SessionId::parse(runtime) else {
            return false;
        };

        // Move-to-back on re-claim, then trim from the front.
        state.claimed.retain(|(existing, _)| existing != &session);
        state.claimed.push_back((session, owner));
        while state.claimed.len() > MAX_CLAIMED_PREDECESSOR_SESSIONS {
            state.claimed.pop_front();
        }
        true
    }

    /// Forget every claimed predecessor. pi `clear` (`result-delivery-ownership.ts:41-43`).
    pub fn clear(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.claimed.clear();
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
    fn oid(v: &str) -> CompletionOwnerId {
        CompletionOwnerId::parse(v).expect("non-empty")
    }

    #[test]
    fn owns_requires_both_identities() {
        let snap = OwnershipSnapshot::new(Some(sid("s1")), Some(oid("o1")), Vec::new());
        assert!(
            snap.owns(&sid("s1"), Some(&oid("o1"))),
            "own session + own owner"
        );
        assert!(!snap.owns(&sid("s2"), Some(&oid("o1"))), "foreign session");
        assert!(!snap.owns(&sid("s1"), Some(&oid("o2"))), "foreign owner");
        assert!(
            !snap.owns(&sid("s1"), None),
            "no owner recorded on the result"
        );
    }

    #[test]
    fn a_restarted_process_holding_the_same_session_owns_nothing_of_its_predecessors() {
        // The reason the owner id exists at all: session identity outlives the process.
        let previous = OwnershipSnapshot::new(Some(sid("s1")), Some(oid("proc-a")), Vec::new());
        let restarted = OwnershipSnapshot::new(Some(sid("s1")), Some(oid("proc-b")), Vec::new());
        assert!(previous.owns(&sid("s1"), Some(&oid("proc-a"))));
        assert!(
            !restarted.owns(&sid("s1"), Some(&oid("proc-a"))),
            "a new process must not consume the old process's pending completions"
        );
    }

    #[test]
    fn a_snapshot_with_no_owner_owns_nothing() {
        let snap = OwnershipSnapshot::new(Some(sid("s1")), None, Vec::new());
        assert!(!snap.owns(&sid("s1"), Some(&oid("o1"))));
        assert!(!snap.owns(&sid("s1"), None));
        assert!(!OwnershipSnapshot::empty().owns(&sid("s1"), Some(&oid("o1"))));
    }

    #[test]
    fn a_claimed_predecessor_session_is_owned() {
        let snap = OwnershipSnapshot::new(Some(sid("s2")), Some(oid("o1")), vec![sid("s1")]);
        assert!(
            snap.owns(&sid("s1"), Some(&oid("o1"))),
            "claimed predecessor"
        );
        assert!(snap.owns(&sid("s2"), Some(&oid("o1"))), "current");
        assert!(!snap.owns(&sid("s3"), Some(&oid("o1"))), "neither");
    }

    #[test]
    fn readable_sessions_lead_with_the_current_one_and_never_duplicate() {
        let snap =
            OwnershipSnapshot::new(Some(sid("s1")), Some(oid("o1")), vec![sid("s1"), sid("s0")]);
        assert_eq!(snap.readable_sessions(), vec![sid("s1"), sid("s0")]);
    }

    #[test]
    fn readable_sessions_of_a_headless_snapshot_is_empty() {
        assert!(OwnershipSnapshot::empty().readable_sessions().is_empty());
    }

    // --- claim_predecessor ---------------------------------------------------------------

    #[test]
    fn a_claim_needs_all_three_values_to_agree() {
        let own = ResultDeliveryOwnership::new(Some(sid("s1")), Some(oid("o1")));
        assert!(!own.claim_predecessor(None, Some("s1")), "no file");
        assert!(!own.claim_predecessor(Some("s1"), None), "no runtime id");
        assert!(
            !own.claim_predecessor(Some("sX"), Some("s1")),
            "file != runtime"
        );
        assert!(
            !own.claim_predecessor(Some("sX"), Some("sX")),
            "runtime != current"
        );
        assert!(
            own.claim_predecessor(Some("s1"), Some("s1")),
            "all three agree"
        );
    }

    #[test]
    fn a_claim_without_an_owner_is_refused() {
        let own = ResultDeliveryOwnership::new(Some(sid("s1")), None);
        assert!(!own.claim_predecessor(Some("s1"), Some("s1")));
    }

    #[test]
    fn the_claim_list_is_bounded_at_eight() {
        let own = ResultDeliveryOwnership::new(Some(sid("s")), Some(oid("o1")));
        // Only `current` can be claimed, so re-point current each time via a fresh instance.
        for i in 0..12 {
            let own = ResultDeliveryOwnership::new(Some(sid(&format!("s{i}"))), Some(oid("o1")));
            let _ = own.claim_predecessor(Some(&format!("s{i}")), Some(&format!("s{i}")));
            assert!(own.snapshot().claimed.len() <= MAX_CLAIMED_PREDECESSOR_SESSIONS);
        }
        let _ = own;
    }

    #[test]
    fn re_claiming_moves_a_session_to_the_back_rather_than_leaving_it_stale() {
        // The JS `Map` semantics pi relies on: `delete` then `set` refreshes recency. A plain map
        // with arbitrary eviction passes a naive test and fails this one.
        let own = ResultDeliveryOwnership::new(Some(sid("s1")), Some(oid("o1")));
        assert!(own.claim_predecessor(Some("s1"), Some("s1")));
        assert!(
            own.claim_predecessor(Some("s1"), Some("s1")),
            "re-claim is admitted"
        );
        let snap = own.snapshot();
        assert_eq!(snap.claimed, vec![sid("s1")], "no duplicate entry");
    }

    #[test]
    fn clear_forgets_every_claim() {
        let own = ResultDeliveryOwnership::new(Some(sid("s1")), Some(oid("o1")));
        assert!(own.claim_predecessor(Some("s1"), Some("s1")));
        assert_eq!(own.snapshot().claimed.len(), 1);
        own.clear();
        assert!(own.snapshot().claimed.is_empty());
    }

    #[test]
    fn a_snapshot_filters_claims_made_under_a_different_owner() {
        // pi `claimedSessionIds` (`:27-31`) filters on `claimedOwner === owner`.
        let own = ResultDeliveryOwnership::new(Some(sid("s1")), Some(oid("o1")));
        assert!(own.claim_predecessor(Some("s1"), Some("s1")));
        {
            let mut state = own.inner.lock().expect("lock");
            state.owner = Some(oid("o2"));
        }
        assert!(
            own.snapshot().claimed.is_empty(),
            "claims made under the previous owner must not survive an owner change"
        );
    }

    #[test]
    fn a_snapshot_is_a_value_and_does_not_track_later_mutations() {
        let own = ResultDeliveryOwnership::new(Some(sid("s1")), Some(oid("o1")));
        let before = own.snapshot();
        assert!(own.claim_predecessor(Some("s1"), Some("s1")));
        assert!(before.claimed.is_empty(), "the earlier snapshot is frozen");
        assert_eq!(own.snapshot().claimed.len(), 1);
    }
}
