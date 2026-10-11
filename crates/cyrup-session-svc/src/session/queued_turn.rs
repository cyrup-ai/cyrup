//! SUBA-229 — a turn-triggering injection the pump queued onto a LIVE run, owned until its fate is
//! known.
//!
//! pi routes `sendCustomMessage(msg, { triggerTurn })` on a streaming session to `agent.steer` /
//! `agent.followUp` (branch 2 of [`super::route::Route`]) and is done: its run loop and its
//! `_handlePostAgentRun` continuation are on the same thread, so a message queued while
//! `isStreaming` is always drained. cyrup's pump is a separate task, and the run can end between
//! the pump's `is_run_active()` read and its push, leaving the message in the agent's queue with
//! the session idle. So the pump does not let go of what it queued: a [`QueuedTurn`] owns the
//! merged message, which agent queue it is in, and the requests merged into it — each with its
//! [`crate::host_services::InjectAck`], and through the ack the SUBA-187
//! [`crate::host_services::PendingTurnInjections`] guard that keeps
//! [`super::AgentSession::wait_for_idle`] waiting while the turn is owed.
//!
//! # Invariants, by construction
//!
//! - **Answered at most once.** [`QueuedTurn::settle`] and [`QueuedTurn::abandon`] take `self`
//!   by value; each answers or hands back every request it holds, and an [`InjectAck`] answer is
//!   itself consuming. There is no flag to forget to set.
//! - **The SUBA-187 count cannot leak or double-release.** The guard lives inside each request's
//!   ack and is dropped exactly when that ack is answered or dropped, which is a move. A reclaimed
//!   request carries its guard back to the pump, so the turn stays counted until the pump's own
//!   run claims the latch.
//! - **Never lost, never twice.** Settling takes the message back out of its agent queue with ONE
//!   atomic predicate-take under that queue's lock ([`cyrup_agent::Agent::take_steering_where`] /
//!   [`cyrup_agent::Agent::take_follow_up_where`]): either the run's poll drained it first
//!   (delivered: the run carries it), or this take removes it (reclaimed: no run can drain it any
//!   more, and the pump delivers it itself).
//! - **Nothing silently dropped.** The type is `#[must_use]`, and dropping one unsettled is a
//!   `debug_assert!` failure; in a release build each ack's own `Drop` still answers
//!   [`InjectOutcome::SessionUnavailable`], the conservative outcome that tells the producer to
//!   keep its copy.
//!
//! [`InjectAck`]: crate::host_services::InjectAck

use cyrup_agent::{Agent, AgentMessage};
use cyrup_ext::host::InjectOutcome;

use crate::host_services::InjectRequest;

/// Which of the agent's two queues a [`QueuedTurn`] sits in: pi's branch 2 is `agent.steer`, or
/// `agent.followUp` for `deliverAs: "followUp"`. Only those two routes queue onto a live run, so
/// only they are representable here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lane {
    Steer,
    FollowUp,
}

/// The unique id of one [`QueuedTurn`]: a millisecond timestamp no other turn of this pump
/// carries, written into the queued message's own `timestamp`.
///
/// The id has to travel INSIDE the message. The agent's queues hold bare [`AgentMessage`]s, and
/// several paths move them without the pump (a continuation's steering drain and its AGENT-020
/// restore, `clear_queue`, `drain_queue`), so a side-table or queue-slot ticket would not survive
/// them; a field of the message does. Uniqueness among the pump's turns is what identity needs:
/// two turns with the same body (a re-merge, a repeated completion) can never be taken for each
/// other, which comparing content alone allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct TurnId(i64);

/// Issues [`TurnId`]s: the wall clock, bumped past the last id issued, so every id is unique and
/// still a real time (pi's `timestamp: Date.now()`). Owned by the pump task, so `&mut self` and no
/// atomics.
#[derive(Debug, Default)]
pub(super) struct TurnIds {
    last: i64,
}

impl TurnIds {
    fn next(&mut self) -> TurnId {
        let id = super::now_ms().max(self.last.saturating_add(1));
        self.last = id;
        TurnId(id)
    }
}

/// The timestamp a message carries, if its variant has one the pump writes.
fn timestamp(message: &AgentMessage) -> Option<i64> {
    match message {
        AgentMessage::Custom { timestamp, .. } | AgentMessage::User { timestamp, .. } => *timestamp,
        _ => None,
    }
}

