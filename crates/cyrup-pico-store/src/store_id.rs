//! The per-store identity stamp (ADR-0030 F6 §B and §D).

use core::fmt;
use core::num::NonZeroU128;

use serde::de::{Error as DeError, Unexpected, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Identifies one store, for the life of that store.
///
/// Two jobs, both from ADR-0030 F6:
///
/// * **§B** — it sits inside `CursorBytes { store: StoreId, payload }` so that resuming a scan with
///   a cursor minted by a *different* store is caught. Per-scan cursor types make cross-scan reuse
///   a compile error; nothing in the type system can see across two stores, so this is a `checked`
///   comparison. ADR-0030 concedes it *"is easy to dismiss as ceremony — it is one `u128`
///   comparison, and the failure it prevents is a silent hole in a user's transcript"*.
/// * **§D** — it goes in the store's identity record beside the holder pid, so a stale or ignored
///   advisory lock is detectable.
///
/// `NonZeroU128` so that "no store id yet" is not the same value as a store, and so a 128-bit
/// random value collides never rather than rarely. Generating one is a backend's job (S7): this
/// crate takes no random-number dependency for a type it does not mint.
#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StoreId(NonZeroU128);

/// The number of hex digits in the wire form.
const HEX_LEN: usize = 32;

impl StoreId {
    /// A store id from a nonzero 128-bit value.
    #[must_use]
    pub const fn new(raw: NonZeroU128) -> Self {
        Self(raw)
    }

    /// The raw value, so a backend can write it into its identity record by a route other than
    /// serde.
    #[must_use]
    pub const fn get(self) -> NonZeroU128 {
        self.0
    }
}

impl fmt::Display for StoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0.get())
    }
}

impl fmt::Debug for StoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StoreId({self})")
    }
}

/// A fixed-width lowercase hex string, not a number.
///
/// A `u128` does not survive a JSON round trip through every reader — it is outside IEEE-754's
/// exact integer range, so a consumer that parses numbers as doubles silently corrupts the low
/// bits, and the whole point of this value is exact equality. The hex form is lossless everywhere
/// and reads as an opaque stamp rather than as a quantity, which is what it is.
impl Serialize for StoreId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

struct StoreIdVisitor;

impl<'de> Visitor<'de> for StoreIdVisitor {
    type Value = StoreId;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a {HEX_LEN}-digit nonzero hex store id")
    }

    fn visit_str<E: DeError>(self, v: &str) -> Result<Self::Value, E> {
        if v.len() != HEX_LEN || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(E::invalid_value(Unexpected::Str(v), &self));
        }
        let raw =
            u128::from_str_radix(v, 16).map_err(|_| E::invalid_value(Unexpected::Str(v), &self))?;
        NonZeroU128::new(raw)
            .map(StoreId::new)
            .ok_or_else(|| E::invalid_value(Unexpected::Str(v), &self))
    }
}

/// Hand-written: a derived impl would accept any `u128`, including zero, and would accept the
/// number form this type deliberately does not use. Uppercase hex is accepted on the way in —
/// these records are hand-editable during an incident — and lowercase is always written.
impl<'de> Deserialize<'de> for StoreId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(StoreIdVisitor)
    }
}
