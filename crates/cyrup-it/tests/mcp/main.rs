//! Seam tests drained from **`crates/cyrup-mcp`** — the `pi-mcp-adapter` port (gap-analysis 13).
//!
//! What makes a test belong here rather than in `crates/cyrup-mcp/src/**/#[cfg(test)]`: it needs a
//! genuinely assembled [`cyrup_session_svc::AgentSession`]. The adapter is a *native built-in*, and
//! its entire load-time contract is what it puts into a live session's tool registry during
//! `NativeExtension::init` — a contract an `InitApi` double cannot observe, because `InitApi` is
//! upstream of the `natives_to_load` gate, the trust bootstrap, and the dynamic-tool view the model
//! is actually offered. Every assertion that stops at "the extension called `register_tool`" stays
//! in the source crate; this target starts where that one stops.
//!
//! Curation note: `cyrup-mcp` is a 425-unit port landing in cuts, so this target will grow. The bar
//! is unchanged — a real session, a real process, or a real socket. A unit that can assert against
//! `register_surface` and a `TempDir` belongs in `crates/cyrup-mcp/src/registration.rs`, where 179
//! of them already live.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

#[path = "../support/mod.rs"]
mod support;

mod activation;
