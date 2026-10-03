//! The persisted document value (ADR-0030 F4; §4 is this file's before/after sketch).

use core::fmt;
use std::sync::Arc;

use indexmap::IndexMap;
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Serialize, Serializer};

/// A JSON number. `serde_json::Number::from_f64` returns `Option`, so a non-finite value is
/// unrepresentable *inside* the tree and the only place it can be rejected is the boundary.
pub type JsonNum = serde_json::Number;

/// A document object: insertion-ordered keys, order-insensitive equality.
///
/// `IndexMap`'s `PartialEq` compares as a set of pairs, which is exactly Chord's *"equality
/// ignores key order"* (`packages/chord/src/delta/README.md`), so Pico5's no-op normalisation rule
/// (`spec.md:1357-1361`) comes from the container rather than from a hand-written walk. Insertion
/// order survives into the bytes because `serde_json/preserve_order` is on workspace-wide
/// (`Cargo.toml`).
pub type DocMap = IndexMap<Arc<str>, DocValue>;

/// The persisted document value. Containers are `Arc`'d so successive revisions share every
/// unchanged subtree.
///
/// # LOAD-BEARING: no variant contains interior mutability
///
/// No variant contains `Cell`, `RefCell`, `Mutex`, `RwLock`, `OnceCell`, `OnceLock` or
/// `UnsafeCell`. That, and only that, is what makes `Arc<DocValue>` *immutable* rather than merely
/// *shared*, and it is the single rule ADR-0030 F4's whole deletion rests on. Adding an
/// interior-mutable variant — or admitting a host value type that contains one — reopens every
/// hazard this type closes, and no test can catch it, so it is the one thing a reviewer must check
/// on a change to this enum.
///
/// Upstream carries the same guarantee as a sentence: *"Immutability is an ownership contract.
/// Nothing is frozen or defensively copied, so an illegal mutation is not detected. It silently
/// corrupts state."* (`packages/chord/src/delta/README.md`), operationalised as a ten-row
/// mutation-rights table plus a recursive deep copy on both storage paths
/// (`packages/durable/src/storage/memory.ts:92-100`). Here the table is the type, and
/// `spec.md:4528-4530`'s *"trusted immutable revisions"* is `unrepresentable` instead of trusted.
#[derive(Clone, PartialEq, Debug)]
pub enum DocValue {
    /// JSON `null`. Also what `Option::None` and `()` parse to, as in `serde_json`.
    Null,
    /// JSON `true` / `false`.
    Bool(bool),
    /// A finite JSON number. Non-finite is rejected by [`DocValue::parse`], not stored.
    Num(JsonNum),
    /// A JSON string. `Arc<str>` so a string shared between revisions is shared, not copied —
    /// which is exactly what `spec.md:1566`'s *"immutable strings may be shared"* licenses.
    Str(Arc<str>),
    /// A dense JSON array. Chord keeps arrays dense; there is no sparse representation here.
    List(Arc<Vec<DocValue>>),
    /// A JSON object.
    Map(Arc<DocMap>),
}

impl DocValue {
    /// A string value, without going through [`DocValue::parse`]'s serializer.
    ///
    /// Sound because a Rust `&str` is already strict JSON: there is no non-finite string, so this
    /// path cannot smuggle past the one check `parse` exists to perform.
    #[must_use]
    pub fn string(s: &str) -> Self {
        Self::Str(Arc::from(s))
    }

    /// An integer value. Integers cannot be non-finite, so this needs no check either.
    #[must_use]
    pub fn integer(n: i64) -> Self {
        Self::Num(JsonNum::from(n))
    }

    /// A floating-point value, rejecting non-finite.
    ///
    /// **This is the whole of ADR-0030 F4's one surviving runtime check**, in the one-argument
    /// case. `serde_json::Value::from(f64::NAN)` is `Value::Null` — it does not error — so a port
    /// that reached for `to_value` would lose the user's number and raise nothing.
    pub fn number(n: f64) -> Result<Self, NotStrictJson> {
        JsonNum::from_f64(n)
            .map(Self::Num)
            .ok_or(NotStrictJson::NonFinite(n))
    }

    /// `true` when this is JSON `null`.
    #[must_use]
    pub const fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// The object behind a [`DocValue::Map`], if this is one.
    #[must_use]
    pub fn as_map(&self) -> Option<&DocMap> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// The array behind a [`DocValue::List`], if this is one.
    #[must_use]
    pub fn as_list(&self) -> Option<&[DocValue]> {
        match self {
            Self::List(l) => Some(l),
            _ => None,
        }
    }

    /// The string behind a [`DocValue::Str`], if this is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The name of this value's JSON type, for a diagnostic.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "boolean",
            Self::Num(_) => "number",
            Self::Str(_) => "string",
            Self::List(_) => "array",
            Self::Map(_) => "object",
        }
    }
}

/// A document root is a JSON object, so [`DocValue::Str`] as a root is unrepresentable
/// (`spec.md:4240` types every stored base's `value` as `JsonObject`).
///
/// The field is private and there is no `From<DocMap>`: a root is built by [`DocRoot::parse`],
/// by [`DocRoot::empty`], by the applier, or by a decode that went through the
/// hand-written `Deserialize`. That is what makes "any owned `DocValue` is a valid root" true in
/// ADR-0030 F4's sense while still keeping a non-object out.
#[derive(Clone, PartialEq, Debug)]
pub struct DocRoot(Arc<DocMap>);

impl DocRoot {
    /// The empty document object. `initial()` in `spec.md:955` terms, before a definition supplies
    /// its own.
    #[must_use]
    pub fn empty() -> Self {
        Self(Arc::new(DocMap::new()))
    }

