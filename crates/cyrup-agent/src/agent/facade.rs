//! The agent's synchronous surface: subscription, configuration reads/writes, queue control,
//! and the abort/idle signals — everything that does not start or settle a run.

use super::util::lock;
use super::{Agent, AgentBuilder, HeaderFn};
use crate::event::AgentMessage;
use crate::queue::QueueMode;
use crate::state::AgentStateSnapshot;
use crate::stream_fn::StreamFn;
use crate::subscriber::EventSubscriber;
use crate::error::{AgentError, BusyEntry};
use cyrup_core::{AssistantMessage, CancelToken, ModelRef, ModelThinkingLevel, Tool};
use std::sync::{Arc, Mutex};

/// The detach handle [`Agent::subscribe`] returns — cyrup's analogue of the `() => void` closure pi
/// hands back (`packages/agent/src/agent.ts:243-246` @v0.83.0). AGENT-S02.
///
/// Dropping it does NOT unsubscribe (pi's closure has to be invoked); call [`Self::unsubscribe`].
/// Holds only a `Weak` reference to the agent's subscriber list, so a live handle never keeps a
/// disposed agent alive.
///
/// Deliberately NOT `#[must_use]`: pi's callers discard the returned closure whenever the listener
/// is permanent (`agent-session.ts`'s own subscription is never detached), and cyrup's two in-tree
/// subscribers are the same shape, so `agent.subscribe(s);` as a statement is correct usage.
pub struct Subscription {
    subscribers: std::sync::Weak<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    subscriber: Arc<dyn EventSubscriber>,
}

impl Subscription {
    /// Detach the subscriber — pi `() => this.listeners.delete(listener)`. Idempotent, and a no-op
    /// once the agent is gone. Removes the FIRST registration of this exact `Arc` (pi's `Set` holds
    /// each listener once).
    pub fn unsubscribe(&self) {
        if let Some(subs) = self.subscribers.upgrade() {
            let mut subs = lock(&subs);
            if let Some(idx) = subs.iter().position(|s| Arc::ptr_eq(s, &self.subscriber)) {
                subs.remove(idx);
            }
        }
    }
}

impl Agent {
    /// An agent WITH a model. For a modelless agent use [`AgentBuilder::new`] and skip
    /// [`AgentBuilder::model`].
    #[must_use]
    pub fn builder(model: ModelRef, stream_fn: Arc<dyn StreamFn>) -> AgentBuilder {
        AgentBuilder::new(stream_fn).model(model)
    }

    /// Register a notify-only subscriber (func-02 R-02-012) and return the handle that detaches it
    /// again — pi `subscribe(listener): () => void { this.listeners.add(listener); return () =>
    /// this.listeners.delete(listener); }` (`packages/agent/src/agent.ts:243-246` @v0.83.0,
    /// `:250-253` @v0.84.1). AGENT-S02.
    ///
    /// The handle is deliberately NOT auto-detaching on drop: pi's returned closure has to be
    /// *called*, and the two in-tree subscribers register permanently and ignore the return value.
    /// The upstream consumers of the detach handle are session disposal (`agent-session.ts:395`,
    /// `:829-831`) and the rpc-mode stdout-backpressure listener (`modes/rpc/rpc-mode.ts:355-361`,
    /// `:732-733`), which is unsubscribed on every rebind and at shutdown.
    pub fn subscribe(&self, s: Arc<dyn EventSubscriber>) -> Subscription {
        lock(&self.subscribers).push(s.clone());
        Subscription { subscribers: Arc::downgrade(&self.subscribers), subscriber: s }
    }

    pub async fn snapshot(&self) -> AgentStateSnapshot {
        // Read the latch, then take the lock — never hold the lock while touching the channel.
        let running = self.is_running();
        lock(&self.state).snapshot(running)
    }

    // --- scalar/array state setters (R-02-038/044) ---
    pub async fn set_system_prompt(&self, s: String) {
        lock(&self.state).system_prompt = s;
    }

    /// `None` makes the agent modelless: the next `prompt`/`continue_run` returns
    /// [`AgentError::NoModelSelected`]. A run already in flight keeps its own baseline (pi
    /// `agent.state.model = next` is likewise a between-turns write, agent-session.ts:1643).
    pub async fn set_model(&self, m: Option<ModelRef>) {
        lock(&self.state).model = m;
    }

    /// Replace the per-request header overlay (pi recomputes it per request inside `streamFn`,
    /// `sdk.ts:318-327`). The session facade calls this on every model change so provider-attribution
    /// and opencode session-affinity headers follow the ACTIVE provider.
    pub async fn set_headers(&self, h: Option<cyrup_provider::HeaderMap>) {
        lock(&self.state).headers = h;
    }

    /// Install (or clear) the per-turn header resolver — pi's `transformHeaders` closure
    /// (`sdk.ts:318-327` @v0.83.0). See [`HeaderFn`]. When installed it is consulted with the model
    /// of the turn being dispatched, so a mid-run `TurnUpdate::model` override can no longer carry
    /// the previous provider's attribution headers (AGENT-029). The static [`Self::set_headers`]
    /// overlay remains the fallback for a model the resolver has no opinion about.
    pub fn set_header_fn(&self, f: Option<Arc<HeaderFn>>) {
        *lock(&self.header_fn) = f;
    }

