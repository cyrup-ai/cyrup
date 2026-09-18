//! The nested single-turn `Agent` both watchdog seams run — upstream's
//! `new Agent({ … }); await agent.prompt(…)`.
//!
//! **Tag pinning.** The two modules this serves ([`super::review`], [`super::permission_arbiter`])
//! are internally-consistent ports of `pi-subagents` **v0.43.0** and their ~60 citations each are
//! correct at that tag. Everything in THIS file is pinned to **v0.68.0** and every citation says so
//! explicitly (`review.ts:350 @v0.68.0`), because the turn is the part that moved: the
//! `getRegisteredProviderConfig` preference (`permission-arbiter.ts:99-104 @v0.68.0`) and the
//! `opencodeSessionHeaders` overlay (`:109 @v0.68.0`) do not exist at v0.43.0 at all.
//!
//! Upstream builds the same agent twice, once per seam, and the two differ in exactly four places:
//! the system prompt, the user prompt, the tool list, and the `beforeToolCall` refusal sentence.
//! Everything else — model + thinking resolution onto the agent, the provider/transport choice, the
//! credential overlay, the `getApiKey` narrowing, `toolExecution: "sequential"`, the cancel wiring
//! around `prompt`, and the terminal-assistant fold — is byte-identical between
//! `permission-arbiter.ts:111-129 @v0.68.0` and `review.ts:324-358 @v0.68.0`, so it is expressed
//! ONCE here and parameterised by [`WatchdogTurnRequest`].
//!
//! [CYRUP-DELTA] upstream spreads `agentStreamOptions(streamFn)`
//! (`shared/agent-stream-options.ts @v0.68.0`, 5 lines), which returns `{ streamFunction, streamFn }`
//! — two spellings of one option for belt-and-braces compatibility. [`cyrup_agent::AgentBuilder`]
//! takes exactly one [`StreamFn`], so there is nothing to spread and no cyrup counterpart is
//! needed.

use std::sync::Arc;

