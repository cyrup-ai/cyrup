//! In-crate unit tests (relocated from `crates/cyrup-ext-sdk/tests/`).
//!
//! Each file under `tests/` is a separate Cargo integration-test BINARY and process; these
//! assertions are pure host-target checks over the guest event payload structs and need no seam,
//! so they compile with the library instead.
//!
//! `tests/ergonomic.rs` deliberately STAYS external: it exercises the SDK strictly through the
//! public API surface an extension author sees, and that public-API-surface property is the point
//! of the test.
//!
//! Assertions are unchanged from the integration-test original; only the crate self-reference
//! moved (`cyrup_ext_sdk::X` -> `crate::X`).

mod payload_fidelity;
mod dialog_options_timeout;
