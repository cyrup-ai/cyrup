//! Regression cover for the 02-cyrup-agent backlog items closed in this pass.
//!
//! Every assertion here is derived from a pi source range that was opened at `v0.83.0` (the ported
//! baseline) or, for the `upstream-drift` items, at `v0.84.1`; the citation sits on each test.
//! Offline throughout — the faux provider and in-test tools only.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::agent::HeaderFn;
use crate::loop_fn::{run_agent_loop, AgentContext, AgentEventSink, AgentLoopConfig};
use crate::state::{reduce, GenerationConfig, StateInner};
use crate::{
    Agent, AgentError, AgentEvent, AgentMessage, ApiKeyResolver, BeforeOutcome, BeforeToolCall,
    HookError, Hooks, ProviderStreamFn, StreamFn, ToolExecution, ToolResultMessage,
};
use crate::{EventSubscriber, PendingQueue};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, EventStream, ModelRef, ModelThinkingLevel, ProviderId,
    RunCancel, StopReason, Tool, ToolCallId, ToolError, ToolResult, ToolUpdate, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::{Context, Provider, ProviderError, StreamEvent, StreamOptions};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Wiring
// ---------------------------------------------------------------------------

fn model_ref() -> ModelRef {
    ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
}

fn obj_schema() -> Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": true })
}

fn faux_stream_fn(responses: Vec<AssistantMessage>) -> Arc<dyn StreamFn> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;
    Arc::new(ProviderStreamFn::new(provider))
}

/// The `StreamOptions` fields these tests discriminate on.
#[derive(Clone, Default)]
struct Captured {
    api_key: Option<String>,
    headers: Option<cyrup_provider::HeaderMap>,
    metadata: Option<serde_json::Map<String, Value>>,
    websocket_connect_timeout_ms: Option<u64>,
}

struct RecordingStreamFn {
    inner: Arc<dyn StreamFn>,
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl StreamFn for RecordingStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.captured.lock().unwrap().push(Captured {
            api_key: opts.api_key.clone(),
            headers: opts.headers.clone(),
            metadata: opts.metadata.clone(),
            websocket_connect_timeout_ms: opts.websocket_connect_timeout_ms,
        });
        self.inner.stream(model, ctx, opts)
    }
}

fn recording_stream_fn(
    responses: Vec<AssistantMessage>,
) -> (Arc<dyn StreamFn>, Arc<Mutex<Vec<Captured>>>) {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> =
        Arc::new(RecordingStreamFn { inner: faux_stream_fn(responses), captured: captured.clone() });
    (sf, captured)
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSubscriber for Recorder {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
        self.events.lock().unwrap().push(event.clone());
    }
}

impl Recorder {
    fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }
    fn turn_starts(&self) -> usize {
        self.snapshot().iter().filter(|e| matches!(e, AgentEvent::TurnStart)).count()
    }
    fn end_results(&self) -> Vec<Value> {
        self.snapshot()
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
                _ => None,
            })
            .collect()
    }
    fn tool_result_messages(&self) -> Vec<ToolResultMessage> {
        self.snapshot()
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::MessageEnd { message: AgentMessage::ToolResult(t) } => Some(t),
                _ => None,
            })
            .collect()
    }
}

/// A subscriber that aborts the run the first time it sees `pred`.
struct AbortOn {
    agent: Weak<Agent>,
    pred: fn(&AgentEvent) -> bool,
    fired: Mutex<bool>,
}

#[async_trait::async_trait]
impl EventSubscriber for AbortOn {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
        let mut fired = self.fired.lock().unwrap();
        if !*fired
            && (self.pred)(event)
            && let Some(a) = self.agent.upgrade()
        {
            *fired = true;
            a.abort();
        }
    }
}

fn abort_on(agent: &Arc<Agent>, pred: fn(&AgentEvent) -> bool) -> Arc<AbortOn> {
    Arc::new(AbortOn { agent: Arc::downgrade(agent), pred, fired: Mutex::new(false) })
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

struct OkTool {
    name: String,
    params: Value,
    terminate: bool,
}

impl OkTool {
    fn new(name: &str, terminate: bool) -> Arc<Self> {
        Arc::new(Self { name: name.into(), params: obj_schema(), terminate })
    }
}

#[async_trait::async_trait]
impl Tool for OkTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("ok")],
            terminate: self.terminate,
            ..Default::default()
        })
    }
}

/// Emits `n` updates SYNCHRONOUSLY (no await between them) before settling — the burst AGENT-003's
/// 64-slot bounded channel used to truncate.
struct BurstTool {
    name: String,
    params: Value,
    n: usize,
}

#[async_trait::async_trait]
impl Tool for BurstTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        mut on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        for i in 0..self.n {
            on_update(ToolUpdate {
                content: vec![Content::text(format!("step-{i}"))],
                details: None,
                terminate: None,
            });
        }
        Ok(ToolResult { content: vec![Content::text("done")], ..Default::default() })
    }
}

/// Parks until the test releases it, so a run can be held open deterministically.
struct ParkingTool {
    name: String,
    params: Value,
    release: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    entered: Arc<tokio::sync::Notify>,
}

