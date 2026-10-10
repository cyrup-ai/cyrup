//! One execution, from the caller's side (pi `runtime/host.ts:63-274` `Execution` @v1.0.1).
//!
//! [`execute`] starts an isolate thread, then supervises it on the caller's runtime: it relays the
//! script's tool calls to the registered callbacks, collects output, and ends the execution on
//! the first of the script settling, the deadline, the caller's cancel token, or the sandbox
//! closing. Whatever the reason, the same teardown follows (`Execution.finish` upstream): the
//! still-pending calls are recorded `cancelled` and their tool tokens fire, and the isolate is
//! stopped.
//!
//! The supervisor is a plain future with no task of its own, so dropping the `execute` future
//! cancels the execution: a guard stops the isolate and fires the tool tokens on the way out.

use std::collections::{BTreeMap, HashMap};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use cyrup_codemode::types::OutputItem;
use cyrup_core::CancelToken;
use futures::FutureExt;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use super::isolate;
use super::lifecycle::{KillSwitch, Running};
use super::protocol::{
    CallTarget, HostReply, IsolateInit, ReplyPayload, ScriptSettled, UnobservedReport,
    WorkerMessage, effective_memory_limit, globals_json, out_of_memory_json, return_value,
    script_error, store_json, store_writes, tools_json,
};
use super::{Isolation, MAX_UNOBSERVED_CHARS, MAX_UNOBSERVED_SHOWN, process};
use crate::types::{
    CallStatus, CodemodeCall, CodemodeError, CodemodeResult, CodemodeStoreWrites, CodemodeTool,
    CodemodeToolContext, Deadline, ErrorKind, ReturnValue, ToolResult, UnobservedError,
    UnobservedErrors,
};

/// Stack of an isolate thread. V8 limits script recursion well below this (so deep recursion is a
/// catchable `RangeError`); the rest is for the engine's own frames.
pub(super) const ISOLATE_THREAD_STACK_BYTES: usize = 8 * 1024 * 1024;

/// What an execution shares with its sandbox.
pub(super) struct Hub {
    /// Cancelled by `close()`.
    pub close: CancelToken,
    pub running: Arc<Running>,
}

/// Everything one execution needs, captured when it starts: later `register_tool` calls do not
/// reach it (`host.ts:343` `new Map(this.toolsByName)`).
pub(super) struct Plan {
    pub isolation: Isolation,
    pub code: String,
    pub tools: Vec<CodemodeTool>,
    pub globals: Vec<CodemodeTool>,
    pub deadline: Deadline,
    pub active_limit: Option<Duration>,
    pub memory_limit: Option<u64>,
    pub store: Map<String, Value>,
    pub cancel: Option<CancelToken>,
    /// The message of an abort by `cancel`; [`ABORTED_BY_CALLER`] when unset.
    pub cancel_reason: Option<String>,
}

/// Stops the isolate and fires every tool token when the execution ends, however it ends,
/// including by the future being dropped.
struct Teardown {
    kill: Arc<KillSwitch>,
    tools: CancelToken,
}

impl Drop for Teardown {
    fn drop(&mut self) {
        self.tools.cancel();
        self.kill.kill();
    }
}

/// A call that has been handed to a tool callback and has not settled.
struct PendingCall {
    /// Index into the recorded calls; `None` for a global, which is not recorded.
    record: Option<usize>,
    started: Instant,
    /// The tool or global called.
    name: String,
}

/// [CYRUP-DELTA] A call that failed, kept until the script ends in case it never heard of it: the
/// reply is queued for the isolate, which stops listening the moment the script returns, so a call
/// that failed while the script was still on the line that made it (`tools.read(missing); return 1`)
/// is answered to nobody. See [`unobserved_errors`].
struct FailedCall {
    name: String,
    message: String,
}

/// How many failed calls a run remembers, the newest. A call the isolate had not heard of when the
/// script ended is one of the last it started, at most [`MAX_PENDING_CALLS`](super::MAX_PENDING_CALLS)
/// of them; a loop of failing calls it did hear of must not grow the host.
const MAX_REMEMBERED_FAILURES: usize = 4096;

