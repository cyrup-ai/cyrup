//! The codemode sandbox: a script in a fresh V8 isolate (pi `packages/codemode/src/runtime/*`
//! @v1.0.1, CODE-002; ADR-0031 fixes the engine to `deno_core`).
//!
//! | here | upstream |
//! |---|---|
//! | [`CodemodeSandbox`] | `host.ts:285-361` `CodemodeSandbox` |
//! | `execution` | `host.ts:63-274` `Execution` (the supervisor) |
//! | `isolate` | `worker.ts` (the worker thread) |
//! | `prelude.js` | `prelude-source.ts` `PRELUDE_SOURCE` (JavaScript, ported near-verbatim) |
//! | `protocol` | `protocol.ts` and the JSON helpers of `host.ts` |
//! | `names` | `host.ts:22-34`, `:295-314` (global-name validation) |
//! | `process`, `child`, `wire` | none: the isolate in a process of its own (see "Where the isolate lives") |
//!
//! # Shape
//!
//! One `execute` is one isolate on its own OS thread, as upstream's is one worker and one VM: a
//! runaway script, including one that only spins the microtask queue, is stopped by
//! `terminate_execution` and cannot poison a later run. The tool callbacks never run on that
//! thread. They are `BoxFuture`s that may need the caller's runtime, so they are polled on the
//! runtime `execute` was awaited on and their results cross a channel, as upstream's cross a
//! message port. The script's tool calls, its output and its end cross the other way.
//!
//! # Where the isolate lives
//!
//! An isolate on a thread of the host is not enough, and this was measured rather than assumed. V8
//! answers one allocation the heap cannot satisfy with a fatal out-of-memory that aborts the
//! process; `new Array(2 ** 27).fill(0)` under the tool's 256 MiB heap limit killed the whole `cyrup`
//! process with SIGTRAP after eight seconds, and the near-heap-limit callback can only raise the
//! limit by a margin, never save that allocation. Guarding every primitive that allocates cannot be
//! made complete either: typed-array copies resolved their constructor through
//! `SpeciesConstructor` to the engine's intrinsic and walked past the guards on the constructors
//! (943 MB resident, no error, under the same 256 MiB limit).
//!
//! So the production placement ([`Isolation::Process`], used by the tool's
//! [`IsolatedSandboxFactory`](crate::tool::IsolatedSandboxFactory)) runs each execution's isolate
//! in a process of its own. `process` is the supervisor's end, `child` the process
//! ([`run_sandbox_process`], which the host binary calls when started with [`HOST_SUBCOMMAND`]) and
//! `wire` the framing of the pipe between them. The messages are the ones the isolate thread
//! already exchanged with the supervisor ([`protocol`]), so [`execution`] cannot tell the two
//! placements apart:
//!
//! * a deadline, a cancel or `close()` kills the process, which also stops a native built-in that
//!   ignores `terminate_execution` (an isolate thread cannot be killed);
//! * the process starts with an empty environment, sets no core file and an `RLIMIT_DATA` ceiling
//!   (the heap limit, the `ArrayBuffer` budget and 768 MiB for the engine, the binary and thread
//!   stacks; an idle process measured 325 MB of writable memory, 78 MB resident, on a debug build),
//!   and is sized by the script's memory limit;
//! * the engine's out-of-memory handler reports the abort over the pipe, so the script fails with
//!   the same `InternalError: out of memory` as a heap limit that V8 handled itself, with the
//!   output printed before it; a process that dies without a word is a `sandbox` failure that
//!   names the signal or status;
//! * the supervisor reads frames of at most 512 MiB, so a process that lies about a length cannot
//!   make the host allocate without bound.
//!
//! The prelude's guards (the `ArrayBuffer` budget, now also on the species-resolved copies) stay:
//! they give the script a catchable error well before the ceiling, and they are the only defence of
//! the in-process placement ([`Isolation::InProcess`]), which remains for tests and embedders that
//! cannot re-execute themselves. Its residual is exactly V8's: a single allocation larger than the
//! heap can satisfy aborts the host. The one platform residual of the process placement is the
//! ceiling: `RLIMIT_DATA` is not enforced on macOS and does not exist on Windows, so there the
//! process is still disposable but its memory is bounded only by the heap limit and the prelude.
//!
//! # What the script sees
//!
//! `tools` (by identifier and by raw name), `ALL_TOOLS`, `console.*`, `text()`, `image()`,
//! `exit()`, `store()`/`load()`, the configured globals, and ECMAScript's own built-ins: nothing
//! else. The engine's host handle (`Deno`, `__bootstrap`), `WebAssembly`, `SharedArrayBuffer`,
//! `Atomics.wait`/`waitAsync`, `Intl` and `queueMicrotask` are removed (`prelude.js`, end of the
//! function), `WebAssembly` compilation is also refused by the engine, and there is no module
//! loader, so dynamic `import()` rejects. Timers, `fetch`, `process`, `require` never existed.
//! `tests::runtime::the_global_object_holds_ecmascript_and_the_sandbox_and_nothing_else` lists the
//! whole global object.
//!
//! # Differences a script can observe, against upstream's QuickJS
//!
//! * The engine is V8: the messages of a `SyntaxError` and of engine-raised errors read as V8's,
//!   and the language is V8's (for example `Temporal`, `Float16Array` and the `Iterator` helpers
//!   exist). `Intl` does not, as in QuickJS.
//! * Running out of the V8 heap (`memory_limit_bytes`) is not catchable. QuickJS throws
//!   `InternalError: out of memory` into the script, which can `try`/`catch` it; V8 cannot
//!   recover from a heap limit, so the isolate is terminated and the execution fails with the
//!   same `kind: script`, `name` and `message` an uncaught `InternalError` produces there. Only
//!   the stack differs: it is the head line alone, since no script frame is left after the
//!   engine unwinds. Memory behind `ArrayBuffer`s and typed arrays is outside the V8 heap and is
//!   counted by the prelude against the same number; there the error *is* catchable, exactly as
//!   in QuickJS.
//! * A `memory_limit_bytes` below [`MIN_MEMORY_LIMIT_BYTES`] is raised to it: V8 aborts the whole
//!   process when its own bootstrap does not fit the limit. Upstream's tiny limits make the
//!   script fail instead. With no limit set, memory behind `ArrayBuffer`s is capped at 1 GiB.
//! * [CYRUP-DELTA] A returned value reaches the host as the JSON text the script wrote and stays
//!   text ([`ReturnValue`](crate::types::ReturnValue)): the host prints it without reading it into a
//!   parsed value, which `serde_json` holds at tens of times the size of the text for data made of
//!   many small objects. It has no depth bound either, as upstream's `JSON.parse` has none;
//!   `serde_json` stopped a parsed value at 128 levels, and a returned value used to fail past it.
//!   `store()` values and tool arguments are still parsed, and are bounded at [`MAX_JSON_DEPTH`].
//! * An error's `stack` is V8's: frames read `at codemode.js:2:7`, and an out-of-memory error
//!   has none. [CYRUP-DELTA] Columns are those of the script as written, also on its first line:
//!   upstream's wrapper `(async (tools, console) => {` shifts them by its length, here the script's
//!   origin starts at a negative column that cancels the wrapper (`isolate::compile_script`).
//! * [CYRUP-DELTA] The wrapper is `(async () => {SCRIPT\n})` with no parameters; `tools` and
//!   `console` are the globals the prelude defines. Upstream passes them as parameters, so a
//!   script's own `const tools = ...` is a `SyntaxError` there (`Identifier 'tools' has already
//!   been declared`) and legal here. `text`, `image`, `store`, `load` and `exit` are globals in both:
//!   `const text = ...` shadows them in the script that declares it.
//! * [CYRUP-DELTA] A rejected tool call's error carries the script frame that made the call
//!   (`prelude.js` `atCallSite`); upstream's has no script frame, since it is made from the host's
//!   reply.
//! * [CYRUP-DELTA] `console` has `dir`, `group`/`groupCollapsed`/`groupEnd`, `assert`,
//!   `count`/`countReset`, `time`/`timeLog`/`timeEnd` and `table` besides upstream's five levels,
//!   and `text()`, `console.*` and the return value render an `Error` (its stack), a `Map` (its
//!   entries), a `Set` (its values) and a `BigInt` (decimal digits) where `JSON.stringify` gave `{}`
//!   or threw. Values that `JSON.stringify` renders itself are untouched.
//! * [CYRUP-DELTA] Limits upstream's sandbox does not have: a script's own running time
//!   ([`SandboxOptions::active_limit`](crate::types::SandboxOptions::active_limit); the `codemode`
//!   tool sets it and a wall deadline unless `timeout_ms` says otherwise), a depth of
//!   [`MAX_JSON_DEPTH`] for `store()` values and tool arguments (a `RangeError` in the script rather
//!   than a broken bridge), [`MAX_PENDING_CALLS`] tool calls started and not yet settled and
//!   [`MAX_PENDING_ARGUMENT_WEIGHT`] for what their arguments weigh (the call past either throws a
//!   `RangeError`), and [`MAX_IMAGE_BYTES`] for each `image()`.
//! * [CYRUP-DELTA] An error the script never looked at is reported with its result
//!   ([`CodemodeResult::Completed::unobserved`](crate::types::CodemodeResult)): a promise that
//!   rejected with no handler by the time the script ended (a tool call or an `async` function that
//!   was not awaited), and a tool call that failed after the script had stopped listening. A call
//!   that failed after the script ended and that something was waiting on (a `Promise.all` that had
//!   already rejected, a `.catch()`, an `await` in an `async` function nobody awaited) is reported
//!   apart (`UnobservedErrors::late_total`): the engine cannot tell whether the script was done
//!   with it. Upstream ends such a script as a plain success, with the error nowhere (`prelude.js`
//!   `unobservedReport`, `execution::unobserved_errors`).
//! * Built-ins are frozen, as upstream's `lockdown()` does: a polyfill such as
//!   `Array.prototype.chunk = ...` is silently ignored (the script is sloppy-mode) and
//!   `Object.prototype.x = 1` too.
//! * Everything that crosses to the host is JSON, as upstream: tool arguments, tool results,
//!   `store()` values and the return value. A `Date` arrives as its ISO string, `undefined` object
//!   members are dropped, a function or `Symbol` has no JSON form, and a `Map` or `Set` is
//!   a plain array (see above). There are no timers, so no `sleep`: the only thing a script can
//!   wait on is a tool call.
//! * [CYRUP-DELTA] A string with a lone surrogate (a `slice` that cut an emoji) arrives at the host
//!   with U+FFFD in its place, in tool arguments, the return value, `store()` values and keys and
//!   thrown errors alike (`prelude.js` `wellFormedJson`). Upstream's `JSON.parse` reads the escape
//!   `JSON.stringify` writes for it; `serde_json` refuses it, which failed the call, the return
//!   value or the whole execution.

