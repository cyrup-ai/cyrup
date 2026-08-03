//! Round-5 untracked-miss closures (spec/gap-analysis/03-cyrup-agent.md), each proven against the
//! ASSEMBLED loop behavior — not a hand-built isolated case — and cross-referenced to the Pi source:
//!
//! - #1 snapshot isolation: a `prepare_next_turn` context override reaches the LLM payload of the
//!   next turn (Pi `currentContext`) but does NOT leak into the observable `agent.state.messages`
//!   (Pi `_state.messages` grows independently), agent.ts:424-429,519-522; agent-loop.ts:228.
//! - #2 synthetic failure message carries `[{type:"text", text:""}]`, NOT empty content (agent.ts:497).
//! - #3 `Date.now()` timestamps on prompt user + tool-result messages REACH the wire payload
//!   (agent.ts:393; agent-loop.ts:741), asserted on the captured `convert_to_llm` output.
//! - #4 abort returns the ACCUMULATED partial content with `stopReason:"aborted"` (agent-loop.ts:344).
//! - #6 the loop stops consuming the stream on the `done` terminal — a post-terminal event yields no
//!   stray `message_update` (agent-loop.ts:342-355).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::print_stdout
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use cyrup_agent::{
    Agent, AgentEvent, AgentMessage, EventSubscriber, HookError, Hooks, PostTurn, ProviderStreamFn,
    StreamFn, TurnUpdate,
};
use cyrup_core::{
    CancelToken, Content, EventStream, Message, ModelRef, StopReason, Tool, ToolCallId, ToolError,
    ToolResult, ToolUpdate, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::{Context, Provider, StreamEvent, StreamOptions};
use futures::StreamExt;
use serde_json::{json, Value};

fn model_ref() -> ModelRef {
    ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
}

// ---------------------------------------------------------------------------
// A StreamFn that records the full LLM payload (Context.messages) of every call, then delegates to
// a faux provider for the actual stream — so we can byte-check the assembled payload.
// ---------------------------------------------------------------------------

struct PayloadRecordingStreamFn {
    inner: Arc<dyn StreamFn>,
    payloads: Arc<Mutex<Vec<Vec<Message>>>>,
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

/// Captured LLM payloads: one `Vec<Message>` per provider request.
type PayloadLog = Arc<Mutex<Vec<Vec<Message>>>>;

fn payload_recording(
    responses: Vec<cyrup_core::AssistantMessage>,
) -> (Arc<dyn StreamFn>, PayloadLog) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;
    let inner: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));
    let payloads = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> =
        Arc::new(PayloadRecordingStreamFn { inner, payloads: payloads.clone() });
    (sf, payloads)
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

struct EchoTool {
    name: String,
    params: Value,
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
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult { content: vec![Content::text("ok")], details: None, terminate: false, ..Default::default() })
    }
}

fn obj_schema() -> Value {
    json!({ "type": "object" })
}

fn user_text_of(m: &Message) -> Option<String> {
    match m {
        Message::User { content, .. } => content.iter().find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

fn agent_user_text_of(m: &AgentMessage) -> Option<String> {
    match m {
        AgentMessage::User { content, .. } => content.iter().find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    }
}

// ===========================================================================
// Miss #1 — snapshot isolation: the loop SNAPSHOTS messages; a context override hits the loop's
// LLM payload but NOT the observable agent.state.messages.
// ===========================================================================

struct ContextOverrideHook {
    turns: AtomicUsize,
}

#[async_trait::async_trait]
impl Hooks for ContextOverrideHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>) -> Result<Option<TurnUpdate>, HookError> {
        // After the FIRST turn only, replace the loop's working context with a sentinel transcript.
        // Pi `currentContext = snapshot.context` (agent-loop.ts:228) replaces only the loop copy.
        if self.turns.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Some(TurnUpdate {
                context: Some(vec![AgentMessage::user_text("CTXOVERRIDE")]),
                ..TurnUpdate::default()
            }))
        } else {
            Ok(None)
        }
    }
    async fn should_stop_after_turn(&self, ctx: PostTurn<'_>) -> Result<bool, HookError> {
        Ok(ctx.turn_index >= 2)
    }
}

