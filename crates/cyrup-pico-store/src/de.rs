//! The one implementation of "require an unsigned integer, reject everything else".
//!
//! ADR-0030 §10's serde table requires `Id<K>`, `Seq`, `DefVersion` and `PageLimit` to *"require a
//! u64 (reject string, float, negative), reject 0, build through the private constructor"*. Writing
//! that four times invites the fourth copy to drift, so it is written once here and the four
//! `Deserialize` impls differ only in the `expecting` text and the range they then narrow to.
//!
//! The visitors implement **only** `visit_u64`. Every other `Visitor` method keeps serde's default,
//! which is `Err(invalid_type)` — so a JSON string reaches `visit_str` and is rejected, a float
//! reaches `visit_f64` and is rejected, and a negative reaches `visit_i64` and is rejected, with no
//! code here to get wrong. Zero is the one rejection that needs a line, because it arrives through
//! `visit_u64` as a well-typed value.
//!
//! `deserialize_any` is used rather than `deserialize_u64` so the visitor sees the value's real
//! type and the error names it ("invalid type: string \"7\"") instead of the format's own guess.
//! That is sound because every record in this crate is persisted as JSON or JSONL, both
//! self-describing; the SQLite backend (ADR-0030 §9) reads columns through `rusqlite`, not serde.

use core::fmt;
use core::num::{NonZeroU32, NonZeroU64};

use serde::Deserializer;
use serde::de::{Error as DeError, Unexpected, Visitor};

struct NonZeroU64Visitor {
    expecting: &'static str,
}

impl<'de> Visitor<'de> for NonZeroU64Visitor {
    type Value = NonZeroU64;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_u64<E: DeError>(self, v: u64) -> Result<Self::Value, E> {
        NonZeroU64::new(v).ok_or_else(|| E::invalid_value(Unexpected::Unsigned(v), &self))
    }
}

/// Decode a nonzero `u64`, rejecting a string, a float, a negative, a boolean, `null`, a sequence,
/// a map and zero.
pub(crate) fn nonzero_u64<'de, D>(d: D, expecting: &'static str) -> Result<NonZeroU64, D::Error>
where
    D: Deserializer<'de>,
{
    d.deserialize_any(NonZeroU64Visitor { expecting })
}

struct NonZeroU32Visitor {
    expecting: &'static str,
}

impl<'de> Visitor<'de> for NonZeroU32Visitor {
    type Value = NonZeroU32;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.expecting)
    }

    fn visit_u64<E: DeError>(self, v: u64) -> Result<Self::Value, E> {
        let narrowed =
            u32::try_from(v).map_err(|_| E::invalid_value(Unexpected::Unsigned(v), &self))?;
        NonZeroU32::new(narrowed).ok_or_else(|| E::invalid_value(Unexpected::Unsigned(v), &self))
    }
}

/// Decode a nonzero `u32`. Beyond [`nonzero_u64`]'s rejections this also rejects a value past
/// `u32::MAX`, which a damaged file can hold and which must fail rather than wrap.
pub(crate) fn nonzero_u32<'de, D>(d: D, expecting: &'static str) -> Result<NonZeroU32, D::Error>
where
    D: Deserializer<'de>,
{
    d.deserialize_any(NonZeroU32Visitor { expecting })
}