/// How a [`QueuedTurn`] settled.
#[must_use = "reclaimed requests are still owed a delivery"]
#[derive(Debug)]
pub(super) enum Settled {
    /// The run drained the message; every request was answered `Accepted`.
    Delivered,
    /// The message was still queued and has been taken back: these requests (acks unanswered,
    /// guards held) are the pump's again.
    Reclaimed(Vec<InjectRequest>),
}

/// A merged turn-triggering injection the pump queued onto a live run. See the module docs.
#[must_use = "a queued turn must be settled (`settle`) or abandoned (`abandon`)"]
#[derive(Debug)]
pub(super) struct QueuedTurn {
    id: TurnId,
    /// The exact message queued, carrying `id` as its timestamp.
    message: AgentMessage,
    lane: Lane,
    /// Non-empty while the turn is unsettled; emptied only by a consuming method.
    requests: Vec<InjectRequest>,
}

impl QueuedTurn {
    /// Stamp `message` with a fresh [`TurnId`], push it onto `agent`'s `lane` queue, and keep it
    /// with the `requests` merged into it.
    ///
    /// `message` is a merge of `requests` (`merge_turn_groups`), so it is a custom or a user
    /// message, the two variants that carry a timestamp.
    pub(super) fn queue(
        agent: &Agent,
        lane: Lane,
        mut message: AgentMessage,
        requests: Vec<InjectRequest>,
        ids: &mut TurnIds,
    ) -> Self {
        debug_assert!(!requests.is_empty(), "a queued turn carries its requests");
        let id = ids.next();
        if let AgentMessage::Custom { timestamp, .. } | AgentMessage::User { timestamp, .. } =
            &mut message
        {
            *timestamp = Some(id.0);
        }
        match lane {
            Lane::Steer => agent.steer(message.clone()),
            Lane::FollowUp => agent.follow_up(message.clone()),
        }
        Self {
            id,
            message,
            lane,
            requests,
        }
    }

    /// Whether `message` would merge into this turn's group (SUBA-017's [`merge_key`] rule): the
    /// pump then takes the turn back and re-merges rather than queueing a second message.
    ///
    /// [`merge_key`]: super::inject::merge_key
    pub(super) fn merges_with(&self, message: &crate::host_services::InjectMessage) -> bool {
        let key = super::inject::merge_key(message);
        key.is_some()
            && key
                == self
                    .requests
                    .first()
                    .and_then(|first| super::inject::merge_key(&first.message))
    }

    /// How many requests this turn carries.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.requests.len()
    }

    /// Settle this turn against `agent`'s queue: take the message back if it is still there
    /// ([`Settled::Reclaimed`], the requests returned), otherwise the run drained it and every
    /// request is answered `Accepted` ([`Settled::Delivered`]).
    ///
    /// "Not in the queue" also covers a user's `clear_queue` (pi `clearQueue` clears the agent
    /// queues, steered custom messages included), where pi too has accepted the message and lost
    /// it; cyrup answers as pi's `sendMessage` returned: accepted.
    pub(super) fn settle(self, agent: &Agent) -> Settled {
        if self.take_back(agent) {
            Settled::Reclaimed(self.reclaim())
        } else {
            self.delivered();
            Settled::Delivered
        }
    }

    /// Answer every request with `outcome` without touching any queue: the session is gone.
    pub(super) fn abandon(mut self, outcome: InjectOutcome) {
        for req in std::mem::take(&mut self.requests) {
            req.ack.answer(outcome.clone());
        }
    }

    /// The run drained it: answer every request `Accepted`, which drops each SUBA-187 guard.
    fn delivered(mut self) {
        for req in std::mem::take(&mut self.requests) {
            req.ack.answer(InjectOutcome::Accepted);
        }
    }

    /// Hand the requests back, acks unanswered and guards held.
    fn reclaim(mut self) -> Vec<InjectRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Take THIS turn's message out of its lane, atomically; `true` when it was still there.
    /// At most one message is taken: the first carrying this turn's id and equal to it (the
    /// equality only rejects a foreign message that happens to share the millisecond).
    fn take_back(&self, agent: &Agent) -> bool {
        let mut wanted = true;
        let pred = |candidate: &AgentMessage| {
            let hit =
                wanted && timestamp(candidate) == Some(self.id.0) && *candidate == self.message;
            if hit {
                wanted = false;
            }
            hit
        };
        let taken = match self.lane {
            Lane::Steer => agent.take_steering_where(pred),
            Lane::FollowUp => agent.take_follow_up_where(pred),
        };
        !taken.is_empty()
    }
}

