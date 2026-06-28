//! The high-level [`Agent`] + the turn-based loop (arch-02 §3.5 / §6, func-02 §5/§6).
//!
//! One run = one tokio task that owns the `RunCancel` root. All event emission and hook invocation
//! happen on that single task, so ordering is deterministic; only tool `execute` bodies run
//! concurrently (on a `JoinSet`). The state lock is taken only for the synchronous reducer and is
//! never held across a subscriber `await` (deadlock-freedom, arch-02 §5.5).
//
// KNOWN GAPS (tracked):
// - R-02-056: a low-level free-fn loop layer for embedders is not provided; the high-level `Agent`
//   covers front-ends/extensions today. Revisit if an embedder needs the bare loop primitive.
// - R-02-020: `prepare_arguments` + JSON-Schema argument validation are not implemented; blocked
//   until arch-03 adds them to the `Tool` trait plus a validator. Args mutated by `before_tool_call`
//   currently run as-is, without re-validation (see R-02-022).
// - R-02-055 (partial): `thinking_level` / `thinkingBudgets` are stored on state but not forwarded to
//   the provider; blocked until `cyrup_provider::StreamOptions` gains thinking fields.
// - A-02-10 (second half): no mutable-aliasing state getter is exposed (snapshots are copies and
//   setters copy-on-assign). Intentional Rust `[CYRUP-DELTA]` from the TS source.

use crate::error::AgentError;
use crate::event::{AgentEvent, AgentMessage, ToolResultMessage};
use crate::hooks::{
    AfterToolCall, BeforeOutcome, BeforeToolCall, DefaultHooks, Hooks, PostTurn, TurnUpdate,
};
use crate::queue::{PendingQueue, QueueMode, ToolExecution};
use crate::state::{reduce, AgentStateSnapshot, StateInner};
use crate::stream_fn::{ApiKeyResolver, StreamFn};
use crate::subscriber::EventSubscriber;
use cyrup_core::{
    AssistantMessage, Content, ExecMode, ModelRef, RunCancel, SessionId, StopReason, ThinkingLevel,
    Tool, ToolCall, ToolCallId, ToolError, ToolResult, ToolUpdate, ToolUpdateSink, Usage,
};
use cyrup_provider::{Context, StreamEvent, StreamOptions};
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

fn empty_assistant(model: &ModelRef) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.model.to_string(),
        api: model.api.clone(),
        response_model: None,
        response_id: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    }
}

fn tool_calls(a: &AssistantMessage) -> Vec<ToolCall> {
    a.content
        .iter()
        .filter_map(|c| if let Content::ToolCall(tc) = c { Some(tc.clone()) } else { None })
        .collect()
}

fn result_value_of(content: &[Content], details: &Option<Value>) -> Value {
    serde_json::json!({ "content": content, "details": details })
}

fn update_value(u: &ToolUpdate) -> Value {
    serde_json::json!({ "content": u.content, "details": u.details })
}

// ---------------------------------------------------------------------------
// Public entry-point helpers
// ---------------------------------------------------------------------------

/// Input to [`Agent::prompt`]. Convertible from `&str`/`String`/`AgentMessage`/`Vec<AgentMessage>`.
pub struct PromptInput {
    pub messages: Vec<AgentMessage>,
}

impl From<&str> for PromptInput {
    fn from(s: &str) -> Self {
        Self { messages: vec![AgentMessage::user_text(s)] }
    }
}
impl From<String> for PromptInput {
    fn from(s: String) -> Self {
        Self { messages: vec![AgentMessage::user_text(s)] }
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
    Immediate(Finalized),
    Ready { tool: Arc<dyn Tool>, args: Value },
}

struct Batch {
    messages: Vec<ToolResultMessage>,
    terminate: bool,
}

enum EntryStart {
    Prompt(Vec<AgentMessage>),
    Continue,
}

// ---------------------------------------------------------------------------
// The run context (owns one run's working state; lives on the run task)
// ---------------------------------------------------------------------------

struct RunCtx {
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
    tools: Vec<Arc<dyn Tool>>,
    cancel: RunCancel,
    new_messages: Vec<AgentMessage>,
    next_override: Option<TurnUpdate>,
    turn_index: usize,
}

impl RunCtx {
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

    fn poll_steering(&self) -> Vec<AgentMessage> {
        lock(&self.steering).drain()
    }

