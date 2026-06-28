//! cyrup-agent — the turn-based agent loop (arch-02; conformance: func-02).
//!
//! Ordered event stream, parallel/sequential tool execution, the `Hooks` mutating seam +
//! notify-only `EventSubscriber`, steering/follow-up queues, abort/idle, and managed agent state.
//!
//! Entry point: [`Agent`] (built via [`AgentBuilder`]). The loop is provider-agnostic — it talks to
//! a [`StreamFn`] (use [`ProviderStreamFn`] to wrap a `cyrup_provider::Provider`). No-panic policy
//! is enforced crate-wide via `[workspace.lints]`.
#![forbid(unsafe_code)]

pub mod agent;
pub mod error;
pub mod event;
pub mod hooks;
pub mod queue;
pub mod state;
pub mod stream_fn;
pub mod subscriber;

pub use agent::{Agent, AgentBuilder, PromptInput, RunHandle};
pub use error::{AgentError, HookError};
pub use event::{AgentEvent, AgentMessage, ToolResultMessage};
pub use hooks::{
    default_convert_to_llm, AfterOverride, AfterToolCall, BeforeOutcome, BeforeToolCall,
    DefaultHooks, Hooks, PostTurn, TurnUpdate,
};
pub use queue::{PendingQueue, QueueMode, ToolExecution};
pub use state::AgentStateSnapshot;
pub use stream_fn::{ApiKeyResolver, ProviderStreamFn, StreamFn};
pub use subscriber::EventSubscriber;

// Re-export the load-bearing provider/core types the agent's public API exposes, so downstream
// crates can drive the agent without depending on cyrup-provider directly for these.
pub use cyrup_provider::{Context, StreamEvent, StreamOptions, ToolDef};