impl Drop for QueuedTurn {
    fn drop(&mut self) {
        // Every consuming method empties `requests` first, so a non-empty set here is a turn
        // nobody settled. Release builds still answer each ack `SessionUnavailable` (its `Drop`).
        debug_assert!(
            self.requests.is_empty() || std::thread::panicking(),
            "a QueuedTurn was dropped without being settled: {:?}",
            self.id
        );
    }
}

/// Settle every turn in `queued` (emptying it): returns the requests of those still queued, in
/// queue order, for the pump to own again.
pub(super) fn settle_all(queued: &mut Vec<QueuedTurn>, agent: &Agent) -> Vec<InjectRequest> {
    let mut reclaimed = Vec::new();
    for turn in queued.drain(..) {
        match turn.settle(agent) {
            Settled::Delivered => {}
            Settled::Reclaimed(requests) => reclaimed.extend(requests),
        }
    }
    reclaimed
}

/// Test-only observation and control of the pump's live-run queueing, so tests wait on
/// conditions instead of sleeping.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct LiveTurnProbe {
    /// How many requests the pump currently holds in turns queued onto a live run; published after
    /// every queueing pass and every settle.
    pub(crate) held: tokio::sync::watch::Sender<usize>,
    /// Run once, right before the next push onto the agent's queue (after the routing read): the
    /// window in which a run can end between "it is streaming" and "the message is queued".
    #[allow(clippy::type_complexity)]
    pub(crate) before_queue: std::sync::Mutex<Option<Box<dyn FnOnce(&super::AgentSession) + Send>>>,
}

#[cfg(test)]
impl LiveTurnProbe {
    pub(super) fn publish(&self, queued: &[QueuedTurn]) {
        self.held
            .send_replace(queued.iter().map(QueuedTurn::len).sum());
    }

    pub(super) fn fire_before_queue(&self, session: &super::AgentSession) {
        let hook = self
            .before_queue
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(hook) = hook {
            hook(session);
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    //! SUBA-229 — the ownership contract of [`QueuedTurn`], against a real [`Agent`] queue. Each
    //! test names the mutation it kills.

    use std::sync::Arc;

    use cyrup_agent::{Agent, AgentBuilder, AgentMessage, StreamFn};
    use cyrup_ext::host::InjectOutcome;
    use tokio::sync::oneshot::error::TryRecvError;

    use super::{Lane, QueuedTurn, Settled, TurnIds};
    use crate::host_services::{InjectAck, InjectMessage, InjectRequest, PendingTurnInjections};

    /// A stream function no test calls: these tests never run the agent, only its queues.
    struct NeverStreams;

    impl StreamFn for NeverStreams {
        fn stream(
            &self,
            _model: &cyrup_core::ModelRef,
            _ctx: &cyrup_provider::Context,
            _opts: &cyrup_provider::StreamOptions,
        ) -> cyrup_core::EventStream<cyrup_provider::StreamEvent> {
            panic!("the queue tests never stream")
        }
    }

    fn agent() -> Agent {
        AgentBuilder::new(Arc::new(NeverStreams)).build()
    }

    fn message(body: &str) -> AgentMessage {
        AgentMessage::Custom {
            kind: "subagent-notify".to_string(),
            payload: serde_json::Value::String(body.to_string()),
            details: None,
            display: true,
            timestamp: Some(1),
        }
    }

    /// A counted turn-triggering request (as `LiveHostServices::enqueue_injection` builds one) and
    /// the receiver of its ack.
    fn request(
        body: &str,
        pending: &PendingTurnInjections,
    ) -> (InjectRequest, tokio::sync::oneshot::Receiver<InjectOutcome>) {
        let (ack, rx) = InjectAck::channel();
        let req = InjectRequest {
            message: InjectMessage {
                content: body.to_string(),
                custom_type: Some("subagent-notify".to_string()),
                display: true,
                details: None,
                trigger_turn: true,
                steer: false,
            },
            ack: ack.counting_turn(pending),
        };
        (req, rx)
    }

    fn count(pending: &PendingTurnInjections) -> usize {
        *pending.subscribe().borrow()
    }

    /// Drained by the run → `Delivered`: the ack is answered `Accepted` exactly once and the
    /// SUBA-187 count drops to 0. Kills: `settle` treating "not found" as reclaimable (it would
    /// hand back a request the run already delivered — a second delivery).
    #[test]
    fn a_turn_the_run_drained_is_delivered_and_releases_its_count() {
        let agent = agent();
        let pending = PendingTurnInjections::default();
        let (req, mut rx) = request("c1", &pending);
        let mut ids = TurnIds::default();
        let turn = QueuedTurn::queue(&agent, Lane::Steer, message("c1"), vec![req], &mut ids);
        assert_eq!(count(&pending), 1);
        // The run's `poll_steering`.
        assert_eq!(agent.take_steering_where(|_| true).len(), 1);
        assert!(matches!(turn.settle(&agent), Settled::Delivered));
        assert_eq!(rx.try_recv(), Ok(InjectOutcome::Accepted));
        assert_eq!(count(&pending), 0);
    }

    /// Still queued → `Reclaimed`: the message leaves the agent's queue (no run can drain it any
    /// more), the request comes back UNANSWERED with its guard, so the turn stays counted until
    /// the pump answers it. Kills: `take_back` never taking (the message strands, answered
    /// `Accepted`), and a reclaim that answers the ack (the pump's own run would then be uncounted).
    #[test]
    fn a_turn_still_queued_is_reclaimed_with_its_obligation_and_count() {
        let agent = agent();
        let pending = PendingTurnInjections::default();
        let (req, mut rx) = request("c1", &pending);
        let mut ids = TurnIds::default();
        let turn = QueuedTurn::queue(&agent, Lane::FollowUp, message("c1"), vec![req], &mut ids);
        assert!(agent.has_queued_messages());
        let Settled::Reclaimed(back) = turn.settle(&agent) else {
            panic!("the message was still queued");
        };
        assert!(
            !agent.has_queued_messages(),
            "taken out of the follow-up queue"
        );
        assert_eq!(back.len(), 1);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty), "not answered yet");
        assert_eq!(count(&pending), 1, "still owed");
        for req in back {
            req.ack.answer(InjectOutcome::Accepted);
        }
        assert_eq!(rx.try_recv(), Ok(InjectOutcome::Accepted));
        assert_eq!(count(&pending), 0);
    }

