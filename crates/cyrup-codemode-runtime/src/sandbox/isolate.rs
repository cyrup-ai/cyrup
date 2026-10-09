//! One script in one V8 isolate on its own thread (pi `runtime/worker.ts` @v1.0.1).
//!
//! Upstream runs one script per worker thread in a fresh QuickJS VM, relays tool calls and output
//! to the host over the worker's message port, and relies on the host terminating the worker.
//! This is the same shape on V8: [`run_thread`] is the thread body, the ops below are the bridge
//! the prelude calls, and the supervisor ([`super::execution`]) stops the isolate through a
//! [`KillSwitch`] when the script settles, times out, is cancelled, or runs out of memory.
//!
//! Nothing here runs on the caller's runtime. The isolate thread has a current-thread tokio
//! runtime because the engine's event loop needs one (dynamic `import()` settles there); tool
//! callbacks run on the caller's runtime, and their results come back over a channel.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use cyrup_codemode::types::OutputItem;
use deno_core::error::JsError;
use deno_core::{JsRuntime, OpState, PollEventLoopOptions, RuntimeOptions, op2, v8};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::lifecycle::{KillSwitch, RunningGuard};
use super::protocol::{
    CallTarget, HostReply, IsolateInit, ReplyPayload, ScriptSettled, WorkerMessage,
    out_of_memory_json,
};
use super::{
    IMAGE_HELPER_EXPECTS, MAX_OUTPUT_CHARS, MAX_OUTPUT_ITEMS, MAX_STORE_TOTAL_CHARS,
    MAX_STORE_VALUE_CHARS,
};

/// How far the heap limit is raised when the script reaches it. The limit must be raised, never
/// left: V8's own response to an unhandled heap limit is `abort()`, which would take the host
/// process down. The headroom only has to last until the termination request is processed.
const OUT_OF_MEMORY_HEADROOM_BYTES: usize = 16 * 1024 * 1024;

/// The limit on memory behind `ArrayBuffer`s and typed arrays when the sandbox sets none. The
/// engine's heap limit does not count it, so without a number a script could grow the host
/// process until the operating system stopped it.
const DEFAULT_EXTERNAL_MEMORY_LIMIT_BYTES: usize = 1024 * 1024 * 1024;

/// How many bytes of new `ArrayBuffer`s the prelude lets through between two memory checks: a
/// sixteenth of the limit, within these bounds. A check is a full collection when over the limit
/// and a counter read otherwise.
const MIN_MEMORY_CHECK_STRIDE_BYTES: usize = 64 * 1024;
const MAX_MEMORY_CHECK_STRIDE_BYTES: usize = 4 * 1024 * 1024;

/// Consecutive event-loop errors tolerated before the isolate is given up on.
const MAX_EVENT_LOOP_ERRORS: usize = 10_000;

// ------------------------------------------------------------------------------------------------
// The bridge: what the prelude can call (worker.ts:64-100 `bridge`)
// ------------------------------------------------------------------------------------------------

struct Bridge {
    to_host: UnboundedSender<WorkerMessage>,
    /// Set when the script reported its end, so the thread stops waiting for tool results.
    settled: Rc<Cell<bool>>,
    /// Most memory the script may hold behind `ArrayBuffer`s and typed arrays.
    external_limit: usize,
}

fn post(state: &OpState, message: WorkerMessage) {
    if let Some(bridge) = state.try_borrow::<Bridge>() {
        // A closed channel means the supervisor already settled the execution.
        let _ = bridge.to_host.send(message);
    }
}

/// `bridge("call" | "global", id, name, argsJson)`.
#[op2(fast)]
fn op_codemode_call(
    state: &mut OpState,
    #[smi] id: u32,
    #[string] target: &str,
    #[string] name: &str,
    #[string] args: &str,
    has_args: bool,
) {
    let target = if target == "call" {
        CallTarget::Tool
    } else {
        CallTarget::Global
    };
    post(
        state,
        WorkerMessage::Call {
            id,
            target,
            name: name.to_owned(),
            args: has_args.then(|| args.to_owned()),
        },
    );
}

