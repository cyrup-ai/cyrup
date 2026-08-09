//! The high-level [`Agent`] + the turn-based loop (arch-02 §3.5 / §6, func-02 §5/§6).
//!
//! One run = one tokio task that owns the `RunCancel` root. All event emission and hook invocation
//! happen on that single task, so ordering is deterministic; only tool `execute` bodies run
//! concurrently (on a `JoinSet`). The state lock is taken only for the synchronous reducer and is
//! never held across a subscriber `await` (deadlock-freedom, arch-02 §5.5).
//
// KNOWN GAPS (tracked):
// - R-02-020: DONE — JSON-Schema argument validation + coercion runs in preflight via
//   `cyrup_provider::validate_tool_call` (func-01 R-01-034): raw args are first normalized by the
//   tool's `prepare_arguments` compat shim (Pi `prepareToolCallArguments`, agent-loop.ts:548-560),
//   then validated/coerced before `before_tool_call`, and a validation failure yields an immediate
//   isError tool-result (the model retries) without executing the tool. Args mutated by
//   `before_tool_call` still run as-is, without re-validation (R-02-022).
// - A-02-10 (second half): no mutable-aliasing state getter is exposed (snapshots are copies and
//   setters copy-on-assign). Intentional Rust `[CYRUP-DELTA]` from the TS source.
// - thinkingBudgets (Pi `AgentOptions.thinkingBudgets`, agent.ts:112): DONE — forwarded via
//   `GenerationConfig.thinking_budgets` into `cyrup_provider::StreamOptions.thinking_budgets`
//   (anthropic-messages.ts:792-797 lowers it per-level). The unified `reasoning` level is forwarded
//   alongside it.
// - Proxy `StreamFn` (Pi `streamProxy`, proxy.ts): PORTED in `proxy.rs` — the wire enum
//   (`ProxyAssistantMessageEvent`), client-side partial rebuild (`ProxyMessageBuilder`, Pi
//   `processProxyEvent`), options/body (`ProxyStreamOptions`/`buildProxyRequestOptions`), and the
//   `POST {proxyUrl}/api/stream` bearer-SSE transport (`stream_proxy`/`ProxyStreamFn`). Transport
//   reuses cyrup-provider's existing SSE client (`open_sse`) — no new dependency.

use crate::error::AgentError;
use crate::event::{AgentEvent, AgentMessage, ToolResultMessage};
use crate::hooks::{
    AfterToolCall, AgentContextView, BeforeOutcome, BeforeToolCall, DefaultHooks, Hooks, PostTurn,
};
use crate::queue::{PendingQueue, QueueMode, ToolExecution};
use crate::state::{reduce, AgentStateSnapshot, GenerationConfig, StateInner};
use crate::stream_fn::{ApiKeyResolver, StreamFn};
use crate::subscriber::EventSubscriber;
use cyrup_core::{
    ApiId, AssistantMessage, CancelToken, Content, ExecMode, ModelRef, ProviderId, RunCancel,
    SessionId, StopReason, ModelThinkingLevel, Tool, ToolCall, ToolCallId, ToolError, ToolResult,
    ToolUpdate, ToolUpdateSink, Usage, UNRESOLVED_API,
};
use cyrup_provider::{validate_tool_call, Context, StreamEvent, StreamOptions};
use futures::future::FutureExt;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinSet;

/// Lock a `std::sync::Mutex` ignoring poisoning (no panic on a poisoned lock; arch-00 no-panic).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wall-clock milliseconds since the Unix epoch — the Rust analogue of Pi `Date.now()`
/// (agent.ts:383,504; agent-loop.ts:741). Used to stamp prompt user messages, tool-result messages,
/// and the synthetic failure message so the value reaches the `convert_to_llm` wire payload exactly
/// as Pi's does. Never panics: a clock before the epoch degrades to `0`.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// An errored assistant transcript message matching Pi `handleRunFailure` (agent.ts:497-506): one
/// EMPTY text block (`[{type:"text", text:""}]`, NOT empty content) plus a `Date.now()` timestamp.
/// Both reach the wire payload via `convert_to_llm`, so they must mirror Pi byte-for-byte.
/// `cyrup_core::AssistantMessage::errored` yields `content: []`/`timestamp: 0`; this overlays Pi's
/// single empty text block and wall-clock stamp on top (the `errored` type lives in cyrup-core and is
/// shared, so the Pi-specific transcript shape is applied here at the agent boundary).
fn errored_assistant(
    provider: ProviderId,
    model: &str,
    api: Option<ApiId>,
    stop_reason: StopReason,
    msg: impl Into<String>,
) -> AssistantMessage {
    let mut m = AssistantMessage::errored(provider, model, api, stop_reason, msg);
    m.content = vec![Content::text("")];
    m.timestamp = now_millis();
    m
}

fn empty_assistant(model: &ModelRef) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.model.to_string(),
        // Pi: AssistantMessage.api is required (types.ts:386). A ModelRef built from a not-yet-
        // resolved user selection may carry no api; fall back to the sentinel so the field is set.
        api: model.api.clone().unwrap_or_else(|| ApiId::from(UNRESOLVED_API)),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        // This message is only ever a PARTIAL — it seeds `partial` before the first `start` event
        // and is replaced wholesale by `event.partial()` thereafter (agent-loop.ts:313-314). Pi's
        // corresponding seed is `stopReason: "pending"` in each stream function's `output`; a
        // `Stop` seed made a `message_start` emitted on a pre-first-event abort claim a completed
        // turn. It never reaches `message_end`: every return path stamps a settled reason.
        stop_reason: StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    }
}

fn tool_calls(a: &AssistantMessage) -> Vec<ToolCall> {
    a.content
        .iter()
        .filter_map(|c| if let Content::ToolCall(tc) = c { Some(tc.clone()) } else { None })
        .collect()
}

/// The `tool_execution_end.result` payload — Pi emits the full `AgentToolResult`
/// (`{content, details, usage?, addedToolNames?, terminate}`) including the early-termination hint
/// (agent-loop.ts:723-731). `usage`/`addedToolNames` are optional upstream, so — like Pi's
/// `JSON.stringify`, which drops `undefined` — an absent/empty value produces NO key at all rather
/// than a `null`.
fn result_value_of(
    content: &[Content],
    details: &Option<Value>,
    usage: Option<&Usage>,
    added_tool_names: &[String],
    terminate: bool,
) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("content".to_string(), serde_json::to_value(content).unwrap_or(Value::Null));
    obj.insert("details".to_string(), details.clone().unwrap_or(Value::Null));
    if let Some(u) = usage {
        obj.insert("usage".to_string(), serde_json::to_value(u).unwrap_or(Value::Null));
    }
    if !added_tool_names.is_empty() {
        obj.insert(
            "addedToolNames".to_string(),
            serde_json::to_value(added_tool_names).unwrap_or(Value::Null),
        );
    }
    obj.insert("terminate".to_string(), Value::Bool(terminate));
    Value::Object(obj)
}

/// The `tool_execution_update.partialResult` payload — Pi emits the tool's `AgentToolResult`
/// (`{content, details, terminate?}`), where `terminate` is OMITTED when the tool left it
/// `undefined` (agent-loop.ts:641-653; types.ts:350-360). Mirror that: include `terminate` only
/// when `Some`, so an absent hint produces no key (rather than a `null`).
fn update_value(u: &ToolUpdate) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("content".to_string(), serde_json::to_value(&u.content).unwrap_or(Value::Null));
    obj.insert("details".to_string(), u.details.clone().unwrap_or(Value::Null));
    if let Some(t) = u.terminate {
        obj.insert("terminate".to_string(), Value::Bool(t));
    }
    Value::Object(obj)
}

/// Emit one event without a [`RunCtx`] — the same reduce-then-await-subscribers path as
/// [`RunCtx::emit`], used by the catch-all failure path (Pi `handleRunFailure`, agent.ts:496-511)
/// after the run task has unwound and `RunCtx` is gone. Subscriber panics are contained.
async fn emit_standalone(
    subscribers: &Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    state: &Arc<Mutex<StateInner>>,
    ev: AgentEvent,
) {
    {
        let mut st = lock(state);
        reduce(&mut st, &ev);
    }
    let subs = { lock(subscribers).clone() };
    for s in subs.iter() {
        let _ = std::panic::AssertUnwindSafe(s.on_event(&ev)).catch_unwind().await;
    }
}

/// Recover a human-readable message from a caught panic payload (Pi
/// `error instanceof Error ? error.message : String(error)`, agent.ts:505). A `panic!`/`unwrap`
/// payload is typically a `&str` or `String`, which we downcast to recover the real text; any other
/// payload type falls back to a generic label.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "run task failed".to_string()
    }
}

// ---------------------------------------------------------------------------
// Public entry-point helpers
// ---------------------------------------------------------------------------

/// Input to [`Agent::prompt`]. Convertible from `&str`/`String`/`AgentMessage`/`Vec<AgentMessage>`.
pub struct PromptInput {
    pub messages: Vec<AgentMessage>,
}

impl PromptInput {
    /// A single user message carrying `text` followed by image attachments (Pi
    /// `normalizePromptInput`, agent.ts:379-383): `[{type:"text"}, ...images]`.
    pub fn text_with_images(text: impl Into<String>, images: Vec<Content>) -> Self {
        let mut content = vec![Content::text(text)];
        content.extend(images);
        // Pi `normalizePromptInput` stamps the string-input user message with `Date.now()`
        // (agent.ts:393); this value reaches the wire payload via `convert_to_llm`.
        Self { messages: vec![AgentMessage::User { content, timestamp: Some(now_millis()) }] }
    }

    /// The single message this input wraps (panics-free: returns an empty user message if empty).
    fn into_one(mut self) -> AgentMessage {
        if self.messages.is_empty() {
            AgentMessage::user_text("")
        } else {
            self.messages.remove(0)
        }
    }
}