mod child;
mod execution;
mod isolate;
mod lifecycle;
mod names;
mod process;
mod protocol;
mod wire;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cyrup_core::CancelToken;

pub use child::run_sandbox_process;
pub use names::SandboxConfigError;
pub use process::{HOST_SUBCOMMAND, HostCommand};
pub use protocol::MIN_MEMORY_LIMIT_BYTES;

use self::execution::{Hub, Plan};
use self::lifecycle::Running;
use crate::types::{
    CodemodeResult, CodemodeTool, Deadline, ExecuteOptions, SandboxClosed, SandboxOptions,
    ScriptSandbox,
};

/// Largest JSON text one `store()` value may have (`prelude-source.ts:28`).
pub const MAX_STORE_VALUE_CHARS: usize = 256 * 1024;
/// Largest total of keys and JSON texts a script's store may hold (`prelude-source.ts:29`).
pub const MAX_STORE_TOTAL_CHARS: usize = 1024 * 1024;
/// Output one script may produce with `text()`, `image()` and `console.*`, in characters of text
/// and base64 image data (`prelude-source.ts:36`). The host keeps all output until the script
/// ends, so without a limit a script that prints in a loop would grow the host's memory until it
/// crashed.
pub const MAX_OUTPUT_CHARS: usize = 16 * 1024 * 1024;
/// Output items one script may produce (`prelude-source.ts:37`); covers loops that print empty
/// strings.
pub const MAX_OUTPUT_ITEMS: usize = 100_000;