/// `bridge("output", "text", text)`, `bridge("output", "console", text)` and
/// `bridge("output", "image", data, mimeType)` (`worker.ts:80-85` @v1.1.0).
#[op2(fast)]
fn op_codemode_output(
    state: &mut OpState,
    #[string] kind: &str,
    #[string] data: String,
    #[string] mime_type: String,
) {
    let item = match kind {
        "image" => OutputItem::Image { data, mime_type },
        "console" => OutputItem::Console(data),
        _ => OutputItem::Text(data),
    };
    post(state, WorkerMessage::Output(item));
}

/// `bridge("done", true, valueJson, writesJson)`.
#[op2(fast)]
fn op_codemode_done_ok(
    state: &mut OpState,
    #[string] value: &str,
    has_value: bool,
    #[string] writes: String,
) {
    mark_settled(state);
    post(
        state,
        WorkerMessage::Done(ScriptSettled::Returned {
            value: has_value.then(|| value.to_owned()),
            writes,
        }),
    );
}

/// `bridge("done", false, errorJson)`.
#[op2(fast)]
fn op_codemode_done_err(state: &mut OpState, #[string] error: String) {
    mark_settled(state);
    post(state, WorkerMessage::Done(ScriptSettled::Threw(error)));
}

/// Whether the memory behind the script's `ArrayBuffer`s and typed arrays is over its limit.
/// The engine counts it as external memory; garbage is collected first so that only live buffers
/// are held against the script.
#[op2(fast)]
fn op_codemode_memory_exceeded(state: &mut OpState, isolate: &mut v8::Isolate) -> bool {
    let limit = state
        .try_borrow::<Bridge>()
        .map_or(usize::MAX, |bridge| bridge.external_limit);
    if isolate.get_heap_statistics().external_memory() <= limit {
        return false;
    }
    isolate.low_memory_notification();
    isolate.get_heap_statistics().external_memory() > limit
}

fn mark_settled(state: &OpState) {
    if let Some(bridge) = state.try_borrow::<Bridge>() {
        bridge.settled.set(true);
    }
}

deno_core::extension!(
    cyrup_codemode_sandbox,
    ops = [
        op_codemode_call,
        op_codemode_output,
        op_codemode_done_ok,
        op_codemode_done_err,
        op_codemode_memory_exceeded,
    ],
    docs = "The only capability a codemode script reaches: its prelude holds these four ops in a \
            closure and removes `Deno` before the script runs."
);

// ------------------------------------------------------------------------------------------------
// The prelude
// ------------------------------------------------------------------------------------------------

const PRELUDE_TEMPLATE: &str = include_str!("prelude.js");

/// `prelude.js` with its limits filled in (upstream interpolates them into a template literal,
/// `prelude-source.ts:202-206`).
static PRELUDE: LazyLock<String> = LazyLock::new(|| {
    let expects = serde_json::to_string(IMAGE_HELPER_EXPECTS).unwrap_or_default();
    PRELUDE_TEMPLATE
        .replace(
            "__MAX_STORE_VALUE_CHARS__",
            &MAX_STORE_VALUE_CHARS.to_string(),
        )
        .replace(
            "__MAX_STORE_TOTAL_CHARS__",
            &MAX_STORE_TOTAL_CHARS.to_string(),
        )
        .replace("__MAX_OUTPUT_CHARS__", &MAX_OUTPUT_CHARS.to_string())
        .replace("__MAX_OUTPUT_ITEMS__", &MAX_OUTPUT_ITEMS.to_string())
        .replace("__IMAGE_HELPER_EXPECTS__", &expects)
});

/// The prelude source, for tests that check it parses and carries the limits.
#[cfg(test)]
pub(super) fn prelude_source() -> &'static str {
    &PRELUDE
}

/// Whether V8 compiles `source` as a script (upstream's `new vm.Script(PRELUDE_SOURCE)` check).
#[cfg(test)]
pub(super) fn parses_as_javascript(source: &str) -> Result<(), String> {
    let mut js = {
        let _lifecycle = isolate_lock();
        JsRuntime::new(RuntimeOptions::default())
    };
    let outcome = js
        .execute_script("prelude.js", source.to_owned())
        .map(|_| ())
        .map_err(|error| error.to_string());
    let _lifecycle = isolate_lock();
    drop(js);
    outcome
}

// ------------------------------------------------------------------------------------------------
// The thread
// ------------------------------------------------------------------------------------------------

