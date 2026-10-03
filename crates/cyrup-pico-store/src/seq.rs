//! Commit sequences and document read points (ADR-0030 §2.2 rows 2 and 4; `spec.md:99-100, 4224`).

use core::fmt;
use core::num::NonZeroU64;

use serde::de::{Error as DeError, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A store commit sequence. Strictly increases across the store's whole life, including across a
/// reopen; gaps are permitted (`spec.md:99-100`).
///
/// A separate type from [`crate::Id`] and not interchangeable with one: upstream both are plain
/// `number` and the distinct brand exists, in the specification's own words, *"to prevent an entity
/// ID from being used as a document commit point"* (`spec.md:247`). Here that is a type error.
///
/// **This type does not enforce the ordering.** It cannot: *strictly increasing* is a property of a
/// sequence of values read back across a process boundary, so ADR-0030 §2.2 classifies it `checked`
/// and places the check at the backend's write and recovery boundaries. What the type does is make
/// the comparison well-typed and give [`Lifetime`](crate::Lifetime) one implementation of the
/// half-open test instead of one per read site.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Seq(NonZeroU64);

impl Seq {
    /// The lowest sequence a store can allocate. Zero is not a sequence, for the same reason it is
    /// not an id: it would make "no sequence yet" and "the first commit" the same value.
    pub const FIRST: Self = Self(NonZeroU64::MIN);

    /// A sequence from a nonzero number.
    ///
    /// Public, unlike [`Id::new`](crate::Id), and the asymmetry is deliberate: an id's invariant is
    /// *who allocated it*, which a public constructor would break, while a sequence's invariant is
    /// *ordering against the store's other sequences*, which no constructor can establish and which
    /// the backend checks. Taking `NonZeroU64` rather than `u64` keeps zero out without a `Result`.
    #[must_use]
    pub const fn new(n: NonZeroU64) -> Self {
        Self(n)
    }

    /// The next sequence, or `None` at `u64::MAX`.
    ///
    /// Fallible rather than wrapping: a wrapped sequence is a silently non-increasing one, which
    /// makes every lifetime interval and every delta ordering in the store meaningless. Gaps being
    /// legal means a backend need not use this, only that when it increments by one it does so in
    /// one place.
    #[must_use]
    pub const fn successor(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for Seq {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u64(self.0.get())
    }
}

/// Hand-written, per ADR-0030 §10. The table adds a second line for `Seq` specifically: *"a decoded
/// `Seq` that does not strictly increase is **corruption** and must fail the open, not be
/// normalised"*. That half is the backend's, at the recovery boundary (pi does it at
/// `jsonl/storage.ts:543-546`); this impl owns the per-value half — a sequence is an unsigned
/// integer and nothing else.
impl<'de> Deserialize<'de> for Seq {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        crate::de::nonzero_u64(d, "a nonzero unsigned integer commit sequence").map(Self::new)
    }
}

/// The point a document is read at: one historical commit sequence, or the current state
/// (`spec.md:4224`, `type DocumentPoint = Seq | "current"`).
///
/// A two-variant enum rather than typestate, by `docs/RUST-DESIGN-REVIEW.md:57`'s rule and
/// ADR-0030 §2.2's note that it is *"inspected dynamically against stored values"*: which variant a
/// caller holds is chosen at runtime from a request, and both arms are live at every read site.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum DocumentPoint {
    /// Membership and content as of this commit sequence.
    At(Seq),
    /// Membership and content as of now.
    Current,
}

impl fmt::Display for DocumentPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::At(seq) => seq.fmt(f),
            Self::Current => f.write_str("current"),
        }
    }
}

/// Upstream's wire form exactly: the number, or the string `"current"`.
impl Serialize for DocumentPoint {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::At(seq) => seq.serialize(s),
            Self::Current => s.serialize_str(CURRENT),
        }
    }
}

const CURRENT: &str = "current";

struct DocumentPointVisitor;

impl<'de> Visitor<'de> for DocumentPointVisitor {
    type Value = DocumentPoint;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a nonzero unsigned integer commit sequence or the string \"current\"")
    }

    fn visit_u64<E: DeError>(self, v: u64) -> Result<Self::Value, E> {
        NonZeroU64::new(v)
            .map(|n| DocumentPoint::At(Seq::new(n)))
            .ok_or_else(|| E::invalid_value(Unexpected::Unsigned(v), &self))
    }

    fn visit_str<E: DeError>(self, v: &str) -> Result<Self::Value, E> {
        if v == CURRENT {
            Ok(DocumentPoint::Current)
        } else {
            Err(E::invalid_value(Unexpected::Str(v), &self))
        }
    }
}

/// Hand-written because this is the one type here whose legal wire form is a union, and the derived
/// untagged-enum impl would accept any string as `Current` by falling through to the second arm.
/// Only the exact token `"current"` is that variant; every other string is an error that names
/// itself, and a float, a negative, zero, a boolean and `null` are rejected by the inherited
/// `Visitor` defaults.
impl<'de> Deserialize<'de> for DocumentPoint {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(DocumentPointVisitor)
    }
}