impl From<&str> for PromptInput {
    fn from(s: &str) -> Self {
        // Pi `normalizePromptInput` stamps a string prompt with `Date.now()` (agent.ts:389-393).
        Self { messages: vec![AgentMessage::User { content: vec![Content::text(s)], timestamp: Some(now_millis()) }] }
    }
}
impl From<String> for PromptInput {
    fn from(s: String) -> Self {
        // Pi `normalizePromptInput` stamps a string prompt with `Date.now()` (agent.ts:389-393).
        Self { messages: vec![AgentMessage::User { content: vec![Content::text(s)], timestamp: Some(now_millis()) }] }
    }
}
impl From<AgentMessage> for PromptInput {
    fn from(m: AgentMessage) -> Self {
        Self { messages: vec![m] }
    }
}
impl From<Vec<AgentMessage>> for PromptInput {
    fn from(messages: Vec<AgentMessage>) -> Self {
        Self { messages }
    }
}

/// Resolves to the NEW messages created during the run (func-02 R-02-002).
pub struct RunHandle {
    new_messages: oneshot::Receiver<Vec<AgentMessage>>,
}

impl RunHandle {
    /// Await the run; yields the new messages (empty if the task was dropped).
    pub async fn finished(self) -> Vec<AgentMessage> {
        self.new_messages.await.unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Internal run-loop types
// ---------------------------------------------------------------------------

enum ToolRuntimeMsg {
    Update { call_id: ToolCallId, partial: ToolUpdate },
    Finished { call_id: ToolCallId, source_index: usize, tool_name: String, outcome: Result<ToolResult, ToolError> },
}

struct Finalized {
    source_index: usize,
    tool_call_id: ToolCallId,
    tool_name: String,
    result_value: Value,
    is_error: bool,
    terminate: bool,
    message: ToolResultMessage,
}

enum Prep {
    /// Boxed: `Finalized` embeds a whole `ToolResultMessage` and dwarfs the `Ready` arm, so an
    /// unboxed variant makes every `Prep` (including the common prepared-call case) pay for it.
    Immediate(Box<Finalized>),
    Ready { tool: Arc<dyn Tool>, args: Value },
}

struct Batch {
    messages: Vec<ToolResultMessage>,
    terminate: bool,
}

pub(crate) enum EntryStart {
    Prompt(Vec<AgentMessage>),
    Continue,
}

// ---------------------------------------------------------------------------
// The run context (owns one run's working state; lives on the run task)
// ---------------------------------------------------------------------------

pub(crate) struct RunCtx {
    state: Arc<Mutex<StateInner>>,
    subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    steering: Arc<Mutex<PendingQueue>>,
    follow_up: Arc<Mutex<PendingQueue>>,
    hooks: Arc<dyn Hooks>,
    stream_fn: Arc<dyn StreamFn>,
    key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
    system_prompt: String,
    /// Running model baseline; a `prepare_next_turn` model override updates it stickily
    /// (Pi `config.model`, agent.ts:425 / agent-loop.ts:228-238).
    model: ModelRef,
    /// Running thinking level; a `prepare_next_turn` `thinking_level` override updates it stickily
    /// (Pi `config.reasoning`, agent.ts:426 / agent-loop.ts:228-238).
    thinking_level: ModelThinkingLevel,
    /// Generation params + telemetry forwarded into `StreamOptions` (Pi `AgentLoopConfig`).
    gen_config: GenerationConfig,
    tools: Vec<Arc<dyn Tool>>,
    cancel: RunCancel,
    new_messages: Vec<AgentMessage>,
    /// The loop's OWN working transcript — Pi `currentContext.messages`, a `.slice()` SNAPSHOT of the
    /// agent's `messages` taken at run start, NOT the live `Arc` (agent.ts:424-429; agent-loop.ts:104-107).
    /// This is the array the loop reads to build each LLM payload and that a `prepare_next_turn`
    /// context override replaces. The agent's observable `state.messages` grows INDEPENDENTLY via the
    /// reducer on `message_end` (agent.ts:519-522), so neither a context override nor a mid-run
    /// external `set_messages` leaks between the two — exactly as in Pi.
    messages: Vec<AgentMessage>,
    turn_index: usize,
    /// On continue-from-assistant, the first `getSteeringMessages` poll returns `[]` so a second
    /// queued steering message is not drained a turn too early (Pi `skipInitialSteeringPoll`,
    /// agent.ts:351,440-446).
    skip_initial_steering_poll: bool,
}

impl RunCtx {
    /// Assemble a run context from already-built shared handles. Used by [`Agent::start_run`] and by
    /// the low-level free-function loop (`crate::loop_fn`) so both drive the identical, tested loop.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        state: Arc<Mutex<StateInner>>,
        subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
        steering: Arc<Mutex<PendingQueue>>,
        follow_up: Arc<Mutex<PendingQueue>>,
        hooks: Arc<dyn Hooks>,
        stream_fn: Arc<dyn StreamFn>,
        key_resolver: Option<Arc<dyn ApiKeyResolver>>,
        tool_execution: ToolExecution,
        session_id: Option<SessionId>,
        system_prompt: String,
        model: ModelRef,
        thinking_level: ModelThinkingLevel,
        gen_config: GenerationConfig,
        tools: Vec<Arc<dyn Tool>>,
        messages: Vec<AgentMessage>,
        cancel: RunCancel,
        skip_initial_steering_poll: bool,
    ) -> Self {
        Self {
            state,
            subscribers,
            steering,
            follow_up,
            hooks,
            stream_fn,
            key_resolver,
            tool_execution,
            session_id,
            system_prompt,
            model,
            thinking_level,
            gen_config,
            tools,
            cancel,
            new_messages: Vec::new(),
            messages,
            turn_index: 0,
            skip_initial_steering_poll,
        }
    }

    /// The sole emission path (arch-02 §5.1): reduce managed state (lock released BEFORE awaiting),
    /// then await each subscriber in registration order before returning.
    async fn emit(&self, ev: AgentEvent) {
        {
            let mut st = lock(&self.state);
            reduce(&mut st, &ev);
        }
        let subs = { lock(&self.subscribers).clone() };
        for s in subs.iter() {
            // Contain a panicking (or otherwise failing) subscriber (func-02 R-02-048): the panic is
            // caught here so the run loop continues normally and still emits its full closing
            // sequence. A caught panic is swallowed — a subscriber failure MUST NOT halt the loop.
            // `AssertUnwindSafe` is sound: emission is the sole writer of managed state and the lock
            // is released before this await, so no broken invariant can leak across the boundary
            // (keeps the crate `#![forbid(unsafe_code)]`).
            let _ = std::panic::AssertUnwindSafe(s.on_event(&ev)).catch_unwind().await;
        }
    }

    /// Pi `handleRunFailure` (agent.ts:496-511) reached from INSIDE the loop: the post-turn hooks
    /// (`prepareNextTurn`, agent-loop.ts:231; `shouldStopAfterTurn`, agent-loop.ts:246-252) are
    /// awaited with no try/catch, so a throw unwinds out of `runLoop` into `runWithLifecycle`'s
    /// catch (agent.ts:489-490) and is reported as a run FAILURE: one synthetic errored assistant
    /// message (empty text block, wall-clock timestamp, `stopReason` aborted-vs-error, the thrown
    /// `error.message`) followed by `message_start` → `message_end` → `turn_end` (with NO tool
    /// results) → `agent_end` carrying `[failureMessage]` and nothing else (agent.ts:508-511).
    ///
    /// The post-unwind twin of this path lives at [`Agent::run`]'s `catch_unwind` arm, which must
    /// synthesize the same quartet through [`emit_standalone`] because its `RunCtx` is already gone;
    /// here the live `RunCtx` is intact, so emission goes through the ordinary [`RunCtx::emit`] and
    /// the reducer records `error_message`/`stop_reason` exactly as it does for a streamed message.
    ///
    /// `new_messages` is REPLACED by the failure message so the run's returned value matches
    /// `agent_end.messages` — Pi's failed run resolves its promise without the loop-local
    /// `newMessages` accumulator (the throw at agent.ts:488 never reaches `runLoop`'s return), and the
    /// `catch_unwind` twin settles the same single-element vector.
    async fn emit_run_failure(&mut self, error_message: String) {
        // Pi reads `this._state.model` (agent.ts:500-502) — the agent's state model, not the loop's
        // possibly-overridden running baseline.
        let model = { lock(&self.state).model.clone() };
        // Pi `stopReason: aborted ? "aborted" : "error"` (agent.ts:504).
        let stop_reason =
            if self.cancel.is_cancelled() { StopReason::Aborted } else { StopReason::Error };
        let failure = errored_assistant(
            model.provider.clone(),
            model.model.as_str(),
            model.api.clone(),
            stop_reason,
            error_message,
        );
        let fm = AgentMessage::Assistant(failure);
        self.emit(AgentEvent::MessageStart { message: fm.clone() }).await;
        self.emit(AgentEvent::MessageEnd { message: fm.clone() }).await;
        self.emit(AgentEvent::TurnEnd { message: fm.clone(), tool_results: Vec::new() }).await;
        self.emit(AgentEvent::AgentEnd { messages: vec![fm.clone()] }).await;
        self.new_messages = vec![fm];
    }

    fn poll_steering(&self) -> Vec<AgentMessage> {
        lock(&self.steering).drain()
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        lock(&self.follow_up).drain()
    }

    fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    pub(crate) async fn run(&mut self, entry: EntryStart) -> Vec<AgentMessage> {
        self.emit(AgentEvent::AgentStart).await;
        match entry {
            EntryStart::Prompt(prompts) => {
                self.emit(AgentEvent::TurnStart).await;
                for p in prompts {
                    self.emit(AgentEvent::MessageStart { message: p.clone() }).await;
                    self.emit(AgentEvent::MessageEnd { message: p.clone() }).await;
                    // Pi appends each prompt to the loop's working copy (`currentContext.messages`,
                    // agent-loop.ts:106/187) — the observable `state.messages` grows separately via
                    // the reducer on the `message_end` above.
                    self.messages.push(p.clone());
                    self.new_messages.push(p);
                }
                self.run_loop(true).await;
            }
            EntryStart::Continue => {
                self.emit(AgentEvent::TurnStart).await;
                self.run_loop(true).await;
            }
        }
        self.new_messages.clone()
    }

    async fn run_loop(&mut self, mut turn_started: bool) {
        // Pi polls steering at the very top (agent-loop.ts:167), but a continue-from-assistant run
        // already drained one steering message and passes it as the prompt; `skipInitialSteeringPoll`
        // makes this first poll return `[]` so the next queued steering message is not drained a turn
        // too early under `one-at-a-time` (agent.ts:351,440-446).
        let mut pending = if self.skip_initial_steering_poll {
            self.skip_initial_steering_poll = false;
            Vec::new()
        } else {
            self.poll_steering()
        };
        loop {
            let mut has_more_tools = true;
            while has_more_tools || !pending.is_empty() {
                if turn_started {
                    turn_started = false;
                } else {
                    self.emit(AgentEvent::TurnStart).await;
                }
                for m in std::mem::take(&mut pending) {
                    self.emit(AgentEvent::MessageStart { message: m.clone() }).await;
                    self.emit(AgentEvent::MessageEnd { message: m.clone() }).await;
                    // Pi pushes each injected steering/follow-up message onto the loop's working copy
                    // (`currentContext.messages.push`, agent-loop.ts:186).
                    self.messages.push(m.clone());
                    self.new_messages.push(m);
                }

                let asst = self.stream_assistant().await;
                // Pi's `streamAssistantResponse` leaves the final assistant message in the loop's
                // working copy (`currentContext.messages`, agent-loop.ts:346/348/361/363); mirror that
                // before tool execution / the post-turn hooks read the context.
                self.messages.push(AgentMessage::Assistant(asst.clone()));
                self.new_messages.push(AgentMessage::Assistant(asst.clone()));

                if matches!(asst.stop_reason, StopReason::Error | StopReason::Aborted) {
                    self.emit(AgentEvent::TurnEnd {
                        message: AgentMessage::Assistant(asst),
                        tool_results: Vec::new(),
                    })
                    .await;
                    self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
                    return;
                }

                let calls = tool_calls(&asst);
                let mut tool_results = Vec::new();
                has_more_tools = false;
                if !calls.is_empty() {
                    // A `length` stop means the output was cut off by the token limit, so every
                    // tool call in the message may carry truncated arguments. Fail them all
                    // instead of executing potentially borked calls (Pi agent-loop.ts:207-216).
                    let batch = if matches!(asst.stop_reason, StopReason::Length) {
                        self.fail_truncated_tool_calls(&calls).await
                    } else {
                        self.execute_tool_calls(&asst, &calls).await
                    };
                    tool_results = batch.messages;
                    // `terminate` ends only TOOL-driven continuation (the whole batch must set it,
                    // `shouldTerminateToolBatch`, agent-loop.ts:210,544-546); queued steering /
                    // follow-up still flow through the post-turn path below.
                    has_more_tools = !batch.terminate;
                    for r in &tool_results {
                        // Pi pushes each tool result onto the loop's working copy
                        // (`currentContext.messages.push(result)`, agent-loop.ts:213).
                        self.messages.push(AgentMessage::ToolResult(r.clone()));
                        self.new_messages.push(AgentMessage::ToolResult(r.clone()));
                    }
                }

                self.emit(AgentEvent::TurnEnd {
                    message: AgentMessage::Assistant(asst.clone()),
                    tool_results: tool_results.clone(),
                })
                .await;
                self.turn_index += 1;

                // NOTE: there is NO early return on terminate. Pi still runs the post-turn path —
                // `prepareNextTurn`, `shouldStopAfterTurn`, then the steering poll — and continues
                // if any steering / follow-up is queued (agent-loop.ts:210,218-262). So a terminating
                // turn still fires both post-turn hooks and still drains any queued steering /
                // follow-up; absent a queue the inner loop simply exits (`has_more_tools` is false)
                // and the run ends via the normal `agent_end` below.

                // Post-turn hook context: the completed assistant message, this turn's tool results,
                // the live context (system prompt + tools + full transcript), and the new-message
                // accumulator (Pi `ShouldStopAfterTurnContext`/`PrepareNextTurnContext`,
                // types.ts:116-138).
                let ctx_messages = self.messages.clone();
                let prep = {
                    let ctx = PostTurn {
                        messages: &self.new_messages,
                        turn_index: self.turn_index,
                        message: &asst,
                        tool_results: &tool_results,
                        context: AgentContextView {
                            system_prompt: &self.system_prompt,
                            messages: &ctx_messages,
                            tools: &self.tools,
                        },
                    };
                    self.hooks.prepare_next_turn(ctx).await
                };
                match prep {
                    Ok(Some(u)) => {
                        // Overrides are STICKY: Pi reassigns the running `config`/`currentContext`
                        // so a model / reasoning / context override returned once becomes the new
                        // baseline for EVERY later turn in the run (agent-loop.ts:226-239), not a
                        // one-shot. We fold each provided field into the run baseline here.
                        if let Some(m) = u.model {
                            self.model = m;
                        }
                        if let Some(t) = u.thinking_level {
                            self.thinking_level = t;
                        }
                        if let Some(ctx) = u.context {
                            // `currentContext = snapshot.context ?? currentContext`
                            // (agent-loop.ts:228): the override replaces ONLY the loop's working copy.
                            // The agent's observable `state.messages` keeps growing via the reducer, so
                            // the override never leaks into `agent.state.messages` (Pi keeps the two
                            // arrays distinct, agent.ts:519-522). Subsequent turns append onto the
                            // override here.
                            self.messages = ctx;
                        }
                        // The tool array and system prompt travel inside Pi's `context` on the same
                        // return (`{...previousContext, systemPrompt, tools:
                        // this.agent.state.tools.slice()}`, agent-session.ts:530-534) and are just as
                        // sticky. Folding them here is what lets a tool that becomes active MID-RUN
                        // be called on the very next turn — the precondition an `addedToolNames`
                        // anchor asserts (DRIFT-001) and what EXT-004's late registration needs to
                        // reach the model before the run ends.
                        if let Some(tools) = u.tools {
                            self.tools = tools;
                        }
                        if let Some(prompt) = u.system_prompt {
                            self.system_prompt = prompt;
                        }
                    }
                    Ok(None) => {}
                    // A THROWING `prepareNextTurn` is not caught by `runLoop` (agent-loop.ts:231 has
                    // no try/catch): the rejection escapes into `runWithLifecycle`'s catch
                    // (agent.ts:489-490) and lands in `handleRunFailure` — a synthetic errored
                    // assistant message plus the FULL closing quartet, not a bare `agent_end`.
                    Err(e) => {
                        self.emit_run_failure(e.to_string()).await;
                        return;
                    }
                }

                // Pi passes the UPDATED `currentContext` to `shouldStopAfterTurn` (it runs AFTER the
                // `prepareNextTurn` reassignment, agent-loop.ts:241-251), so re-snapshot the (possibly
                // overridden) transcript for this hook's context view.
                let ctx_messages_after = self.messages.clone();
                let stop = {
                    let ctx = PostTurn {
                        messages: &self.new_messages,
                        turn_index: self.turn_index,
                        message: &asst,
                        tool_results: &tool_results,
                        context: AgentContextView {
                            system_prompt: &self.system_prompt,
                            messages: &ctx_messages_after,
                            tools: &self.tools,
                        },
                    };
                    self.hooks.should_stop_after_turn(ctx).await
                };
                match stop {
                    Ok(true) => {
                        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
                        return;
                    }
                    Ok(false) => {}
                    // Same as `prepareNextTurn` above: `shouldStopAfterTurn` is awaited bare
                    // (agent-loop.ts:246-252), so a throw escapes to `handleRunFailure` rather than
                    // ending the run with the ordinary `agent_end` of the `Ok(true)` arm.
                    Err(e) => {
                        self.emit_run_failure(e.to_string()).await;
                        return;
                    }
                }

                pending = self.poll_steering();
            }

            let follow = self.poll_follow_up();
            if !follow.is_empty() {
                pending = follow;
                continue;
            }
            break;
        }
        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
    }

    /// The LLM boundary (arch-02 §6.2). Always emits the assistant `message_start..message_end`
    /// (including on hook error / abort) so the caller's closing sequence is complete.
    async fn stream_assistant(&self) -> AssistantMessage {
        // The running baseline. `prepare_next_turn` overrides are STICKY: a returned
        // model/reasoning/context override is folded into the run's baseline (`self.model`,
        // `self.thinking_level`, and the live `state.messages`) in `run_loop`, so it persists for
        // ALL later turns in the run (Pi `config = {...config, model, reasoning}` /
        // `currentContext = snapshot.context ?? currentContext`, agent-loop.ts:226-239). A
        // non-reasoning model silently ignores the level (func-01 R-01-041).
        let model = self.model.clone();
        let effective_thinking = self.thinking_level;
        // Read the loop's OWN working copy (Pi `context.messages`, agent-loop.ts:283), NOT the live
        // `state.messages` Arc — a `prepare_next_turn` context override or a mid-run external
        // `set_messages` must not cross between the two.
        let base_messages = self.messages.clone();

        let transformed =
            match self.hooks.transform_context(base_messages, self.cancel.child()).await {
                Ok(m) => m,
                // Pi awaits `transformContext` bare (agent-loop.ts:288-292), so a throw unwinds to
                // `handleRunFailure`, whose `errorMessage` is the thrown value's own text
                // (`error instanceof Error ? error.message : String(error)`, agent.ts:504). Surface
                // `e.to_string()` — never a fixed label — or the hook's reason is lost outright.
                Err(e) => return self.emit_error_assistant(e.to_string(), &model).await,
            };
        let llm = match self.hooks.convert_to_llm(&transformed).await {
            Ok(m) => m,
            // Same bare await for `convertToLlm` (agent-loop.ts:295) → same `handleRunFailure` text.
            Err(e) => return self.emit_error_assistant(e.to_string(), &model).await,
        };

        // Dynamic key wins; fall back to the run's static key (Pi `... || config.apiKey`,
        // agent-loop.ts:301-302).
        let api_key = match &self.key_resolver {
            Some(r) => r.get_api_key(&model.provider).await,
            None => None,
        }
        .or_else(|| self.gen_config.api_key.clone());

        // Forward each tool's `description` to the model (Pi `Context.tools`, agent-loop.ts:289-296;
        // spec §4.3) — an empty description left the model unable to use the tool.
        let tool_defs: Vec<cyrup_provider::ToolDef> = self
            .tools
            .iter()
            .map(|t| cyrup_provider::ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                parameters: t.parameters().clone(),
            })
            .collect();

        // Forward the generation params + telemetry + reasoning level (Pi `AgentLoopConfig` →
        // `streamSimple`, agent-loop.ts:298-308 / agent.ts:421-447).
        let opts = StreamOptions {
            cancel: Some(self.cancel.child()),
            api_key,
            session_id: self.session_id.clone(),
            reasoning: effective_thinking,
            temperature: self.gen_config.temperature,
            max_tokens: self.gen_config.max_tokens,
            cache_retention: self.gen_config.cache_retention,
            // LIVE, not `gen_config`: pi rebuilds these inside `streamFn` for the model the request
            // is actually going to (`sdk.ts:318-327`), so a cross-provider `/model` switch must not
            // keep sending the previous provider's attribution headers. Read per TURN off the
            // shared state the facade writes through `Agent::set_headers`.
            headers: lock(&self.state).headers.clone(),
            transport: self.gen_config.transport,
            max_retry_delay_ms: self.gen_config.max_retry_delay_ms,
            max_retries: self.gen_config.max_retries,
            thinking_budgets: self.gen_config.thinking_budgets,
            on_payload: self.gen_config.on_payload.clone(),
            on_response: self.gen_config.on_response.clone(),
            // Provider-scoped env overlay (e.g. the `httpProxy` setting) + request idle timeout (Pi
            // `applyHttpProxySettings`/`configureHttpDispatcher`, main.ts:744-745).
            env: self.gen_config.env.clone(),
            timeout_ms: self.gen_config.timeout_ms,
            ..Default::default()
        };
        let ctx = Context {
            system_prompt: Some(self.system_prompt.clone()),
            messages: llm,
            tools: tool_defs,
        };

        let mut stream = self.stream_fn.stream(&model, &ctx, &opts);
        let cancel_tok = self.cancel.token();
        let mut started = false;
        // The structured partial assistant message, kept in lockstep with the provider's per-event
        // `partial` snapshot (Pi `event.partial`, agent-loop.ts:313-340): distinct text / thinking /
        // toolCall content blocks (with signatures) and streaming tool-call args — NOT a single
        // collapsed text block. The provider exposes this via `StreamEvent::partial()` (stream.rs).
        let mut partial = empty_assistant(&model);
        let mut final_msg: Option<AssistantMessage> = None;

        'consume: loop {
            tokio::select! {
                biased;
                _ = cancel_tok.cancelled() => {
                    if !started {
                        self.emit(AgentEvent::MessageStart {
                            message: AgentMessage::Assistant(partial.clone()),
                        })
                        .await;
                    }
                    // Pi returns the stream's own `result()` terminal on abort (agent-loop.ts:344),
                    // which carries the ACCUMULATED partial content with `stopReason:"aborted"` — NOT
                    // a fresh empty message. Reuse the structured partial we have been tracking and
                    // only stamp the terminal reason, so a subscriber/transcript sees the streamed
                    // text/thinking/tool-call blocks rather than `[]`. The terminal's `errorMessage`
                    // is Pi's uniform abort string `"Request was aborted"` — every provider throws
                    // `new Error("Request was aborted")` on `signal.aborted` and the catch sets
                    // `output.errorMessage = error.message` (anthropic-messages.ts:718,733-734; the
                    // faux provider's `createAbortedMessage` uses the same string, faux.ts:291-297) —
                    // NOT the bare `"aborted"`.
                    let mut aborted = partial.clone();
                    aborted.stop_reason = StopReason::Aborted;
                    aborted.error_message = Some("Request was aborted".to_string());
                    self.emit(AgentEvent::MessageEnd {
                        message: AgentMessage::Assistant(aborted.clone()),
                    })
                    .await;
                    return aborted;
                }
                ev = stream.next() => {
                    let e = match ev {
                        None => break,
                        Some(e) => e,
                    };
                    // Refresh the structured partial from the event's own snapshot for every
                    // non-terminal event (Pi assigns `partialMessage = event.partial`).
                    if let Some(p) = e.partial() {
                        partial = p.clone();
                    }
                    match &e {
                        StreamEvent::Start { .. } => {
                            started = true;
                            self.emit(AgentEvent::MessageStart {
                                message: AgentMessage::Assistant(partial.clone()),
                            })
                            .await;
                        }
                        // Pi RETURNS from `streamAssistantResponse` immediately on the `done`/`error`
                        // terminal (agent-loop.ts:342-355): it stops consuming the stream right here.
                        // Break out of the consume loop so a (non-conforming) post-terminal event can
                        // neither emit a stray `message_update` nor overwrite the final `partial`.
                        StreamEvent::Done { message, .. } => {
                            final_msg = Some(message.clone());
                            break 'consume;
                        }
                        StreamEvent::Error { error, .. } => {
                            final_msg = Some(error.clone());
                            break 'consume;
                        }
                        // Every other event is a content-block start/delta/end (text, thinking, OR
                        // tool-call): re-emit the refreshed partial on `message_update` (Pi emits
                        // `message_update` for all nine block events once the partial exists,
                        // agent-loop.ts:319-340).
                        _ => {
                            if started {
                                self.emit(AgentEvent::MessageUpdate {
                                    message: AgentMessage::Assistant(partial.clone()),
                                    assistant_message_event: Box::new(e.clone()),
                                })
                                .await;
                            }
                        }
                    }
                }
            }
        }