/// V8 mutates process-global state when an isolate is created and when it is disposed, and
/// `deno_core` guards neither off Windows. Creation and disposal of every codemode isolate take
/// this lock; execution does not, so executions run in parallel.
fn isolate_lock() -> MutexGuard<'static, ()> {
    static ISOLATE_LIFECYCLE: Mutex<()> = Mutex::new(());
    // The lock protects an engine-internal critical section, not a Rust value: a poisoned lock
    // holds no inconsistent data.
    ISOLATE_LIFECYCLE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// The thread body. Reports an engine failure to the supervisor as a [`WorkerMessage::Crash`].
pub(super) fn run_thread(
    init: IsolateInit,
    to_host: UnboundedSender<WorkerMessage>,
    from_host: UnboundedReceiver<HostReply>,
    kill: Arc<KillSwitch>,
    guard: RunningGuard,
) {
    // Dropped when the thread ends, after the isolate is disposed.
    let _guard = guard;
    if let Err(message) = run(init, &to_host, from_host, &kill) {
        let _ = to_host.send(WorkerMessage::Crash(message));
    }
}

fn run(
    init: IsolateInit,
    to_host: &UnboundedSender<WorkerMessage>,
    from_host: UnboundedReceiver<HostReply>,
    kill: &KillSwitch,
) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Failed to start the isolate runtime: {error}"))?;
    runtime.block_on(async move {
        let out_of_memory = Arc::new(AtomicBool::new(false));
        let create_params = init
            .memory_limit
            .map(|max| v8::CreateParams::default().heap_limits(0, max));
        let mut js = {
            let _lifecycle = isolate_lock();
            JsRuntime::new(RuntimeOptions {
                extensions: vec![cyrup_codemode_sandbox::init()],
                create_params,
                ..RuntimeOptions::default()
            })
        };
        let outcome = drive(&mut js, &init, to_host, from_host, kill, &out_of_memory).await;
        {
            // Disposal goes through the same lock as creation.
            let _lifecycle = isolate_lock();
            drop(js);
        }
        outcome
    })
}

/// Engine-level refusals that hold whatever the script manages to reach: `WebAssembly` cannot
/// compile even if a script found the global the prelude removes.
fn harden(js: &mut JsRuntime) {
    js.v8_isolate()
        .set_allow_wasm_code_generation_callback(deny_wasm_codegen);
}

/// A hardened isolate without the prelude, to check what the engine refuses by itself.
#[cfg(test)]
pub(super) fn evaluate_hardened(source: &str) -> Result<String, String> {
    let mut js = {
        let _lifecycle = isolate_lock();
        JsRuntime::new(RuntimeOptions::default())
    };
    harden(&mut js);
    let outcome = js
        .execute_script("hardened.js", source.to_owned())
        .map_err(|error| error.to_string())
        .map(|value| {
            deno_core::scope!(scope, js);
            v8::Local::new(scope, value).to_rust_string_lossy(scope)
        });
    let _lifecycle = isolate_lock();
    drop(js);
    outcome
}

/// `extern "C"` because V8 calls it through its C ABI; a pure predicate.
extern "C" fn deny_wasm_codegen(
    _context: v8::Local<'_, v8::Context>,
    _source: v8::Local<'_, v8::String>,
) -> bool {
    false
}

