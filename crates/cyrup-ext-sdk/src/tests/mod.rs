//! In-crate unit tests (relocated from `crates/cyrup-ext-sdk/tests/`).
//!
//! A file in a crate-level `tests/` directory would be a separate Cargo integration-test BINARY
//! and process; these assertions are pure host-target checks over the guest event payload structs
//! and the ergonomic layer, and need no seam, so they compile with the library instead.
//!
//! The crate therefore has no `tests/` directory: every unit test lives here, under `src/tests/`,
//! so `cargo test -p cyrup-ext-sdk` runs all of them with no feature gate. That includes
//! `ergonomic.rs`, the host-target suite over the ergonomic guest layer — it still drives
//! the SDK through `crate::prelude::*`, the very re-export module an extension author imports,
//! rather than reaching for private items, because the author-surface property is the point of
//! those tests; being in-crate only changes how they are compiled, not what they touch.
//!
//! Assertions are unchanged from the integration-test originals; only the crate self-reference
//! moved (`cyrup_ext_sdk::X` -> `crate::X`).

mod payload_fidelity;
mod dialog_options_timeout;
mod world_import_coverage;
mod ergonomic;
mod prelude_export_parity;
