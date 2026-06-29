//! Low-level free-function agent loop (Pi `agent-loop.ts:25,31-143`, re-exported via `index.ts:4`).
//!
//! These are the embedder-facing primitives that drive the SAME turn/run loop as the high-level
//! [`crate::Agent`] without the stateful agent: an embedder supplies an [`AgentContext`] snapshot
//! and an [`AgentLoopConfig`], and either
//!
//! - pushes events to its own [`AgentEventSink`] via [`run_agent_loop`]/[`run_agent_loop_continue`]
//!   (Pi `runAgentLoop`/`runAgentLoopContinue`, agent-loop.ts:95-143), or
//! - pulls events from a returned finalizing [`EventStream`] whose result resolves to the run's new
//!   messages via [`agent_loop`]/[`agent_loop_continue`] (Pi `agentLoop`/`agentLoopContinue`,
//!   agent-loop.ts:31-93).
//!
//! Internally both construct a [`crate::agent::RunCtx`] over the supplied context and drive the
//! identical, already-tested loop, so behavior is bit-for-bit the same as the high-level agent.

use crate::agent::{EntryStart, RunCtx};
use crate::error::AgentError;
use crate::event::{AgentEvent, AgentMessage};
use crate::hooks::{DefaultHooks, Hooks};
use crate::queue::{PendingQueue, ToolExecution};
use crate::state::{GenerationConfig, StateInner};
use crate::stream_fn::{ApiKeyResolver, StreamFn};
use crate::subscriber::EventSubscriber;
use cyrup_core::{
    finalizing_channel, FinalizingStream, ModelRef, ModelThinkingLevel, RunCancel, SessionId, Tool,
};
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Pi `AgentEventSink` (agent-loop.ts:25): an async consumer the low-level loop pushes every event
/// to, in order. Mirrors the notify-only [`EventSubscriber`] shape; a sink is registered as the run
/// context's sole subscriber so it sees the full ordered stream.
#[async_trait::async_trait]
pub trait AgentEventSink: Send + Sync {
    async fn emit(&self, event: &AgentEvent);
}

/// A context snapshot handed to the low-level loop (Pi `AgentContext`, types.ts:396-404): the system
/// prompt, the transcript visible to the model, and the tools available for the run.
#[derive(Clone, Default)]
pub struct AgentContext {
    pub system_prompt: String,
    pub messages: Vec<AgentMessage>,
    pub tools: Vec<Arc<dyn Tool>>,
}

/// Per-run configuration for the low-level loop (Pi `AgentLoopConfig`, types.ts:140-282). The hook
/// methods (`convertToLlm`/`transformContext`/`beforeToolCall`/`afterToolCall`/`prepareNextTurn`/
/// `shouldStopAfterTurn`) live behind the [`Hooks`] seam; the steering/follow-up callbacks
/// (`getSteeringMessages`/`getFollowUpMessages`) are backed by the two [`PendingQueue`] handles.
#[derive(Clone)]
pub struct AgentLoopConfig {
    /// Active model for the run's first turn (Pi `config.model`, types.ts:141). Sticky
    /// `prepare_next_turn` overrides update the running baseline thereafter.
    pub model: ModelRef,
    /// Reasoning level forwarded to the provider (Pi `config.reasoning`, agent.ts:426).
    pub thinking_level: ModelThinkingLevel,
    /// The mutating lifecycle seam (Pi's `convertToLlm`/`transformContext`/tool + post-turn hooks).
    pub hooks: Arc<dyn Hooks>,
    /// Dynamic per-request key resolution (Pi `getApiKey`, types.ts:201).
    pub key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    /// Tool-execution mode (Pi `toolExecution`, types.ts:259). Default parallel.
    pub tool_execution: ToolExecution,
    /// Session id forwarded into `StreamOptions` (Pi `sessionId`).
    pub session_id: Option<SessionId>,
    /// Generation params + telemetry forwarded into `StreamOptions` (Pi `SimpleStreamOptions`).
    pub gen_config: GenerationConfig,
    /// Steering queue drained mid-run (Pi `getSteeringMessages`, types.ts:235).
    pub steering: Arc<Mutex<PendingQueue>>,
    /// Follow-up queue drained when the loop would otherwise stop (Pi `getFollowUpMessages`,
    /// types.ts:248).
    pub follow_up: Arc<Mutex<PendingQueue>>,
}