#[async_trait::async_trait]
impl Tool for ParkingTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.entered.notify_waiters();
        let rx = self.release.lock().unwrap().take();
        if let Some(rx) = rx {
            let _ = rx.await;
        }
        Ok(ToolResult { content: vec![Content::text("released")], ..Default::default() })
    }
}

// ===========================================================================
// AGENT-009 — error tool results carry `details: {}` and NO `terminate` key.
//
// pi `createErrorToolResult` returns `{ content: [{type:"text", text: message}], details: {} }`
// (`packages/agent/src/agent-loop.ts:756-761` @v0.83.0) — an object LITERAL, and no `terminate`.
// `emitToolExecutionEnd` (`:763-771`) emits `result: finalized.result` verbatim, so
// `JSON.stringify` drops every absent key. The transcript writer therefore records `"details":{}`.
// ===========================================================================

#[tokio::test]
async fn agent009_error_result_has_empty_details_object_and_no_terminate_key() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("ghost", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let ends = rec.end_results();
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0]["details"], json!({}), "pi writes the empty object literal, not null");
    assert!(
        ends[0].get("terminate").is_none(),
        "`createErrorToolResult` sets no `terminate`, so `JSON.stringify` emits no key: {}",
        ends[0]
    );

    // The JSONL half: `ToolResultMessage.details` is `skip_serializing_if = Option::is_none`, so
    // `None` used to OMIT the key where pi writes `"details":{}`.
    let msgs = rec.tool_result_messages();
    assert_eq!(msgs.len(), 1);
    let wire = serde_json::to_value(&msgs[0]).unwrap();
    assert_eq!(wire["details"], json!({}), "the transcript entry must carry `\"details\":{{}}`");
}

/// A SUCCESSFUL result whose tool set neither `details` nor `terminate` must emit neither key —
/// pi's `result: finalized.result` is the tool's own object, and `JSON.stringify` drops
/// `undefined`.
#[tokio::test]
async fn agent009_plain_success_result_omits_details_and_terminate() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("t", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![OkTool::new("t", false)]).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let ends = rec.end_results();
    assert_eq!(ends.len(), 1);
    assert!(ends[0].get("details").is_none(), "absent details ⇒ no key: {}", ends[0]);
    assert!(ends[0].get("terminate").is_none(), "absent terminate ⇒ no key: {}", ends[0]);
}

// ===========================================================================
// AGENT-010 / AGENT-032(a) — the loop's own error strings, byte-for-byte.
// ===========================================================================

/// `` createErrorToolResult(`Tool ${toolCall.name} not found`) `` — `agent-loop.ts:611` @v0.83.0.
/// NO quotes around the name; cyrup emitted `Tool 'ghost' not found`.
#[tokio::test]
async fn agent010_tool_not_found_string_is_pis() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("ghost", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let msgs = rec.tool_result_messages();
    assert_eq!(text_of(&msgs[0].content), "Tool ghost not found");
}

fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

struct BlockWith {
    reason: Option<String>,
    terminate: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Hooks for BlockWith {
    async fn before_tool_call(
        &self,
        _ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(BeforeOutcome::Block { reason: self.reason.clone(), terminate: self.terminate })
    }
}

/// `createErrorToolResult(beforeResult.reason || "Tool execution was blocked")` —
/// `agent-loop.ts:639` @v0.83.0. `||` is JS-falsy, so BOTH an absent reason and an EMPTY-STRING
/// reason (which the WIT `block(option<string>)` seam can produce) yield the default text; cyrup
/// used to emit its own label for `None` and an empty text content block for `Some("")`, and an
/// empty text block is a 400 from Anthropic's Messages API.
#[tokio::test]
async fn agent010_032_blocked_reason_falls_back_on_none_and_on_empty_string() {
    for reason in [None, Some(String::new())] {
        let sf = faux_stream_fn(vec![
            faux_assistant_message(vec![faux_tool_call("t", json!({}))], StopReason::ToolUse),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ]);
        let agent = Agent::builder(model_ref(), sf)
            .tools(vec![OkTool::new("t", false)])
            .hooks(Arc::new(BlockWith {
                reason: reason.clone(),
                terminate: false,
                calls: Arc::new(AtomicUsize::new(0)),
            }))
            .build();
        let rec = Arc::new(Recorder::default());
        agent.subscribe(rec.clone());
        agent.prompt("go").await.unwrap().finished().await;
        agent.wait_for_idle().await;

        let msgs = rec.tool_result_messages();
        assert_eq!(
            text_of(&msgs[0].content),
            "Tool execution was blocked",
            "reason {reason:?} must fall back to pi's default text"
        );
    }
}

// ===========================================================================
// AGENT-012 — `before_tool_call` runs even on an already-cancelled run, and an abort
// out-votes a block.
//
// `prepareToolCall` (`agent-loop.ts:616-656` @v0.83.0) has NO pre-hook abort check: the only checks
// are `if (signal?.aborted)` at `:629` — immediately after the hook returns and BEFORE the block
// branch at `:636` — and a second at `:644`.
// ===========================================================================

#[tokio::test]
async fn agent012_before_tool_call_still_runs_on_a_cancelled_run_and_abort_beats_block() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("t", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Arc::new(
        Agent::builder(model_ref(), sf)
            .tools(vec![OkTool::new("t", false)])
            .hooks(Arc::new(BlockWith {
                reason: Some("denied by policy".into()),
                terminate: false,
                calls: calls.clone(),
            }))
            .build(),
    );
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    // Cancel while the batch is being prepared: the abort lands before `prepare` is entered.
    agent.subscribe(abort_on(&agent, |e| matches!(e, AgentEvent::ToolExecutionStart { .. })));

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "pi invokes `beforeToolCall` for EVERY call, cancelled or not — extensions keeping audit \
         logs / ref-counted resources depend on it"
    );
    let msgs = rec.tool_result_messages();
    assert_eq!(
        text_of(&msgs[0].content),
        "Operation aborted",
        "abort out-votes the block: pi checks the signal at :629, before the block branch at :636"
    );
}

