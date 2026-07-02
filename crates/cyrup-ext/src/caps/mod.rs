//! Real, reusable capability-grant engines backing the `wasm-host`'s [`crate::host::HostServices`]
//! (arch-08 §3.6). Kept separate from both the WIT router (`host::live`, which converts bindgen
//! types at the boundary) and the capability-grant trait itself (`host::services`, which defines the
//! deny-by-default contract every backend implements) so each engine is host-agnostic and
//! unit-testable without a live wasm guest — the analog of `cyrup_tools::ProcOps` backing the `exec`
//! grant, except these engines live in-crate (no cross-crate DTO duplication needed).
//!
//! `http` (outbound HTTP, pi-mcp-adapter-port.md §3.2) is the first; future capability additions
//! (`proc`, `tcp-listen`, `browser` — see the same spec's §3.1/§3.3) are expected to land the same
//! way, one submodule per capability.

pub mod http;