/// [CYRUP-DELTA] Deepest nesting of a `store()` value or of tool arguments, in levels of arrays and
/// objects. The host reads their JSON with `serde_json`, which stops at 128 levels, and a value
/// deeper than that used to end the execution as "Sandbox bridge broken" after the script's tool
/// calls had run; the prelude refuses it with a `RangeError` the script can catch. Lower than 128 so
/// that the entries the session wraps a store value in still read back.
pub const MAX_JSON_DEPTH: usize = 64;
/// [CYRUP-DELTA] Tool calls a script may have started and not seen settle. The host keeps each one
/// (its arguments and a task) until it ends, so a loop that starts calls without awaiting them
/// used to grow the host for as long as the script ran; the call past this limit throws a
/// `RangeError` in the script instead. Far above any script that awaits its calls, which the host
/// runs [`MAX_CONCURRENT_NESTED_CALLS`](crate::tool::execute::MAX_CONCURRENT_NESTED_CALLS) at a time.
pub const MAX_PENDING_CALLS: usize = 10_000;
/// [CYRUP-DELTA] What the arguments of a script's unsettled tool calls may weigh together.
/// [`MAX_PENDING_CALLS`] counts calls, not their size: a call's arguments cross to the host as
/// text, are read into a value there, and stay until the call settles (several copies for each of
/// the few that run), so 5000 calls with 1 MiB arguments took the host past a gigabyte and the
/// sandbox process down. The call that would pass this throws a `RangeError` in the script, as the
/// call past [`MAX_PENDING_CALLS`] does. A call whose arguments alone pass it can never be made.
///
/// A character of JSON weighs one, so this is 32 Mi characters of strings (what one call of
/// 30 million characters cost the host: about 200 MB, on a debug build). Data made of many small
/// values weighs more, by [`ARGUMENT_COMMA_WEIGHT`] and [`ARGUMENT_CONTAINER_WEIGHT`].
pub const MAX_PENDING_ARGUMENT_WEIGHT: usize = 32 * 1024 * 1024;
/// [CYRUP-DELTA] What each comma in the JSON of a call's arguments adds to its weight (see
/// [`MAX_PENDING_ARGUMENT_WEIGHT`]): an array element or an object member is a value of its own to
/// the host. Measured on a debug build, one call with 2 million integers took the host 654 MB (330
/// bytes for each, which weighs 55 here) and one with a million short strings 447 MB (450 bytes for
/// each, 58 here), so a limit's worth of either is 200 to 260 MB.
pub const ARGUMENT_COMMA_WEIGHT: usize = 48;
/// [CYRUP-DELTA] What each array and object in the JSON of a call's arguments adds to its weight
/// (see [`MAX_PENDING_ARGUMENT_WEIGHT`]). A small object costs the host a map besides its members: one
/// call with 250 thousand `{ a: n }` took it 589 MB (2.4 KB each, 316 here) and two with a million
/// each 3.2 GB, so a limit's worth is 170 to 250 MB.
pub const ARGUMENT_CONTAINER_WEIGHT: usize = 256;
/// [CYRUP-DELTA] Largest image `image()` accepts, decoded. Providers reject an image over about
/// 5 MB, and an image block that was saved to the conversation is sent again with every later turn.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// [CYRUP-DELTA] Errors the result of a script that succeeded names, at most: the ones it never
/// handled (see the module documentation). The count of the others is reported with them.
pub const MAX_UNOBSERVED_SHOWN: usize = 5;
/// [CYRUP-DELTA] Characters of one such error in the result; a tool's error can be a whole page of
/// output.
pub const MAX_UNOBSERVED_CHARS: usize = 300;
/// [CYRUP-DELTA] Unhandled rejections the prelude keeps to tell whether a handler arrives later. Past
/// this they are counted and not kept, so a loop that makes a million rejected promises holds none
/// of them in memory.
pub const MAX_UNOBSERVED_TRACKED: usize = 64;

