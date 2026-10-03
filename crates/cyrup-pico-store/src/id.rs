//! Kind-tagged identity (ADR-0030 F6 §A, §2.2 rows 1 and 5-6; `spec.md:238-249, 4274-4282`).
//!
//! # Why `NonZeroU64` and `PhantomData<fn() -> K>`
//!
//! `NonZeroU64` because `ROOT_CONVERSATION_ID` is 1 and 0 is never a valid id
//! (`spec.md:249`), so *"id zero"* is unrepresentable rather than checked, and `Option<Id<K>>` is
//! niche-packed to eight bytes for free — which matters because `Lifetime::retired_at` is one.
//!
//! `PhantomData<fn() -> K>` rather than `PhantomData<K>` so that `Id<K>` is **covariant** in `K`
//! and is `Send`/`Sync`/`Unpin` regardless of what `K` is. With `PhantomData<K>` an `Id<K>` would
//! inherit auto traits from a type that is pure phantom and never exists at runtime. ADR-0030 F6 §A
//! says to *"get that right once"*; a static assertion in this crate's test module pins it, so a
//! later edit to this line fails the build rather than quietly making `Id<K>` unsendable.
//!
//! # What this does not buy
//!
//! Stated here because the natural reading of `spec.md:4274` is wrong in one direction and right in
//! the other. **Lifted in-process:** with no `From<u64>` and no public constructor, a number minted
//! as an entry cannot become a `TaskId` without crossing a decode boundary, so the cross-table half
//! of id collision is a compile error here, not a runtime check. **Not lifted:** the number carries
//! no tag, so a record read from the task index deserialises its `id` as `Id<Task>` *because that
//! is the field's type*. A corrupted file that files an entry id under tasks is accepted by the
//! field and must still be caught by the backend's ownership check against its committed index.
//! Nor does any of this give monotonicity or non-reuse across a reopen, which are
//! `checked`, nor a correct-kind-but-wrong-number id, which nothing can give.

use core::borrow::Borrow;
use core::cmp::Ordering;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::num::NonZeroU64;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod private {
    /// Seals [`super::IdKind`]. Outside this crate the trait cannot be implemented, so the five
    /// kinds are a closed set and `Id<MyType>` does not compile.
    pub trait Sealed {}
}

/// The five id kinds, as a value.
///
/// [`IdKind`] is the compile-time half and cannot be a struct field; this is the runtime half, for
/// diagnostics that must name a kind — `RejectedReason::IdAlreadyOwned { id: RawId, by: IdKindTag }`
/// in S3. Closed, and deliberately not `#[non_exhaustive]`: `spec.md:242-247` names exactly five
/// and adding a sixth is a storage-format event, not an additive change.
///
/// `Serialize` only. It is reachable from `RejectedReason`, which ADR-0030 §10's serde table makes
/// loggable but not constructible from data.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IdKindTag {
    /// A conversation record.
    Conversation,
    /// An entry record.
    Entry,
    /// A task record.
    Task,
    /// A submission record.
    Submission,
    /// A document incarnation record.
    Document,
}

impl IdKindTag {
    /// The lowercase name upstream brands with (`spec.md:243-247`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conversation => "conversation",
            Self::Entry => "entry",
            Self::Task => "task",
            Self::Submission => "submission",
            Self::Document => "document",
        }
    }
}

impl fmt::Display for IdKindTag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The kind parameter of [`Id`]. Sealed: implemented for exactly the five marker types in this
/// module and for nothing else, ever.
pub trait IdKind: private::Sealed + 'static {
    /// This kind, as a value, for diagnostics.
    const KIND: IdKindTag;
}