// ===========================================================================
// AGENT-015 + AGENT-022 — an aborted parallel batch folds `terminate` over the slots it actually
// FILLED, and a blocking hook can ask for termination.
//
// pi's `finalizedCalls` (`agent-loop.ts:497` @v0.83.0) holds only pushed entries, and
// `shouldTerminateToolBatch` (`:582-584`) is
// `finalizedCalls.length > 0 && finalizedCalls.every(f => f.result.terminate === true)` over that
// shortened list. The `terminate` on a BLOCKED call is v0.84.1 drift: `types.ts:61-69` adds
// `terminate?: boolean` to `BeforeToolCallResult`, consumed at `agent-loop.ts:636-645`.
// ===========================================================================

/// Aborts the run as soon as call #1's `tool_execution_end` is emitted, i.e. exactly in the window
/// between two `prepare()` calls, which is the only way a slot is left unfilled.
#[tokio::test]
async fn agent015_022_blocked_terminate_survives_an_abort_that_leaves_a_slot_unprepared() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(
            vec![faux_tool_call("a", json!({})), faux_tool_call("b", json!({}))],
            StopReason::ToolUse,
        ),
        // Only consumed if the batch WRONGLY fails to terminate.
        faux_assistant_message(vec![faux_text("second turn")], StopReason::Stop),
    ]);
    let agent = Arc::new(
        Agent::builder(model_ref(), sf)
            .tools(vec![OkTool::new("a", false), OkTool::new("b", false)])
            .tool_execution(ToolExecution::Parallel)
            .hooks(Arc::new(BlockWith {
                reason: Some("denied".into()),
                terminate: true,
                calls: Arc::new(AtomicUsize::new(0)),
            }))
            .build(),
    );
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.subscribe(abort_on(&agent, |e| matches!(e, AgentEvent::ToolExecutionEnd { .. })));

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // Call `a` was blocked WITH `terminate`; call `b` was never prepared, so pi's array never
    // received an entry for it and cannot be vetoed by it.
    let ends = rec.end_results();
    assert_eq!(ends.len(), 1, "only the prepared call reported an end: {ends:?}");
    assert_eq!(ends[0]["terminate"], json!(true), "AGENT-022: the block carried the hint");
    assert_eq!(
        rec.turn_starts(),
        1,
        "AGENT-015: the never-prepared slot must not veto termination, so no second turn runs"
    );
}

// ===========================================================================
// AGENT-011 / AGENT-018 — the reducer.
// ===========================================================================

fn empty_state() -> StateInner {
    StateInner {
        system_prompt: String::new(),
        model: model_ref(),
        thinking_level: ModelThinkingLevel::Off,
        tools: Vec::new(),
        messages: Vec::new(),
        is_streaming: false,
        streaming_message: None,
        pending_tool_calls: std::collections::HashSet::new(),
        error_message: None,
        headers: None,
        transport: None,
    }
}

fn errored(stop: StopReason, msg: Option<&str>) -> AgentMessage {
    let mut a = AssistantMessage::errored(
        ProviderId::from("faux"),
        "faux-1",
        None,
        stop,
        msg.unwrap_or_default(),
    );
    a.error_message = msg.map(str::to_string);
    AgentMessage::Assistant(a)
}

/// `case "turn_end": if (event.message.role === "assistant" && event.message.errorMessage) { ... }`
/// — `agent.ts:558-562` @v0.83.0. Presence only: no stop-reason gate, no synthetic fallback.
#[test]
fn agent011_turn_end_error_message_is_presence_gated_with_no_fallback() {
    // (a) an aborted turn carrying NO errorMessage leaves state untouched — pi never fabricates
    // "turn ended with error", which surfaced on a deliberate Ctrl-C.
    let mut st = empty_state();
    reduce(
        &mut st,
        &AgentEvent::TurnEnd { message: errored(StopReason::Aborted, None), tool_results: vec![] },
    );
    assert_eq!(st.error_message, None, "no message ⇒ no state write, and never a synthetic one");

    // (b) an errorMessage on a NON-error stop reason still updates state — cyrup's stop-reason gate
    // dropped this recoverable-error annotation.
    let mut st = empty_state();
    reduce(
        &mut st,
        &AgentEvent::TurnEnd {
            message: errored(StopReason::Stop, Some("recovered after 2 retries")),
            tool_results: vec![],
        },
    );
    assert_eq!(st.error_message.as_deref(), Some("recovered after 2 retries"));
}

