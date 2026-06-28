//! cyrup-session-svc — the `AgentSession` facade (arch-11; the single integration seam).
//!
//! Wires provider + agent + tools + session + config + resources + ext into the one surface every
//! front-end (`cyrup-tui`/`cyrup-modes`/`cyrup-sdk`) and embedder consumes (func-11 R-11-023).
//!
//! ## The seam
//! - [`SessionBuilder`] (+ [`SessionConfig`]/[`SessionTarget`]) assembles a run from the real
//!   subsystems: resolve settings + trust + auth + model; discover resources; build the tool
//!   registry with isolation decorators + permission policy; open/create the session tree and wire
//!   compaction; assemble the system prompt via `SystemPromptBuilder` + `ContextStore`; build the
//!   extension host with native built-ins and attach BOTH ext seams (`EventSubscriber` + `Hooks`)
//!   to the agent; resolve the provider into the agent loop.
//! - [`AgentSession`] is the runtime surface: `prompt` (→ an [`EventStream`] of
//!   [`AgentSessionEvent`]), `steer`/`follow_up`, `abort`, `compact`, `branch`/`fork`, `set_model`,
//!   `wait_for_idle` — with durable persistence across every turn.
//! - [`SessionServiceError`] aggregates the subsystem errors.
#![forbid(unsafe_code)]

mod builder;
mod compact;
mod error;
mod event;
mod hooks;
mod services;
mod session;
mod subscriber;

pub use builder::{SessionBuilder, SessionConfig, SessionTarget};
pub use error::SessionServiceError;
pub use event::{AgentSessionEvent, InputSource, PromptAccepted, StreamingBehavior, UserInput};
pub use services::AgentSessionServices;
pub use session::AgentSession;

// Load-bearing re-exports so embedders need not depend on every subsystem directly.
pub use cyrup_core::EventStream;
pub use cyrup_config::AppMode;
pub use cyrup_tools::{Availability, PermissionPolicy};