        let final_msg = final_msg.unwrap_or_else(|| {
            errored_assistant(
                model.provider.clone(),
                model.model.as_str(),
                model.api.clone(),
                StopReason::Error,
                "stream ended without a terminal event",
            )
        });
        if !started {
            self.emit(AgentEvent::MessageStart {
                message: AgentMessage::Assistant(final_msg.clone()),
            })
            .await;
        }
        self.emit(AgentEvent::MessageEnd { message: AgentMessage::Assistant(final_msg.clone()) })
            .await;
        final_msg
    }

    async fn emit_error_assistant(
        &self,
        msg: impl Into<String>,
        model: &ModelRef,
    ) -> AssistantMessage {
        // Pi routes a `transformContext`/`convertToLlm` throw through `handleRunFailure`, whose
        // failure message carries one empty text block + `Date.now()` (agent.ts:497-506).
        let asst = errored_assistant(
            model.provider.clone(),
            model.model.as_str(),
            model.api.clone(),
            StopReason::Error,
            msg,
        );
        self.emit(AgentEvent::MessageStart { message: AgentMessage::Assistant(asst.clone()) })
            .await;
        self.emit(AgentEvent::MessageEnd { message: AgentMessage::Assistant(asst.clone()) }).await;
        asst
    }

    async fn execute_tool_calls(&self, assistant: &AssistantMessage, calls: &[ToolCall]) -> Batch {
        let any_seq = calls.iter().any(|c| {
            self.find_tool(&c.name).map(|t| t.execution_mode() == ExecMode::Sequential).unwrap_or(false)
        });
        let sequential = any_seq || matches!(self.tool_execution, ToolExecution::Sequential);
        // Snapshot the loop's working transcript once for the per-call hook context view (Pi
        // `currentContext.messages`, agent-loop.ts:691).
        let ctx_messages = self.messages.clone();
        if sequential {
            self.execute_sequential(assistant, &ctx_messages, calls).await
        } else {
            self.execute_parallel(assistant, &ctx_messages, calls).await
        }
    }

    /// Fail every tool call from an assistant message that was truncated by the output token limit
    /// (Pi `failToolCallsFromTruncatedMessage`, agent-loop.ts:374-405).
    ///
    /// Streamed tool-call arguments are finalized with a best-effort JSON salvage parser
    /// (`cyrup-provider` `parse_streaming_json_object`), so a truncated message can yield tool calls
    /// whose arguments parse and validate but are silently incomplete. None of them are safe to
    /// execute; report each as an error so the model can re-issue them. No tool is located, no
    /// `before_tool_call`/`after_tool_call` hook runs, and the batch never terminates the loop —
    /// Pi returns `{ messages, terminate: false }` so the model gets its turn to re-issue.
    ///
    /// Per call, in source order, the emitted sequence mirrors Pi exactly:
    /// `tool_execution_start` → `tool_execution_end` (`isError`) → `message_start` / `message_end`.
    async fn fail_truncated_tool_calls(&self, calls: &[ToolCall]) -> Batch {
        let mut tool_results = Vec::new();
        for call in calls {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object(call.arguments.clone()),
            })
            .await;
            let fin = self.immediate_error(
                call,
                format!(
                    "Tool call \"{}\" was not executed: the response hit the output token limit, \
                     so its arguments may be truncated. Re-issue the tool call with complete \
                     arguments.",
                    call.name
                ),
            );
            self.emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: fin.tool_call_id.clone(),
                tool_name: fin.tool_name.clone(),
                result: fin.result_value.clone(),
                is_error: fin.is_error,
            })
            .await;
            let msg = AgentMessage::ToolResult(fin.message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await;
            self.emit(AgentEvent::MessageEnd { message: msg }).await;
            tool_results.push(fin.message);
        }
        // Pi `{ messages, terminate: false }` (agent-loop.ts:404).
        Batch { messages: tool_results, terminate: false }
    }

    /// Preflight: locate tool → normalize args (`prepare_arguments`) → validate/coerce → `before_tool_call`.
    /// Returns an immediate (finalized) error result or a prepared executor (func-02 R-02-019/020/021/022).
    async fn prepare(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[AgentMessage],
        call: &ToolCall,
    ) -> Prep {
        let tool = match self.find_tool(&call.name) {
            Some(t) => t,
            None => {
                return Prep::Immediate(Box::new(
                    self.immediate_error(call, format!("Tool '{}' not found", call.name)),
                ))
            }
        };
        // Normalize the raw model-emitted arguments via the tool's `prepare_arguments` compat shim
        // BEFORE schema validation (Pi `prepareToolCallArguments` → `validateToolArguments`,
        // agent-loop.ts:548-560,578-579). Default impl is identity.
        let prepared = tool.prepare_arguments(Value::Object(call.arguments.clone())).await;
        // Validate AND coerce against the tool's JSON-Schema `parameters` (R-02-020 / func-01
        // R-01-034). On failure surface an immediate isError tool-result so the model can retry on
        // the next turn; the tool is NOT executed.
        let mut args = match validate_tool_call(tool.parameters(), prepared) {
            Ok(coerced) => coerced,
            Err(e) => return Prep::Immediate(Box::new(self.immediate_error(call, e.to_string()))),
        };
        if self.cancel.is_cancelled() {
            return Prep::Immediate(Box::new(self.immediate_error(call, "Operation aborted")));
        }
        let before = {
            let ctx = BeforeToolCall {
                tool_name: &call.name,
                tool_call_id: &call.id,
                args: &mut args,
                messages: &self.new_messages,
                assistant_message: assistant,
                tool_call: call,
                context: AgentContextView {
                    system_prompt: &self.system_prompt,
                    messages: ctx_messages,
                    tools: &self.tools,
                },
            };
            self.hooks.before_tool_call(ctx, self.cancel.child()).await
        };
        match before {
            // Pi's `prepareToolCall` wraps the `beforeToolCall` await in the same try that guards
            // argument preparation/validation, and its catch returns
            // `createErrorToolResult(error instanceof Error ? error.message : String(error))`
            // (agent-loop.ts:657-662) — the hook's OWN text reaches the model, exactly as the
            // validation failure two arms up already does.
            Err(e) => Prep::Immediate(Box::new(self.immediate_error(call, e.to_string()))),
            Ok(BeforeOutcome::Block { reason }) => Prep::Immediate(Box::new(self.immediate_error(
                call,
                reason.unwrap_or_else(|| "Tool call blocked by beforeToolCall".to_string()),
            ))),
            // Args mutated in place are executed as-is, WITHOUT re-validation (R-02-022).
            Ok(BeforeOutcome::Proceed) => {
                if self.cancel.is_cancelled() {
                    Prep::Immediate(Box::new(self.immediate_error(call, "Operation aborted")))
                } else {
                    Prep::Ready { tool, args }
                }
            }
        }
    }

    fn immediate_error(&self, call: &ToolCall, msg: impl Into<String>) -> Finalized {
        let message = ToolResultMessage {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            content: vec![Content::text(msg)],
            details: None,
            // Pi's `createErrorToolResult` builds `{content, details:{}}` and nothing else
            // (agent-loop.ts:754-759): a call that did not run reports no usage and cannot have
            // introduced a tool, so an error result never anchors deferred tool loading.
            usage: None,
            added_tool_names: Vec::new(),
            is_error: true,
            // Pi `createToolResultMessage` stamps every tool result with `Date.now()`
            // (agent-loop.ts:741); this reaches the wire payload via `convert_to_llm`.
            timestamp: now_millis(),
        };
        Finalized {
            source_index: 0,
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            result_value: result_value_of(&message.content, &message.details, None, &[], false),
            is_error: true,
            terminate: false,
            message,
        }
    }

    /// Apply `after_tool_call` (replace-not-merge per field, R-02-025) and build the finalized
    /// result. On hook `Err`: error result, `terminate` ignored (R-02-025/050).
    #[allow(clippy::too_many_arguments)]
    async fn finalize(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[AgentMessage],
        call: &ToolCall,
        source_index: usize,
        args: Value,
        outcome: Result<ToolResult, ToolError>,
    ) -> Finalized {
        let call_id = call.id.clone();
        let tool_name = call.name.clone();
        // `added_tool_names` rides through untouched: Pi's `finalizeExecutedToolCall` spreads
        // `{...result}` before applying the hook's explicit fields (agent-loop.ts:736-742) and
        // `addedToolNames` is not one of them, so no hook can set or clear it.
        let (
            mut content,
            mut details,
            mut usage,
            mut added_tool_names,
            mut terminate,
            mut is_error,
        ) = match outcome {
            Ok(r) => (r.content, r.details, r.usage, r.added_tool_names, r.terminate, false),
            Err(e) => (vec![Content::text(e.to_string())], None, None, Vec::new(), false, true),
        };

        let hook_result = {
            let ctx = AfterToolCall {
                tool_name: &tool_name,
                tool_call_id: &call_id,
                args: &args,
                content: &content,
                details: details.as_ref(),
                usage: usage.as_ref(),
                is_error,
                terminate,
                assistant_message: assistant,
                tool_call: call,
                context: AgentContextView {
                    system_prompt: &self.system_prompt,
                    messages: ctx_messages,
                    tools: &self.tools,
                },
            };
            self.hooks.after_tool_call(ctx, self.cancel.child()).await
        };
        match hook_result {
            Ok(Some(ov)) => {
                if let Some(c) = ov.content {
                    content = c;
                }
                if let Some(d) = ov.details {
                    details = Some(d);
                }
                // Replace-not-merge, the same rule as `content`/`details` (Pi
                // `usage: afterResult.usage ?? result.usage`, agent-loop.ts:738; types.ts:70-78:
                // "There is no deep merge for `content`, `details`, or `usage`").
                if let Some(u) = ov.usage {
                    usage = Some(u);
                }
                if let Some(e) = ov.is_error {
                    is_error = e;
                }
                if let Some(t) = ov.terminate {
                    terminate = t;
                }
            }
            Ok(None) => {}
            Err(e) => {
                // Pi discards the whole result for `createErrorToolResult(…)` when the hook throws
                // (agent-loop.ts:743-745), and that carries neither usage nor added tool names. The
                // replacement content is the thrown value's own text
                // (`error instanceof Error ? error.message : String(error)`, agent-loop.ts:744), so
                // the failing hook's reason — not a fixed label — is what the model reads back.
                content = vec![Content::text(e.to_string())];
                details = None;
                usage = None;
                added_tool_names = Vec::new();
                is_error = true;
                terminate = false;
            }
        }

        let message = ToolResultMessage {
            tool_call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            content,
            details,
            usage,
            added_tool_names,
            is_error,
            // Pi `createToolResultMessage` stamps every tool result with `Date.now()`
            // (agent-loop.ts:741); this reaches the wire payload via `convert_to_llm`.
            timestamp: now_millis(),
        };
        Finalized {
            source_index,
            tool_call_id: call_id,
            tool_name,
            result_value: result_value_of(
                &message.content,
                &message.details,
                message.usage.as_ref(),
                &message.added_tool_names,
                terminate,
            ),
            is_error,
            terminate,
            message,
        }
    }

    /// Parallel batch: `tool_execution_start` in source order, `tool_execution_end` in completion
    /// order, tool-result messages + `turn_end.toolResults` in source order (R-02-015/016/017).
    ///
    /// Preparation and execution are two distinct phases. Pi's `executeToolCallsParallel` pushes a
    /// LAZY closure per prepared call while it walks the batch (agent-loop.ts:522-533) and only
    /// invokes them in the `Promise.all` that follows the loop (agent-loop.ts:540-542), so NO tool
    /// body starts until EVERY call in the batch has been prepared. That matters because
    /// `before_tool_call` is where the permission dialog blocks on a human: starting call #1 while
    /// call #2's dialog is still open would let a tool run against state the user has not yet
    /// approved. Deferring the start is not serialization — once the whole batch is prepared the
    /// bodies are spawned together and run concurrently, exactly as `Promise.all` does.
    async fn execute_parallel(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[AgentMessage],
        calls: &[ToolCall],
    ) -> Batch {
        let n = calls.len();
        let mut finalized: Vec<Option<Finalized>> = (0..n).map(|_| None).collect();
        let (tx, mut rx) = mpsc::channel::<ToolRuntimeMsg>(64);
        let mut joinset: JoinSet<()> = JoinSet::new();
        /// One prepared-but-not-yet-started call — the Rust analogue of Pi's deferred
        /// `finalizedCalls.push(async () => …)` closure.
        struct Deferred {
            source_index: usize,
            tool: Arc<dyn Tool>,
            args: Value,
            call_id: ToolCallId,
            tool_name: String,
        }
        let mut deferred: Vec<Deferred> = Vec::new();

        for (idx, call) in calls.iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object(call.arguments.clone()),
            })
            .await;
            match self.prepare(assistant, ctx_messages, call).await {
                Prep::Immediate(fin) => {
                    let mut fin = *fin;
                    fin.source_index = idx;
                    self.emit(AgentEvent::ToolExecutionEnd {
                        tool_call_id: fin.tool_call_id.clone(),
                        tool_name: fin.tool_name.clone(),
                        result: fin.result_value.clone(),
                        is_error: fin.is_error,
                    })
                    .await;
                    if let Some(slot) = finalized.get_mut(idx) {
                        *slot = Some(fin);
                    }
                }
                // Prepared only — the body is NOT started here. Pi defers it to the
                // post-loop `Promise.all` so a later call's `before_tool_call` cannot
                // still be open while this one runs.
                Prep::Ready { tool, args } => deferred.push(Deferred {
                    source_index: idx,
                    tool,
                    args,
                    call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                }),
            }
            if self.cancel.is_cancelled() {
                break;
            }
        }

        // Phase two — every call in the batch is prepared; start them all together
        // (Pi `await Promise.all(finalizedCalls.map(…))`, agent-loop.ts:540-542). Calls
        // deferred before an abort broke the loop are still started, exactly as Pi's
        // already-pushed closures are.
        let mut remaining = deferred.len();
        for Deferred { source_index, tool, args, call_id, tool_name } in deferred {
            let accepting = Arc::new(AtomicBool::new(true));
            let acc2 = accepting.clone();
            let utx = tx.clone();
            let ftx = tx.clone();
            let cid = call_id;
            let child = self.cancel.child();
            joinset.spawn(async move {
                let sink_cid = cid.clone();
                let on_update: ToolUpdateSink = Box::new(move |u: ToolUpdate| {
                    if acc2.load(Ordering::Acquire) {
                        let _ = utx.try_send(ToolRuntimeMsg::Update {
                            call_id: sink_cid.clone(),
                            partial: u,
                        });
                    }
                });
                let outcome = tool.execute(cid.clone(), args, child, on_update).await;
                accepting.store(false, Ordering::Release);
                let _ = ftx
                    .send(ToolRuntimeMsg::Finished {
                        call_id: cid,
                        source_index,
                        tool_name,
                        outcome,
                    })
                    .await;
            });
        }
        drop(tx);

        while remaining > 0 {
            match rx.recv().await {
                None => break,
                Some(ToolRuntimeMsg::Update { call_id, partial }) => {
                    let (tn, ar) = calls
                        .iter()
                        .find(|c| c.id == call_id)
                        .map(|c| (c.name.clone(), Value::Object(c.arguments.clone())))
                        .unwrap_or_default();
                    self.emit(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: call_id,
                        tool_name: tn,
                        args: ar,
                        partial_result: update_value(&partial),
                    })
                    .await;
                }
                Some(ToolRuntimeMsg::Finished { call_id, source_index, tool_name, outcome }) => {
                    let (args, call) = calls
                        .iter()
                        .find(|c| c.id == call_id)
                        .map(|c| (Value::Object(c.arguments.clone()), c.clone()))
                        .unwrap_or_else(|| {
                            // Defensive: the id always matches a source call; synthesize a stand-in.
                            (Value::Null, ToolCall {
                                id: call_id.clone(),
                                name: tool_name.clone(),
                                arguments: serde_json::Map::new(),
                                thought_signature: None,
                            })
                        });
                    let fin =
                        self.finalize(assistant, ctx_messages, &call, source_index, args, outcome).await;
                    self.emit(AgentEvent::ToolExecutionEnd {
                        tool_call_id: call_id,
                        tool_name,
                        result: fin.result_value.clone(),
                        is_error: fin.is_error,
                    })
                    .await;
                    if let Some(slot) = finalized.get_mut(source_index) {
                        *slot = Some(fin);
                    }
                    remaining -= 1;
                }
            }
        }
        while joinset.join_next().await.is_some() {}

        let mut tool_results = Vec::new();
        let mut all_terminate = !finalized.is_empty();
        for slot in finalized.into_iter() {
            match slot {
                Some(fin) => {
                    if !fin.terminate {
                        all_terminate = false;
                    }
                    let msg = AgentMessage::ToolResult(fin.message.clone());
                    self.emit(AgentEvent::MessageStart { message: msg.clone() }).await;
                    self.emit(AgentEvent::MessageEnd { message: msg }).await;
                    tool_results.push(fin.message);
                }
                None => all_terminate = false,
            }
        }
        Batch { messages: tool_results, terminate: all_terminate }
    }

    /// Sequential batch: each call fully processed before the next; abort breaks the loop (R-02-018).
    async fn execute_sequential(
        &self,
        assistant: &AssistantMessage,
        ctx_messages: &[AgentMessage],
        calls: &[ToolCall],
    ) -> Batch {
        let mut tool_results = Vec::new();
        let mut all_terminate = !calls.is_empty();
        let mut produced = 0usize;

        for (idx, call) in calls.iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: Value::Object(call.arguments.clone()),
            })
            .await;

            let fin = match self.prepare(assistant, ctx_messages, call).await {
                Prep::Immediate(fin) => {
                    let mut fin = *fin;
                    fin.source_index = idx;
                    fin
                }
                Prep::Ready { tool, args } => {
                    let (utx, mut urx) = mpsc::channel::<ToolUpdate>(64);
                    let accepting = Arc::new(AtomicBool::new(true));
                    let acc2 = accepting.clone();
                    let on_update: ToolUpdateSink = Box::new(move |u| {
                        if acc2.load(Ordering::Acquire) {
                            let _ = utx.try_send(u);
                        }
                    });
                    let child = self.cancel.child();
                    let exec = tool.execute(call.id.clone(), args.clone(), child, on_update);
                    tokio::pin!(exec);
                    let outcome = loop {
                        tokio::select! {
                            biased;
                            u = urx.recv() => {
                                if let Some(u) = u {
                                    self.emit(AgentEvent::ToolExecutionUpdate {
                                        tool_call_id: call.id.clone(),
                                        tool_name: call.name.clone(),
                                        args: Value::Object(call.arguments.clone()),
                                        partial_result: update_value(&u),
                                    })
                                    .await;
                                }
                            }
                            r = &mut exec => break r,
                        }
                    };
                    accepting.store(false, Ordering::Release);
                    self.finalize(assistant, ctx_messages, call, idx, args, outcome).await
                }
            };

            self.emit(AgentEvent::ToolExecutionEnd {
                tool_call_id: fin.tool_call_id.clone(),
                tool_name: fin.tool_name.clone(),
                result: fin.result_value.clone(),
                is_error: fin.is_error,
            })
            .await;
            if !fin.terminate {
                all_terminate = false;
            }
            let msg = AgentMessage::ToolResult(fin.message.clone());
            self.emit(AgentEvent::MessageStart { message: msg.clone() }).await;
            self.emit(AgentEvent::MessageEnd { message: msg }).await;
            tool_results.push(fin.message);
            produced += 1;

            if self.cancel.is_cancelled() {
                break;
            }
        }
        if produced == 0 {
            all_terminate = false;
        }
        Batch { messages: tool_results, terminate: all_terminate }
    }
}

