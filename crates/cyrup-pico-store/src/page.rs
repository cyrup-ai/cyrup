//! Page limits (ADR-0030 §2.2's `limit` row, F6 §B; `spec.md:4338-4339`).

use core::fmt;
use core::num::NonZeroU32;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The maximum size of one scan page.
///
/// Upstream is a bare `number`, which admits 0, a negative and an absurd value
/// (`spec.md:4338-4339`, *"`limit` is always the maximum page size"*). `NonZeroU32` plus
/// [`PageLimit::MAX`] makes all three unrepresentable in the value.
///
/// The *"maximum, never a guarantee"* half stays a documentation contract, as ADR-0030 §2.2 says:
/// a backend may legitimately return fewer items than asked for and signal continuation with a
/// cursor instead, so no type can promise the count.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PageLimit(NonZeroU32);

impl PageLimit {
    /// The largest page this kernel will accept.
    ///
    /// **A cyrup decision, not a specification value** — `spec.md` caps `limit` nowhere, and
    /// ADR-0030 §2.2 asks for *"`PageLimit(NonZeroU32)` parsed with a `MAX`"* without naming the
    /// number. 10 000 is chosen because a page is materialised into one `Vec` of owned records by
    /// every backend (ownership, not borrowing, is what ADR-0030 F4 buys), so the limit is the
    /// bound on a single response's allocation. A caller that wants more pages the scan with a
    /// cursor, which is the paging mechanism that exists; a caller that wants one enormous page is
    /// asking for the all-record scan `spec.md:4344` says there is no such thing as.
    pub const MAX: u32 = 10_000;

    /// The validating constructor: rejects zero and anything past [`PageLimit::MAX`].
    pub const fn parse(n: u32) -> Result<Self, PageLimitError> {
        if n > Self::MAX {
            return Err(PageLimitError::AboveMax {
                requested: n,
                max: Self::MAX,
            });
        }
        match NonZeroU32::new(n) {
            Some(nz) => Ok(Self(nz)),
            None => Err(PageLimitError::Zero),
        }
    }

    /// The limit, for a backend to bound its page with.
    ///
    /// Not a forgery path: there is no `From<u32>` and no `Default`, so this number cannot become a
    /// `PageLimit` again without going back through [`PageLimit::parse`].
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

/// Why a page limit was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, thiserror::Error)]
pub enum PageLimitError {
    /// A page of zero items is not a page.
    #[error("a page limit of 0 requests no items")]
    Zero,
    /// Past [`PageLimit::MAX`].
    #[error("page limit {requested} is above the maximum of {max}")]
    AboveMax {
        /// What was asked for.
        requested: u32,
        /// [`PageLimit::MAX`].
        max: u32,
    },
}

impl fmt::Display for PageLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Serialize for PageLimit {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_u32(self.0.get())
    }
}

/// Hand-written, per ADR-0030 §10: *"through `parse`; a derived impl over a private `NonZeroU32`
/// accepts values past `MAX`"*.
impl<'de> Deserialize<'de> for PageLimit {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let nz = crate::de::nonzero_u32(d, "a page limit between 1 and PageLimit::MAX")?;
        Self::parse(nz.get()).map_err(serde::de::Error::custom)
    }
}