impl AgentLoopConfig {
    /// A config for `model` with all-default hooks, empty steering/follow-up queues, parallel tool
    /// execution, and no generation overrides — the minimal embedder setup.
    pub fn new(model: ModelRef) -> Self {
        Self {
            model,
            thinking_level: ModelThinkingLevel::Off,
            hooks: Arc::new(DefaultHooks),
            key_resolver: None,
            tool_execution: ToolExecution::Parallel,
            session_id: None,
            gen_config: GenerationConfig::default(),
            steering: Arc::new(Mutex::new(PendingQueue::default())),
            follow_up: Arc::new(Mutex::new(PendingQueue::default())),
        }
    }
}

/// Adapts an [`AgentEventSink`] to the loop's notify-only [`EventSubscriber`] seam so the run
/// context's existing emission path delivers every event to it, awaited in order.
struct SinkSubscriber(Arc<dyn AgentEventSink>);

#[async_trait::async_trait]
impl EventSubscriber for SinkSubscriber {
    async fn on_event(&self, event: &AgentEvent) {
        self.0.emit(event).await;
    }
}

/// Build a [`RunCtx`] over a borrowed-then-owned [`AgentContext`] + [`AgentLoopConfig`], wiring the
/// caller's sink as the sole subscriber. The initial `messages` seed `state.messages`, so the loop's
/// reducer appends prompt/turn messages onto them exactly as Pi's
/// `currentContext.messages = [...context.messages, ...prompts]` does (agent-loop.ts:104-107).
fn build_run_ctx(
    context: AgentContext,
    config: AgentLoopConfig,
    sink: Arc<dyn AgentEventSink>,
    cancel: RunCancel,
    stream_fn: Arc<dyn StreamFn>,
) -> RunCtx {
    let AgentContext { system_prompt, messages, tools } = context;
    let state = Arc::new(Mutex::new(StateInner {
        system_prompt: system_prompt.clone(),
        model: config.model.clone(),
        thinking_level: config.thinking_level,
        tools: tools.clone(),
        messages,
        is_streaming: true,
        streaming_message: None,
        pending_tool_calls: HashSet::new(),
        error_message: None,
    }));
    let subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>> =
        Arc::new(Mutex::new(vec![Arc::new(SinkSubscriber(sink))]));
    RunCtx::new(
        state,
        subscribers,
        config.steering,
        config.follow_up,
        config.hooks,
        stream_fn,
        config.key_resolver,
        config.tool_execution,
        config.session_id,
        system_prompt,
        config.model,
        config.thinking_level,
        config.gen_config,
        tools,
        cancel,
        false,
    )
}

/// Run an agent loop from a NEW set of prompt messages, pushing every event to `sink` and resolving
/// to the run's new messages (Pi `runAgentLoop`, agent-loop.ts:95-118). The prompts are appended to
/// the context and emitted (`message_start`/`message_end`) before the first assistant turn.
pub async fn run_agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    sink: Arc<dyn AgentEventSink>,
    cancel: RunCancel,
    stream_fn: Arc<dyn StreamFn>,
) -> Vec<AgentMessage> {
    let mut rc = build_run_ctx(context, config, sink, cancel, stream_fn);
    rc.run(EntryStart::Prompt(prompts)).await
}

/// Continue an agent loop from the current context WITHOUT adding a new message (Pi
/// `runAgentLoopContinue`, agent-loop.ts:120-143). The transcript must be non-empty and must NOT end
/// with an assistant message (the provider would otherwise reject the request).
pub async fn run_agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    sink: Arc<dyn AgentEventSink>,
    cancel: RunCancel,
    stream_fn: Arc<dyn StreamFn>,
) -> Result<Vec<AgentMessage>, AgentError> {
    if context.messages.is_empty() {
        return Err(AgentError::NoMessages);
    }
    if context.messages.last().map(|m| m.is_assistant()).unwrap_or(false) {
        return Err(AgentError::ContinueFromAssistant);
    }
    let mut rc = build_run_ctx(context, config, sink, cancel, stream_fn);
    Ok(rc.run(EntryStart::Continue).await)
}

