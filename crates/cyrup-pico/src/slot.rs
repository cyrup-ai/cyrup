//! The subscription slot: one bounded pending buffer, shared between the mutation line that fills it
//! and the consumer that drains it **off** the line.
//!
//! # Why there is one slot type and two handles over it
//!
//! [`crate::DocState`] and [`crate::DocWatch`] are different consumer shapes over the same
//! obligation: *"capture their baseline and subscription atomically on the Session line: a later
//! commit already present becomes the baseline, while one committed after registration is delivered"*
//! (`spec.md:1515-1519`). Writing that twice would be writing the ordering argument twice, so it is
//! written once, here, and the two handles differ only in how their consumer is driven.
//!
//! The slot is **untyped**. A [`DocumentId`]-keyed registry cannot be generic over each subscriber's
//! `T`, and type-erasing with `dyn Any` would put a downcast on the delivery path for no gain: the
//! value is a [`DocRoot`] either way, and the `T` is a tag the *handle* carries
//! ([`crate::Revision`]). So there is no `Any`, no downcast, and no per-subscriber monomorphisation
//! of the line's fan-out.
//!
//! # How the ordering is made exact, without a lock held across the commit
//!
//! The hazard is a one-instruction window: adoption swaps the authority and releases its lock, then
//! publication fans frames out. A subscription registered inside that window would otherwise either
//! miss the frame (and be one commit stale forever) or receive a frame equal to the baseline it just
//! captured.
//!
//! Neither happens, and the mechanism is a **watermark rather than a longer lock**. Acquisition does
//! three things in this order: register the slot, read the baseline, then [`DocSlot::arm`] the slot
//! with the value *and* the tracker revision ([`cyrup_pico_doc::Revision`]) it was read at. Every
//! frame the line pushes carries the revision its value is at, so:
//!
//! | the commit adopts | baseline read | buffered frame | delivered |
//! |---|---|---|---|
//! | before the baseline is read | `r+1` | `r+1` | no — *"a commit already present becomes the baseline"* |
//! | after the baseline is read | `r` | `r+1` | yes |
//! | inside the registration window | either | kept, then filtered by `arm` | exactly once |
//!
//! The third row is why registration comes **first**: a slot registered after the fan-out would miss
//! the frame with nothing to detect it, while a slot registered before it can only ever buffer one too
//! many — and one too many is a thing a watermark removes. There is no fourth row, because ADR-0030
//! F1 left exactly one committer: no second commit can interleave between an adoption and its
//! publication.
//!
//! **Adoption itself is untouched.** `spec.md:1305-1310` forbids adoption from doing anything that
//! can fail or allocate, so the fan-out is at publication and
//! [`DocIndex::adopt`](crate::docs::DocIndex::adopt) never reads this registry —
//! `src/tests/adoption.rs` and `tests/adoption_is_allocation_free.rs` stay green by construction.

use core::future::Future;
use core::pin::pin;
use core::task::Poll;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, RwLock, Weak};

use cyrup_pico_doc::{DocRoot, Op, OpBatch, Revision as DocRevision};
use cyrup_pico_store::{Cx, DocumentId};
use tokio::sync::Notify;

use crate::observe::Retirable;
use crate::revision::FrameContext;

/// How many committed frames one subscription buffers, excluding the one being delivered.
///
/// `spec.md:3875-3882`: *"A watch retains at most 100 pending committed frames, excluding the frame
/// already being delivered. Adding frame 101 replaces the complete undelivered suffix with one
/// self-contained root replacement `[["r", newestValue]]`, using the newest exact immutable revision
/// and that commit's Context."*
///
/// The replacement is **self-contained**, which is why it is sound to drop the suffix: the newest
/// value is already an immutable `Arc`, so collapsing is one refcount bump and *"no serialized-byte
/// measurement, value copy, operation replay, or re-diff occurs"* holds literally. A consumer that
/// needs every transition is told to persist them itself — *"this is convergent observation, not an
/// audit stream"*.
pub const MAX_PENDING_FRAMES: usize = 100;

