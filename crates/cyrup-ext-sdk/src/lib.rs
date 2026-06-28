//! cyrup-ext-sdk — the guest SDK for authoring cyrup extensions in Rust (arch-08; binds ADR-0002).
//!
//! This crate is compiled to `wasm32-wasip2` and is **not** part of the host build graph
//! (excluded from workspace default-members). Agent-authored and third-party extensions depend on
//! it to implement the `cyrup:ext` WIT world (tools, commands, event hooks, UI commands) against
//! host-provided capabilities.
//!
//! Scaffold stub — the WIT bindings (`wit-bindgen`) and the ergonomic extension API land with
//! arch-08 implementation.
