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
pub mod loop_fn;
pub mod proxy;
pub mod queue;
pub mod state;
pub mod stream_fn;
pub mod subscriber;

pub use agent::{Agent, AgentBuilder, HeaderFn, PromptInput, RunHandle, Subscription};
pub use error::{AgentError, HookError};
pub use event::{AgentEvent, AgentMessage, ToolResultMessage};
pub use loop_fn::{
    agent_loop, agent_loop_continue, run_agent_loop, run_agent_loop_continue, AgentContext,
    AgentEventSink, AgentLoopConfig, AgentLoopStream,
};
pub use hooks::{
    default_convert_to_llm, AfterOverride, AfterToolCall, AgentContextView, BeforeOutcome,
    BeforeToolCall, DefaultHooks, Hooks, PostTurn, TurnUpdate,
};
pub use proxy::{
    stream_proxy, ProxyAssistantMessageEvent, ProxyMessageBuilder, ProxyStreamFn, ProxyStreamOptions,
};
pub use queue::{PendingQueue, QueueMode, ToolExecution};
pub use state::{AgentStateSnapshot, GenerationConfig};
pub use stream_fn::{ApiKeyResolver, ProviderStreamFn, StreamFn};
pub use subscriber::EventSubscriber;

// Re-export the load-bearing provider/core types the agent's public API exposes, so downstream
// crates can drive the agent without depending on cyrup-provider directly for these.
pub use cyrup_provider::{Context, StreamEvent, StreamOptions, ToolDef};

#[cfg(test)]
mod tests;
