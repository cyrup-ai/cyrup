//! cyrup-sdk — the public, embeddable API (arch-11; conformance: func-11 §7, R-11-019/023).
//!
//! A clean, ergonomic, **stable** surface for running cyrup in-process. It is a thin, documented
//! layer over [`cyrup_session_svc::AgentSession`] — the single integration seam every built-in
//! front-end consumes (R-11-023) — and adds **no behaviour**: it does not reimplement the agent
//! loop, persistence, tools, or extensions. It only wraps the facade with embedder-friendly
//! ergonomics and re-exports the load-bearing types so embedders need not depend on internal crates.
//!
//! # Quick start
//! ```no_run
//! use std::sync::Arc;
//! use cyrup_sdk::{Cyrup, SessionConfig};
//!
//! # async fn demo(provider: Arc<dyn cyrup_provider::Provider>) -> cyrup_sdk::SdkResult<()> {
//! // 1. Describe the run (cwd + agent dir + defaults).
//! let config = SessionConfig::new(".", "/home/me/.cyrup/agent");
//!
//! // 2. Build a session over a resolved provider.
//! let session = Cyrup::builder().build_session(provider, config).await?;
//!
//! // 3. Run a prompt to completion and read the final assistant text.
//! let answer = session.run("explain this codebase").await?;
//! println!("{answer}");
//! # Ok(()) }
//! ```
//!
//! # Streaming
//! ```no_run
//! # use futures::StreamExt;
//! # async fn demo(session: &cyrup_sdk::Session) -> cyrup_sdk::SdkResult<()> {
//! let mut events = session.prompt("write a haiku").await?;
//! while let Some(event) = events.next().await {
//!     println!("{}", event.kind());
//! }
//! # Ok(()) }
//! ```
//!
//! # What it re-exports
//! The load-bearing types an embedder needs are re-exported here (and in [`prelude`]):
//! [`AgentSessionEvent`], [`UserInput`], [`SessionConfig`], [`SessionTarget`], [`EventStream`],
//! [`SessionServiceError`], plus [`InputSource`], [`PromptAccepted`], [`StreamingBehavior`], and
//! [`AgentSessionServices`]. The foundational crates are also re-exported as modules
//! ([`agent`], [`core`], [`provider`], [`session`], [`session_svc`]).
#![forbid(unsafe_code)]

mod client;
mod error;
mod handle;

pub mod prelude;

// ---- the embedding surface ----
pub use client::{Cyrup, CyrupBuilder};
pub use error::{SdkError, SdkResult};
pub use handle::Session;

// ---- load-bearing seam types, re-exported so embedders depend only on this crate ----
pub use cyrup_session_svc::{
    AgentSession, AgentSessionEvent, AgentSessionServices, InputSource, PromptAccepted,
    SessionBuilder, SessionConfig, SessionServiceError, SessionTarget, StreamingBehavior, UserInput,
};

/// The single streaming primitive (arch-00 §3.1): every event feed is an `EventStream<T>`.
pub use cyrup_core::EventStream;

// ---- foundational crates, re-exported as modules for full access when needed ----
pub use cyrup_agent as agent;
pub use cyrup_core as core;
pub use cyrup_provider as provider;
pub use cyrup_session as session;
pub use cyrup_session_svc as session_svc;
