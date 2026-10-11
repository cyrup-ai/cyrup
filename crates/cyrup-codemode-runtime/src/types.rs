//! The sandbox-facing types (pi `packages/codemode/src/types.ts` @v1.0.1): what a host registers
//! with a sandbox, what one execution is asked to do, and what it settles to.
//!
//! The declaration-only half of `types.ts` (`CodemodeTool` minus `execute`) is
//! `cyrup_codemode::types::ToolDeclaration`; [`CodemodeTool`] here is that plus the callback.

use std::sync::Arc;
use std::time::Duration;

use cyrup_codemode::types::{OutputItem, ToolDeclaration};
use cyrup_core::CancelToken;
use futures::future::BoxFuture;
use serde_json::value::RawValue;
use serde_json::{Map, Value};

/// What a tool callback is handed besides its arguments (pi `CodemodeToolContext`, `types.ts:3-8`).
#[derive(Clone, Debug)]
pub struct CodemodeToolContext {
    /// Cancelled when the script finishes (including unawaited calls), the execution times out,
    /// the caller cancels, or the sandbox is closed.
    pub cancel: CancelToken,
}

/// A tool callback's settled value: `Ok(None)` is a JavaScript `undefined` result, `Err(message)`
/// surfaces in the script as an `Error` with that message (`types.ts:36-40`).
pub type ToolResult = Result<Option<Value>, String>;

/// The callback of a [`CodemodeTool`]. `args` is whatever the script passed, after a JSON round
/// trip (`None` is `undefined`); for a spread global it is a JSON array of all the call arguments.
pub type ToolCallback =
    Arc<dyn Fn(Option<Value>, CodemodeToolContext) -> BoxFuture<'static, ToolResult> + Send + Sync>;

/// A tool or global the script can call (pi `CodemodeTool`, `types.ts:14-41`).
#[derive(Clone)]
pub struct CodemodeTool {
    pub declaration: ToolDeclaration,
    pub execute: ToolCallback,
}

impl std::fmt::Debug for CodemodeTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodemodeTool")
            .field("declaration", &self.declaration)
            .finish_non_exhaustive()
    }
}

/// How one nested call ended (pi `CodemodeCallStatus`, `types.ts:55`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallStatus {
    Ok,
    Error,
    Cancelled,
}

/// One tool call a script made (pi `CodemodeCall`, `types.ts:57-61`). Globals are not recorded.
#[derive(Clone, Debug, PartialEq)]
pub struct CodemodeCall {
    pub name: String,
    pub status: CallStatus,
    pub duration_ms: u64,
}

/// Why an execution failed (pi `CodemodeErrorKind`, `types.ts:63-73`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// The script threw or failed to parse. `name` and `stack` come from the script's error.
    Script,
    /// The overall deadline expired. The isolate was terminated.
    Timeout,
    /// The caller cancelled or the sandbox was closed. The isolate was terminated.
    Aborted,
    /// The engine failed outside the script's control.
    Sandbox,
}

/// pi `CodemodeError` (`types.ts:75-80`).
#[derive(Clone, Debug, PartialEq)]
pub struct CodemodeError {
    pub kind: ErrorKind,
    pub name: Option<String>,
    pub message: String,
    pub stack: Option<String>,
}

/// Keys the script changed with `store()` (pi `CodemodeStoreWrites`, `types.ts:83-88`). Only
/// successful executions report writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CodemodeStoreWrites {
    pub set: Map<String, Value>,
    /// Keys stored as `undefined`.
    pub delete: Vec<String>,
}

/// The JSON text of the value a script returned, checked to be one JSON value.
///
/// [CYRUP-DELTA] Upstream parses the value (`JSON.parse`) and prints it again (`JSON.stringify`).
/// Here the host keeps the text the script's `JSON.stringify` made and never builds a [`Value`] tree
/// of it: `serde_json` holds data made of many small objects at tens of times the size of its text
/// (a returned array of 1.25 million `{ a: n }` objects, 16 MB of JSON, took the host about 0.7 GB),
/// for a value that is only shown as text. The text is what the script's `JSON.stringify` wrote, so
/// printing it is the same as printing the parsed value (a test compares the two over a corpus).
/// Checking the text does not recurse, so there is no depth bound either, as with upstream's
/// `JSON.parse`; `serde_json` stopped a tree at 128 levels.
#[derive(Clone, Debug)]
pub struct ReturnValue(Box<RawValue>);