/// `case "message_start": this._state.streamingMessage = event.message; break;` —
/// `agent.ts:531-533` @v0.83.0. No role check.
#[test]
fn agent018_message_start_reduces_for_non_assistant_messages() {
    let mut st = empty_state();
    reduce(&mut st, &AgentEvent::MessageStart { message: AgentMessage::user_text("hi") });
    assert!(
        st.streaming_message.is_some(),
        "pi publishes the user message as `streamingMessage`; the assistant-only guard showed the \
         front-end nothing"
    );
}

/// `agent_end` clears only `streamingMessage` (`agent.ts:564-566` @v0.83.0); `pendingToolCalls` is
/// reset in `finishRun()` (`:514-520`), which runs from the `finally` at `:491-493` — AFTER every
/// listener settles. A subscriber reading the set on `agent_end` to spot calls abandoned by an
/// aborted run saw an empty set under cyrup and the real set under pi.
#[test]
fn agent018_agent_end_does_not_clear_pending_tool_calls() {
    let mut st = empty_state();
    reduce(
        &mut st,
        &AgentEvent::ToolExecutionStart {
            tool_call_id: ToolCallId::from("call-1"),
            tool_name: "t".into(),
            args: json!({}),
        },
    );
    reduce(&mut st, &AgentEvent::AgentEnd { messages: vec![] });
    assert!(st.streaming_message.is_none(), "agent_end still clears streamingMessage");
    assert_eq!(
        st.pending_tool_calls.len(),
        1,
        "pendingToolCalls survives agent_end and is cleared by settlement, as pi's finishRun does"
    );
}

/// The settlement half of the same move: once the run has settled the set IS empty, so nothing
/// leaks between runs.
#[tokio::test]
async fn agent018_settlement_clears_pending_tool_calls() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("t", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![OkTool::new("t", false)]).build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;
    assert!(agent.snapshot().await.pending_tool_calls.is_empty());
}

// ===========================================================================
// AGENT-021 — the low-level loop sends `AgentLoopConfig.gen_config.headers`.
//
// pi `AgentLoopConfig extends SimpleStreamOptions` (`packages/agent/src/types.ts:271`) and
// `agent-loop.ts:308-312` spreads the whole config into the provider call, so a low-level caller
// that sets `headers` has them on the wire by construction. `build_run_ctx` hardcoded
// `headers: None`, so the field compiled, was accepted, and sent nothing.
// ===========================================================================

struct NullSink;

#[async_trait::async_trait]
impl AgentEventSink for NullSink {
    async fn emit(&self, _event: &AgentEvent) {}
}

#[tokio::test]
async fn agent021_low_level_loop_forwards_configured_headers() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let mut headers = cyrup_provider::HeaderMap::new();
    headers.insert("x-probe".to_string(), Some("42".to_string()));

    let mut config = AgentLoopConfig::new(model_ref());
    config.gen_config = GenerationConfig { headers: Some(headers), ..GenerationConfig::default() };
    // The queues are per-run handles; the default config already supplies empty ones.
    assert!(config.steering.lock().unwrap().is_empty());
    assert!(config.follow_up.lock().unwrap().is_empty());

    run_agent_loop(
        vec![AgentMessage::user_text("go")],
        AgentContext::default(),
        config,
        Arc::new(NullSink),
        RunCancel::new(),
        sf,
    )
    .await;

    let seen = captured.lock().unwrap();
    assert_eq!(seen.len(), 1, "one provider request");
    assert_eq!(
        seen[0].headers.as_ref().and_then(|h| h.get("x-probe")),
        Some(&Some("42".to_string())),
        "an embedder's auth / attribution / gateway headers must reach the wire"
    );
}

// ===========================================================================
// AGENT-S03 + AGENT-031 — `metadata` and `websocketConnectTimeoutMs` have a path out of the agent.
// ===========================================================================

#[tokio::test]
async fn agent_s03_031_metadata_and_ws_connect_timeout_reach_stream_options() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let mut meta = serde_json::Map::new();
    meta.insert("user_id".to_string(), json!("u-7"));

    let agent = Agent::builder(model_ref(), sf)
        .metadata(meta)
        .websocket_connect_timeout_ms(2_500)
        .build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let seen = captured.lock().unwrap();
    assert_eq!(seen[0].metadata.as_ref().and_then(|m| m.get("user_id")), Some(&json!("u-7")));
    assert_eq!(seen[0].websocket_connect_timeout_ms, Some(2_500));
}

// ===========================================================================
// AGENT-032(b) — an EMPTY resolved api key falls through to the static one.
//
// `(config.getApiKey ? await config.getApiKey(...) : undefined) || config.apiKey`
// (`agent-loop.ts:306`, identical at both tags) — `||` is JS-falsy.
// ===========================================================================