/// Why a subscription ended. The **first** reason wins.
///
/// `spec.md:3790-3793`'s `WatchEnd`, with one name changed and one added. Upstream's `"stopped"` and
/// `"cancelled"` are both here; `Retired` is its `"retired"`; `SessionClosed` is its
/// `"session_closed"`; and [`WatchEnd::ListenerError`] carries the failure instead of a bare reason
/// tag, because a Rust listener returns a `Result` and throwing the error away would make
/// *"every late rejection is observed"* (`spec.md:3902`) impossible to honour.
///
/// Used by [`crate::DocState`] as well as [`crate::DocWatch`]: a document-state subscription ends for
/// the same four non-listener reasons, and `spec.md:1252` calls it *"disposable"* for exactly the
/// first of them.
#[derive(Clone, Debug)]
pub enum WatchEnd {
    /// The consumer disposed it, or dropped the handle that owned the registration.
    Stopped,
    /// The acquisition [`Cx`] was cancelled.
    Cancelled,
    /// The Session closed. `spec.md:1531`: *"the Harness stops watches ... there"*.
    SessionClosed,
    /// The bound incarnation was retired, and its terminal frame was delivered.
    ///
    /// *"A watch remains bound to its original incarnation ... and never follows a replacement
    /// incarnation"* (`spec.md:3896-3898`).
    Retired,
    /// The listener failed. Only this subscription closes.
    ListenerError(ListenerFailed),
}

/// A watch listener reported failure.
///
/// `Arc` rather than `Box` so [`WatchEnd`] is `Clone`, which is what lets
/// [`crate::Watching::stop`] and the delivery future both return *the same* terminal
/// result — `spec.md:3899`'s *"returns the common terminal promise"*.
#[derive(Clone)]
pub struct ListenerFailed(Arc<dyn core::error::Error + Send + Sync>);

impl ListenerFailed {
    /// Wrap a listener's error.
    #[must_use]
    pub fn new(error: impl core::error::Error + Send + Sync + 'static) -> Self {
        Self(Arc::new(error))
    }

    /// The error.
    #[must_use]
    pub fn source(&self) -> &(dyn core::error::Error + Send + Sync) {
        &*self.0
    }
}

impl core::fmt::Debug for ListenerFailed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("ListenerFailed").field(&self.0).finish()
    }
}

impl core::fmt::Display for ListenerFailed {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "a watch listener failed: {}", self.0)
    }
}

impl core::error::Error for ListenerFailed {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        Some(&*self.0)
    }
}

/// One buffered delivery, before a handle tags it with its `T`.
#[derive(Clone, Debug)]
pub(crate) struct RawFrame {
    /// `None` is retirement. A [`DocRoot`] is a JSON object by construction, so upstream's
    /// `[["r", null]]` has no spelling — see [`crate::Frame`] for the `CYRUP-DELTA`.
    pub(crate) value: Option<DocRoot>,
    pub(crate) ops: OpBatch,
    pub(crate) context: FrameContext,
    /// The tracker revision this frame's value is at, and the watermark half of this module's
    /// ordering table. `None` is a retirement, which is never filtered: a subscription that could
    /// have registered after the retiring adoption never registered at all, because its address had
    /// no live incarnation left to resolve.
    pub(crate) revision: Option<DocRevision>,
}

/// The mutable half of a slot.
#[derive(Debug)]
struct SlotInner {
    /// The revision a consumer currently sees: the acquisition value until a frame is taken, then
    /// the value of the frame being delivered. `None` once retirement is delivered.
    ///
    /// `spec.md:3871-3872`: *"Immediately before invocation, `watch.value` advances to that value."*
    current: Option<DocRoot>,
    pending: VecDeque<RawFrame>,
    end: Option<WatchEnd>,
    /// The watermark, once [`DocSlot::arm`] has read it. `None` is the registration window: frames
    /// arriving then are buffered with their revisions and filtered when the watermark lands.
    baseline: Option<DocRevision>,
}

/// One subscription's shared state.
#[derive(Debug)]
pub(crate) struct DocSlot {
    document: DocumentId,
    inner: Mutex<SlotInner>,
    wake: Notify,
}

impl Retirable for DocSlot {
    /// Idempotent: [`DocSlot::terminate`] keeps the first reason.
    fn retire(&self) {
        self.terminate(WatchEnd::Stopped);
    }

    fn is_retired(&self) -> bool {
        self.end().is_some()
    }
}

impl DocSlot {
    /// Open a buffer for one incarnation. **Registered before its baseline is read**, which is the
    /// order this module's table depends on: a frame that lands in the window between registration
    /// and [`DocSlot::arm`] is buffered with its revision and then filtered against the watermark,
    /// so neither a missed frame nor a duplicate of the baseline is possible.
    pub(crate) fn new(document: DocumentId) -> Self {
        Self {
            document,
            inner: Mutex::new(SlotInner {
                current: None,
                pending: VecDeque::new(),
                end: None,
                baseline: None,
            }),
            wake: Notify::new(),
        }
    }