impl ReturnValue {
    /// # Errors
    ///
    /// `json` is not exactly one JSON value.
    pub fn from_json(json: String) -> Result<Self, serde_json::Error> {
        RawValue::from_string(json).map(Self)
    }

    /// The JSON text, as the script wrote it.
    #[must_use]
    pub fn as_json(&self) -> &str {
        self.0.get()
    }

    /// The value as a tree, for callers that need to look inside it; it costs what this type
    /// exists to avoid.
    ///
    /// # Errors
    ///
    /// The text nests deeper than `serde_json` reads into a tree (128 levels).
    pub fn to_value(&self) -> Result<Value, serde_json::Error> {
        serde_json::from_str(self.as_json())
    }

    /// The value as the script's `text()` prints it: a string as it is, anything else as its JSON.
    #[must_use]
    pub fn into_text(self) -> String {
        let json: Box<str> = self.0.into();
        if json.starts_with('"')
            && let Ok(text) = serde_json::from_str::<String>(&json)
        {
            return text;
        }
        json.into_string()
    }
}

impl From<Value> for ReturnValue {
    fn from(value: Value) -> Self {
        // A `Value` always serializes; `null` is only the arm that keeps this total.
        Self(serde_json::value::to_raw_value(&value).unwrap_or_else(|_| RawValue::NULL.to_owned()))
    }
}

impl PartialEq for ReturnValue {
    fn eq(&self, other: &Self) -> bool {
        self.as_json() == other.as_json()
    }
}

impl Eq for ReturnValue {}

/// [CYRUP-DELTA] One error a script that succeeded never looked at.
#[derive(Clone, Debug, PartialEq)]
pub struct UnobservedError {
    /// The tool or global whose call failed, when the error is a call's. `None` for any other
    /// rejection (an `async` function that threw and was not awaited, `Promise.reject(...)`).
    pub call: Option<String>,
    /// The error as the script would have read it, on one line and cut to a bounded length.
    pub message: String,
}

/// [CYRUP-DELTA] The errors a script that succeeded never looked at. Upstream ends such a script as
/// a plain success: a `tools.write(...)` that was not awaited and failed, an `async` function that
/// threw and was not awaited, leave nothing in the result, so a model that forgot an `await` is
/// told its side effect happened. `total` counts them all; `shown` holds the first few.
///
/// `late_total` and `late_shown` are the calls that failed after the script had ended and that
/// something was waiting on when it did (an `await` in an `async` function nobody awaited, a
/// `.then()`, a `.catch()`, the siblings of a `Promise.all` that had already rejected). The engine
/// cannot tell whether the script was done with them, so they are kept apart from the errors it
/// certainly lost.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnobservedErrors {
    pub total: usize,
    pub shown: Vec<UnobservedError>,
    pub late_total: usize,
    pub late_shown: Vec<UnobservedError>,
}

/// What an execution settles to (pi `CodemodeResult`, `types.ts:91-100`). `output` is kept for
/// failed executions too, up to the failure; `exit()` completes with `value: None`.
#[derive(Clone, Debug, PartialEq)]
pub enum CodemodeResult {
    Completed {
        value: Option<ReturnValue>,
        output: Vec<OutputItem>,
        calls: Vec<CodemodeCall>,
        store_writes: CodemodeStoreWrites,
        /// See [`UnobservedErrors`].
        unobserved: UnobservedErrors,
    },
    Failed {
        error: CodemodeError,
        output: Vec<OutputItem>,
        calls: Vec<CodemodeCall>,
    },
}

/// The deadline of an execution (`timeoutMs`, `types.ts:106-110`; `Infinity` disables it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Deadline {
    After(Duration),
    Never,
}

/// pi `DEFAULT_TIMEOUT_MS` (`host.ts`, 300 000 ms).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(300_000);

