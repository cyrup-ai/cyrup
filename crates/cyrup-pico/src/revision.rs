//! A published revision, a delivered frame, and the frame's context.
//!
//! PICO5-PLAN S6's three value types. All three are about the same sentence —
//! `spec.md:4528-4530`'s *"every published revision is immutable for all time"* — approached from the
//! consumer's side rather than the authority's.
//!
//! # Why [`Revision<T>`] is typed at all, when the value is already a [`DocRoot`]
//!
//! `spec.md:1252` types a document state as `Readonly<T> | null` and `spec.md:1448` makes the typed
//! view **access-driven**: the caller supplies the token, so the caller's `T` is what the value is
//! read back as. The phantom parameter carries that `T` from the acquisition to every delivered
//! frame, so a `DocState<LiveDoc>` cannot deliver a `Revision<JobOutput>` by accident — while the
//! value itself stays one shared `Arc`, because [`Revision::typed`] is a projection the consumer asks
//! for and not something the delivery path pays for.
//!
//! # Why [`Revision::typed`] is fallible and what its failure means
//!
//! `spec.md:1063-1065` is explicit that *"the persisted record, not the token, is the authority on
//! scope and history"*, and ADR-0030 §2.3 keeps that a **check** rather than a type: extension code
//! reloads independently of its data. So a document whose committed value does not fit the `T` the
//! caller named is **one broken document**, reported as [`BrokenDocument`] — not an
//! [`crate::UncertainCommit`], not a failed open, and not a reason the Session stops working. That
//! split is what `src/tests/observation.rs`'s
//! `a_broken_typed_projection_is_one_document_and_not_a_session_failure` pins.
//!
//! [`Revision::raw`] is the half that never fails, and it is the half the *kernel* uses: nothing in
//! the delivery path calls `typed()`, so a document that one consumer cannot parse still reaches
//! every other consumer.
//!
//! # Why [`FrameContext`] has no token field
//!
//! `spec.md:3889`: *"The selected commit Context's values are preserved without inheriting the
//! producer's cancellation."* Upstream states that of a value; here it is the shape of the type. A
//! delivered frame carries a [`FrameContext`], and a `FrameContext` has no cancellation field, no
//! `token()`, no `is_cancelled()` and no `From<Cx>` — so a watch callback that tried to inherit the
//! commit's cancellation has no expression to write, which is what
//! `tests/compile-fail/a_frame_context_cannot_inherit_the_producers_cancellation.rs` proves.
//!
//! This is ADR-0030 **open question 6** standing open: cyrup has no value-carrying context, so
//! [`FrameContext::for_commit`] is the one place a future one would be projected, and today it
//! produces an empty one. The *values* half is therefore provisional; the *no token* half is not,
//! because it is the half the guarantee rests on.

use core::marker::PhantomData;
use std::sync::Arc;

use cyrup_pico_doc::{DocMap, DocRoot, DocValue, OpBatch};
use cyrup_pico_store::{Cx, DocumentId};
use serde::de::DeserializeOwned;

/// One committed, shareable, immutable document revision, tagged with the type it was acquired as.
///
/// **There is no `&mut` path out of this type, and that is the whole of
/// `G-DOC-SOURCE-NO-MUTABLE-OBJECT` on the observation side.** [`Revision::raw`] hands out a shared
/// reference to a [`DocRoot`], which is an `Arc` over a type with no interior-mutable variant
/// (ADR-0030 F4), so `spec.md:1251-1253`'s *"mutation of it or any retained descendant is
/// unsupported"* needs no sentence here: there is no method to call.
/// `tests/compile-fail/a_published_revision_has_no_mutable_path.rs` is the canary PICO5-PLAN S6 asks
/// for by name.
#[derive(Debug)]
pub struct Revision<T> {
    document: DocumentId,
    root: DocRoot,
    /// `fn() -> T` rather than `T`, for ADR-0030 F6 §A's reason: the parameter is a tag, so it must
    /// not make this type invariant in `T` or inherit `T`'s auto traits.
    _t: PhantomData<fn() -> T>,
}

impl<T> Clone for Revision<T> {
    /// Hand-written, because `derive(Clone)` would require `T: Clone` for a parameter that is only a
    /// tag. Cloning is an `Arc` refcount bump: a revision is shared, never copied.
    fn clone(&self) -> Self {
        Self {
            document: self.document,
            root: self.root.clone(),
            _t: PhantomData,
        }
    }
}

impl<T> Revision<T> {
    /// Tag one committed root with the type it was acquired as.
    ///
    /// Crate-private: a revision exists because a commit published one. There is no public
    /// constructor, so a `Revision` cannot be fabricated and handed to a consumer as though it were
    /// committed state — the same reasoning as [`crate::Publication`]'s, one step further out.
    pub(crate) const fn new(document: DocumentId, root: DocRoot) -> Self {
        Self {
            document,
            root,
            _t: PhantomData,
        }
    }

    /// The incarnation this revision belongs to.
    #[must_use]
    pub const fn document(&self) -> DocumentId {
        self.document
    }

    /// The committed value, shared and immutable.
    ///
    /// The infallible half. Returns `&DocRoot` rather than PICO5-PLAN S6's literal
    /// `&Arc<DocRoot>` — see this crate's `lib.rs` for why, since `DocRoot` already *is*
    /// `Arc<DocMap>`.
    #[must_use]
    pub const fn raw(&self) -> &DocRoot {
        &self.root
    }

