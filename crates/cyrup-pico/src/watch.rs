//! `watchDoc()`: an acquisition that buffers, a **consuming** `start`, and a delivery loop the host
//! drives.
//!
//! # Why `start` consumes the watch
//!
//! `spec.md:3797` says *"installs the **sole** serialized asynchronous listener"* and `spec.md:3867`
//! repeats it: *"`start()` installs the sole listener and schedules delivery; it never invokes user
//! code inline."* Upstream enforces the first half with a runtime guard and the second with
//! scheduling discipline. Here [`DocWatch::start`] takes `self`, so a second listener is `E0382`
//! (`tests/compile-fail/a_watch_cannot_install_a_second_listener.rs`) — and it returns the delivery
//! future instead of running it, so *"never invokes user code inline"* is the signature: there is no
//! code path on which `start` could call the listener, because `start` is not `async`.
//!
//! # Why `start` hands back a future rather than spawning one
//!
//! This crate calls `tokio::spawn` nowhere and its `tokio` feature list is `sync` alone
//! ([`crate::committer`] explains why for the line; the same reason applies here). So the delivery
//! loop is a value: the host spawns it, drives it on a `LocalSet`, or polls it in its own loop, and
//! the watch's lifetime is visible in the host's task graph rather than hidden in a detached task.
//! Dropping the future stops the watch, because the future owns the registration's [`Disposer`] —
//! which is the right owner: nothing is delivering any more, so nothing should be buffered.
//!
//! # Before `start`
//!
//! `spec.md:3856`: *"Before `start()`, `watch.value` remains the acquisition revision while later
//! frames buffer."* [`DocWatch::value`] is that revision and the slot is already registered, so the
//! `initializeConsumer(watch.value)`-then-`start` shape in the specification's own example works here
//! with nothing lost in between.
//!
//! # The 100-frame bound
//!
//! `spec.md:3875-3882`'s overflow rule is in [`crate::MAX_PENDING_FRAMES`], applied by the slot at
//! push time, because the line is where the newest revision is known and the collapse must cost one
//! refcount bump.

use core::future::Future;
use core::marker::PhantomData;
use std::sync::Arc;

use cyrup_pico_store::{Cx, DocumentId};

use crate::observe::Disposer;
use crate::revision::{Frame, Revision};
use crate::slot::{DocSlot, ListenerFailed, WatchEnd};
use crate::state::typed_frame;

/// An acquired watch, buffering, with no listener yet.
///
/// Dropping it without calling [`DocWatch::start`] disposes the registration.
#[derive(Debug)]
pub struct DocWatch<T> {
    slot: Arc<DocSlot>,
    disposer: Disposer,
    cx: Cx,
    _t: PhantomData<fn() -> T>,
}

impl<T> DocWatch<T> {
    /// **One call site**, in [`Session::watch_doc`](crate::Session::watch_doc).
    pub(crate) const fn new(slot: Arc<DocSlot>, disposer: Disposer, cx: Cx) -> Self {
        Self {
            slot,
            disposer,
            cx,
            _t: PhantomData,
        }
    }

    /// The incarnation this watch is bound to, for its whole life.
    #[must_use]
    pub fn document(&self) -> DocumentId {
        self.slot.document()
    }

    /// The acquisition revision, until a listener starts advancing it.
    #[must_use]
    pub fn value(&self) -> Option<Revision<T>> {
        self.slot
            .current()
            .map(|root| Revision::new(self.slot.document(), root))
    }