/// [CYRUP-DELTA] How long the `codemode` tool lets a script run in all, tool calls included, when
/// its options line sets no `timeout_ms`. Upstream's tool sets no deadline at all
/// (`execute.ts:427`, `timeoutMs` undefined), so `await tools.bash({command: "sleep infinity"})`
/// waits until a user aborts, which in a headless run or a subagent is never. The number is
/// generous on purpose: an image generation or a long build is a legitimate wait.
pub const DEFAULT_TOOL_WALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// [CYRUP-DELTA] How much of its own time a script may use when its options line sets no
/// `timeout_ms`: time the isolate spends running script code, not time it waits for tool calls
/// (see [`SandboxOptions::active_limit`]). It is what stops `while (true) {}` early; waiting is
/// bounded by [`DEFAULT_TOOL_WALL_TIMEOUT`].
pub const DEFAULT_ACTIVE_LIMIT: Duration = Duration::from_secs(120);

/// pi `CodemodeSandboxOptions` (`types.ts:102-135`) without the wasm/worker members, which exist
/// only because upstream's engine is QuickJS-in-wasm in a worker thread.
#[derive(Clone, Debug)]
pub struct SandboxOptions {
    pub tools: Vec<CodemodeTool>,
    /// Functions exposed as top-level identifiers (or `<namespace>.<member>`) instead of on
    /// `tools`. They are not recorded in the result's `calls`.
    pub globals: Vec<CodemodeTool>,
    pub deadline: Deadline,
    /// [CYRUP-DELTA] The most time the isolate may spend running script code. A script that waits
    /// for a tool call does not run, so that time does not count; a script that spins does, so a
    /// busy loop ends here long before a generous [`deadline`](Self::deadline). `None` (the
    /// default, as upstream) has no such limit. Fails the execution as [`ErrorKind::Timeout`].
    pub active_limit: Option<Duration>,
    /// Maximum memory the isolate may allocate; `None` leaves V8's default.
    pub memory_limit_bytes: Option<u64>,
}

impl Default for SandboxOptions {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            globals: Vec::new(),
            deadline: Deadline::After(DEFAULT_TIMEOUT),
            active_limit: None,
            memory_limit_bytes: None,
        }
    }
}

/// pi `CodemodeExecuteOptions` (`types.ts:137-145`).
#[derive(Clone, Debug, Default)]
pub struct ExecuteOptions {
    pub cancel: Option<CancelToken>,
    /// The message of the [`ErrorKind::Aborted`] failure a `cancel` produces. A [`CancelToken`]
    /// carries no reason, so the caller supplies what upstream reads off `signal.reason`: the
    /// reason's message when it is an `Error` (an ordinary `AbortController.abort()` is a
    /// `DOMException` "This operation was aborted"), and `host.ts`'s constant `Execution aborted`
    /// otherwise. `None` is that non-`Error` branch.
    pub cancel_reason: Option<String>,
    /// Overrides the sandbox default for this execution.
    pub deadline: Option<Deadline>,
    /// Values the script reads with `load(key)`; the script's own `store()` calls come back as the
    /// result's `store_writes`, and persisting them is the caller's job.
    pub store: Map<String, Value>,
}

/// What the `codemode` tool needs from a sandbox (pi `CodemodeSandbox`, `host.ts:285-361`). The
/// engine-bound implementation is `sandbox::CodemodeSandbox`; the tool's tests use a fake.
#[async_trait::async_trait]
pub trait ScriptSandbox: Send + Sync {
    /// `code` is an async function body: `return` and top-level `await` work. Never fails for a
    /// script's failure; those come back as [`CodemodeResult::Failed`]. `Err` is only "closed".
    async fn execute(
        &self,
        code: &str,
        options: ExecuteOptions,
    ) -> Result<CodemodeResult, SandboxClosed>;

    /// Cancels in-flight executions (they settle as [`ErrorKind::Aborted`]) and refuses new ones.
    async fn close(&self);
}

/// `execute` on a closed sandbox (`host.ts:341`: `"Sandbox is closed"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Sandbox is closed")]
pub struct SandboxClosed;