#[tokio::test]
async fn miss1_context_override_isolated_from_observable_state() {
    // turn0: a tool call (forces turn1); turn1: plain text.
    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let tool = Arc::new(EchoTool { name: "echo".into(), params: obj_schema() });
    let agent = Agent::builder(model_ref(), sf)
        .tools(vec![tool])
        .hooks(Arc::new(ContextOverrideHook { turns: AtomicUsize::new(0) }))
        .build();
    agent.prompt("go").await.unwrap().finished().await;

    let captured = { payloads.lock().unwrap().clone() };
    assert_eq!(captured.len(), 2, "two provider requests");

    // turn1's payload must reflect the override — Pi reads the loop's `currentContext.messages`.
    let turn1 = &captured[1];
    assert!(
        turn1.iter().filter_map(user_text_of).any(|t| t == "CTXOVERRIDE"),
        "the prepare_next_turn context override must reach the NEXT turn's LLM payload"
    );
    assert!(
        !turn1.iter().filter_map(user_text_of).any(|t| t == "go"),
        "the override replaced the loop's working transcript, so the original prompt is absent"
    );

    // The observable agent state must NOT have been clobbered by the override: it grew the natural
    // event-driven transcript (Pi `_state.messages` via processEvents, agent.ts:519-522).
    let snap = agent.snapshot().await;
    assert!(
        snap.messages.iter().filter_map(agent_user_text_of).any(|t| t == "go"),
        "the original prompt must remain in observable agent.state.messages (override must NOT leak)"
    );
    assert!(
        !snap.messages.iter().filter_map(agent_user_text_of).any(|t| t == "CTXOVERRIDE"),
        "the context override must NOT appear in observable agent.state.messages"
    );
    // Natural transcript: user prompt + assistant(toolcall) + toolResult + assistant(done) = 4.
    assert_eq!(snap.messages.len(), 4, "observable state holds the full natural transcript");
}

#[tokio::test]
async fn miss1_mid_run_set_messages_does_not_leak_into_loop_payload() {
    // A subscriber mutates agent.state.messages mid-run (on the first assistant message_end). Pi
    // isolates the loop's `currentContext` from `_state.messages`, so the loop's next payload must
    // NOT pick up the external write.
    struct Meddler {
        agent: Mutex<Option<Arc<Agent>>>,
        fired: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl EventSubscriber for Meddler {
        async fn on_event(&self, event: &AgentEvent) {
            if let AgentEvent::MessageEnd { message: AgentMessage::Assistant(_) } = event
                && self.fired.fetch_add(1, Ordering::SeqCst) == 0
            {
                let a = self.agent.lock().unwrap().clone();
                if let Some(a) = a {
                    a.set_messages(vec![AgentMessage::user_text("INJECTED")]).await;
                }
            }
        }
    }

    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let tool = Arc::new(EchoTool { name: "echo".into(), params: obj_schema() });
    let agent = Arc::new(Agent::builder(model_ref(), sf).tools(vec![tool]).build());
    let meddler = Arc::new(Meddler { agent: Mutex::new(Some(agent.clone())), fired: AtomicUsize::new(0) });
    agent.subscribe(meddler);
    agent.prompt("go").await.unwrap().finished().await;

    let payloads = payloads.lock().unwrap();
    assert_eq!(payloads.len(), 2);
    // The loop's working copy is isolated; the external set_messages must NOT appear in turn1's
    // payload (it stays the natural prompt+assistant+toolResult transcript).
    assert!(
        !payloads[1].iter().filter_map(user_text_of).any(|t| t == "INJECTED"),
        "a mid-run external set_messages must NOT leak into the loop's LLM payload"
    );
    assert!(
        payloads[1].iter().filter_map(user_text_of).any(|t| t == "go"),
        "the loop's working transcript keeps the original prompt"
    );
}

// ===========================================================================
// Miss #3 — Date.now() timestamps on prompt user + tool-result messages REACH the wire payload.
// ===========================================================================

#[tokio::test]
async fn miss3_timestamps_reach_llm_payload() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let (sf, payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let tool = Arc::new(EchoTool { name: "echo".into(), params: obj_schema() });
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.prompt("go").await.unwrap().finished().await;

    let payloads = payloads.lock().unwrap();
    // turn1's payload contains the prompt user message AND the tool result — both stamped Date.now().
    let turn1 = &payloads[1];
    let user_ts = turn1
        .iter()
        .find_map(|m| match m {
            Message::User { timestamp, .. } => Some(*timestamp),
            _ => None,
        })
        .expect("a user message in the payload");
    let tool_ts = turn1
        .iter()
        .find_map(|m| match m {
            Message::ToolResult { timestamp, .. } => Some(*timestamp),
            _ => None,
        })
        .expect("a tool-result message in the payload");

    assert!(user_ts >= before, "prompt user timestamp must be a real Date.now() (was None/0): {user_ts}");
    assert!(tool_ts >= before, "tool-result timestamp must be a real Date.now() (was 0): {tool_ts}");
}

// ===========================================================================
// Miss #2/#3 — synthetic failure message: one empty text block + Date.now() (Pi handleRunFailure).
// ===========================================================================

struct PanicHook;

#[async_trait::async_trait]
impl Hooks for PanicHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>) -> Result<Option<TurnUpdate>, HookError> {
        panic!("boom");
    }
}