    /// Wrap an already-parsed object as a root.
    ///
    /// Not a bypass: every `DocMap` in existence was itself built by [`DocValue::parse`], by the
    /// applier, or by a validating decode, so there is no path from unparsed host data to here.
    #[must_use]
    pub fn from_map(map: DocMap) -> Self {
        Self(Arc::new(map))
    }

    /// The object. A shared reference, never `&mut`: a published revision has no mutable view.
    #[must_use]
    pub fn as_map(&self) -> &DocMap {
        &self.0
    }

    /// One top-level member.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&DocValue> {
        self.0.get(key)
    }

    /// Number of top-level members.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the root has no members.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// This root as an ordinary value, in O(1).
    #[must_use]
    pub fn into_value(self) -> DocValue {
        DocValue::Map(self.0)
    }

    /// Whether these two roots are the **same allocation**, not merely equal.
    ///
    /// The one honest way to assert ADR-0030 §2.3's *"unchanged subtrees are structurally shared
    /// between successive revisions"* and its mirror image, S3's detachment obligation (*"a committed
    /// value cannot be observed to change"*). Public because those assertions belong in the crates
    /// that make the claim, and this is cheaper and clearer than handing out the `Arc`.
    #[must_use]
    pub fn shares_allocation_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    /// Re-wrap an object the applier already owns. Crate-private: the invariant it preserves is
    /// "this `Arc` came from a root", which only this crate can know.
    pub(crate) fn from_arc(map: Arc<DocMap>) -> Self {
        Self(map)
    }

    /// Build a root from a value, requiring an object.
    pub(crate) fn from_value(value: DocValue) -> Result<Self, NotStrictJson> {
        match value {
            DocValue::Map(m) => Ok(Self(m)),
            other => Err(NotStrictJson::RootIsNotAnObject {
                found: other.type_name(),
            }),
        }
    }
}

/// Why a host value is not strict JSON.
///
/// Eight of Chord's ten rejected placement shapes (`undefined` members, functions, symbols,
/// bigints, accessors, symbol keys, sparse arrays, foreign prototypes) are not representable in
/// Rust and have no variant here, because there is no check to fail. What remains is real:
/// non-finite floats, an integer outside JSON's range, a non-string object key, unbounded recursion
/// through a host `Serialize` impl, and a root that is not an object.
#[derive(Clone, PartialEq, Debug, thiserror::Error)]
pub enum NotStrictJson {
    /// A `NaN` or an infinity. **The regression this type exists to prevent.**
    #[error("not strict JSON: the number {0} is not finite")]
    NonFinite(f64),
    /// An `i128`/`u128` outside the range `serde_json::Number` can hold without loss.
    #[error("not strict JSON: the integer {0} is outside the range a JSON number can hold")]
    IntegerOutOfRange(String),
    /// A map key that is not a string. JSON object keys are strings.
    #[error("not strict JSON: an object key serialized as {found}, not a string")]
    KeyIsNotAString {
        /// The JSON type the key serialized as.
        found: &'static str,
    },
    /// A map entry whose value was serialized before its key, which no correct `Serialize` does.
    #[error("not strict JSON: an object value was serialized with no key")]
    ValueBeforeKey,
    /// Nesting past [`MAX_DEPTH`].
    ///
    /// A host `Serialize` over a cyclic `Rc` graph recurses without bound, and
    /// `#![forbid(unsafe_code)]` does not protect against a stack overflow — so this is a real
    /// limit, not a hygiene limit (ADR-0030 F4, *"guarantee not gained"* item 3).
    #[error(
        "not strict JSON: nesting deeper than {MAX_DEPTH} levels (a cycle, or a pathological value)"
    )]
    TooDeep,
    /// A root that is not a JSON object.
    #[error("a document root must be a JSON object, found {found}")]
    RootIsNotAnObject {
        /// The JSON type the value serialized as.
        found: &'static str,
    },
    /// The host's own `Serialize` impl failed.
    #[error("the value's own Serialize implementation failed: {0}")]
    Host(String),
}

impl serde::ser::Error for NotStrictJson {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Self::Host(msg.to_string())
    }
}

/// The nesting limit [`DocValue::parse`] enforces.
///
/// A document is configuration-shaped state, not a tree structure: `spec.md` nests no stored shape
/// past a handful of levels. 128 is chosen because it is far past any legitimate document and far
/// short of the recursion depth that overflows a default 2 MiB stack while building `DocValue`
/// frames.
pub const MAX_DEPTH: u32 = 128;

// ---------------------------------------------------------------------------------------------
// Serialize. The DEserialize half is in `de.rs`, hand-written with the same depth limit, because a
// damaged file can nest arbitrarily and that is corruption rather than absence (ADR-0030 §10).
// ---------------------------------------------------------------------------------------------

impl Serialize for DocValue {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Null => s.serialize_unit(),
            Self::Bool(b) => s.serialize_bool(*b),
            Self::Num(n) => n.serialize(s),
            Self::Str(v) => s.serialize_str(v),
            Self::List(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items.iter() {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Self::Map(entries) => {
                let mut map = s.serialize_map(Some(entries.len()))?;
                for (k, v) in entries.iter() {
                    map.serialize_entry(&**k, v)?;
                }
                map.end()
            }
        }
    }
}

impl Serialize for DocRoot {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in self.0.iter() {
            map.serialize_entry(&**k, v)?;
        }
        map.end()
    }
}

impl fmt::Display for DocValue {
    /// Compact JSON, for a diagnostic. Not a storage path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match serde_json::to_string(self) {
            Ok(text) => f.write_str(&text),
            Err(_) => f.write_str("<unserializable document value>"),
        }
    }
}
