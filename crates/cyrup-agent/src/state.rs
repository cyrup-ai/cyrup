//! In-memory agent state + the event reducer (arch-02 §4.1 / func-02 §10).

use crate::event::{AgentEvent, AgentMessage};
use cyrup_core::{ModelRef, StopReason, ModelThinkingLevel, Tool, ToolCallId};
use cyrup_provider::{CacheRetention, OnPayload, OnResponseHook, ThinkingBudgets, Transport};
use std::collections::HashSet;
use std::sync::Arc;

/// The per-run model-call configuration the agent forwards into `cyrup_provider::StreamOptions`
/// (Pi `AgentOptions`/`AgentLoopConfig` generation params, agent.ts:96-116). All fields are
/// `None`/default unless the builder sets them, so the provider keeps its own defaults (additive,
/// backward-compatible). Captured once at run start (Pi `createLoopConfig`, agent.ts:421-447).
#[derive(Clone, Default)]
pub struct GenerationConfig {
    /// Sampling temperature (Pi `SimpleStreamOptions.temperature`).
    pub temperature: Option<f32>,
    /// Max output tokens (Pi `SimpleStreamOptions.maxTokens`).
    pub max_tokens: Option<u64>,
    /// Prompt-cache retention preference (Pi `SimpleStreamOptions.cacheRetention`).
    pub cache_retention: Option<CacheRetention>,
    /// Per-request header overlay (Pi `SimpleStreamOptions.headers`).
    pub headers: Option<cyrup_provider::HeaderMap>,
    /// Preferred transport (Pi `AgentOptions.transport`, agent.ts:118/228). Pi defaults to `auto`.
    ///
    /// This is only the BUILD-TIME seed: pi keeps `transport` as a mutable public field on the
    /// `Agent` (`agent.ts:204`) that `/settings` writes live (`interactive-mode.ts:4213-4216`), so
    /// the authoritative value lives in [`StateInner::transport`] and this seeds it in
    /// `AgentBuilder::build`.
    pub transport: Option<Transport>,
    /// Cap (ms) on server-requested retry delays (Pi `AgentOptions.maxRetryDelayMs`, agent.ts:114).
    pub max_retry_delay_ms: Option<u64>,
    /// Max client-side retry attempts (Pi `SimpleStreamOptions.maxRetries`).
    pub max_retries: Option<u32>,
    /// Per-level custom thinking token budgets (Pi `AgentOptions.thinkingBudgets`, agent.ts:112).
    /// Forwarded into `cyrup_provider::StreamOptions.thinking_budgets`; budget-based providers
    /// (e.g. anthropic-messages) honor it, others ignore it.
    pub thinking_budgets: Option<ThinkingBudgets>,
    /// Static API key fallback used when no dynamic resolver yields one (Pi `config.apiKey`
    /// fallback, agent-loop.ts:301-302).
    pub api_key: Option<String>,
    /// Telemetry: inspect/replace the provider payload before sending (Pi `onPayload`, agent.ts:102).
    pub on_payload: Option<OnPayload>,
    /// Telemetry: invoked after the HTTP response arrives, before its body is read (Pi `onResponse`,
    /// agent.ts:103).
    pub on_response: Option<OnResponseHook>,
    /// Provider-scoped env overlay forwarded into `cyrup_provider::StreamOptions.env` (Pi
    /// `StreamOptions.env`, types.ts:184). The session builder seeds it with the `httpProxy` setting
    /// (`HTTP_PROXY`/`HTTPS_PROXY`) so the provider's proxy resolver honors the configured proxy (Pi
    /// `applyHttpProxySettings`, main.ts:744). Empty/absent ⇒ the ambient process env is used.
    pub env: Option<cyrup_provider::ProviderEnv>,
    /// HTTP request idle timeout (ms) forwarded into `cyrup_provider::StreamOptions.timeout_ms` (Pi
    /// `StreamOptions.timeoutMs` / `configureHttpDispatcher`, main.ts:745).
    ///
    /// Bounds the wait for the response head and, once streaming, the gap between body frames — it
    /// is NOT a deadline on the whole generation. `None` inherits the process-global default that
    /// `cyrup_provider::configure_http_idle_timeout` installs (Pi's global undici dispatcher, 5
    /// minutes); `Some(0)` disables the timeout entirely, matching `httpIdleTimeoutMs: 0` /
    /// `"disabled"` upstream.
    ///
    /// This previously claimed the value applied only to "providers that support it" and that
    /// `None`/`0` meant "no cap". Both were wrong about Pi *and* about cyrup: Pi's
    /// `configureHttpDispatcher` is called unconditionally at startup (`cli.ts:18`, `main.ts:538`)
    /// and bounds every provider connection at 5 minutes by default, and cyrup now does the same
    /// for every wire API — the value is honored by the shared SSE transport, not per provider.
    pub timeout_ms: Option<u64>,
}

