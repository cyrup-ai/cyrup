//! `ExtSubscriber` — the notify-only agent seam (arch-08 §5.4). Implements
//! `cyrup_agent::EventSubscriber`: each agent event maps to a `HostEvent` dispatched to subscribed
//! extensions, awaited in load order, before the agent emits the next event (R-02-012/048,
//! R-08-004/011). Near-zero cost when nobody subscribes (R-08-034).

use crate::dispatch::Dispatcher;
use crate::event::{EventKind, HostEvent};
use cyrup_agent::{AgentEvent, EventSubscriber};
use cyrup_core::CancelToken;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The notify-only subscriber handed to the agent (arch-08 §3.1).
pub struct ExtSubscriber {
    dispatcher: Arc<Dispatcher>,
    /// Turn counter mirroring Pi's `AgentSession._turnIndex` (agent-session.ts:302/616/635). The
    /// upstream `AgentEvent::{TurnStart, TurnEnd}` are payload-less, so — exactly like Pi's
    /// `_emitExtensionEvent` — the turn index a `turn_start`/`turn_end` carries is DERIVED in this
    /// fan-out layer: reset to 0 on `agent_start`, read for both turn events, incremented after each
    /// `turn_end`. Single-instance + sequential (the agent awaits each `on_event`), so `Relaxed` is
    /// sufficient; the counter advances even when nobody subscribed so the index never drifts.
    turn_index: AtomicU32,
}

impl ExtSubscriber {
    /// EXT-061: no subscriber-lifetime token. It used to take one, store it behind
    /// `#[allow(dead_code)]` — the compiler stating the field is never read — under a doc that
    /// asserted it "stays as the fallback for a caller that hands us a token detached from any
    /// run". No such fallback existed: [`Self::on_event`] uses only the per-event `cancel`
    /// argument, and the `allow` is what suppressed the warning that would have shown it. pi has
    /// no subscriber-lifetime signal either — it passes the run's signal per listener (`await
    /// listener(event, signal)`, `packages/agent/src/agent.ts:574` @v0.83.0) — so the code matched
    /// upstream and the doc did not. Removed rather than implemented, which is the port.
    pub fn new(dispatcher: Arc<Dispatcher>) -> Self {
        Self {
            dispatcher,
            turn_index: AtomicU32::new(0),
        }
    }
}

#[async_trait::async_trait]
impl EventSubscriber for ExtSubscriber {
    /// `cancel` is the run's abort signal, the second argument pi passes to every listener
    /// (`await listener(event, signal)`, `packages/agent/src/agent.ts:574` @v0.83.0) — a FRESH
    /// child of the run token, so it is what a dispatched handler should race against rather than
    /// a token captured at construction — which is why this type keeps none (EXT-061).
    async fn on_event(&self, event: &AgentEvent, cancel: CancelToken) {
        // Maintain the Pi turn counter BEFORE the subscription gate so the index stays correct even
        // when the intervening events have no subscribers (Pi agent-session.ts:615-635). `agent_start`
        // resets; `turn_end` reads-then-increments (fetch_add returns the pre-increment value, which
        // is the index Pi emits with the `turn_end`).
        let turn_index = match event {
            AgentEvent::AgentStart => {
                self.turn_index.store(0, Ordering::Relaxed);
                0
            }
            AgentEvent::TurnEnd { .. } => self.turn_index.fetch_add(1, Ordering::Relaxed),
            _ => self.turn_index.load(Ordering::Relaxed),
        };

        // Cheap gate: map to kind and bail before any serialization if nobody subscribed
        // (R-08-034 / R-ARCH-EXT-014).
        let Some(kind) = EventKind::from_agent(event) else {
            return;
        };
        if self.dispatcher.no_subscribers(kind) {
            return;
        }
        let Some(host_ev) = HostEvent::from_agent(event) else {
            return;
        };
        // Inject the derived turn index into the turn events (the raw upstream events omit it).
        let host_ev = match host_ev {
            HostEvent::TurnStart { timestamp, .. } => HostEvent::TurnStart {
                turn_index,
                timestamp,
            },
            HostEvent::TurnEnd {
                message,
                tool_results,
                ..
            } => HostEvent::TurnEnd {
                turn_index,
                message,
                tool_results,
            },
            other => other,
        };
        self.dispatcher.dispatch_notify(&host_ev, &cancel).await;
    }
}
