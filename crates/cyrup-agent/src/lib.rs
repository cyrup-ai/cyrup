//! cyrup-agent — the turn-based agent loop (arch-02; conformance: func-02).
//!
//! Ordered event stream, parallel/sequential tool execution, the `Hooks` mutating seam +
//! notify-only `EventSubscriber`, steering/follow-up queues, abort/idle, and managed agent state.
//!
//! Entry point: [`Agent`] (built via [`AgentBuilder`]). The loop is provider-agnostic — it talks to
//! a [`StreamFn`] (use [`ProviderStreamFn`] to wrap a `cyrup_provider::Provider`). No-panic policy
//! is enforced crate-wide via `[workspace.lints]`.
#![forbid(unsafe_code)]

// MODULE-VISIBILITY POLICY. These modules are `pub` because downstream crates (notably
// cyrup-session-svc) browse them by path, so the module path is a second public API: anything
// `pub` inside them escapes the crate whether or not it is named below. The rule is therefore —
// every `pub` item inside these modules IS public API and MUST appear in the root `pub use` list
// that follows, which is the audit checklist for this crate's surface. An item that should not be
// public gets `pub(crate)` at its definition rather than being merely omitted from the list.
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
pub use error::{AgentError, BusyEntry, ContinueSurface, HookError};
pub use event::{AgentEvent, AgentMessage, ToolResultMessage, APP_MESSAGE_ROLES};
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

// THIRD-PARTY RE-EXPORT POLICY. Every `cyrup_provider`/`cyrup_core` type that appears in a public
// signature of this crate is re-exported here, so a downstream crate can drive the agent — builder
// setters, `Agent` setters, `ProviderStreamFn` — without depending on cyrup-provider or cyrup-core
// directly. A type that appears in no public signature does NOT belong in these lists. On a name
// collision with a cyrup-agent item above, the cyrup-agent name wins and the third-party one is
// dropped (none today).
pub use cyrup_provider::{
    CacheRetention, Context, HeaderMap, OnPayload, OnResponseHook, Provider, ProviderEnv,
    StreamEvent, StreamOptions, ThinkingBudgets, Transport,
};
pub use cyrup_core::{
    AssistantMessage, CancelToken, Content, EventStream, ModelRef, ModelThinkingLevel, ProviderId,
    SessionId, StopReason, Tool, ToolCallId,
};

#[cfg(test)]
mod tests;