macro_rules! id_kind {
    ($(#[$attr:meta])* $name:ident => $tag:ident) => {
        $(#[$attr])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name;

        impl private::Sealed for $name {}

        impl IdKind for $name {
            const KIND: IdKindTag = IdKindTag::$tag;
        }
    };
}

id_kind! {
    /// Type-level marker for a conversation id. Never held as a value.
    Conversation => Conversation
}
id_kind! {
    /// Type-level marker for an entry id. Never held as a value.
    Entry => Entry
}
id_kind! {
    /// Type-level marker for a task id. Never held as a value.
    Task => Task
}
id_kind! {
    /// Type-level marker for a submission id. Never held as a value.
    Submission => Submission
}
id_kind! {
    /// Type-level marker for a document-incarnation id. Never held as a value.
    Document => Document
}

/// A durable entity id, drawn from the one global monotone numeric namespace
/// (`spec.md:4282`) and tagged with the kind of record it names.
///
/// There is **no public constructor**. Allocation is `Storage::mint` (S3) and nothing else:
/// no free function, no `Default`, no `From<u64>`, no builder. [`ROOT_CONVERSATION_ID`] is the
/// single exception, and it is an exception the specification itself fixes.
///
/// `Serialize` writes the bare number, because `spec.md:242-245` requires exactly that on the wire.
/// `Deserialize` is hand-written (see [`crate::de`]); a derived impl over the private `NonZeroU64`
/// would accept any nonzero number and therefore buy nothing, but it would also accept a float or
/// a quoted number depending on the format, which is the bypass the hand-written impl closes.
#[repr(transparent)]
pub struct Id<K: IdKind>(NonZeroU64, PhantomData<fn() -> K>);

impl<K: IdKind> Id<K> {
    /// The only non-serde construction path, and it is crate-private on purpose: ADR-0030 F6 §A
    /// puts allocation on the storage handle so that an in-memory fast path or a stale cached
    /// high-water mark cannot reissue an id.
    pub(crate) const fn new(n: NonZeroU64) -> Self {
        Self(n, PhantomData)
    }

    /// This kind, as a value.
    #[must_use]
    pub const fn kind(self) -> IdKindTag {
        K::KIND
    }
}

/// The root conversation (`spec.md:249`: `const ROOT_CONVERSATION_ID = 1 as ConversationId`).
///
/// The one id that is fixed rather than minted, and the reason the inner type is `NonZeroU64`.
pub const ROOT_CONVERSATION_ID: Id<Conversation> = Id::new(NonZeroU64::MIN);

// Every trait below is hand-written rather than derived. `derive` on a generic struct adds a bound
// on the parameter — `#[derive(Clone)]` would produce `impl<K: IdKind + Clone> Clone for Id<K>` —
// and the marker types are phantom, so inheriting anything from them is exactly the mistake
// `PhantomData<fn() -> K>` was chosen to avoid.

impl<K: IdKind> Clone for Id<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: IdKind> Copy for Id<K> {}

impl<K: IdKind> PartialEq for Id<K> {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<K: IdKind> Eq for Id<K> {}

impl<K: IdKind> PartialOrd for Id<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// `Ord` so a backend can key a `BTreeMap<Id<K>, _>` per table — which is what makes the
/// in-table and in-process cross-table halves of id collision unrepresentable rather than
/// checked (ADR-0030 §2.2) — and so an inclusive id range is a range over the key type.
impl<K: IdKind> Ord for Id<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.cmp(&other.0)
    }
}

impl<K: IdKind> Hash for Id<K> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

/// `Borrow<NonZeroU64>` exists for exactly one reason: so a backend can take an **id range** over a
/// `BTreeSet<Id<K>>` or a `BTreeMap<Id<K>, _>` with a bound it does not hold as a typed id.
///
/// `spec.md:4334-4336` requires `scan_entries` to *"page the inclusive ID range in newest-first
/// order"*, and a scan resumes from a cursor whose payload is **backend-private bytes**
/// (`spec.md:4320`) — a bare number, because that is all the payload ever was. Without this impl the
/// only way to honour the bound is a filtered walk of the whole set, which is precisely the
/// degradation ADR-0030 §2.2 prices: *"if they degrade to scans, cost grows with total history."*
/// With it, the bound is an `O(log n)` descent.
///
/// # It is not a construction path, and cannot become one
///
/// `Borrow` hands out a `&NonZeroU64` **of an id that already exists**. Nothing maps a `&NonZeroU64`
/// back to an `Id<K>`: [`Id::new`] is still crate-private and [`RawId`] is still one-way. So the one
/// guarantee this module exists to keep — a number cannot become a typed id outside
/// [`StorageExt::mint`] — is untouched.
///
/// # Why the impl is sound in `Borrow`'s own terms
///
/// `Borrow` requires `Eq`, `Ord` and `Hash` to agree across the two types, because a `BTreeMap` lookup
/// through a borrowed key must find the same entry the owned key would. All three agree here by
/// construction: every one of `Id<K>`'s impls above delegates to `self.0`, which **is** the
/// `NonZeroU64`. A range taken through this `Borrow` therefore selects exactly the ids `Id<K>`'s own
/// ordering would.
///
/// [`StorageExt::mint`]: crate::StorageExt::mint
impl<K: IdKind> Borrow<NonZeroU64> for Id<K> {
    fn borrow(&self) -> &NonZeroU64 {
        &self.0
    }
}

impl<K: IdKind> fmt::Debug for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id<{}>({})", K::KIND, self.0)
    }
}

