//! Steering / follow-up queues + their drain modes (func-02 §9).

use crate::event::AgentMessage;
use std::collections::VecDeque;

/// Drain granularity for a queue (func-02 §9). Default `OneAtATime`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QueueMode {
    #[default]
    OneAtATime,
    All,
}

/// The two strings pi accepts (`settings-manager.ts:101-102`, `:745-757`; the RPC arm in
/// `cyrup-modes/src/rpc/types.rs:33-36` emits the same). Strict: anything else is `Err`. The
/// settings boundary that wants pi's lenient fallback wraps this and says so — see
/// `cyrup-session-svc`'s `parse_queue_mode`.
impl std::str::FromStr for QueueMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "all" => Ok(Self::All),
            "one-at-a-time" => Ok(Self::OneAtATime),
            other => Err(format!(
                "unrecognised queue mode {other:?}; expected \"all\" or \"one-at-a-time\""
            )),
        }
    }
}

/// Global tool-execution preference (func-02 R-02-014). Default `Parallel`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ToolExecution {
    #[default]
    Parallel,
    Sequential,
}

/// A FIFO queue of pending steering/follow-up messages with a per-queue drain mode.
#[derive(Default)]
pub struct PendingQueue {
    items: VecDeque<AgentMessage>,
    mode: QueueMode,
}

impl PendingQueue {
    pub fn new(mode: QueueMode) -> Self {
        Self { items: VecDeque::new(), mode }
    }

    pub fn push(&mut self, m: AgentMessage) {
        self.items.push_back(m);
    }

    pub fn set_mode(&mut self, mode: QueueMode) {
        self.mode = mode;
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Drain per the current mode: the single oldest message, or all of them (func-02 R-02-034/035).
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        match self.mode {
            QueueMode::OneAtATime => self.items.pop_front().into_iter().collect(),
            QueueMode::All => self.items.drain(..).collect(),
        }
    }

    /// Take everything (for abort restore, func-02 R-02-037), ignoring mode.
    pub fn take_all(&mut self) -> Vec<AgentMessage> {
        self.items.drain(..).collect()
    }

    /// Put a previously-[`Self::drain`]ed batch BACK at the head of the queue, preserving the
    /// batch's own order (AGENT-020).
    ///
    /// pi never needs this: `continue()` throws its run-active guard at `agent.ts:351-353` @v0.83.0
    /// **before** `steeringQueue.drain()` at `:361` / `followUpQueue.drain()` at `:367`, and
    /// single-threaded JS makes "check then claim" indivisible. Rust cannot: a run can be claimed
    /// between [`Agent::is_running`](crate::Agent::is_running) and the latch CAS inside `claim_and_snapshot`,
    /// so the hoisted guard is only a fast path. The restore is what actually makes the drain
    /// lossless — a rejected continuation leaves both queues exactly as pi leaves them, and the
    /// message is still delivered at the next drain point (`agent-loop.ts:259`/`:263`).
    pub fn push_front(&mut self, batch: Vec<AgentMessage>) {
        for m in batch.into_iter().rev() {
            self.items.push_front(m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> AgentMessage {
        AgentMessage::user_text(text)
    }

    fn texts(q: &PendingQueue) -> Vec<String> {
        q.items
            .iter()
            .map(|m| match m {
                AgentMessage::User { content, .. } => content
                    .iter()
                    .filter_map(|c| match c {
                        cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<String>(),
                _ => String::new(),
            })
            .collect()
    }

    /// AGENT-020 — the restore half. `drain()` + `push_front()` must be a round trip that leaves the
    /// queue byte-identical, so a `continue_run` whose `claim_and_snapshot` lost the latch race returns the
    /// batch exactly where pi's guard-first ordering (`agent.ts:351-353` @v0.83.0) would have left
    /// it — head of the queue, original order, ahead of anything queued since.
    #[test]
    fn push_front_restores_a_drained_batch_at_the_head_in_order() {
        let mut q = PendingQueue::new(QueueMode::All);
        q.push(user("a"));
        q.push(user("b"));

        let drained = q.drain();
        assert_eq!(drained.len(), 2);
        assert!(q.is_empty(), "drain removed them");

        // Something else queued while the rejected continuation was in flight.
        q.push(user("later"));
        q.push_front(drained);
        assert_eq!(texts(&q), vec!["a", "b", "later"], "restored at the HEAD, order preserved");

        assert_eq!(texts(&q).len(), 3);
        let again = q.drain();
        assert_eq!(again.len(), 3, "and they drain again normally");
    }

    /// The `OneAtATime` default drains a single message; restoring it must not reorder the tail.
    #[test]
    fn push_front_round_trips_a_one_at_a_time_drain() {
        let mut q = PendingQueue::new(QueueMode::OneAtATime);
        q.push(user("first"));
        q.push(user("second"));

        let drained = q.drain();
        assert_eq!(drained.len(), 1);
        assert_eq!(texts(&q), vec!["second"]);

        q.push_front(drained);
        assert_eq!(texts(&q), vec!["first", "second"], "the queue is exactly as it was");
    }
}
