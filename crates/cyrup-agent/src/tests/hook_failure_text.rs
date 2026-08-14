//! A failing hook's OWN message must reach the observer — never a fixed generic label.
//!
//! Pi provenance (all at the ported baseline v0.83.0):
//!
//! - `prepareToolCall`'s catch wraps the `beforeToolCall` await and returns
//!   `createErrorToolResult(error instanceof Error ? error.message : String(error))`
//!   (`packages/agent/src/agent-loop.ts:657-662`).
//! - `finalizeExecutedToolCall`'s catch replaces the whole result with
//!   `createErrorToolResult(error instanceof Error ? error.message : String(error))`
//!   (`agent-loop.ts:743-745`).
//! - `transformContext` / `convertToLlm` are awaited BARE in `streamAssistantResponse`
//!   (`agent-loop.ts:288-295`), so a throw unwinds into `runWithLifecycle`'s catch
//!   (`packages/agent/src/agent.ts:488-490`) and lands in `handleRunFailure`, whose synthetic
//!   assistant carries `errorMessage: error instanceof Error ? error.message : String(error)`
//!   (`agent.ts:504`).
//!
//! Cyrup's `HookError` is `#[error("{0}")]` over the caller's string (`src/error.rs:5-11`), so
//! `e.to_string()` is the exact analog of Pi's `error.message`. The regression these tests guard
//! is binding `Err(_)` and substituting a constant such as `"beforeToolCall failed"`, which drops
//! the reason on the floor: the model retries the same blocked call and the user sees a label with
//! no cause.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::print_stdout
)]

use std::sync::Arc;
use std::sync::Mutex;

use crate::{
    AfterOverride, AfterToolCall, Agent, AgentEvent, AgentMessage, BeforeOutcome, BeforeToolCall,
    EventSubscriber, HookError, Hooks, ProviderStreamFn, StreamFn, ToolResultMessage,
};
use cyrup_core::{
    CancelToken, Content, Message, ModelRef, StopReason, Tool, ToolCallId, ToolError, ToolResult,
    ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn model_ref() -> ModelRef {
    ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
}

fn faux_stream_fn(responses: Vec<cyrup_core::AssistantMessage>) -> Arc<dyn StreamFn> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn cyrup_provider::Provider> = faux;
    Arc::new(ProviderStreamFn::new(provider))
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<AgentEvent>>,
}

#[async_trait::async_trait]
impl EventSubscriber for Recorder {
    async fn on_event(&self, event: &AgentEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

impl Recorder {
    fn snapshot(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }
}

/// The `turn_end.toolResults` of the first turn that produced any.
fn first_turn_results(events: &[AgentEvent]) -> Vec<ToolResultMessage> {
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

fn last_assistant(events: &[AgentEvent]) -> cyrup_core::AssistantMessage {
    events
        .iter()
        .rev()
        .find_map(|e| match e {
            AgentEvent::MessageEnd { message: AgentMessage::Assistant(a) } => Some(a.clone()),
            _ => None,
        })
        .expect("an assistant message_end")
}

/// The concatenated text of a tool result's content blocks.
fn result_text(t: &ToolResultMessage) -> String {
    t.content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// A trivial tool so a tool-call turn has something to dispatch to.
struct EchoTool {
    params: Value,
}

impl EchoTool {
    fn new() -> Arc<Self> {
        Arc::new(Self { params: json!({ "type": "object" }) })
    }
}

#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
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
            content: vec![Content::text("ran:echo")],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            terminate: false,
        })
    }
}

// ---------------------------------------------------------------------------
// before_tool_call — agent-loop.ts:657-662
// ---------------------------------------------------------------------------

struct FailingBefore;

#[async_trait::async_trait]
impl Hooks for FailingBefore {
    async fn before_tool_call(
        &self,
        _ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        Err(HookError::new("policy store unreachable: /var/run/gate.sock"))
    }
}

/// The tool result handed back to the model is the hook's own message, so the model can adapt
/// instead of retrying the identical call (Pi `createErrorToolResult(error.message)`,
/// agent-loop.ts:660).
#[tokio::test]
async fn failing_before_tool_call_surfaces_the_hooks_own_message() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::new()])
        .hooks(Arc::new(FailingBefore))
        .build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert!(te[0].is_error, "a throwing before_tool_call yields an isError result");
    assert_eq!(
        result_text(&te[0]),
        "policy store unreachable: /var/run/gate.sock",
        "the hook's own text must reach the model, not a fixed label"
    );
}

// ---------------------------------------------------------------------------
// after_tool_call — agent-loop.ts:743-745
// ---------------------------------------------------------------------------

struct FailingAfter;

#[async_trait::async_trait]
impl Hooks for FailingAfter {
    async fn after_tool_call(
        &self,
        _ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        Err(HookError::new("redaction pass failed on block 3"))
    }
}

/// The replacement result's content is the hook's message (Pi
/// `result = createErrorToolResult(error.message)`, agent-loop.ts:744) — the existing
/// `throwing_after_tool_call_clears_usage_and_anchor` test covers the cleared usage/anchor but
/// never looked at the text.
#[tokio::test]
async fn failing_after_tool_call_surfaces_the_hooks_own_message() {
    let sf = faux_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![EchoTool::new()])
        .hooks(Arc::new(FailingAfter))
        .build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let te = first_turn_results(&rec.snapshot());
    assert!(te[0].is_error);
    assert_eq!(
        result_text(&te[0]),
        "redaction pass failed on block 3",
        "the hook's own text must replace the result, not a fixed label"
    );
}

// ---------------------------------------------------------------------------
// transform_context / convert_to_llm — agent-loop.ts:288-295 → agent.ts:504
// ---------------------------------------------------------------------------

struct FailingTransform;

#[async_trait::async_trait]
impl Hooks for FailingTransform {
    async fn transform_context(
        &self,
        _msgs: Vec<AgentMessage>,
        _cancel: CancelToken,
    ) -> Result<Vec<AgentMessage>, HookError> {
        Err(HookError::new("compaction budget exceeded: 412k > 200k"))
    }
}

/// A throwing `transform_context` ends the run with the hook's own reason as the assistant's
/// `errorMessage` (Pi `handleRunFailure`, agent.ts:504).
#[tokio::test]
async fn failing_transform_context_surfaces_the_hooks_own_message() {
    let sf = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_text("unused")],
        StopReason::Stop,
    )]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(FailingTransform)).build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let failure = last_assistant(&rec.snapshot());
    assert_eq!(failure.stop_reason, StopReason::Error);
    assert_eq!(
        failure.error_message.as_deref(),
        Some("compaction budget exceeded: 412k > 200k"),
        "the user must see the hook's cause, not a fixed label"
    );
}

struct FailingConvert;

#[async_trait::async_trait]
impl Hooks for FailingConvert {
    async fn convert_to_llm(&self, _msgs: &[AgentMessage]) -> Result<Vec<Message>, HookError> {
        Err(HookError::new("custom role \"memo\" has no LLM projection"))
    }
}

/// Same for `convert_to_llm`, awaited bare one line later (agent-loop.ts:295).
#[tokio::test]
async fn failing_convert_to_llm_surfaces_the_hooks_own_message() {
    let sf = faux_stream_fn(vec![faux_assistant_message(
        vec![faux_text("unused")],
        StopReason::Stop,
    )]);
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(FailingConvert)).build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    let failure = last_assistant(&rec.snapshot());
    assert_eq!(failure.stop_reason, StopReason::Error);
    assert_eq!(
        failure.error_message.as_deref(),
        Some("custom role \"memo\" has no LLM projection"),
        "the user must see the hook's cause, not a fixed label"
    );
}