struct EmptyKeyResolver;

#[async_trait::async_trait]
impl ApiKeyResolver for EmptyKeyResolver {
    async fn get_api_key(&self, _provider: &ProviderId) -> Option<String> {
        Some(String::new())
    }
}

#[tokio::test]
async fn agent032_empty_resolved_api_key_falls_back_to_the_static_key() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf)
        .api_key("static-key")
        .key_resolver(Arc::new(EmptyKeyResolver))
        .build();
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(
        captured.lock().unwrap()[0].api_key.as_deref(),
        Some("static-key"),
        "an empty resolved key is falsy upstream and must not be sent"
    );
}

// ===========================================================================
// AGENT-023 — `reset()` is refused under a live run (pi `agent.ts:332-345` @v0.84.1).
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent023_reset_is_refused_while_a_run_is_in_flight() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("park", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let entered = Arc::new(tokio::sync::Notify::new());
    let tool = Arc::new(ParkingTool {
        name: "park".into(),
        params: obj_schema(),
        release: Mutex::new(Some(rx)),
        entered: entered.clone(),
    });
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();

    let waiter = entered.notified();
    let handle = agent.prompt("go").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), waiter).await.expect("tool body entered");

    let before = agent.snapshot().await.messages.len();
    let refused_reset = agent.reset().await;
    assert!(
        matches!(refused_reset, Err(AgentError::RunActive(crate::BusyEntry::Reset))),
        "upstream throws \"Agent is already processing. Wait for completion before resetting.\""
    );
    // AGENT-034 — and it must be `reset()`'s own text, not the latch's or `continue()`'s. pi keys
    // four distinct messages off the same `this.activeRun` condition (agent.ts:335 @v0.84.1).
    assert_eq!(
        refused_reset.err().map(|e| e.to_string()).unwrap_or_default(),
        "Agent is already processing. Wait for completion before resetting."
    );
    assert_eq!(
        agent.snapshot().await.messages.len(),
        before,
        "a refused reset must not empty the transcript the live loop is still writing into"
    );

    tx.send(()).unwrap();
    handle.finished().await;
    agent.wait_for_idle().await;

    // Once idle it is allowed again, and it really does clear.
    assert!(agent.reset().await.is_ok());
    assert!(agent.snapshot().await.messages.is_empty());
}

// ===========================================================================
// AGENT-034 — pi keys FOUR distinct throw messages off one `this.activeRun` condition, and two
// more off the two `continue` surfaces' empty-transcript check. cyrup collapsed all six into
// three generic Rust strings, none of which matched pi's text. The strings are not cosmetic:
// `AgentError` is re-emitted verbatim by `SessionServiceError::Agent` ("agent: {0}",
// `crates/cyrup-session-svc/src/error.rs:16-17`), so they reach the user. pi's own suite asserts
// them (`packages/agent/test/agent.test.ts:508-547`, `:548-583`;
// `packages/agent/test/agent-loop.test.ts:1368-1385`, all @v0.83.0).
//
// The second half of the item is control flow, not text: pi's `prompt()` carries its OWN guard at
// `agent.ts:340-344`, ahead of `normalizePromptInput` and of the latch inside `runWithLifecycle`
// (`:472-474`). cyrup's `prompt` had no guard at all and fell through to the latch, so a prompt
// during a live run reported the latch's bare message instead of the one string in the family
// that tells the caller what to do instead ("Use steer() or followUp() to queue messages").
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent034_busy_entry_points_carry_pis_per_entry_point_message() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("park", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let entered = Arc::new(tokio::sync::Notify::new());
    let tool = Arc::new(ParkingTool {
        name: "park".into(),
        params: obj_schema(),
        release: Mutex::new(Some(rx)),
        entered: entered.clone(),
    });
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();

    let waiter = entered.notified();
    let handle = agent.prompt("go").await.unwrap();
    // Park inside the tool body: the run holds the latch for the whole of the assertions below,
    // with no wall-clock sleep anywhere.
    tokio::time::timeout(Duration::from_secs(5), waiter).await.expect("tool body entered");
    assert!(agent.is_running());

    // (1) `prompt` — pi agent.ts:341-343 @v0.83.0. RED before: cyrup had no guard here, so this
    // fell through to the latch and reported "Agent is already processing." instead.
    let busy_prompt = agent.prompt("second").await.err().expect("a second prompt is refused");
    assert!(matches!(busy_prompt, AgentError::RunActive(crate::BusyEntry::Prompt)));
    assert_eq!(
        busy_prompt.to_string(),
        "Agent is already processing a prompt. Use steer() or followUp() to queue messages, \
         or wait for completion."
    );

    // The image overload is the same upstream method behind the same guard (agent.ts:326).
    let busy_images = agent
        .prompt_with_images("second", Vec::new())
        .await
        .err()
        .expect("the image overload is refused identically");
    assert!(matches!(busy_images, AgentError::RunActive(crate::BusyEntry::Prompt)));

    // (2) `continue` — pi agent.ts:352 @v0.83.0. Different sentence from (1) and (3).
    let busy_continue = agent.continue_run().await.err().expect("a continuation is refused");
    assert!(matches!(busy_continue, AgentError::RunActive(crate::BusyEntry::Continue)));
    assert_eq!(
        busy_continue.to_string(),
        "Agent is already processing. Wait for completion before continuing."
    );

    // (3) `reset` — pi agent.ts:335 @v0.84.1 (the AGENT-023 drift).
    let busy_reset = agent.reset().await.err().expect("a reset is refused");
    assert!(matches!(busy_reset, AgentError::RunActive(crate::BusyEntry::Reset)));
    assert_eq!(
        busy_reset.to_string(),
        "Agent is already processing. Wait for completion before resetting."
    );

    // All three are distinct sentences, which is the whole point of the item.
    assert_ne!(busy_prompt.to_string(), busy_continue.to_string());
    assert_ne!(busy_continue.to_string(), busy_reset.to_string());

    tx.send(()).unwrap();
    handle.finished().await;
    agent.wait_for_idle().await;
}