    /// Install the baseline the registration captured, and drop every buffered frame at or below it.
    ///
    /// One critical section, so *"a later commit already present becomes the baseline, while one
    /// committed after registration is delivered"* (`spec.md:1515-1519`) is decided once rather than
    /// per frame.
    pub(crate) fn arm(&self, value: DocRoot, baseline: DocRevision) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.current = Some(value);
            inner.baseline = Some(baseline);
            inner
                .pending
                .retain(|f| f.revision.is_none_or(|r| r > baseline));
        }
    }

    /// The incarnation this subscription is bound to, for its whole life.
    pub(crate) const fn document(&self) -> DocumentId {
        self.document
    }

    /// The revision a consumer sees now.
    pub(crate) fn current(&self) -> Option<DocRoot> {
        self.inner.lock().ok().and_then(|i| i.current.clone())
    }

    /// The terminal reason, if the subscription has ended.
    pub(crate) fn end(&self) -> Option<WatchEnd> {
        self.inner.lock().ok().and_then(|i| i.end.clone())
    }

    /// End the subscription. **First reason wins**, pending frames are discarded, and a waiting
    /// consumer is woken so it observes the end rather than blocking on it.
    ///
    /// Idempotent, which is the whole of [`crate::Disposer`]'s contract: a second call finds `end`
    /// already set and changes nothing.
    pub(crate) fn terminate(&self, reason: WatchEnd) {
        if let Ok(mut inner) = self.inner.lock()
            && inner.end.is_none()
        {
            inner.end = Some(reason);
            inner.pending.clear();
        }
        self.wake.notify_waiters();
    }

    /// Buffer one committed frame. Called on the mutation line, at publication.
    ///
    /// Runs no user code and takes no lock a consumer holds across an `await`, so the line is not
    /// exposed to a consumer's latency — which is `spec.md:1524`'s *"document-state and watch user
    /// callbacks run later"* as a property of this function rather than of a scheduler.
    pub(crate) fn push(&self, frame: RawFrame) {
        if let Ok(mut inner) = self.inner.lock() {
            if inner.end.is_some() {
                return;
            }
            // The watermark. Unset only inside the registration window, where every frame is kept
            // and `arm` filters them in one pass.
            if let (Some(base), Some(at)) = (inner.baseline, frame.revision)
                && at <= base
            {
                return;
            }
            if inner.pending.len() >= MAX_PENDING_FRAMES {
                // Overflow: collapse the complete undelivered suffix into one self-contained frame.
                // `Op::ReplaceRoot` *is* upstream's `["r", newestValue]`, and the retirement arm is
                // its *"folds it into an overflow replacement with value `null`"*.
                let collapsed = match &frame.value {
                    Some(root) => RawFrame {
                        value: Some(root.clone()),
                        ops: OpBatch::new(vec![Op::ReplaceRoot(root.clone())]),
                        context: frame.context,
                        revision: frame.revision,
                    },
                    None => RawFrame {
                        value: None,
                        ops: OpBatch::empty(),
                        context: frame.context,
                        revision: None,
                    },
                };
                inner.pending.clear();
                inner.pending.push_back(collapsed);
            } else {
                inner.pending.push_back(frame);
            }
        }
        self.wake.notify_waiters();
    }

    /// Take the next buffered frame, advancing what a consumer sees.
    ///
    /// Taking a terminal frame also sets the terminal reason, so the frame after a retirement is
    /// always `None` and *"never follows a replacement incarnation"* needs no second check.
    pub(crate) fn take(&self) -> Option<RawFrame> {
        let mut inner = self.inner.lock().ok()?;
        let frame = inner.pending.pop_front()?;
        inner.current = frame.value.clone();
        if frame.value.is_none() && inner.end.is_none() {
            inner.end = Some(WatchEnd::Retired);
        }
        Some(frame)
    }

    /// How many frames are buffered and undelivered.
    pub(crate) fn pending(&self) -> usize {
        self.inner
            .lock()
            .map(|i| i.pending.len())
            .unwrap_or_default()
    }

    /// The next frame, awaiting one if the buffer is empty.
    ///
    /// `None` means the subscription ended — disposed, cancelled, closed, or retirement already
    /// delivered. `cx` is the **acquisition** context: `spec.md:3900` makes it what *"governs watch
    /// lifetime"*, so a cancelled acquisition stops future delivery here rather than in a
    /// supervisor.
    ///
    /// # Why this is a hand-rolled `poll_fn` rather than `tokio::select!`
    ///
    /// `select!` lives behind tokio's `macros` feature, and this crate's dependency list is
    /// `sync` **only** on purpose (see [`crate::committer`]): it spawns nothing and sleeps nowhere.
    /// Two pinned futures and one `poll_fn` keep that true, and the registration order below is the
    /// part that matters — [`Notified::enable`] is called **before** the buffer is inspected, so a
    /// frame pushed concurrently cannot be a lost wakeup.
    ///
    /// [`Notified::enable`]: tokio::sync::futures::Notified::enable
    pub(crate) async fn next(&self, cx: &Cx) -> Option<RawFrame> {
        let mut cancelled = pin!(cx.cancelled());
        loop {
            let mut notified = pin!(self.wake.notified());
            notified.as_mut().enable();
            if let Some(frame) = self.take() {
                return Some(frame);
            }
            if self.end().is_some() {
                return None;
            }
            let woken = core::future::poll_fn(|task| {
                if cancelled.as_mut().poll(task).is_ready() {
                    return Poll::Ready(false);
                }
                if notified.as_mut().poll(task).is_ready() {
                    return Poll::Ready(true);
                }
                Poll::Pending
            })
            .await;
            if !woken {
                self.terminate(WatchEnd::Cancelled);
                return None;
            }
        }
    }
}