/// A finalizing event stream whose terminal (`agent_end`) resolves to the run's new messages — the
/// cyrup analogue of Pi's `EventStream<AgentEvent, AgentMessage[]>` (agent-loop.ts:31-54).
pub type AgentLoopStream = FinalizingStream<AgentEvent, Vec<AgentMessage>>;

/// The shared producer half driving an [`AgentLoopStream`] from the spawned run task.
type AgentLoopSink = Arc<Mutex<cyrup_core::FinalizingSink<AgentEvent, Vec<AgentMessage>>>>;

fn agent_event_stream() -> (AgentLoopSink, AgentLoopStream) {
    let (sink, stream) = finalizing_channel(
        |e: &AgentEvent| matches!(e, AgentEvent::AgentEnd { .. }),
        |e: &AgentEvent| match e {
            AgentEvent::AgentEnd { messages } => messages.clone(),
            _ => Vec::new(),
        },
        Vec::new,
    );
    (Arc::new(Mutex::new(sink)), stream)
}

/// Pushes loop events into a finalizing [`AgentLoopStream`] for the pull-based variants.
struct StreamPushSink {
    sink: AgentLoopSink,
}

#[async_trait::async_trait]
impl AgentEventSink for StreamPushSink {
    async fn emit(&self, event: &AgentEvent) {
        if let Ok(mut s) = self.sink.lock() {
            s.push(event.clone());
        }
    }
}

/// Start an agent loop from a new prompt and return a pull-based finalizing stream of its events; the
/// stream's result resolves to the run's new messages (Pi `agentLoop`, agent-loop.ts:31-54). The run
/// is driven on a spawned task; dropping the stream stops consumption.
pub fn agent_loop(
    prompts: Vec<AgentMessage>,
    context: AgentContext,
    config: AgentLoopConfig,
    cancel: RunCancel,
    stream_fn: Arc<dyn StreamFn>,
) -> AgentLoopStream {
    let (sink, stream) = agent_event_stream();
    let push: Arc<dyn AgentEventSink> = Arc::new(StreamPushSink { sink: sink.clone() });
    tokio::spawn(async move {
        let messages = run_agent_loop(prompts, context, config, push, cancel, stream_fn).await;
        if let Ok(mut s) = sink.lock() {
            // Mirror Pi's `stream.end(messages)` (agent-loop.ts:50): close iteration and resolve the
            // result to the run's new messages (identical to the captured `agent_end` payload).
            s.end(Some(messages));
        }
    });
    stream
}

/// Continue an agent loop from the current context and return a pull-based finalizing stream of its
/// events (Pi `agentLoopContinue`, agent-loop.ts:64-93). Validates the transcript up front (same
/// rules as [`run_agent_loop_continue`]) before spawning.
pub fn agent_loop_continue(
    context: AgentContext,
    config: AgentLoopConfig,
    cancel: RunCancel,
    stream_fn: Arc<dyn StreamFn>,
) -> Result<AgentLoopStream, AgentError> {
    if context.messages.is_empty() {
        return Err(AgentError::NoMessages);
    }
    if context.messages.last().map(|m| m.is_assistant()).unwrap_or(false) {
        return Err(AgentError::ContinueFromAssistant);
    }
    let (sink, stream) = agent_event_stream();
    let push: Arc<dyn AgentEventSink> = Arc::new(StreamPushSink { sink: sink.clone() });
    tokio::spawn(async move {
        // Validation already passed; this resolves to `Ok`.
        let messages = run_agent_loop_continue(context, config, push, cancel, stream_fn)
            .await
            .unwrap_or_default();
        if let Ok(mut s) = sink.lock() {
            s.end(Some(messages));
        }
    });
    Ok(stream)
}
