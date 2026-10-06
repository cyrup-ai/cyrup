//! The agent's synchronous surface: subscription, configuration reads/writes, queue control,
//! and the abort/idle signals — everything that does not start or settle a run.

use super::util::lock;
use super::{Agent, AgentBuilder, HeaderFn};
use crate::error::{AgentError, BusyEntry};
use crate::event::AgentMessage;
use crate::queue::{QueueMode, ToolExecution};
use crate::state::AgentStateSnapshot;
use crate::stream_fn::StreamFn;
use crate::subscriber::EventSubscriber;
use cyrup_core::{AssistantMessage, CancelToken, ModelRef, ModelThinkingLevel, Tool, ToolLoadout};
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
        Subscription {
            subscribers: Arc::downgrade(&self.subscribers),
            subscriber: s,
        }
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
        lock(&self.state).tools = ToolLoadout::from_tools(tools);
    }

    /// Replace the tool loadout (Pi `agent.state.tools = declared`, `_applyToolLoadout`,
    /// `agent-session.ts:1569-1571` @v1.0.1). The loadout was resolved against the session's
    /// registry, so hidden tools are already out and the `prepare_loadout` hooks have already run.
    pub async fn set_loadout(&self, loadout: ToolLoadout) {
        lock(&self.state).tools = loadout;
    }

    /// The agent's CURRENT tool set (Pi `agent.state.tools`, read by `_installAgentNextTurnRefresh`
    /// as `this.agent.state.tools.slice()`, agent-session.ts:533). `AgentStateSnapshot` reports only
    /// `tool_count` because a tool is not serializable; a caller that must re-push the live array
    /// onto a running loop — via [`crate::TurnUpdate::tools`] — needs the handles themselves.
    pub async fn tools(&self) -> Vec<Arc<dyn Tool>> {
        lock(&self.state).tools.executable().to_vec()
    }

    /// The agent's current [`ToolLoadout`] — what a caller that re-pushes the live set onto a
    /// running loop via [`crate::TurnUpdate::tools`] hands back, keeping the hidden declarations.
    pub async fn loadout(&self) -> ToolLoadout {
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
    ///
    /// # The latch is read UNDER the state lock (ICOM-068)
    ///
    /// A run is claimed in two steps — the latch CAS, then the transcript snapshot under this same
    /// state lock (`claim_and_snapshot`). Reading the latch before taking the lock left a gap in
    /// which a claim could land between the check and the edit: the edit then went into
    /// `state.messages` AFTER the run had snapshotted, so the run's model request lacked it while
    /// every later run saw it out of order. Checking with the lock held closes that gap: a claim
    /// that lands before the check is refused here, and one that lands after it cannot snapshot
    /// until this edit has released the lock — so the run it starts sees the edit.
    pub fn edit_transcript<R>(
        &self,
        f: impl FnOnce(&mut Vec<AgentMessage>) -> R,
    ) -> Result<R, AgentError> {
        let mut st = lock(&self.state);
        if self.is_running() {
            return Err(AgentError::RunActive(BusyEntry::Edit));
        }
        Ok(f(&mut st.messages))
    }

    /// Append ONE finalized message to the live transcript, under the state lock, WITHOUT consulting
    /// the run latch — the single write pi's `_refreshFinalizedContext()` performs
    /// (`agent-session.ts:730-736` @v0.87.1, `this.agent.state.messages = projection.messages`) when
    /// the only tree change since the transcript was last in step is the one entry just appended.
    ///
    /// Pi calls that re-seed from inside a live run: `_appendCustomMessage` ends with it
    /// (`agent-session.ts:1979`) and the `turn_end` arm of `_handleAgentEvent` flushes through it
    /// (`:965-973`), while `_isAgentRunActive` is still `true`. So this write has to be legal
    /// mid-run, which is why it is a method of its own instead of a call to
    /// [`Self::edit_transcript`].
    ///
    /// # Why this is safe while a run is active
    ///
    /// The latch [`Self::edit_transcript`] consults exists to stop CONCURRENT, ARBITRARY mutation
    /// racing the run's own writes (ICOM-068: an edit landing after the run snapshotted put the
    /// message into `state.messages` out of tree order). Neither hazard applies here:
    ///
    /// * **Not arbitrary.** This appends; it cannot truncate, reorder, replace or read back. The
    ///   worst a caller can do is add a message at the tail — which is what the run's own reducer
    ///   does on every `message_end`.
    /// * **Not concurrent.** `state.messages` is written by exactly two parties: the reducer, and
    ///   this method. Both take the same state lock, and the run's sole emission path reduces
    ///   and then AWAITS each subscriber in order on the run task — so a caller reached from a run
    ///   event (the `turn_end` flush) runs between two reductions on that one task, never beside
    ///   one.
    /// * **Order-preserving.** The run loop drives its turns from the private `.slice()` copy taken
    ///   at `claim_and_snapshot` (pi `createContextSnapshot`, `agent.ts:457-461`), so this
    ///   append cannot perturb the in-flight run at all; it is read by the NEXT snapshot
    ///   (`continue_run`, or the next `prompt`), exactly as pi's assignment is.
    ///
    /// Callers that need the general, checked edit — anything that removes or rewrites history —
    /// must keep using [`Self::edit_transcript`], which still refuses mid-run.
    pub fn append_finalized_message(&self, message: AgentMessage) {
        lock(&self.state).messages.push(message);
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

    /// Remove and return every queued steering message `pred` selects, preserving the order of
    /// both what is taken and what is left.
    ///
    /// ICOM-035 — the session's injection pump steers a no-turn message onto a live run and has to
    /// find out, once the run is over, whether the run's loop drained it or whether it landed after
    /// the loop's last `poll_steering` and is still sitting here. Taking exactly its own messages
    /// back (never the user's queued steers) is what lets the pump re-deliver a stranded one instead
    /// of leaving it for whichever unrelated run polls the queue next.
    pub fn take_steering_where(
        &self,
        pred: impl FnMut(&AgentMessage) -> bool,
    ) -> Vec<AgentMessage> {
        lock(&self.steering).take_where(pred)
    }

    /// Put a batch previously taken with [`Self::take_steering_where`] back at the HEAD of the
    /// steering queue, in its own order — the undo for a take whose follow-up could not happen.
    pub fn restore_steering_front(&self, batch: Vec<AgentMessage>) {
        if !batch.is_empty() {
            lock(&self.steering).push_front(batch);
        }
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
        (
            lock(&self.steering).take_all(),
            lock(&self.follow_up).take_all(),
        )
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

    /// How a batch of tool calls runs (Pi `agent.toolExecution`, read at
    /// `agent-session.ts:704` @v1.0.1 to decide whether a nested call is exclusive).
    pub fn tool_execution(&self) -> ToolExecution {
        self.tool_execution
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

    /// Preview the messages the loop would select for the next turn, without consuming them (Pi
    /// `peekQueuedMessages`, `agent.ts:326-330` @v0.87.1, added v0.87.0): the steering queue's next
    /// batch if it has one, else the follow-up queue's, each honouring its own [`QueueMode`].
    ///
    /// Host/extension API: pi's one caller is the coding-agent's boundary-context preview
    /// (`agent-session.ts:787-789` `_getPendingBoundaryMessages`, behind the actionable
    /// `turn_end`/`agent_before_settle` extension boundaries), which cyrup has not ported.
    pub fn peek_queued_messages(&self) -> Vec<AgentMessage> {
        let steering = lock(&self.steering).peek();
        if !steering.is_empty() {
            return steering;
        }
        lock(&self.follow_up).peek()
    }
}
