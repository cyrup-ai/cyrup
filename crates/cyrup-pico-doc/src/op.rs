//! Chord's seven operation tuples (`packages/chord/src/delta/README.md`, *"Operations"*).
//!
//! | Tuple | Meaning | here |
//! |---|---|---|
//! | `["r", value]` | Replace the complete value. | [`Op::ReplaceRoot`] |
//! | `["s", path, value]` | Set an object property or array element. | [`Op::Set`] |
//! | `["d", path]` | Delete an object property or remove an array element. | [`Op::Delete`] |
//! | `["a", path, text]` | Append to a string. | [`Op::AppendStr`] |
//! | `["t", path, count]` | Remove `count` code units from a string's front. | [`Op::TrimStrFront`] |
//! | `["p", path, index, remove, items]` | Splice an array. | [`Op::Splice`] |
//! | `["m", path, permutation]` | Reorder an array: `new[i] = old[permutation[i]]`. | [`Op::Permute`] |
//!
//! Three of the seven get a sharper type than the tuple:
//!
//! * `["r", value]` takes a [`DocRoot`], not a value. A root replacement whose payload is a string
//!   is unrepresentable, which `spec.md:4240`'s `JsonObject` already required and upstream enforced
//!   only by where the operation is produced.
//! * `["t", path, count]` takes a [`TrimLen`], which is nonzero: a zero trim is a no-op the tracker
//!   never emits, and an op batch that is "nonempty" only because it contains no-ops would defeat
//!   `spec.md:1357`'s empty-batch rule. Its **unit is bytes**, settling ADR-0030 §14 open question 7
//!   — see `apply::trim_str_front`, which records the decision and the incompatibility it accepts.
//! * `["m", path, permutation]` takes a [`Permutation`], validated at construction to be a
//!   permutation of `0..n` rather than an arbitrary number array.
//!
//! **Deserialize is hand-written for all of it**, because every one of those constructors
//! establishes an invariant and this is a persisted shape: a derived impl would reconstruct a
//! reserved path segment, a zero trim and a non-permutation straight out of a damaged file, which is
//! exactly the bypass ADR-0030 §10's serde table exists to close.

use core::fmt;
use core::num::NonZeroU32;
use std::sync::Arc;

use serde::de::{Error as DeError, SeqAccess, Visitor};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::path::{Path, Seg, UnsafePathError};
use crate::value::{DocRoot, DocValue};

/// How many **bytes** to remove from a string's front. Nonzero.
///
/// The unit is cyrup's, not Chord's: ADR-0030 §14 open question 7 is settled as byte offsets, with the
/// reasoning and the recorded incompatibility on `apply::trim_str_front`.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TrimLen(NonZeroU32);

impl TrimLen {
    /// A trim length, rejecting zero.
    ///
    /// # Errors
    ///
    /// [`ZeroTrim`] for 0.
    pub const fn parse(n: u32) -> Result<Self, ZeroTrim> {
        match NonZeroU32::new(n) {
            Some(nz) => Ok(Self(nz)),
            None => Err(ZeroTrim),
        }
    }

    /// The count.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// A trim of zero code units, which is a no-op rather than an operation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("a trim of zero bytes is a no-op, not an operation")]
pub struct ZeroTrim;

/// A reordering of an array: `new[i] = old[permutation[i]]`.
///
/// Validated at construction to be a permutation of `0..len`, so "two elements claim the same source"
/// and "an element claims a source that does not exist" are unrepresentable rather than apply-time
/// failures. Whether it matches the *target* array's length is still apply-time, because the array
/// is not in scope here.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Permutation(Arc<[u32]>);

impl Permutation {
    /// A permutation of `0..indices.len()`.
    ///
    /// # Errors
    ///
    /// [`NotAPermutation`] when an index repeats or is out of range.
    pub fn parse(indices: impl Into<Arc<[u32]>>) -> Result<Self, NotAPermutation> {
        let indices: Arc<[u32]> = indices.into();
        let len = indices.len();
        let mut seen = vec![false; len];
        for &i in indices.iter() {
            let at = usize::try_from(i).unwrap_or(usize::MAX);
            match seen.get_mut(at) {
                Some(slot) if !*slot => *slot = true,
                Some(_) => return Err(NotAPermutation::Repeated { index: i }),
                None => return Err(NotAPermutation::OutOfRange { index: i, len }),
            }
        }
        Ok(Self(indices))
    }

    /// The source index for each target position.
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.0
    }

