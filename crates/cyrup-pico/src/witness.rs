//! ADR-0030 F5's witness chain: `commit` → `adopt` → `publish`, each step consuming the proof the
//! step before it produced.
//!
//! Upstream's guarantee is a sentence — *"there is only one call site, placed after the commit
//! resolved"* (`session.ts` `#runCommit`) — and `spec.md:4591` lists a volatile publication path as a
//! **non-goal**, which is the admission that nothing but that one call site enforces invariants 2
//! and 3. The failure mode F5 names is not exotic: the live view wants a tool result rendered inside
//! the 100–200 ms throttle window, so somebody adds `publish_provisional(..)`, and now an observer
//! renders state a crash erases.
//!
//! Here publishing before durability has **no spelling**. [`Durable`] is minted at exactly one
//! place — the kernel's single [`Storage::commit`] call site in [`crate::tx`] — and is consumed by
//! [`Tx::adopt`], which is the only producer of a [`Publication`], which is consumed by
//! [`DocIndex`]-backed [`crate::session::SessionShared::publish`]. Each witness is moved, so a
//! publication cannot be replayed either.
//!
//! [`Storage::commit`]: cyrup_pico_store::Storage::commit
//! [`Tx::adopt`]: crate::Tx::adopt
//! [`DocIndex`]: crate::docs::DocIndex

use std::sync::Arc;

use cyrup_pico_doc::{DocRoot, OpBatch};
use cyrup_pico_store::{DocumentAddress, DocumentId, DocumentPoint, Seq};

/// Proof that storage reported one batch committed at `seq`.
///
/// **Crate-private, and that is the half ADR-0030 §8's third load-bearing reason is about:**
/// `Storage` is implemented *outside* this crate, so a backend cannot be made to hand back an
/// unforgeable witness — it returns a [`Seq`], and the kernel's one call site turns that `Seq` into
/// this. The witness therefore orders **kernel-internal** steps, which is where the mistake would be
/// made, because the kernel is what runs adoption and publication.
///
/// No `Clone`, no `Default`, no serde in either direction. The `_seal: ()` field is what makes a
/// struct literal outside this module a compile error, and is why `derive(Deserialize)` must never be
/// added: ADR-0030 §10's serde table lists this type as **no** in both columns.
///
/// # What it does and does not claim
///
/// It claims *storage said committed*. Under
/// [`Durability::ProcessCrash`](cyrup_pico_store::Seq) — the weaker of the JSONL backend's two tiers
/// — that is page-cache only (`spec.md:4420`). That half is the stated envelope, not a lie the
/// witness tells.
#[derive(Debug)]
pub(crate) struct Durable {
    seq: Seq,
    _seal: (),
}

impl Durable {
    /// Mint the witness. **One call site**, in `Tx::store`, immediately after
    /// `Storage::commit` returned `Ok`.
    pub(crate) const fn new(seq: Seq) -> Self {
        Self { seq, _seal: () }
    }

    /// The sequence storage allocated.
    pub(crate) const fn seq(&self) -> Seq {
        self.seq
    }
}

/// One committed document change, as an observer sees it.
///
/// Every field is already-immutable: a [`DocRoot`] is an `Arc` over a type with no interior-mutable
/// variant (ADR-0030 F4), so *"commit observers may only capture immutable state"*
/// (`spec.md:1527-1533`) is a property of what they are handed rather than a rule they are asked to
/// follow.
#[derive(Clone, Debug)]
pub struct Change {
    document: DocumentId,
    address: DocumentAddress,
    ops: OpBatch,
    value: DocRoot,
    created: bool,
    retired: bool,
}

impl Change {
    /// Assemble a change during preparation — **before** the storage commit, which is what leaves
    /// adoption with nothing to allocate (ADR-0030 §2.3, `spec.md:1305-1310`).
    pub(crate) const fn new(
        document: DocumentId,
        address: DocumentAddress,
        ops: OpBatch,
        value: DocRoot,
        created: bool,
        retired: bool,
    ) -> Self {
        Self {
            document,
            address,
            ops,
            value,
            created,
            retired,
        }
    }

    /// The incarnation this change wrote.
    #[must_use]
    pub const fn document(&self) -> DocumentId {
        self.document
    }

