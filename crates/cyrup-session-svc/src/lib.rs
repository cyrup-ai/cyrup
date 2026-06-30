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

mod attribution;
mod bash;
mod builder;
mod command;
mod compact;
mod error;
mod event;
mod factory;
mod host_services;
mod hooks;
mod runtime;
mod services;
mod session;
mod state;
mod subscriber;
mod tools;

pub use attribution::merge_provider_attribution_headers;
pub use bash::{BashOptions, BashResult};
pub use builder::{NoTools, SessionBuilder, SessionConfig, SessionTarget};
pub use command::{SessionCommand, SessionCommandOutput};
pub use error::SessionServiceError;
pub use event::{
    AgentSessionEvent, DeliverAs, InputSource, PromptAccepted, StreamingBehavior, UserInput,
};
pub use factory::SessionFactory;
pub use host_services::{ControlSink, LiveHostServices};
pub use runtime::{AgentSessionRuntime, RuntimeForkResult, SwitchResult};
pub use services::AgentSessionServices;
pub use session::{
    AgentSession, ForkAnchor, ForkOutcome, ForkPosition, ModelCycleResult, ScopedModel,
};
pub use state::{CompactionResult, ContextUsage, SessionStateView, SessionStats};
pub use tools::ToolInfo;

// Load-bearing re-exports so embedders need not depend on every subsystem directly.
pub use cyrup_core::EventStream;
pub use cyrup_config::AppMode;
pub use cyrup_tools::{Availability, PermissionPolicy};
