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

/// What an execution settles to (pi `CodemodeResult`, `types.ts:91-100`). `output` is kept for
/// failed executions too, up to the failure; `exit()` completes with `value: None`.
#[derive(Clone, Debug, PartialEq)]
pub enum CodemodeResult {
    Completed {
        value: Option<Value>,
        output: Vec<OutputItem>,
        calls: Vec<CodemodeCall>,
        store_writes: CodemodeStoreWrites,
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

/// pi `CodemodeSandboxOptions` (`types.ts:102-135`) without the wasm/worker members, which exist
/// only because upstream's engine is QuickJS-in-wasm in a worker thread.
#[derive(Clone, Debug)]
pub struct SandboxOptions {
    pub tools: Vec<CodemodeTool>,
    /// Functions exposed as top-level identifiers (or `<namespace>.<member>`) instead of on
    /// `tools`. They are not recorded in the result's `calls`.
    pub globals: Vec<CodemodeTool>,
    pub deadline: Deadline,
    /// Maximum memory the isolate may allocate; `None` leaves V8's default.
    pub memory_limit_bytes: Option<u64>,
}

impl Default for SandboxOptions {
    fn default() -> Self {
        Self {
            tools: Vec::new(),
            globals: Vec::new(),
            deadline: Deadline::After(DEFAULT_TIMEOUT),
            memory_limit_bytes: None,
        }
    }
}

/// pi `CodemodeExecuteOptions` (`types.ts:137-145`).
#[derive(Clone, Debug, Default)]
pub struct ExecuteOptions {
    pub cancel: Option<CancelToken>,
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
    async fn execute(&self, code: &str, options: ExecuteOptions) -> Result<CodemodeResult, SandboxClosed>;

    /// Cancels in-flight executions (they settle as [`ErrorKind::Aborted`]) and refuses new ones.
    async fn close(&self);
}

/// `execute` on a closed sandbox (`host.ts:341`: `"Sandbox is closed"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("Sandbox is closed")]
pub struct SandboxClosed;