/// Settlement safety-net (func-02 R-02-048): flips the run's settlement signals on scope exit —
/// the happy path AND any unwind (e.g. an uncontained panic on the run task) — so `wait_for_idle()`
/// can NEVER deadlock. The happy path records the run's new messages via [`SettlementGuard::complete`];
/// on an unwind the oneshot resolves to an empty `Vec`.
///
/// The run-active flag it clears is `running_tx` ITSELF, and deliberately not a second bool beside
/// it: `wait_for_idle()` releases on `running_tx` going false, so any separate "is a run in flight"
/// latch cleared AFTERWARDS opens a window in which a caller that has just been woken by this very
/// send is told the agent is idle and is then rejected with [`AgentError::RunActive`] by
/// [`Agent::start_run`]. That window is exactly two statements wide but a preemption between them
/// (routine under a loaded machine) stretches it to milliseconds — long enough for a woken caller
/// to run a full `prompt` preflight — which is how a `prompt(); wait_for_idle(); prompt()` sequence
/// could fail non-deterministically under parallel load.
struct SettlementGuard {
    state: Arc<Mutex<StateInner>>,
    cancel_slot: Arc<Mutex<Option<RunCancel>>>,
    running_tx: watch::Sender<bool>,
    result_tx: Option<oneshot::Sender<Vec<AgentMessage>>>,
    new_messages: Vec<AgentMessage>,
}