    fn poll_follow_up(&self) -> Vec<AgentMessage> {
        lock(&self.follow_up).drain()
    }

    fn find_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    async fn run(&mut self, entry: EntryStart) -> Vec<AgentMessage> {
        self.emit(AgentEvent::AgentStart).await;
        match entry {
            EntryStart::Prompt(prompts) => {
                self.emit(AgentEvent::TurnStart).await;
                for p in prompts {
                    self.emit(AgentEvent::MessageStart { message: p.clone() }).await;
                    self.emit(AgentEvent::MessageEnd { message: p.clone() }).await;
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
        let mut pending = self.poll_steering();
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
                    self.new_messages.push(m);
                }

                let asst = self.stream_assistant().await;
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
                let mut terminated = false;
                if !calls.is_empty() {
                    let batch = self.execute_tool_calls(&calls).await;
                    tool_results = batch.messages;
                    terminated = batch.terminate;
                    has_more_tools = !terminated;
                    for r in &tool_results {
                        self.new_messages.push(AgentMessage::ToolResult(r.clone()));
                    }
                }

                self.emit(AgentEvent::TurnEnd { message: AgentMessage::Assistant(asst), tool_results })
                    .await;
                self.turn_index += 1;

                if terminated {
                    self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
                    return;
                }

                let prep = {
                    let ctx = PostTurn { messages: &self.new_messages, turn_index: self.turn_index };
                    self.hooks.prepare_next_turn(ctx).await
                };
                match prep {
                    Ok(Some(u)) => self.next_override = Some(u),
                    Ok(None) => {}
                    Err(_) => {
                        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
                        return;
                    }
                }

                let stop = {
                    let ctx = PostTurn { messages: &self.new_messages, turn_index: self.turn_index };
                    self.hooks.should_stop_after_turn(ctx).await
                };
                match stop {
                    Ok(true) => {
                        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
                        return;
                    }
                    Ok(false) => {}
                    Err(_) => {
                        self.emit(AgentEvent::AgentEnd { messages: self.new_messages.clone() }).await;
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
    async fn stream_assistant(&mut self) -> AssistantMessage {
        let over = self.next_override.take();
        let model = over.as_ref().and_then(|u| u.model.clone()).unwrap_or_else(|| self.model.clone());
        let ctx_override = over.and_then(|u| u.context);

        let base_messages = match ctx_override {
            Some(m) => m,
            None => lock(&self.state).messages.clone(),
        };

        let transformed =
            match self.hooks.transform_context(base_messages, self.cancel.child()).await {
                Ok(m) => m,
                Err(_) => return self.emit_error_assistant("transformContext failed", &model).await,
            };
        let llm = match self.hooks.convert_to_llm(&transformed).await {
            Ok(m) => m,
            Err(_) => return self.emit_error_assistant("convertToLlm failed", &model).await,
        };

        let api_key = match &self.key_resolver {
            Some(r) => r.get_api_key(&model.provider).await,
            None => None,
        };

        let tool_defs: Vec<cyrup_provider::ToolDef> = self
            .tools
            .iter()
            .map(|t| cyrup_provider::ToolDef {
                name: t.name().to_string(),
                description: String::new(),
                parameters: t.parameters().clone(),
            })
            .collect();

        let opts = StreamOptions {
            cancel: Some(self.cancel.child()),
            api_key,
            session_id: self.session_id.clone(),
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
        let mut acc = String::new();
        let mut partial = empty_assistant(&model);
        let mut final_msg: Option<AssistantMessage> = None;

        loop {
            tokio::select! {
                biased;
                _ = cancel_tok.cancelled() => {
                    if !started {
                        self.emit(AgentEvent::MessageStart {
                            message: AgentMessage::Assistant(partial.clone()),
                        })
                        .await;
                    }
                    let aborted = AssistantMessage::errored(
                        model.provider.clone(),
                        model.model.as_str(),
                        StopReason::Aborted,
                        "aborted",
                    );
                    self.emit(AgentEvent::MessageEnd {
                        message: AgentMessage::Assistant(aborted.clone()),
                    })
                    .await;
                    return aborted;
                }
                ev = stream.next() => {
                    match ev {
                        None => break,
                        Some(e) => match &e {
                            StreamEvent::Start => {
                                started = true;
                                self.emit(AgentEvent::MessageStart {
                                    message: AgentMessage::Assistant(partial.clone()),
                                })
                                .await;
                            }
                            StreamEvent::TextDelta { delta, .. }
                            | StreamEvent::ThinkingDelta { delta, .. } => {
                                acc.push_str(delta);
                                partial.content = vec![Content::text(acc.clone())];
                                self.emit(AgentEvent::MessageUpdate {
                                    message: AgentMessage::Assistant(partial.clone()),
                                    assistant_message_event: Box::new(e.clone()),
                                })
                                .await;
                            }
                            StreamEvent::ToolCallDelta { .. } => {
                                self.emit(AgentEvent::MessageUpdate {
                                    message: AgentMessage::Assistant(partial.clone()),
                                    assistant_message_event: Box::new(e.clone()),
                                })
                                .await;
                            }
                            StreamEvent::Done { message } | StreamEvent::Error { message } => {
                                final_msg = Some(message.clone());
                            }
                            _ => {}
                        },
                    }
                }
            }
        }

        let final_msg = final_msg.unwrap_or_else(|| {
            AssistantMessage::errored(
                model.provider.clone(),
                model.model.as_str(),
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

    async fn emit_error_assistant(&self, msg: &str, model: &ModelRef) -> AssistantMessage {
        let asst = AssistantMessage::errored(
            model.provider.clone(),
            model.model.as_str(),
            StopReason::Error,
            msg,
        );
        self.emit(AgentEvent::MessageStart { message: AgentMessage::Assistant(asst.clone()) })
            .await;
        self.emit(AgentEvent::MessageEnd { message: AgentMessage::Assistant(asst.clone()) }).await;
        asst
    }

    async fn execute_tool_calls(&self, calls: &[ToolCall]) -> Batch {
        let any_seq = calls.iter().any(|c| {
            self.find_tool(&c.name).map(|t| t.execution_mode() == ExecMode::Sequential).unwrap_or(false)
        });
        let sequential = any_seq || matches!(self.tool_execution, ToolExecution::Sequential);
        if sequential {
            self.execute_sequential(calls).await
        } else {
            self.execute_parallel(calls).await
        }
    }

    /// Preflight: locate tool → `before_tool_call`. Returns an immediate (finalized) error result
    /// or a prepared executor (func-02 R-02-019/021/022).
    async fn prepare(&self, call: &ToolCall) -> Prep {
        let tool = match self.find_tool(&call.name) {
            Some(t) => t,
            None => {
                return Prep::Immediate(
                    self.immediate_error(call, format!("Tool '{}' not found", call.name)),
                )
            }
        };
        if self.cancel.is_cancelled() {
            return Prep::Immediate(self.immediate_error(call, "Operation aborted"));
        }
        let mut args = call.arguments.clone();
        let before = {
            let ctx = BeforeToolCall {
                tool_name: &call.name,
                tool_call_id: &call.id,
                args: &mut args,
                messages: &self.new_messages,
            };
            self.hooks.before_tool_call(ctx, self.cancel.child()).await
        };
        match before {
            Err(_) => Prep::Immediate(self.immediate_error(call, "beforeToolCall failed")),
            Ok(BeforeOutcome::Block { reason }) => Prep::Immediate(self.immediate_error(
                call,
                reason.unwrap_or_else(|| "Tool call blocked by beforeToolCall".to_string()),
            )),
            // Args mutated in place are executed as-is, WITHOUT re-validation (R-02-022).
            Ok(BeforeOutcome::Proceed) => {
                if self.cancel.is_cancelled() {
                    Prep::Immediate(self.immediate_error(call, "Operation aborted"))
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
            is_error: true,
            timestamp: 0,
        };
        Finalized {
            source_index: 0,
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            result_value: result_value_of(&message.content, &message.details),
            is_error: true,
            terminate: false,
            message,
        }
    }

    /// Apply `after_tool_call` (replace-not-merge per field, R-02-025) and build the finalized
    /// result. On hook `Err`: error result, `terminate` ignored (R-02-025/050).
    async fn finalize(
        &self,
        call_id: ToolCallId,
        tool_name: String,
        source_index: usize,
        args: Value,
        outcome: Result<ToolResult, ToolError>,
    ) -> Finalized {
        let (mut content, mut details, mut terminate, mut is_error) = match outcome {
            Ok(r) => (r.content, r.details, r.terminate, false),
            Err(e) => (vec![Content::text(e.to_string())], None, false, true),
        };

        let hook_result = {
            let ctx = AfterToolCall {
                tool_name: &tool_name,
                tool_call_id: &call_id,
                args: &args,
                content: &content,
                details: details.as_ref(),
                is_error,
                terminate,
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
                if let Some(e) = ov.is_error {
                    is_error = e;
                }
                if let Some(t) = ov.terminate {
                    terminate = t;
                }
            }
            Ok(None) => {}
            Err(_) => {
                content = vec![Content::text("afterToolCall failed")];
                details = None;
                is_error = true;
                terminate = false;
            }
        }

        let message = ToolResultMessage {
            tool_call_id: call_id.clone(),
            tool_name: tool_name.clone(),
            content,
            details,
            is_error,
            timestamp: 0,
        };
        Finalized {
            source_index,
            tool_call_id: call_id,
            tool_name,
            result_value: result_value_of(&message.content, &message.details),
            is_error,
            terminate,
            message,
        }
    }

    /// Parallel batch: `tool_execution_start` in source order, `tool_execution_end` in completion
    /// order, tool-result messages + `turn_end.toolResults` in source order (R-02-015/016/017).
    async fn execute_parallel(&self, calls: &[ToolCall]) -> Batch {
        let n = calls.len();
        let mut finalized: Vec<Option<Finalized>> = (0..n).map(|_| None).collect();
        let (tx, mut rx) = mpsc::channel::<ToolRuntimeMsg>(64);
        let mut joinset: JoinSet<()> = JoinSet::new();
        let mut remaining = 0usize;

        for (idx, call) in calls.iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: call.arguments.clone(),
            })
            .await;
            match self.prepare(call).await {
                Prep::Immediate(mut fin) => {
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
                Prep::Ready { tool, args } => {
                    remaining += 1;
                    let accepting = Arc::new(AtomicBool::new(true));
                    let acc2 = accepting.clone();
                    let utx = tx.clone();
                    let ftx = tx.clone();
                    let cid = call.id.clone();
                    let tname = call.name.clone();
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
                                source_index: idx,
                                tool_name: tname,
                                outcome,
                            })
                            .await;
                    });
                }
            }
            if self.cancel.is_cancelled() {
                break;
            }
        }
        drop(tx);

        while remaining > 0 {
            match rx.recv().await {
                None => break,
                Some(ToolRuntimeMsg::Update { call_id, partial }) => {
                    let (tn, ar) = calls
                        .iter()
                        .find(|c| c.id == call_id)
                        .map(|c| (c.name.clone(), c.arguments.clone()))
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
                    let args = calls
                        .iter()
                        .find(|c| c.id == call_id)
                        .map(|c| c.arguments.clone())
                        .unwrap_or(Value::Null);
                    let fin = self.finalize(call_id.clone(), tool_name.clone(), source_index, args, outcome).await;
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
    async fn execute_sequential(&self, calls: &[ToolCall]) -> Batch {
        let mut tool_results = Vec::new();
        let mut all_terminate = !calls.is_empty();
        let mut produced = 0usize;

        for (idx, call) in calls.iter().enumerate() {
            self.emit(AgentEvent::ToolExecutionStart {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                args: call.arguments.clone(),
            })
            .await;

            let fin = match self.prepare(call).await {
                Prep::Immediate(mut fin) => {
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
                                        args: call.arguments.clone(),
                                        partial_result: update_value(&u),
                                    })
                                    .await;
                                }
                            }
                            r = &mut exec => break r,
                        }
                    };
                    accepting.store(false, Ordering::Release);
                    self.finalize(call.id.clone(), call.name.clone(), idx, args, outcome).await
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
struct SettlementGuard {
    state: Arc<Mutex<StateInner>>,
    cancel_slot: Arc<Mutex<Option<RunCancel>>>,
    running_tx: watch::Sender<bool>,
    active: Arc<Mutex<bool>>,
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
        let _ = self.running_tx.send(false);
        *lock(&self.active) = false;
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
    active: Arc<Mutex<bool>>,
    running_tx: watch::Sender<bool>,
    running_rx: watch::Receiver<bool>,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
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
    pub async fn set_thinking_level(&self, t: ThinkingLevel) {
        lock(&self.state).thinking_level = t;
    }
    /// Copies the top-level Vec (the caller's array is decoupled, R-02-038).
    pub async fn set_tools(&self, tools: Vec<Arc<dyn Tool>>) {
        lock(&self.state).tools = tools;
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

    pub async fn reset(&self) -> Result<(), AgentError> {
        if *lock(&self.active) {
            return Err(AgentError::RunActive);
        }
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
        self.start_run(EntryStart::Prompt(input.messages)).await
    }

    pub async fn continue_run(&self) -> Result<RunHandle, AgentError> {
        let messages = lock(&self.state).messages.clone();
        if messages.is_empty() {
            return Err(AgentError::NoMessages);
        }
        let last_is_assistant = messages.last().map(|m| m.is_assistant()).unwrap_or(false);
        if last_is_assistant {
            // R-02-005: drain steering, else follow-up, treat as a fresh prompt; else error.
            let mut drained = lock(&self.steering).drain();
            if drained.is_empty() {
                drained = lock(&self.follow_up).drain();
            }
            if drained.is_empty() {
                return Err(AgentError::ContinueFromAssistant);
            }
            return self.start_run(EntryStart::Prompt(drained)).await;
        }
        self.start_run(EntryStart::Continue).await
    }

    async fn start_run(&self, entry: EntryStart) -> Result<RunHandle, AgentError> {
        {
            let mut a = lock(&self.active);
            if *a {
                return Err(AgentError::RunActive);
            }
            *a = true;
        }
        let cancel = RunCancel::new();
        *lock(&self.cancel_slot) = Some(cancel.clone());

        let (system_prompt, model, tools) = {
            let mut st = lock(&self.state);
            st.error_message = None;
            st.is_streaming = true;
            (st.system_prompt.clone(), st.model.clone(), st.tools.clone())
        };
        let _ = self.running_tx.send(true);

        let mut rc = RunCtx {
            state: self.state.clone(),
            subscribers: self.subscribers.clone(),
            steering: self.steering.clone(),
            follow_up: self.follow_up.clone(),
            hooks: self.hooks.clone(),
            stream_fn: self.stream_fn.clone(),
            key_resolver: self.key_resolver.clone(),
            tool_execution: self.tool_execution,
            session_id: self.session_id.clone(),
            system_prompt,
            model,
            tools,
            cancel,
            new_messages: Vec::new(),
            next_override: None,
            turn_index: 0,
        };

        let (tx, rx) = oneshot::channel();
        let state = self.state.clone();
        let running_tx = self.running_tx.clone();
        let active = self.active.clone();
        let cancel_slot = self.cancel_slot.clone();

        tokio::spawn(async move {
            // The guard settles on scope exit no matter how this task ends (normal return OR an
            // unwind), so `wait_for_idle()` can never deadlock (func-02 R-02-048).
            let mut guard = SettlementGuard {
                state,
                cancel_slot,
                running_tx,
                active,
                result_tx: Some(tx),
                new_messages: Vec::new(),
            };
            let new = rc.run(entry).await;
            guard.complete(new);
        });

        Ok(RunHandle { new_messages: rx })
    }
}

/// Builder for [`Agent`].
pub struct AgentBuilder {
    system_prompt: String,
    model: ModelRef,
    thinking_level: ThinkingLevel,
    tools: Vec<Arc<dyn Tool>>,
    messages: Vec<AgentMessage>,
    hooks: Option<Arc<dyn Hooks>>,
    stream_fn: Arc<dyn StreamFn>,
    key_resolver: Option<Arc<dyn ApiKeyResolver>>,
    steering_mode: QueueMode,
    follow_up_mode: QueueMode,
    tool_execution: ToolExecution,
    session_id: Option<SessionId>,
}

impl AgentBuilder {
    pub fn new(model: ModelRef, stream_fn: Arc<dyn StreamFn>) -> Self {
        Self {
            system_prompt: String::new(),
            model,
            thinking_level: ThinkingLevel::Off,
            tools: Vec::new(),
            messages: Vec::new(),
            hooks: None,
            stream_fn,
            key_resolver: None,
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            tool_execution: ToolExecution::Parallel,
            session_id: None,
        }
    }

    pub fn system_prompt(mut self, s: impl Into<String>) -> Self {
        self.system_prompt = s.into();
        self
    }
    pub fn thinking_level(mut self, t: ThinkingLevel) -> Self {
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
            active: Arc::new(Mutex::new(false)),
            running_tx,
            running_rx,
            tool_execution: self.tool_execution,
            session_id: self.session_id,
        }
    }
}
