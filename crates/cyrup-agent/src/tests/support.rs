//! Shared fixtures for the crate-local test suite.
//!
//! Every helper here was previously copy-pasted into 2-12 of the sibling test files, and three of
//! those copies had already DRIFTED apart under one name (`obj_schema`, `faux_stream_fn`,
//! `Recorder`). This module is the single home, so a change to the fixture surface — a new
//! `AgentBuilder` argument, a new `EventSubscriber` method, a new `Tool` method — is made once and
//! every test file sees the same thing.
//!
//! Nothing here is a test: it is wiring only. Assertions live in the sibling files.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    AgentEvent, AgentMessage, EventSubscriber, HookError, Hooks, PostTurn, ProviderStreamFn,
    StreamFn, ToolResultMessage, TurnUpdate,
};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, EventStream, Message, ModelRef, TerminateHint, Tool,
    ToolCallId, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::FauxProvider;
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Model refs
// ---------------------------------------------------------------------------

/// The offline model every faux-provider-driven test runs against.
pub(super) fn model_ref() -> ModelRef {
    ModelRef {
        provider: "faux".into(),
        api: Some("faux".into()),
        model: "faux-1".into(),
    }
}

/// A real-looking anthropic model ref, for the tests that assert on what a transport does with the
/// model it is handed rather than on the loop's own behavior.
pub(super) fn anthropic_model_ref() -> ModelRef {
    ModelRef {
        provider: "anthropic".into(),
        api: Some("anthropic-messages".into()),
        model: "claude".into(),
    }
}

/// The permissive "any object" tool schema used by every in-test tool that does not validate its
/// arguments.
pub(super) fn obj_schema() -> Value {
    json!({ "type": "object" })
}

// ---------------------------------------------------------------------------
// Stream-fn wiring
// ---------------------------------------------------------------------------

/// A `StreamFn` backed by a scripted [`FauxProvider`]. The provider is returned alongside the
/// stream fn for the tests that assert on what the provider itself saw; callers that only need the
/// transport take `.1`.
pub(super) fn faux_stream_fn(
    responses: Vec<AssistantMessage>,
) -> (Arc<FauxProvider>, Arc<dyn StreamFn>) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux.clone();
    let sf: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));
    (faux, sf)
}

/// A `StreamFn` that records one `C` per provider call — whatever `capture` extracts from the
/// forwarded model / context / options — then delegates to the inner transport. The captured
/// payload type stays local to each test file, because what each file discriminates on differs.
pub(super) struct RecordingStreamFn<C, F> {
    inner: Arc<dyn StreamFn>,
    capture: F,
    captured: Arc<Mutex<Vec<C>>>,
}

impl<C, F> StreamFn for RecordingStreamFn<C, F>
where
    C: Send + 'static,
    F: Fn(&ModelRef, &Context, &StreamOptions) -> C + Send + Sync + 'static,
{
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.captured
            .lock()
            .unwrap()
            .push((self.capture)(model, ctx, opts));
        self.inner.stream(model, ctx, opts)
    }
}

/// Scripted faux responses + a recorder in front of them.
pub(super) fn recording_stream_fn<C, F>(
    responses: Vec<AssistantMessage>,
    capture: F,
) -> (Arc<dyn StreamFn>, Arc<Mutex<Vec<C>>>)
where
    C: Send + 'static,
    F: Fn(&ModelRef, &Context, &StreamOptions) -> C + Send + Sync + 'static,
{
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(RecordingStreamFn {
        inner: faux_stream_fn(responses).1,
        capture,
        captured: captured.clone(),
    });
    (sf, captured)
}

/// Captured LLM payloads: one `Vec<Message>` per provider request.
pub(super) type PayloadLog = Arc<Mutex<Vec<Vec<Message>>>>;

/// Records the full `Context.messages` of every provider call, then delegates to the faux provider
/// — so a test can byte-check the assembled payload.
pub(super) struct PayloadRecordingStreamFn {
    inner: Arc<dyn StreamFn>,
    payloads: PayloadLog,
}

impl StreamFn for PayloadRecordingStreamFn {
    fn stream(
        &self,
        model: &ModelRef,
        ctx: &Context,
        opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.payloads.lock().unwrap().push(ctx.messages.clone());
        self.inner.stream(model, ctx, opts)
    }
}

pub(super) fn payload_recording(
    responses: Vec<AssistantMessage>,
) -> (Arc<dyn StreamFn>, PayloadLog) {
    let payloads: PayloadLog = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(PayloadRecordingStreamFn {
        inner: faux_stream_fn(responses).1,
        payloads: payloads.clone(),
    });
    (sf, payloads)
}

// ---------------------------------------------------------------------------
// Event collection
// ---------------------------------------------------------------------------

/// Collects every [`AgentEvent`] an `Agent` publishes, in order.
#[derive(Default)]
pub(super) struct EventRecorder {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSubscriber for EventRecorder {
    async fn on_event(&self, event: &AgentEvent, _cancel: CancelToken) {
        self.events.lock().unwrap().push(event.clone());
    }
}

impl EventRecorder {
    pub(super) fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }

