//! [`AgentBuilder`] — assembly of an [`Agent`] and its initial managed state.

use super::Agent;
use crate::event::AgentMessage;
use crate::hooks::{DefaultHooks, Hooks};
use crate::queue::{PendingQueue, QueueMode, ToolExecution};
use crate::state::{GenerationConfig, StateInner};
use crate::stream_fn::{ApiKeyResolver, StreamFn};
use cyrup_core::{ModelRef, ModelThinkingLevel, SessionId, Tool};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

/// Builder for [`Agent`].
pub struct AgentBuilder {
    system_prompt: String,
    model: ModelRef,
    thinking_level: ModelThinkingLevel,
    tools: Vec<Arc<dyn Tool>>,
    messages: Vec<AgentMessage>,
    hooks: Option<Arc<dyn Hooks>>,
    stream_fn: Arc<dyn StreamFn>,
    key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
    gen_config: GenerationConfig,
}

impl AgentBuilder {
    #[must_use]
    pub fn new(model: ModelRef, stream_fn: Arc<dyn StreamFn>) -> Self {
        Self {
            system_prompt: String::new(),
            model,
            thinking_level: ModelThinkingLevel::Off,
            tools: Vec::new(),
            messages: Vec::new(),
            hooks: None,
            stream_fn,
            key_resolver: None,
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            tool_execution: ToolExecution::Parallel,
            session_id: None,
            // Pi defaults `transport` to `"auto"` (agent.ts:217); every other gen param is unset so
            // the provider keeps its own defaults.
            gen_config: GenerationConfig {
                transport: Some(cyrup_provider::Transport::Auto),
                ..GenerationConfig::default()
            },
        }
    }

    #[must_use]
    pub fn system_prompt(mut self, s: impl Into<String>) -> Self {
        self.system_prompt = s.into();
        self
    }

    #[must_use]
    pub fn thinking_level(mut self, t: ModelThinkingLevel) -> Self {
        self.thinking_level = t;
        self
    }

    #[must_use]
    pub fn tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    pub fn messages(mut self, messages: Vec<AgentMessage>) -> Self {
        self.messages = messages;
        self
    }