#[tokio::test]
async fn miss2_3_synthetic_failure_has_empty_text_block_and_timestamp() {
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let (sf, _payloads) =
        payload_recording(vec![faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(PanicHook)).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    let new = agent.prompt("go").await.unwrap().finished().await;

    let failure = new
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Assistant(a) if a.stop_reason == StopReason::Error => Some(a.clone()),
            _ => None,
        })
        .expect("a synthetic error assistant");
    assert_eq!(
        failure.content,
        vec![Content::text("")],
        "Pi handleRunFailure content is [{{type:text, text:\"\"}}], NOT empty content"
    );
    assert!(failure.timestamp >= before, "failure message carries a Date.now() timestamp");

    // The same shape must appear on the wire-bound message_end event.
    let end_ok = rec.snapshot().into_iter().any(|e| match e {
        AgentEvent::MessageEnd { message: AgentMessage::Assistant(a) } => {
            a.stop_reason == StopReason::Error && a.content == vec![Content::text("")]
        }
        _ => false,
    });
    assert!(end_ok, "the failure message_end carries the single empty text block");
}

// ===========================================================================
// Miss #4 — abort returns the ACCUMULATED partial content with stopReason aborted.
// ===========================================================================

/// A StreamFn that emits `start` + a `text_delta` whose partial already carries text, then PENDS
/// forever — so an abort fires while a non-empty partial is in flight.
struct HangingStreamFn;

impl StreamFn for HangingStreamFn {
    fn stream(
        &self,
        _model: &ModelRef,
        _ctx: &Context,
        _opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let empty = faux_assistant_message(vec![], StopReason::Stop);
        let with_text = faux_assistant_message(vec![faux_text("hello")], StopReason::Stop);
        let head = futures::stream::iter(vec![
            StreamEvent::Start { partial: empty },
            StreamEvent::TextDelta { content_index: 0, delta: "hello".into(), partial: with_text },
        ]);
        Box::pin(head.chain(futures::stream::pending()))
    }
}

#[tokio::test]
async fn miss4_abort_carries_streamed_partial_content() {
    let agent = Arc::new(Agent::builder(model_ref(), Arc::new(HangingStreamFn)).build());
    let handle = agent.prompt("go").await.unwrap();
    // Let the two stream events flow through, then abort while the partial ("hello") is in flight.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    agent.abort();
    let new = handle.finished().await;

    let aborted = new
        .iter()
        .rev()
        .find_map(|m| match m {
            AgentMessage::Assistant(a) if a.stop_reason == StopReason::Aborted => Some(a.clone()),
            _ => None,
        })
        .expect("an aborted assistant message");
    let has_hello = aborted.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "hello"));
    assert!(
        has_hello,
        "the aborted assistant must carry the ACCUMULATED partial content (was empty): {:?}",
        aborted.content
    );
    // Byte-1:1 abort terminal: stopReason `aborted` + Pi's uniform `errorMessage` string. Every
    // provider throws `new Error("Request was aborted")` on `signal.aborted` and the catch assigns
    // `output.errorMessage = error.message` (anthropic-messages.ts:718,733-734; faux.ts:291-297).
    assert_eq!(
        aborted.error_message.as_deref(),
        Some("Request was aborted"),
        "the aborted terminal must carry Pi's uniform abort errorMessage"
    );
}

// ===========================================================================
// Miss #6 — the loop stops consuming on the `done` terminal; a post-terminal event emits no stray
// message_update.
// ===========================================================================

/// A StreamFn that emits a valid turn ending in `done`, then an EXTRA `text_delta` AFTER the
/// terminal (a non-conforming provider). Pi returns on `done` and never sees the extra event.
struct PostTerminalStreamFn;

