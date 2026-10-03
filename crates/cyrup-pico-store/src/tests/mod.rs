//! Unit tests for this crate's two slices.
//!
//! S1's newtypes and S3's storage contract. The conformance suite — the cases that are the *contract*
//! rather than this backend's behaviour (`spec.md:4369`) — is not here: it lives behind
//! `feature = "conformance"` and is driven from `tests/conformance.rs`, because ADR-0030 §8 requires
//! it to be runnable by a backend crate that has never heard of this module.
//!
//! The shape of this suite is set by ADR-0030 §7: *"`Id<K>`'s and `Seq`'s hand-written
//! `Deserialize` impls each get a test rejecting a string, a float, a negative and zero"*,
//! *"`Lifetime::new` absorbs the inverted-interval case while keeping the empty lifetime legal"*,
//! and *"`PageLimit::parse` absorbs zero and over-`MAX`"*. One test per rejected shape per type,
//! named for the shape, so a failure names what was reintroduced.
//!
//! The compile-fail half lives in `tests/compile-fail/` and is driven by `tests/compile_fail.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod batch;
mod cursor;
mod error;
mod id;
mod kind;
mod lifetime;
mod memory;
mod page;
mod records;
mod seq;
mod store_id;
mod version;

use core::num::NonZeroU64;

use crate::Seq;

/// A sequence, for a test that does not care which.
pub(crate) fn seq(n: u64) -> Seq {
    Seq::new(NonZeroU64::new(n).expect("test sequences are nonzero"))
}

/// The four wire shapes ADR-0030 §10's serde table requires every numeric newtype to reject,
/// as JSON literals, plus the shapes a damaged file can also hold.
pub(crate) const REJECTED_SHAPES: &[(&str, &str)] = &[
    ("string", "\"7\""),
    ("float", "7.5"),
    ("negative", "-7"),
    ("zero", "0"),
    ("boolean", "true"),
    ("null", "null"),
    ("array", "[7]"),
    ("object", "{\"n\":7}"),
];
