//! Immediate-bash seam (Pi `executeBash`/`recordBashResult`/`abortBash`/`isBashRunning`/
//! `hasPendingBashMessages`/`_flushPendingBashMessages`, agent-session.ts:2582-2684). The out-of-loop
//! bash RPC path: a command runs against the session's process backend (NOT the agent loop's `bash`
//! tool), its result is recorded as a `bashExecution` custom message, and — when a run is streaming —
//! deferred into a pending queue flushed after the turn so tool_use/tool_result ordering is intact.

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_tools::{ExecSpec, ExitStatus, ProcOps, ShellConfig};

/// The outcome of an immediate bash execution (Pi `BashResult`, bash-executor.ts).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BashResult {
    /// Combined stdout+stderr.
    pub output: String,
    /// Process exit code (`None` when killed/signaled without a code).
    pub exit_code: Option<i32>,
    /// Whether the command was cancelled via [`crate::AgentSession::abort_bash`].
    pub cancelled: bool,
}

/// A streaming sink for combined bash output chunks (Pi `onChunk`, agent-session.ts:2589).
pub type BashChunkSink = Option<Box<dyn FnMut(&str) + Send>>;

/// Options for [`crate::AgentSession::execute_bash`] (Pi `executeBash` options, agent-session.ts:2588).
#[derive(Clone, Debug, Default)]
pub struct BashOptions {
    /// `!!` prefix: keep the output out of the LLM context (still recorded for history).
    pub exclude_from_context: bool,
}

/// Run `command` against `proc` in `cwd`, streaming combined output to `on_chunk`, honoring `cancel`
/// (Pi `executeBashWithOperations`). The default local backend kills the whole tree on cancel.
pub(crate) async fn run_bash(
    proc: &Arc<dyn ProcOps>,
    shell: &ShellConfig,
    cwd: PathBuf,
    command: String,
    cancel: cyrup_core::CancelToken,
    mut on_chunk: BashChunkSink,
) -> BashResult {
    let spec = ExecSpec { command, cwd, env: Vec::new(), shell: shell.clone() };
    let mut buf: Vec<u8> = Vec::new();
    let status = proc
        .exec(spec, cancel, None, &mut |data: &[u8]| {
            buf.extend_from_slice(data);
            if let Some(cb) = on_chunk.as_mut() {
                cb(&String::from_utf8_lossy(data));
            }
        })
        .await;
    let output = String::from_utf8_lossy(&buf).into_owned();
    match status {
        Ok(ExitStatus::Exited(code)) => BashResult { output, exit_code: Some(code), cancelled: false },
        Ok(ExitStatus::Signaled) => BashResult { output, exit_code: None, cancelled: false },
        Ok(ExitStatus::Killed) => BashResult { output, exit_code: None, cancelled: true },
        Ok(ExitStatus::TimedOut) => BashResult { output, exit_code: None, cancelled: false },
        // A backend failure surfaces as cancelled-with-message so the caller still records history.
        Err(e) => BashResult { output: format!("{output}{e}"), exit_code: None, cancelled: false },
    }
}

/// Build the `bashExecution` custom-message payload Pi records (agent-session.ts:2628-2640).
pub(crate) fn bash_message_payload(command: &str, result: &BashResult, exclude_from_context: bool) -> serde_json::Value {
    serde_json::json!({
        "command": command,
        "output": result.output,
        "exitCode": result.exit_code,
        "cancelled": result.cancelled,
        "excludeFromContext": exclude_from_context,
    })
}