async fn drive(
    js: &mut JsRuntime,
    init: &IsolateInit,
    to_host: &UnboundedSender<WorkerMessage>,
    mut from_host: UnboundedReceiver<HostReply>,
    kill: &KillSwitch,
    out_of_memory: &Arc<AtomicBool>,
) -> Result<(), String> {
    let handle = js.v8_isolate().thread_safe_handle();
    harden(js);
    {
        let flag = Arc::clone(out_of_memory);
        let stopper = handle.clone();
        js.add_near_heap_limit_callback(move |current, _initial| {
            flag.store(true, Ordering::SeqCst);
            stopper.terminate_execution();
            current.saturating_add(OUT_OF_MEMORY_HEADROOM_BYTES)
        });
    }
    // A kill that arrived while the isolate was being built stops the thread here.
    if !kill.arm(handle) {
        return Ok(());
    }

    let external_limit = init
        .memory_limit
        .unwrap_or(DEFAULT_EXTERNAL_MEMORY_LIMIT_BYTES);
    let stride =
        (external_limit / 16).clamp(MIN_MEMORY_CHECK_STRIDE_BYTES, MAX_MEMORY_CHECK_STRIDE_BYTES);
    let settled = Rc::new(Cell::new(false));
    js.op_state().borrow_mut().put(Bridge {
        to_host: to_host.clone(),
        settled: Rc::clone(&settled),
        external_limit,
    });

    let tools = js_string(&init.tools_json);
    let globals = js_string(&init.globals_json);
    let store = js_string(&init.store_json);
    let api = js
        .execute_script(
            "codemode-prelude.js",
            format!("{}({tools}, {globals}, {store}, {stride})", *PRELUDE),
        )
        .map_err(|error| format!("Failed to set up the sandbox: {error}"))?;
    let api = into_object(js, &api).ok_or("The sandbox prelude did not return its API")?;

    // The prefix shares the first line with the script, so reported line numbers match the
    // script as written (`worker.ts:142-146`).
    let function = match js.execute_script(
        "codemode.js",
        format!("(async (tools, console) => {{{}\n}})", init.code),
    ) {
        Ok(function) => function,
        Err(error) => {
            finish_failed_start(js, &error, out_of_memory, to_host);
            return Ok(());
        }
    };

    let mut step = call_method(js, &api, "run", &[Arg::Value(&function)]);
    loop {
        if matches!(step, Step::Terminated) || out_of_memory.load(Ordering::SeqCst) {
            return finish_terminated(js, out_of_memory, to_host);
        }
        if let Step::Failed(message) = step {
            return Err(message);
        }
        match pump(js).await {
            Pump::Idle => {}
            Pump::Terminated => return finish_terminated(js, out_of_memory, to_host),
            Pump::Failed(message) => return Err(message),
        }
        if settled.get() {
            return Ok(());
        }
        // A script that waits while no host call is pending can never resume: nothing in the
        // isolate is a timer or I/O (`prelude.js` `stalled()`).
        match call_method(js, &api, "stalled", &[]) {
            Step::Continue => {}
            Step::Terminated => return finish_terminated(js, out_of_memory, to_host),
            Step::Failed(message) => return Err(message),
        }
        if settled.get() {
            return Ok(());
        }
        let Some(reply) = from_host.recv().await else {
            // The supervisor settled the execution and dropped its end.
            return Ok(());
        };
        step = match reply.payload {
            ReplyPayload::Value(Some(json)) => call_method(
                js,
                &api,
                "settle",
                &[Arg::Int(reply.id), Arg::Bool(true), Arg::Str(&json)],
            ),
            ReplyPayload::Value(None) => call_method(
                js,
                &api,
                "settle",
                &[Arg::Int(reply.id), Arg::Bool(true), Arg::Undefined],
            ),
            ReplyPayload::Failure(message) => call_method(
                js,
                &api,
                "settle",
                &[Arg::Int(reply.id), Arg::Bool(false), Arg::Str(&message)],
            ),
        };
    }
}

/// The isolate was stopped, either by the supervisor (which has already settled the execution and
/// needs nothing more) or by the heap limit, which this thread must report.
fn finish_terminated(
    js: &mut JsRuntime,
    out_of_memory: &AtomicBool,
    to_host: &UnboundedSender<WorkerMessage>,
) -> Result<(), String> {
    if out_of_memory.load(Ordering::SeqCst) {
        let _ = to_host.send(WorkerMessage::Done(ScriptSettled::Threw(
            out_of_memory_json(),
        )));
    }
    js.v8_isolate().cancel_terminate_execution();
    Ok(())
}

/// The script failed to compile (a `SyntaxError`), or the isolate was stopped while compiling.
fn finish_failed_start(
    js: &mut JsRuntime,
    error: &JsError,
    out_of_memory: &AtomicBool,
    to_host: &UnboundedSender<WorkerMessage>,
) {
    if out_of_memory.load(Ordering::SeqCst) || js.v8_isolate().is_execution_terminating() {
        let _ = finish_terminated(js, out_of_memory, to_host);
        return;
    }
    let _ = to_host.send(WorkerMessage::Done(ScriptSettled::Threw(
        compile_error_json(error),
    )));
}