impl SettlementGuard {
    fn complete(&mut self, new_messages: Vec<AgentMessage>) {
        self.new_messages = new_messages;
    }
}

impl Drop for SettlementGuard {
    fn drop(&mut self) {
        {
            let mut st = lock(&self.state);
            st.is_streaming = false;
        }
        *lock(&self.cancel_slot) = None;
        // The ONE settlement write. Everything a waiter can observe about "is a run in flight" is
        // this channel, so the instant it reads `false` a fresh `start_run` is guaranteed to be
        // accepted — there is no second flag left set behind it.
        let _ = self.running_tx.send(false);
        if let Some(tx) = self.result_tx.take() {
            let _ = tx.send(std::mem::take(&mut self.new_messages));
        }
    }
}

// ---------------------------------------------------------------------------
// The public Agent
// ---------------------------------------------------------------------------

/// The stateful, high-level agent front-ends and extensions use (func-02 R-02-057).
pub struct Agent {
    state: Arc<Mutex<StateInner>>,
    subscribers: Arc<Mutex<Vec<Arc<dyn EventSubscriber>>>>,
    steering: Arc<Mutex<PendingQueue>>,
    follow_up: Arc<Mutex<PendingQueue>>,
    hooks: Arc<dyn Hooks>,
    stream_fn: Arc<dyn StreamFn>,
    key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    cancel_slot: Arc<Mutex<Option<RunCancel>>>,
    /// The SINGLE run-in-flight latch (R-02-045..048). `start_run` claims it with an atomic
    /// compare-and-set (`watch::Sender::send_if_modified`), [`SettlementGuard`] releases it, and
    /// both [`Agent::wait_for_idle`] and [`Agent::is_running`] read it — so "the waiter observed
    /// idle" and "a new run may start" are the same fact, never two facts written in sequence.
    running_tx: watch::Sender<bool>,
    running_rx: watch::Receiver<bool>,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
    gen_config: GenerationConfig,
}

