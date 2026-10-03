//! Paths into a document, and the mutation spine.
//!
//! `packages/chord/src/delta/README.md`: *"Paths contain object keys and non-negative integer array
//! indices."* Two things about them are contractual rather than incidental:
//!
//! * **Reserved segments.** `__proto__`, `constructor` and `prototype` are never emitted as path
//!   segments, and every applier *"rejects those segments with `UnsafePathError`"*. In Rust those
//!   keys are ordinary `IndexMap` keys and carry no prototype hazard at all — but the rejection is
//!   part of the operation contract a wire batch is validated against, so a decoded batch carrying
//!   one is rejected here rather than applied to something pi would have refused. Keeping it is
//!   feature parity; dropping it would be a behavioural difference.
//! * **Dense arrays.** An index path addresses an existing element or, for a set, the one position
//!   past the end. *"Writing past the next index and deleting an element throw."*

use std::sync::Arc;

use serde::ser::SerializeSeq;
use serde::{Serialize, Serializer};

use crate::value::{DocMap, DocValue};

/// One step of a path: an object key or an array index.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Seg {
    /// An object member.
    Key(Arc<str>),
    /// An array element, zero-based.
    Index(u32),
}

impl Seg {
    /// A key segment, rejecting the three reserved names.
    ///
    /// # Errors
    ///
    /// [`UnsafePathError`] for `__proto__`, `constructor` or `prototype`.
    pub fn key(name: &str) -> Result<Self, UnsafePathError> {
        if RESERVED_SEGMENTS.contains(&name) {
            return Err(UnsafePathError {
                segment: name.to_owned(),
            });
        }
        Ok(Self::Key(Arc::from(name)))
    }

    /// An index segment. Always legal to *name*; whether it addresses anything is apply-time.
    #[must_use]
    pub const fn index(i: u32) -> Self {
        Self::Index(i)
    }
}

/// The segments no path may contain (`packages/chord/src/delta/README.md`, *"Reserved keys"*).
pub const RESERVED_SEGMENTS: [&str; 3] = ["__proto__", "constructor", "prototype"];

/// A path segment an operation may not use.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("unsafe path segment {segment:?}: Chord never emits it and every applier rejects it")]
pub struct UnsafePathError {
    /// The rejected segment.
    pub segment: String,
}

/// A path from the document root to one value. An empty path is the root itself.
#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct Path(Arc<[Seg]>);

impl Path {
    /// The root path.
    #[must_use]
    pub fn root() -> Self {
        Self::default()
    }

    /// A path from segments, rejecting a reserved key.
    ///
    /// # Errors
    ///
    /// [`UnsafePathError`] if any segment is reserved.
    pub fn new(segs: impl IntoIterator<Item = Seg>) -> Result<Self, UnsafePathError> {
        let segs: Arc<[Seg]> = segs.into_iter().collect();
        for seg in segs.iter() {
            if let Seg::Key(k) = seg
                && RESERVED_SEGMENTS.contains(&&**k)
            {
                return Err(UnsafePathError {
                    segment: k.to_string(),
                });
            }
        }
        Ok(Self(segs))
    }

    /// A path of object keys, the common case: `Path::keys(["a", "b"])`.
    ///
    /// # Errors
    ///
    /// [`UnsafePathError`] if any key is reserved.
    pub fn keys<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<Self, UnsafePathError> {
        let mut segs = Vec::new();
        for key in keys {
            segs.push(Seg::key(key)?);
        }
        Self::new(segs)
    }

    /// The segments.
    #[must_use]
    pub fn segments(&self) -> &[Seg] {
        &self.0
    }

    /// Whether this is the root path.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// This path with one more segment.
    #[must_use]
    pub fn child(&self, seg: Seg) -> Self {
        let mut segs: Vec<Seg> = self.0.to_vec();
        segs.push(seg);
        Self(segs.into())
    }
}

impl Serialize for Path {
    /// Chord's wire form: a flat array mixing strings and numbers.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for seg in self.0.iter() {
            match seg {
                Seg::Key(k) => seq.serialize_element(&**k)?,
                Seg::Index(i) => seq.serialize_element(i)?,
            }
        }
        seq.end()
    }
}

