//! Unit tests for S2's functional core. **All pure**: no runtime, no storage, no temp files.
//!
//! The shape of the suite is set by ADR-0030 §7 and PICO5-PLAN S2:
//!
//! * *"`DocValue::parse` ... **must carry a named negative test for `f64::NAN`** — that test is the
//!   artefact that stops someone reintroducing `serde_json::to_value`"*: [`parse`]'s
//!   `nan_is_rejected_rather_than_becoming_null`, which also pins the upstream behaviour it guards
//!   against by asserting what `serde_json` does.
//! * *"It also needs a depth-limit test"*: [`parse`]'s two depth cases.
//! * *"`ReplayPlan::parse` absorbs the two corruption rules ... as two `Err` arms with literal record
//!   sets and no storage, so `materialize` is then a pure total function tested with table cases"*:
//!   [`replay`].
//! * *"Structural sharing asserted by `Arc::ptr_eq` on an untouched subtree before and after a
//!   change"*: [`sharing`].
//! * *"One canary test that the authority revision is unchanged after a draft write (the
//!   refcount-≥-2 invariant of the `Change` constructor)"*: [`sharing`]'s
//!   `authority_revision_is_untouched_by_a_draft_write`.
//!
//! The compile-fail half lives in `tests/compile-fail/` and is driven by `tests/compile_fail.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod apply;
mod checkpoint;
mod op_wire;
mod parse;
mod path;
mod replay;
mod sharing;
mod version;

use core::num::NonZeroU32;

use crate::{DefVersion, DocMap, DocRoot, DocValue, Op, OpBatch, Path, StoredContent};

/// A version, for a test that does not care which.
pub(crate) fn ver(n: u32) -> DefVersion {
    DefVersion::new(NonZeroU32::new(n).expect("test versions are nonzero"))
}

/// A path of object keys.
pub(crate) fn p(keys: &[&str]) -> Path {
    Path::keys(keys.iter().copied()).expect("test paths are safe")
}

/// A root from `(key, value)` pairs.
pub(crate) fn root(entries: &[(&str, DocValue)]) -> DocRoot {
    let mut map = DocMap::new();
    for (k, v) in entries {
        map.insert((*k).into(), v.clone());
    }
    DocRoot::from_map(map)
}

/// A list value.
pub(crate) fn list(items: &[DocValue]) -> DocValue {
    DocValue::List(items.to_vec().into())
}

/// A base record.
pub(crate) fn base(version: u32, value: DocRoot) -> StoredContent {
    StoredContent::Base {
        version: ver(version),
        value,
    }
}

/// A delta record.
pub(crate) fn delta(version: u32, ops: &[Op]) -> StoredContent {
    StoredContent::Delta {
        version: ver(version),
        ops: OpBatch::new(ops.to_vec()),
    }
}