/// The bare latch message (pi `runWithLifecycle`, agent.ts:472-474 @v0.83.0) is the fourth
/// variant. In pi it is unreachable from `prompt`/`continue`/`reset` because single-threaded JS
/// makes their guards atomic with the claim; in cyrup it is what the check-then-claim race falls
/// through to, so it must exist and must carry pi's shorter sentence.
#[test]
fn agent034_latch_variant_carries_pis_bare_run_with_lifecycle_message() {
    assert_eq!(
        AgentError::RunActive(crate::BusyEntry::Run).to_string(),
        "Agent is already processing."
    );
}

/// The two `continue` surfaces reject an empty transcript with DIFFERENT pi strings, and the
/// assistant-tail rejection uses one string on all three of pi's sites.
#[test]
fn agent034_continue_validation_messages_are_surface_specific() {
    assert_eq!(
        AgentError::NoMessages(crate::ContinueSurface::Agent).to_string(),
        "No messages to continue from",
        "pi Agent.continue, agent.ts:357 @v0.83.0"
    );
    assert_eq!(
        AgentError::NoMessages(crate::ContinueSurface::Loop).to_string(),
        "Cannot continue: no messages in context",
        "pi agentLoopContinue agent-loop.ts:71 / runAgentLoopContinue :128, both tags"
    );
    assert_eq!(
        AgentError::ContinueFromAssistant.to_string(),
        "Cannot continue from message role: assistant",
        "pi agent.ts:373, agent-loop.ts:75 and :132 — one string on all three sites"
    );
}

// ===========================================================================
// AGENT-025 — a `transform_context` / `convert_to_llm` failure emits pi's `handleRunFailure`
// payload: `agent_end.messages == [failureMessage]`, and `aborted` when the run was cancelled.
// ===========================================================================

struct FailingTransform;

#[async_trait::async_trait]
impl Hooks for FailingTransform {
    async fn transform_context(
        &self,
        _msgs: Vec<AgentMessage>,
        _cancel: CancelToken,
    ) -> Result<Vec<AgentMessage>, HookError> {
        Err(HookError::new("compaction budget exceeded"))
    }
}

#[tokio::test]
async fn agent025_transform_failure_agent_end_carries_only_the_failure_message() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("x")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(FailingTransform)).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    let returned = agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let end = rec
        .snapshot()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::AgentEnd { messages } => Some(messages),
            _ => None,
        })
        .expect("an agent_end");
    assert_eq!(
        end.len(),
        1,
        "pi emits `{{ type: \"agent_end\", messages: [failureMessage] }}` (agent.ts:511) — the \
         single synthetic message, NOT the whole run accumulator: {end:?}"
    );
    match &end[0] {
        AgentMessage::Assistant(a) => {
            assert_eq!(a.stop_reason, StopReason::Error);
            assert_eq!(a.error_message.as_deref(), Some("compaction budget exceeded"));
        }
        other => panic!("expected the synthetic assistant failure, got {other:?}"),
    }
    assert_eq!(returned.len(), 1, "`RunHandle::finished()` returns the same vector as `agent_end`");
}

/// A hook that fails only after the run has been cancelled — the compaction / context-budget hook
/// under Ctrl-C. pi reports `stopReason: "aborted"` (`agent.ts:504`); cyrup hardcoded `error` and
/// showed a red error state for a clean cancel.
struct CancelAwareTransform;