type CallOutcome = (u32, ToolResult);

pub(super) async fn execute(hub: &Hub, plan: Plan) -> CodemodeResult {
    let _execution = hub.running.enter();
    let cancel = plan.cancel.clone().unwrap_or_default();
    if cancel.is_cancelled() {
        // Upstream aborts in the constructor, before any worker starts (`host.ts:114-116`).
        return failed(
            aborted(plan.cancel_reason.as_deref().unwrap_or(ABORTED_BY_CALLER)),
            Vec::new(),
            Vec::new(),
        );
    }

    let (to_host, mut from_worker) = mpsc::unbounded_channel::<WorkerMessage>();
    let (reply_tx, from_host) = mpsc::unbounded_channel::<HostReply>();
    let kill = Arc::new(KillSwitch::new());
    let tool_cancel = CancelToken::new();
    let _teardown = Teardown {
        kill: Arc::clone(&kill),
        tools: tool_cancel.clone(),
    };

    let init = IsolateInit {
        code: plan.code,
        tools_json: tools_json(&plan.tools),
        globals_json: globals_json(&plan.globals),
        store_json: store_json(&plan.store),
        memory_limit: plan.memory_limit.map(effective_memory_limit),
        active_limit_ms: plan.active_limit.map(millis),
    };
    let thread_guard = hub.running.enter();
    let started = match &plan.isolation {
        Isolation::InProcess => std::thread::Builder::new()
            .name(String::from("codemode-isolate"))
            .stack_size(ISOLATE_THREAD_STACK_BYTES)
            .spawn({
                let kill = Arc::clone(&kill);
                move || isolate::run_thread(init, to_host, from_host, kill, thread_guard)
            })
            .map(drop)
            .map_err(|error| format!("Failed to start worker: {error}")),
        Isolation::Process(command) => {
            process::start(command, init, to_host, from_host, &kill, thread_guard)
                .map_err(|error| format!("Failed to start the sandbox process: {error}"))
        }
    };
    if let Err(message) = started {
        return failed(sandbox_error(message), Vec::new(), Vec::new());
    }

    let (calls_tx, mut calls_rx) = mpsc::unbounded_channel::<CallOutcome>();
    let mut run = Run {
        tools: by_name(plan.tools),
        globals: by_name(plan.globals),
        tool_cancel,
        reply: reply_tx,
        calls_tx,
        active_limit: plan.active_limit,
        output: Vec::new(),
        calls: Vec::new(),
        pending: HashMap::new(),
        failures: BTreeMap::new(),
    };

    let deadline = async {
        match plan.deadline {
            Deadline::After(after) => tokio::time::sleep(after).await,
            Deadline::Never => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline);

    // Cancellation and the deadline are checked before messages: a script that floods the
    // channel (a loop that prints) must not starve them.
    let finish: Finish = loop {
        tokio::select! {
            biased;
            () = hub.close.cancelled() => break Err(aborted(ABORTED_BY_CLOSE)),
            () = cancel.cancelled() => break Err(aborted(
                plan.cancel_reason.as_deref().unwrap_or(ABORTED_BY_CALLER),
            )),
            () = &mut deadline => break Err(timed_out(plan.deadline)),
            message = from_worker.recv() => match message {
                None => break Err(sandbox_error(String::from(
                    "Worker exited before the script settled",
                ))),
                Some(message) => {
                    if let Some(finish) = run.handle(message) {
                        break finish;
                    }
                }
            },
            Some((id, result)) = calls_rx.recv() => run.complete(id, result),
        }
    };
    run.finish(finish)
}

const ABORTED_BY_CALLER: &str = "Execution aborted";
const ABORTED_BY_CLOSE: &str = "Sandbox closed";

fn aborted(message: &str) -> CodemodeError {
    CodemodeError {
        kind: ErrorKind::Aborted,
        name: None,
        message: message.to_owned(),
        stack: None,
    }
}

fn timed_out(deadline: Deadline) -> CodemodeError {
    let millis = match deadline {
        Deadline::After(after) => after.as_millis(),
        Deadline::Never => 0,
    };
    CodemodeError {
        kind: ErrorKind::Timeout,
        name: None,
        message: format!("Execution timed out after {millis} ms"),
        stack: None,
    }
}

/// [CYRUP-DELTA] The failure of a script that used up its running time. `Timeout`, like the
/// deadline, because it is the same remedy: the script needs more time than it was given.
fn active_limit_exceeded(limit: Option<Duration>) -> CodemodeError {
    let millis = limit.map_or(0, millis);
    CodemodeError {
        kind: ErrorKind::Timeout,
        name: None,
        message: format!(
            "Script ran for its limit of {millis} ms of its own time (waiting for tool calls does not count)"
        ),
        stack: None,
    }
}

fn sandbox_error(message: String) -> CodemodeError {
    CodemodeError {
        kind: ErrorKind::Sandbox,
        name: None,
        message,
        stack: None,
    }
}

fn failed(
    error: CodemodeError,
    output: Vec<OutputItem>,
    calls: Vec<CodemodeCall>,
) -> CodemodeResult {
    CodemodeResult::Failed {
        error,
        output,
        calls,
    }
}

fn by_name(tools: Vec<CodemodeTool>) -> HashMap<String, CodemodeTool> {
    tools
        .into_iter()
        .map(|tool| (tool.declaration.name.clone(), tool))
        .collect()
}

/// A script that ran to its end, before the output and calls are attached.
struct Completed {
    value: Option<ReturnValue>,
    store_writes: CodemodeStoreWrites,
    unobserved: UnobservedErrors,
}

type Finish = Result<Completed, CodemodeError>;

struct Run {
    tools: HashMap<String, CodemodeTool>,
    globals: HashMap<String, CodemodeTool>,
    /// Parent of every call's token.
    tool_cancel: CancelToken,
    reply: mpsc::UnboundedSender<HostReply>,
    calls_tx: mpsc::UnboundedSender<CallOutcome>,
    /// What the isolate was told to stop at; names the limit in the failure it reports.
    active_limit: Option<Duration>,
    output: Vec<OutputItem>,
    calls: Vec<CodemodeCall>,
    pending: HashMap<u32, PendingCall>,
    /// By call id, oldest first.
    failures: BTreeMap<u32, FailedCall>,
}

impl Run {
    /// `host.ts:183-199` `handleMessage`. `Some` ends the execution.
    fn handle(&mut self, message: WorkerMessage) -> Option<Finish> {
        match message {
            WorkerMessage::Output(item) => {
                self.output.push(item);
                None
            }
            WorkerMessage::Call {
                id,
                target,
                name,
                args,
            } => {
                self.start_call(id, target, name, args);
                None
            }
            WorkerMessage::Done(settled) => Some(settle(settled, &self.failures)),
            WorkerMessage::Crash(message) => Some(Err(sandbox_error(message))),
            WorkerMessage::OutOfMemory => Some(settle(
                ScriptSettled::Threw(out_of_memory_json()),
                &self.failures,
            )),
            WorkerMessage::ActiveLimit => Some(Err(active_limit_exceeded(self.active_limit))),
        }
    }

    /// `host.ts:210-240` `handleCall`, up to the tool callback's first suspension: a callback that
    /// does its work before its first `await` has done it by the time the next message is read.
    fn start_call(&mut self, id: u32, target: CallTarget, name: String, args: Option<String>) {
        let record = (target == CallTarget::Tool).then(|| {
            self.calls.push(CodemodeCall {
                name: name.clone(),
                status: CallStatus::Cancelled,
                duration_ms: 0,
            });
            self.calls.len().saturating_sub(1)
        });
        self.pending.insert(
            id,
            PendingCall {
                record,
                started: Instant::now(),
                name: name.clone(),
            },
        );
        let table = match target {
            CallTarget::Tool => &self.tools,
            CallTarget::Global => &self.globals,
        };
        let Some(tool) = table.get(&name) else {
            let kind = match target {
                CallTarget::Tool => "tool",
                CallTarget::Global => "global",
            };
            self.complete(id, Err(format!("Unknown {kind} \"{name}\"")));
            return;
        };
        let args = match args
            .as_deref()
            .map(serde_json::from_str::<Value>)
            .transpose()
        {
            Ok(args) => args,
            Err(error) => {
                self.complete(id, Err(error.to_string()));
                return;
            }
        };
        let callback = Arc::clone(&tool.execute);
        let context = CodemodeToolContext {
            cancel: self.tool_cancel.child_token(),
        };
        let Ok(mut future) = std::panic::catch_unwind(AssertUnwindSafe(|| callback(args, context)))
        else {
            self.complete(id, Err(String::from(TOOL_PANICKED)));
            return;
        };
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        // A noop waker is sound for a first poll: a future that returns `Pending` registers the
        // waker it is polled with next, which is the task's.
        match std::panic::catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(&mut cx))) {
            Err(_) => self.complete(id, Err(String::from(TOOL_PANICKED))),
            Ok(Poll::Ready(result)) => self.complete(id, result),
            Ok(Poll::Pending) => {
                let calls_tx = self.calls_tx.clone();
                tokio::spawn(async move {
                    let result = AssertUnwindSafe(future)
                        .catch_unwind()
                        .await
                        .unwrap_or_else(|_| Err(String::from(TOOL_PANICKED)));
                    // The execution may have ended; the result is then of no use.
                    let _ = calls_tx.send((id, result));
                });
            }
        }
    }

    /// The settled half of `handleCall`: record the status and duration, answer the script.
    fn complete(&mut self, id: u32, result: ToolResult) {
        // Not pending: the execution ended while the call ran, and the record already says
        // `cancelled` (`host.ts:234`).
        let Some(pending) = self.pending.remove(&id) else {
            return;
        };
        let (status, payload) = match result {
            Ok(value) => match value.as_ref().map(serde_json::to_string).transpose() {
                Ok(json) => (CallStatus::Ok, ReplyPayload::Value(json)),
                Err(error) => (CallStatus::Error, ReplyPayload::Failure(error.to_string())),
            },
            Err(message) => (CallStatus::Error, ReplyPayload::Failure(message)),
        };
        if let ReplyPayload::Failure(message) = &payload {
            self.failures.insert(
                id,
                FailedCall {
                    name: pending.name.clone(),
                    message: one_line(message),
                },
            );
            while self.failures.len() > MAX_REMEMBERED_FAILURES {
                self.failures.pop_first();
            }
        }
        if let Some(record) = pending.record.and_then(|index| self.calls.get_mut(index)) {
            record.status = status;
            record.duration_ms = millis(pending.started.elapsed());
        }
        // A closed channel is an isolate that already ended.
        let _ = self.reply.send(HostReply { id, payload });
    }

    /// `host.ts:242-273` `finish`: still-pending calls keep their `cancelled` status and get the
    /// time they ran, and the result is assembled. (The teardown that fires the tool tokens and
    /// stops the isolate is the [`Teardown`] guard, which runs as `execute` returns.)
    fn finish(self, finish: Finish) -> CodemodeResult {
        let Run {
            output,
            mut calls,
            pending,
            tool_cancel,
            ..
        } = self;
        for call in pending.into_values() {
            if let Some(record) = call.record.and_then(|index| calls.get_mut(index)) {
                record.duration_ms = millis(call.started.elapsed());
            }
        }
        tool_cancel.cancel();
        match finish {
            Ok(Completed {
                value,
                store_writes,
                unobserved,
            }) => CodemodeResult::Completed {
                value,
                output,
                calls,
                store_writes,
                unobserved,
            },
            Err(error) => failed(error, output, calls),
        }
    }
}

