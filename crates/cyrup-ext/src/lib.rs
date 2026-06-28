//! cyrup-ext — the WASM extension host (arch-08; conformance: func-08; binds ADR-0002).
//!
//! Wasmtime Component Model host + WIT world, capability scoping + epoch/fuel preemption +
//! memory limits, subscription-gated event dispatch, native built-in extension registry, and the
//! Tier-1 build/artifact-cache loop. Bridges to the agent via `cyrup_agent::Hooks` (mutating) and
//! `cyrup_agent::EventSubscriber` (notify).
//!
//! Scaffold stub (wasmtime/wit-bindgen added during arch-08 implementation).

/// Extension host error (arch-08 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum ExtError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}
