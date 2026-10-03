//! Unit tests for PICO5-PLAN S7.
//!
//! The suite's shape is set by S7's own test list, and every case names the line of the specification
//! or of ADR-0030 it pins. The ones that need two processes, and the compile-fail cases, are integration
//! tests under `tests/`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

mod durability;
mod fixture;
mod lock;
mod query_plan;
mod reclaim;
mod recovery;
mod reopen;
