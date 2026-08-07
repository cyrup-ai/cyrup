//! Model-boundary conformance: the payload the agent forwards to the provider (tool descriptions,
//! reasoning level, generation params, static key fallback), the structured streaming partial
//! (distinct text/thinking/toolCall blocks), enriched hook contexts, `prepare_arguments`,
//! `skip_initial_steering_poll`, `tool_execution_end.result.terminate`, the synthetic
//! `handleRunFailure` closing sequence, and the small Agent accessors (`signal`,
//! `has_queued_messages`, unconditional `reset`, `prompt_with_images`). Each closes a gap from
//! spec/gap-analysis/03-cyrup-agent.md and cites the Pi behavior it mirrors.
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
    Agent, AgentEvent, AgentMessage, BeforeOutcome, BeforeToolCall, EventSubscriber, Hooks,
    HookError, PostTurn, ProviderStreamFn, StreamFn, TurnUpdate,
};
use cyrup_core::{
    CancelToken, Content, EventStream, ModelRef, ModelThinkingLevel, StopReason, Tool, ToolCallId,
    ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_thinking, faux_tool_call, FauxProvider};
use cyrup_provider::{CacheRetention, Context, Provider, StreamEvent, StreamOptions, Transport};
use serde_json::{json, Value};

// ----------------------------------------------------------------------------
// Wiring
// ----------------------------------------------------------------------------

fn model_ref() -> ModelRef {
    ModelRef { provider: "faux".into(), api: Some("faux".into()), model: "faux-1".into() }
}

/// What the agent forwarded into one provider call.
#[derive(Clone, Default)]
struct Captured {
    tools: Vec<(String, String)>, // (name, description)
    reasoning: ModelThinkingLevel,
    temperature: Option<f32>,
    max_tokens: Option<u64>,
    cache_retention: Option<CacheRetention>,
    transport: Option<Transport>,
    max_retry_delay_ms: Option<u64>,
    max_retries: Option<u32>,
    api_key: Option<String>,
    system_prompt: Option<String>,
    env: Option<cyrup_provider::ProviderEnv>,
    timeout_ms: Option<u64>,
    headers: Option<cyrup_provider::HeaderMap>,
}

/// A `StreamFn` that records the forwarded `Context`/`StreamOptions`, then delegates to a faux
/// provider for the actual event stream.
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
            tools: ctx.tools.iter().map(|t| (t.name.clone(), t.description.clone())).collect(),
            reasoning: opts.reasoning,
            temperature: opts.temperature,
            max_tokens: opts.max_tokens,
            cache_retention: opts.cache_retention,
            transport: opts.transport,
            max_retry_delay_ms: opts.max_retry_delay_ms,
            max_retries: opts.max_retries,
            api_key: opts.api_key.clone(),
            system_prompt: ctx.system_prompt.clone(),
            env: opts.env.clone(),
            timeout_ms: opts.timeout_ms,
            headers: opts.headers.clone(),
        });
        self.inner.stream(model, ctx, opts)
    }
}

fn recording_stream_fn(
    responses: Vec<cyrup_core::AssistantMessage>,
) -> (Arc<dyn StreamFn>, Arc<Mutex<Vec<Captured>>>) {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;
    let inner: Arc<dyn StreamFn> = Arc::new(ProviderStreamFn::new(provider));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let sf: Arc<dyn StreamFn> = Arc::new(RecordingStreamFn { inner, captured: captured.clone() });
    (sf, captured)
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

// ----------------------------------------------------------------------------
// Tools
// ----------------------------------------------------------------------------

/// A tool that carries a description + label, records its executed params, and (optionally) injects
/// a default field via `prepare_arguments`.
struct DescribedTool {
    name: String,
    params: Value,
    inject: bool,
    seen: Arc<Mutex<Option<Value>>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for DescribedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "Echoes its arguments back to the model."
    }
    fn label(&self) -> Option<&str> {
        Some("Echo")
    }
    async fn prepare_arguments(&self, mut args: Value) -> Value {
        if self.inject && let Value::Object(ref mut m) = args {
            m.insert("injected".into(), json!(true));
        }
        args
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        *self.seen.lock().unwrap() = Some(params);
        Ok(ToolResult { content: vec![Content::text("ok")], details: None, terminate: false, ..Default::default() })
    }
}

