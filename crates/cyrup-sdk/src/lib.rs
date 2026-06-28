//! cyrup-sdk — the public embeddable API (arch-11; conformance: func-11 §7).
//!
//! The surface external embedders use to create and drive an agent session in-process — the
//! *same* seam the built-in front-ends use (func-11 R-11-023). It re-exports the facade and the
//! foundational crates so embedders depend on one crate.
//!
//! Scaffold stub: re-exports only; the constructor/runtime API lands with arch-11.

pub use cyrup_agent as agent;
pub use cyrup_core as core;
pub use cyrup_provider as provider;
pub use cyrup_session as session;
pub use cyrup_session_svc as session_svc;