impl Agent {
    pub fn builder(model: ModelRef, stream_fn: Arc<dyn StreamFn>) -> AgentBuilder {
        AgentBuilder::new(model, stream_fn)
    }

    /// Register a notify-only subscriber (func-02 R-02-012).
    pub fn subscribe(&self, s: Arc<dyn EventSubscriber>) {
        lock(&self.subscribers).push(s);
    }

    pub async fn snapshot(&self) -> AgentStateSnapshot {
        lock(&self.state).snapshot()
    }

    // --- scalar/array state setters (R-02-038/044) ---
    pub async fn set_system_prompt(&self, s: String) {
        lock(&self.state).system_prompt = s;
    }
    pub async fn set_model(&self, m: ModelRef) {
        lock(&self.state).model = m;
    }
    /// Replace the per-request header overlay (pi recomputes it per request inside `streamFn`,
    /// `sdk.ts:318-327`). The session facade calls this on every model change so provider-attribution
    /// and opencode session-affinity headers follow the ACTIVE provider.
    pub async fn set_headers(&self, h: Option<cyrup_provider::HeaderMap>) {
        lock(&self.state).headers = h;
    }
    /// Replace the preferred transport on the RUNNING agent — pi's `this.session.agent.transport =
    /// transport` (`interactive-mode.ts:4215`), the second half of the `/settings` "Transport"
    /// handler (the first half persists the setting). Applies from the next run onward, matching
    /// pi's read of `this.transport` in `createLoopConfig` (agent.ts:442).
    pub async fn set_transport(&self, t: Option<cyrup_provider::Transport>) {
        lock(&self.state).transport = t;
    }
    pub async fn set_thinking_level(&self, t: ModelThinkingLevel) {
        lock(&self.state).thinking_level = t;
    }
    /// Copies the top-level Vec (the caller's array is decoupled, R-02-038).
    pub async fn set_tools(&self, tools: Vec<Arc<dyn Tool>>) {
        lock(&self.state).tools = tools;
    }
    /// The agent's CURRENT tool set (Pi `agent.state.tools`, read by `_installAgentNextTurnRefresh`
    /// as `this.agent.state.tools.slice()`, agent-session.ts:533). `AgentStateSnapshot` reports only
    /// `tool_count` because a tool is not serializable; a caller that must re-push the live array
    /// onto a running loop — via [`crate::TurnUpdate::tools`] — needs the handles themselves.
    pub async fn tools(&self) -> Vec<Arc<dyn Tool>> {
        lock(&self.state).tools.clone()
    }
    /// Copies the top-level Vec (the caller's array is decoupled, R-02-038).
    pub async fn set_messages(&self, msgs: Vec<AgentMessage>) {
        lock(&self.state).messages = msgs;
    }

