//! The steering / follow-up queues and abort.
//!
//! Pi `agent-session.ts:476-477,1393-1416,1545`. The facade mirrors of the agent's authoritative
//! queues (for `queue_update` emission and introspection), their drain modes, and the two abort
//! entry points — the fire-and-forget `abort` and the bounded `abort_and_settle`.

use crate::event::AgentSessionEvent;

use super::AgentSession;

/// Upper bound on the `await this.waitForIdle()` tail of [`AgentSession::abort_and_settle`]
/// (SEAM-024). Pi's `abort()` awaits unboundedly (agent-session.ts:1545), but its callers are a
/// browser-style event loop; here the same await sits on `dispose`, i.e. on every `quit`, every
/// session replacement and the RPC `abort` verb, so a tool wedged in an uninterruptible syscall
/// would otherwise make the process unkillable-by-Ctrl-C. On expiry the caller continues exactly as
/// the pre-SEAM-024 fire-and-forget `abort()` did — never worse than the old behaviour.
const ABORT_SETTLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl AgentSession {
    /// The pending steering messages, in order (Pi `getSteeringMessages`, agent-session.ts:1408).
    pub fn steering_messages(&self) -> Vec<String> {
        Self::lock(&self.steering_messages).clone()
    }

    /// The pending follow-up messages, in order (Pi `getFollowUpMessages`, agent-session.ts:1412).
    pub fn follow_up_messages(&self) -> Vec<String> {
        Self::lock(&self.follow_up_messages).clone()
    }

    /// Total queued (steering + follow-up) message count (Pi `pendingMessageCount`,
    /// agent-session.ts:1393).
    pub fn pending_message_count(&self) -> usize {
        Self::lock(&self.steering_messages).len() + Self::lock(&self.follow_up_messages).len()
    }

    /// Clear both queues (Pi `clearQueue`, agent-session.ts:1416): drains the agent's authoritative
    /// queues and the facade mirrors, then emits `queue_update`.
    pub async fn clear_queue(&self) {
        self.agent.clear_all_queues();
        Self::lock(&self.steering_messages).clear();
        Self::lock(&self.follow_up_messages).clear();
        self.emit_queue_update().await;
    }

    /// Take-all both queues and RETURN what was drained, in Pi's `(steering, followUp)` shape
    /// (`AgentSession.clearQueue()` returns `{steering, followUp}`, agent-session.ts:1416 — the
    /// value `restoreQueuedMessagesToEditor` reads at interactive-mode.ts:4065).
    ///
    /// [`Self::clear_queue`] throws that value away, which forces a caller that wants the text to
    /// read `steering_messages()`/`follow_up_messages()` first and clear second — a lost-update race
    /// with a concurrent `steer`/`follow_up`. This is the atomic form: the mirrors and the agent's
    /// authoritative queues are taken in one pass (`Agent::drain_queues_for_restore`), then
    /// `queue_update` is emitted so the footer count drops to zero.
    pub async fn drain_queue(&self) -> (Vec<String>, Vec<String>) {
        // Both mirrors are taken under their guards together so the pair is consistent; the agent
        // drain happens after they are released, keeping the facade→agent lock nesting `steer` /
        // `follow_up` avoid (they too drop the mirror guard before calling into the agent).
        let drained = {
            let mut steering = Self::lock(&self.steering_messages);
            let mut follow_up = Self::lock(&self.follow_up_messages);
            (std::mem::take(&mut *steering), std::mem::take(&mut *follow_up))
        };
        self.agent.drain_queues_for_restore();
        self.emit_queue_update().await;
        drained
    }

    /// Emit a `queue_update` snapshot of both facade queues (Pi `_emitQueueUpdate`,
    /// agent-session.ts:1382).
    pub(super) async fn emit_queue_update(&self) {
        let steering = Self::lock(&self.steering_messages).clone();
        let follow_up = Self::lock(&self.follow_up_messages).clone();
        self.fanout_emit(AgentSessionEvent::QueueUpdate { steering, follow_up }).await;
    }

    /// Interrupt the active run (idempotent, R-11-018 / func-02 R-02-045).
    ///
    /// SEAM-023 — the retry backoff is cancelled FIRST, exactly as Pi's `abort()` does
    /// (`abortRetry(); this.agent.abort(); await this.waitForIdle();`, agent-session.ts:1542-1546).
    /// `agent.abort()` cancels the PER-RUN token; the auto-retry backoff sleeps on a *separate*
    /// child of `session_cancel` ([`Self::prepare_retry`]), so without this an Escape / SIGINT /
    /// RPC `abort` landing during provider-retry backoff left the backoff running and the retry
    /// fired later against a session the user had already aborted.
    ///
    /// This is the SYNCHRONOUS half (what a signal handler and `ctx.abort()` need). Callers that
    /// must observe the run actually settle — teardown, compaction, the RPC `abort` verb — use
    /// [`Self::abort_and_settle`], which adds Pi's `await this.waitForIdle()` tail.
    pub fn abort(&self) {
        self.abort_retry();
        self.agent.abort();
    }

    /// Interrupt the active run **and await its settlement** — the full Pi `abort()`
    /// (agent-session.ts:1542-1546: `this.abortRetry(); this.agent.abort(); await
    /// this.waitForIdle();`), in that exact order.
    ///
    /// SEAM-024. The order is load-bearing and the reason this is not simply
    /// `wait_for_idle().await` after a plain abort: the retry backoff sleeps on a child of
    /// `session_cancel` that `agent.abort()` does not touch, so awaiting idle BEFORE cancelling it
    /// would block for the whole remaining backoff (up to `retry.baseDelayMs * 2^attempt`).
    ///
    /// Pi's `teardownCurrent` states why teardown must await: "Settle any active response first so
    /// the aborted turn (including tool results) is persisted to the outgoing session before it is
    /// replaced" (agent-session-runtime.ts:167-169), and its RPC `abort` verb likewise replies only
    /// after `await session.abort()` (rpc-mode.ts:427-430).
    ///
    /// Unlike Pi the wait is BOUNDED ([`ABORT_SETTLE_TIMEOUT`]): a wedged tool must not make `quit`
    /// hang forever. On expiry the caller proceeds exactly as the old fire-and-forget `abort()` did.
    pub async fn abort_and_settle(&self) {
        self.abort();
        let _ = tokio::time::timeout(ABORT_SETTLE_TIMEOUT, self.wait_for_idle()).await;
    }

    /// The agent's current steering mode (Pi `steeringMode` getter, agent-session.ts:845).
    pub fn steering_mode(&self) -> cyrup_agent::QueueMode {
        *Self::lock(&self.steering_mode)
    }

    /// The agent's current follow-up mode (Pi `followUpMode` getter, agent-session.ts:850).
    pub fn follow_up_mode(&self) -> cyrup_agent::QueueMode {
        *Self::lock(&self.follow_up_mode)
    }

    /// Set the steering-message delivery mode (Pi `setSteeringMode`, agent-session.ts:1631).
    pub fn set_steering_mode(&self, mode: cyrup_agent::QueueMode) {
        self.agent.set_steering_mode(mode);
        *Self::lock(&self.steering_mode) = mode;
    }

    /// Set the follow-up-message delivery mode (Pi `setFollowUpMode`, agent-session.ts:1640).
    pub fn set_follow_up_mode(&self, mode: cyrup_agent::QueueMode) {
        self.agent.set_follow_up_mode(mode);
        *Self::lock(&self.follow_up_mode) = mode;
    }
}