use async_trait::async_trait;
use cyrup_agent::hooks::{BeforeOutcome, BeforeToolCall, Hooks};
use cyrup_agent::queue::ToolExecution;
use cyrup_agent::stream_fn::{ApiKeyResolver, ProviderStreamFn, StreamFn};
use cyrup_agent::{Agent, AgentMessage};
use cyrup_core::{
    CancelToken, Content, ExecMode, ModelRef, ModelThinkingLevel, ProviderId, TerminateHint, Tool,
    ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::Provider;
use serde_json::Value;

use super::model_selection::WatchdogModelInfo;
use super::permission_arbiter::WatchdogPermissionDecisionTool;
use super::register_main::{WatchdogServicesFn, watchdog_config_dirs};
use super::review::{
    WATCHDOG_WARN_TOOL_LABEL, WATCHDOG_WARN_TOOL_NAME, WatchdogReviewModelSelection,
    WatchdogWarnTool, watchdog_warn_parameters_schema, watchdog_warn_tool_description,
};
use super::runtime::WatchdogWarningEmitter;

/// `OPENCODE_HOST` (`shared/opencode-session-headers.ts:3 @v0.68.0`).
const OPENCODE_HOST: &str = "opencode.ai";

/// `matchesOpenCodeHost(baseUrl)` (`shared/opencode-session-headers.ts:5-11 @v0.68.0`) — a
/// non-URL base falls through to `false`, exactly as upstream's `catch` does.
fn matches_opencode_host(base_url: &str) -> bool {
    let Some((_, rest)) = base_url.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.rsplit_once(':') {
        // Do not strip an IPv6 bracket group's internal colon.
        Some((h, port)) if !h.ends_with(']') && port.chars().all(|c| c.is_ascii_digit()) => h,
        _ => host,
    };
    host == OPENCODE_HOST
}

/// `opencodeSessionHeaders(model, sessionId)` (`shared/opencode-session-headers.ts:23-30 @v0.68.0`).
///
/// [CYRUP-DELTA] ported PRIVATELY here rather than depended on. The identical logic already exists
/// in-tree as `session_headers` (`cyrup-session-svc/src/attribution.rs:102-118`, the host constant
/// at `:26` and the two header names at `:114-115`), but `cyrup-session-svc` is a DEV-dependency of
/// this crate (`Cargo.toml`'s `[dev-dependencies]` block says so, and the crate-boundary rule there
/// is deliberate: a subagents extension must not pull the session service into its production
/// graph). Omitting the headers is not an option — upstream's own doc says why
/// (`:13-21 @v0.68.0`): "subagent-internal calls (watchdog review, permission arbiter, …) stream
/// through bare Agents that bypass that path. Without them OpenCode falls back to client-IP
/// affinity and loses prompt-cache routing (see pi issue #4847)."
fn opencode_session_headers(
    provider: &str,
    base_url: &str,
    session_id: Option<&str>,
) -> Option<cyrup_provider::HeaderMap> {
    let session_id = session_id?;
    if session_id.is_empty() {
        return None;
    }
    if provider != "opencode" && provider != "opencode-go" && !matches_opencode_host(base_url) {
        return None;
    }
    Some(cyrup_provider::HeaderMap::from([
        (
            "x-opencode-session".to_string(),
            Some(session_id.to_string()),
        ),
        ("x-opencode-client".to_string(), Some("pi".to_string())),
    ]))
}

/// `thinkingLevel: selection.thinkingLevel` (`permission-arbiter.ts:120`, `review.ts:332` @v0.68.0)
/// onto the agent's own enum. Every string [`super::review::resolve_watchdog_review_model`] can
/// produce is one of [`super::model_selection::THINKING_LEVELS`]; anything else is upstream's
/// `undefined`, which is `off`.
fn watchdog_thinking_level(level: &str) -> ModelThinkingLevel {
    match level {
        "minimal" => ModelThinkingLevel::Minimal,
        "low" => ModelThinkingLevel::Low,
        "medium" => ModelThinkingLevel::Medium,
        "high" => ModelThinkingLevel::High,
        "xhigh" => ModelThinkingLevel::Xhigh,
        "max" => ModelThinkingLevel::Max,
        _ => ModelThinkingLevel::Off,
    }
}

/// `model: selection.model` (`permission-arbiter.ts:119`, `review.ts:331` @v0.68.0) as the agent's
/// [`ModelRef`]. `api` rides along when the registry knew the model, so the provider resolves the
/// same wire API the session would.
fn watchdog_model_ref(model: &WatchdogModelInfo) -> ModelRef {
    ModelRef {
        provider: ProviderId::from(model.provider.as_str()),
        api: model
            .api
            .as_deref()
            .map(cyrup_core::ApiId::from)
            .filter(|api| !api.as_str().is_empty()),
        model: cyrup_core::ModelId::from(model.id.as_str()),
    }
}

/// `getApiKey: (providerName) => providerName === selection.model.provider ? auth.apiKey :
/// undefined` (`permission-arbiter.ts:125`, `review.ts:337` @v0.68.0) — a cross-provider request
/// gets nothing, so a review model's key can never leak onto another provider's call.
struct WatchdogKeyResolver {
    provider: ProviderId,
    api_key: Option<String>,
}

#[async_trait]
impl ApiKeyResolver for WatchdogKeyResolver {
    async fn get_api_key(&self, provider: &ProviderId) -> Option<String> {
        if provider == &self.provider {
            self.api_key.clone()
        } else {
            None
        }
    }
}

/// `beforeToolCall: async ({ toolCall }) => … ? undefined : { block: true, reason }`
/// (`permission-arbiter.ts:126`, `review.ts:338-340` @v0.68.0) — the EXECUTION-TIME half of the
/// tool policy, which is a different layer from the tool LIST the agent was given: a model can name
/// a tool the harness supplied from elsewhere, and only this hook refuses it.
struct WatchdogToolPolicy {
    #[allow(clippy::type_complexity)]
    block_reason: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

#[async_trait]
impl Hooks for WatchdogToolPolicy {
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> BeforeOutcome {
        match (self.block_reason)(ctx.tool_name) {
            None => BeforeOutcome::Proceed,
            // Upstream's `{ block: true, reason }` carries no `terminate`, so the blocked result
            // does not vote in `shouldTerminateToolBatch` — [`TerminateHint::Unspecified`] is that
            // absent key.
            Some(reason) => BeforeOutcome::Block {
                reason: Some(reason),
                terminate: TerminateHint::Unspecified,
            },
        }
    }
}

/// `createWatchdogWarnTool(warnRequest)` (`review.ts:194-219,306 @v0.68.0`) as a runtime
/// [`Tool`]. Every descriptor comes from [`WatchdogWarnTool`] so the two cannot drift, and
/// `execute` routes through [`WatchdogWarnTool::execute`] — which is what reaches the runtime's
/// emission guard AND produces the model-visible result text telling it whether the warning was
/// taken.
pub struct WatchdogWarnAgentTool {
    emitter: WatchdogWarningEmitter,
    parameters: Value,
    description: String,
}

impl WatchdogWarnAgentTool {
    /// Bind the tool to this review's emitter (`review.ts:193 @v0.68.0`).
    #[must_use]
    pub fn new(emitter: WatchdogWarningEmitter) -> Self {
        Self {
            emitter,
            parameters: watchdog_warn_parameters_schema(),
            description: watchdog_warn_tool_description(),
        }
    }
}

#[async_trait]
impl Tool for WatchdogWarnAgentTool {
    fn name(&self) -> &str {
        WATCHDOG_WARN_TOOL_NAME
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execution_mode(&self) -> ExecMode {
        if WatchdogWarnTool::SEQUENTIAL {
            ExecMode::Sequential
        } else {
            ExecMode::Parallel
        }
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn label(&self) -> Option<&str> {
        Some(WATCHDOG_WARN_TOOL_LABEL)
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // The borrowed descriptor is rebuilt per call because it borrows the emitter; the emitter
        // itself is the runtime's and is cloned once at construction.
        let tool = WatchdogWarnTool::new(&self.emitter);
        // `nonEmptyString` THROWS upstream (`review.ts:177-178 @v0.68.0`), which the harness turns
        // into a tool error the model reads and can correct. Swallowing it would turn a malformed
        // warning into silence.
        let result = tool.execute(&params).map_err(ToolError::new)?;
        Ok(ToolResult {
            content: vec![Content::text(result.text)],
            details: Some(serde_json::json!({ "accepted": result.accepted })),
            ..ToolResult::default()
        })
    }
}

/// The `watchdog_permission_decision` tool (`permission-arbiter.ts:78-88 @v0.68.0`) as a runtime
/// [`Tool`]. The first-call-wins latch and every descriptor live on
/// [`WatchdogPermissionDecisionTool`]; this is only the adapter.
pub struct WatchdogPermissionDecisionAgentTool {
    inner: Arc<WatchdogPermissionDecisionTool>,
    parameters: Value,
}

impl WatchdogPermissionDecisionAgentTool {
    /// Wrap the shared latch so the arbiter can read the decision back after the turn.
    #[must_use]
    pub fn new(inner: Arc<WatchdogPermissionDecisionTool>) -> Self {
        let parameters = inner.parameters();
        Self { inner, parameters }
    }
}

#[async_trait]
impl Tool for WatchdogPermissionDecisionAgentTool {
    fn name(&self) -> &str {
        WatchdogPermissionDecisionTool::NAME
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execution_mode(&self) -> ExecMode {
        if WatchdogPermissionDecisionTool::SEQUENTIAL {
            ExecMode::Sequential
        } else {
            ExecMode::Parallel
        }
    }

    fn description(&self) -> &str {
        WatchdogPermissionDecisionTool::DESCRIPTION
    }

    fn label(&self) -> Option<&str> {
        Some(WatchdogPermissionDecisionTool::LABEL)
    }

    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        // `:84-87 @v0.68.0` — a malformed decision is upstream's schema-validation throw, which the
        // harness reports to the model as a tool error; a WELL-FORMED second call is silently
        // ignored and still answered "Permission decision recorded." (`:85-86`).
        self.inner.record(&params).map_err(ToolError::new)?;
        Ok(ToolResult {
            content: vec![Content::text(
                WatchdogPermissionDecisionTool::RESULT_TEXT.to_string(),
            )],
            details: Some(serde_json::json!({ "recorded": true })),
            ..ToolResult::default()
        })
    }
}

/// One nested turn's input — the four things the two seams differ in, plus the resolved model.
pub struct WatchdogTurnRequest<'a> {
    /// `initialState.systemPrompt` (`permission-arbiter.ts:113-118`, `review.ts:326-330` @v0.68.0).
    pub system_prompt: String,
    /// `agent.prompt(...)`'s argument (`permission-arbiter.ts:129`, `review.ts:350` @v0.68.0).
    pub prompt: String,
    /// The resolved review model, thinking level and credential overlay.
    pub selection: &'a WatchdogReviewModelSelection,
    /// `initialState.tools` (`permission-arbiter.ts:121`, `review.ts:333` @v0.68.0).
    pub tools: Vec<Arc<dyn Tool>>,
    /// `beforeToolCall`'s refusal (`permission-arbiter.ts:126`, `review.ts:338-340` @v0.68.0).
    /// `None` permits the call.
    #[allow(clippy::type_complexity)]
    pub block_reason: Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
    /// The turn's cancellation, which aborts the nested agent
    /// (`permission-arbiter.ts:138`, `review.ts:345-347` @v0.68.0).
    pub cancel: CancelToken,
}

/// The shared nested-`Agent` construction (`permission-arbiter.ts:94-132`, `review.ts:271-358`
/// @v0.68.0), bound to the LIVE session's capability backend so the turn can prefer the session's
/// own provider and carry its session id.
pub struct WatchdogAgentTurn {
    services: WatchdogServicesFn,
}

impl std::fmt::Debug for WatchdogAgentTurn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatchdogAgentTurn").finish_non_exhaustive()
    }
}

/// `return { stopReason: "aborted" }` (`review.ts:349` and the same early return at `:355`,
/// `permission-arbiter.ts:137-141` @v0.68.0) — the answer for a turn cancelled BEFORE
/// `agent.prompt` ever ran, so no assistant message exists to carry the reason.
///
/// [CYRUP-DELTA] upstream returns the RESULT object (`{ stopReason }`) straight out of
/// `runWatchdogAttempt`; this port's seam returns the MESSAGE LIST that
/// [`super::review::final_stop_reason`] folds, because that is what `WatchdogReviewAgent::run` is
/// contracted to hand back. An empty list is NOT the same value: the fold's no-assistant-message
/// arm maps it to `Stop` — a CLEAN review — so a pre-prompt cancel would report as a passed
/// review. The one terminal assistant message below is the smallest value that folds to `Aborted`,
/// and it is the same shape the MID-turn abort produces naturally (`cyrup-agent`'s
/// `settle_aborted`, `agent/run/assistant_stream.rs:137-143`). The two keys are the two the fold
/// reads, spelled as `AssistantMessage`'s own serializer writes them (`cyrup-core`'s
/// `message/assistant.rs:150` for `role`, `:168` for `stopReason`).
/// No `errorMessage` key: upstream's pre-prompt return is the bare `{ stopReason: "aborted" }`
/// (`:349`), unlike its post-turn return which may carry the terminal's `errorMessage` (`:358`).
fn aborted_before_prompt() -> Vec<Value> {
    vec![serde_json::json!({ "role": "assistant", "stopReason": "aborted" })]
}

impl WatchdogAgentTurn {
    /// Bind the turn to the late-resolved capability backend. `services` answers `None` before the
    /// host attaches one (the native extension is constructed before `set_host_services`), which is
    /// upstream's absent `ctx` — the turn then falls back to the built-in provider stack.
    #[must_use]
    pub fn new(services: WatchdogServicesFn) -> Self {
        Self { services }
    }

    /// `ctx.modelRegistry.getRegisteredProviderConfig(selection.model.provider)` +
    /// `registeredProvider?.streamSimple && registeredProvider.api === selection.model.api ? … :
    /// streamSimple` (`permission-arbiter.ts:99-104`, `review.ts:276-281` @v0.68.0).
    ///
    /// The SESSION's own provider is preferred so a subagent-internal model call streams through
    /// the same transport (and the same credentials, proxy and catalog overlay) the session does,
    /// instead of rebuilding one. The api-equality guard is upstream's and is load-bearing: a
    /// provider registered for a DIFFERENT wire API would encode the request wrongly.
    fn resolve_provider(&self, model: &WatchdogModelInfo) -> Result<Arc<dyn Provider>, String> {
        if let Some(services) = (self.services)()
            && let Some(registered) = services.registered_provider(&model.provider)
        {
            let api_matches = match model.api.as_deref() {
                // No api on the selection is upstream's `undefined`, which its `===` comparison
                // only satisfies for an equally-undefined registered api. cyrup's registry fills
                // `api` from the catalog whenever it knows the model, so the absent case is the
                // bare `provider/id` inherited-session-model arm — there the session's provider IS
                // the model's provider by construction, so preferring it is correct.
                None => true,
                Some(api) => registered
                    .get_model(&model.id)
                    .is_some_and(|m| m.api.as_str() == api),
            };
            if api_matches {
                return Ok(registered);
            }
        }
        // `streamSimple` (`permission-arbiter.ts:104 @v0.68.0`): the generic transport, which for
        // cyrup is the built-in provider stack over the process's own `auth.json` — the SAME
        // credential source `BuiltinWatchdogModelRegistry` answered `hasConfiguredAuth` from, so a
        // model that resolved is a model this stack can stream.
        let dirs = watchdog_config_dirs().ok_or_else(|| {
            "Watchdog model turn cannot resolve the cyrup config directories.".to_string()
        })?;
        let store: Arc<dyn cyrup_provider::CredentialStore> =
            Arc::new(cyrup_config::AuthStore::open(&dirs));
        let registry = Arc::new(cyrup_provider::api::builtin_registry());
        cyrup_provider::all_providers_with(store, registry)
            .into_iter()
            .find(|p| p.id().as_str() == model.provider)
            .ok_or_else(|| {
                format!(
                    "Watchdog model turn has no provider for '{}'.",
                    model.provider
                )
            })
    }

    /// Run the turn and hand back its message list, serialized the way
    /// [`super::review::final_stop_reason`] reads it.
    ///
    /// # Errors
    ///
    /// Provider resolution failure, or [`Agent::prompt`]'s own error. A transport failure does NOT
    /// come back here — it arrives as a terminal `error` assistant message, which the stop-reason
    /// fold reports and the arbiter's caller maps to a DENIAL.
    pub async fn run(&self, request: WatchdogTurnRequest<'_>) -> Result<Vec<Value>, String> {
        let selection = request.selection;
        let provider = self.resolve_provider(&selection.model)?;
        let model_ref = watchdog_model_ref(&selection.model);
        // `const sessionId = request.ctx.sessionManager.getSessionId()` (`permission-arbiter.ts:98`,
        // `review.ts:282` @v0.68.0).
        let session_id = (self.services)().and_then(|s| s.session_id());
        // `headers: { ...opencodeSessionHeaders(model, sessionId), ...(streamOptions?.headers ??
        // {}), ...(auth.headers ?? {}) }` (`permission-arbiter.ts:109`, `review.ts:291` @v0.68.0) —
        // `auth.headers` LAST, so the overlay wins over the session-affinity pair.
        let base_url = provider
            .get_model(&selection.model.id)
            .map(|m| m.base_url.to_string())
            .unwrap_or_default();
        let mut headers =
            opencode_session_headers(&selection.model.provider, &base_url, session_id.as_deref())
                .unwrap_or_default();
        for (name, value) in selection.auth.headers.iter().flatten() {
            headers.insert(name.clone(), Some(value.clone()));
        }
        let stream_fn: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));
        let mut builder = Agent::builder(model_ref, stream_fn)
            .system_prompt(request.system_prompt)
            .thinking_level(watchdog_thinking_level(&selection.thinking_level))
            .tools(request.tools)
            .hooks(Arc::new(WatchdogToolPolicy {
                block_reason: Arc::clone(&request.block_reason),
            }))
            .key_resolver(Arc::new(WatchdogKeyResolver {
                provider: ProviderId::from(selection.model.provider.as_str()),
                api_key: selection.auth.api_key.clone(),
            }))
            // `toolExecution: "sequential"` (`permission-arbiter.ts:127`, `review.ts:341`
            // @v0.68.0) — the AGENT-wide setting, distinct from each tool's own `executionMode`.
            .tool_execution(ToolExecution::Sequential);
        if !headers.is_empty() {
            builder = builder.headers(headers);
        }
        // `...(auth.apiKey ? { apiKey: auth.apiKey } : {})` (`permission-arbiter.ts:107`,
        // `review.ts:289` @v0.68.0) — the static fallback beside the dynamic `getApiKey`.
        if let Some(api_key) = selection.auth.api_key.clone() {
            builder = builder.api_key(api_key);
        }
        // `env: auth.env || streamOptions?.env ? { ... } : undefined` (`permission-arbiter.ts:108`,
        // `review.ts:290` @v0.68.0).
        if let Some(env) = selection.auth.env.as_ref().filter(|e| !e.is_empty()) {
            builder = builder.provider_env(env.iter().cloned().collect());
        }
        if let Some(session_id) = session_id {
            builder = builder.session_id(cyrup_core::SessionId::from(session_id));
        }
        let agent = builder.build();
        // `if (ctx.signal?.aborted || request.signal?.aborted) return { stopReason: "aborted" }`
        // (`review.ts:349 @v0.68.0`) — checked once more immediately before the prompt.
        if request.cancel.is_cancelled() {
            return Ok(aborted_before_prompt());
        }
        let handle = agent
            .prompt(request.prompt)
            .await
            .map_err(|error| error.to_string())?;
        // `const abort = () => agent.abort(); ctx.signal?.addEventListener("abort", abort)`
        // (`review.ts:345-347`, `permission-arbiter.ts:138` @v0.68.0). Racing the token against the
        // run and then STILL awaiting the run is the listener's shape: the abort makes the loop
        // unwind, and the messages it produced are what the fold reads.
        let finished = handle.finished();
        tokio::pin!(finished);
        let messages: Vec<AgentMessage> = tokio::select! {
            messages = &mut finished => messages,
            () = request.cancel.cancelled() => {
                agent.abort();
                (&mut finished).await
            }
        };
        // `agent.state.messages.findLast(m => m.role === "assistant")` (`review.ts:356 @v0.68.0`).
        // `AssistantMessage`'s hand-written serializer writes `role` (`cyrup-core`'s
        // `message/assistant.rs:150`) and `stopReason` (`:168`), so the fold reads these verbatim
        // and the trait's `Vec<Value>` return needs no change.
        Ok(messages
            .iter()
            .filter_map(|message| serde_json::to_value(message).ok())
            .collect())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn opencode_headers_ride_only_on_opencode_calls() {
        assert!(
            opencode_session_headers("anthropic", "https://api.anthropic.com", Some("s1"))
                .is_none()
        );
        assert!(opencode_session_headers("opencode", "https://example.test", Some("s1")).is_some());
        assert!(
            opencode_session_headers("custom", "https://opencode.ai/v1", Some("s1")).is_some(),
            "the host match is the third arm of `:28 @v0.68.0`"
        );
        assert!(
            opencode_session_headers("opencode", "https://example.test", None).is_none(),
            "no session id means no headers (`:27 @v0.68.0`)"
        );
        let headers =
            opencode_session_headers("opencode-go", "https://example.test", Some("abc")).unwrap();
        assert_eq!(headers.get("x-opencode-session"), Some(&Some("abc".into())));
        assert_eq!(headers.get("x-opencode-client"), Some(&Some("pi".into())));
    }

    #[test]
    fn a_non_url_base_is_not_an_opencode_host() {
        assert!(!matches_opencode_host("opencode.ai"));
        assert!(matches_opencode_host("https://opencode.ai"));
        assert!(matches_opencode_host("https://user@opencode.ai:443/v1"));
        assert!(!matches_opencode_host("https://not-opencode.ai/v1"));
    }

    #[test]
    fn thinking_levels_map_onto_the_agents_enum_and_unknown_is_off() {
        assert_eq!(watchdog_thinking_level("high"), ModelThinkingLevel::High);
        assert_eq!(watchdog_thinking_level("max"), ModelThinkingLevel::Max);
        assert_eq!(watchdog_thinking_level("off"), ModelThinkingLevel::Off);
        assert_eq!(watchdog_thinking_level("nonsense"), ModelThinkingLevel::Off);
    }

    /// Drive one tool name through the real [`WatchdogToolPolicy`] hook — the `Hooks` seam the
    /// nested agent's loop calls — under a caller-supplied policy.
    #[allow(clippy::type_complexity)]
    async fn before_tool_call_outcome(
        block_reason: &Arc<dyn Fn(&str) -> Option<String> + Send + Sync>,
        name: &str,
    ) -> BeforeOutcome {
        use cyrup_core::{
            AssistantMessage, LazyArgs, ProviderId, StopReason, ToolCall, ToolCallId,
        };

        let policy = WatchdogToolPolicy {
            block_reason: Arc::clone(block_reason),
        };
        let assistant = AssistantMessage::errored(
            ProviderId::from("faux"),
            "faux-1",
            None,
            StopReason::ToolUse,
            "",
        );
        let call = ToolCall {
            id: ToolCallId::from("call-1"),
            name: name.to_string(),
            arguments: LazyArgs::default(),
            thought_signature: None,
        };
        let mut args = serde_json::json!({});
        policy
            .before_tool_call(
                BeforeToolCall {
                    tool_name: name,
                    tool_call_id: &call.id,
                    args: &mut args,
                    messages: &[],
                    assistant_message: &assistant,
                    tool_call: &call,
                    context: cyrup_agent::hooks::AgentContextView {
                        system_prompt: "",
                        messages: &[],
                        tools: &[],
                    },
                },
                CancelToken::new(),
            )
            .await
    }

    /// The policy the PRODUCTION review agent installs, taken from the production agent itself
    /// rather than rebuilt here — `ModelTurnReviewAgent::turn_request` puts this exact value on
    /// every review turn.
    #[allow(clippy::type_complexity)]
    fn production_review_policy() -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        super::super::review::ModelTurnReviewAgent::new(std::env::temp_dir(), Arc::new(|| None))
            .tool_call_block_reason()
    }

    /// The policy the PRODUCTION arbiter installs, likewise taken from the production agent.
    #[allow(clippy::type_complexity)]
    fn production_arbiter_policy() -> Arc<dyn Fn(&str) -> Option<String> + Send + Sync> {
        super::super::permission_arbiter::ModelTurnPermissionAgent::new(
            Arc::new(super::super::model_selection::BuiltinWatchdogModelRegistry::new(None)),
            Arc::new(super::super::review::AmbientReviewAuth),
            Arc::new(|| None),
        )
        .tool_call_block_reason()
    }

    /// The EXECUTION-TIME half of the two-layer tool policy (`review.ts:338-340` @v0.68.0), driven
    /// through the `Hooks` seam the nested agent's loop actually calls, under the policy the
    /// PRODUCTION [`super::super::review::ModelTurnReviewAgent`] installs.
    ///
    /// This layer is not reachable end to end from the integration test in `cyrup-it`, and the
    /// reason is worth stating: cyrup's nested agent gets its tool list from
    /// `ModelTurnReviewAgent::turn_request` and nowhere else, so a tool the permit list rejects is
    /// also a tool the agent does not have — and the loop answers "Tool <name> not found" before
    /// any hook runs, because `cyrup-agent` locates the tool first (`agent/run/tools/preflight.rs`
    /// `:17`, `:80`). Upstream's second layer guards the case where the harness supplies tools of
    /// its own; here it guards a future widening of `cyrup_tools::read_only_tools`. So the sentence
    /// is pinned HERE — but pinned against the production value, so gutting
    /// `ModelTurnReviewAgent::tool_call_block_reason` turns this red.
    #[tokio::test]
    async fn the_tool_policy_hook_blocks_with_upstreams_own_sentence() {
        let policy = production_review_policy();
        assert!(matches!(
            before_tool_call_outcome(&policy, "read").await,
            BeforeOutcome::Proceed
        ));
        assert!(matches!(
            before_tool_call_outcome(&policy, "watchdog_warn").await,
            BeforeOutcome::Proceed
        ));
        let BeforeOutcome::Block { reason, terminate } =
            before_tool_call_outcome(&policy, "write").await
        else {
            panic!("a write from the reviewer must be blocked at execution time");
        };
        assert_eq!(
            reason.as_deref(),
            Some("Watchdog reviews are read-only; tool 'write' is not allowed.")
        );
        assert_eq!(
            terminate,
            TerminateHint::Unspecified,
            "upstream's `{{ block: true, reason }}` carries no `terminate` key"
        );
    }

    /// The arbiter's half of the same layer (`permission-arbiter.ts:126 @v0.68.0`): every tool but
    /// `watchdog_permission_decision` is refused, at execution time, by the closure the PRODUCTION
    /// [`super::super::permission_arbiter::ModelTurnPermissionAgent`] installs.
    ///
    /// The previous version of this test declared its own copy of that closure and asserted on it,
    /// so gutting the production one to `|_| None` — letting an arbiter turn call anything —
    /// changed nothing here. It now fails.
    #[tokio::test]
    async fn the_arbiters_block_reason_permits_only_its_one_tool() {
        let policy = production_arbiter_policy();
        assert!(matches!(
            before_tool_call_outcome(&policy, WatchdogPermissionDecisionTool::NAME).await,
            BeforeOutcome::Proceed
        ));
        for forbidden in ["read", "write", "bash", "watchdog_warn"] {
            let BeforeOutcome::Block { reason, terminate } =
                before_tool_call_outcome(&policy, forbidden).await
            else {
                panic!("the arbiter must refuse '{forbidden}' at execution time");
            };
            assert_eq!(
                reason.as_deref(),
                Some(format!("Permission arbiter tool '{forbidden}' is not allowed.").as_str())
            );
            assert_eq!(terminate, TerminateHint::Unspecified);
        }
    }

    /// **The pre-prompt cancel arm.** A turn whose token is already cancelled when `run` reaches
    /// the check at `review.ts:349 @v0.68.0` reports `aborted`, NOT a clean stop.
    ///
    /// The window is real: `run` resolves a provider and builds the whole agent between the
    /// caller's own cancel check (`review.rs`'s post-resolution check) and this one. Returning an
    /// empty list here folds to [`super::super::runtime::ReviewStopReason::Stop`] through
    /// `final_stop_reason`'s no-assistant-message arm — i.e. a cancelled review would report as a
    /// PASSED review, and `MainWatchdogRuntime::review_delta` would never count it as failed.
    ///
    /// Nothing is streamed: the cancel check precedes `agent.prompt`, so this runs offline.
    #[tokio::test]
    async fn a_turn_cancelled_before_the_prompt_reports_aborted_not_stop() {
        use super::super::review::{WatchdogReviewAuth, WatchdogReviewModelSelection};
        use super::super::runtime::ReviewStopReason;

        let cancel = CancelToken::new();
        cancel.cancel();
        let selection = WatchdogReviewModelSelection {
            // A builtin provider id, so `resolve_provider` gets past its lookup without any
            // credential: the fallback stack is built from the embedded catalogs.
            model: WatchdogModelInfo::new("anthropic", "claude-opus-4-8"),
            thinking_level: "off".to_string(),
            auth: WatchdogReviewAuth::default(),
            explicit: true,
        };
        let messages = WatchdogAgentTurn::new(Arc::new(|| None))
            .run(WatchdogTurnRequest {
                system_prompt: "system".to_string(),
                prompt: "prompt".to_string(),
                selection: &selection,
                tools: Vec::new(),
                block_reason: production_review_policy(),
                cancel,
            })
            .await
            .expect("a cancelled turn is not an error");
        assert_eq!(
            super::super::review::final_stop_reason(&messages),
            ReviewStopReason::Aborted,
            "a cancelled review must not fold to a CLEAN review; messages: {messages:#?}"
        );
    }

    #[test]
    fn a_model_ref_carries_the_catalog_api_when_the_registry_knew_it() {
        let mut info = WatchdogModelInfo::new("anthropic", "claude-x");
        assert_eq!(watchdog_model_ref(&info).api, None);
        info.api = Some("anthropic-messages".to_string());
        let model_ref = watchdog_model_ref(&info);
        assert_eq!(model_ref.provider.as_str(), "anthropic");
        assert_eq!(model_ref.model.as_str(), "claude-x");
        assert_eq!(
            model_ref.api.map(|a| a.as_str().to_string()),
            Some("anthropic-messages".to_string())
        );
    }
}