    /// Its logical address.
    #[must_use]
    pub const fn address(&self) -> &DocumentAddress {
        &self.address
    }

    /// The operations that produced the new value, in order.
    #[must_use]
    pub const fn ops(&self) -> &OpBatch {
        &self.ops
    }

    /// The committed value. Shareable and immutable for all time (`spec.md:4528-4530`).
    #[must_use]
    pub const fn value(&self) -> &DocRoot {
        &self.value
    }

    /// Whether this commit created the incarnation.
    #[must_use]
    pub const fn created(&self) -> bool {
        self.created
    }

    /// Whether this commit retired it.
    #[must_use]
    pub const fn retired(&self) -> bool {
        self.retired
    }
}

/// One storage-backed document **copy**, as an observer sees it (PICO5-PLAN S9).
///
/// # Why this is a second type and not a [`Change`] with an empty value
///
/// `spec.md:1502-1504`: *"Definition-free copies publish explicit `document.copy` metadata rather than
/// a value. That metadata announces Storage-backed initial state and **is never interpreted as a
/// document value**."* A fork that nothing in the creating transaction touched has no value in Session
/// memory *at all* — storage materialised the source and wrote the child's base, and the Session never
/// saw it (`spec.md:1499`: *"an unaccessed copy retains only its descriptor in Session memory"*).
///
/// So the last sentence is not a rule an observer is asked to follow: this type has **no**
/// `value()`, no `ops()` and no [`DocRoot`] field, so interpreting a copy as a value has no
/// expression. `tests/compile-fail/a_copied_document_has_no_value.rs` is the canary, and it is why
/// [`Publication::copies`] is a separate list rather than a flag on [`Change`] — a flag would leave
/// the value field there to be read by a consumer that forgot to check it, which is the whole failure.
///
/// A copy a typed acquisition *did* touch is not here: it was replaced by an ordinary create and
/// appears in [`Publication::changes`] with its value, exactly as `spec.md:1500-1502` says.
#[derive(Clone, PartialEq, Debug)]
pub struct Copied {
    document: DocumentId,
    address: DocumentAddress,
    source: DocumentId,
    at: DocumentPoint,
    retired: bool,
}

impl Copied {
    /// Assemble the announcement during preparation, beside [`Change::new`].
    pub(crate) const fn new(
        document: DocumentId,
        address: DocumentAddress,
        source: DocumentId,
        at: DocumentPoint,
        retired: bool,
    ) -> Self {
        Self {
            document,
            address,
            source,
            at,
            retired,
        }
    }

    /// The child incarnation this commit created.
    #[must_use]
    pub const fn document(&self) -> DocumentId {
        self.document
    }

    /// Its logical address in the child conversation.
    #[must_use]
    pub const fn address(&self) -> &DocumentAddress {
        &self.address
    }

    /// The source incarnation storage read.
    #[must_use]
    pub const fn source(&self) -> DocumentId {
        self.source
    }

    /// The point the source was read at: the fork commit for `asOf`,
    /// [`DocumentPoint::Current`] for `current`.
    #[must_use]
    pub const fn at(&self) -> DocumentPoint {
        self.at
    }

    /// Whether the same commit also retired the child.
    #[must_use]
    pub const fn retired(&self) -> bool {
        self.retired
    }
}

/// Proof that one commit is durable **and** adopted, and therefore publishable.
///
/// # Why this one is `pub` where [`Durable`] is `pub(crate)`
///
/// ADR-0030 §10 writes `pub(crate) struct Publication`, and in the same block writes
/// `trait CommitObserver { fn observe(&self, p: &Publication<'_>); }` as a **public** trait. Those
/// two cannot both hold: a `pub(crate)` type in a public trait's method signature is `E0446`. The
/// guarantee F5 rests on is not the visibility of the *name*, it is that nothing outside this module
/// can **construct** one — so the name is public and every construction path is closed:
///
/// * private fields plus `_seal: ()`, so a struct literal outside this module does not compile;
/// * no `Clone`, so a publication cannot be replayed;
/// * no `Default`, no public constructor;
/// * no `Serialize` and **no `Deserialize`**, per ADR-0030 §10's serde table — a `Deserialize` here
///   would let a log line become a publication, which is exactly the path `_seal` closes.
///
/// A second emitter — the optimistic channel invariant 3 and `spec.md:4591` forbid — has no
/// `Publication` to pass and cannot make one. The discrepancy with §10 is recorded rather than
/// papered over.
#[derive(Debug)]
pub struct Publication {
    seq: Seq,
    changes: Arc<[Change]>,
    copies: Arc<[Copied]>,
    _seal: (),
}

