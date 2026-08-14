//! PRINT mode — one-shot human-oriented text output (func-11 R-11-005/009; arch-11 §2.2).
//!
//! Drives a whole turn — the initial prompt plus every follow-up message — to completion over the
//! [`AgentSession`] seam, then writes ONLY the final assistant message to the caller-supplied
//! [`std::io::Write`] sinks. This mirrors Pi's `runPrintMode` (`modes/print-mode.ts:121-146`), whose
//! send loop produces no output and whose single terminal output block reads
//! `state.messages[state.messages.length - 1]` — the final message — exactly once, *outside* the
//! loop. On a failed/aborted final turn Pi writes the error to stderr and suppresses the assistant
//! stdout entirely; this adapter does the same. The sinks and the input are parameters — the binary
//! wires real stdout/stderr/stdin later.

use std::io::Write;

use cyrup_core::{Content, Message, StopReason};
use cyrup_session_svc::{AgentSessionEvent, AgentSessionRuntime, BindOptions, UserInput};
use futures::StreamExt;

use crate::error::ModesError;

/// Knobs for [`run_print`].
#[derive(Clone, Copy, Debug, Default)]
pub struct PrintOptions {
    /// Narrate `[tool] <name>` lines as tools begin executing (human debugging aid).
    pub show_tools: bool,
}