impl<K: IdKind> fmt::Display for Id<K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}#{}", K::KIND, self.0)
    }
}

impl<K: IdKind> Serialize for Id<K> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.0.get())
    }
}

impl<'de, K: IdKind> Deserialize<'de> for Id<K> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        crate::de::nonzero_u64(d, "a nonzero unsigned integer entity id").map(Self::new)
    }
}

/// An entity id with its kind erased, for a diagnostic that has to name a number whose kind is
/// what is in dispute — `RejectedReason::{IdAlreadyOwned, IdWrittenTwice}` in S3.
///
/// The erasure is **one-way by construction**: `From<Id<K>>` exists, nothing maps a `RawId` back,
/// and there is no `Deserialize`. [`RawId::get`] hands out the number because a log line needs it,
/// and that is not a round trip — rebuilding an [`Id`] still requires [`Id::new`], which is
/// crate-private.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct RawId(NonZeroU64);

impl RawId {
    /// Wrap a number an allocator has just produced.
    ///
    /// This is the **allocator's** constructor and the only reason it is public: ADR-0030 F6 §A puts
    /// minting on the storage handle, and backends live in other crates
    /// (`cyrup-pico-store-jsonl`, and S12's SQLite), so a backend must be able to return the number
    /// it allocated from [`Storage::mint_raw`].
    ///
    /// It is not a way back to an [`Id<K>`]. [`Id::new`] stays crate-private, so the only conversion
    /// from a `RawId` to a typed id is [`StorageExt::mint`], inside this crate, at the one call site
    /// that knows which kind was asked for.
    ///
    /// [`Storage::mint_raw`]: crate::Storage::mint_raw
    /// [`StorageExt::mint`]: crate::StorageExt::mint
    #[must_use]
    pub const fn new(n: NonZeroU64) -> Self {
        Self(n)
    }

    /// The number, for formatting a diagnostic.
    #[must_use]
    pub const fn get(self) -> NonZeroU64 {
        self.0
    }
}

impl<K: IdKind> From<Id<K>> for RawId {
    fn from(id: Id<K>) -> Self {
        Self(id.0)
    }
}

impl fmt::Display for RawId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// `Serialize` only, exactly as ADR-0030 §10's table has `RejectedReason`: a rejection reason must
/// be loggable without becoming a construction path. A `Deserialize` here would let a corrupted log
/// line or an IPC hop turn a number back into an id under a kind of the reader's choosing.
impl Serialize for RawId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.0.get())
    }
}
