//! Crate-internal test modules (relocated from `tests/` so the whole crate's tests
//! build and run as ONE binary instead of one process per file).

// The workspace DENIES these four (see `/Cargo.toml` `[workspace.lints.clippy]`); test code is
// exactly where panicking on a broken invariant is the correct reaction, so the whole tree opts
// out once here — lint levels propagate down the module tree to every descendant file.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

mod json_event;
mod modes;
mod rpc_agent_settled;
mod rpc_client;
mod rpc_host_seam;
mod rpc_output_decoupling;