const TOOL_PANICKED: &str = "The tool callback panicked";

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// `message` on one line and no longer than [`MAX_UNOBSERVED_CHARS`] characters.
fn one_line(message: &str) -> String {
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    match flat.char_indices().nth(MAX_UNOBSERVED_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", flat.get(..end).unwrap_or_default()),
        None => flat,
    }
}

/// [CYRUP-DELTA] The errors a script that succeeded never looked at: the rejections the isolate says
/// nobody handled, and the calls that failed and that the script never heard of, because it ended
/// first (the isolate lists the calls it started and never saw settle; the host knows which of them
/// failed). Disjoint by construction: a reply the isolate received is not unsettled there.
///
/// The isolate lists those calls in two groups. A call nothing was waiting on is an error the script
/// lost (`total`, `shown`). A call something was waiting on (a `try`/`await` or a `Promise.all` that
/// had already rejected on a sibling, but also an `await` inside an `async` function nobody awaited,
/// or a `.then()` without a `.catch()`) is a failure the script did not see either, and whether it
/// minded is not something the engine can tell from outside: it is kept apart (`late_total`,
/// `late_shown`) for a note that claims nothing about what the script should have done.
fn unobserved_errors(
    report: UnobservedReport,
    failures: &BTreeMap<u32, FailedCall>,
) -> UnobservedErrors {
    let mut total = report.total;
    let mut shown: Vec<UnobservedError> = Vec::new();
    for rejection in report.unhandled {
        if shown.len() < MAX_UNOBSERVED_SHOWN {
            shown.push(UnobservedError {
                call: rejection.call,
                message: one_line(&rejection.message),
            });
        }
    }
    for failure in report.unsettled.iter().filter_map(|id| failures.get(id)) {
        total += 1;
        if shown.len() < MAX_UNOBSERVED_SHOWN {
            shown.push(failed_call_error(failure));
        }
    }
    let mut late_total = 0;
    let mut late_shown: Vec<UnobservedError> = Vec::new();
    for failure in report.waited.iter().filter_map(|id| failures.get(id)) {
        late_total += 1;
        if late_shown.len() < MAX_UNOBSERVED_SHOWN {
            late_shown.push(failed_call_error(failure));
        }
    }
    UnobservedErrors {
        total,
        shown,
        late_total,
        late_shown,
    }
}

