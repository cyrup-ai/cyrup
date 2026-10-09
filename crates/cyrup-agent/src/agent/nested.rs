//! Tool calls that a tool makes while it runs — pi `NestedToolCallRunner`
//! (`packages/coding-agent/src/core/nested-tool-calls.ts:112-261` @v1.0.1), for example from
//! codemode scripts.
//!
//! The agent loop does not know about them: the host (the session) runs each one through the
//! agent's tool pipeline ([`super::run_tool_call`]) with its own hooks, emits
//! `tool_execution_*` events with `parentToolCallId` ([`NestedToolExecutionEvent`]), and records the
//! calls and their usage on the model-issued call's tool-result message
//! ([`NestedToolCallRunner::take_record`]).
//!
//! Nothing here runs until a tool calls [`NestedToolCallRunner::execute`].
//!
//! # Why the events are their own type
//!
//! Upstream types them as the loop's `tool_execution_*` events plus an optional `parentToolCallId`
//! (`WithParentToolCallId`, `agent-session.ts:182-190`). Here the loop's [`crate::AgentEvent`]
//! variants keep their shape — every consumer that matches them exhaustively is untouched, and a
//! loop event can never carry the key — and a nested event is a different value: the parent id is
//! required, not optional, so "a nested event without a parent" is unrepresentable. On the wire
//! both are `tool_execution_*` events; only a nested one has `parentToolCallId`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use cyrup_core::{
    CancelToken, Content, ExecMode, NestedCallHandle, NestedCallRecorder, NestedCallResult,
    NestedToolCalls, Tool, ToolCall, ToolCallId, ToolUpdate, ToolUpdateSink, Usage,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::ToolCallOutcome;
use super::message::{result_value_of, update_value};

/// A `tool_execution_*` event of a nested call (pi `NestedToolExecutionEvent`,
/// `nested-tool-calls.ts:130-148` @v1.0.1). The payload fields are those of the loop's own
/// `tool_execution_*` events; `parentToolCallId` is last, as pi's object literals write it.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum NestedToolExecutionEvent {
    ToolExecutionStart {
        tool_call_id: ToolCallId,
        tool_name: String,
        args: Value,
        parent_tool_call_id: ToolCallId,
    },
    ToolExecutionUpdate {
        tool_call_id: ToolCallId,
        tool_name: String,
        args: Value,
        partial_result: Value,
        parent_tool_call_id: ToolCallId,
    },
    ToolExecutionEnd {
        tool_call_id: ToolCallId,
        tool_name: String,
        result: Value,
        is_error: bool,
        /// Milliseconds the nested call's `execute()` took; absent when it did not run (pi
        /// `durationMs?`, before `parentToolCallId`, `nested-tool-calls.ts:124-128` @v1.1.0).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        parent_tool_call_id: ToolCallId,
    },
}

/// What the nested calls of one model-issued tool call leave on its tool-result message (pi
/// `NestedCallSummary`, `nested-tool-calls.ts:36-41`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NestedCallSummary {
    /// Becomes `nestedCalls`. `None` when no nested call was made.
    pub calls: Option<NestedToolCalls>,
    /// Summed `usage` of the nested results, added to the message's `usage`.
    pub usage: Option<Usage>,
}

/// Options of one nested call (pi `NestedToolCallOptions`, `nested-tool-calls.ts:105-110`).
#[derive(Default)]
pub struct NestedToolCallOptions {
    /// The call's abort signal. Pi: *"Defaults to the calling tool's signal"* — the default is the
    /// caller's to apply (`ctx.executeTool`); `None` here is an uncancellable call, as in pi.
    pub cancel: Option<CancelToken>,
    /// Receives partial results of the nested tool, in addition to `tool_execution_update`
    /// events.
    pub on_update: Option<ToolUpdateSink>,
}

/// The session side of [`NestedToolCallRunner`] (pi `NestedToolCallHost`,
/// `nested-tool-calls.ts:150-162`): everything the runner needs from outside itself, so the runner
/// has no session in it.
#[async_trait::async_trait]
pub trait NestedToolCallHost: Send + Sync {
    /// Tools nested calls resolve against.
    fn tools(&self) -> Vec<Arc<dyn Tool>>;

    /// Whether every nested call runs exclusively, as when the agent executes tool calls
    /// sequentially.
    fn is_sequential(&self) -> bool;

    /// Run the call through the tool pipeline, with hooks that report `parent`. `on_update`
    /// receives the tool's partial results while it runs.
    async fn run_tool_call(
        &self,
        call: ToolCall,
        parent: &ToolCallId,
        cancel: Option<CancelToken>,
        on_update: ToolUpdateSink,
    ) -> ToolCallOutcome;

    /// Deliver a nested `tool_execution_*` event.
    async fn emit(&self, event: NestedToolExecutionEvent);
}

/// Calls below one model-issued call share its recorder.
struct CallScope {
    recorder: Arc<Mutex<NestedCallRecorder>>,
    next_id: u32,
    /// Set inside a call that holds the exclusive queue, so its own nested calls do not wait on
    /// it — a nested call that waited on the queue its own caller holds would never run.
    holds_queue: bool,
}

