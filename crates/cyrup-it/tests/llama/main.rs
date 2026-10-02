//! Seam tests for **`crates/cyrup-llama`** and the host it plugs into: pi's bundled llama.cpp
//! support (EXT-027, `tmp/pi/packages/coding-agent/src/extensions/llama/`) proven against a loopback
//! fake `llama-server` in router mode.
//!
//! What makes a test belong here rather than in `crates/cyrup-llama/src/tests/`: it needs the
//! REAL host. The unit tests there drive the provider, the client and the `/llama` flow against a
//! fake host; this target assembles a real session (or spawns the real binary), attaches the
//! extension the way `cyrup` does, and asserts on what a user and a server can observe.
//!
//! * [`session_chain`] - a real `AgentSession` built through `cyrup::session_launch::build_factory`:
//!   discovery, a streamed turn with `enable_thinking`, availability, `--no-extensions`, and the
//!   hidden startup listing.
//! * [`classify`] - the classifier twin and `Models::classify` against the fake's `/tokenize`,
//!   `/apply-template` and `/completion`.
//! * [`binary`] - the spawned `cyrup`: `--list-models`, a one-shot turn, `/llama` over RPC.
//!
//! Curation note: the fake in [`fake`] is the only fake. A test that needs a second server shape
//! belongs in `cyrup-llama`'s own fake, where the router management api (load, unload, download,
//! SSE) already lives.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

mod binary;
mod classify;
mod fake;
mod fixture;
mod session_chain;