#[async_trait::async_trait]
impl Hooks for CancelAwareTransform {
    async fn transform_context(
        &self,
        _msgs: Vec<AgentMessage>,
        cancel: CancelToken,
    ) -> Result<Vec<AgentMessage>, HookError> {
        cancel.cancelled().await;
        Err(HookError::new("context build interrupted"))
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent025_transform_failure_after_abort_reports_aborted() {
    let sf = faux_stream_fn(vec![faux_assistant_message(vec![faux_text("x")], StopReason::Stop)]);
    let agent =
        Arc::new(Agent::builder(model_ref(), sf).hooks(Arc::new(CancelAwareTransform)).build());
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    // `transform_context` runs after the prompt's `message_end`, so aborting there parks the hook.
    agent.subscribe(abort_on(&agent, |e| {
        matches!(e, AgentEvent::MessageEnd { message: AgentMessage::User { .. } })
    }));

    tokio::time::timeout(Duration::from_secs(5), async {
        agent.prompt("go").await.unwrap().finished().await;
        agent.wait_for_idle().await;
    })
    .await
    .expect("the cancelled hook must let the run settle");

    let end = rec
        .snapshot()
        .into_iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::AgentEnd { messages } => Some(messages),
            _ => None,
        })
        .expect("an agent_end");
    assert_eq!(end.len(), 1);
    match &end[0] {
        AgentMessage::Assistant(a) => assert_eq!(
            a.stop_reason,
            StopReason::Aborted,
            "`stopReason: aborted ? \"aborted\" : \"error\"` (agent.ts:504)"
        ),
        other => panic!("expected the synthetic assistant failure, got {other:?}"),
    }
}

// ===========================================================================
// AGENT-003 — every `tool_execution_update` is delivered; the only drop rule is pi's
// `acceptingUpdates` flag (`agent-loop.ts:672`/`:680`/`:694`/`:698`/`:705` @v0.83.0).
// ===========================================================================

async fn burst_delivers_all(execution: ToolExecution) {
    const N: usize = 500;
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("burst", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let tool = Arc::new(BurstTool { name: "burst".into(), params: obj_schema(), n: N });
    let agent =
        Agent::builder(model_ref(), sf).tools(vec![tool]).tool_execution(execution).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let updates = rec
        .snapshot()
        .into_iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionUpdate { .. }))
        .count();
    assert_eq!(
        updates, N,
        "a 64-slot bounded channel silently truncated a synchronous burst; pi collects every \
         emission into `updateEvents` and awaits them all"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent003_parallel_batch_delivers_every_update() {
    burst_delivers_all(ToolExecution::Parallel).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent003_sequential_batch_delivers_every_update() {
    burst_delivers_all(ToolExecution::Sequential).await;
}

// ===========================================================================
// AGENT-S02 — `subscribe` returns a detach handle (pi `agent.ts:243-246` @v0.83.0).
// ===========================================================================

#[tokio::test]
async fn agent_s02_subscribe_returns_a_working_detach_handle() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_text("one")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("two")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).build();
    let rec = Arc::new(Recorder::default());
    let sub = agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;
    let after_first = rec.snapshot().len();
    assert!(after_first > 0, "the subscriber received the first run's events");

    sub.unsubscribe();
    agent.prompt("again").await.unwrap().finished().await;
    agent.wait_for_idle().await;
    assert_eq!(
        rec.snapshot().len(),
        after_first,
        "a detached listener receives nothing — pi's returned closure does `listeners.delete`"
    );

    // Idempotent, and safe after the fact.
    sub.unsubscribe();
}

/// The second half of AGENT-S02: `await listener(event, signal)` (`agent.ts:574` @v0.83.0). A
/// subscriber doing expensive work must be able to see that the run it is servicing was aborted.
#[derive(Default)]
struct SignalWatchingSubscriber {
    saw_cancelled: Mutex<Vec<bool>>,
}