    /// Replace the preferred transport on the RUNNING agent — pi's `this.session.agent.transport =
    /// transport` (`interactive-mode.ts:4215`), the second half of the `/settings` "Transport"
    /// handler (the first half persists the setting). Applies from the next run onward, matching
    /// pi's read of `this.transport` in `createLoopConfig` (agent.ts:442).
    pub async fn set_transport(&self, t: Option<cyrup_provider::Transport>) {
        lock(&self.state).transport = t;
    }

    pub async fn set_thinking_level(&self, t: ModelThinkingLevel) {
        lock(&self.state).thinking_level = t;
    }

    /// Copies the top-level Vec (the caller's array is decoupled, R-02-038).
    pub async fn set_tools(&self, tools: Vec<Arc<dyn Tool>>) {
        lock(&self.state).tools = tools;
    }

    /// The agent's CURRENT tool set (Pi `agent.state.tools`, read by `_installAgentNextTurnRefresh`
    /// as `this.agent.state.tools.slice()`, agent-session.ts:533). `AgentStateSnapshot` reports only
    /// `tool_count` because a tool is not serializable; a caller that must re-push the live array
    /// onto a running loop — via [`crate::TurnUpdate::tools`] — needs the handles themselves.
    pub async fn tools(&self) -> Vec<Arc<dyn Tool>> {
        lock(&self.state).tools.clone()
    }

    /// Copies the top-level Vec (the caller's array is decoupled, R-02-038).
    pub async fn set_messages(&self, msgs: Vec<AgentMessage>) {
        lock(&self.state).messages = msgs;
    }

    /// Atomic transcript edit under the state lock — the replacement for every
    /// `snapshot → mutate → set_messages` triplet, which spanned two awaits with no lock and could
    /// interleave with the reducer. Refused while a run is in flight (the same latch `reset`
    /// observes), so it can never race the run's own appends.
    ///
    /// The AGENT-030 post-run gap — after `agent_end` releases this latch but before the session's
    /// driver decides whether to continue — is the SESSION's to gate: `is_run_active()` reads
    /// `driver_tx`, which the agent cannot see. This method is the second line, not the first.
    pub fn edit_transcript<R>(
        &self,
        f: impl FnOnce(&mut Vec<AgentMessage>) -> R,
    ) -> Result<R, AgentError> {
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Edit));
        }
        let mut st = lock(&self.state);
        Ok(f(&mut st.messages))
    }

    /// Pop the trailing assistant message iff `pred` holds for it, returning it. The one operation
    /// both session retry predicates need — "any trailing assistant" and "a trailing
    /// `Error`/`Length` assistant" — expressed as a predicate rather than as two copies of the pop.
    pub fn pop_trailing_assistant_if(
        &self,
        pred: impl FnOnce(&AssistantMessage) -> bool,
    ) -> Result<Option<Arc<AssistantMessage>>, AgentError> {
        self.edit_transcript(|m| match m.last() {
            Some(AgentMessage::Assistant(a)) if pred(a) => {
                let a = Arc::clone(a);
                m.pop();
                Some(a)
            }
            _ => None,
        })
    }

    // --- queues (R-02-034..037) ---
    pub fn steer(&self, m: AgentMessage) {
        lock(&self.steering).push(m);
    }

    pub fn follow_up(&self, m: AgentMessage) {
        lock(&self.follow_up).push(m);
    }

    pub fn set_steering_mode(&self, mode: QueueMode) {
        lock(&self.steering).set_mode(mode);
    }

    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        lock(&self.follow_up).set_mode(mode);
    }

    pub fn clear_steering_queue(&self) {
        lock(&self.steering).clear();
    }

    pub fn clear_follow_up_queue(&self) {
        lock(&self.follow_up).clear();
    }

    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }

    #[must_use]
    pub fn drain_queues_for_restore(&self) -> (Vec<AgentMessage>, Vec<AgentMessage>) {
        (lock(&self.steering).take_all(), lock(&self.follow_up).take_all())
    }

    // --- lifecycle (R-02-045..047) ---
    /// Signal the active run's abort token (idempotent, R-02-045).
    pub fn abort(&self) {
        if let Some(c) = lock(&self.cancel_slot).as_ref() {
            c.cancel();
        }
    }

    /// Resolve only after the current run emits `agent_end` and all awaited `agent_end` subscribers
    /// settle (R-02-047). Safe to call repeatedly; concurrent callers resolve together.
    pub async fn wait_for_idle(&self) {
        let mut rx = self.running_rx.clone();
        loop {
            if !*rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Whether a run is in flight, read WITHOUT awaiting (Pi `_isAgentRunActive`, the flag behind
    /// `AgentSession.isIdle`, agent-session.ts:881-883). The sync counterpart of
    /// [`Self::wait_for_idle`]: an extension's `ctx.isIdle()` host import is a synchronous read and
    /// cannot await the latch.
    pub fn is_running(&self) -> bool {
        *self.running_rx.borrow()
    }

    /// Active run's abort signal, if one is active (Pi `agent.signal`, agent.ts:294-297). Callers can
    /// observe cancellation without holding the agent's internal slot.
    pub fn signal(&self) -> Option<CancelToken> {
        lock(&self.cancel_slot).as_ref().map(|c| c.token())
    }

    /// `true` when either queue still holds pending messages (Pi `hasQueuedMessages`,
    /// agent.ts:289-292).
    pub fn has_queued_messages(&self) -> bool {
        !lock(&self.steering).is_empty() || !lock(&self.follow_up).is_empty()
    }
}