/// The message of every refusal by `image()` (`prelude-source.ts:39-40`).
pub(crate) const IMAGE_HELPER_EXPECTS: &str = "image expects a non-empty image URL string, an object with image_url, or a raw MCP image block";

/// How long [`CodemodeSandbox::close`] waits for the isolate threads of a sandbox that runs in
/// this process. A thread inside a native built-in does not look at `terminate_execution`; after
/// this the thread is left behind and `close()` returns.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Where a script's isolate lives.
#[derive(Clone, Debug, Default)]
pub enum Isolation {
    /// On a thread of this process. A script that makes the engine abort (one allocation the heap
    /// cannot satisfy) takes the whole process with it; the memory guards in the prelude make that
    /// hard but cannot rule it out. For tests and embedders that accept this.
    #[default]
    InProcess,
    /// In a process of its own, started from the [`HostCommand`], which runs
    /// [`run_sandbox_process`]. Whatever the script does to its process, the host sees a failed
    /// script.
    Process(HostCommand),
}

/// Runs JavaScript in a V8 isolate. See the [module documentation](self).
///
/// Each `execute()` gets its own isolate, on its own thread or in its own process (see
/// [`Isolation`]); the sandbox only holds the tool table and the defaults. `close()` aborts
/// in-flight executions.
pub struct CodemodeSandbox {
    /// In registration order; names are unique.
    tools: Mutex<Vec<CodemodeTool>>,
    /// Fixed at construction, in order; names are unique and valid.
    globals: Vec<CodemodeTool>,
    deadline: Deadline,
    active_limit: Option<Duration>,
    memory_limit_bytes: Option<u64>,
    isolation: Isolation,
    close_timeout: Duration,
    closed: AtomicBool,
    hub: Hub,
}

