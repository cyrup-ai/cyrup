//! `ExtSubscriber` — the notify-only agent seam (arch-08 §5.4). Implements
//! `cyrup_agent::EventSubscriber`: each agent event maps to a `HostEvent` dispatched to subscribed
//! extensions, awaited in load order, before the agent emits the next event (R-02-012/048,
//! R-08-004/011). Near-zero cost when nobody subscribes (R-08-034).

use crate::dispatch::Dispatcher;
use crate::event::{EventKind, HostEvent};
use cyrup_agent::{AgentEvent, EventSubscriber};
use cyrup_core::CancelToken;
use std::sync::Arc;

/// The notify-only subscriber handed to the agent (arch-08 §3.1).
pub struct ExtSubscriber {
    dispatcher: Arc<Dispatcher>,
    cancel: CancelToken,
}

impl ExtSubscriber {
    pub fn new(dispatcher: Arc<Dispatcher>, cancel: CancelToken) -> Self {
        Self { dispatcher, cancel }
    }
}

#[async_trait::async_trait]
impl EventSubscriber for ExtSubscriber {
    async fn on_event(&self, event: &AgentEvent) {
        // Cheap gate FIRST: map to kind and bail before any serialization if nobody subscribed
        // (R-08-034 / R-ARCH-EXT-014).
        let Some(kind) = EventKind::from_agent(event) else { return };
        if self.dispatcher.no_subscribers(kind) {
            return;
        }
        let Some(host_ev) = HostEvent::from_agent(event) else { return };
        self.dispatcher.dispatch_notify(&host_ev, &self.cancel).await;
    }
}
