//! AGENT-047 — `run_tool_call`: one tool call run on behalf of a caller, through the same
//! preparation, validation and hooks as a model-issued one, emitting nothing and appending nothing.
//!
//! Pins pi's `runToolCall` contract (`packages/agent/src/agent-loop.ts:801-818` @v1.0.1):
//! *"Run one tool call through the same steps as a model-issued call … Emits no events and adds no
//! messages … Never rejects for tool failures: unknown tools, validation errors, blocked calls, and
//! thrown errors come back as `isError: true`."*

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::support::{EchoTool, EventRecorder, faux_stream_fn, model_ref, obj_schema};
use crate::hooks::{
    AfterOutcome, AfterOverride, AfterToolCall, AgentContextView, BeforeOutcome, BeforeToolCall,
    DefaultHooks,
};
use crate::{Agent, Hooks, RunToolCallOptions, ToolCallOutcome, run_tool_call};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, StopReason, Tool, ToolCall, ToolCallId, ToolError,
    ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::faux_assistant_message;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn assistant() -> AssistantMessage {
    faux_assistant_message(vec![Content::text("calling")], StopReason::ToolUse)
}

fn call(name: &str, args: Value) -> ToolCall {
    let arguments = match args {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    ToolCall {
        id: ToolCallId::from("nested-1"),
        name: name.to_string(),
        arguments: arguments.into(),
        thought_signature: None,
        namespace: None,
    }
}

/// The text the model would have seen, concatenated.
fn text_of(outcome: &ToolCallOutcome) -> String {
    outcome
        .result
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// A tool whose schema REQUIRES `n: number`, so a call with the wrong shape fails
/// `validate_tool_call` before the body is reached.
struct StrictTool {
    params: Value,
    calls: Arc<AtomicUsize>,
}

impl StrictTool {
    fn new() -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                params: json!({
                    "type": "object",
                    "properties": { "n": { "type": "number" } },
                    "required": ["n"],
                    "additionalProperties": false,
                }),
                calls: calls.clone(),
            }),
            calls,
        )
    }
}

#[async_trait::async_trait]
impl Tool for StrictTool {
    fn name(&self) -> &str {
        "strict"
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::default())
    }
}

/// A tool that signals failure the way cyrup's contract requires — `Err(ToolError)` — carrying
/// structured failure data (`ToolError::with_details`).
struct FailingTool {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for FailingTool {
    fn name(&self) -> &str {
        "boom"
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
        Err(ToolError::new("the tool blew up").with_details(json!({ "code": 7 })))
    }
}

/// A tool that declares an `output_schema` and returns the machine-readable half with it — the
/// shipped shape upstream's codemode resolution reads (`extensions/codemode/execute.ts:310`:
/// `if (tool.outputSchema && result.structuredContent !== undefined) return result.structuredContent`).
struct SchemaTool {
    params: Value,
    out: Value,
}

impl SchemaTool {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            params: obj_schema(),
            out: json!({ "type": "object", "properties": { "exitCode": { "type": "number" } } }),
        })
    }
}

#[async_trait::async_trait]
impl Tool for SchemaTool {
    fn name(&self) -> &str {
        "typed"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn output_schema(&self) -> Option<&Value> {
        Some(&self.out)
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("exit 0")],
            structured_content: Some(json!({ "exitCode": 0, "output": "hi" })),
            ..Default::default()
        })
    }
}

/// Blocks every call with a fixed reason, and records that it saw one.
struct BlockingHooks(Arc<AtomicUsize>);

#[async_trait::async_trait]
impl Hooks for BlockingHooks {
    async fn before_tool_call(&self, _ctx: BeforeToolCall<'_>, _c: CancelToken) -> BeforeOutcome {
        self.0.fetch_add(1, Ordering::SeqCst);
        BeforeOutcome::Block {
            reason: Some("policy says no".to_string()),
            terminate: cyrup_core::TerminateHint::Unspecified,
        }
    }
}