    #[must_use]
    pub fn hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }

    #[must_use]
    pub fn key_resolver(mut self, r: Arc<dyn ApiKeyResolver>) -> Self {
        self.key_resolver = Some(r);
        self
    }

    #[must_use]
    pub fn steering_mode(mut self, mode: QueueMode) -> Self {
        self.steering_mode = mode;
        self
    }

    #[must_use]
    pub fn follow_up_mode(mut self, mode: QueueMode) -> Self {
        self.follow_up_mode = mode;
        self
    }

    #[must_use]
    pub fn tool_execution(mut self, mode: ToolExecution) -> Self {
        self.tool_execution = mode;
        self
    }

    #[must_use]
    pub fn session_id(mut self, id: SessionId) -> Self {
        self.session_id = Some(id);
        self
    }

    // --- generation params + telemetry (Pi `AgentOptions`, agent.ts:96-116) ---

    /// Sampling temperature forwarded to the provider (Pi `SimpleStreamOptions.temperature`).
    #[must_use]
    pub fn temperature(mut self, t: f32) -> Self {
        self.gen_config.temperature = Some(t);
        self
    }

    /// Max output tokens forwarded to the provider (Pi `SimpleStreamOptions.maxTokens`).
    #[must_use]
    pub fn max_tokens(mut self, n: u64) -> Self {
        self.gen_config.max_tokens = Some(n);
        self
    }

    /// Prompt-cache retention preference (Pi `SimpleStreamOptions.cacheRetention`).
    #[must_use]
    pub fn cache_retention(mut self, r: cyrup_provider::CacheRetention) -> Self {
        self.gen_config.cache_retention = Some(r);
        self
    }

    /// Per-request header overlay (Pi `SimpleStreamOptions.headers`).
    #[must_use]
    pub fn headers(mut self, h: cyrup_provider::HeaderMap) -> Self {
        self.gen_config.headers = Some(h);
        self
    }

    /// Preferred transport (Pi `AgentOptions.transport`, agent.ts:113).
    #[must_use]
    pub fn transport(mut self, t: cyrup_provider::Transport) -> Self {
        self.gen_config.transport = Some(t);
        self
    }

    /// Cap (ms) on server-requested retry delays (Pi `AgentOptions.maxRetryDelayMs`, agent.ts:114).
    #[must_use]
    pub fn max_retry_delay_ms(mut self, ms: u64) -> Self {
        self.gen_config.max_retry_delay_ms = Some(ms);
        self
    }

    /// Max client-side retry attempts (Pi `SimpleStreamOptions.maxRetries`).
    #[must_use]
    pub fn max_retries(mut self, n: u32) -> Self {
        self.gen_config.max_retries = Some(n);
        self
    }

    /// Per-level custom thinking token budgets (Pi `AgentOptions.thinkingBudgets`, agent.ts:112).
    /// Forwarded into `cyrup_provider::StreamOptions.thinking_budgets`; budget-based providers honor
    /// it, others ignore it.
    #[must_use]
    pub fn thinking_budgets(mut self, b: cyrup_provider::ThinkingBudgets) -> Self {
        self.gen_config.thinking_budgets = Some(b);
        self
    }

    /// Static API-key fallback used when no dynamic [`ApiKeyResolver`] yields one (Pi `config.apiKey`
    /// fallback, agent-loop.ts:301-302).
    #[must_use]
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.gen_config.api_key = Some(key.into());
        self
    }

    /// Provider-scoped env overlay forwarded into `StreamOptions.env` (Pi `StreamOptions.env`,
    /// types.ts:184): the session builder seeds it with the `httpProxy` setting so the provider's
    /// proxy resolver honors the configured proxy (Pi `applyHttpProxySettings`, main.ts:744).
    #[must_use]
    pub fn provider_env(mut self, env: cyrup_provider::ProviderEnv) -> Self {
        self.gen_config.env = Some(env);
        self
    }

    /// Provider request metadata forwarded into `StreamOptions.metadata` (Pi
    /// `SimpleStreamOptions.metadata`, e.g. Anthropic `user_id`). AGENT-S03.
    #[must_use]
    pub fn metadata(mut self, m: serde_json::Map<String, Value>) -> Self {
        self.gen_config.metadata = Some(m);
        self
    }

    /// WebSocket connect (handshake) timeout in ms forwarded into
    /// `StreamOptions.websocket_connect_timeout_ms` (Pi
    /// `SimpleStreamOptions.websocketConnectTimeoutMs`, `packages/ai/src/types.ts:159`; the session
    /// seeds it from `settingsManager.getWebSocketConnectTimeoutMs()` in `sdk.ts`). AGENT-031.
    #[must_use]
    pub fn websocket_connect_timeout_ms(mut self, ms: u64) -> Self {
        self.gen_config.websocket_connect_timeout_ms = Some(ms);
        self
    }

    /// HTTP request idle timeout (ms) forwarded into `StreamOptions.timeout_ms` (Pi
    /// `configureHttpDispatcher(getHttpIdleTimeoutMs())`, main.ts:745).
    ///
    /// Honored by the shared SSE transport for every wire API — see
    /// [`GenerationConfig::timeout_ms`](crate::state::GenerationConfig::timeout_ms) for the exact semantics
    /// (idle, not total; `0` disables).
    #[must_use]
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.gen_config.timeout_ms = Some(ms);
        self
    }

    /// Telemetry: inspect/replace the provider payload before sending (Pi `AgentOptions.onPayload`,
    /// agent.ts:102).
    #[must_use]
    pub fn on_payload(mut self, f: cyrup_provider::OnPayload) -> Self {
        self.gen_config.on_payload = Some(f);
        self
    }

    /// Telemetry: invoked after the HTTP response arrives, before its body is read (Pi
    /// `AgentOptions.onResponse`, agent.ts:103).
    #[must_use]
    pub fn on_response(mut self, f: cyrup_provider::OnResponseHook) -> Self {
        self.gen_config.on_response = Some(f);
        self
    }

    #[must_use]
    pub fn build(self) -> Agent {
        let (running_tx, running_rx) = watch::channel(false);
        let state = StateInner {
            system_prompt: self.system_prompt,
            model: self.model,
            thinking_level: self.thinking_level,
            tools: self.tools,
            messages: self.messages,
            is_streaming: false,
            streaming_message: None,
            pending_tool_calls: HashSet::new(),
            error_message: None,
            // Seeded from the builder, then kept LIVE by `set_headers`.
            headers: self.gen_config.headers.clone(),
            // Same shape for `transport` (pi's public `agent.transport` field, agent.ts:204/228):
            // seeded from `AgentBuilder::transport`, then kept LIVE by `set_transport`.
            transport: self.gen_config.transport,
        };
        Agent {
            state: Arc::new(Mutex::new(state)),
            subscribers: Arc::new(Mutex::new(Vec::new())),
            steering: Arc::new(Mutex::new(PendingQueue::new(self.steering_mode))),
            follow_up: Arc::new(Mutex::new(PendingQueue::new(self.follow_up_mode))),
            hooks: self.hooks.unwrap_or_else(|| Arc::new(DefaultHooks)),
            stream_fn: self.stream_fn,
            key_resolver: self.key_resolver,
            cancel_slot: Arc::new(Mutex::new(None)),
            running_tx,
            running_rx,
            tool_execution: self.tool_execution,
            session_id: self.session_id,
            header_fn: Arc::new(Mutex::new(None)),
            gen_config: self.gen_config,
        }
    }
}