#[async_trait::async_trait]
impl EventSubscriber for SignalWatchingSubscriber {
    async fn on_event(&self, _event: &AgentEvent, cancel: CancelToken) {
        self.saw_cancelled.lock().unwrap().push(cancel.is_cancelled());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_s02_on_event_receives_the_runs_abort_signal() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("t", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Arc::new(
        Agent::builder(model_ref(), sf).tools(vec![OkTool::new("t", false)]).build(),
    );
    let watcher = Arc::new(SignalWatchingSubscriber::default());
    agent.subscribe(watcher.clone());
    agent.subscribe(abort_on(&agent, |e| matches!(e, AgentEvent::ToolExecutionStart { .. })));

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let seen = watcher.saw_cancelled.lock().unwrap();
    assert!(seen.iter().any(|c| !*c), "early events are delivered with a live token");
    assert!(
        seen.iter().any(|c| *c),
        "every event after the abort carries a cancelled token: {seen:?}"
    );
}

// ===========================================================================
// AGENT-024 — the post-turn hooks receive the run's abort signal.
//
// pi binds it at the Agent-options layer: `prepareNextTurn: async (context) => { ... return await
// this.prepareNextTurn?.(this.signal); }` (`agent.ts:463-471` @v0.84.1; the same `this.signal`
// argument exists at v0.83.0) and `shouldStopAfterTurn: async (context) => await
// shouldStopAfterTurn(context, this.signal)` (`:460-462`, v0.84.1 drift).
// ===========================================================================

#[derive(Default)]
struct SignalWatchingPostTurn {
    saw_cancelled: Arc<Mutex<Vec<bool>>>,
}

#[async_trait::async_trait]
impl Hooks for SignalWatchingPostTurn {
    async fn prepare_next_turn(
        &self,
        _ctx: crate::PostTurn<'_>,
        cancel: CancelToken,
    ) -> Result<Option<crate::TurnUpdate>, HookError> {
        self.saw_cancelled.lock().unwrap().push(cancel.is_cancelled());
        Ok(None)
    }
    async fn should_stop_after_turn(
        &self,
        _ctx: crate::PostTurn<'_>,
        cancel: CancelToken,
    ) -> Result<bool, HookError> {
        self.saw_cancelled.lock().unwrap().push(cancel.is_cancelled());
        Ok(false)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent024_post_turn_hooks_observe_the_runs_abort_signal() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("t", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let seen = Arc::new(Mutex::new(Vec::new()));
    let agent = Arc::new(
        Agent::builder(model_ref(), sf)
            .tools(vec![OkTool::new("t", false)])
            .hooks(Arc::new(SignalWatchingPostTurn { saw_cancelled: seen.clone() }))
            .build(),
    );
    // Abort once the first turn's tool has finished, i.e. before the post-turn hooks run.
    agent.subscribe(abort_on(&agent, |e| matches!(e, AgentEvent::ToolExecutionEnd { .. })));

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty(), "both post-turn hooks ran");
    assert!(
        seen.iter().any(|c| *c),
        "a post-turn hook must be able to see that the run was aborted — cyrup passed no token at \
         all, so abort latency was bounded by the slowest post-turn hook: {seen:?}"
    );
}

// ===========================================================================
// AGENT-013 — pi's two-tier proxy failure text (`proxy.ts:167-175` @v0.83.0).
// ===========================================================================

#[test]
fn agent013_proxy_error_message_is_pis_two_tier_construction() {
    // Tier 2: the body parses as `{error?: string}` and its `error` is truthy.
    assert_eq!(
        crate::proxy::proxy_error_message(&ProviderError::Http {
            status: 502,
            message: r#"{"error":"upstream down"}"#.to_string(),
        }),
        "Proxy error: upstream down"
    );
    // Tier 1: the body is not JSON.
    assert_eq!(
        crate::proxy::proxy_error_message(&ProviderError::Http {
            status: 502,
            message: "<html>bad gateway</html>".to_string(),
        }),
        "Proxy error: 502 Bad Gateway"
    );
    // Tier 1 again: JS-falsy `errorData.error` does NOT upgrade.
    assert_eq!(
        crate::proxy::proxy_error_message(&ProviderError::Http {
            status: 429,
            message: r#"{"error":""}"#.to_string(),
        }),
        "Proxy error: 429 Too Many Requests"
    );
    // Non-HTTP failures keep their raw text: pi reaches those through its outer catch.
    assert_eq!(
        crate::proxy::proxy_error_message(&ProviderError::Decode("bad frame".to_string())),
        "decode error: bad frame"
    );
}

// ===========================================================================
// AGENT-035 — pi's abort text on the proxy path. `streamProxy` checks the signal BY HAND at two
// points and throws a literal both times — `proxy.ts:186-190` (between reads) and `:208-211`
// (after the read loop drains) @v0.83.0, `:188-192` / `:210-213` @v0.84.1 — and the outer catch
// puts that text straight into `partial.errorMessage` (`:215-218`). cyrup surfaced
// `ProviderError::Aborted`'s bare `Display`, `"aborted"`, so an aborted proxy turn showed a
// different string in the transcript from the one pi writes. The RUN-level abort string already
// matched on both sides (`agent.rs`'s `"Operation aborted"` vs `agent-loop.ts:632`); this is the
// TRANSPORT-level one, which nothing covered.
// ===========================================================================

#[test]
fn agent035_proxy_abort_carries_pis_request_aborted_by_user() {
    assert_eq!(
        crate::proxy::proxy_error_message(&ProviderError::Aborted),
        "Request aborted by user",
        "pi proxy.ts:189 / :210 @v0.83.0 — a literal in pi's own source, not a runtime artifact"
    );
    // The neighbouring arms are untouched: only `Http` and `Aborted` are special-cased.
    assert_eq!(
        crate::proxy::proxy_error_message(&ProviderError::Decode("bad frame".to_string())),
        "decode error: bad frame"
    );
}

// ===========================================================================
// AGENT-029 (regression guard) — the per-turn header resolver is consulted with the model the turn
// is ACTUALLY going to, which is what `set_header_fn` exists for (`sdk.ts:312-328` @v0.83.0).
// ===========================================================================

#[tokio::test]
async fn agent029_header_fn_is_keyed_on_the_dispatched_model() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).build();
    let f: Arc<HeaderFn> = Arc::new(|m: &ModelRef| {
        let mut h = cyrup_provider::HeaderMap::new();
        h.insert("x-for-model".to_string(), Some(m.model.to_string()));
        Some(h)
    });
    agent.set_header_fn(Some(f));

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    assert_eq!(
        captured.lock().unwrap()[0].headers.as_ref().and_then(|h| h.get("x-for-model")),
        Some(&Some("faux-1".to_string()))
    );
}

/// Keeps the `PendingQueue` import honest: the low-level config exposes the same queue handles the
/// high-level agent uses, and AGENT-021's test relies on them starting empty.
#[test]
fn low_level_config_queues_start_empty() {
    let q = PendingQueue::default();
    assert!(q.is_empty());
}