/// Counts both tool hooks and rewrites the content from `after_tool_call`, so a nested call can
/// prove BOTH ran. Records the tool list each hook was shown.
#[derive(Default)]
struct BothHooks {
    before: Arc<AtomicUsize>,
    after: Arc<AtomicUsize>,
    shown_tools: Arc<std::sync::Mutex<Vec<Vec<String>>>>,
}

#[async_trait::async_trait]
impl Hooks for BothHooks {
    async fn before_tool_call(&self, ctx: BeforeToolCall<'_>, _c: CancelToken) -> BeforeOutcome {
        self.before.fetch_add(1, Ordering::SeqCst);
        self.shown_tools.lock().unwrap().push(
            ctx.context
                .tools
                .iter()
                .map(|t| t.name().to_string())
                .collect(),
        );
        BeforeOutcome::Proceed
    }
    async fn after_tool_call(&self, _ctx: AfterToolCall<'_>, _c: CancelToken) -> AfterOutcome {
        self.after.fetch_add(1, Ordering::SeqCst);
        AfterOutcome::Override(Box::new(AfterOverride {
            content: Some(vec![Content::text("rewritten by after_tool_call")]),
            // Returned ALONGSIDE `content`, which is what keeps the structured half under
            // upstream's drop rule (`types.ts:85-88` @v1.0.1).
            structured_content: Some(json!({ "exitCode": 0, "output": "hi" })),
            ..Default::default()
        }))
    }
}

/// Run `call` against `tools` with `hooks` and nothing else — no agent, no run, no subscriber.
async fn run(call: ToolCall, tools: &[Arc<dyn Tool>], hooks: &dyn Hooks) -> ToolCallOutcome {
    run_tool_call(
        call,
        RunToolCallOptions {
            tools,
            assistant_message: &assistant(),
            context: AgentContextView {
                system_prompt: "sys",
                messages: &[],
                tools,
            },
            hooks,
            cancel: None,
            on_update: None,
            parent_tool_call_id: None,
        },
    )
    .await
}

// ---------------------------------------------------------------------------
// The four failure classes — every one RETURNS, none errors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent047_an_unknown_tool_comes_back_as_is_error() {
    let tools: Vec<Arc<dyn Tool>> = vec![EchoTool::named("echo")];
    let outcome = run(call("nope", json!({})), &tools, &DefaultHooks).await;
    assert!(outcome.is_error, "an unknown tool is a returned failure");
    // AGENT-010 — pi's exact text, no quotes around the name (`agent-loop.ts:719` @v1.0.1).
    assert_eq!(text_of(&outcome), "Tool nope not found");
    assert_eq!(outcome.tool_call.name, "nope", "the call is echoed back");
    assert!(outcome.result.structured_content.is_none());
}

#[tokio::test]
async fn agent047_a_validation_failure_comes_back_as_is_error_and_the_body_never_runs() {
    let (strict, ran) = StrictTool::new();
    let tools: Vec<Arc<dyn Tool>> = vec![strict];
    let outcome = run(
        call("strict", json!({ "n": "not a number" })),
        &tools,
        &DefaultHooks,
    )
    .await;
    assert!(outcome.is_error, "a schema failure is a returned failure");
    assert!(
        !text_of(&outcome).is_empty(),
        "the validation message reaches the caller"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool was not executed");
}

#[tokio::test]
async fn agent047_a_blocked_call_comes_back_as_is_error_and_the_body_never_runs() {
    let (echo, ran) = EchoTool::new("echo");
    let tools: Vec<Arc<dyn Tool>> = vec![echo];
    let seen = Arc::new(AtomicUsize::new(0));
    let hooks = BlockingHooks(seen.clone());
    let outcome = run(call("echo", json!({})), &tools, &hooks).await;
    assert!(outcome.is_error, "a blocked call is a returned failure");
    assert_eq!(
        text_of(&outcome),
        "policy says no",
        "the hook's own reason reaches the caller"
    );
    assert_eq!(
        seen.load(Ordering::SeqCst),
        1,
        "before_tool_call applies to a nested call — that is the point of the seam"
    );
    assert_eq!(ran.load(Ordering::SeqCst), 0, "the tool was not executed");
}

#[tokio::test]
async fn agent047_a_tool_that_errs_comes_back_as_is_error() {
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(FailingTool {
        params: obj_schema(),
    })];
    let outcome = run(call("boom", json!({})), &tools, &DefaultHooks).await;
    assert!(outcome.is_error, "a thrown tool is a returned failure");
    assert_eq!(text_of(&outcome), "the tool blew up");
    // `ToolError::details` survives to the caller, as it already does to the transcript
    // (`finalize.rs`'s `Err` arm) — AGENT-046's CORRECTED note.
    assert_eq!(outcome.result.details, Some(json!({ "code": 7 })));
    // `createErrorToolResult` carries no structured half (AGENT-045).
    assert!(outcome.result.structured_content.is_none());
}