/// Why a path did not address a value, or an operation could not be applied.
///
/// Every variant names a shape pi's appliers also refuse. None of them is a programmer error that
/// `unwrap` would be acceptable for: a decoded batch from a damaged file reaches [`apply`] with
/// arbitrary paths, and `ReplayPlan::parse` turns these into `Corruption` rather than a panic.
///
/// [`apply`]: crate::apply()
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum PathError {
    /// A key step into something that is not an object.
    #[error("path step {key:?} expected an object, found {found}")]
    NotAnObject {
        /// The key that was being followed.
        key: String,
        /// What was there instead.
        found: &'static str,
    },
    /// An index step into something that is not an array.
    #[error("path step [{index}] expected an array, found {found}")]
    NotAnArray {
        /// The index that was being followed.
        index: u32,
        /// What was there instead.
        found: &'static str,
    },
    /// A key that is not present.
    #[error("no member {key:?}")]
    NoSuchKey {
        /// The missing key.
        key: String,
    },
    /// An index past the end of the array.
    #[error("index {index} is past the end of an array of {len} elements")]
    IndexOutOfRange {
        /// The index asked for.
        index: u32,
        /// The array's length.
        len: usize,
    },
    /// A string operation on something that is not a string.
    #[error("a string operation found {found}")]
    NotAString {
        /// What was there instead.
        found: &'static str,
    },
    /// An array operation on something that is not an array.
    #[error("an array operation found {found}")]
    OperationNeedsAnArray {
        /// What was there instead.
        found: &'static str,
    },
    /// A trim longer than the string, in bytes.
    #[error("cannot trim {count} bytes from a string of {len}")]
    TrimPastEnd {
        /// The count asked for.
        count: u32,
        /// The string's length in bytes.
        len: usize,
    },
    /// A trim landing inside a multi-byte character.
    ///
    /// The counterpart of upstream's split-surrogate case, and it is a real arm rather than a
    /// hygiene arm: a trim count is a persisted number, so a damaged record can name any offset.
    /// Upstream cannot reach this, because a JS string *is* UTF-16 and half a surrogate pair is a
    /// representable (if useless) string; a Rust `str` is well-formed UTF-8 by construction, so the
    /// only honest answer is to refuse.
    #[error("trimming {count} bytes would land inside a multi-byte character")]
    TrimNotACharBoundary {
        /// The count asked for.
        count: u32,
    },
    /// A splice whose start is past the end of the array.
    #[error("splice at {at} is past the end of an array of {len} elements")]
    SpliceStartOutOfRange {
        /// The start index.
        at: u32,
        /// The array's length.
        len: usize,
    },
    /// A splice removing more elements than follow its start.
    #[error("splice at {at} cannot remove {remove} of {len} elements")]
    SpliceRemoveOutOfRange {
        /// The start index.
        at: u32,
        /// The count asked for.
        remove: u32,
        /// The array's length.
        len: usize,
    },
    /// A permutation whose length does not match the array's.
    #[error("a permutation of {perm_len} cannot reorder an array of {len}")]
    PermutationLengthMismatch {
        /// The permutation's length.
        perm_len: usize,
        /// The array's length.
        len: usize,
    },
    /// A delete of an array element, which Chord's appliers refuse: arrays stay dense and a
    /// deletion is expressed as a splice.
    #[error("delete does not address an array element; arrays stay dense, so use a splice")]
    DeleteOfArrayElement,
    /// A set at an index more than one past the end, which would leave a hole.
    #[error("set at index {index} would leave a hole in an array of {len} elements")]
    SetWouldLeaveAHole {
        /// The index asked for.
        index: u32,
        /// The array's length.
        len: usize,
    },
    /// A path operation addressed the root, where only a root replacement is meaningful.
    #[error("this operation cannot address the document root")]
    RootIsNotAddressable,
}

/// Walk the mutation spine to the value a path addresses, cloning exactly the containers on the way.
///
/// `Arc::make_mut` **is** ADR-0030 F4's structural sharing: it clones a container only when the
/// `Arc` is shared, which it is for every container reachable from a published revision, and leaves
/// every untouched subtree pointing at the same allocation. Upstream calls this `applyImmutable`'s
/// *"unchanged subtrees are structurally shared with base"*; here it is a refcount consequence and
/// there is no code to port.
///
/// Crate-private on purpose: a `&mut DocValue` into a revision is precisely what
/// `spec.md:4528-4530` forbids a consumer from having, so the only `&mut` in this crate is the one
/// the applier holds for the duration of one operation.
pub(crate) fn at_mut<'a>(
    root: &'a mut DocValue,
    path: &Path,
) -> Result<&'a mut DocValue, PathError> {
    let mut cur = root;
    for seg in path.segments() {
        cur = step_mut(cur, seg)?;
    }
    Ok(cur)
}

/// One step of [`at_mut`].
fn step_mut<'a>(cur: &'a mut DocValue, seg: &Seg) -> Result<&'a mut DocValue, PathError> {
    match seg {
        Seg::Key(key) => match cur {
            DocValue::Map(map) => {
                Arc::make_mut(map)
                    .get_mut(&**key)
                    .ok_or_else(|| PathError::NoSuchKey {
                        key: key.to_string(),
                    })
            }
            other => Err(PathError::NotAnObject {
                key: key.to_string(),
                found: other.type_name(),
            }),
        },
        Seg::Index(index) => match cur {
            DocValue::List(list) => {
                let list = Arc::make_mut(list);
                let len = list.len();
                let at = usize::try_from(*index).unwrap_or(usize::MAX);
                list.get_mut(at)
                    .ok_or(PathError::IndexOutOfRange { index: *index, len })
            }
            other => Err(PathError::NotAnArray {
                index: *index,
                found: other.type_name(),
            }),
        },
    }
}

/// Split a path into its parent and last segment, for the operations that address a *slot* rather
/// than a value: set, delete.
pub(crate) fn split_last(path: &Path) -> Result<(Path, &Seg), PathError> {
    let segs = path.segments();
    match segs.split_last() {
        Some((last, head)) => Ok((Path(Arc::from(head)), last)),
        None => Err(PathError::RootIsNotAddressable),
    }
}

/// The object a key slot lives in, cloned only if shared.
pub(crate) fn map_mut<'a>(slot: &'a mut DocValue, key: &str) -> Result<&'a mut DocMap, PathError> {
    match slot {
        DocValue::Map(map) => Ok(Arc::make_mut(map)),
        other => Err(PathError::NotAnObject {
            key: key.to_owned(),
            found: other.type_name(),
        }),
    }
}

/// The array an index slot lives in, cloned only if shared.
pub(crate) fn list_mut(slot: &mut DocValue) -> Result<&mut Vec<DocValue>, PathError> {
    match slot {
        DocValue::List(list) => Ok(Arc::make_mut(list)),
        other => Err(PathError::OperationNeedsAnArray {
            found: other.type_name(),
        }),
    }
}
