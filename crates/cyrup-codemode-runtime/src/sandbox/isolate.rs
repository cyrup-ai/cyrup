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
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use cyrup_codemode::types::OutputItem;
use deno_core::error::JsError;
use deno_core::{JsRuntime, OpState, PollEventLoopOptions, RuntimeOptions, op2, v8};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::child;
use super::lifecycle::{KillSwitch, RunningGuard};
use super::protocol::{
    CallTarget, HostReply, IsolateInit, ReplyPayload, ScriptSettled, WorkerMessage,
    out_of_memory_json,
};
use super::{
    ARGUMENT_COMMA_WEIGHT, ARGUMENT_CONTAINER_WEIGHT, IMAGE_HELPER_EXPECTS, MAX_IMAGE_BYTES,
    MAX_JSON_DEPTH, MAX_OUTPUT_CHARS, MAX_OUTPUT_ITEMS, MAX_PENDING_ARGUMENT_WEIGHT,
    MAX_PENDING_CALLS, MAX_STORE_TOTAL_CHARS, MAX_STORE_VALUE_CHARS, MAX_UNOBSERVED_CHARS,
    MAX_UNOBSERVED_SHOWN, MAX_UNOBSERVED_TRACKED,
};

/// How far the heap limit is raised when the script reaches it. The limit must be raised, never
/// left: V8's own response to an unhandled heap limit is `abort()`, which would take the host
/// process down. The headroom only has to last until the termination request is processed.
const OUT_OF_MEMORY_HEADROOM_BYTES: usize = 16 * 1024 * 1024;

/// The limit on memory behind `ArrayBuffer`s and typed arrays when the sandbox sets none. The
/// engine's heap limit does not count it, so without a number a script could grow the host
/// process until the operating system stopped it.
const DEFAULT_EXTERNAL_MEMORY_LIMIT_BYTES: usize = 1024 * 1024 * 1024;

/// The heap limit a sandbox process is sized for when the sandbox sets none: V8's own default
/// (the largest it picks on a 64-bit machine).
const DEFAULT_HEAP_LIMIT_BYTES: usize = 4 * 1024 * 1024 * 1024;

/// The heap and the `ArrayBuffer` budget (with the headroom the heap limit callback grants) that
/// an isolate under `memory_limit` can use; the sandbox process sizes its address-space ceiling
/// from them.
pub(super) fn memory_budgets(memory_limit: Option<usize>) -> (usize, usize) {
    let heap = memory_limit
        .unwrap_or(DEFAULT_HEAP_LIMIT_BYTES)
        .saturating_add(OUT_OF_MEMORY_HEADROOM_BYTES);
    let external = memory_limit.unwrap_or(DEFAULT_EXTERNAL_MEMORY_LIMIT_BYTES);
    (heap, external)
}

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
    /// The end of a script that ran to its end, until it is reported.
    held: Arc<Held>,
}

/// [CYRUP-DELTA] How long the end of a script waits for the engine to look at what the script left
/// behind (`prelude.js` `release`) before the host is told without it. The wait is for the event loop
/// to turn once, which takes microseconds; the bound is for a script that returned and left a
/// continuation that never stops, which used to be left behind with the isolate at the return.
const RELEASE_GRACE: Duration = Duration::from_secs(1);

/// The end of a script that ran to its end, between `op_codemode_hold` and the report of it by
/// `op_codemode_release` or, if the event loop does not come back in time, by a timer thread.
#[derive(Default)]
struct Held {
    settlement: Mutex<Option<ScriptSettled>>,
    /// Dropped to stop the timer once the report is made.
    timer: Mutex<Option<mpsc::Sender<()>>>,
}