impl std::fmt::Debug for CodemodeSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodemodeSandbox")
            .field("tools", &self.tools())
            .field("globals", &self.globals)
            .field("deadline", &self.deadline)
            .field("active_limit", &self.active_limit)
            .field("memory_limit_bytes", &self.memory_limit_bytes)
            .field("isolation", &self.isolation)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl CodemodeSandbox {
    /// `host.ts:295-314`. Refuses a duplicate tool name and an invalid, duplicate or conflicting
    /// global name, with upstream's messages.
    pub fn new(options: SandboxOptions) -> Result<Self, SandboxConfigError> {
        Self::with_isolation(options, Isolation::InProcess)
    }

    /// [`CodemodeSandbox::new`] with the isolates placed as `isolation` says.
    pub fn with_isolation(
        options: SandboxOptions,
        isolation: Isolation,
    ) -> Result<Self, SandboxConfigError> {
        let sandbox = Self {
            tools: Mutex::new(Vec::new()),
            globals: options.globals,
            deadline: options.deadline,
            active_limit: options.active_limit,
            memory_limit_bytes: options.memory_limit_bytes,
            isolation,
            close_timeout: CLOSE_TIMEOUT,
            closed: AtomicBool::new(false),
            hub: Hub {
                close: CancelToken::new(),
                running: Arc::new(Running::default()),
            },
        };
        for tool in options.tools {
            sandbox.register_tool(tool)?;
        }
        names::validate_globals(&sandbox.globals)?;
        Ok(sandbox)
    }

    /// Adds a tool the next executions can call. Refuses a name that is already registered
    /// (`host.ts:317-320`).
    pub fn register_tool(&self, tool: CodemodeTool) -> Result<(), SandboxConfigError> {
        let mut tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        if tools
            .iter()
            .any(|existing| existing.declaration.name == tool.declaration.name)
        {
            return Err(SandboxConfigError::ToolAlreadyRegistered {
                name: tool.declaration.name,
            });
        }
        tools.push(tool);
        Ok(())
    }

    /// Whether a tool of that name was registered and is now removed. Executions already running
    /// keep the tools they started with.
    pub fn unregister_tool(&self, name: &str) -> bool {
        let mut tools = self.tools.lock().unwrap_or_else(PoisonError::into_inner);
        let before = tools.len();
        tools.retain(|tool| tool.declaration.name != name);
        tools.len() != before
    }

    /// The registered tools, in registration order.
    #[must_use]
    pub fn tools(&self) -> Vec<CodemodeTool> {
        self.tools
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The configured globals, in configuration order.
    #[must_use]
    pub fn globals(&self) -> Vec<CodemodeTool> {
        self.globals.clone()
    }

    /// `code` is an async function body: `return` and top-level `await` work. A script's failure
    /// is a [`CodemodeResult::Failed`], never an `Err`; `Err` is "closed" (`host.ts:339-354`).
    ///
    /// Dropping the returned future cancels the execution: the isolate is stopped and every
    /// tool callback's token fires.
    pub async fn execute(
        &self,
        code: &str,
        options: ExecuteOptions,
    ) -> Result<CodemodeResult, SandboxClosed> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(SandboxClosed);
        }
        let plan = Plan {
            isolation: self.isolation.clone(),
            code: code.to_owned(),
            tools: self.tools(),
            globals: self.globals.clone(),
            deadline: options.deadline.unwrap_or(self.deadline),
            active_limit: self.active_limit,
            memory_limit: self.memory_limit_bytes,
            store: options.store,
            cancel: options.cancel,
            cancel_reason: options.cancel_reason,
        };
        Ok(execution::execute(&self.hub, plan).await)
    }

    /// Aborts in-flight executions (they settle with `ErrorKind::Aborted` and the message
    /// `Sandbox closed`), refuses new ones, and returns once every execution's isolate thread has
    /// ended (`host.ts:357-360`).
    ///
    /// The wait is bounded: a thread that is stuck inside a native built-in cannot be stopped, and
    /// the caller must not hang with it. After five seconds the thread is detached, which leaves
    /// it running until the built-in returns, still counted as live until then. A sandbox
    /// process has no such thread: it is killed.
    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.hub.close.cancel();
        let _ = self.hub.running.wait_idle(self.close_timeout).await;
    }

    /// Executions and isolate threads still alive; zero once everything has wound down.
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        self.hub.running.live()
    }
}

#[async_trait::async_trait]
impl ScriptSandbox for CodemodeSandbox {
    async fn execute(
        &self,
        code: &str,
        options: ExecuteOptions,
    ) -> Result<CodemodeResult, SandboxClosed> {
        CodemodeSandbox::execute(self, code, options).await
    }

    async fn close(&self) {
        CodemodeSandbox::close(self).await;
    }
}