/// Run a whole turn to completion and write ONLY the final assistant message (Pi `runPrintMode`,
/// print-mode.ts:121-146).
///
/// `messages` is the ordered turn: the initial submission first, then each CLI follow-up (Pi's
/// `initialMessage` followed by `messages[]`). Every message is prompted to completion in order and
/// the send loop itself is silent — with [`PrintOptions::show_tools`] each tool start is narrated to
/// `out` as it streams (an additive cyrup aid Pi lacks; off by default).
///
/// After every message has settled, the *final* transcript message is emitted once (Pi's terminal
/// `if (mode === "text")` block, print-mode.ts:129-146):
/// - if it is an assistant message whose `stop_reason` is [`StopReason::Error`] or
///   [`StopReason::Aborted`], its `error_message` (or `"Request <reason>"` when absent/empty) is
///   written to `err` and NOTHING is written to `out` — the failed turn's partial content is
///   suppressed (print-mode.ts:133-137);
/// - otherwise each `text` content block is written to `out` as its own `${text}\n` line
///   (print-mode.ts:138-144).
///
/// The process exit code is derived separately by the caller from the same terminal stop reason
/// (arch-11 §6.6), so this returns `()` on success.
///
/// ## Why this takes the RUNTIME, not a bare session (SEAM-006)
/// Pi's entry point is `runPrintMode(runtimeHost: AgentSessionRuntime, options)`
/// (print-mode.ts:32) — it has no bare-session host at all. Two things follow, both of which cyrup
/// lost by binding print/json to a standalone `AgentSession`:
/// 1. A loaded extension's `ctx.newSession()`/`ctx.fork()`/`ctx.switchSession()`/`ctx.reload()`
///    needs a runtime to act on. Without one, `AgentSession` answers
///    `SessionServiceError::NoRuntimeHost` and the failure is only `tracing::warn!`-ed. This matters
///    well beyond `cyrup -p`: the print/json arm is what a spawned subagent child re-execs into.
/// 2. When a replacement DOES happen, the host must re-read the active session — Pi's
///    `rebindSession` does exactly `session = runtimeHost.session` (print-mode.ts:72). That is why
///    the send loop below re-acquires the session per message instead of hoisting it.
pub async fn run_print<W, E>(
    runtime: &AgentSessionRuntime,
    messages: impl IntoIterator<Item = UserInput>,
    out: &mut W,
    err: &mut E,
    opts: PrintOptions,
) -> Result<i32, ModesError>
where
    W: Write,
    E: Write,
{
    // Pi's `await rebindSession()` at print-mode.ts:119 — the FIRST statement of the mode's `try`
    // block and strictly ahead of the send loop at :121. `rebindSession` ends in
    // `session.bindExtensions(...)` (:73), whose tail emits `_sessionStartEvent`
    // (agent-session.ts:2250). SEAM-033: the announcement belongs HERE and not in the runtime
    // constructor, because `main.ts` applies `--name` (:650) and `--models` (:742-750) between
    // building the session and running the mode; announcing at construction time would show every
    // `session_start` handler an unconfigured session. Idempotent per session
    // (`AgentSession::emit_session_start` latches on `start_announced`), so a host that already
    // announced via `AgentSessionRuntime::create` is unaffected.
    // SEAM-006: pi's `bindExtensions({ mode, commandContextActions, onError })` third key —
    //   `onError: (err) => { console.error(`Extension error (${err.extensionPath}): ${err.error}`); }`
    // (print-mode.ts:98-100 @v0.83.0, `:101-103` @v0.84.1). Without it an extension that faults
    // under `cyrup -p` was contained and NEVER surfaced: nothing on stderr, nothing in the event
    // stream, so a broken extension looked like a silently degraded run — and this arm is what a
    // spawned subagent child re-execs into, so every subagent run inherited it.
    runtime
        .session()
        .await
        .bind_extensions_with(BindOptions { on_error: Some(extension_error_sink()) })
        .await;

    // Send loop (Pi print-mode.ts:121-127): prompt each message to completion, in order, producing
    // no assistant output. Each run stream terminates at `agent_end`; `wait_for_idle` then confirms
    // the agent is settled before the next prompt is submitted.
    for input in messages {
        // Pi's `rebindSession` (print-mode.ts:71-72) — re-read the runtime's active session, so a
        // message submitted after an extension replaced it addresses the NEW session.
        let session = runtime.session().await;
        let mut stream = session.prompt(input).await?;
        while let Some(ev) = stream.next().await {
            if opts.show_tools
                && let AgentSessionEvent::ToolExecutionStart { tool_name, .. } = &ev
            {
                writeln!(out, "[tool] {tool_name}")?;
            }
        }
        session.wait_for_idle().await;
    }

    // Terminal output block (Pi print-mode.ts:129-146): read the FINAL transcript message once,
    // outside the loop — from the session that is active NOW (Pi's `const state = session.state`
    // reads the rebound `session`, print-mode.ts:130). Only an assistant final message produces
    // output.
    //
    // SEAM-016 — the exit code is decided HERE, from that SAME `lastMessage`, because that is the
    // only place pi decides it: `exitCode` is initialised to 0 (`print-mode.ts:35`) and is mutated
    // by exactly one statement, `exitCode = 1` inside the `error`/`aborted` arm (`:147`); every
    // other terminal state — including a last message that is not an assistant message at all —
    // keeps the 0. It used to be recomputed in `run.rs` by REVERSE-SCANNING the transcript for the
    // most recent assistant message, which disagrees with this block whenever the final message is
    // something else (`flush_pending_bash_messages` appends `Custom` bash messages after the
    // assistant), and mapped `aborted` to 130 and `pending` to 1 where pi emits 1 and 0.
    let mut exit_code = 0;
    let transcript = runtime.session().await.messages().await;
    if let Some(Message::Assistant(assistant)) = transcript.last() {
        match assistant.stop_reason {
            // A failed/aborted turn: the error goes to stderr and the assistant stdout is suppressed
            // (Pi `console.error(errorMessage || `Request ${stopReason}`)`, print-mode.ts:133-137).
            StopReason::Error | StopReason::Aborted => {
                let reason = match assistant.stop_reason {
                    StopReason::Aborted => "aborted",
                    _ => "error",
                };
                let fallback = format!("Request {reason}");
                let message = assistant
                    .error_message
                    .as_deref()
                    .filter(|m| !m.is_empty())
                    .unwrap_or(&fallback);
                // pi's error line is `console.error` (stderr, print-mode.ts:146), NOT
                // `writeRawStdout` — so it keeps the plain write. Only the stdout protocol path
                // below carries the retry (TOOL-037).
                writeln!(err, "{message}")?;
                // Pi `exitCode = 1` (print-mode.ts:147) — the mode's ONLY assignment.
                exit_code = 1;
            }
            // A clean turn: one line per text content block (Pi print-mode.ts:138-144).
            _ => {
                for content in &assistant.content {
                    if let Content::Text { text, .. } = content {
                        // TOOL-037 — pi: `writeRawStdout(`${content.text}\n`)`
                        // (print-mode.ts:141 @v0.84.1), retry loop included.
                        crate::raw_stdout::write_raw_stdout(out, &format!("{text}\n")).await?;
                    }
                }
            }
        }
    }
    crate::raw_stdout::flush_raw_stdout(out).await?;
    err.flush()?;
    Ok(exit_code)
}

/// pi's `onError` listener for the print/json hosts (`print-mode.ts:98-100` @v0.83.0):
///
/// ```text
/// onError: (err) => {
///     console.error(`Extension error (${err.extensionPath}): ${err.error}`);
/// },
/// ```
///
/// `console.error` is process stderr, NOT the mode's injected `err` writer — pi's sink is the
/// console regardless of which writer the mode prints its transcript through, and the listener
/// outlives the borrow of that writer anyway (`ErrorListener` is `Arc<dyn Fn + Send + Sync>`).
///
/// CYRUP-DELTA — pi interpolates `err.extensionPath`; cyrup's [`cyrup_ext::ExtensionError`] carries
/// the extension **id** (`extension: ExtensionId`, `cyrup-ext/src/dispatch.rs:27-32`) and no path,
/// because a native built-in has no path to name. The id is what identifies the extension on every
/// other cyrup diagnostic surface, so it is what goes in the parentheses.
pub(crate) fn extension_error_sink() -> cyrup_ext::ErrorListener {
    std::sync::Arc::new(|err: &cyrup_ext::ExtensionError| {
        eprintln!("Extension error ({}): {}", err.extension, err.error);
    })
}