impl Held {
    fn settlement(&self) -> MutexGuard<'_, Option<ScriptSettled>> {
        // Two plain values; a poisoned lock holds no broken invariant.
        self.settlement
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn timer(&self) -> MutexGuard<'_, Option<mpsc::Sender<()>>> {
        self.timer.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Reports a held end after [`RELEASE_GRACE`] unless the returned sender is dropped first.
fn start_release_timer(
    held: Arc<Held>,
    to_host: UnboundedSender<WorkerMessage>,
) -> Option<mpsc::Sender<()>> {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name(String::from("codemode-release-grace"))
        .spawn(move || {
            if matches!(
                stopped.recv_timeout(RELEASE_GRACE),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) && let Some(settled) = held.settlement().take()
            {
                let _ = to_host.send(WorkerMessage::Done(settled));
            }
        })
        .ok()
        .map(|_| stop)
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

/// `bridge("done", true, valueJson, writesJson, reportJson)`.
#[op2(fast)]
fn op_codemode_done_ok(
    state: &mut OpState,
    #[string] value: &str,
    has_value: bool,
    #[string] writes: String,
    #[string] report: &str,
) {
    mark_settled(state);
    post(
        state,
        WorkerMessage::Done(ScriptSettled::Returned {
            value: has_value.then(|| value.to_owned()),
            writes,
            // [CYRUP-DELTA] Advisory, so a report that does not decode is an empty one.
            report: serde_json::from_str(report).unwrap_or_default(),
        }),
    );
}

/// `opHold(valueJson, hasValue, writesJson, reportJson)`: the script ran to its end; its report waits for
/// `op_codemode_release`.
#[op2(fast)]
fn op_codemode_hold(
    state: &mut OpState,
    #[string] value: &str,
    has_value: bool,
    #[string] writes: String,
    #[string] report: &str,
) {
    let Some(bridge) = state.try_borrow::<Bridge>() else {
        return;
    };
    *bridge.held.settlement() = Some(ScriptSettled::Returned {
        value: has_value.then(|| value.to_owned()),
        writes,
        report: serde_json::from_str(report).unwrap_or_default(),
    });
    let timer = start_release_timer(Arc::clone(&bridge.held), bridge.to_host.clone());
    *bridge.held.timer() = timer;
}

/// `opRelease(reportJson)`: the event loop has turned; report the held end with what it found.
#[op2(fast)]
fn op_codemode_release(state: &mut OpState, #[string] report: &str) {
    let Some(bridge) = state.try_borrow::<Bridge>() else {
        return;
    };
    drop(bridge.held.timer().take());
    let held = bridge.held.settlement().take();
    mark_settled(state);
    // Gone already when the timer reported it: the host has the end of the script.
    if let Some(ScriptSettled::Returned { value, writes, .. }) = held {
        post(
            state,
            WorkerMessage::Done(ScriptSettled::Returned {
                value,
                writes,
                report: serde_json::from_str(report).unwrap_or_default(),
            }),
        );
    }
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
///
/// Not a fast op: the collection runs inside the call, and V8's fast-call contract forbids a call
/// to allocate on the engine's heap or trigger a garbage collection. A regular call may.
#[op2(nofast)]
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

/// [CYRUP-DELTA] Whether anything reacts to `promise`: an `await`, a `.then`, a `.catch`, a
/// `Promise.all` that took it in. The engine records it on the promise when a reaction is attached
/// (V8 `Promise::HasHandler`), which a script cannot observe and `prelude.js` cannot tell from the
/// outside: `await p` on a native promise never calls `p.then`. `prelude.js` `unobservedReport` asks
/// it of the calls the script left unsettled, to tell a call the script abandoned from one it was
/// waiting for. Anything that is not a promise has no handler.
#[op2(fast)]
fn op_codemode_promise_handled<'s>(promise: v8::Local<'s, v8::Value>) -> bool {
    v8::Local::<v8::Promise>::try_from(promise).is_ok_and(|promise| promise.has_handler())
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
        op_codemode_hold,
        op_codemode_release,
        op_codemode_promise_handled,
        op_codemode_memory_exceeded,
    ],
    docs = "The only capability a codemode script reaches: its prelude holds these ops in a \
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
        .replace("__MAX_JSON_DEPTH__", &MAX_JSON_DEPTH.to_string())
        .replace("__MAX_PENDING_CALLS__", &MAX_PENDING_CALLS.to_string())
        .replace(
            "__MAX_PENDING_ARGUMENT_WEIGHT__",
            &MAX_PENDING_ARGUMENT_WEIGHT.to_string(),
        )
        .replace(
            "__ARGUMENT_COMMA_WEIGHT__",
            &ARGUMENT_COMMA_WEIGHT.to_string(),
        )
        .replace(
            "__ARGUMENT_CONTAINER_WEIGHT__",
            &ARGUMENT_CONTAINER_WEIGHT.to_string(),
        )
        .replace(
            "__MAX_UNOBSERVED_SHOWN__",
            &MAX_UNOBSERVED_SHOWN.to_string(),
        )
        .replace(
            "__MAX_UNOBSERVED_CHARS__",
            &MAX_UNOBSERVED_CHARS.to_string(),
        )
        .replace(
            "__MAX_UNOBSERVED_TRACKED__",
            &MAX_UNOBSERVED_TRACKED.to_string(),
        )
        .replace("__MAX_IMAGE_BYTES__", &MAX_IMAGE_BYTES.to_string())
        .replace("__IMAGE_HELPER_EXPECTS__", &expects)
});

