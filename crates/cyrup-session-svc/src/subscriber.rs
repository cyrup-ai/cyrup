//! The internal event fan-out + the durable-persistence subscriber (arch-11 §3.2/§4.3).
//!
//! One [`SvcSubscriber`] is registered with the agent (ordered/awaited, func-02 R-02-012). On every
//! agent event it (1) appends finalized messages to the session tree so persistence is durable
//! across the turn (arch-04), and (2) fans the event out — as an [`AgentSessionEvent`] — to every
//! live subscription. Run-scoped subscriptions (returned by `prompt`) are terminated after the
//! run's `agent_end`; persistent subscriptions (returned by `subscribe`) live until dropped.

use std::sync::{Arc, Mutex};

use cyrup_agent::{AgentEvent, AgentMessage, EventSubscriber};
use cyrup_core::EventStream;
use cyrup_session::manager::SessionManager;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_stream::wrappers::ReceiverStream;

use crate::event::{agent_message_to_core, AgentSessionEvent};
use crate::session::SessionHandle;

const CHANNEL_CAPACITY: usize = 1024;

/// Lock a `std::sync::Mutex` ignoring poisoning (no panic on a poisoned lock; arch-00 no-panic).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Multi-consumer fan-out of [`AgentSessionEvent`] backed by bounded `mpsc` (no broadcast backlog,
/// arch-11 §5.4). `persistent` survives across runs; `run_scoped` is cleared after `agent_end`.
#[derive(Default)]
pub(crate) struct Fanout {
    persistent: Mutex<Vec<mpsc::Sender<AgentSessionEvent>>>,
    run_scoped: Mutex<Vec<mpsc::Sender<AgentSessionEvent>>>,
}

impl Fanout {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// A long-lived subscription (TUI / SDK observer) — lives until the receiver is dropped.
    pub(crate) fn subscribe_persistent(&self) -> EventStream<AgentSessionEvent> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        lock(&self.persistent).push(tx);
        Box::pin(ReceiverStream::new(rx))
    }

    /// A run-scoped subscription — terminated after the in-flight run emits `agent_end`.
    pub(crate) fn subscribe_run(&self) -> EventStream<AgentSessionEvent> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        lock(&self.run_scoped).push(tx);
        Box::pin(ReceiverStream::new(rx))
    }

    /// Emit a facade-originated (session-level) event onto every live subscription (arch-11 §3.2).
    pub(crate) async fn emit_external(&self, ev: AgentSessionEvent) {
        self.emit(ev).await;
    }

    /// Send to every live subscription, awaited (backpressure → slows the agent, never drops).
    async fn emit(&self, ev: AgentSessionEvent) {
        // Snapshot the senders so we never hold the lock across an `.await`.
        let persistent: Vec<_> = lock(&self.persistent).clone();
        for s in &persistent {
            let _ = s.send(ev.clone()).await;
        }
        let run: Vec<_> = lock(&self.run_scoped).clone();
        for s in &run {
            let _ = s.send(ev.clone()).await;
        }
        // Prune closed persistent senders (run-scoped are cleared wholesale in `end_run`).
        lock(&self.persistent).retain(|s| !s.is_closed());
    }

    /// Drop all run-scoped senders, terminating the streams returned by the just-finished `prompt`.
    /// Called by the persist+fan-out subscriber (unbound legacy path) on `agent_end`, and by the
    /// post-run driver (bound path) once the WHOLE post-run loop settles.
    pub(crate) fn end_run(&self) {
        lock(&self.run_scoped).clear();
    }

    /// Invalidate every subscription on session replacement (R-11-021, arch-11 §3.2): emit a
    /// terminal `SessionReplaced` so consumers re-subscribe against the new generation, then drop
    /// every sender (both run-scoped and persistent) so the streams end.
    pub(crate) async fn invalidate(&self, generation: u64) {
        self.emit(AgentSessionEvent::SessionReplaced { generation }).await;
        lock(&self.run_scoped).clear();
        lock(&self.persistent).clear();
    }
}

/// The single agent subscriber the facade registers: persists + fans out, in order (arch-11 §3.2).
/// On a BOUND session it is also the cyrup analogue of Pi's `_handleAgentEvent` (agent-session.ts:512)
/// — queue-mirror draining, overflow/retry-counter resets, last-assistant tracking, and the
/// `agent_end.willRetry` payload — reached via the shared [`SessionHandle`].
pub(crate) struct SvcSubscriber {
    fanout: Arc<Fanout>,
    manager: Arc<AsyncMutex<SessionManager>>,
    handle: Arc<SessionHandle>,
}

impl SvcSubscriber {
    pub(crate) fn new(
        fanout: Arc<Fanout>,
        manager: Arc<AsyncMutex<SessionManager>>,
        handle: Arc<SessionHandle>,
    ) -> Self {
        Self { fanout, manager, handle }
    }
}

#[async_trait::async_trait]
impl EventSubscriber for SvcSubscriber {
    async fn on_event(&self, event: &AgentEvent) {
        let session = self.handle.get();

        // 0. `_handleAgentEvent` head (Pi agent-session.ts:514-535): on a USER `message_start`, reset
        //    the overflow latch and drain the matching queue mirror entry + emit `queue_update`.
        if let (Some(s), AgentEvent::MessageStart { message }) = (&session, event)
            && matches!(message, AgentMessage::User { .. })
        {
            s.on_user_message_start(message).await;
        }

        // 1. Durable persistence: a finalized message lands in the session tree on `message_end`
        //    (arch-04 §6). User → assistant(toolCall) → toolResult → assistant, in event order.
        if let AgentEvent::MessageEnd { message } = event {
            if let Some(core) = agent_message_to_core(message) {
                // Append the finalized message to the session tree (durable across the turn).
                let _ = self.manager.lock().await.append_message(core);
            }
            // `_handleAgentEvent` tail (Pi :562-577): track the last assistant + reset retry/overflow.
            if let (Some(s), AgentMessage::Assistant(a)) = (&session, message) {
                s.on_assistant_message_end(a).await;
            }
        }

        // 2. Fan the event out to live subscriptions (awaited, in order). `agent_end` carries the
        //    live `willRetry` decision when bound (Pi :541); unbound emits `false`.
        let is_end = matches!(event, AgentEvent::AgentEnd { .. });
        let svc_ev = match (event, &session) {
            (AgentEvent::AgentEnd { messages }, Some(s)) => AgentSessionEvent::AgentEnd {
                messages: messages.clone(),
                will_retry: s.will_retry_after_agent_end(messages),
            },
            _ => AgentSessionEvent::from_agent(event),
        };
        self.fanout.emit(svc_ev).await;

        // 3. Terminate run-scoped subscriptions once the run settles — but ONLY on an unbound session.
        //    A bound session's post-run driver owns run termination (it may continue past this
        //    `agent_end` for a retry / compaction / queued continuation).
        if is_end && session.is_none() {
            self.fanout.end_run();
        }
    }
}
