//! Shared fixtures for every `[[test]]` target in this crate.
//!
//! Included from each target's `main.rs` as
//!
//! ```ignore
//! #[path = "../support/mod.rs"]
//! mod support;
//! ```
//!
//! rather than living in `cyrup-test-support`, for one reason: everything here is about THIS
//! crate's own build script (`CYRUP_IT_BIN_*`) or about the shape of a spawned child, and none of
//! it is usable from a `#[cfg(test)]` module inside a source crate. Anything that IS usable there
//! belongs in `cyrup-test-support` instead — see docs/TEST-ARCHITECTURE.md §3.3, which counts 66
//! hand-rolled `fixture()`s, 41 `base_config()`s and 21 `fixture_binary_path()`s as the cost of
//! getting that split wrong. Add here only what a source crate could not use.
//!
//! Nothing in this module mutates the process environment. `std::env::set_var`/`remove_var` became
//! `unsafe` in edition 2024 and std's own conclusion is that *"the only sound option is to not use
//! set_var or remove_var in multi-threaded programs at all"* — and a consolidated test binary is
//! exactly such a program. Everything here is per-`Command` (§4 R2) or per-`TempDir` (§4 R1).

// The suite is migrated crate-by-crate, so at any moment most of these helpers are unused by most
// targets. That is not a defect and must not be a warning.
#![allow(dead_code)]

pub mod bins;
pub mod env;
pub mod scratch;