/// Runs and records the tool calls a tool makes while it runs (pi `NestedToolCallRunner`).
///
/// One per session. Its state is the open scopes (by the id of the calling tool call) and the
/// exclusive queue; it holds no host — each [`Self::execute`] is handed one, so a host that borrows
/// the session needs no self-reference.
pub struct NestedToolCallRunner {
    /// Scopes by the id of the calling tool call. Never held across an await.
    scopes: Mutex<HashMap<ToolCallId, CallScope>>,
    /// Serializes nested calls that must not run concurrently. FIFO: tokio's mutex queues its
    /// waiters in arrival order, which is the order of pi's promise chain (`queueTail`).
    queue: Arc<tokio::sync::Mutex<()>>,
}

impl Default for NestedToolCallRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Removes a nested call's own scope when its run ends, however it ends — pi's `finally`
/// (`this.scopes.delete(toolCall.id)`) also covers a run whose future is dropped.
struct ScopeGuard<'a> {
    scopes: &'a Mutex<HashMap<ToolCallId, CallScope>>,
    id: ToolCallId,
}

impl Drop for ScopeGuard<'_> {
    fn drop(&mut self) {
        lock(self.scopes).remove(&self.id);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The text blocks of a result, joined (pi `textOf`, `nested-tool-calls.ts:171-176`).
fn text_of(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl NestedToolCallRunner {
    pub fn new() -> Self {
        Self {
            scopes: Mutex::new(HashMap::new()),
            queue: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Run `name` on behalf of the call `caller`. The nested call gets the id `<caller>/<n>`.
    /// **Never fails for tool failures**: an unknown tool, a validation error, a blocked call and a
    /// thrown error all come back as an outcome with `is_error` set (pi: *"Never rejects for tool
    /// failures"*).
    ///
    /// `args` is the call's arguments object; `Null` is an empty object (pi `args ?? {}`).
    /// Anything else is not an arguments object and settles as an error outcome without reaching
    /// the host's pipeline, because a [`ToolCall`] carries a JSON object and nothing else.
    pub async fn execute(
        &self,
        host: &dyn NestedToolCallHost,
        caller: &ToolCallId,
        name: &str,
        args: Value,
        options: NestedToolCallOptions,
    ) -> ToolCallOutcome {
        let NestedToolCallOptions { cancel, on_update } = options;
        let (recorder, number, caller_holds_queue) = {
            let mut scopes = lock(&self.scopes);
            let scope = scopes.entry(caller.clone()).or_insert_with(|| CallScope {
                recorder: Arc::new(Mutex::new(NestedCallRecorder::new())),
                next_id: 1,
                holds_queue: false,
            });
            let number = scope.next_id;
            scope.next_id += 1;
            (scope.recorder.clone(), number, scope.holds_queue)
        };
        let (arguments, malformed) = match args {
            Value::Object(map) => (map, None),
            Value::Null => (serde_json::Map::new(), None),
            other => (serde_json::Map::new(), Some(json_type_name(&other))),
        };
        let call = ToolCall {
            id: ToolCallId::from(format!("{caller}/{number}")),
            name: name.to_string(),
            arguments: arguments.into(),
            thought_signature: None,
            namespace: None,
        };
        let args_value = Value::Object((*call.arguments).clone());
        let record = lock(&recorder).start(&call);
        host.emit(NestedToolExecutionEvent::ToolExecutionStart {
            tool_call_id: call.id.clone(),
            tool_name: name.to_string(),
            args: args_value.clone(),
            parent_tool_call_id: caller.clone(),
        })
        .await;

        let outcome = match malformed {
            Some(kind) => ToolCallOutcome {
                is_error: true,
                // Refused before it ran.
                duration_ms: None,
                result: cyrup_core::ToolResult {
                    content: vec![Content::text(format!(
                        "Invalid arguments for tool {name}: expected an object, got {kind}"
                    ))],
                    details: Some(Value::Object(serde_json::Map::new())),
                    ..cyrup_core::ToolResult::default()
                },
                tool_call: call.clone(),
            },
            None => {
                self.run_exclusive_if_needed(
                    host,
                    caller,
                    caller_holds_queue,
                    &recorder,
                    call.clone(),
                    &args_value,
                    cancel,
                    on_update,
                )
                .await
            }
        };

        self.settle(&recorder, record, &outcome);
        host.emit(NestedToolExecutionEvent::ToolExecutionEnd {
            tool_call_id: call.id.clone(),
            tool_name: name.to_string(),
            result: result_value_of(
                &outcome.result.content,
                &outcome.result.details,
                outcome.result.usage.as_ref(),
                &outcome.result.added_tool_names,
                outcome.result.terminate,
                outcome.result.structured_content.as_ref(),
                outcome.result.is_error,
            ),
            is_error: outcome.is_error,
            duration_ms: outcome.duration_ms,
            parent_tool_call_id: caller.clone(),
        })
        .await;
        outcome
    }

    /// Take the exclusive queue when this call must not run concurrently with others, open the
    /// call's own scope, and run it through the host.
    #[allow(clippy::too_many_arguments)]
    async fn run_exclusive_if_needed(
        &self,
        host: &dyn NestedToolCallHost,
        caller: &ToolCallId,
        caller_holds_queue: bool,
        recorder: &Arc<Mutex<NestedCallRecorder>>,
        call: ToolCall,
        args_value: &Value,
        cancel: Option<CancelToken>,
        on_update: Option<ToolUpdateSink>,
    ) -> ToolCallOutcome {
        let exclusive = !caller_holds_queue
            && (host.is_sequential()
                || host
                    .tools()
                    .iter()
                    .find(|tool| tool.name() == call.name)
                    .is_some_and(|tool| tool.execution_mode() == ExecMode::Sequential));
        // Held for the whole run; dropped (released) when this function returns, or when its future
        // is dropped.
        let _queue = if exclusive {
            Some(self.queue.clone().lock_owned().await)
        } else {
            None
        };
        lock(&self.scopes).insert(
            call.id.clone(),
            CallScope {
                recorder: recorder.clone(),
                next_id: 1,
                holds_queue: caller_holds_queue || exclusive,
            },
        );
        let _scope = ScopeGuard {
            scopes: &self.scopes,
            id: call.id.clone(),
        };
        self.drive(host, caller, call, args_value, cancel, on_update)
            .await
    }

    /// Run the call through the host while relaying its partial results: each goes to the caller's
    /// `on_update` and out as a `tool_execution_update` event.
    async fn drive(
        &self,
        host: &dyn NestedToolCallHost,
        caller: &ToolCallId,
        call: ToolCall,
        args_value: &Value,
        cancel: Option<CancelToken>,
        mut on_update: Option<ToolUpdateSink>,
    ) -> ToolCallOutcome {
        // Unbounded for the reason the batch runtime's is: the only drop rule is the tool having
        // settled, and a tool that emits and returns without awaiting must still be heard.
        let (tx, mut rx) = mpsc::unbounded_channel::<ToolUpdate>();
        let sink: ToolUpdateSink = Box::new(move |update| {
            let _ = tx.send(update);
        });
        let call_id = call.id.clone();
        let name = call.name.clone();
        let relay = async |update: ToolUpdate, on_update: &mut Option<ToolUpdateSink>| {
            if let Some(f) = on_update.as_mut() {
                f(update.clone());
            }
            host.emit(NestedToolExecutionEvent::ToolExecutionUpdate {
                tool_call_id: call_id.clone(),
                tool_name: name.clone(),
                args: args_value.clone(),
                partial_result: update_value(&update),
                parent_tool_call_id: caller.clone(),
            })
            .await;
        };
        let run = host.run_tool_call(call, caller, cancel, sink);
        tokio::pin!(run);
        // A tool may drop its sink long before it returns; once every sender is gone `recv` yields
        // `None` forever, so the branch is disabled rather than polled again (a biased select would
        // otherwise never reach `run`).
        let mut sink_open = true;
        let outcome = loop {
            tokio::select! {
                biased;
                update = rx.recv(), if sink_open => match update {
                    Some(update) => relay(update, &mut on_update).await,
                    None => sink_open = false,
                },
                outcome = &mut run => break outcome,
            }
        };
        // A partial result sent immediately before the tool returned is still in the channel.
        while let Ok(update) = rx.try_recv() {
            relay(update, &mut on_update).await;
        }
        outcome
    }

    /// Record how the call ended and what it spent (pi `:236-238`). Nested results are not
    /// persisted, so their usage is only counted through the recorder.
    fn settle(
        &self,
        recorder: &Arc<Mutex<NestedCallRecorder>>,
        record: Option<NestedCallHandle>,
        outcome: &ToolCallOutcome,
    ) {
        let mut recorder = lock(recorder);
        if outcome.is_error {
            let text = text_of(&outcome.result.content);
            recorder.finish(record, NestedCallResult::Error(&text));
        } else {
            recorder.finish(record, NestedCallResult::Ok);
        }
        if let Some(usage) = &outcome.result.usage {
            recorder.add_usage(usage);
        }
    }

    /// Remove and return the record of the nested calls a model-issued call made (pi
    /// `takeRecord`). `None` when it made none, and for every call after the first take.
    pub fn take_record(&self, tool_call_id: &ToolCallId) -> Option<NestedCallSummary> {
        let scope = lock(&self.scopes).remove(tool_call_id)?;
        let recorder = lock(&scope.recorder);
        Some(NestedCallSummary {
            calls: recorder.snapshot(),
            usage: recorder.total_usage().cloned(),
        })
    }

    /// Forget every open scope (pi `clear`, at the end of a run).
    pub fn clear(&self) {
        lock(&self.scopes).clear();
    }
}

/// JSON type name for the error text of a malformed arguments value.
fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}