    /// The committed value, as an owned shared handle.
    #[must_use]
    pub fn into_raw(self) -> DocRoot {
        self.root
    }

    /// The typed projection.
    ///
    /// # Errors
    ///
    /// [`BrokenDocument`] when the committed value does not fit `T`. That is one document's problem:
    /// the revision is still readable through [`Revision::raw`], every other subscriber is
    /// unaffected, and the Session is not poisoned.
    pub fn typed(&self) -> Result<T, BrokenDocument>
    where
        T: DeserializeOwned,
    {
        // The projection bridge. `serde_json::to_value` is forbidden on the document **write** path
        // (ADR-0030 §4: its `serialize_f64` turns a `NaN` into a silent `null`), and this is the
        // other direction: a `DocValue::Num` is already a finite `serde_json::Number`, so the one
        // lossy case cannot arise from a value that exists. Keeping the codec out of
        // `BrokenDocument` is deliberate — replacing this bridge with a `Deserializer` written
        // directly over `&DocValue` must not be a semver event.
        let bridge = serde_json::to_value(&self.root).map_err(|e| BrokenDocument {
            document: self.document,
            detail: e.to_string(),
        })?;
        serde_json::from_value(bridge).map_err(|e| BrokenDocument {
            document: self.document,
            detail: e.to_string(),
        })
    }
}

/// One document's committed value does not fit the type it was acquired as.
///
/// Named for what it is. `spec.md:4352-4360`'s channel split says a legitimately absent record is
/// never reported as damaged data; this is the third channel neither of those is — the data is
/// present and intact, and the *caller's* type is what disagrees with it.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("document {document}'s committed value does not fit the type it was acquired as: {detail}")]
pub struct BrokenDocument {
    /// Which incarnation.
    pub document: DocumentId,
    /// What the projection said, as text. Deliberately not a `serde_json::Error`: see
    /// [`Revision::typed`].
    pub detail: String,
}

/// A delivered frame's context: the commit's **values**, and nothing that cancels.
///
/// Cheap to clone (one `Arc`), and `Default` is deliberately **not** derived — a context with no
/// values is [`FrameContext::empty`], which says so.
#[derive(Clone, Debug)]
pub struct FrameContext {
    /// The one field. There is no `CancellationToken` beside it, and
    /// `tests/compile-fail/a_frame_context_cannot_inherit_the_producers_cancellation.rs` is what
    /// keeps it that way.
    values: Arc<DocMap>,
}

impl FrameContext {
    /// A context carrying no values.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            values: Arc::new(DocMap::new()),
        }
    }

    /// A context carrying `values`.
    ///
    /// Taken by value: the map is moved in and shared from then on, so a host cannot mutate what a
    /// frame already carries.
    #[must_use]
    pub fn from_values(values: DocMap) -> Self {
        Self {
            values: Arc::new(values),
        }
    }

    /// One value by name.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&DocValue> {
        self.values.get(key)
    }

    /// How many values.
    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Whether the context carries nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Project a commit's context into the frame context its publications carry.
    ///
    /// **The single projection site**, and the whole of ADR-0030 open question 6 in one function: a
    /// [`Cx`] carries cancellation and nothing else today, so there is nothing to copy and this
    /// returns [`FrameContext::empty`]. When cyrup grows a value-carrying context, its values are
    /// read **here** and the token still has nowhere to go, because [`FrameContext`] has no field
    /// for one. That is why this is a named projection rather than a `From<Cx>` impl: a conversion
    /// would read as inheritance, and inheritance is the thing the type forbids.
    pub(crate) fn for_commit(_cx: &Cx) -> Self {
        Self::empty()
    }
}

/// One delivery to a document-state or watch consumer.
///
/// Two variants rather than a nullable value, which is ADR-0030's rule applied to
/// `spec.md:3796`'s `Readonly<T> | null`: a [`DocRoot`] is a JSON object by construction, so
/// upstream's terminal `[["r", null]]` root replacement has no spelling here. **A `CYRUP-DELTA`:**
/// the same guarantee — *"if the state remains exposed, consumers see `null`, never stale state"*
/// (`spec.md:3827-3829`) — with retirement as a named variant a consumer must match rather than a
/// null root it could mistake for a value.
#[derive(Debug)]
pub enum Frame<T> {
    /// One exact committed revision, with the exact operation batch that produced it.
    Revision {
        /// The committed value.
        value: Revision<T>,
        /// The operations that produced it, in order. Empty for an overflow replacement's
        /// predecessor-free form — see [`crate::MAX_PENDING_FRAMES`].
        ops: OpBatch,
        /// That commit's context values.
        context: FrameContext,
    },
    /// The incarnation was retired. Terminal: no later frame follows on this subscription.
    Retired {
        /// The retiring commit's context values.
        context: FrameContext,
    },
}

impl<T> Frame<T> {
    /// The revision this frame delivers, or `None` for a retirement.
    #[must_use]
    pub const fn value(&self) -> Option<&Revision<T>> {
        match self {
            Self::Revision { value, .. } => Some(value),
            Self::Retired { .. } => None,
        }
    }

    /// The frame's context values.
    #[must_use]
    pub const fn context(&self) -> &FrameContext {
        match self {
            Self::Revision { context, .. } | Self::Retired { context } => context,
        }
    }

    /// Whether this frame ends the subscription.
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Retired { .. })
    }
}
