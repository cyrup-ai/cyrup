//! `documentState()`: one committed baseline and one buffering registration, as **one value from one
//! constructor**.
//!
//! # What the single constructor forecloses
//!
//! `spec.md:1515-1519` states the obligation twice, the second time as the thing that goes wrong:
//! *"Document states, watches, and mounted views hydrate by capturing their baseline and subscription
//! atomically on the Session line … A mounted aggregate must acquire all of its document baselines and
//! commit subscription in one Session-line operation **so it never exposes a mixture from one
//! commit**."*
//!
//! Upstream can only say that, because `documentState()` is one `await` and nothing stops a caller
//! from snapshotting, awaiting something else, and subscribing afterwards. Here the two halves are not
//! two calls: [`Session::attach_doc_state`](crate::Session::attach_doc_state) returns one
//! [`Attachment`], which **is** the baseline and **is** the live registration, and there is no API
//! that produces either one alone. A caller cannot interleave anything between them because there is
//! no between.
//!
//! # Why activation consumes the attachment
//!
//! [`Attachment::activate`] takes `self`. Before it, frames buffer and the baseline is readable;
//! after it, the [`DocState`] is the only handle and the attachment is gone. Activating twice is
//! `E0382` — `tests/compile-fail/an_attachment_cannot_be_activated_twice.rs` — which is what makes
//! *"one concrete incarnation, bound once"* (`spec.md:3826`) a property of the type rather than a
//! sentence. The two-step shape is not ceremony: it is the window in which a consumer initialises
//! itself from the baseline **while already buffering**, which is the only way to hydrate without a
//! gap.
//!
//! # Where this runs
//!
//! Not on the mutation line. `spec.md:1524`: *"Document-state subscribers and watch listeners still
//! run later, off the line."* A [`DocState`]'s consumer drains frames with
//! [`DocState::next_frame`] on its own task, and the line's only contact with it is
//! [`crate::slot::DocSlot::push`] — a buffer append under a mutex, no user code. That is the second of
//! the two capabilities [`crate::observe`] tabulates, and it is a different type from the first rather
//! than the same type with a flag.

use core::marker::PhantomData;
use std::sync::Arc;

use cyrup_pico_store::{Cx, DocumentId};

use crate::observe::Disposer;
use crate::revision::{Frame, Revision};
use crate::slot::{DocSlot, RawFrame, WatchEnd};

/// Tag one buffered frame with the type its subscription was acquired as.
pub(crate) fn typed_frame<T>(document: DocumentId, raw: RawFrame) -> Frame<T> {
    match raw.value {
        Some(root) => Frame::Revision {
            value: Revision::new(document, root),
            ops: raw.ops,
            context: raw.context,
        },
        None => Frame::Retired {
            context: raw.context,
        },
    }
}

/// A captured baseline **and** a live buffering registration, inseparably.
///
/// Dropping one without activating it disposes the registration, because the [`Disposer`] inside it is
/// dropped with it — so an abandoned acquisition does not leave the line fanning frames into a buffer
/// nobody drains.
#[derive(Debug)]
pub struct Attachment<T> {
    slot: Arc<DocSlot>,
    disposer: Disposer,
    cx: Cx,
    _t: PhantomData<fn() -> T>,
}

impl<T> Attachment<T> {
    /// Assemble the one value. **One call site**, in
    /// [`Session::attach_doc_state`](crate::Session::attach_doc_state), after the slot is registered
    /// and armed.
    pub(crate) const fn new(slot: Arc<DocSlot>, disposer: Disposer, cx: Cx) -> Self {
        Self {
            slot,
            disposer,
            cx,
            _t: PhantomData,
        }
    }

    /// The incarnation this attachment is bound to, for its whole life.
    #[must_use]
    pub fn document(&self) -> DocumentId {
        self.slot.document()
    }

    /// The captured baseline: the exact shareable immutable revision acquisition saw.
    ///
    /// `spec.md:3821-3823`: *"The returned state is already hydrated with the exact shareable
    /// immutable revision."* `None` cannot happen before activation — a retirement is delivered as a
    /// frame, not by erasing the baseline — and is `None` only if the Session closed in between.
    #[must_use]
    pub fn baseline(&self) -> Option<Revision<T>> {
        self.slot
            .current()
            .map(|root| Revision::new(self.slot.document(), root))
    }

    /// How many committed frames arrived since acquisition and are still undelivered.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.slot.pending()
    }

    /// Hand the baseline and the buffer over to a live state. **Consuming.**
    #[must_use]
    pub fn activate(self) -> DocState<T> {
        DocState {
            slot: self.slot,
            disposer: self.disposer,
            cx: self.cx,
            _t: PhantomData,
        }
    }
}

/// A disposable, read-only view of one committed incarnation's revision stream.
///
/// `spec.md:1252`: *"`documentState()` returns a disposable read-only Chord state bound to one
/// committed incarnation."* Read-only is not a convention here: every value out of this type is a
/// [`Revision<T>`], which has no `&mut` path at all
/// (`tests/compile-fail/a_published_revision_has_no_mutable_path.rs`), and there is no method that
/// writes. `spec.md:3824`'s *"Pico remains the sole document mutator"* is therefore
/// `G-DOC-SOURCE-NO-MUTABLE-OBJECT` restated: the only way a document changes is a draft inside a
/// mutation transaction, and this type is not one.
#[derive(Debug)]
pub struct DocState<T> {
    slot: Arc<DocSlot>,
    disposer: Disposer,
    cx: Cx,
    _t: PhantomData<fn() -> T>,
}

impl<T> DocState<T> {
    /// The incarnation this state is bound to.
    #[must_use]
    pub fn document(&self) -> DocumentId {
        self.slot.document()
    }

    /// The revision this state currently exposes.
    ///
    /// Advances as frames are taken, and is `None` once retirement has been delivered —
    /// `spec.md:3827-3829`'s *"if the state remains exposed, consumers see `null`, never stale
    /// state"*, with `null` spelled as the absence it is.
    #[must_use]
    pub fn value(&self) -> Option<Revision<T>> {
        self.slot
            .current()
            .map(|root| Revision::new(self.slot.document(), root))
    }

    /// Take the next buffered frame without waiting.
    #[must_use]
    pub fn try_next_frame(&self) -> Option<Frame<T>> {
        self.slot
            .take()
            .map(|raw| typed_frame(self.slot.document(), raw))
    }

    /// Await the next committed frame.
    ///
    /// `None` when the subscription has ended: disposed, the acquisition [`Cx`] cancelled, the Session
    /// closed, or the bound incarnation retired and its terminal frame already delivered.
    /// [`DocState::end`] says which.
    pub async fn next_frame(&self) -> Option<Frame<T>> {
        self.slot
            .next(&self.cx)
            .await
            .map(|raw| typed_frame(self.slot.document(), raw))
    }

    /// How many committed frames are buffered and undelivered.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.slot.pending()
    }

    /// Why the subscription ended, or `None` while it is live.
    #[must_use]
    pub fn end(&self) -> Option<WatchEnd> {
        self.slot.end()
    }

    /// Unregister this observation. Idempotent, and also done on drop.
    ///
    /// `spec.md:3823-3824`: *"Disposing it unregisters **only that observation**."*
    pub fn dispose(&self) {
        self.disposer.dispose();
    }
}
