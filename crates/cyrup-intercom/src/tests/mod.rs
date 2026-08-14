//! Crate-internal test modules.
//!
//! These were formerly `tests/*.rs` integration binaries. Cargo compiles every file under
//! `tests/` into its OWN binary and runs it as its OWN process; for host-independent assertions
//! (pure serde on the wire types) that cost buys nothing, so they live here as ordinary
//! `#[cfg(test)]` modules of the library instead. Only the tests that genuinely need a SEAM (a
//! spawned broker process, a hostile `UnixListener`, a built artifact) remain under `tests/`.
//!
//! Note the bar is "no process-global state", not merely "no subprocess": `cargo test` runs the
//! whole crate's unit tests as parallel threads in ONE process, so a test that mutates the process
//! environment is only isolated while it owns a `tests/` binary of its own. Neither module below
//! touches env — both are pure serde over the wire types.
//!
//! The assertions themselves are unchanged from their integration-test form — only the crate
//! self-reference (`cyrup_intercom::…` → `crate::…`) was rewritten.

mod protocol_number_overflow;
mod protocol_residual_parity;
