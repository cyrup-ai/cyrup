//! The internal event fan-out + the durable-persistence subscriber (arch-11 §3.2/§4.3).
//!
//! One [`SvcSubscriber`] is registered with the agent (ordered/awaited, func-02 R-02-012). On every
//! agent event it (1) appends finalized messages to the session tree so persistence is durable
//! across the turn (arch-04), and (2) fans the event out — as an [`AgentSessionEvent`] — to every
//! live subscription. Run-scoped subscriptions (returned by `prompt`) are terminated after the run
//! SETTLES — their last event is `agent_settled`, not `agent_end` (SEAM-005), since a run may
//! continue past an `agent_end` for an auto-retry / post-run compaction / queued continuation.
//! Persistent subscriptions (returned by `subscribe`) live until dropped.

use std::sync::{Arc, Mutex};

use cyrup_agent::{AgentEvent, AgentMessage, EventSubscriber};
use cyrup_core::{CancelToken, EventStream};
use cyrup_ext::ExtensionHost;
use cyrup_session::manager::SessionManager;
use tokio::sync::{mpsc, Mutex as AsyncMutex};
use tokio_stream::wrappers::ReceiverStream;

use crate::event::{agent_message_to_core, core_message_to_agent, AgentSessionEvent};
use crate::session::SessionHandle;

const CHANNEL_CAPACITY: usize = 1024;

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
        crate::sync::lock(&self.persistent).push(tx);
        Box::pin(ReceiverStream::new(rx))
    }

    /// A run-scoped subscription — terminated once the in-flight run SETTLES (its final event is
    /// `agent_settled`, which follows the run's last `agent_end`).
    pub(crate) fn subscribe_run(&self) -> EventStream<AgentSessionEvent> {
        let (tx, rx) = mpsc::channel(CHANNEL_CAPACITY);
        crate::sync::lock(&self.run_scoped).push(tx);
        Box::pin(ReceiverStream::new(rx))
    }

    /// Emit a facade-originated (session-level) event onto every live subscription (arch-11 §3.2).
    pub(crate) async fn emit_external(&self, ev: AgentSessionEvent) {
        self.emit(ev).await;
    }

    /// Send to every live subscription, awaited (backpressure → slows the agent, never drops).
    async fn emit(&self, ev: AgentSessionEvent) {
        // Snapshot the senders so we never hold the lock across an `.await`.
        let persistent: Vec<_> = crate::sync::lock(&self.persistent).clone();
        for s in &persistent {
            let _ = s.send(ev.clone()).await;
        }
        let run: Vec<_> = crate::sync::lock(&self.run_scoped).clone();
        for s in &run {
            let _ = s.send(ev.clone()).await;
        }
        // Prune closed persistent senders (run-scoped are cleared wholesale in `end_run`).
        crate::sync::lock(&self.persistent).retain(|s| !s.is_closed());
    }

    /// Drop all run-scoped senders, terminating the streams returned by the just-finished `prompt`.
    /// Called by the persist+fan-out subscriber (unbound legacy path) and by the post-run driver
    /// (bound path), in both cases immediately AFTER `agent_settled` is emitted, so a run-scoped
    /// consumer observes the settle as its last event.
    pub(crate) fn end_run(&self) {
        crate::sync::lock(&self.run_scoped).clear();
    }

    /// Invalidate every subscription on session replacement (R-11-021, arch-11 §3.2): emit a
    /// terminal `SessionReplaced` so consumers re-subscribe against the new generation, then drop
    /// every sender (both run-scoped and persistent) so the streams end.
    pub(crate) async fn invalidate(&self, generation: u64) {
        self.emit(AgentSessionEvent::SessionReplaced { generation }).await;
        crate::sync::lock(&self.run_scoped).clear();
        crate::sync::lock(&self.persistent).clear();
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
    /// The extension host (gap-08 #1): the `message_end` [mutate] re-dispatch seam. Sourced here —
    /// NOT via `handle.get()` — because `message_end` fires on unbound sessions too.
    ext_host: Arc<ExtensionHost>,
    /// The session cancel token; `message_end` re-dispatch runs under a child of it so a session
    /// teardown cancels an in-flight guest call (never hangs).
    session_cancel: CancelToken,
}

impl SvcSubscriber {
    pub(crate) fn new(
        fanout: Arc<Fanout>,
        manager: Arc<AsyncMutex<SessionManager>>,
        handle: Arc<SessionHandle>,
        ext_host: Arc<ExtensionHost>,
        session_cancel: CancelToken,
    ) -> Self {
        Self { fanout, manager, handle, ext_host, session_cancel }
    }
}