/// A tool that always sets `terminate` on its result.
struct TerminateTool {
    name: String,
    params: Value,
}

#[async_trait::async_trait]
impl Tool for TerminateTool {
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
        Ok(ToolResult { content: vec![Content::text("bye")], details: None, terminate: true, ..Default::default() })
    }
}

fn obj_schema() -> Value {
    json!({ "type": "object" })
}

// ----------------------------------------------------------------------------
// Gap #1: tool descriptions reach the provider.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap1_tool_description_forwarded_to_provider() {
    let seen = Arc::new(Mutex::new(None));
    let tool = Arc::new(DescribedTool {
        name: "echo".into(),
        params: obj_schema(),
        inject: false,
        seen: seen.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let (sf, captured) = recording_stream_fn(vec![faux_assistant_message(
        vec![faux_text("hi")],
        StopReason::Stop,
    )]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.prompt("go").await.unwrap().finished().await;

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 1);
    assert_eq!(cap[0].tools.len(), 1);
    assert_eq!(cap[0].tools[0].0, "echo");
    assert_eq!(
        cap[0].tools[0].1, "Echoes its arguments back to the model.",
        "tool description must reach the model (was String::new())"
    );
}

// ----------------------------------------------------------------------------
// Gap #3/#5/#6/#8: reasoning + generation params forwarded into StreamOptions.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap3_5_6_8_generation_params_forwarded() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf)
        .system_prompt("sys")
        .thinking_level(ModelThinkingLevel::Medium)
        .temperature(0.25)
        .max_tokens(512)
        .cache_retention(CacheRetention::Long)
        .transport(Transport::Sse)
        .max_retry_delay_ms(7000)
        .max_retries(4)
        .build();
    agent.prompt("go").await.unwrap().finished().await;

    let cap = captured.lock().unwrap();
    let c = &cap[0];
    assert_eq!(c.reasoning, ModelThinkingLevel::Medium, "thinking level must forward as reasoning");
    assert_eq!(c.temperature, Some(0.25));
    assert_eq!(c.max_tokens, Some(512));
    assert_eq!(c.cache_retention, Some(CacheRetention::Long));
    assert_eq!(c.transport, Some(Transport::Sse));
    assert_eq!(c.max_retry_delay_ms, Some(7000));
    assert_eq!(c.max_retries, Some(4));
    assert_eq!(c.system_prompt.as_deref(), Some("sys"));
}

#[tokio::test]
async fn provider_env_overlay_and_timeout_forward_to_stream_options() {
    // The HTTP-proxy overlay + idle timeout (Pi `applyHttpProxySettings`/`configureHttpDispatcher`,
    // main.ts:744-745) must reach `StreamOptions.env`/`timeout_ms` so the provider's proxy resolver
    // honors the configured proxy.
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let mut overlay = cyrup_provider::ProviderEnv::new();
    overlay.insert("HTTP_PROXY".to_string(), "http://proxy.local:8080".to_string());
    overlay.insert("HTTPS_PROXY".to_string(), "http://proxy.local:8080".to_string());
    let agent = Agent::builder(model_ref(), sf)
        .provider_env(overlay)
        .timeout_ms(120_000)
        .build();
    agent.prompt("go").await.unwrap().finished().await;

    let cap = captured.lock().unwrap();
    let c = &cap[0];
    let env = c.env.as_ref().expect("the provider env overlay must forward");
    assert_eq!(env.get("HTTP_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
    assert_eq!(env.get("HTTPS_PROXY").map(String::as_str), Some("http://proxy.local:8080"));
    assert_eq!(c.timeout_ms, Some(120_000));
}

#[tokio::test]
async fn transport_defaults_to_auto_like_pi() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).build();
    agent.prompt("go").await.unwrap().finished().await;
    assert_eq!(captured.lock().unwrap()[0].transport, Some(Transport::Auto));
}

