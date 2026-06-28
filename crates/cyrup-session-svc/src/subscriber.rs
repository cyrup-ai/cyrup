//! The internal event fan-out + the durable-persistence subscriber (arch-11 §3.2/§4.3).
//!
//! One [`SvcSubscriber`] is registered with the agent (ordered/awaited, func-02 R-02-012). On every
//! agent event it (1) appends finalized messages to the session tree so persistence is durable
//! across the turn (arch-04), and (2) fans the event out — as an [`AgentSessionEvent`] — to every
//! live subscription. Run-scoped subscriptions (returned by `prompt`) are terminated after the
//! run's `agent_end`; persistent subscriptions (returned by `subscribe`) live until dropped.

use std::sync::{Arc, Mutex};

use cyrup_agent::{AgentEvent, EventSubscriber};
use cyrup_core::EventStream;
use cyrup_session::manager::SessionManager;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_stream::wrappers::ReceiverStream;

use crate::event::{agent_message_to_core, AgentSessionEvent};

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
    fn end_run(&self) {
        lock(&self.run_scoped).clear();
    }
}

/// The single agent subscriber the facade registers: persists + fans out, in order (arch-11 §3.2).
pub(crate) struct SvcSubscriber {
    fanout: Arc<Fanout>,
    manager: Arc<AsyncMutex<SessionManager>>,
}

impl SvcSubscriber {
    pub(crate) fn new(fanout: Arc<Fanout>, manager: Arc<AsyncMutex<SessionManager>>) -> Self {
        Self { fanout, manager }
    }
}

#[async_trait::async_trait]
impl EventSubscriber for SvcSubscriber {
    async fn on_event(&self, event: &AgentEvent) {
        // 1. Durable persistence: a finalized message lands in the session tree on `message_end`
        //    (arch-04 §6). User → assistant(toolCall) → toolResult → assistant, in event order.
        if let AgentEvent::MessageEnd { message } = event
            && let Some(core) = agent_message_to_core(message) {
                // Append the finalized message to the session tree (durable across the turn).
                let _ = self.manager.lock().await.append_message(core);
            }

        // 2. Fan the event out to live subscriptions (awaited, in order).
        let svc_ev = AgentSessionEvent::from_agent(event);
        let is_end = matches!(event, AgentEvent::AgentEnd { .. });
        self.fanout.emit(svc_ev).await;

        // 3. Terminate run-scoped subscriptions once the run settles.
        if is_end {
            self.fanout.end_run();
        }
    }
}