/// Every live document subscription, keyed by the incarnation it is bound to.
///
/// [`Weak`] rather than `Arc`, which is what makes [`crate::Disposer`]'s *"a `Weak` slot plus
/// `Drop`"* true from both ends: the registry does not keep a dropped consumer's subscription alive,
/// and a consumer's disposer does not keep the registry's entry alive.
#[derive(Debug, Default)]
pub(crate) struct Subscriptions {
    by_document: RwLock<HashMap<DocumentId, Vec<Weak<DocSlot>>>>,
}

impl Subscriptions {
    /// Register one slot against its incarnation. **Before** its baseline is read — see this
    /// module's ordering table.
    pub(crate) fn register(&self, slot: &Arc<DocSlot>) {
        if let Ok(mut by_document) = self.by_document.write() {
            let list = by_document.entry(slot.document()).or_default();
            list.retain(|weak| weak.strong_count() > 0);
            list.push(Arc::downgrade(slot));
        }
    }

    /// Buffer one committed change into every live subscription on its incarnation.
    ///
    /// `revision` is the document's tracker revision **after** adoption, read in one acquisition with
    /// the fan-out so the watermark compares two revisions and not two moments. `None` means the
    /// incarnation left the authority, which only a retirement does.
    pub(crate) fn deliver(
        &self,
        change: &crate::Change,
        revision: Option<DocRevision>,
        context: &FrameContext,
    ) {
        let Ok(mut by_document) = self.by_document.write() else {
            return;
        };
        let Some(list) = by_document.get_mut(&change.document()) else {
            return;
        };
        list.retain(|weak| weak.strong_count() > 0);
        if list.is_empty() {
            by_document.remove(&change.document());
            return;
        }
        let frame = if change.retired() {
            RawFrame {
                value: None,
                ops: change.ops().clone(),
                context: context.clone(),
                revision: None,
            }
        } else {
            RawFrame {
                value: Some(change.value().clone()),
                ops: change.ops().clone(),
                context: context.clone(),
                revision,
            }
        };
        for weak in list.iter() {
            if let Some(slot) = weak.upgrade() {
                slot.push(frame.clone());
            }
        }
    }

    /// End every subscription, because the Session is closing.
    ///
    /// `spec.md:1531`: *"the Harness stops watches and signals task invocations there."* Here the
    /// Session does it itself, because it is the thing that knows: a watch whose Session has closed
    /// can never receive another committed frame, so leaving it open would be leaving a consumer
    /// waiting on something that cannot happen.
    pub(crate) fn close_all(&self) {
        if let Ok(mut by_document) = self.by_document.write() {
            for (_, list) in by_document.drain() {
                for weak in list {
                    if let Some(slot) = weak.upgrade() {
                        slot.terminate(WatchEnd::SessionClosed);
                    }
                }
            }
        }
    }

    /// How many incarnations have at least one registered subscription.
    #[cfg(test)]
    pub(crate) fn documents(&self) -> usize {
        self.by_document.read().map(|m| m.len()).unwrap_or_default()
    }
}