fn failed_call_error(failure: &FailedCall) -> UnobservedError {
    UnobservedError {
        call: Some(failure.name.clone()),
        message: failure.message.clone(),
    }
}

/// `host.ts:201-208` `handleDone`.
fn settle(settled: ScriptSettled, failures: &BTreeMap<u32, FailedCall>) -> Finish {
    match settled {
        ScriptSettled::Threw(error_json) => {
            Err(script_error(&error_json).unwrap_or_else(|error| sandbox_error(error.to_string())))
        }
        ScriptSettled::Returned {
            value,
            writes,
            report,
        } => {
            let value = value
                .map(return_value)
                .transpose()
                .map_err(|error| sandbox_error(error.to_string()))?;
            let store_writes =
                store_writes(&writes).map_err(|error| sandbox_error(error.to_string()))?;
            Ok(Completed {
                value,
                store_writes,
                unobserved: unobserved_errors(report, failures),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]

    use std::sync::atomic::{AtomicBool, Ordering};

    use cyrup_codemode::types::ToolDeclaration;

    use super::*;

    fn run_with_tool(tool: CodemodeTool) -> (Run, mpsc::UnboundedReceiver<HostReply>) {
        let (reply, replies) = mpsc::unbounded_channel();
        let (calls_tx, _calls_rx) = mpsc::unbounded_channel();
        let run = Run {
            tools: by_name(vec![tool]),
            globals: HashMap::new(),
            tool_cancel: CancelToken::new(),
            reply,
            calls_tx,
            active_limit: None,
            output: Vec::new(),
            calls: Vec::new(),
            pending: HashMap::new(),
            failures: BTreeMap::new(),
        };
        (run, replies)
    }

    /// pi's `reports a broken bridge as a sandbox error` (`sandbox.test.ts` @v1.0.4, #10444): what
    /// the prelude sends when it finishes is decoded strictly, and a payload that does not decode
    /// ends the execution as a `sandbox` error that says why, instead of a script error with an
    /// empty message or a store write that is silently dropped.
    #[test]
    fn a_settlement_that_does_not_decode_is_a_sandbox_error_naming_the_reason() {
        let sandbox_message = |settled: ScriptSettled| match settle(settled, &BTreeMap::new()) {
            Ok(_) => panic!("a malformed settlement was accepted"),
            Err(error) => {
                assert_eq!(error.kind, ErrorKind::Sandbox, "{error:?}");
                error.message
            }
        };
        let returned = |writes: &str| ScriptSettled::Returned {
            value: Some(String::from("1")),
            writes: writes.to_owned(),
            report: UnobservedReport::default(),
        };
        for (settled, reason) in [
            (returned("null"), "store writes are not an array"),
            (returned("[1]"), "store writes contain a malformed entry"),
            (
                returned(r#"[["k","{"]]"#),
                "store value for \"k\" is not valid JSON",
            ),
            (
                ScriptSettled::Returned {
                    value: Some(String::from("{")),
                    writes: String::from("[]"),
                    report: UnobservedReport::default(),
                },
                "return value is not valid JSON",
            ),
            (
                ScriptSettled::Threw(String::from("5")),
                "script error is not an object",
            ),
            (
                ScriptSettled::Threw(String::from("{}")),
                "script error is malformed",
            ),
        ] {
            assert!(
                sandbox_message(settled).starts_with(&format!("Sandbox bridge broken: {reason}. ")),
                "{reason}"
            );
        }
    }

    fn call(id: u32, name: &str, args: Option<&str>) -> WorkerMessage {
        WorkerMessage::Call {
            id,
            target: CallTarget::Tool,
            name: name.to_owned(),
            args: args.map(str::to_owned),
        }
    }

    #[tokio::test]
    async fn a_tool_future_runs_to_its_first_suspension_before_the_next_message_is_read() {
        // Upstream's `tool.execute(...)` runs synchronously until its first `await` inside
        // `handleCall`; so does the callback's future here, polled once by the supervisor
        // before it is handed to a task. The test awaits nothing between the message and the
        // check, so a callback that has not run by then was not polled by the supervisor.
        let started = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&started);
        let tool = CodemodeTool {
            declaration: ToolDeclaration::new("slow"),
            execute: Arc::new(move |_, _| {
                let flag = Arc::clone(&flag);
                Box::pin(async move {
                    flag.store(true, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                    Ok(None)
                })
            }),
        };
        let (mut run, _replies) = run_with_tool(tool);
        assert!(run.handle(call(1, "slow", None)).is_none());
        assert!(started.load(Ordering::SeqCst));
        assert_eq!(run.pending.len(), 1, "still running, so still pending");
    }

    #[tokio::test]
    async fn a_tool_that_is_ready_at_its_first_poll_is_answered_at_once() {
        let tool = CodemodeTool {
            declaration: ToolDeclaration::new("now"),
            execute: Arc::new(|args, _| Box::pin(async move { Ok(args) })),
        };
        let (mut run, mut replies) = run_with_tool(tool);
        assert!(run.handle(call(7, "now", Some(r#"{"a":1}"#))).is_none());
        let reply = replies.try_recv().unwrap();
        assert_eq!(reply.id, 7);
        assert!(
            matches!(reply.payload, ReplyPayload::Value(Some(ref text)) if text == r#"{"a":1}"#)
        );
        assert!(run.pending.is_empty());
        assert_eq!(run.calls[0].status, CallStatus::Ok);
    }

    #[tokio::test]
    async fn an_unknown_tool_and_unreadable_arguments_are_errors_the_script_can_catch() {
        let tool = CodemodeTool {
            declaration: ToolDeclaration::new("known"),
            execute: Arc::new(|_, _| Box::pin(async { Ok(None) })),
        };
        let (mut run, mut replies) = run_with_tool(tool);
        run.handle(call(1, "missing", None));
        run.handle(call(2, "known", Some("{not json")));
        let first = replies.try_recv().unwrap();
        assert!(
            matches!(first.payload, ReplyPayload::Failure(ref m) if m == "Unknown tool \"missing\"")
        );
        let second = replies.try_recv().unwrap();
        assert!(matches!(second.payload, ReplyPayload::Failure(_)));
        assert_eq!(
            run.calls.iter().map(|c| c.status).collect::<Vec<_>>(),
            [CallStatus::Error, CallStatus::Error]
        );
    }
}