// ---------------------------------------------------------------------------
// The explicit `tools` slice, the hooks, and the structured half
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent047_resolves_against_the_options_tools_not_the_contexts() {
    // pi `prepareToolCall(..., tools)` resolves against the PARAMETER (`agent-loop.ts:713`,
    // `:715` @v1.0.1), and `runToolCall` passes `options.tools` (`:812`). So a slice that
    // excludes the turn's tools decides what runs.
    let (nested, nested_ran) = EchoTool::new("only-nested");
    let (turn, turn_ran) = EchoTool::new("turn-only");
    let resolve: Vec<Arc<dyn Tool>> = vec![nested];
    let context_tools: Vec<Arc<dyn Tool>> = vec![turn];

    let hooks = BothHooks::default();
    let shown = hooks.shown_tools.clone();
    let outcome = run_tool_call(
        call("only-nested", json!({})),
        RunToolCallOptions {
            tools: &resolve,
            assistant_message: &assistant(),
            context: AgentContextView {
                system_prompt: "sys",
                messages: &[],
                // Deliberately NOT the resolution slice.
                tools: &context_tools,
            },
            hooks: &hooks,
            cancel: None,
            on_update: None,
            parent_tool_call_id: None,
        },
    )
    .await;

    assert!(!outcome.is_error, "the slice's tool resolved and ran");
    assert_eq!(nested_ran.load(Ordering::SeqCst), 1);
    assert_eq!(turn_ran.load(Ordering::SeqCst), 0);
    assert_eq!(
        shown.lock().unwrap().as_slice(),
        [vec!["turn-only".to_string()]],
        "the hooks are shown `context.tools`, which is a DIFFERENT list from the one the name \
         resolved against — pi keeps `currentContext` and `tools` separate for exactly this"
    );

    // And the converse: a name present only in the context is not callable.
    let outcome = run_tool_call(
        call("turn-only", json!({})),
        RunToolCallOptions {
            tools: &resolve,
            assistant_message: &assistant(),
            context: AgentContextView {
                system_prompt: "sys",
                messages: &[],
                tools: &context_tools,
            },
            hooks: &DefaultHooks,
            cancel: None,
            on_update: None,
            parent_tool_call_id: None,
        },
    )
    .await;
    assert!(outcome.is_error);
    assert_eq!(text_of(&outcome), "Tool turn-only not found");
    assert_eq!(turn_ran.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn agent047_a_successful_nested_call_runs_both_hooks_and_yields_the_structured_half() {
    let tools: Vec<Arc<dyn Tool>> = vec![SchemaTool::new()];
    let hooks = BothHooks::default();
    let before = hooks.before.clone();
    let after = hooks.after.clone();
    let outcome = run(call("typed", json!({})), &tools, &hooks).await;

    assert!(!outcome.is_error);
    assert_eq!(before.load(Ordering::SeqCst), 1, "before_tool_call ran");
    assert_eq!(after.load(Ordering::SeqCst), 1, "after_tool_call ran");
    assert_eq!(
        text_of(&outcome),
        "rewritten by after_tool_call",
        "the after hook's rewrite is what the caller reads — proof it ran on the nested call"
    );
    // The row's point: a programmatic caller gets a TYPED value back, which is what
    // `CODE-006`/`CODE-008` resolve a tool call to.
    assert_eq!(
        outcome.result.structured_content,
        Some(json!({ "exitCode": 0, "output": "hi" })),
        "`structured_content` reaches the programmatic caller"
    );
}

// ---------------------------------------------------------------------------
// Emits no events, adds no messages
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent047_emits_no_event_and_appends_no_message() {
    let hooks: Arc<dyn Hooks> = Arc::new(BothHooks::default());
    let tools: Vec<Arc<dyn Tool>> = vec![SchemaTool::new()];
    let agent = Agent::builder(model_ref(), faux_stream_fn(Vec::new()).1)
        .system_prompt("sys")
        .tools(tools.clone())
        .hooks(hooks.clone())
        .build();
    let recorder = Arc::new(EventRecorder::default());
    let _sub = agent.subscribe(recorder.clone());
    let before = agent.snapshot().await.messages;

    let outcome = run_tool_call(
        call("typed", json!({})),
        RunToolCallOptions {
            tools: &tools,
            assistant_message: &assistant(),
            context: AgentContextView {
                system_prompt: "sys",
                messages: &[],
                tools: &tools,
            },
            hooks: hooks.as_ref(),
            cancel: None,
            on_update: None,
            parent_tool_call_id: None,
        },
    )
    .await;

    assert!(!outcome.is_error, "the call itself succeeded");
    assert!(
        recorder.snapshot().is_empty(),
        "no AgentEvent at all — pi: \"Emits no events\" (agent-loop.ts:803 @v1.0.1); got {:?}",
        recorder.names()
    );
    assert_eq!(
        agent.snapshot().await.messages.len(),
        before.len(),
        "no message appended to the transcript — pi: \"adds no messages\""
    );
}

// ---------------------------------------------------------------------------
// Streamed updates reach the caller's sink, and stop when the body settles
// ---------------------------------------------------------------------------

/// A tool that streams one update, then returns.
struct StreamingTool {
    params: Value,
}

#[async_trait::async_trait]
impl Tool for StreamingTool {
    fn name(&self) -> &str {
        "streamer"
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
        on_update(cyrup_core::ToolUpdate {
            content: vec![Content::text("half way")],
            ..Default::default()
        });
        Ok(ToolResult {
            content: vec![Content::text("done")],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn agent047_the_callers_on_update_sink_receives_the_tools_partials() {
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(StreamingTool {
        params: obj_schema(),
    })];
    let seen: Arc<std::sync::Mutex<Vec<String>>> = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_seen = seen.clone();
    let outcome = run_tool_call(
        call("streamer", json!({})),
        RunToolCallOptions {
            tools: &tools,
            assistant_message: &assistant(),
            context: AgentContextView {
                system_prompt: "sys",
                messages: &[],
                tools: &tools,
            },
            hooks: &DefaultHooks,
            cancel: None,
            // pi `onUpdate?: ToolUpdateSink` (`agent-loop.ts:798` @v1.0.1) — a plain callback, NOT
            // an event sink. That narrowing is what makes "emits no events" possible.
            on_update: Some(Box::new(move |u| {
                for c in &u.content {
                    if let Content::Text { text, .. } = c {
                        sink_seen.lock().unwrap().push(text.to_string());
                    }
                }
            })),
            parent_tool_call_id: None,
        },
    )
    .await;

    assert!(!outcome.is_error);
    assert_eq!(text_of(&outcome), "done");
    assert_eq!(
        seen.lock().unwrap().as_slice(),
        ["half way".to_string()],
        "the partial reached the caller's own sink"
    );
}
