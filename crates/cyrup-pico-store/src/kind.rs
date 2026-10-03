//! A record kind: the stable string that names a definition (`spec.md:166`, `:1086`, `:1601`).
//!
//! Entries, tasks and documents all carry one, and all three persist it. A bare `String` in those
//! fields admits the empty kind, a kind with a newline in it (which a JSONL backend writes as a
//! record boundary), and a kind of arbitrary length — so this is a newtype with a parse, per
//! `RUST-DESIGN-REVIEW.md`'s *"create it at the system boundary"* rule.
//!
//! # What this does not do, deliberately
//!
//! ADR-0030 §2.2's last row wants *"a `Kind` newtype parsed at registration that rejects the
//! reserved namespace for third parties"*. That rejection is **not here**, and the omission is a
//! decision rather than an oversight: deciding whether a kind is third-party needs the registering
//! party's identity, which exists only at definition registration — PICO5-PLAN S5, in
//! `cyrup-pico`. A store-level rejection would also be wrong in the other direction, because the
//! kernel's own built-ins must be able to write reserved kinds through this same type.
//!
//! What is here is the half the storage layer owns: the shape. [`Kind::is_reserved`] is provided so
//! S5's registration check is a one-line call rather than a re-derived string test, and
//! [`RESERVED_PREFIX`] names the one prefix, in one place, because it is persisted.

use core::fmt;
use std::sync::Arc;

use serde::de::{Deserializer, Unexpected, Visitor};
use serde::{Serialize, Serializer};

/// The prefix reserved for kinds the kernel itself defines.
///
/// `spec.md:4549-4552` reserves `pi.` upstream — *"nothing enforces it"*, in §12's own words. This
/// store's records are cyrup's own persisted format, written and read by cyrup alone, so the
/// reserved prefix is cyrup's: a `pi.`-prefixed kind means nothing here and is an ordinary
/// application kind.
pub const RESERVED_PREFIX: &str = "cyrup.";

/// The longest kind this store accepts, in bytes.
///
/// A limit exists because the value is persisted and indexed, and because a damaged record can
/// claim any length. The number itself is a budget, not a domain fact.
pub const MAX_KIND_LEN: usize = 128;

/// A record kind.
///
/// `Arc<str>` rather than `String` because a kind is read far more often than it is built — every
/// scan filter and every document address compares one — and because cloning it into an index key
/// must not copy bytes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Kind(Arc<str>);

/// Why a string is not a [`Kind`].
///
/// `Serialize` only (ADR-0030 §10's rule for diagnostics): a rejection must be loggable without
/// becoming a construction path.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, thiserror::Error)]
pub enum NotAKind {
    /// The empty string. A kind names a definition; nothing is named by nothing.
    #[error("a record kind cannot be empty")]
    Empty,
    /// Longer than [`MAX_KIND_LEN`] bytes.
    #[error("a record kind is at most {max} bytes, found {found}")]
    TooLong {
        /// How long the candidate was.
        found: usize,
        /// The limit.
        max: usize,
    },
    /// A control character.
    ///
    /// A newline is the one that matters: `main.jsonl` is one record per line, so a kind containing
    /// `\n` would let a record forge a record boundary. The whole control range is rejected because
    /// there is no kind that legitimately contains one.
    #[error("a record kind cannot contain a control character (found U+{code:04X})")]
    ControlCharacter {
        /// The offending code point.
        code: u32,
    },
}

impl Kind {
    /// Parse a kind.
    ///
    /// # Errors
    ///
    /// [`NotAKind`] when the string is empty, longer than [`MAX_KIND_LEN`], or contains a control
    /// character.
    pub fn parse(s: &str) -> Result<Self, NotAKind> {
        if s.is_empty() {
            return Err(NotAKind::Empty);
        }
        if s.len() > MAX_KIND_LEN {
            return Err(NotAKind::TooLong {
                found: s.len(),
                max: MAX_KIND_LEN,
            });
        }
        if let Some(c) = s.chars().find(|c| c.is_control()) {
            return Err(NotAKind::ControlCharacter { code: c as u32 });
        }
        Ok(Self(Arc::from(s)))
    }

    /// The kind as a string.
    ///
    /// Not a bypass: a `&str` cannot become a `Kind` again without [`Kind::parse`].
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this kind is in the kernel's reserved namespace ([`RESERVED_PREFIX`]).
    ///
    /// The storage layer does not act on this. It exists so that S5's registration check reads one
    /// predicate instead of re-deriving the prefix test at each call site.
    #[must_use]
    pub fn is_reserved(&self) -> bool {
        self.0.starts_with(RESERVED_PREFIX)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Kind({:?})", &*self.0)
    }
}

impl Serialize for Kind {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

struct KindVisitor;

impl<'de> Visitor<'de> for KindVisitor {
    type Value = Kind;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "a record kind: 1..={MAX_KIND_LEN} bytes, no control characters"
        )
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Kind, E> {
        Kind::parse(v).map_err(|e| E::custom(e))
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Kind, E> {
        Err(E::invalid_type(Unexpected::Signed(v), &self))
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Kind, E> {
        Err(E::invalid_type(Unexpected::Unsigned(v), &self))
    }
}

/// Hand-written, per ADR-0030 §10's serde table: the derived impl over a private `Arc<str>` would
/// accept the empty kind and a kind containing a newline, which is exactly the bypass
/// `RUST-DESIGN-REVIEW.md:73` calls *"serde is a construction path"*.
impl<'de> serde::Deserialize<'de> for Kind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_str(KindVisitor)
    }
}