    /// How many elements this permutation reorders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether this permutation reorders nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Why an index array is not a permutation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
pub enum NotAPermutation {
    /// Two target positions claim the same source element.
    #[error("source index {index} appears twice")]
    Repeated {
        /// The repeated index.
        index: u32,
    },
    /// A target position claims a source element that does not exist.
    #[error("source index {index} is out of range for {len} elements")]
    OutOfRange {
        /// The offending index.
        index: u32,
        /// The permutation's length.
        len: usize,
    },
}

/// One operation. The payload is an already-parsed [`DocValue`], which is why upstream's
/// *"`push(valid, invalid)` inserts nothing"* rule is not a rule here but the shape of a function
/// taking a wholly-parsed vector (ADR-0030 F4).
#[derive(Clone, PartialEq, Debug)]
pub enum Op {
    /// `["r", value]` — replace the complete root.
    ReplaceRoot(DocRoot),
    /// `["s", path, value]` — set an object member or an array element.
    Set {
        /// Where.
        path: Path,
        /// What.
        value: DocValue,
    },
    /// `["d", path]` — delete an object member.
    Delete {
        /// Where.
        path: Path,
    },
    /// `["a", path, text]` — append to a string.
    AppendStr {
        /// Where.
        path: Path,
        /// What to append.
        text: Arc<str>,
    },
    /// `["t", path, count]` — remove bytes from a string's front.
    TrimStrFront {
        /// Where.
        path: Path,
        /// How many bytes.
        count: TrimLen,
    },
    /// `["p", path, index, remove, items]` — splice an array.
    Splice {
        /// Which array.
        path: Path,
        /// Where in it.
        at: u32,
        /// How many elements to remove.
        remove: u32,
        /// What to insert in their place.
        items: Arc<Vec<DocValue>>,
    },
    /// `["m", path, permutation]` — reorder an array.
    Permute {
        /// Which array.
        path: Path,
        /// The reordering.
        perm: Permutation,
    },
}

impl Op {
    /// The wire tag this operation serializes as.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::ReplaceRoot(_) => "r",
            Self::Set { .. } => "s",
            Self::Delete { .. } => "d",
            Self::AppendStr { .. } => "a",
            Self::TrimStrFront { .. } => "t",
            Self::Splice { .. } => "p",
            Self::Permute { .. } => "m",
        }
    }

    /// Whether this operation replaces the whole root.
    ///
    /// `spec.md:1407-1409`: *"A Chord root-replacement operation remains a delta unless the
    /// definition selected a checkpoint; it does not authorize reclamation."* So this is a question
    /// callers ask, and answering it must not be confused with selecting a base.
    #[must_use]
    pub const fn is_root_replacement(&self) -> bool {
        matches!(self, Self::ReplaceRoot(_))
    }
}

/// An ordered batch of operations: one prepared change's exact edit.
///
/// `spec.md:4350` keeps order meaningful *within* one incarnation's delta tail, so this is a
/// sequence and not a set — unlike S3's `Batch`, where order is deliberately not expressible.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct OpBatch(Arc<[Op]>);

impl OpBatch {
    /// The empty batch. `spec.md:1357`: *"An existing current-version document with an empty batch
    /// writes and publishes nothing."*
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// A batch from operations.
    #[must_use]
    pub fn new(ops: impl Into<Arc<[Op]>>) -> Self {
        Self(ops.into())
    }

    /// The operations, in order.
    #[must_use]
    pub fn ops(&self) -> &[Op] {
        &self.0
    }

    /// How many operations.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the batch is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for OpBatch {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for op in self.0.iter() {
            seq.serialize_element(op)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for OpBatch {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Vec::<Op>::deserialize(d).map(|ops| Self(ops.into()))
    }
}

impl Serialize for Op {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::ReplaceRoot(root) => {
                let mut t = s.serialize_seq(Some(2))?;
                t.serialize_element("r")?;
                t.serialize_element(root)?;
                t.end()
            }
            Self::Set { path, value } => {
                let mut t = s.serialize_seq(Some(3))?;
                t.serialize_element("s")?;
                t.serialize_element(path)?;
                t.serialize_element(value)?;
                t.end()
            }
            Self::Delete { path } => {
                let mut t = s.serialize_seq(Some(2))?;
                t.serialize_element("d")?;
                t.serialize_element(path)?;
                t.end()
            }
            Self::AppendStr { path, text } => {
                let mut t = s.serialize_seq(Some(3))?;
                t.serialize_element("a")?;
                t.serialize_element(path)?;
                t.serialize_element(&**text)?;
                t.end()
            }
            Self::TrimStrFront { path, count } => {
                let mut t = s.serialize_seq(Some(3))?;
                t.serialize_element("t")?;
                t.serialize_element(path)?;
                t.serialize_element(&count.get())?;
                t.end()
            }
            Self::Splice {
                path,
                at,
                remove,
                items,
            } => {
                let mut t = s.serialize_seq(Some(5))?;
                t.serialize_element("p")?;
                t.serialize_element(path)?;
                t.serialize_element(at)?;
                t.serialize_element(remove)?;
                t.serialize_element(&**items)?;
                t.end()
            }
            Self::Permute { path, perm } => {
                let mut t = s.serialize_seq(Some(3))?;
                t.serialize_element("m")?;
                t.serialize_element(path)?;
                t.serialize_element(perm.indices())?;
                t.end()
            }
        }
    }
}