    // --- queues (R-02-034..037) ---
    pub fn steer(&self, m: AgentMessage) {
        lock(&self.steering).push(m);
    }
    pub fn follow_up(&self, m: AgentMessage) {
        lock(&self.follow_up).push(m);
    }
    pub fn set_steering_mode(&self, mode: QueueMode) {
        lock(&self.steering).set_mode(mode);
    }
    pub fn set_follow_up_mode(&self, mode: QueueMode) {
        lock(&self.follow_up).set_mode(mode);
    }
    pub fn clear_steering_queue(&self) {
        lock(&self.steering).clear();
    }
    pub fn clear_follow_up_queue(&self) {
        lock(&self.follow_up).clear();
    }
    pub fn clear_all_queues(&self) {
        self.clear_steering_queue();
        self.clear_follow_up_queue();
    }
    pub fn drain_queues_for_restore(&self) -> (Vec<AgentMessage>, Vec<AgentMessage>) {
        (lock(&self.steering).take_all(), lock(&self.follow_up).take_all())
    }

    // --- lifecycle (R-02-045..047) ---
    /// Signal the active run's abort token (idempotent, R-02-045).
    pub fn abort(&self) {
        if let Some(c) = lock(&self.cancel_slot).as_ref() {
            c.cancel();
        }
    }