    /// How many committed frames have buffered since acquisition.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.slot.pending()
    }

    /// Install the sole listener and return the handle plus the delivery loop. **Consuming.**
    ///
    /// The listener is awaited before the next callback starts — `spec.md:3872-3874`'s *"the watch
    /// awaits the listener before starting its next callback, but Session commits never wait for
    /// callback settlement"*, which here is two facts about two places: this loop is sequential, and
    /// it is not the line.
    ///
    /// A listener that returns `Err` ends **only this watch**, as
    /// [`WatchEnd::ListenerError`] — `spec.md:3901`'s *"listener failure discards pending work and
    /// closes only that watch"*.
    #[must_use = "the second element is the delivery loop; dropping it stops the watch"]
    pub fn start<L, Fut>(self, mut listener: L) -> (Watching<T>, impl Future<Output = WatchEnd>)
    where
        L: FnMut(Frame<T>) -> Fut,
        Fut: Future<Output = Result<(), ListenerFailed>>,
    {
        let Self {
            slot,
            disposer,
            cx,
            _t,
        } = self;
        let handle = Watching {
            slot: Arc::clone(&slot),
            _t: PhantomData,
        };
        let delivery = async move {
            // Owned by the loop, so dropping the loop unregisters the subscription.
            let _registration = disposer;
            let document = slot.document();
            loop {
                let Some(raw) = slot.next(&cx).await else {
                    // `next` returns `None` only once the slot has a terminal reason, and
                    // `WatchEnd::Stopped` is the one a dropped handle or an explicit `stop` sets.
                    break slot.end().unwrap_or(WatchEnd::Stopped);
                };
                let frame = typed_frame::<T>(document, raw);
                let terminal = frame.is_terminal();
                if let Err(failed) = listener(frame).await {
                    let end = WatchEnd::ListenerError(failed);
                    slot.terminate(end.clone());
                    break end;
                }
                if terminal {
                    // `DocSlot::take` already set `Retired`; this is where the loop observes it,
                    // which is `spec.md:3897`'s *"after that callback settles, the watch closes as
                    // retired"*.
                    break slot.end().unwrap_or(WatchEnd::Retired);
                }
            }
        };
        (handle, delivery)
    }
}

/// A started watch's handle: the value it currently exposes, and an idempotent stop.
///
/// Deliberately does **not** own the registration — the delivery loop does. A host can therefore drop
/// the handle and leave a watch running, or keep the handle and drop the loop to stop it, and in both
/// cases the thing that owns the subscription is the thing doing the work.
#[derive(Debug)]
pub struct Watching<T> {
    slot: Arc<DocSlot>,
    _t: PhantomData<fn() -> T>,
}

impl<T> Watching<T> {
    /// The incarnation this watch is bound to.
    #[must_use]
    pub fn document(&self) -> DocumentId {
        self.slot.document()
    }

    /// The latest delivered revision, or `None` once retirement was delivered.
    ///
    /// `spec.md:3871`: *"immediately before invocation, `watch.value` advances to that value."*
    #[must_use]
    pub fn value(&self) -> Option<Revision<T>> {
        self.slot
            .current()
            .map(|root| Revision::new(self.slot.document(), root))
    }

    /// How many committed frames are buffered and undelivered.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.slot.pending()
    }

    /// Stop the watch, and return the terminal result.
    ///
    /// `spec.md:3899-3903`: *"`stop()` is idempotent. It synchronously unregisters, discards pending
    /// frames, prevents another callback from starting, and returns the common terminal promise. An
    /// already-running callback is not aborted or joined and remains caller-owned. … The first
    /// termination reason wins."*
    ///
    /// All four clauses, in order: the call is synchronous and sets the terminal reason, which clears
    /// the pending buffer and makes the next [`crate::slot::DocSlot::next`] return `None`; a callback
    /// already awaiting is untouched, because this function does not hold it; and the reason returned
    /// is whichever landed first, so a stop after a retirement reports `Retired`.
    pub fn stop(&self) -> WatchEnd {
        self.slot.terminate(WatchEnd::Stopped);
        self.slot.end().unwrap_or(WatchEnd::Stopped)
    }

    /// The terminal reason, or `None` while the watch is live.
    #[must_use]
    pub fn end(&self) -> Option<WatchEnd> {
        self.slot.end()
    }
}
