//! The inter-extension event bus (`pi.events`) — the host-owned coordination channel.
//!
//! Upstream this is one `createEventBus()` per process, threaded onto EVERY `ExtensionAPI` the
//! loader builds regardless of what kind of extension it is going to serve
//! (`events: eventBus,` on the returned API object,
//! `pi/packages/coding-agent/src/core/extensions/loader.ts:389` @v0.83.0; impl
//! `core/event-bus.ts:12-32`). pi has exactly one extension kind, so "every extension gets the bus"
//! needs no further qualification.
//!
//! cyrup has two tiers — WASM guests and compiled-in natives — and the bus lived inside the
//! `wasm-host` feature gate, so the three extensions cyrup actually ships (permission-system,
//! intercom, subagents) were all natives with no `pi.events` at all (EXT-018). It lives here,
//! outside every cfg, for the same reason pi puts it on the base API: which tier an extension
//! happens to run in is not something the coordination channel is allowed to know.
//!
//! **Why delivery is deferred rather than synchronous.** pi's `emit` runs every listener
//! synchronously inside the emit call (a node `EventEmitter`), which a WASM guest cannot do: a
//! guest emitting from inside its own `bus.emit` import already holds its store, and delivering to
//! a subscriber re-enters that same single-instance store. So cyrup queues on `emit` and fans out
//! at the next seam boundary ([`crate::ExtensionHost::deliver_bus_events`]). That deferral is
//! forced; the CYRUP-DELTA is recorded on `SharedBus::emit`.

use cyrup_core::ExtensionId;
use serde_json::Value;
use std::collections::VecDeque;
use std::sync::Mutex;

/// The host-owned inter-extension event bus (pi `createEventBus()`, `core/event-bus.ts:12-32`
/// @v0.83.0). One per [`crate::ExtensionHost`], shared into every loaded guest AND consulted for
/// every loaded native.
#[derive(Default)]
pub struct SharedBus {
    /// `(owner, topic)` subscriptions in registration/load order (pi's per-channel listener list).
    subs: Mutex<Vec<(ExtensionId, String)>>,
    /// Emitted `(topic, payload)` awaiting fan-out, FIFO (pi emits in call order).
    pending: Mutex<VecDeque<(String, Value)>>,
}

impl SharedBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `owner` listens on `topic` (pi `pi.events.on`, `event-bus.ts:18`). Idempotent
    /// per `(owner, topic)` pair so a re-declared subscription does not duplicate delivery.
    pub fn subscribe(&self, owner: ExtensionId, topic: String) {
        if let Ok(mut g) = self.subs.lock()
            && !g.iter().any(|(o, t)| *o == owner && *t == topic)
        {
            g.push((owner, topic));
        }
    }

    /// Stop `owner` listening on `topic` (EXT-050). pi's `on()` returns an unsubscribe closure —
    /// `return runtime.trackEventBusSubscription(eventBus.on(channel, handler));`
    /// (`extensions/loader.ts:413-421` @v0.84.1) — so a listener that is only wanted while a mode
    /// is active can be taken down. Returns whether a subscription was actually removed.
    pub fn unsubscribe(&self, owner: &ExtensionId, topic: &str) -> bool {
        let Ok(mut g) = self.subs.lock() else { return false };
        let before = g.len();
        g.retain(|(o, t)| !(o == owner && t == topic));
        g.len() != before
    }

    /// Drop every subscription belonging to `owner` (EXT-050 teardown). This is the structural
    /// analog of pi's `invalidate()`, which runs every tracked unsubscribe and clears the set
    /// (`extensions/loader.ts:206-214` @v0.84.1). Called when an extension leaves the host's live
    /// map, so a replaced or unloaded instance stops receiving. Returns how many were removed.
    pub fn unsubscribe_all(&self, owner: &ExtensionId) -> usize {
        let Ok(mut g) = self.subs.lock() else { return 0 };
        let before = g.len();
        g.retain(|(o, _)| o != owner);
        before - g.len()
    }

    /// Enqueue an emitted event for deferred fan-out.
    ///
    /// CYRUP-DELTA: pi delivers synchronously — `emit: (channel, data) => { emitter.emit(channel,
    /// data); }` over a node `EventEmitter` runs every listener at the emit call
    /// (`core/event-bus.ts:12-32` @v0.83.0), so upstream has no queue and nothing can be pending.
    /// cyrup cannot: a WASM guest's `bus.emit` import runs while that guest holds its own
    /// single-instance store, and delivering to a subscriber inside it would re-enter the store
    /// that is already borrowed. The queue is drained at the next seam boundary instead
    /// ([`crate::ExtensionHost::deliver_bus_events`]).
    pub fn emit(&self, topic: String, payload: Value) {
        if let Ok(mut g) = self.pending.lock() {
            g.push_back((topic, payload));
        }
    }

    /// Drain every queued event (the host delivers them, then re-checks for cascaded emits).
    pub fn take_pending(&self) -> Vec<(String, Value)> {
        self.pending.lock().map(|mut g| g.drain(..).collect()).unwrap_or_default()
    }

    /// How many events are still queued. Used by the fan-out to tell "the queue emptied" from
    /// "the round bound was reached with work left" (EXT-057).
    pub fn pending_len(&self) -> usize {
        self.pending.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Discard every queued event, returning how many were dropped. Called only when the fan-out
    /// gives up at its round bound, so the drop is explicit and reportable rather than a silent
    /// fall-out of a `for` loop (EXT-057).
    pub fn drop_pending(&self) -> usize {
        self.pending.lock().map(|mut g| g.drain(..).count()).unwrap_or(0)
    }

    /// The extension ids subscribed to `topic`, in subscription order (pi listener order).
    pub fn subscribers_for(&self, topic: &str) -> Vec<ExtensionId> {
        self.subs
            .lock()
            .map(|g| g.iter().filter(|(_, t)| t == topic).map(|(o, _)| o.clone()).collect())
            .unwrap_or_default()
    }

    /// Drop all subscriptions + queued events (hot-reload, R-08-005): the fresh load re-declares
    /// them.
    pub fn clear(&self) {
        if let Ok(mut g) = self.subs.lock() {
            g.clear();
        }
        if let Ok(mut g) = self.pending.lock() {
            g.clear();
        }
    }
}
