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
use std::sync::{Mutex, Weak};

use crate::native::NativeExtension;

/// One typed-bus listener: its owner, its topic and the native that handles it. Held WEAKLY: the
/// native owns (through its `HostServices`) a handle to this bus, so a strong edge back would be a
/// cycle; the host's native map is what keeps a loaded native alive.
type TypedSub = (ExtensionId, String, Weak<dyn NativeExtension>);

/// The host-owned inter-extension event bus (pi `createEventBus()`, `core/event-bus.ts:12-32`
/// @v0.83.0). One per [`crate::ExtensionHost`], shared into every loaded guest AND consulted for
/// every loaded native.
#[derive(Default)]
pub struct SharedBus {
    /// `(owner, topic)` subscriptions in registration/load order (pi's per-channel listener list).
    subs: Mutex<Vec<(ExtensionId, String)>>,
    /// Emitted `(topic, payload)` awaiting fan-out, FIFO (pi emits in call order).
    pending: Mutex<VecDeque<(String, Value)>>,
    /// Typed-bus listeners ([`Self::subscribe_typed`]) in load order.
    typed_subs: Mutex<Vec<TypedSub>>,
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
        let Ok(mut g) = self.subs.lock() else {
            return false;
        };
        let before = g.len();
        g.retain(|(o, t)| !(o == owner && t == topic));
        g.len() != before
    }

    /// Drop every subscription belonging to `owner` (EXT-050 teardown). This is the structural
    /// analog of pi's `invalidate()`, which runs every tracked unsubscribe and clears the set
    /// (`extensions/loader.ts:206-214` @v0.84.1). Called when an extension leaves the host's live
    /// map, so a replaced or unloaded instance stops receiving. Returns how many were removed.
    pub fn unsubscribe_all(&self, owner: &ExtensionId) -> usize {
        let typed = self
            .typed_subs
            .lock()
            .map(|mut g| {
                let before = g.len();
                g.retain(|(o, _, _)| o != owner);
                before - g.len()
            })
            .unwrap_or(0);
        let Ok(mut g) = self.subs.lock() else {
            return typed;
        };
        let before = g.len();
        g.retain(|(o, _)| o != owner);
        typed + before - g.len()
    }

    /// Record that the native `listener` (owned by `owner`) listens on the typed `topic`
    /// ([`crate::InitApi::subscribe_typed_bus`]). Idempotent per `(owner, topic)` pair.
    pub fn subscribe_typed(
        &self,
        owner: ExtensionId,
        topic: String,
        listener: Weak<dyn NativeExtension>,
    ) {
        if let Ok(mut g) = self.typed_subs.lock()
            && !g.iter().any(|(o, t, _)| *o == owner && *t == topic)
        {
            g.push((owner, topic, listener));
        }
    }

    /// Hand `event` to every native listening on the typed `topic`, in load order, and return once
    /// each has run — pi's synchronous `emit` (`core/event-bus.ts:15-17` @v0.87.1), which is what
    /// lets an emitter read what its listeners did to `event` (a `claim`) as soon as this returns.
    ///
    /// Not queued, unlike [`Self::emit`]: the listeners are natives, which share no store to
    /// re-enter, so nothing forces the deferral here. The listener list is copied out of the lock
    /// before any listener runs, so a listener may itself emit or subscribe.
    pub fn emit_typed(&self, topic: &str, event: &dyn std::any::Any) {
        let listeners: Vec<Weak<dyn NativeExtension>> = self
            .typed_subs
            .lock()
            .map(|g| {
                g.iter()
                    .filter(|(_, t, _)| t == topic)
                    .map(|(_, _, l)| l.clone())
                    .collect()
            })
            .unwrap_or_default();
        for listener in listeners.iter().filter_map(Weak::upgrade) {
            listener.on_typed_bus_event(topic, event);
        }
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
        self.pending
            .lock()
            .map(|mut g| g.drain(..).collect())
            .unwrap_or_default()
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
        self.pending
            .lock()
            .map(|mut g| g.drain(..).count())
            .unwrap_or(0)
    }

    /// The extension ids subscribed to `topic`, in subscription order (pi listener order).
    pub fn subscribers_for(&self, topic: &str) -> Vec<ExtensionId> {
        self.subs
            .lock()
            .map(|g| {
                g.iter()
                    .filter(|(_, t)| t == topic)
                    .map(|(o, _)| o.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The fan-out seam (EXT-034).
///
/// Upstream there is nothing to abstract: `emit` runs every listener inline
/// (`pi/packages/coding-agent/src/core/event-bus.ts:12-32` @v0.83.0), so pi has no entry point from
/// which an emit can go undelivered. cyrup must defer (see [`SharedBus::emit`]), which means every
/// seam that can have re-entered a guest has to drain afterwards — including
/// [`crate::dispatch::Dispatcher`]'s event entry points, which know nothing about the host that owns
/// the live/native maps. This trait is the handle the dispatcher holds so it can drain without
/// depending on the facade.
#[async_trait::async_trait]
pub trait BusDrain: Send + Sync {
    /// Fan out every queued event to its subscribers, then re-check for cascaded emits.
    /// Re-entrant calls must be no-ops (the outer drain owns the queue).
    async fn drain_bus(&self, cancel: &cyrup_core::CancelToken);
}

/// Re-entrancy latch for a drain in progress.
///
/// Deliberately RAII rather than a plain `store(false)` at the end of the drain: a Rust future can
/// be dropped at any `.await` (an aborted run drops the whole dispatch tree), and a latch cleared
/// only on the success path would then stay set forever and permanently disable bus delivery. pi
/// cannot hit this — a node `EventEmitter` callback is synchronous and always runs to completion.
pub(crate) struct DrainLatch<'a>(&'a std::sync::atomic::AtomicBool);

impl<'a> DrainLatch<'a> {
    /// `Some` iff no drain was already in progress on this flag.
    pub(crate) fn acquire(flag: &'a std::sync::atomic::AtomicBool) -> Option<Self> {
        use std::sync::atomic::Ordering;
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(flag))
    }
}

impl Drop for DrainLatch<'_> {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}
