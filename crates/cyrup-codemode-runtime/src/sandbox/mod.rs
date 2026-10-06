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
//! * A value the script returns nested deeper than 128 levels cannot be read by the host
//!   (`serde_json`'s recursion bound) and fails the execution as a `RangeError`; `JSON.parse` in
//!   upstream has no such bound.
//! * An error's `stack` is V8's: frames read `at codemode.js:2:7`, and an out-of-memory error
//!   has none.

mod execution;
mod isolate;
mod lifecycle;
mod names;
mod protocol;

#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use cyrup_core::CancelToken;

pub use names::SandboxConfigError;
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

/// The message of every refusal by `image()` (`prelude-source.ts:39-40`).
pub(crate) const IMAGE_HELPER_EXPECTS: &str = "image expects a non-empty image URL string, an object with image_url, or a raw MCP image block";

/// Runs JavaScript in a V8 isolate. See the [module documentation](self).
///
/// Each `execute()` gets its own isolate and thread; the sandbox only holds the tool table and
/// the defaults. `close()` aborts in-flight executions.
pub struct CodemodeSandbox {
    /// In registration order; names are unique.
    tools: Mutex<Vec<CodemodeTool>>,
    /// Fixed at construction, in order; names are unique and valid.
    globals: Vec<CodemodeTool>,
    deadline: Deadline,
    memory_limit_bytes: Option<u64>,
    closed: AtomicBool,
    hub: Hub,
}

impl std::fmt::Debug for CodemodeSandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodemodeSandbox")
            .field("tools", &self.tools())
            .field("globals", &self.globals)
            .field("deadline", &self.deadline)
            .field("memory_limit_bytes", &self.memory_limit_bytes)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

impl CodemodeSandbox {
    /// `host.ts:295-314`. Refuses a duplicate tool name and an invalid, duplicate or conflicting
    /// global name, with upstream's messages.
    pub fn new(options: SandboxOptions) -> Result<Self, SandboxConfigError> {
        let sandbox = Self {
            tools: Mutex::new(Vec::new()),
            globals: options.globals,
            deadline: options.deadline,
            memory_limit_bytes: options.memory_limit_bytes,
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
            code: code.to_owned(),
            tools: self.tools(),
            globals: self.globals.clone(),
            deadline: options.deadline.unwrap_or(self.deadline),
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
    pub async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.hub.close.cancel();
        self.hub.running.wait_idle().await;
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