/// `worker.ts:46-50` `describeException`: V8 reports a syntax error with a location but no stack,
/// so the stack is the head and that location as one frame, in the shape of a thrown error's.
fn compile_error_json(error: &JsError) -> String {
    let name = error
        .name
        .clone()
        .unwrap_or_else(|| String::from("SyntaxError"));
    let message = error
        .message
        .clone()
        .unwrap_or_else(|| error.exception_message.clone());
    let head = if message.is_empty() {
        name.clone()
    } else {
        format!("{name}: {message}")
    };
    let stack = match error.frames.first() {
        Some(frame) => format!(
            "{head}\n    at {}:{}:{}",
            frame.file_name.as_deref().unwrap_or("codemode.js"),
            frame.line_number.unwrap_or(0),
            frame.column_number.unwrap_or(0),
        ),
        None => head,
    };
    serde_json::json!({ "name": name, "message": message, "stack": stack }).to_string()
}

// ------------------------------------------------------------------------------------------------
// Driving V8
// ------------------------------------------------------------------------------------------------

/// What happened to a call into the prelude's API.
enum Step {
    Continue,
    /// `terminate_execution` stopped it.
    Terminated,
    /// The call threw, which the prelude never does: an engine fault.
    Failed(String),
}

enum Pump {
    Idle,
    Terminated,
    Failed(String),
}

/// Runs queued jobs until the isolate has nothing left to do. An error from the event loop is a
/// rejection the engine reports, such as an unawaited failed call; it does not end the script, so
/// the loop is polled again.
async fn pump(js: &mut JsRuntime) -> Pump {
    let mut errors = 0_usize;
    loop {
        match js.run_event_loop(PollEventLoopOptions::default()).await {
            Ok(()) => return Pump::Idle,
            Err(error) => {
                if js.v8_isolate().is_execution_terminating() {
                    return Pump::Terminated;
                }
                errors += 1;
                if errors > MAX_EVENT_LOOP_ERRORS {
                    return Pump::Failed(format!("The isolate's event loop failed: {error}"));
                }
            }
        }
    }
}

enum Arg<'a> {
    Undefined,
    Bool(bool),
    Int(u32),
    Str(&'a str),
    Value(&'a v8::Global<v8::Value>),
}

fn into_object(
    js: &mut JsRuntime,
    value: &v8::Global<v8::Value>,
) -> Option<v8::Global<v8::Object>> {
    deno_core::scope!(scope, js);
    let local = v8::Local::new(scope, value);
    let object = v8::Local::<v8::Object>::try_from(local).ok()?;
    Some(v8::Global::new(scope, object))
}

/// `api.<method>(...args)`.
fn call_method(
    js: &mut JsRuntime,
    api: &v8::Global<v8::Object>,
    method: &str,
    args: &[Arg<'_>],
) -> Step {
    deno_core::scope!(scope, js);
    v8::tc_scope!(let scope, scope);
    let api = v8::Local::new(scope, api);
    let Some(key) = v8::String::new(scope, method) else {
        return Step::Failed(format!("The sandbox API name {method:?} is not a string"));
    };
    let Some(function) = api
        .get(scope, key.into())
        .and_then(|value| v8::Local::<v8::Function>::try_from(value).ok())
    else {
        return Step::Failed(format!("The sandbox API has no {method}()"));
    };
    let mut locals: Vec<v8::Local<'_, v8::Value>> = Vec::with_capacity(args.len());
    for arg in args {
        let local: Option<v8::Local<'_, v8::Value>> = match arg {
            Arg::Undefined => Some(v8::undefined(scope).into()),
            Arg::Bool(flag) => Some(v8::Boolean::new(scope, *flag).into()),
            Arg::Int(number) => Some(v8::Number::new(scope, f64::from(*number)).into()),
            Arg::Str(text) => v8::String::new(scope, text).map(Into::into),
            Arg::Value(value) => Some(v8::Local::new(scope, *value)),
        };
        let Some(local) = local else {
            return Step::Failed(String::from(
                "A value was too large to pass into the isolate",
            ));
        };
        locals.push(local);
    }
    match function.call(scope, api.into(), &locals) {
        Some(_) => Step::Continue,
        None if scope.has_terminated() || scope.is_execution_terminating() => Step::Terminated,
        None => Step::Failed(format!("The sandbox API threw in {method}()")),
    }
}

fn js_string(text: &str) -> String {
    // A JSON string literal is a JavaScript string literal.
    serde_json::to_string(text).unwrap_or_else(|_| String::from("\"\""))
}