impl StreamFn for PostTerminalStreamFn {
    fn stream(
        &self,
        _model: &ModelRef,
        _ctx: &Context,
        _opts: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let empty = faux_assistant_message(vec![], StopReason::Stop);
        let ok = faux_assistant_message(vec![faux_text("ok")], StopReason::Stop);
        let leak = faux_assistant_message(vec![faux_text("LEAK")], StopReason::Stop);
        let events = vec![
            StreamEvent::Start { partial: empty.clone() },
            StreamEvent::TextDelta { content_index: 0, delta: "ok".into(), partial: ok.clone() },
            StreamEvent::terminal(ok),
            // Post-terminal stray event — must be ignored.
            StreamEvent::TextDelta { content_index: 0, delta: "LEAK".into(), partial: leak },
        ];
        Box::pin(futures::stream::iter(events))
    }
}

#[tokio::test]
async fn miss6_no_message_update_after_terminal() {
    let agent = Agent::builder(model_ref(), Arc::new(PostTerminalStreamFn)).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    let new = agent.prompt("go").await.unwrap().finished().await;

    // No message_update may carry the post-terminal "LEAK" partial.
    let leaked = rec.snapshot().into_iter().any(|e| match e {
        AgentEvent::MessageUpdate { message: AgentMessage::Assistant(a), .. } => {
            a.content.iter().any(|c| matches!(c, Content::Text { text, .. } if text == "LEAK"))
        }
        _ => false,
    });
    assert!(!leaked, "a post-terminal event must NOT produce a stray message_update (Pi returns on done)");

    // The final assistant message is the `done` message, not the post-terminal partial.
    let last = new.iter().rev().find_map(|m| match m {
        AgentMessage::Assistant(a) => Some(a.clone()),
        _ => None,
    });
    let text = last
        .map(|a| a.content.iter().filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        }).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(text.iter().any(|t| t == "ok"), "final message is the done terminal");
    assert!(!text.iter().any(|t| t == "LEAK"), "final message must not be overwritten by post-terminal partial");
}

// ===========================================================================
// Residual #1 — tool_execution_update.partialResult carries `terminate`
// (agent-loop.ts:641-653; AgentToolResult.terminate, types.ts:350-360).
// cyrup_core::ToolUpdate now mirrors Pi's AgentToolResult by carrying an optional `terminate`,
// threaded onto the emitted `partialResult` (omitted when None, matching Pi's `terminate?`).
// ===========================================================================

/// A tool that streams two partials: the first sets `terminate = Some(true)` (with a detail), the
/// second leaves it `None`, then settles.
struct UpdatingTool {
    name: String,
    params: Value,
}

#[async_trait::async_trait]
impl Tool for UpdatingTool {
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
        on_update(ToolUpdate {
            content: vec![Content::text("step")],
            details: Some(json!({ "k": 1 })),
            terminate: Some(true),
        });
        on_update(ToolUpdate {
            content: vec![Content::text("more")],
            details: None,
            terminate: None,
        });
        Ok(ToolResult { content: vec![Content::text("done")], details: None, terminate: false, ..Default::default() })
    }
}

#[tokio::test]
async fn residual1_tool_update_partial_result_carries_terminate_byte_for_byte() {
    let (sf, _payloads) = payload_recording(vec![
        faux_assistant_message(vec![faux_tool_call("upd", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);
    let tool = Arc::new(UpdatingTool { name: "upd".into(), params: obj_schema() });
    let rec = Arc::new(Recorder::default());
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.subscribe(rec.clone());

    agent.prompt("go").await.unwrap().finished().await;
    agent.wait_for_idle().await;

    // The `partialResult` JSON of every tool_execution_update, in order.
    let partials: Vec<Value> = rec
        .snapshot()
        .into_iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionUpdate { partial_result, .. } => Some(partial_result),
            _ => None,
        })
        .collect();
    assert_eq!(partials.len(), 2, "two streamed updates were emitted");

    // Byte-shape #1: a partial with `terminate = true` serializes the key with `true` (Pi passes the
    // tool's `AgentToolResult` straight onto `partialResult`).
    assert_eq!(
        partials[0],
        json!({
            "content": [{ "type": "text", "text": "step" }],
            "details": { "k": 1 },
            "terminate": true
        }),
        "first partialResult must carry terminate:true"
    );
    // Byte-shape #2: a partial with `terminate = None` OMITS the key entirely — exactly as Pi omits
    // an `undefined` `terminate?` (NOT a `null`).
    assert_eq!(
        partials[1],
        json!({
            "content": [{ "type": "text", "text": "more" }],
            "details": null
        }),
        "second partialResult must omit the terminate key"
    );
    assert!(
        partials[1].get("terminate").is_none(),
        "absent terminate must produce no key, matching Pi's optional field"
    );
}