    /// Resolve only after the current run emits `agent_end` and all awaited `agent_end` subscribers
    /// settle (R-02-047). Safe to call repeatedly; concurrent callers resolve together.
    pub async fn wait_for_idle(&self) {
        let mut rx = self.running_rx.clone();
        loop {
            if !*rx.borrow() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }

    /// Whether a run is in flight, read WITHOUT awaiting (Pi `_isAgentRunActive`, the flag behind
    /// `AgentSession.isIdle`, agent-session.ts:881-883). The sync counterpart of
    /// [`Self::wait_for_idle`]: an extension's `ctx.isIdle()` host import is a synchronous read and
    /// cannot await the latch.
    pub fn is_running(&self) -> bool {
        *self.running_rx.borrow()
    }

    /// Active run's abort signal, if one is active (Pi `agent.signal`, agent.ts:294-297). Callers can
    /// observe cancellation without holding the agent's internal slot.
    pub fn signal(&self) -> Option<CancelToken> {
        lock(&self.cancel_slot).as_ref().map(|c| c.token())
    }

    /// `true` when either queue still holds pending messages (Pi `hasQueuedMessages`,
    /// agent.ts:289-292).
    pub fn has_queued_messages(&self) -> bool {
        !lock(&self.steering).is_empty() || !lock(&self.follow_up).is_empty()
    }

    /// Clear transcript, runtime state, and queued messages — unconditionally, even mid-run (Pi
    /// `reset`, agent.ts:313-322). The `Result` is retained for signature back-compat and is always
    /// `Ok`.
    pub async fn reset(&self) -> Result<(), AgentError> {
        {
            let mut st = lock(&self.state);
            st.messages.clear();
            st.is_streaming = false;
            st.streaming_message = None;
            st.pending_tool_calls.clear();
            st.error_message = None;
        }
        lock(&self.steering).clear();
        lock(&self.follow_up).clear();
        Ok(())
    }

    // --- run entry points (R-02-001..006) ---
    pub async fn prompt(&self, input: impl Into<PromptInput>) -> Result<RunHandle, AgentError> {
        let input = input.into();
        self.start_run(EntryStart::Prompt(input.messages), false).await
    }

    /// Start a prompt from text plus image attachments (Pi `prompt(input, images?)`,
    /// agent.ts:326,379-383): the images are appended to the user message content after the text.
    pub async fn prompt_with_images(
        &self,
        text: impl Into<String>,
        images: Vec<Content>,
    ) -> Result<RunHandle, AgentError> {
        self.start_run(EntryStart::Prompt(vec![PromptInput::text_with_images(text, images).into_one()]), false)
            .await
    }

    pub async fn continue_run(&self) -> Result<RunHandle, AgentError> {
        let messages = lock(&self.state).messages.clone();
        if messages.is_empty() {
            return Err(AgentError::NoMessages);
        }
        let last_is_assistant = messages.last().map(|m| m.is_assistant()).unwrap_or(false);
        if last_is_assistant {
            // R-02-005: drain steering, else follow-up, treat as a fresh prompt; else error.
            // A steering-drain continuation skips the loop's FIRST steering poll so a second queued
            // steering message is not drained a turn too early (Pi `skipInitialSteeringPoll`,
            // agent.ts:349-352); a follow-up-drain continuation does NOT skip (agent.ts:354-357).
            let steering = lock(&self.steering).drain();
            if !steering.is_empty() {
                return self.start_run(EntryStart::Prompt(steering), true).await;
            }
            let follow = lock(&self.follow_up).drain();
            if follow.is_empty() {
                return Err(AgentError::ContinueFromAssistant);
            }
            return self.start_run(EntryStart::Prompt(follow), false).await;
        }
        self.start_run(EntryStart::Continue, false).await
    }

    async fn start_run(
        &self,
        entry: EntryStart,
        skip_initial_steering_poll: bool,
    ) -> Result<RunHandle, AgentError> {
        // Claim the run-in-flight latch with an atomic compare-and-set on the very channel
        // `wait_for_idle`/`is_running` observe (Pi's `_isAgentRunActive` guard, agent.ts:398-400 —
        // single-threaded JS gets this atomicity for free; Rust has to ask for it). `send_if_modified`
        // runs the closure under the channel's own write lock and notifies receivers only when it
        // returns `true`, so this both rejects a concurrent second run and publishes "running" in
        // one indivisible step. Using a SEPARATE bool here (as this did) meant a caller woken by
        // `SettlementGuard`'s `send(false)` could reach this guard before the guard's next statement
        // cleared that bool, and get a spurious `RunActive`.
        let claimed = self.running_tx.send_if_modified(|running| {
            if *running {
                false
            } else {
                *running = true;
                true
            }
        });
        if !claimed {
            return Err(AgentError::RunActive);
        }
        let cancel = RunCancel::new();
        *lock(&self.cancel_slot) = Some(cancel.clone());
        // A clone kept for the catch-all failure path so it can distinguish an aborted run from a
        // genuine error after `RunCtx` (which owns the run's `cancel`) has unwound (Pi
        // `handleRunFailure(error, signal.aborted)`, agent.ts:490,496-511).
        let fail_cancel = cancel.clone();

        let (system_prompt, model, thinking_level, tools, messages, transport) = {
            let mut st = lock(&self.state);
            st.error_message = None;
            st.is_streaming = true;
            // Pi `createContextSnapshot` hands the loop a `.slice()` COPY of `messages`
            // (agent.ts:424-429); the loop mutates only that copy while the agent's observable
            // `state.messages` grows independently via the reducer on `message_end`.
            (
                st.system_prompt.clone(),
                st.model.clone(),
                st.thinking_level,
                st.tools.clone(),
                st.messages.clone(),
                st.transport,
            )
        };
        // `transport` is LIVE state, not a build-time constant: pi reads `this.transport` when it
        // assembles the loop config at RUN START (`createLoopConfig`, agent.ts:442) and the
        // `/settings` row mutates that field on the running agent (`interactive-mode.ts:4215`).
        // Overlaying it here — rather than reading it per-turn inside the loop — reproduces pi's
        // snapshot semantics exactly: a `set_transport` between runs takes effect on the next run
        // and never re-targets an in-flight one.
        let gen_config = GenerationConfig { transport, ..self.gen_config.clone() };

        let mut rc = RunCtx::new(
            self.state.clone(),
            self.subscribers.clone(),
            self.steering.clone(),
            self.follow_up.clone(),
            self.hooks.clone(),
            self.stream_fn.clone(),
            self.key_resolver.clone(),
            self.tool_execution,
            self.session_id.clone(),
            system_prompt,
            model,
            thinking_level,
            gen_config,
            tools,
            messages,
            cancel,
            skip_initial_steering_poll,
        );

        let (tx, rx) = oneshot::channel();
        let state = self.state.clone();
        let running_tx = self.running_tx.clone();
        let cancel_slot = self.cancel_slot.clone();
        // Independent handles for the catch-all failure path (Pi `handleRunFailure`,
        // agent.ts:496-511): they must outlive the unwound `RunCtx`.
        let fail_state = self.state.clone();
        let fail_subs = self.subscribers.clone();

        tokio::spawn(async move {
            // The guard settles on scope exit no matter how this task ends (normal return OR an
            // unwind), so `wait_for_idle()` can never deadlock (func-02 R-02-048).
            let mut guard = SettlementGuard {
                state,
                cancel_slot,
                running_tx,
                result_tx: Some(tx),
                new_messages: Vec::new(),
            };
            // Run the loop; if its task UNWINDS (an uncontained panic in a hook/executor), synthesize
            // Pi's closing sequence — an error assistant message + `message_start/message_end/
            // turn_end/agent_end` — so subscribers always see a complete, well-formed termination
            // (Pi `handleRunFailure`, agent.ts:496-511), then settle with that message.
            match std::panic::AssertUnwindSafe(rc.run(entry)).catch_unwind().await {
                Ok(new) => guard.complete(new),
                Err(payload) => {
                    let model = { lock(&fail_state).model.clone() };
                    // Pi: `stopReason = aborted ? "aborted" : "error"` (agent.ts:504). An aborted run
                    // that unwinds is reported as aborted, everything else as error.
                    let aborted = fail_cancel.is_cancelled();
                    let stop_reason =
                        if aborted { StopReason::Aborted } else { StopReason::Error };
                    // Pi: `errorMessage = error instanceof Error ? error.message : String(error)`
                    // (agent.ts:505). Rust `catch_unwind` cannot recover an arbitrary error value,
                    // but a `panic!`/`unwrap` payload is a `&str`/`String` we can downcast to recover
                    // the real message; otherwise fall back to a generic string.
                    let error_message = panic_message(payload.as_ref());
                    // Pi `handleRunFailure` failure message: one empty text block + `Date.now()`
                    // (agent.ts:497-506), NOT empty content / a zero timestamp.
                    let failure = errored_assistant(
                        model.provider.clone(),
                        model.model.as_str(),
                        model.api.clone(),
                        stop_reason,
                        error_message,
                    );
                    let fm = AgentMessage::Assistant(failure);
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        AgentEvent::MessageStart { message: fm.clone() },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        AgentEvent::MessageEnd { message: fm.clone() },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        AgentEvent::TurnEnd { message: fm.clone(), tool_results: Vec::new() },
                    )
                    .await;
                    emit_standalone(
                        &fail_subs,
                        &fail_state,
                        AgentEvent::AgentEnd { messages: vec![fm.clone()] },
                    )
                    .await;
                    guard.complete(vec![fm]);
                }
            }
        });

        Ok(RunHandle { new_messages: rx })
    }
}

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

    pub fn system_prompt(mut self, s: impl Into<String>) -> Self {
        self.system_prompt = s.into();
        self
    }
    pub fn thinking_level(mut self, t: ModelThinkingLevel) -> Self {
        self.thinking_level = t;
        self
    }
    pub fn tools(mut self, tools: Vec<Arc<dyn Tool>>) -> Self {
        self.tools = tools;
        self
    }
    pub fn messages(mut self, messages: Vec<AgentMessage>) -> Self {
        self.messages = messages;
        self
    }
    pub fn hooks(mut self, hooks: Arc<dyn Hooks>) -> Self {
        self.hooks = Some(hooks);
        self
    }
    pub fn key_resolver(mut self, r: Arc<dyn ApiKeyResolver>) -> Self {
        self.key_resolver = Some(r);
        self
    }
    pub fn steering_mode(mut self, mode: QueueMode) -> Self {
        self.steering_mode = mode;
        self
    }
    pub fn follow_up_mode(mut self, mode: QueueMode) -> Self {
        self.follow_up_mode = mode;
        self
    }
    pub fn tool_execution(mut self, mode: ToolExecution) -> Self {
        self.tool_execution = mode;
        self
    }
    pub fn session_id(mut self, id: SessionId) -> Self {
        self.session_id = Some(id);
        self
    }

    // --- generation params + telemetry (Pi `AgentOptions`, agent.ts:96-116) ---

    /// Sampling temperature forwarded to the provider (Pi `SimpleStreamOptions.temperature`).
    pub fn temperature(mut self, t: f32) -> Self {
        self.gen_config.temperature = Some(t);
        self
    }
    /// Max output tokens forwarded to the provider (Pi `SimpleStreamOptions.maxTokens`).
    pub fn max_tokens(mut self, n: u64) -> Self {
        self.gen_config.max_tokens = Some(n);
        self
    }
    /// Prompt-cache retention preference (Pi `SimpleStreamOptions.cacheRetention`).
    pub fn cache_retention(mut self, r: cyrup_provider::CacheRetention) -> Self {
        self.gen_config.cache_retention = Some(r);
        self
    }
    /// Per-request header overlay (Pi `SimpleStreamOptions.headers`).
    pub fn headers(mut self, h: cyrup_provider::HeaderMap) -> Self {
        self.gen_config.headers = Some(h);
        self
    }
    /// Preferred transport (Pi `AgentOptions.transport`, agent.ts:113).
    pub fn transport(mut self, t: cyrup_provider::Transport) -> Self {
        self.gen_config.transport = Some(t);
        self
    }
    /// Cap (ms) on server-requested retry delays (Pi `AgentOptions.maxRetryDelayMs`, agent.ts:114).
    pub fn max_retry_delay_ms(mut self, ms: u64) -> Self {
        self.gen_config.max_retry_delay_ms = Some(ms);
        self
    }
    /// Max client-side retry attempts (Pi `SimpleStreamOptions.maxRetries`).
    pub fn max_retries(mut self, n: u32) -> Self {
        self.gen_config.max_retries = Some(n);
        self
    }
    /// Per-level custom thinking token budgets (Pi `AgentOptions.thinkingBudgets`, agent.ts:112).
    /// Forwarded into `cyrup_provider::StreamOptions.thinking_budgets`; budget-based providers honor
    /// it, others ignore it.
    pub fn thinking_budgets(mut self, b: cyrup_provider::ThinkingBudgets) -> Self {
        self.gen_config.thinking_budgets = Some(b);
        self
    }
    /// Static API-key fallback used when no dynamic [`ApiKeyResolver`] yields one (Pi `config.apiKey`
    /// fallback, agent-loop.ts:301-302).
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.gen_config.api_key = Some(key.into());
        self
    }
    /// Provider-scoped env overlay forwarded into `StreamOptions.env` (Pi `StreamOptions.env`,
    /// types.ts:184): the session builder seeds it with the `httpProxy` setting so the provider's
    /// proxy resolver honors the configured proxy (Pi `applyHttpProxySettings`, main.ts:744).
    pub fn provider_env(mut self, env: cyrup_provider::ProviderEnv) -> Self {
        self.gen_config.env = Some(env);
        self
    }
    /// HTTP request idle timeout (ms) forwarded into `StreamOptions.timeout_ms` (Pi
    /// `configureHttpDispatcher(getHttpIdleTimeoutMs())`, main.ts:745).
    ///
    /// Honored by the shared SSE transport for every wire API — see
    /// [`GenConfig::timeout_ms`](crate::state::GenConfig::timeout_ms) for the exact semantics
    /// (idle, not total; `0` disables).
    pub fn timeout_ms(mut self, ms: u64) -> Self {
        self.gen_config.timeout_ms = Some(ms);
        self
    }
    /// Telemetry: inspect/replace the provider payload before sending (Pi `AgentOptions.onPayload`,
    /// agent.ts:102).
    pub fn on_payload(mut self, f: cyrup_provider::OnPayload) -> Self {
        self.gen_config.on_payload = Some(f);
        self
    }
    /// Telemetry: invoked after the HTTP response arrives, before its body is read (Pi
    /// `AgentOptions.onResponse`, agent.ts:103).
    pub fn on_response(mut self, f: cyrup_provider::OnResponseHook) -> Self {
        self.gen_config.on_response = Some(f);
        self
    }

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
            gen_config: self.gen_config,
        }
    }
}