impl Publication {
    /// Mint the publication. **One call site**, in [`crate::tx::Tx::adopt`], after the pointer swaps
    /// succeeded.
    pub(crate) const fn new(seq: Seq, changes: Arc<[Change]>, copies: Arc<[Copied]>) -> Self {
        Self {
            seq,
            changes,
            copies,
            _seal: (),
        }
    }

    /// The commit sequence this publication reports.
    #[must_use]
    pub const fn seq(&self) -> Seq {
        self.seq
    }

    /// The document changes, in assembly order.
    #[must_use]
    pub fn changes(&self) -> &[Change] {
        &self.changes
    }

    /// The storage-backed copies this commit created, in assembly order (PICO5-PLAN S9).
    ///
    /// A **separate** list from [`Publication::changes`], because `spec.md:1502-1504` requires copy
    /// metadata to be announced and never interpreted as a document value — see [`Copied`] for why
    /// that is a type and not a rule.
    #[must_use]
    pub fn copies(&self) -> &[Copied] {
        &self.copies
    }
}

/// Adoption failed **after** storage already committed.
///
/// This is ADR-0030 F1's *dropped second poison path*, and it carries the `seq` because
/// [`UncertainKind::AdoptionFailedAfterCommit`] needs it: durable state is known-good and known
/// *ahead* of memory, so the host reopens and has nothing to reconcile. `#runCommit`'s own upstream
/// comment is the whole argument — *"Storage already committed; a failed adoption leaves memory
/// behind durable state."*
///
/// [`UncertainKind::AdoptionFailedAfterCommit`]: crate::UncertainKind::AdoptionFailedAfterCommit
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("storage committed at sequence {seq} but in-memory adoption failed: {cause}")]
pub struct AdoptionFailed {
    /// The sequence storage committed at.
    pub seq: Seq,
    /// Which pointer swap could not be made.
    pub cause: AdoptionCause,
}

/// Why a pointer swap could not be made.
///
/// Both arms are *"this cannot happen while the Session is the sole committer"* — which is the point:
/// they are the states whose impossibility is a property of F1's ownership, not of this function, so
/// they are reported rather than asserted. `spec.md:1305-1310` forbids adoption from doing anything
/// that can fail, and these two are the residue of that: a check, not work.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionCause {
    /// The authority revision moved while this change was open, so the preparation is stale.
    StalePreparation {
        /// The revision the preparation was made against.
        prepared_against: u64,
        /// The revision the authority is at now.
        authority_at: u64,
    },
    /// The incarnation this change was prepared for is no longer in the index.
    DocumentVanished(DocumentId),
    /// Adoption was reached with nothing prepared.
    ///
    /// Unreachable by sequence: [`Tx::prepare`](crate::Tx) sets the staged swaps before
    /// [`Tx::adopt`](crate::Tx) can be reached, and the only caller runs them in that order inside
    /// one function. The arm exists because the alternative is `unwrap` on the held line.
    NothingStaged,
    /// A panic unwound through an earlier swap and left the authority half-written.
    ///
    /// The one arm here that is **not** impossible by construction, and it is why this enum is not
    /// two variants. F1's *"a panic in a line observer still unwinds — `Fn -> ()` forbids `Result`,
    /// not `panic!`"* is the same hazard one step earlier: a panic anywhere inside the swap window
    /// leaves memory in a state nobody can describe, and continuing would prepare the next change
    /// against that state. So it is fatal, exactly like the other two.
    AuthorityPoisoned,
}

impl core::fmt::Display for AdoptionCause {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::StalePreparation {
                prepared_against,
                authority_at,
            } => write!(
                f,
                "a change prepared against revision {prepared_against} met authority revision {authority_at}"
            ),
            Self::DocumentVanished(id) => write!(f, "document {id} left the index"),
            Self::NothingStaged => f.write_str("adoption was reached with nothing prepared"),
            Self::AuthorityPoisoned => {
                f.write_str("a panic unwound through an earlier swap in this adoption")
            }
        }
    }
}