// ----------------------------------------------------------------------------
// Gap #21: static api_key fallback forwarded when no resolver yields a key.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap21_static_api_key_fallback() {
    let (sf, captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).api_key("sk-static").build();
    agent.prompt("go").await.unwrap().finished().await;
    assert_eq!(captured.lock().unwrap()[0].api_key.as_deref(), Some("sk-static"));
}

// ----------------------------------------------------------------------------
// Gap #3 (per-turn override): TurnUpdate.thinking_level overrides reasoning for the next request.
// ----------------------------------------------------------------------------

struct BumpThinkingHook {
    bumped: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Hooks for BumpThinkingHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>) -> Result<Option<TurnUpdate>, HookError> {
        // Only override once (turn 0 → turn 1).
        if self.bumped.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(Some(TurnUpdate {
                thinking_level: Some(ModelThinkingLevel::High),
                ..TurnUpdate::default()
            }))
        } else {
            Ok(None)
        }
    }
    async fn should_stop_after_turn(&self, ctx: PostTurn<'_>) -> Result<bool, HookError> {
        // Stop after the second assistant turn.
        Ok(ctx.turn_index >= 2)
    }
}

#[tokio::test]
async fn gap3_turn_update_thinking_level_overrides_next_request() {
    // First turn: a tool call (forces a second request); second turn: plain text.
    let (sf, captured) = recording_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let seen = Arc::new(Mutex::new(None));
    let tool = Arc::new(DescribedTool {
        name: "echo".into(),
        params: obj_schema(),
        inject: false,
        seen,
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let agent = Agent::builder(model_ref(), sf)
        .thinking_level(ModelThinkingLevel::Low)
        .tools(vec![tool])
        .hooks(Arc::new(BumpThinkingHook { bumped: Arc::new(AtomicUsize::new(0)) }))
        .build();
    agent.prompt("go").await.unwrap().finished().await;

    let cap = captured.lock().unwrap();
    assert_eq!(cap.len(), 2, "two provider requests");
    assert_eq!(cap[0].reasoning, ModelThinkingLevel::Low, "turn 1 uses the run default");
    assert_eq!(cap[1].reasoning, ModelThinkingLevel::High, "turn 2 uses the TurnUpdate override");
}

// ----------------------------------------------------------------------------
// Gap #9/#10/#11/#29: structured streaming partial — distinct thinking/text/toolCall blocks.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap9_10_11_streaming_partial_keeps_distinct_blocks() {
    // A response that streams thinking, then text, then a tool call.
    let (sf, _captured) = recording_stream_fn(vec![
        faux_assistant_message(
            vec![faux_thinking("let me think"), faux_text("answer"), faux_tool_call("echo", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let seen = Arc::new(Mutex::new(None));
    let tool = Arc::new(DescribedTool {
        name: "echo".into(),
        params: obj_schema(),
        inject: false,
        seen,
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;

    // Inspect the partials carried on message_update.
    let mut saw_thinking_block = false;
    let mut saw_text_block = false;
    let mut saw_toolcall_block = false;
    for ev in rec.snapshot() {
        if let AgentEvent::MessageUpdate { message: AgentMessage::Assistant(a), .. } = ev {
            for c in &a.content {
                match c {
                    Content::Thinking { thinking, .. } if !thinking.is_empty() => {
                        saw_thinking_block = true;
                    }
                    Content::Text { text, .. } if !text.is_empty() => saw_text_block = true,
                    Content::ToolCall(_) => saw_toolcall_block = true,
                    _ => {}
                }
            }
        }
    }
    assert!(saw_thinking_block, "partial must carry a distinct Thinking block (not merged into text)");
    assert!(saw_text_block, "partial must carry a Text block");
    assert!(saw_toolcall_block, "streaming tool call must appear in the partial (was never shown)");
}

// ----------------------------------------------------------------------------
// Gap #18: prepare_arguments normalizes raw args before validation/execution.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap18_prepare_arguments_runs_before_execute() {
    let seen = Arc::new(Mutex::new(None));
    let tool = Arc::new(DescribedTool {
        name: "echo".into(),
        params: obj_schema(),
        inject: true,
        seen: seen.clone(),
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let (sf, _captured) = recording_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({"a": 1}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    agent.prompt("go").await.unwrap().finished().await;

    let got = seen.lock().unwrap().clone().expect("tool executed");
    assert_eq!(got["a"], json!(1), "original arg preserved");
    assert_eq!(got["injected"], json!(true), "prepare_arguments injected a default before execute");
}

// ----------------------------------------------------------------------------
// Gap #15: before_tool_call sees assistant_message, full tool_call, and the context view.
// ----------------------------------------------------------------------------

struct InspectHook {
    saw_tool_call_name: Arc<Mutex<Option<String>>>,
    saw_assistant_has_call: Arc<Mutex<bool>>,
    saw_system_prompt: Arc<Mutex<Option<String>>>,
    saw_tool_count: Arc<Mutex<usize>>,
}

#[async_trait::async_trait]
impl Hooks for InspectHook {
    async fn before_tool_call(
        &self,
        ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        *self.saw_tool_call_name.lock().unwrap() = Some(ctx.tool_call.name.clone());
        let has_call = ctx
            .assistant_message
            .content
            .iter()
            .any(|c| matches!(c, Content::ToolCall(_)));
        *self.saw_assistant_has_call.lock().unwrap() = has_call;
        *self.saw_system_prompt.lock().unwrap() = Some(ctx.context.system_prompt.to_string());
        *self.saw_tool_count.lock().unwrap() = ctx.context.tools.len();
        Ok(BeforeOutcome::Proceed)
    }
}

#[tokio::test]
async fn gap15_before_tool_call_context_enriched() {
    let seen = Arc::new(Mutex::new(None));
    let tool = Arc::new(DescribedTool {
        name: "echo".into(),
        params: obj_schema(),
        inject: false,
        seen,
        calls: Arc::new(AtomicUsize::new(0)),
    });
    let (sf, _captured) = recording_stream_fn(vec![
        faux_assistant_message(vec![faux_tool_call("echo", json!({}))], StopReason::ToolUse),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let hook = Arc::new(InspectHook {
        saw_tool_call_name: Arc::new(Mutex::new(None)),
        saw_assistant_has_call: Arc::new(Mutex::new(false)),
        saw_system_prompt: Arc::new(Mutex::new(None)),
        saw_tool_count: Arc::new(Mutex::new(0)),
    });
    let agent = Agent::builder(model_ref(), sf)
        .system_prompt("the-sys-prompt")
        .tools(vec![tool])
        .hooks(hook.clone())
        .build();
    agent.prompt("go").await.unwrap().finished().await;

    assert_eq!(hook.saw_tool_call_name.lock().unwrap().as_deref(), Some("echo"));
    assert!(*hook.saw_assistant_has_call.lock().unwrap(), "assistant_message carries the tool call");
    assert_eq!(hook.saw_system_prompt.lock().unwrap().as_deref(), Some("the-sys-prompt"));
    assert_eq!(*hook.saw_tool_count.lock().unwrap(), 1, "context view exposes the tools");
}

// ----------------------------------------------------------------------------
// Gap #26: tool_execution_end.result carries `terminate`.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap26_tool_execution_end_result_includes_terminate() {
    let tool = Arc::new(TerminateTool { name: "stop".into(), params: obj_schema() });
    let (sf, _captured) = recording_stream_fn(vec![faux_assistant_message(
        vec![faux_tool_call("stop", json!({}))],
        StopReason::ToolUse,
    )]);
    let agent = Agent::builder(model_ref(), sf).tools(vec![tool]).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.prompt("go").await.unwrap().finished().await;

    let end = rec
        .snapshot()
        .into_iter()
        .find_map(|e| match e {
            AgentEvent::ToolExecutionEnd { result, .. } => Some(result),
            _ => None,
        })
        .expect("a tool_execution_end");
    assert_eq!(end["terminate"], json!(true), "result must include the terminate hint");
}

// ----------------------------------------------------------------------------
// Gap #19: skip_initial_steering_poll on continue-from-assistant.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap19_continue_from_assistant_skips_initial_steering_poll() {
    // Transcript ends with an assistant message; two steering messages are queued. With
    // skip_initial_steering_poll, the SECOND steering message is processed AFTER the first
    // assistant turn completes, not jammed into the first turn alongside the drained prompt.
    let (sf, _captured) = recording_stream_fn(vec![
        faux_assistant_message(vec![faux_text("a1")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("a2")], StopReason::Stop),
    ]);
    let agent = Agent::builder(model_ref(), sf)
        .messages(vec![AgentMessage::Assistant(faux_assistant_message(
            vec![faux_text("seed")],
            StopReason::Stop,
        ))])
        .build();
    agent.steer(AgentMessage::user_text("steer-1"));
    agent.steer(AgentMessage::user_text("steer-2"));

    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    agent.continue_run().await.unwrap().finished().await;

    // Find the order of the user message_start events vs the first assistant message_end.
    let events = rec.snapshot();
    let user_text = |m: &AgentMessage| match m {
        AgentMessage::User { content, .. } => content.iter().find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        }),
        _ => None,
    };
    let mut steer1_at = None;
    let mut steer2_at = None;
    let mut first_assistant_end_at = None;
    for (i, ev) in events.iter().enumerate() {
        match ev {
            AgentEvent::MessageStart { message } => match user_text(message).as_deref() {
                Some("steer-1") => steer1_at.get_or_insert(i),
                Some("steer-2") => steer2_at.get_or_insert(i),
                _ => continue,
            },
            AgentEvent::MessageEnd { message: AgentMessage::Assistant(_) } => {
                first_assistant_end_at.get_or_insert(i)
            }
            _ => continue,
        };
    }
    let s1 = steer1_at.expect("steer-1 processed");
    let s2 = steer2_at.expect("steer-2 processed");
    let a_end = first_assistant_end_at.expect("an assistant completed");
    assert!(s1 < a_end, "first steering message is the prompt (before the first assistant)");
    assert!(
        s2 > a_end,
        "second steering message must be deferred past the first assistant turn (skip flag)"
    );
}

// ----------------------------------------------------------------------------
// Gap #20: synthetic closing sequence when the run task unwinds (a hook panics).
// ----------------------------------------------------------------------------

struct PanicHook;

#[async_trait::async_trait]
impl Hooks for PanicHook {
    async fn prepare_next_turn(&self, _ctx: PostTurn<'_>) -> Result<Option<TurnUpdate>, HookError> {
        panic!("hook exploded");
    }
}

#[tokio::test]
async fn gap20_run_failure_emits_synthetic_closing_sequence() {
    let (sf, _captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("a1")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).hooks(Arc::new(PanicHook)).build();
    let rec = Arc::new(Recorder::default());
    agent.subscribe(rec.clone());
    let new = agent.prompt("go").await.unwrap().finished().await;

    let events = rec.snapshot();
    // The stream must terminate with agent_end even though a hook panicked.
    assert!(
        matches!(events.last(), Some(AgentEvent::AgentEnd { .. })),
        "the run must always end with agent_end (Pi handleRunFailure)"
    );
    // A synthetic error assistant message closes the run.
    let last_assistant_errored = new.iter().rev().find_map(|m| match m {
        AgentMessage::Assistant(a) => Some(a.stop_reason == StopReason::Error),
        _ => None,
    });
    assert_eq!(last_assistant_errored, Some(true), "synthetic assistant carries stop_reason error");
    // wait_for_idle must not deadlock after a failed run.
    agent.wait_for_idle().await;
}

// ----------------------------------------------------------------------------
// Gaps #22/#23/#24/#25: small Agent accessors.
// ----------------------------------------------------------------------------

#[tokio::test]
async fn gap23_24_signal_and_has_queued_messages() {
    let (sf, _captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).build();
    assert!(agent.signal().is_none(), "no signal before a run");
    assert!(!agent.has_queued_messages());
    agent.follow_up(AgentMessage::user_text("later"));
    assert!(agent.has_queued_messages(), "queued follow-up is observable");
}

#[tokio::test]
async fn gap25_reset_is_unconditional() {
    let (sf, _captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf)
        .messages(vec![AgentMessage::user_text("seed")])
        .build();
    agent.steer(AgentMessage::user_text("q"));
    agent.reset().await.unwrap();
    let snap = agent.snapshot().await;
    assert!(snap.messages.is_empty(), "reset clears the transcript");
    assert!(!agent.has_queued_messages(), "reset clears the queues");
}

#[tokio::test]
async fn gap22_prompt_with_images_builds_multimodal_user_message() {
    let (sf, _captured) =
        recording_stream_fn(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]);
    let agent = Agent::builder(model_ref(), sf).build();
    let image = Content::Image { data: "aGk=".into(), mime_type: "image/png".into() };
    let new = agent
        .prompt_with_images("look at this", vec![image])
        .await
        .unwrap()
        .finished()
        .await;
    let user = new.iter().find_map(|m| match m {
        AgentMessage::User { content, .. } => Some(content.clone()),
        _ => None,
    });
    let content = user.expect("a user message");
    assert!(matches!(content.first(), Some(Content::Text { .. })), "text first");
    assert!(
        content.iter().any(|c| matches!(c, Content::Image { .. })),
        "image attached after the text"
    );
}

/// `Agent::set_headers` must change what the NEXT request actually carries.
///
/// pi recomputes provider-attribution and opencode session-affinity headers inside `streamFn`, on
/// the model the request is going to (`sdk.ts:318-327`). cyrup held them in `GenerationConfig`,
/// fixed at build, so a cross-provider `/model` switch kept sending the previous provider's
/// attribution — an OpenRouter `HTTP-Referer`/`X-Title` on an Anthropic request.
///
/// This asserts at the only level that discriminates: what reaches `StreamOptions`. Two earlier
/// attempts at a session-level test did NOT — with no model switch the pinned build-time map
/// trivially equals the active model's attribution, and the faux catalog offers no
/// attribution-distinguishable pair to switch between. Both passed against the defect.
#[tokio::test]
async fn set_headers_repoints_the_next_requests_header_overlay() {
    let (sf, captured) = recording_stream_fn(vec![
        faux_assistant_message(vec![faux_text("one")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("two")], StopReason::Stop),
    ]);

    let mut built = cyrup_provider::HeaderMap::new();
    built.insert("x-attribution".to_string(), Some("first-provider".to_string()));
    let agent = Agent::builder(model_ref(), sf).headers(built.clone()).build();

    agent.prompt("go").await.expect("first turn");
    agent.wait_for_idle().await;

    // The builder value reaches the first request.
    assert_eq!(
        captured.lock().unwrap()[0].headers.as_ref().and_then(|h| h.get("x-attribution")),
        Some(&Some("first-provider".to_string())),
        "the build-time overlay reaches the first request"
    );

    // Now repoint it, as a `/model` switch does.
    let mut switched = cyrup_provider::HeaderMap::new();
    switched.insert("x-attribution".to_string(), Some("second-provider".to_string()));
    agent.set_headers(Some(switched)).await;

    agent.prompt("go again").await.expect("second turn");
    agent.wait_for_idle().await;

    let seen = captured.lock().unwrap();
    assert_eq!(seen.len(), 2, "two requests were made");
    assert_eq!(
        seen[1].headers.as_ref().and_then(|h| h.get("x-attribution")),
        Some(&Some("second-provider".to_string())),
        "the SECOND request must carry the repointed overlay, not the pinned build-time one"
    );
}
