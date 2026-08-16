use super::*;

/// A streamed message from a running `!`/`!!` bash execution (`bash-execution.ts` output pump).
#[derive(Clone, Debug)]
pub(crate) enum BashMsg {
    /// A sanitized stdout/stderr delta (Pi's `onChunk`, `interactive-mode.ts:6338-6343`).
    Chunk(String),
    /// The run finished — the four `setComplete(exitCode, cancelled, truncationResult,
    /// fullOutputPath)` arguments (`bash-execution.ts:98-103`, fed from `BashResult` at
    /// `interactive-mode.ts:6348-6353`).
    Done {
        exit_code: Option<i32>,
        cancelled: bool,
        truncated: bool,
        full_output_path: Option<String>,
    },
}

/// Run a `!`/`!!` command through the session's own bash seam — Pi's `handleBashCommand`
/// (`interactive-mode.ts:6279-6364`), whose executor line is `await this.session.executeBash(command,
/// (chunk) => this.bashComponent.appendOutput(chunk), { excludeFromContext, operations })`
/// (`:6336-6345`) — streaming its deltas and terminal [`BashResult`] over the returned channel.
///
/// **X13.** This replaced a local `sh -c` pump that reported only an exit code, so
/// `truncated`/`fullOutputPath` were hard-coded `false, None` at the call to `setComplete` and the
/// `Output truncated. Full output: …` row (`bash-execution.ts:195-199`) could never appear in a live
/// session — only on the replay path, which reads the fields back off a persisted
/// `bashExecution` message. The seam that produces them already existed:
/// `AgentSession::execute_bash` → `run_bash` → `BashOutputBuffer` (`cyrup-session-svc/src/bash.rs`,
/// a port of `bash-executor.ts:57-124`), which spills to `cyrup-bash-<id>.log` once the raw stream
/// passes `DEFAULT_MAX_BYTES` and tail-truncates the preview. Nothing in the TUI reached it.
///
/// Routing through the session rather than spawning locally also picks up the rest of Pi's
/// `executeBash` contract that the local pump silently skipped: the `user_bash` extension event and
/// its `result` override ([`AgentSession::execute_bash_with_user_event`], Pi's per-front-end
/// `emitUserBash`, `interactive-mode.ts:6283-6288`), the `shellCommandPrefix`/`shellPath` settings,
/// the managed `bin` dir on `PATH`, ANSI/binary sanitization of every chunk, the
/// `bash_execution_update` event fan-out, and `recordBashResult` (`agent-session.ts:2628`) — which
/// is why the caller no longer appends its own `bashExecution` message.
///
/// Cancellation is the session's (`abortBash`, `agent-session.ts:2660`), not a token handed back
/// here: `execute_bash` installs its own child token and `AgentSession::abort_bash` fires it.
pub(crate) fn spawn_session_bash(
    session: Arc<AgentSession>,
    command: String,
    excluded: bool,
) -> tokio::sync::mpsc::UnboundedReceiver<BashMsg> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<BashMsg>();
    tokio::spawn(async move {
        let chunk_tx = tx.clone();
        // Pi's `(chunk) => { this.bashComponent.appendOutput(chunk); this.ui.requestRender(); }`
        // (`interactive-mode.ts:6338-6343`) — the redraw is the run loop's, so the sink only posts.
        let sink: cyrup_session_svc::BashChunkSink = Some(Box::new(move |delta: &str| {
            let _ = chunk_tx.send(BashMsg::Chunk(delta.to_string()));
        }));
        let options =
            cyrup_session_svc::BashOptions {
            exclude_from_context: excluded,
            id: None,
            operations: None,
        };
        let done = match session.execute_bash_with_user_event(&command, options, sink).await {
            Ok(result) => BashMsg::Done {
                exit_code: result.exit_code,
                cancelled: result.cancelled,
                truncated: result.truncated,
                full_output_path: result.full_output_path,
            },
            // A genuine backend failure (spawn error, missing shell, …). Pi's `catch`
            // (`interactive-mode.ts:6355-6360`) shows the message and calls
            // `setComplete(undefined, false)` — no exit code, not cancelled, no truncation report.
            Err(e) => {
                let _ = tx.send(BashMsg::Chunk(format!("Bash command failed: {e}\n")));
                BashMsg::Done {
                    exit_code: None,
                    cancelled: false,
                    truncated: false,
                    full_output_path: None,
                }
            }
        };
        let _ = tx.send(done);
    });
    rx
}