#[async_trait::async_trait]
impl EventSubscriber for SvcSubscriber {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
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
        //
        //    gap-08 #1: BEFORE persistence, re-dispatch `message_end` through the tested
        //    [`ExtensionHost::emit_message_end`] facade seam so a guest's enforced same-role
        //    replacement (Pi `emitMessageEnd`, runner.ts:781-821) actually mutates the durable +
        //    fanned-out copy instead of being silently dropped. Gated on a live subscriber so the
        //    common (no-extension) path keeps its behavior with zero extra work (mirrors
        //    session.rs:1041/…). NOTE (documented delta, risks §3): this replaces only the persisted
        //    + fanned-out copy, not the agent's already-emitted in-memory transcript.
        let mut replaced_end: Option<AgentMessage> = None;
        if let AgentEvent::MessageEnd { message } = event {
            let effective: Option<cyrup_core::Message> = if !self
                .ext_host
                .dispatcher()
                .no_subscribers(cyrup_ext::EventKind::MessageEnd)
                && let Some(core) = agent_message_to_core(message)
            {
                let cancel = self.session_cancel.child_token();
                match self.ext_host.emit_message_end(core.clone(), &cancel).await {
                    // Same-role replacement enforced host-side (facade.rs): adopt it for BOTH the
                    // durable append AND the fan-out below.
                    Some(repl) => {
                        replaced_end = Some(core_message_to_agent(&repl));
                        Some(repl)
                    }
                    None => Some(core),
                }
            } else {
                agent_message_to_core(message)
            };

            if let Some(core) = effective {
                // Append the finalized (possibly guest-replaced) message to the session tree.
                let _ = self.manager.lock().await.append_message(core);
            } else if let AgentMessage::Custom { kind, payload, .. } = message {
                // Custom messages (queued via `send_custom_message` deliver_as steer/followUp and
                // pulled mid-run) finalize as a `message_end` here. Pi persists them as a
                // CustomMessageEntry (agent-session.ts:546-553 `appendCustomMessageEntry`); without
                // this they would be silently dropped by the `Custom -> None` core mapping.
                // `display`/`details` are not carried on `AgentMessage::Custom`, so persist with the
                // bash-message convention (display=true, no details).
                let _ = self
                    .manager
                    .lock()
                    .await
                    .append_custom_message(kind, payload.clone(), true, None);
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
            // gap-08 #1: a guest-replaced `message_end` message is fanned out (not the original) so
            // downstream observers (TUI / SDK) see the replacement, matching the persisted tree.
            (AgentEvent::MessageEnd { .. }, _) if replaced_end.is_some() => match replaced_end {
                Some(m) => AgentSessionEvent::MessageEnd { message: m },
                None => AgentSessionEvent::from_agent(event),
            },
            _ => AgentSessionEvent::from_agent(event),
        };
        self.fanout.emit(svc_ev).await;

        // 3. Terminate run-scoped subscriptions once the run settles — but ONLY on an unbound session.
        //    A bound session's post-run driver owns run termination (it may continue past this
        //    `agent_end` for a retry / compaction / queued continuation).
        //
        //    SEAM-005: this is ALSO the unbound session's settle point, and therefore where its
        //    `agent_settled` belongs. Pi always settles (`_emitAgentSettled` is in `_runAgentPrompt`'s
        //    `finally`, agent-session.ts:1063-1072). An unbound `AgentSession` has NO post-run driver
        //    — `spawn_run`'s `None` arm starts the run and returns — so no retry, compaction or
        //    queued continuation can follow this `agent_end`; the run is settled by construction the
        //    instant it ends. Emitting here rather than in `spawn_run` matters: `agent.prompt`
        //    returns as soon as the run is DISPATCHED, so a settle emitted there would fire while
        //    the model was still streaming. Ordered before `end_run` so a run-scoped subscriber sees
        //    it, and — like the bound path — extensions are notified before subscribers.
        if is_end && session.is_none() {
            let cancel = self.session_cancel.child_token();
            self.ext_host
                .dispatcher()
                .dispatch_notify(&cyrup_ext::HostEvent::AgentSettled, &cancel)
                .await;
            self.fanout.emit(AgentSessionEvent::AgentSettled).await;
            self.fanout.end_run();
        }
    }
}