/// Live agent state (arch-02 §4.1). Mutated only by the loop's reducer ([`reduce`]) and the
/// `Agent` setters; the state lock is never held across a subscriber `await`.
pub struct StateInner {
    pub system_prompt: String,
    pub model: ModelRef,
    pub thinking_level: ModelThinkingLevel,
    pub tools: Vec<Arc<dyn Tool>>,
    pub messages: Vec<AgentMessage>,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: HashSet<ToolCallId>,
    pub error_message: Option<String>,
    /// Per-request header overlay (pi `SimpleStreamOptions.headers`), LIVE rather than fixed at
    /// build. pi recomputes provider-attribution and session-affinity headers inside `streamFn`
    /// (`sdk.ts:318-327`), dispatched on the model the request is actually going to; holding them in
    /// `GenerationConfig` froze them at session build, so a cross-provider `/model` switch kept
    /// sending the PREVIOUS provider's attribution headers.
    pub headers: Option<cyrup_provider::HeaderMap>,
    /// Preferred transport (pi `SimpleStreamOptions.transport`), LIVE rather than fixed at build.
    ///
    /// pi models this as a mutable PUBLIC field on the agent — `public transport: Transport`
    /// (`agent.ts:204`), seeded from `AgentOptions.transport ?? "auto"` (`agent.ts:228`) and read
    /// back when the loop config is assembled at run start (`createLoopConfig`, `agent.ts:442`).
    /// The `/settings` "Transport" row writes it directly: `this.settingsManager.setTransport(t);
    /// this.session.agent.transport = t` (`interactive-mode.ts:4213-4216`). Holding it only in
    /// `GenerationConfig` froze it at session build, so cycling the row persisted JSON and changed
    /// nothing until the next process start.
    pub transport: Option<Transport>,
}

impl StateInner {
    pub fn snapshot(&self) -> AgentStateSnapshot {
        AgentStateSnapshot {
            system_prompt: self.system_prompt.clone(),
            model: self.model.clone(),
            thinking_level: self.thinking_level,
            messages: self.messages.clone(),
            tool_count: self.tools.len(),
            is_streaming: self.is_streaming,
            streaming_message: self.streaming_message.clone(),
            pending_tool_calls: self.pending_tool_calls.iter().cloned().collect(),
            error_message: self.error_message.clone(),
            headers: self.headers.clone(),
        }
    }
}

/// A `Clone` read-only projection of state (arch-02 §4.1). Consumers never hold the state lock;
/// mutating a snapshot does NOT affect agent state.
#[derive(Clone, Debug)]
pub struct AgentStateSnapshot {
    pub system_prompt: String,
    pub model: ModelRef,
    pub thinking_level: ModelThinkingLevel,
    pub messages: Vec<AgentMessage>,
    pub tool_count: usize,
    pub is_streaming: bool,
    pub streaming_message: Option<AgentMessage>,
    pub pending_tool_calls: Vec<ToolCallId>,
    pub error_message: Option<String>,
    /// The live per-request header overlay (pi `SimpleStreamOptions.headers`).
    pub headers: Option<cyrup_provider::HeaderMap>,
}

/// Reduce one event into managed state (arch-02 §5.1). Cheap and synchronous; called while the
/// state lock is held, BEFORE subscribers are awaited.
///
/// - `streamingMessage` holds the partial assistant message between start..end (R-02-040).
/// - the transcript grows on `message_end` (so the assistant message is visible to `before_tool_call`
///   preflight that runs after the assistant `message_end` barrier, R-02-033).
/// - `pendingToolCalls` tracks ids between tool start..end (R-02-041).
/// - `errorMessage` is set on an error/aborted turn (R-02-042).
pub fn reduce(st: &mut StateInner, ev: &AgentEvent) {
    match ev {
        AgentEvent::MessageStart { message } => {
            if message.is_assistant() {
                st.streaming_message = Some(message.clone());
            }
        }
        AgentEvent::MessageUpdate { message, .. } => {
            st.streaming_message = Some(message.clone());
        }
        AgentEvent::MessageEnd { message } => {
            st.streaming_message = None;
            st.messages.push(message.clone());
        }
        AgentEvent::ToolExecutionStart { tool_call_id, .. } => {
            st.pending_tool_calls.insert(tool_call_id.clone());
        }
        AgentEvent::ToolExecutionEnd { tool_call_id, .. } => {
            st.pending_tool_calls.remove(tool_call_id);
        }
        AgentEvent::TurnEnd { message, .. } => {
            if let AgentMessage::Assistant(a) = message
                && matches!(a.stop_reason, StopReason::Error | StopReason::Aborted) {
                    st.error_message =
                        a.error_message.clone().or_else(|| Some("turn ended with error".to_string()));
                }
        }
        AgentEvent::AgentEnd { .. } => {
            st.streaming_message = None;
            st.pending_tool_calls.clear();
        }
        AgentEvent::AgentStart | AgentEvent::TurnStart | AgentEvent::ToolExecutionUpdate { .. } => {}
    }
}