/// The ops the prelude can call, for the test that checks how each is declared.
#[cfg(test)]
pub(super) fn declared_ops() -> Vec<deno_core::OpDecl> {
    cyrup_codemode_sandbox::init().init_ops().to_vec()
}

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

/// [CYRUP-DELTA] The time a script spends running, as opposed to waiting for the host. The isolate
/// thread is parked while it waits for the result of a tool call, and a script that waits on
/// `sleep infinity` has used none of its running time; a script that spins uses all of it.
struct ActiveClock {
    state: Mutex<ClockState>,
}

struct ClockState {
    /// Running time of the finished stretches.
    used: Duration,
    /// When the current stretch started; `None` while parked.
    since: Option<Instant>,
}

impl ActiveClock {
    fn new() -> Self {
        Self {
            state: Mutex::new(ClockState {
                used: Duration::ZERO,
                since: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ClockState> {
        // The state is two plain values; a poisoned lock holds no broken invariant.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The script runs from now on.
    fn resume(&self) {
        self.lock().since.get_or_insert_with(Instant::now);
    }

    /// The script waits from now on.
    fn park(&self) {
        let mut state = self.lock();
        if let Some(since) = state.since.take() {
            state.used = state.used.saturating_add(since.elapsed());
        }
    }

    fn used(&self) -> Duration {
        let state = self.lock();
        state
            .used
            .saturating_add(state.since.map_or(Duration::ZERO, |since| since.elapsed()))
    }
}

/// The longest a watchdog sleeps in one go.
const WATCHDOG_MAX_SLEEP: Duration = Duration::from_secs(3600);

/// Ends the watchdog thread when dropped.
struct Watchdog {
    _stop: mpsc::Sender<()>,
}

/// Stops the isolate once `clock` shows `limit` of running time, and tells the supervisor why. The
/// earliest the limit can be reached is `limit - used` from now (running time cannot pass faster
/// than the clock), so the thread sleeps exactly that long instead of polling.
fn spawn_watchdog(
    clock: Arc<ActiveClock>,
    limit: Duration,
    handle: v8::IsolateHandle,
    to_host: UnboundedSender<WorkerMessage>,
) -> Option<Watchdog> {
    let (stop, stopped) = mpsc::channel::<()>();
    std::thread::Builder::new()
        .name(String::from("codemode-watchdog"))
        .spawn(move || {
            loop {
                let used = clock.used();
                if used >= limit {
                    let _ = to_host.send(WorkerMessage::ActiveLimit);
                    handle.terminate_execution();
                    return;
                }
                let nap = limit.saturating_sub(used).min(WATCHDOG_MAX_SLEEP);
                // The sender is dropped when the isolate is done: `Disconnected` ends the thread.
                if !matches!(
                    stopped.recv_timeout(nap),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ) {
                    return;
                }
            }
        })
        .ok()
        .map(|_| Watchdog { _stop: stop })
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
    if child::reports_fatal_out_of_memory() {
        js.v8_isolate()
            .set_oom_error_handler(child::on_fatal_out_of_memory);
    }
    {
        let flag = Arc::clone(out_of_memory);
        let stopper = handle.clone();
        js.add_near_heap_limit_callback(move |current, _initial| {
            flag.store(true, Ordering::SeqCst);
            stopper.terminate_execution();
            current.saturating_add(OUT_OF_MEMORY_HEADROOM_BYTES)
        });
    }
    let clock = Arc::new(ActiveClock::new());
    let _watchdog = init.active_limit_ms.and_then(|ms| {
        spawn_watchdog(
            Arc::clone(&clock),
            Duration::from_millis(ms),
            handle.clone(),
            to_host.clone(),
        )
    });
    // A kill that arrived while the isolate was being built stops the thread here.
    if !kill.arm(handle) {
        return Ok(());
    }
    clock.resume();

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
        held: Arc::new(Held::default()),
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

    let function = match compile_script(js, &init.code) {
        Ok(function) => function,
        Err(CompileFailure::Terminated) => {
            let _ = finish_terminated(js, out_of_memory, to_host);
            return Ok(());
        }
        Err(CompileFailure::Error(error)) => {
            finish_failed_start(js, &error, out_of_memory, to_host);
            return Ok(());
        }
        Err(CompileFailure::TooLarge) => {
            let error = serde_json::json!({
                "name": "RangeError",
                "message": "The script is too large to compile",
            });
            let _ = to_host.send(WorkerMessage::Done(ScriptSettled::Threw(error.to_string())));
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
        // [CYRUP-DELTA] A script that ran to its end is reported now and not as it returned: the
        // engine tells the prelude of a rejection nobody handled, or of one handled late, when the
        // event loop runs, which it has just done (`prelude.js` `unobservedReport`).
        match call_method(js, &api, "release", &[]) {
            Step::Continue => {}
            Step::Terminated => return finish_terminated(js, out_of_memory, to_host),
            Step::Failed(message) => return Err(message),
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
        clock.park();
        let Some(reply) = from_host.recv().await else {
            // The supervisor settled the execution and dropped its end.
            return Ok(());
        };
        clock.resume();
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

/// What the script's source is wrapped in. Upstream's wrapper declares `tools` and `console` as
/// parameters (`worker.ts:142-146`), which makes `const tools = await searchTools("x")` a
/// `SyntaxError: Identifier 'tools' has already been declared`. [CYRUP-DELTA] Here they are the
/// globals the prelude defines (the same objects), so a script may declare either name itself.
const SCRIPT_PREFIX: &str = "(async () => {";

/// The script did not become a function.
enum CompileFailure {
    /// The isolate was stopped while compiling.
    Terminated,
    /// A `SyntaxError`.
    Error(Box<JsError>),
    /// The engine cannot hold the source as a string.
    TooLarge,
}

/// Compiles `(async () => {code\n})` as the script `codemode.js` and returns the function.
///
/// The prefix shares the first line with the script, so reported line numbers match the script as
/// written (`worker.ts:142-146`). [CYRUP-DELTA] Upstream's columns on line 1 are offset by the
/// prefix, and `engine.execute_script` offers no way around it; here the script's origin starts at
/// a negative column, so a frame `codemode.js:1:5` is the fifth column of the model's first line.
fn compile_script(js: &mut JsRuntime, code: &str) -> Result<v8::Global<v8::Value>, CompileFailure> {
    deno_core::scope!(scope, js);
    let (Some(name), Some(source)) = (
        v8::String::new(scope, "codemode.js"),
        v8::String::new(scope, &format!("{SCRIPT_PREFIX}{code}\n}})")),
    ) else {
        return Err(CompileFailure::TooLarge);
    };
    let column_offset = -i32::try_from(SCRIPT_PREFIX.len()).unwrap_or(0);
    let origin = v8::ScriptOrigin::new(
        scope,
        name.into(),
        0,
        column_offset,
        false,
        0,
        None,
        false,
        false,
        false,
        None,
    );
    v8::tc_scope!(let scope, scope);
    let value =
        v8::Script::compile(scope, source, Some(&origin)).and_then(|script| script.run(scope));
    match value {
        Some(value) => Ok(v8::Global::new(scope, value)),
        None => match scope.exception() {
            Some(exception) if !scope.has_terminated() => Err(CompileFailure::Error(
                JsError::from_v8_exception(scope, exception),
            )),
            _ => Err(CompileFailure::Terminated),
        },
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
/// failure the engine reports; it does not end the script, so the loop is polled again. A rejection
/// nobody handled is not one of them any more: the prelude takes those over (`unobservedReport`) and
/// reports them with the script's result.
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