    /// Two turns with the SAME body: the run drained the first; settling the first must not take
    /// the second (it is identified by its own id, not by content). Kills: `TurnIds::next`
    /// returning a constant (the two messages are then equal, and settling the first takes the
    /// second's message — the first's requests re-run and the second is answered `Accepted`
    /// without ever being delivered).
    #[test]
    fn a_turn_is_identified_by_its_id_not_its_content() {
        let agent = agent();
        let pending = PendingTurnInjections::default();
        let (first_req, mut first_rx) = request("same", &pending);
        let (second_req, mut second_rx) = request("same", &pending);
        let mut ids = TurnIds::default();
        let first = QueuedTurn::queue(
            &agent,
            Lane::Steer,
            message("same"),
            vec![first_req],
            &mut ids,
        );
        let second = QueuedTurn::queue(
            &agent,
            Lane::Steer,
            message("same"),
            vec![second_req],
            &mut ids,
        );
        // One-at-a-time poll: the run drains the oldest.
        let drained = agent.take_steering_where({
            let mut once = true;
            move |_| std::mem::replace(&mut once, false)
        });
        assert_eq!(drained.len(), 1);
        assert!(matches!(first.settle(&agent), Settled::Delivered));
        assert_eq!(first_rx.try_recv(), Ok(InjectOutcome::Accepted));
        let Settled::Reclaimed(back) = second.settle(&agent) else {
            panic!("the second turn's message is the one still queued");
        };
        assert_eq!(second_rx.try_recv(), Err(TryRecvError::Empty));
        drop(back);
        assert_eq!(
            second_rx.try_recv(),
            Ok(InjectOutcome::SessionUnavailable),
            "a dropped request still answers, conservatively"
        );
        assert_eq!(count(&pending), 0);
    }

    /// An unsettled turn is a bug the debug build reports. Kills: removing the `Drop` assertion.
    #[test]
    #[should_panic(expected = "dropped without being settled")]
    fn dropping_an_unsettled_turn_is_caught() {
        let agent = agent();
        let pending = PendingTurnInjections::default();
        let (req, _rx) = request("c1", &pending);
        let mut ids = TurnIds::default();
        let _turn = QueuedTurn::queue(&agent, Lane::Steer, message("c1"), vec![req], &mut ids);
    }
}