    /// The recorded events as bare kind names, in order.
    pub(super) fn names(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().map(ev_kind).collect()
    }

    pub(super) fn turn_starts(&self) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, AgentEvent::TurnStart))
            .count()
    }

    /// The `tool_execution_end.result` payloads, in order.
    pub(super) fn end_results(&self) -> Vec<Value> {
        self.snapshot()
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
                _ => None,
            })
            .collect()
    }

    /// The `message_end` payloads that carry a tool result.
    pub(super) fn tool_result_messages(&self) -> Vec<ToolResultMessage> {
        self.snapshot()
            .into_iter()
            .filter_map(|e| match e {
                AgentEvent::MessageEnd {
                    message: AgentMessage::ToolResult(t),
                } => Some(t),
                _ => None,
            })
            .collect()
    }
}

/// An event's name, with the message-carrying events qualified by the role they carry.
pub(super) fn ev_name(e: &AgentEvent) -> String {
    match e {
        AgentEvent::AgentStart => "agent_start".into(),
        AgentEvent::TurnStart => "turn_start".into(),
        AgentEvent::MessageStart { message } => format!("message_start:{}", role(message)),
        AgentEvent::MessageUpdate { .. } => "message_update:assistant".into(),
        AgentEvent::MessageEnd { message } => format!("message_end:{}", role(message)),
        AgentEvent::ToolExecutionStart { .. } => "tool_execution_start".into(),
        AgentEvent::ToolExecutionUpdate { .. } => "tool_execution_update".into(),
        AgentEvent::ToolExecutionEnd { .. } => "tool_execution_end".into(),
        AgentEvent::TurnEnd { .. } => "turn_end".into(),
        AgentEvent::AgentEnd { .. } => "agent_end".into(),
    }
}

/// An event's bare kind name — [`ev_name`] without the `:role` qualifier, for the tests that assert
/// on the event SEQUENCE rather than on which role each message carried.
pub(super) fn ev_kind(e: &AgentEvent) -> String {
    let name = ev_name(e);
    match name.split_once(':') {
        Some((kind, _role)) => kind.to_string(),
        None => name,
    }
}

/// The wire role of an agent message.
pub(super) fn role(m: &AgentMessage) -> &'static str {
    match m {
        AgentMessage::User { .. } => "user",
        AgentMessage::Assistant(_) => "assistant",
        AgentMessage::ToolResult(_) => "tool",
        AgentMessage::Custom { .. } => "custom",
        // SESS-043 — a declaration-merged coding-agent role never produced by the agent loop
        // itself; it only enters through `set_messages` when `cyrup-session-svc` seeds the raw
        // context projection, so no event in these tests can carry one.
        AgentMessage::App { .. } => "app",
    }
}

/// The `turn_end.toolResults` of the FIRST turn that produced any.
pub(super) fn first_turn_results(events: &[AgentEvent]) -> Vec<ToolResultMessage> {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnEnd { tool_results, .. } if !tool_results.is_empty() => {
                Some(tool_results.clone())
            }
            _ => None,
        })
        .expect("a turn_end carrying tool results")
}

/// The last assistant message published through `message_end`.
pub(super) fn last_assistant(events: &[AgentEvent]) -> AssistantMessage {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::MessageEnd {
                message: AgentMessage::Assistant(a),
            } => Some((**a).clone()),
            _ => None,
        })
        .expect("an assistant message_end")
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// A trivial tool that echoes its arguments back to the model and counts its executions.
pub(super) struct EchoTool {
    name: String,
    params: Value,
    calls: Arc<AtomicUsize>,
}

impl EchoTool {
    /// The tool AND its execution counter, for callers that assert on how often it ran.
    pub(super) fn new(name: &str) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                name: name.into(),
                params: obj_schema(),
                calls: calls.clone(),
            }),
            calls,
        )
    }

    /// Just the tool, for callers that only need something to dispatch to.
    pub(super) fn named(name: &str) -> Arc<Self> {
        Self::new(name).0
    }
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult {
            content: vec![Content::text(format!("echo:{params}"))],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

// ---------------------------------------------------------------------------
// Hooks that fail
// ---------------------------------------------------------------------------

/// A `prepare_next_turn` that panics with the given payload, for the synthetic-closing-sequence
/// tests (Pi `handleRunFailure`, agent.ts:496-511).
pub(super) struct PanicHook(&'static str);

impl PanicHook {
    pub(super) fn new(payload: &'static str) -> Self {
        Self(payload)
    }
}

#[async_trait::async_trait]
impl Hooks for PanicHook {
    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        panic!("{}", self.0);
    }
}

/// A `transform_context` that fails with the given message (Pi: throws), so the run ends through
/// `handleRunFailure` carrying that exact reason.
pub(super) struct FailingTransform(&'static str);

impl FailingTransform {
    pub(super) fn new(message: &'static str) -> Self {
        Self(message)
    }
}

#[async_trait::async_trait]
impl Hooks for FailingTransform {
    async fn transform_context(
        &self,
        _msgs: Vec<Arc<AgentMessage>>,
        _cancel: CancelToken,
    ) -> Result<Vec<Arc<AgentMessage>>, HookError> {
        Err(HookError::new(self.0))
    }
}
