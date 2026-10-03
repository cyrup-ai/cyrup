//! The decode half, hand-written where ADR-0030 §10's serde table says to.
//!
//! The table's reasoning for each impl here:
//!
//! * [`DocValue`] — *"safe: JSON has no `NaN` literal and `serde_json::Number`'s own `Deserialize`
//!   cannot yield a non-finite, so the decode path cannot forge what `parse` rejects. **Still needs
//!   the same depth limit**, because a damaged file can nest arbitrarily — that is corruption, not
//!   absence."* So this is derive-*equivalent* in what it accepts, and hand-written only to carry
//!   the depth. The depth is threaded through a `DeserializeSeed` rather than a thread-local,
//!   because a thread-local is state a concurrent decode can corrupt.
//! * [`DocRoot`] — *"hand-written: require a JSON object, so the error names the document."*
//! * [`DefVersion`] — a derived impl over a private `NonZeroU32` still accepts any nonzero number,
//!   and a number past `u32::MAX` must fail rather than narrow: narrowing silently would make two
//!   different stored versions compare equal, and comparing versions is the whole of migration
//!   (`spec.md:1441-1464`).

use core::fmt;
use core::num::NonZeroU32;
use std::sync::Arc;

use serde::de::{DeserializeSeed, Error as DeError, MapAccess, SeqAccess, Unexpected, Visitor};
use serde::{Deserialize, Deserializer};

use crate::value::{DocMap, DocRoot, DocValue, MAX_DEPTH, NotStrictJson};
use crate::version::DefVersion;

/// A `DocValue` to be decoded with this many containers already open.
#[derive(Clone, Copy)]
struct AtDepth(u32);

impl AtDepth {
    /// One container deeper, or the corruption arm.
    fn descend<E: DeError>(self) -> Result<Self, E> {
        let depth = self.0.saturating_add(1);
        if depth > MAX_DEPTH {
            return Err(E::custom(NotStrictJson::TooDeep));
        }
        Ok(Self(depth))
    }
}

impl<'de> DeserializeSeed<'de> for AtDepth {
    type Value = DocValue;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<DocValue, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for AtDepth {
    type Value = DocValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_unit<E: DeError>(self) -> Result<DocValue, E> {
        Ok(DocValue::Null)
    }

    fn visit_none<E: DeError>(self) -> Result<DocValue, E> {
        Ok(DocValue::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<DocValue, D::Error> {
        d.deserialize_any(self)
    }

    fn visit_bool<E: DeError>(self, v: bool) -> Result<DocValue, E> {
        Ok(DocValue::Bool(v))
    }

    fn visit_i64<E: DeError>(self, v: i64) -> Result<DocValue, E> {
        Ok(DocValue::Num(v.into()))
    }

    fn visit_u64<E: DeError>(self, v: u64) -> Result<DocValue, E> {
        Ok(DocValue::Num(v.into()))
    }

    /// The asymmetry ADR-0030 F4 turns on: a JSON document cannot *contain* `NaN`, so a decoder
    /// cannot hand one to us through a self-describing format. A format that could (a binary one)
    /// would be rejected right here rather than silently stored.
    fn visit_f64<E: DeError>(self, v: f64) -> Result<DocValue, E> {
        DocValue::number(v).map_err(E::custom)
    }

    fn visit_str<E: DeError>(self, v: &str) -> Result<DocValue, E> {
        Ok(DocValue::Str(Arc::from(v)))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<DocValue, A::Error> {
        let inner = self.descend::<A::Error>()?;
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(64));
        while let Some(item) = seq.next_element_seed(inner)? {
            items.push(item);
        }
        Ok(DocValue::List(Arc::new(items)))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DocValue, A::Error> {
        let inner = self.descend::<A::Error>()?;
        Ok(DocValue::Map(Arc::new(decode_map(&mut map, inner)?)))
    }
}

/// Decode an object's entries at a fixed depth. Shared by [`DocValue`] and [`DocRoot`].
fn decode_map<'de, A: MapAccess<'de>>(map: &mut A, inner: AtDepth) -> Result<DocMap, A::Error> {
    let mut entries = DocMap::with_capacity(map.size_hint().unwrap_or(0).min(64));
    while let Some(key) = map.next_key::<String>()? {
        let value = map.next_value_seed(inner)?;
        entries.insert(Arc::from(key.as_str()), value);
    }
    Ok(entries)
}

impl<'de> Deserialize<'de> for DocValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(AtDepth(0))
    }
}

/// Requires a JSON object, so the error names what the stored base actually was.
impl<'de> Deserialize<'de> for DocRoot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct RootVisitor;

        impl<'de> Visitor<'de> for RootVisitor {
            type Value = DocRoot;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a document root: a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<DocRoot, A::Error> {
                let inner = AtDepth(0).descend::<A::Error>()?;
                Ok(DocRoot::from_map(decode_map(&mut map, inner)?))
            }
        }

        d.deserialize_map(RootVisitor)
    }
}

/// Hand-written, per ADR-0030 §10. Rejects a string, a float, a negative, zero, and a value past
/// `u32::MAX`, and builds through the private constructor.
impl<'de> Deserialize<'de> for DefVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct VersionVisitor;

        impl<'de> Visitor<'de> for VersionVisitor {
            type Value = NonZeroU32;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a nonzero u32 definition version")
            }

            /// `visit_u64` only. Every other `Visitor` method keeps serde's default, which is
            /// `Err(invalid_type)` — so a string reaches `visit_str` and is rejected, a float
            /// reaches `visit_f64` and is rejected, and a negative reaches `visit_i64` and is
            /// rejected, with no code here to get wrong.
            fn visit_u64<E: DeError>(self, v: u64) -> Result<NonZeroU32, E> {
                let narrowed = u32::try_from(v)
                    .map_err(|_| E::invalid_value(Unexpected::Unsigned(v), &self))?;
                NonZeroU32::new(narrowed)
                    .ok_or_else(|| E::invalid_value(Unexpected::Unsigned(v), &self))
            }
        }

        // `deserialize_any` rather than `deserialize_u32` so the visitor sees the value's real type
        // and the error names it ("invalid type: string \"7\"") instead of the format's own guess.
        // Sound because every record here is persisted as JSON or JSONL, both self-describing.
        d.deserialize_any(VersionVisitor).map(DefVersion::new)
    }
}