/// One wire path segment: a string key or a numeric index.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireSeg {
    /// An array index.
    Index(u32),
    /// An object key.
    Key(String),
}

/// A decoded path, with the reserved-segment rejection applied.
struct WirePath(Path);

impl<'de> Deserialize<'de> for WirePath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let wire = Vec::<WireSeg>::deserialize(d)?;
        let mut segs = Vec::with_capacity(wire.len());
        for seg in wire {
            match seg {
                WireSeg::Index(i) => segs.push(Seg::index(i)),
                WireSeg::Key(k) => segs.push(Seg::key(&k).map_err(unsafe_seg::<D::Error>)?),
            }
        }
        Path::new(segs).map(Self).map_err(unsafe_seg::<D::Error>)
    }
}

/// Report a reserved path segment as a decode error.
fn unsafe_seg<E: DeError>(err: UnsafePathError) -> E {
    E::custom(err)
}

impl<'de> Deserialize<'de> for Op {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_seq(OpVisitor)
    }
}

/// Decodes Chord's tuple form. The tag comes first, and each arm then demands exactly the elements
/// its operation has — a short tuple is a decode error rather than a defaulted field.
struct OpVisitor;

impl<'de> Visitor<'de> for OpVisitor {
    type Value = Op;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a Chord operation tuple: [\"r\"|\"s\"|\"d\"|\"a\"|\"t\"|\"p\"|\"m\", ..]")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Op, A::Error> {
        let tag: String = next(&mut seq, "an operation tag")?;
        let op = match tag.as_str() {
            "r" => Op::ReplaceRoot(next(&mut seq, "a replacement root")?),
            "s" => Op::Set {
                path: next::<WirePath, A>(&mut seq, "a path")?.0,
                value: next(&mut seq, "a value")?,
            },
            "d" => Op::Delete {
                path: next::<WirePath, A>(&mut seq, "a path")?.0,
            },
            "a" => Op::AppendStr {
                path: next::<WirePath, A>(&mut seq, "a path")?.0,
                text: Arc::from(next::<String, A>(&mut seq, "text to append")?.as_str()),
            },
            "t" => {
                let path = next::<WirePath, A>(&mut seq, "a path")?.0;
                let raw: u32 = next(&mut seq, "a trim count")?;
                Op::TrimStrFront {
                    path,
                    count: TrimLen::parse(raw).map_err(A::Error::custom)?,
                }
            }
            "p" => Op::Splice {
                path: next::<WirePath, A>(&mut seq, "a path")?.0,
                at: next(&mut seq, "a splice start")?,
                remove: next(&mut seq, "a removal count")?,
                items: Arc::new(next(&mut seq, "items to insert")?),
            },
            "m" => {
                let path = next::<WirePath, A>(&mut seq, "a path")?.0;
                let raw: Vec<u32> = next(&mut seq, "a permutation")?;
                Op::Permute {
                    path,
                    perm: Permutation::parse(raw).map_err(A::Error::custom)?,
                }
            }
            other => {
                return Err(A::Error::custom(format!(
                    "unknown operation tag {other:?}; Chord has exactly seven"
                )));
            }
        };
        // A longer tuple than the operation has is a different operation, not this one with extras.
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(A::Error::custom(format!(
                "operation {tag:?} has trailing tuple elements"
            )));
        }
        Ok(op)
    }
}

/// The next tuple element, or a decode error naming what was expected.
fn next<'de, T, A>(seq: &mut A, expected: &str) -> Result<T, A::Error>
where
    T: Deserialize<'de>,
    A: SeqAccess<'de>,
{
    seq.next_element::<T>()?
        .ok_or_else(|| A::Error::custom(format!("an operation tuple is missing {expected}")))
}
